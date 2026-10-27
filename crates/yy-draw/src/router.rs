//! Deterministic orthogonal routing with bounded A* fallback.
use crate::*;
use std::collections::{BinaryHeap, HashMap, HashSet};
pub struct Router<'a> {
    doc: &'a Document,
    index: Index,
}
/// An owned geometry snapshot; labels, icon assets and pages are never cloned.
pub struct Index {
    nodes: HashMap<Id, (Shape, Rect)>,
    obstacles: Vec<Rect>,
    tiles: HashMap<(i32, i32), Vec<usize>>,
    large: Vec<usize>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub points: Vec<Point>,
    pub resolved: bool,
}
impl<'a> Router<'a> {
    pub fn new(doc: &'a Document) -> Self {
        Self {
            doc,
            index: Index::new(doc),
        }
    }
    pub fn route(&self, c: &Connector) -> Route {
        self.index.route(c)
    }
    pub fn all(&self) -> Vec<Route> {
        self.doc.connectors.iter().map(|c| self.route(c)).collect()
    }
}
impl Index {
    pub fn new(doc: &Document) -> Self {
        let obstacles = doc
            .nodes
            .iter()
            .filter(|n| !matches!(n.shape, Shape::Text | Shape::Frame | Shape::Junction))
            .map(|n| n.rect.expand(8.))
            .collect::<Vec<_>>();
        let mut tiles: HashMap<_, Vec<_>> = HashMap::new();
        let mut large = vec![];
        for (i, r) in obstacles.iter().enumerate() {
            let (x0, y0, x1, y1) = tile_range(*r);
            if i64::from(x1 - x0 + 1) * i64::from(y1 - y0 + 1) > 1000 {
                large.push(i);
                continue;
            }
            for x in x0..=x1 {
                for y in y0..=y1 {
                    tiles.entry((x, y)).or_default().push(i);
                }
            }
        }
        Self {
            nodes: doc
                .nodes
                .iter()
                .map(|n| (n.id, (n.shape, n.rect)))
                .collect(),
            obstacles,
            tiles,
            large,
        }
    }
    fn endpoint(&self, e: Endpoint) -> Point {
        match e {
            Endpoint::Free(p) => p,
            Endpoint::Attached { node, side } => self
                .nodes
                .get(&node)
                .map(|(shape, rect)| ports::point_on(*shape, *rect, side))
                .unwrap_or_default(),
        }
    }
    fn nearby(&self, r: Rect) -> Vec<Rect> {
        let (x0, y0, x1, y1) = tile_range(r);
        if i64::from(x1 - x0 + 1) * i64::from(y1 - y0 + 1) > 5000 {
            return self
                .obstacles
                .iter()
                .copied()
                .filter(|o| overlap(*o, r))
                .collect();
        }
        let mut ids: HashSet<_> = self.large.iter().copied().collect();
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(v) = self.tiles.get(&(x, y)) {
                    ids.extend(v.iter().copied());
                }
            }
        }
        let mut ids = ids.into_iter().collect::<Vec<_>>();
        ids.sort_unstable();
        ids.into_iter()
            .map(|i| self.obstacles[i])
            .filter(|o| overlap(*o, r))
            .collect()
    }
    pub fn route(&self, c: &Connector) -> Route {
        let a = self.endpoint(c.start);
        let b = self.endpoint(c.end);
        if !c.orthogonal {
            let mut points = vec![a];
            points.extend(&c.waypoints);
            points.push(b);
            points.dedup_by(|a, b| a.distance(*b) < 1e-6);
            return Route {
                points,
                resolved: true,
            };
        }
        let stub = |e: Endpoint, p: Point| match e {
            Endpoint::Attached { node, side }
                if self
                    .nodes
                    .get(&node)
                    .is_some_and(|(shape, _)| *shape != Shape::Junction) =>
            {
                let (shape, rect) = self.nodes[&node];
                ports::exit_on(shape, rect, side)
            }
            _ => p,
        };
        let mut anchors = vec![stub(c.start, a)];
        anchors.extend(&c.waypoints);
        anchors.push(stub(c.end, b));
        let mut points = vec![a];
        let mut resolved = true;
        for pair in anchors.windows(2) {
            let (path, ok) = self.between(pair[0], pair[1]);
            points.extend(path);
            resolved &= ok;
        }
        points.push(b);
        simplify(&mut points);
        Route { points, resolved }
    }
    fn between(&self, a: Point, b: Point) -> (Vec<Point>, bool) {
        let bounds = Rect {
            x: a.x.min(b.x),
            y: a.y.min(b.y),
            w: (a.x - b.x).abs(),
            h: (a.y - b.y).abs(),
        };
        for margin in [32., 256., 2048.] {
            let area = bounds.expand(margin);
            let obstacles = self.nearby(area);
            let clear = |p: &[Point]| {
                p.iter().all(|p| area.contains(*p))
                    && p.windows(2)
                        .all(|s| !obstacles.iter().any(|r| blocked(s[0], s[1], *r)))
            };
            let mut candidates = vec![
                vec![a, Point::new(b.x, a.y), b],
                vec![a, Point::new(a.x, b.y), b],
            ];
            let mut xs = vec![(a.x + b.x) / 2.];
            let mut ys = vec![(a.y + b.y) / 2.];
            for r in &obstacles {
                xs.extend([r.x - 1., r.x + r.w + 1.]);
                ys.extend([r.y - 1., r.y + r.h + 1.]);
            }
            for x in &xs {
                candidates.push(vec![a, Point::new(*x, a.y), Point::new(*x, b.y), b]);
            }
            for y in &ys {
                candidates.push(vec![a, Point::new(a.x, *y), Point::new(b.x, *y), b]);
            }
            let mut best = None;
            let mut cost = f64::INFINITY;
            for mut p in candidates {
                simplify(&mut p);
                if clear(&p) {
                    let c = p.windows(2).map(|s| s[0].distance(s[1])).sum::<f64>()
                        + p.len() as f64 * 12.;
                    if c < cost {
                        cost = c;
                        best = Some(p);
                    }
                }
            }
            if let Some(p) = best {
                return (p, true);
            }
            if obstacles.len() <= 60 {
                xs.extend([a.x, b.x, bounds.x - margin, bounds.x + bounds.w + margin]);
                ys.extend([a.y, b.y, bounds.y - margin, bounds.y + bounds.h + margin]);
                xs.retain(|x| *x >= area.x && *x <= area.x + area.w);
                ys.retain(|y| *y >= area.y && *y <= area.y + area.h);
                if let Some(p) = search(a, b, xs, ys, &obstacles) {
                    return (p, true);
                }
            }
        }
        (vec![a, Point::new(b.x, a.y), b], false)
    }
}

/// Prepare once per frame, then route only changed connectors. Camera and style changes
/// leave the obstacle index and route geometry intact.
#[derive(Default)]
pub struct Cache {
    geometry: Vec<(Id, Shape, Rect)>,
    index: Option<Index>,
    routes: HashMap<Id, (Connector, Route)>,
}
impl Cache {
    pub fn prepare(&mut self, doc: &Document) {
        let unchanged = self.index.is_some()
            && self.geometry.len() == doc.nodes.len()
            && self
                .geometry
                .iter()
                .zip(&doc.nodes)
                .all(|((id, shape, rect), n)| *id == n.id && *shape == n.shape && *rect == n.rect);
        if unchanged {
            return;
        }
        self.geometry = doc.nodes.iter().map(|n| (n.id, n.shape, n.rect)).collect();
        self.index = Some(Index::new(doc));
        self.routes.clear();
    }
    pub fn route(&mut self, c: &Connector) -> &Route {
        let unchanged = self.routes.get(&c.id).is_some_and(|(old, _)| {
            old.start == c.start
                && old.end == c.end
                && old.orthogonal == c.orthogonal
                && old.waypoints == c.waypoints
        });
        if !unchanged {
            // A bounded working set for interactive previews, rather than a second full document.
            if self.routes.len() >= 256 {
                self.routes.clear();
            }
            let route = self
                .index
                .as_ref()
                .expect("prepare the routing cache before use")
                .route(c);
            self.routes.insert(c.id, (c.clone(), route));
        }
        &self.routes[&c.id].1
    }
}
fn tile_range(r: Rect) -> (i32, i32, i32, i32) {
    (
        (r.x / 256.).floor() as i32,
        (r.y / 256.).floor() as i32,
        ((r.x + r.w) / 256.).floor() as i32,
        ((r.y + r.h) / 256.).floor() as i32,
    )
}
pub fn overlap(a: Rect, b: Rect) -> bool {
    a.x <= b.x + b.w && a.x + a.w >= b.x && a.y <= b.y + b.h && a.y + a.h >= b.y
}
pub fn blocked(a: Point, b: Point, r: Rect) -> bool {
    if (a.x - b.x).abs() < 1e-6 {
        a.x > r.x + 1e-6
            && a.x < r.x + r.w - 1e-6
            && a.y.min(b.y) < r.y + r.h - 1e-6
            && a.y.max(b.y) > r.y + 1e-6
    } else {
        a.y > r.y + 1e-6
            && a.y < r.y + r.h - 1e-6
            && a.x.min(b.x) < r.x + r.w - 1e-6
            && a.x.max(b.x) > r.x + 1e-6
    }
}
fn simplify(p: &mut Vec<Point>) {
    p.dedup_by(|a, b| a.distance(*b) < 1e-6);
    let mut i = 1;
    while i + 1 < p.len() {
        let a = p[i - 1];
        let b = p[i];
        let c = p[i + 1];
        if ((a.x - b.x).abs() < 1e-6 && (b.x - c.x).abs() < 1e-6 && (b.y - a.y) * (c.y - b.y) >= 0.)
            || ((a.y - b.y).abs() < 1e-6
                && (b.y - c.y).abs() < 1e-6
                && (b.x - a.x) * (c.x - b.x) >= 0.)
        {
            p.remove(i);
        } else {
            i += 1;
        }
    }
}
fn search(
    a: Point,
    b: Point,
    mut xs: Vec<f64>,
    mut ys: Vec<f64>,
    obstacles: &[Rect],
) -> Option<Vec<Point>> {
    xs.sort_by(f64::total_cmp);
    xs.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    ys.sort_by(f64::total_cmp);
    ys.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    if xs.len() * ys.len() > 20000 {
        return None;
    }
    let w = xs.len();
    let index = |p: Point| {
        Some(
            ys.iter().position(|y| (*y - p.y).abs() < 1e-6)? * w
                + xs.iter().position(|x| (*x - p.x).abs() < 1e-6)?,
        )
    };
    let start = index(a)?;
    let goal = index(b)?;
    let point = |i: usize| Point::new(xs[i % w], ys[i / w]);
    let mut costs = vec![u64::MAX; w * ys.len() * 3];
    let mut prev = vec![usize::MAX; costs.len()];
    let mut heap = BinaryHeap::new();
    let first = start * 3 + 2;
    costs[first] = 0;
    heap.push(std::cmp::Reverse((0u64, 0u64, first)));
    let mut finish = None;
    while let Some(std::cmp::Reverse((_, cost, state))) = heap.pop() {
        if cost != costs[state] {
            continue;
        }
        let i = state / 3;
        if i == goal {
            finish = Some(state);
            break;
        }
        let x = i % w;
        let y = i / w;
        for (n, dir) in [
            (if x > 0 { Some(i - 1) } else { None }, 0),
            (if x + 1 < w { Some(i + 1) } else { None }, 0),
            (if y > 0 { Some(i - w) } else { None }, 1),
            (if y + 1 < ys.len() { Some(i + w) } else { None }, 1),
        ] {
            let Some(n) = n else {
                continue;
            };
            let p = point(i);
            let q = point(n);
            if obstacles.iter().any(|r| blocked(p, q, *r)) {
                continue;
            }
            let next = n * 3 + dir;
            let next_cost = cost
                + (p.distance(q) * 1024.).round() as u64
                + if state % 3 != dir && state % 3 != 2 {
                    12288
                } else {
                    0
                };
            if next_cost < costs[next] {
                costs[next] = next_cost;
                prev[next] = state;
                let heuristic = ((q.x - b.x).abs() + (q.y - b.y).abs()) * 1024.;
                heap.push(std::cmp::Reverse((
                    next_cost + heuristic.round() as u64,
                    next_cost,
                    next,
                )));
            }
        }
    }
    let mut at = finish?;
    let mut path = vec![];
    loop {
        path.push(point(at / 3));
        if at == first {
            break;
        }
        at = prev[at];
    }
    path.reverse();
    simplify(&mut path);
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_reuses_geometry_but_invalidates_ports_obstacles_and_connector_edits() {
        let mut d = Document::default();
        let a = d.add_node(Shape::Rectangle, Point::new(0., 0.), None);
        let b = d.add_node(Shape::Ellipse, Point::new(400., 160.), None);
        d.connect(
            Endpoint::Attached { node: a, side: 8 },
            Endpoint::Attached { node: b, side: 16 },
            true,
        );
        let mut cache = Cache::default();
        cache.prepare(&d);
        assert_eq!(
            *cache.route(&d.connectors[0]),
            Router::new(&d).route(&d.connectors[0])
        );
        d.nodes[0].label = "label does not change obstacles".into();
        d.connectors[0].color = 0x123456;
        d.connectors[0].corner_radius = 16.;
        cache.prepare(&d);
        assert_eq!(cache.routes.len(), 1);
        assert_eq!(
            *cache.route(&d.connectors[0]),
            Router::new(&d).route(&d.connectors[0])
        );
        for step in 0..5 {
            match step {
                0 => d.nodes[0].rect.x += 32.,
                1 => d.nodes[1].rect.h += 80.,
                2 => d.nodes[1].shape = Shape::Diamond,
                3 => {
                    d.add_node(Shape::Rectangle, Point::new(240., 0.), None);
                }
                _ => {
                    d.nodes.pop();
                }
            }
            cache.prepare(&d);
            assert!(cache.routes.is_empty());
            assert_eq!(
                *cache.route(&d.connectors[0]),
                Router::new(&d).route(&d.connectors[0])
            );
        }
        d.connectors[0].waypoints.push(Point::new(240., -80.));
        d.connectors[0].end = Endpoint::Attached { node: b, side: 19 };
        cache.prepare(&d);
        assert_eq!(
            *cache.route(&d.connectors[0]),
            Router::new(&d).route(&d.connectors[0])
        );
        let mut c = d.connectors[0].clone();
        for i in 0..600 {
            c.id = 1000 + i;
            cache.route(&c);
            assert!(cache.routes.len() <= 256);
        }
        d.nodes.clear();
        c.start = Endpoint::Free(Point::new(0., 0.));
        c.end = Endpoint::Free(Point::new(80., 80.));
        c.orthogonal = false;
        cache.prepare(&d);
        assert_eq!(*cache.route(&c), Router::new(&d).route(&c));
    }
}

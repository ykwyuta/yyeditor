use crate::*;
pub type Guide = (Point, Point);
/// Capture only the selected geometry once, then apply absolute drag deltas in O(selection).
pub struct MoveSelection {
    nodes: Vec<(usize, Id, Rect, bool)>,
    connectors: Vec<(usize, Connector)>,
}
impl MoveSelection {
    pub fn new(doc: &Document, ids: &[Id]) -> Self {
        let ids: HashSet<_> = ids.iter().copied().collect();
        let locked_groups: HashSet<_> = doc
            .nodes
            .iter()
            .filter(|n| n.locked)
            .filter_map(|n| n.group)
            .collect();
        Self {
            nodes: doc
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| ids.contains(&n.id))
                .map(|(i, n)| {
                    (
                        i,
                        n.id,
                        n.rect,
                        n.locked || n.group.is_some_and(|g| locked_groups.contains(&g)),
                    )
                })
                .collect(),
            connectors: doc
                .connectors
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    ids.contains(&c.id) && !c.group.is_some_and(|g| locked_groups.contains(&g))
                })
                .map(|(i, c)| (i, c.clone()))
                .collect(),
        }
    }
    pub fn apply(&self, doc: &mut Document, delta: Point) {
        for &(index, id, rect, locked) in &self.nodes {
            let index = if doc.nodes.get(index).is_some_and(|n| n.id == id) {
                Some(index)
            } else {
                doc.nodes.iter().position(|n| n.id == id)
            };
            if let Some(index) = index {
                let n = &mut doc.nodes[index];
                n.rect = rect;
                if !locked {
                    n.rect.x += delta.x;
                    n.rect.y += delta.y;
                }
            }
        }
        for (index, original) in &self.connectors {
            let index = if doc
                .connectors
                .get(*index)
                .is_some_and(|c| c.id == original.id)
            {
                Some(*index)
            } else {
                doc.connectors.iter().position(|c| c.id == original.id)
            };
            if let Some(index) = index {
                let c = &mut doc.connectors[index];
                c.clone_from(original);
                for e in [&mut c.start, &mut c.end] {
                    if let Endpoint::Free(p) = e {
                        p.x += delta.x;
                        p.y += delta.y;
                    }
                }
                for p in &mut c.waypoints {
                    p.x += delta.x;
                    p.y += delta.y;
                }
            }
        }
    }
}
pub fn align(doc: &Document, ids: &[Id], delta: Point, tolerance: f64) -> (Point, Vec<Guide>) {
    let ids: HashSet<_> = ids.iter().copied().collect();
    let selected = doc.nodes.iter().filter(|n| ids.contains(&n.id));
    let bounds = selected.map(|n| n.rect).reduce(Rect::union);
    let Some(mut bounds) = bounds else {
        return (delta, vec![]);
    };
    bounds.x += delta.x;
    bounds.y += delta.y;
    let xs = [bounds.x, bounds.x + bounds.w / 2., bounds.x + bounds.w];
    let ys = [bounds.y, bounds.y + bounds.h / 2., bounds.y + bounds.h];
    let mut dx = tolerance;
    let mut dy = tolerance;
    let mut gx = None;
    let mut gy = None;
    for n in doc.nodes.iter().filter(|n| !ids.contains(&n.id)) {
        let r = n.rect;
        for target in [r.x, r.x + r.w / 2., r.x + r.w] {
            for x in xs {
                let d = target - x;
                if d.abs() < dx.abs() {
                    dx = d;
                    gx = Some((
                        Point::new(target, r.y.min(bounds.y) - 12.),
                        Point::new(target, (r.y + r.h).max(bounds.y + bounds.h) + 12.),
                    ));
                }
            }
        }
        for target in [r.y, r.y + r.h / 2., r.y + r.h] {
            for y in ys {
                let d = target - y;
                if d.abs() < dy.abs() {
                    dy = d;
                    gy = Some((
                        Point::new(r.x.min(bounds.x) - 12., target),
                        Point::new((r.x + r.w).max(bounds.x + bounds.w) + 12., target),
                    ));
                }
            }
        }
    }
    (
        Point::new(
            delta.x + if gx.is_some() { dx } else { 0. },
            delta.y + if gy.is_some() { dy } else { 0. },
        ),
        gx.into_iter().chain(gy).collect(),
    )
}
fn projected(p: Point, a: Point, b: Point) -> Point {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = dx * dx + dy * dy;
    if len < 1e-12 {
        return a;
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len).clamp(0., 1.);
    Point::new(a.x + dx * t, a.y + dy * t)
}
pub fn add_waypoint(doc: &mut Document, id: Id, p: Point) -> Result<usize> {
    let c = doc
        .connectors
        .iter()
        .find(|c| c.id == id)
        .ok_or("線が選択されていません")?;
    if c.waypoints.len() >= 1000 {
        return Err("途中点は 1000 個までです".into());
    }
    let route = geometry::route(doc, c);
    let along = |p: Point| {
        let mut distance = 0.;
        let mut best = (f64::INFINITY, 0., p);
        for s in route.windows(2) {
            let q = projected(p, s[0], s[1]);
            let d = p.distance(q);
            if d < best.0 {
                best = (d, distance + s[0].distance(q), q);
            }
            distance += s[0].distance(s[1]);
        }
        best
    };
    let hit = along(p);
    let index = c
        .waypoints
        .iter()
        .position(|w| along(*w).1 > hit.1)
        .unwrap_or(c.waypoints.len());
    let p = doc.snap_point(hit.2);
    doc.connectors
        .iter_mut()
        .find(|c| c.id == id)
        .unwrap()
        .waypoints
        .insert(index, p);
    Ok(index)
}
pub fn remove_waypoint(doc: &mut Document, id: Id, p: Point, tolerance: f64) -> bool {
    let Some(c) = doc.connectors.iter_mut().find(|c| c.id == id) else {
        return false;
    };
    let index = c
        .waypoints
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.distance(p).total_cmp(&b.distance(p)))
        .filter(|(_, v)| v.distance(p) < tolerance)
        .map(|(i, _)| i);
    if let Some(i) = index {
        c.waypoints.remove(i);
        true
    } else {
        false
    }
}
pub fn junction(doc: &mut Document, p: Point, tolerance: f64) -> Id {
    let routes = router::Router::new(doc).all();
    let mut center = doc.snap_point(p);
    if let Some(q) = routes
        .iter()
        .flat_map(|r| r.points.windows(2))
        .map(|s| projected(p, s[0], s[1]))
        .filter(|q| q.distance(p) < tolerance)
        .min_by(|a, b| a.distance(p).total_cmp(&b.distance(p)))
    {
        center = q;
    }
    let id = doc.add_node(Shape::Junction, center, None);
    let node = doc.nodes.last_mut().unwrap();
    node.rect.x = center.x - 4.;
    node.rect.y = center.y - 4.;
    let original = doc.connectors.len();
    for (i, route) in routes.into_iter().enumerate().take(original) {
        if let Some(segment) = route.points.windows(2).position(|s| {
            geometry::distance_to_segment(center, s[0], s[1]) < 0.01
                && center.distance(route.points[0]) > 1.
                && center.distance(*route.points.last().unwrap()) > 1.
        }) {
            let mut after = doc.connectors[i].clone();
            after.id = doc.id();
            after.start = Endpoint::Attached { node: id, side: 0 };
            after.waypoints = route.points[segment + 1..route.points.len() - 1].to_vec();
            after.arrow_start = false;
            after.label.clear();
            let before = &mut doc.connectors[i];
            before.end = Endpoint::Attached { node: id, side: 0 };
            before.waypoints = route.points[1..segment + 1].to_vec();
            before.arrow_end = false;
            doc.connectors.push(after);
        }
    }
    id
}

use crate::*;

pub fn route(doc: &Document, c: &Connector) -> Vec<Point> {
    crate::router::Router::new(doc).route(c).points
}
/// Tangency points for a quadratic rounded bend; adjacent corners share at most half an edge.
pub fn rounded_corner(path: &[Point], index: usize, radius: f64) -> Option<(Point, Point)> {
    if radius <= 0. || index == 0 || index + 1 >= path.len() {
        return None;
    }
    let a = path[index - 1];
    let b = path[index];
    let c = path[index + 1];
    let incoming = b.distance(a);
    let outgoing = b.distance(c);
    if incoming < 0.01 || outgoing < 0.01 || cross(sub(b, a), sub(c, b)).abs() < 1e-6 {
        return None;
    }
    let r = radius.min(incoming / 2.).min(outgoing / 2.);
    Some((
        Point::new(
            b.x + (a.x - b.x) * r / incoming,
            b.y + (a.y - b.y) * r / incoming,
        ),
        Point::new(
            b.x + (c.x - b.x) * r / outgoing,
            b.y + (c.y - b.y) * r / outgoing,
        ),
    ))
}
pub fn rounded_points(path: &[Point], radius: f64) -> Vec<Point> {
    let Some(first) = path.first() else {
        return vec![];
    };
    let mut out = vec![*first];
    for i in 1..path.len() {
        if let Some((a, b)) = rounded_corner(path, i, radius) {
            out.push(a);
            let corner = path[i];
            for step in 1..=12 {
                let t = step as f64 / 12.;
                let s = 1. - t;
                out.push(Point::new(
                    s * s * a.x + 2. * s * t * corner.x + t * t * b.x,
                    s * s * a.y + 2. * s * t * corner.y + t * t * b.y,
                ));
            }
        } else {
            out.push(path[i]);
        }
    }
    out
}
fn cross(a: Point, b: Point) -> f64 {
    a.x * b.y - a.y * b.x
}
fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}
/// Proper interior crossing only. Shared endpoints, bends and collinear overlaps do not jump.
pub fn intersection(a: Point, b: Point, c: Point, d: Point) -> Option<Point> {
    let r = sub(b, a);
    let s = sub(d, c);
    let den = cross(r, s);
    if den.abs() < 1e-8 {
        return None;
    }
    let t = cross(sub(c, a), s) / den;
    let u = cross(sub(c, a), r) / den;
    if !(1e-6..1. - 1e-6).contains(&t) || !(1e-6..1. - 1e-6).contains(&u) {
        return None;
    }
    Some(Point::new(a.x + t * r.x, a.y + t * r.y))
}
pub fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = sub(b, a);
    let l = ab.x * ab.x + ab.y * ab.y;
    if l < 1e-8 {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l).clamp(0., 1.);
    p.distance(Point::new(a.x + ab.x * t, a.y + ab.y * t))
}
pub fn hit(doc: &Document, p: Point, tolerance: f64) -> Option<Id> {
    doc.nodes
        .iter()
        .rev()
        .find(|n| {
            n.rect.expand(tolerance / 2.).contains(p)
                && (n.shape != Shape::Frame || !n.rect.expand(-tolerance).contains(p))
                && shapes::geometry(n.shape).is_none_or(|g| {
                    (!n.label.is_empty() && g.text_rect(n.rect).contains(p))
                        || g.contains(Point::new(
                            (p.x - n.rect.x) / n.rect.w * 100.,
                            (p.y - n.rect.y) / n.rect.h * 100.,
                        ))
                        || g.near_outline(n.rect, p, tolerance / 2.)
                })
        })
        .map(|n| n.id)
        .or_else(|| {
            let router = crate::router::Router::new(doc);
            doc.connectors
                .iter()
                .rev()
                .find(|c| {
                    rounded_points(&router.route(c).points, c.corner_radius)
                        .windows(2)
                        .any(|s| {
                            distance_to_segment(p, s[0], s[1])
                                <= tolerance + if doc.jumps { doc.jump_radius } else { 0. }
                        })
                })
                .map(|c| c.id)
        })
}

/// Index segment bounds, then choose exactly one bridge at each crossing.
pub fn connector_paths(doc: &Document) -> Vec<String> {
    let routes: Vec<_> = crate::router::Router::new(doc)
        .all()
        .into_iter()
        .map(|r| r.points)
        .collect();
    connector_paths_for(doc, &routes)
}
#[derive(Clone, Copy, Debug)]
struct Segment {
    owner: usize,
    index: usize,
    a: Point,
    b: Point,
}
type Crossings = std::collections::BTreeMap<(i64, i64), Vec<(Segment, Point)>>;
fn crossings(segments: &[Segment]) -> Crossings {
    use std::collections::{HashMap, HashSet};
    let mut crossings = Crossings::new();
    let mut tiles = HashMap::<(i32, i32), Vec<usize>>::new();
    let mut large = vec![];
    let mut candidates = HashSet::new();
    for (i, s) in segments.iter().enumerate() {
        let (x0, x1) = (
            (s.a.x.min(s.b.x) / 256.).floor() as i32,
            (s.a.x.max(s.b.x) / 256.).floor() as i32,
        );
        let (y0, y1) = (
            (s.a.y.min(s.b.y) / 256.).floor() as i32,
            (s.a.y.max(s.b.y) / 256.).floor() as i32,
        );
        let is_large =
            (i64::from(x1) - i64::from(x0) + 1) * (i64::from(y1) - i64::from(y0) + 1) > 64;
        candidates.clear();
        if is_large {
            candidates.extend(0..i);
        } else {
            candidates.extend(large.iter().copied());
            for x in x0..=x1 {
                for y in y0..=y1 {
                    if let Some(ids) = tiles.get(&(x, y)) {
                        candidates.extend(ids.iter().copied());
                    }
                }
            }
        }
        for &j in &candidates {
            let t = segments[j];
            if s.owner == t.owner
                || s.a.x.min(s.b.x) > t.a.x.max(t.b.x)
                || t.a.x.min(t.b.x) > s.a.x.max(s.b.x)
                || s.a.y.min(s.b.y) > t.a.y.max(t.b.y)
                || t.a.y.min(t.b.y) > s.a.y.max(s.b.y)
            {
                continue;
            }
            if let Some(p) = intersection(s.a, s.b, t.a, t.b) {
                let bucket = crossings
                    .entry(((p.x * 1e5).round() as i64, (p.y * 1e5).round() as i64))
                    .or_default();
                bucket.push((*s, p));
                bucket.push((t, p));
            }
        }
        if is_large {
            large.push(i);
        } else {
            for x in x0..=x1 {
                for y in y0..=y1 {
                    tiles.entry((x, y)).or_default().push(i);
                }
            }
        }
    }
    crossings
}
pub fn connector_paths_for(doc: &Document, routes: &[Vec<Point>]) -> Vec<String> {
    use std::collections::HashMap;
    let crossings = if doc.jumps && doc.connectors.iter().any(|c| c.jump != Jump::Never) {
        let segments: Vec<_> = routes
            .iter()
            .enumerate()
            .flat_map(|(owner, r)| {
                r.windows(2).enumerate().map(move |(index, p)| Segment {
                    owner,
                    index,
                    a: p[0],
                    b: p[1],
                })
            })
            .collect();
        crossings(&segments)
    } else {
        Crossings::new()
    };
    let mut bridges = HashMap::<(usize, usize), Vec<f64>>::new();
    for participants in crossings.values() {
        let winner = participants
            .iter()
            .map(|(s, _)| s.owner)
            .filter(|i| doc.connectors[*i].jump != Jump::Never)
            .max_by_key(|i| (doc.connectors[*i].jump == Jump::Always, *i));
        if let Some(winner) = winner {
            for (s, p) in participants {
                if s.owner == winner {
                    bridges
                        .entry((s.owner, s.index))
                        .or_default()
                        .push(s.a.distance(*p));
                }
            }
        }
    }
    routes
        .iter()
        .enumerate()
        .map(|(index, path)| build_path(doc, index, path, &bridges))
        .collect()
}
pub fn connector_path(doc: &Document, index: usize) -> String {
    connector_paths(doc).get(index).cloned().unwrap_or_default()
}
fn build_path(
    doc: &Document,
    index: usize,
    path: &[Point],
    bridges: &std::collections::HashMap<(usize, usize), Vec<f64>>,
) -> String {
    let c = &doc.connectors[index];
    let Some(first) = path.first() else {
        return String::new();
    };
    let mut out = format!("M {} {}", first.x, first.y);
    for (segment_index, segment) in path.windows(2).enumerate() {
        let a = segment[0];
        let b = segment[1];
        let len = a.distance(b);
        if len < 0.01 {
            continue;
        }
        let ux = (b.x - a.x) / len;
        let uy = (b.y - a.y) / len;
        let start_trim = rounded_corner(path, segment_index, c.corner_radius)
            .map_or(0., |(_, exit)| a.distance(exit));
        let corner = rounded_corner(path, segment_index + 1, c.corner_radius);
        let end_trim = corner.map_or(0., |(entry, _)| entry.distance(b));
        let mut jumps = bridges
            .get(&(index, segment_index))
            .cloned()
            .unwrap_or_default();
        jumps.sort_by(f64::total_cmp);
        jumps.dedup_by(|a, b| (*a - *b).abs() < 0.1);
        for (i, &t) in jumps.iter().enumerate() {
            let prev = if i == 0 { 0. } else { jumps[i - 1] };
            let next = if i + 1 == jumps.len() {
                len
            } else {
                jumps[i + 1]
            };
            let r = doc
                .jump_radius
                .min((t - prev) / 2.)
                .min((next - t) / 2.)
                .min(
                    t - start_trim
                        - if segment_index == 0 && c.arrow_start {
                            9. + c.width
                        } else {
                            1.
                        },
                )
                .min(
                    len - t
                        - end_trim
                        - if segment_index + 2 == path.len() && c.arrow_end {
                            9. + c.width
                        } else {
                            1.
                        },
                );
            if r < 1. {
                continue;
            }
            let p = Point::new(a.x + ux * (t - r), a.y + uy * (t - r));
            let q = Point::new(a.x + ux * (t + r), a.y + uy * (t + r));
            let nx = uy;
            let ny = -ux;
            out.push_str(&format!(
                " L {} {} C {} {}, {} {}, {} {}",
                p.x,
                p.y,
                p.x + nx * r * 1.3333,
                p.y + ny * r * 1.3333,
                q.x + nx * r * 1.3333,
                q.y + ny * r * 1.3333,
                q.x,
                q.y
            ));
        }
        if let Some((entry, exit)) = corner {
            out.push_str(&format!(
                " L {} {} Q {} {}, {} {}",
                entry.x, entry.y, b.x, b.y, exit.x, exit.y
            ));
        } else {
            out.push_str(&format!(" L {} {}", b.x, b.y));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tiled_crossings_match_all_pairs_for_short_long_and_diagonal_segments() {
        let mut segments = vec![];
        let mut seed = 71u64;
        let mut coordinate = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((seed >> 32) % 8000) as f64 - 4000.
        };
        for i in 0..320 {
            let a = Point::new(coordinate(), coordinate());
            let mut b = Point::new(coordinate(), coordinate());
            if i % 3 == 0 {
                b.x = a.x;
            }
            if i % 3 == 1 {
                b.y = a.y;
            }
            if i % 17 == 0 {
                b.x += 40000.;
            }
            segments.push(Segment {
                owner: i / 2,
                index: i % 2,
                a,
                b,
            });
        }
        // Tile boundaries, zero length, endpoint touches, and negative coordinates.
        for (a, b) in [
            (Point::new(-256., -512.), Point::new(-256., 512.)),
            (Point::new(-512., 256.), Point::new(512., 256.)),
            (Point::new(0., 0.), Point::new(0., 0.)),
            (Point::new(-256., 256.), Point::new(-128., 384.)),
        ] {
            segments.push(Segment {
                owner: segments.len(),
                index: 0,
                a,
                b,
            });
        }
        let mut expected = Crossings::new();
        for (i, s) in segments.iter().enumerate() {
            for t in &segments[..i] {
                if s.owner == t.owner {
                    continue;
                }
                if let Some(p) = intersection(s.a, s.b, t.a, t.b) {
                    let bucket = expected
                        .entry(((p.x * 1e5).round() as i64, (p.y * 1e5).round() as i64))
                        .or_default();
                    bucket.extend([(*s, p), (*t, p)]);
                }
            }
        }
        let normalized = |crossings: Crossings| {
            crossings
                .into_iter()
                .map(|(key, values)| {
                    let mut owners: Vec<_> = values
                        .into_iter()
                        .map(|(s, _)| (s.owner, s.index))
                        .collect();
                    owners.sort_unstable();
                    (key, owners)
                })
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        assert_eq!(normalized(crossings(&segments)), normalized(expected));
    }
}

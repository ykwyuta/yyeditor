//! Stable port identifiers: 0..4 retain the original edge midpoints.
use crate::*;
pub const COUNT: u8 = 20;
pub fn edge(id: u8) -> u8 {
    if id < 4 { id } else { ((id - 4) / 4).min(3) }
}
pub fn count(node: &Node) -> u8 {
    if node.shape == Shape::Junction {
        1
    } else {
        COUNT
    }
}
pub fn point(n: &Node, id: u8) -> Point {
    point_on(n.shape, n.rect, id)
}
pub fn point_on(shape: Shape, r: Rect, id: u8) -> Point {
    if shape == Shape::Junction {
        return r.center();
    }
    let side = edge(id);
    let t = if id < 4 {
        0.5
    } else {
        [1. / 6., 1. / 3., 2. / 3., 5. / 6.][((id - 4) % 4) as usize]
    };
    if let Some(geometry) = shapes::geometry(shape) {
        let p = geometry.port(side, t);
        return Point::new(r.x + r.w * p.x / 100., r.y + r.h * p.y / 100.);
    }
    let mut p = match side {
        0 => Point::new(r.x + r.w * t, r.y),
        1 => Point::new(r.x + r.w, r.y + r.h * t),
        2 => Point::new(r.x + r.w * t, r.y + r.h),
        _ => Point::new(r.x, r.y + r.h * t),
    };
    if id < 4 {
        return p;
    } // Old documents keep precisely the same endpoints.
    let cx = r.x + r.w / 2.;
    let cy = r.y + r.h / 2.;
    match shape {
        Shape::Ellipse => {
            let inset = (1. - (t * 2. - 1.).powi(2)).sqrt();
            match side {
                0 => p.y = cy - r.h / 2. * inset,
                1 => p.x = cx + r.w / 2. * inset,
                2 => p.y = cy + r.h / 2. * inset,
                _ => p.x = cx - r.w / 2. * inset,
            }
        }
        Shape::Diamond | Shape::Triangle => {
            let dx = (p.x - cx) / (r.w / 2.);
            let dy = (p.y - cy) / (r.h / 2.);
            let divisor = if shape == Shape::Diamond {
                dx.abs() + dy.abs()
            } else {
                (2. * dx.abs() - dy).max(dy)
            };
            p = Point::new(cx + dx / divisor * r.w / 2., cy + dy / divisor * r.h / 2.);
        }
        Shape::Cylinder if side == 0 || side == 2 => {
            let ry = (r.h / 6.).min(r.w / 4.);
            let inset = (1. - (t * 2. - 1.).powi(2)).sqrt();
            p.y = if side == 0 {
                r.y + ry - ry * inset
            } else {
                r.y + r.h - ry + ry * inset
            };
        }
        _ => {}
    }
    p
}
pub fn exit(n: &Node, id: u8) -> Point {
    exit_on(n.shape, n.rect, id)
}
pub fn exit_on(shape: Shape, rect: Rect, id: u8) -> Point {
    let mut p = point_on(shape, rect, id);
    if shape == Shape::Junction {
        return p;
    }
    match edge(id) {
        0 => p.y = rect.y - 16.,
        1 => p.x = rect.x + rect.w + 16.,
        2 => p.y = rect.y + rect.h + 16.,
        _ => p.x = rect.x - 16.,
    };
    p
}
impl Document {
    pub fn nearest_port(&self, p: Point, tolerance: f64) -> Option<Endpoint> {
        let mut best = None;
        let mut distance = tolerance;
        for n in self
            .nodes
            .iter()
            .rev()
            .filter(|n| n.rect.expand(tolerance).contains(p))
        {
            for side in 0..count(n) {
                let d = point(n, side).distance(p);
                if d < distance || best.is_none() && d <= distance {
                    distance = d;
                    best = Some(Endpoint::Attached { node: n.id, side });
                }
            }
        }
        best
    }
    pub fn attach_near(&self, p: Point, tolerance: f64) -> Endpoint {
        if let Some(e) = self.nearest_port(p, tolerance) {
            return e;
        }
        if let Some(n) = self.nodes.iter().rev().find(|n| n.rect.contains(p)) {
            let side = (0..count(n))
                .min_by(|a, b| {
                    point(n, *a)
                        .distance(p)
                        .total_cmp(&point(n, *b).distance(p))
                })
                .unwrap_or(0);
            Endpoint::Attached { node: n.id, side }
        } else {
            Endpoint::Free(self.snap_point(p))
        }
    }
}

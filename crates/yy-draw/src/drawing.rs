//! Interactive cardinal drawing, independent of the window and keyboard APIs.
use crate::*;

#[derive(Clone, Debug)]
pub struct Draft {
    pub origin: Point,
    pub pointer: Point,
    pub current: Point,
    pub bends: Vec<Point>,
    pub constrained: bool,
    pub manual: bool,
}
impl Draft {
    pub fn new(origin: Point) -> Self {
        Self {
            origin,
            pointer: origin,
            current: origin,
            bends: vec![],
            constrained: false,
            manual: false,
        }
    }
    pub fn pivot(&self) -> Point {
        self.bends.last().copied().unwrap_or(self.origin)
    }
    pub fn update(&mut self, pointer: Point, shift: bool) {
        self.pointer = pointer;
        self.constrained = shift;
        self.manual |= shift;
        if !shift {
            self.current = pointer;
            return;
        }
        let p = self.pivot();
        let previous = if self.bends.len() >= 2 {
            self.bends[self.bends.len() - 2]
        } else {
            self.origin
        };
        let incoming = Point::new(p.x - previous.x, p.y - previous.y);
        let delta = Point::new(pointer.x - p.x, pointer.y - p.y);
        let directions = [
            Point::new(1., 0.),
            Point::new(0., 1.),
            Point::new(-1., 0.),
            Point::new(0., -1.),
        ];
        let (direction, length) = directions
            .into_iter()
            .filter(|d| incoming.x * d.x + incoming.y * d.y >= -1e-6)
            .map(|d| (d, (delta.x * d.x + delta.y * d.y).max(0.)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        self.current = Point::new(p.x + direction.x * length, p.y + direction.y * length);
    }
    pub fn bend(&mut self) -> bool {
        if !self.constrained || self.bends.len() >= 1000 || self.current.distance(self.pivot()) < 1.
        {
            return false;
        }
        self.bends.push(self.current);
        self.manual = true;
        true
    }
    pub fn undo_bend(&mut self) {
        self.bends.pop();
        self.update(self.pointer, self.constrained);
    }
    pub fn endpoint(&self, doc: &Document, tolerance: f64) -> Endpoint {
        let candidate = if self.constrained {
            doc.nearest_port(self.current, tolerance)
                .unwrap_or(Endpoint::Free(self.current))
        } else {
            doc.attach_near(self.current, tolerance)
        };
        if self.constrained {
            let p = doc.endpoint(candidate);
            let pivot = self.pivot();
            let horizontal = (self.current.y - pivot.y).abs() < 1e-6;
            let aligned = if horizontal {
                (p.y - pivot.y).abs() < 1e-6
            } else {
                (p.x - pivot.x).abs() < 1e-6
            };
            let forward = (p.x - pivot.x) * (self.current.x - pivot.x)
                + (p.y - pivot.y) * (self.current.y - pivot.y)
                >= 0.;
            if !aligned || !forward {
                return Endpoint::Free(self.current);
            }
        }
        candidate
    }
}

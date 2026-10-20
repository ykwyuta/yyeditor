//! Immediate GPU feedback; expensive scene rendering follows after gestures settle.
use super::*;
use std::time::{Duration, Instant};
pub(super) const OVERSCAN: f64 = 96.;
#[derive(Clone, Copy)]
pub struct ZoomMotion {
    uid: u64,
    target: f64,
    anchor: Point,
    cursor: Point,
    last: Instant,
    finish: Instant,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub origin: Point,
    pub scale: f64,
}
impl Camera {
    pub fn position(self, cached: Self, viewport: Point) -> Point {
        Point::new(
            viewport.x + (cached.origin.x - self.origin.x) * self.scale,
            viewport.y + (cached.origin.y - self.origin.y) * self.scale,
        )
    }
}
impl App {
    pub(super) fn tick_zoom(&mut self, now: Instant) -> bool {
        let Some(mut motion) = self.zoom_motion.take() else {
            return false;
        };
        let (l, t, _, _) = self.viewport();
        let tab = &mut self.tabs[self.tab];
        if tab.uid != motion.uid {
            return false;
        }
        let dt = now.saturating_duration_since(motion.last).as_secs_f64();
        let difference = (motion.target / tab.zoom).ln();
        let done = difference.abs() < 0.0005 || now >= motion.finish;
        tab.zoom = if done {
            motion.target
        } else {
            (tab.zoom.ln() + difference * (1. - (-dt / 0.035).exp())).exp()
        };
        tab.origin = Point::new(
            motion.anchor.x - (motion.cursor.x - l as f64) / (tab.zoom * self.dpi),
            motion.anchor.y - (motion.cursor.y - t as f64) / (tab.zoom * self.dpi),
        );
        if !done {
            motion.last = now;
            self.zoom_motion = Some(motion);
        }
        self.invalidate_overlay();
        true
    }
    pub(super) fn follow_zoom_pointer(&mut self, cursor: Point, now: Instant) {
        if self.zoom_motion.is_none() {
            return;
        }
        self.tick_zoom(now);
        let (l, t, w, h) = self.viewport();
        if cursor.x < l as f64
            || cursor.y < t as f64
            || cursor.x > (l as f64 + w as f64)
            || cursor.y > (t as f64 + h as f64)
        {
            return;
        }
        let anchor = self.document_point(cursor);
        if let Some(motion) = &mut self.zoom_motion {
            motion.anchor = anchor;
            motion.cursor = cursor;
        }
    }
    pub(super) fn stop_zoom(&mut self) {
        self.tick_zoom(Instant::now());
        self.zoom_motion = None;
    }
    pub(super) fn wheel(&mut self, cursor: Point, delta: f64, zoom: bool, horizontal: bool) {
        let now = Instant::now();
        self.tick_zoom(now);
        if zoom {
            let tab = &self.tabs[self.tab];
            let target = self.zoom_motion.map_or(tab.zoom, |m| m.target);
            let motion = ZoomMotion {
                uid: tab.uid,
                target: (target * 1.15f64.powf(delta / 120.)).clamp(0.1, 8.),
                anchor: self.document_point(cursor),
                cursor,
                last: now,
                finish: now + Duration::from_millis(200),
            };
            self.view_changed();
            self.zoom_motion = Some(motion);
            return;
        }
        self.stop_zoom();
        let tab = &mut self.tabs[self.tab];
        if horizontal {
            tab.origin.x -= delta / 2. / tab.zoom;
        } else {
            tab.origin.y -= delta / 2. / tab.zoom;
        }
        self.view_changed();
    }
    pub(super) fn frame_ready(&self) -> bool {
        self.dirty
            && self.zoom_motion.is_none()
            && self
                .view_deadline
                .is_none_or(|t| std::time::Instant::now() >= t)
    }
    pub(super) fn view_changed(&mut self) {
        self.invalidate();
        self.view_deadline =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(120));
    }
    pub(super) fn arrow_preview(&mut self) -> Option<paint::Stroke> {
        let (l, t, _, _) = self.viewport();
        let tab = &self.tabs[self.tab];
        let c = if let Some(Drag::Connect {
            start,
            draft,
            orthogonal,
        }) = self.drag.as_ref()
        {
            Connector {
                id: 0,
                start: *start,
                end: draft.endpoint(&tab.doc, 10. / tab.zoom),
                waypoints: draft.bends.clone(),
                orthogonal: *orthogonal && !draft.manual,
                arrow_start: false,
                arrow_end: true,
                jump: Jump::Never,
                color: self.line_color,
                width: 1.5,
                dashed: false,
                label: String::new(),
                corner_radius: if self.rounded_lines {
                    self.corner_radius
                } else {
                    0.
                },
                group: None,
            }
        } else {
            let (uid, id) = self.pending_connector?;
            if uid != tab.uid {
                return None;
            }
            tab.doc.connectors.iter().find(|c| c.id == id)?.clone()
        };
        self.routing.prepare(&tab.doc);
        let route = self.routing.route(&c);
        let scale = tab.zoom * self.dpi;
        let points = route
            .points
            .iter()
            .map(|p| {
                Point::new(
                    l as f64 + (p.x - tab.origin.x) * scale,
                    t as f64 + (p.y - tab.origin.y) * scale,
                )
            })
            .collect();
        Some(paint::Stroke {
            points,
            color: if route.resolved { c.color } else { 0xc53232 },
            width: (c.width * scale) as f32,
            arrow_size: if c.arrow_end {
                ((8. + c.width) * scale) as f32
            } else {
                0.
            },
            corner_radius: c.corner_radius * scale,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_camera_preserves_world_points_during_zoom_and_pan() {
        let cached = Camera {
            origin: Point::new(-24., 32.),
            scale: 1.5,
        };
        let current = Camera {
            origin: Point::new(18., -12.),
            scale: 2.25,
        };
        let viewport = Point::new(260., 70.);
        let position = current.position(cached, viewport);
        let world = Point::new(100., 80.);
        let ratio = current.scale / cached.scale;
        let image = Point::new(
            (world.x - cached.origin.x) * cached.scale,
            (world.y - cached.origin.y) * cached.scale,
        );
        assert!(
            (position.x + image.x * ratio
                - (viewport.x + (world.x - current.origin.x) * current.scale))
                .abs()
                < 1e-8
        );
        assert!(
            (position.y + image.y * ratio
                - (viewport.y + (world.y - current.origin.y) * current.scale))
                .abs()
                < 1e-8
        );
    }
}

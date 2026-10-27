use super::*;
impl App {
    pub(super) fn update_port_hover(&mut self, p: Point) {
        let (l, t, w, h) = self.viewport();
        let inside = p.x >= l as f64
            && p.y >= t as f64
            && p.x < l as f64 + w as f64
            && p.y < t as f64 + h as f64;
        let doc = &self.tabs[self.tab].doc;
        let zoom = self.tabs[self.tab].zoom;
        let q = self.document_point(p);
        let node = if inside {
            doc.nodes
                .iter()
                .rev()
                .find(|n| n.rect.expand(8. / zoom).contains(q))
                .map(|n| n.id)
        } else {
            None
        };
        let wiring = matches!(self.tool, Tool::Connect(_))
            || matches!(
                self.drag,
                Some(Drag::Connect { .. } | Drag::Endpoint { .. })
            );
        let port = if !inside {
            None
        } else if let Some(Drag::Connect { draft, .. }) = &self.drag {
            match draft.endpoint(doc, 10. / zoom) {
                e @ Endpoint::Attached { .. } => Some(e),
                _ => None,
            }
        } else if wiring {
            match doc.attach_near(q, 10. / zoom) {
                e @ Endpoint::Attached { .. } => Some(e),
                _ => None,
            }
        } else {
            doc.nearest_port(q, 6. / zoom)
        };
        let node = match port {
            Some(Endpoint::Attached { node, .. }) => Some(node),
            _ => node,
        };
        if self.hover_node != node || self.hover_port != port {
            self.hover_node = node;
            self.hover_port = port;
            self.invalidate_overlay();
        }
    }
    pub(super) fn connection_markers(&self) -> Vec<paint::PortMarker> {
        if self.palette_state.drag.is_some()
            || matches!(
                self.drag,
                Some(Drag::Move { .. } | Drag::Resize { .. } | Drag::Pan { .. } | Drag::Box { .. })
            )
            || !matches!(self.tool, Tool::Select | Tool::Connect(_))
        {
            return vec![];
        }
        let tab = &self.tabs[self.tab];
        let mut nodes = vec![];
        if let Some(id) = self.hover_node {
            nodes.push(id);
        }
        let start = match &self.drag {
            Some(Drag::Connect { start, .. }) => Some(*start),
            _ => None,
        };
        if let Some(Endpoint::Attached { node, .. }) = start
            && !nodes.contains(&node)
        {
            nodes.push(node);
        }
        for id in tab.selected.iter().take(8) {
            if tab.doc.node(*id).is_some() && !nodes.contains(id) {
                nodes.push(*id);
            }
        }
        let (l, t, _, _) = self.viewport();
        let scale = tab.zoom * self.dpi;
        nodes
            .into_iter()
            .filter_map(|id| tab.doc.node(id))
            .flat_map(|n| (0..yy_draw::ports::count(n)).map(move |side| (n, side)))
            .map(|(n, side)| {
                let endpoint = Endpoint::Attached { node: n.id, side };
                let p = yy_draw::ports::point(n, side);
                paint::PortMarker {
                    position: Point::new(
                        l as f64 + (p.x - tab.origin.x) * scale,
                        t as f64 + (p.y - tab.origin.y) * scale,
                    ),
                    active: self.hover_port == Some(endpoint) || start == Some(endpoint),
                    radius: 3. * self.dpi as f32,
                }
            })
            .collect()
    }
}

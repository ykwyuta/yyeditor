//! Group-aware, linear-time selection and geometry collection; sorting only for distribution.
use crate::*;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    Top,
    Middle,
    Bottom,
    Horizontal,
    Vertical,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reference {
    Selection,
    Page,
}
pub struct Unit {
    pub bounds: Rect,
    pub group: Option<Id>,
    pub locked: bool,
    nodes: Vec<usize>,
    connectors: Vec<usize>,
}
impl Document {
    pub fn expanded_selection(&self, ids: &[Id]) -> Vec<Id> {
        let selected: HashSet<_> = ids.iter().copied().collect();
        let groups: HashSet<_> = self
            .nodes
            .iter()
            .filter(|n| selected.contains(&n.id))
            .filter_map(|n| n.group)
            .chain(
                self.connectors
                    .iter()
                    .filter(|c| selected.contains(&c.id))
                    .filter_map(|c| c.group),
            )
            .collect();
        self.nodes
            .iter()
            .filter(|n| selected.contains(&n.id) || n.group.is_some_and(|g| groups.contains(&g)))
            .map(|n| n.id)
            .chain(
                self.connectors
                    .iter()
                    .filter(|c| {
                        selected.contains(&c.id) || c.group.is_some_and(|g| groups.contains(&g))
                    })
                    .map(|c| c.id),
            )
            .collect()
    }
    pub fn group_objects(&mut self, ids: &[Id]) -> Result<Vec<Id>> {
        let mut selected: HashSet<_> = self.expanded_selection(ids).into_iter().collect();
        for c in &self.connectors {
            if matches!((c.start,c.end),(Endpoint::Attached{node:a,..},Endpoint::Attached{node:b,..}) if selected.contains(&a)&&selected.contains(&b))
            {
                selected.insert(c.id);
            }
        }
        if selected.len() < 2 {
            return Err("グループ化には2個以上のオブジェクトを選択してください".into());
        }
        let group = self.id();
        for n in &mut self.nodes {
            if selected.contains(&n.id) {
                n.group = Some(group);
            }
        }
        for c in &mut self.connectors {
            if selected.contains(&c.id) {
                c.group = Some(group);
            }
        }
        Ok(self.expanded_selection(&selected.into_iter().collect::<Vec<_>>()))
    }
    pub fn ungroup_objects(&mut self, ids: &[Id]) -> Vec<Id> {
        let members = self.expanded_selection(ids);
        let selected: HashSet<_> = members.iter().copied().collect();
        for n in &mut self.nodes {
            if selected.contains(&n.id) {
                n.group = None;
            }
        }
        for c in &mut self.connectors {
            if selected.contains(&c.id) {
                c.group = None;
            }
        }
        members
    }
    pub fn layout_units(&self, ids: &[Id]) -> Vec<Unit> {
        let selected: HashSet<_> = self.expanded_selection(ids).into_iter().collect();
        let mut units: Vec<Unit> = vec![];
        let mut map = HashMap::<(bool, Id), usize>::new();
        let mut add = |id: Id,
                       group: Option<Id>,
                       bounds: Rect,
                       node: Option<usize>,
                       line: Option<usize>,
                       locked: bool| {
            let key = group.map_or((false, id), |g| (true, g));
            let index = *map.entry(key).or_insert_with(|| {
                units.push(Unit {
                    bounds,
                    group,
                    locked,
                    nodes: vec![],
                    connectors: vec![],
                });
                units.len() - 1
            });
            let u = &mut units[index];
            u.bounds = u.bounds.union(bounds);
            u.locked |= locked;
            if let Some(i) = node {
                u.nodes.push(i);
            }
            if let Some(i) = line {
                u.connectors.push(i);
            }
        };
        for (i, n) in self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| selected.contains(&n.id))
        {
            add(n.id, n.group, n.rect, Some(i), None, n.locked);
        }
        // Attached wires follow their nodes; free wires can be aligned independently.
        for (i, c) in self
            .connectors
            .iter()
            .enumerate()
            .filter(|(_, c)| selected.contains(&c.id))
        {
            if let (Endpoint::Free(a), Endpoint::Free(b)) = (c.start, c.end) {
                let mut bounds = Rect {
                    x: a.x.min(b.x),
                    y: a.y.min(b.y),
                    w: (a.x - b.x).abs(),
                    h: (a.y - b.y).abs(),
                };
                for p in &c.waypoints {
                    bounds = bounds.union(Rect {
                        x: p.x,
                        y: p.y,
                        w: 0.,
                        h: 0.,
                    });
                }
                add(c.id, c.group, bounds, None, Some(i), false);
            }
        }
        for (i, c) in self.connectors.iter().enumerate() {
            if selected.contains(&c.id)
                && !matches!((c.start, c.end), (Endpoint::Free(_), Endpoint::Free(_)))
                && let Some(group) = c.group
                && let Some(index) = map.get(&(true, group))
            {
                units[*index].connectors.push(i);
            }
        }
        units
    }
    pub fn arrange(&mut self, ids: &[Id], action: Align, reference: Reference) -> Result<usize> {
        let mut units: Vec<_> = self
            .layout_units(ids)
            .into_iter()
            .filter(|u| !u.locked)
            .collect();
        let distribute = matches!(action, Align::Horizontal | Align::Vertical);
        let minimum = if distribute {
            if reference == Reference::Page { 2 } else { 3 }
        } else if reference == Reference::Page {
            1
        } else {
            2
        };
        if units.len() < minimum {
            return Err(format!(
                "この配置には{minimum}個以上の移動可能な図形・グループを選択してください"
            ));
        }
        let target = if reference == Reference::Page {
            self.page_bounds()
        } else {
            units.iter().map(|u| u.bounds).reduce(Rect::union).unwrap()
        };
        let mut deltas = vec![Point::default(); units.len()];
        if distribute {
            let horizontal = action == Align::Horizontal;
            units.sort_by(|a, b| {
                if horizontal {
                    a.bounds.x.total_cmp(&b.bounds.x)
                } else {
                    a.bounds.y.total_cmp(&b.bounds.y)
                }
            });
            let total: f64 = units
                .iter()
                .map(|u| if horizontal { u.bounds.w } else { u.bounds.h })
                .sum();
            let extent = if horizontal { target.w } else { target.h };
            let gap = (extent - total) / (units.len() - 1) as f64;
            let mut cursor = if horizontal { target.x } else { target.y };
            for (u, delta) in units.iter().zip(&mut deltas) {
                if horizontal {
                    delta.x = cursor - u.bounds.x;
                    cursor += u.bounds.w + gap;
                } else {
                    delta.y = cursor - u.bounds.y;
                    cursor += u.bounds.h + gap;
                }
            }
        } else {
            for (u, d) in units.iter().zip(&mut deltas) {
                let b = u.bounds;
                match action {
                    Align::Left => d.x = target.x - b.x,
                    Align::Center => d.x = target.x + (target.w - b.w) / 2. - b.x,
                    Align::Right => d.x = target.x + target.w - b.w - b.x,
                    Align::Top => d.y = target.y - b.y,
                    Align::Middle => d.y = target.y + (target.h - b.h) / 2. - b.y,
                    Align::Bottom => d.y = target.y + target.h - b.h - b.y,
                    _ => {}
                }
            }
        }
        for (u, d) in units.iter().zip(deltas) {
            for &i in &u.nodes {
                self.nodes[i].rect.x += d.x;
                self.nodes[i].rect.y += d.y;
            }
            for &i in &u.connectors {
                let c = &mut self.connectors[i];
                for e in [&mut c.start, &mut c.end] {
                    if let Endpoint::Free(p) = e {
                        p.x += d.x;
                        p.y += d.y;
                    }
                }
                for p in &mut c.waypoints {
                    p.x += d.x;
                    p.y += d.y;
                }
            }
        }
        Ok(units.len())
    }
}

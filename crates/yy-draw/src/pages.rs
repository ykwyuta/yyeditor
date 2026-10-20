use crate::*;
impl Document {
    /// The active page lives in nodes/connectors; its stored slot is deliberately empty.
    pub fn switch_page(&mut self, index: usize) -> Result<()> {
        if index >= self.pages.len() {
            return Err("ページが見つかりません".into());
        }
        if index != self.active_page {
            self.pages[self.active_page].nodes = std::mem::take(&mut self.nodes);
            self.pages[self.active_page].connectors = std::mem::take(&mut self.connectors);
            self.nodes = std::mem::take(&mut self.pages[index].nodes);
            self.connectors = std::mem::take(&mut self.pages[index].connectors);
            self.active_page = index;
        }
        Ok(())
    }
    pub fn add_page(&mut self, name: String) -> Result<usize> {
        if name.trim().is_empty() || name.len() > 1024 {
            return Err("ページ名は 1～1024 バイトで指定してください".into());
        }
        if self.pages.len() >= 1000 {
            return Err("ページは 1000 枚までです".into());
        }
        let index = self.pages.len();
        self.pages.push(Page {
            name,
            ..Default::default()
        });
        self.switch_page(index)?;
        Ok(index)
    }
    pub fn delete_page(&mut self) -> Result<()> {
        if self.pages.len() == 1 {
            return Err("最後のページは削除できません".into());
        }
        let index = self.active_page;
        let next = if index == 0 { 1 } else { index - 1 };
        self.switch_page(next)?;
        self.pages.remove(index);
        if self.active_page > index {
            self.active_page -= 1;
        }
        Ok(())
    }
    pub fn duplicate_page(&mut self) -> Result<usize> {
        let source = self.fragment(
            &self
                .nodes
                .iter()
                .map(|n| n.id)
                .chain(self.connectors.iter().map(|c| c.id))
                .collect::<Vec<_>>(),
        );
        let page = self.pages[self.active_page].clone();
        let index = self.add_page(format!("{} コピー", page.name))?;
        self.pages[index].width = page.width;
        self.pages[index].height = page.height;
        self.paste(&source, Point::default());
        Ok(index)
    }
    pub fn page_bounds(&self) -> Rect {
        let page = &self.pages[self.active_page];
        Rect {
            x: 0.,
            y: 0.,
            w: page.width,
            h: page.height,
        }
    }
    pub fn fragment(&self, ids: &[Id]) -> Self {
        let mut doc = self.clone();
        doc.nodes.retain(|n| ids.contains(&n.id));
        doc.connectors.retain(|c|ids.contains(&c.id)||matches!((c.start,c.end),(Endpoint::Attached{node:a,..},Endpoint::Attached{node:b,..})if ids.contains(&a)&&ids.contains(&b)));
        let present: HashSet<_> = doc.nodes.iter().map(|n| n.id).collect();
        for c in &mut doc.connectors {
            for e in [&mut c.start, &mut c.end] {
                if matches!(*e,Endpoint::Attached{node,..}if !present.contains(&node)) {
                    *e = Endpoint::Free(self.endpoint(*e));
                }
            }
        }
        doc.assets
            .retain(|a| doc.nodes.iter().any(|n| n.asset == Some(a.id)));
        let assets: HashSet<_> = doc.assets.iter().map(|a| a.id).collect();
        for pack in &mut doc.packs {
            pack.assets.retain(|id| assets.contains(id));
        }
        doc.packs.retain(|p| !p.assets.is_empty());
        doc.pages = vec![Page {
            nodes: vec![],
            connectors: vec![],
            ..self.pages[self.active_page].clone()
        }];
        doc.active_page = 0;
        doc.extra_chunks.clear();
        doc
    }
    pub fn paste(&mut self, source: &Self, offset: Point) -> Vec<Id> {
        use std::collections::HashMap;
        let mut map = HashMap::new();
        let mut groups = HashMap::new();
        let mut selected = vec![];
        for asset in &source.assets {
            let id = if let Some(a) = self.assets.iter().find(|a| a.svg == asset.svg) {
                a.id
            } else {
                let id = self.id();
                let mut a = asset.clone();
                a.id = id;
                self.assets.push(a);
                id
            };
            map.insert(asset.id, id);
        }
        for n in &source.nodes {
            let mut n = n.clone();
            let id = self.id();
            map.insert(n.id, id);
            n.id = id;
            n.rect.x += offset.x;
            n.rect.y += offset.y;
            n.asset = n.asset.and_then(|id| map.get(&id).copied());
            if let Some(group) = n.group {
                n.group = Some(*groups.entry(group).or_insert_with(|| self.id()));
            }
            selected.push(id);
            self.nodes.push(n);
        }
        for c in &source.connectors {
            let mut c = c.clone();
            c.id = self.id();
            if let Some(group) = c.group {
                c.group = Some(*groups.entry(group).or_insert_with(|| self.id()));
            }
            for e in [&mut c.start, &mut c.end] {
                *e = match *e {
                    Endpoint::Attached { node, side } => map.get(&node).map_or_else(
                        || {
                            let p = source.endpoint(*e);
                            Endpoint::Free(Point::new(p.x + offset.x, p.y + offset.y))
                        },
                        |node| Endpoint::Attached { node: *node, side },
                    ),
                    Endpoint::Free(p) => Endpoint::Free(Point::new(p.x + offset.x, p.y + offset.y)),
                };
            }
            for p in &mut c.waypoints {
                p.x += offset.x;
                p.y += offset.y;
            }
            selected.push(c.id);
            self.connectors.push(c);
        }
        for pack in &source.packs {
            let mut pack = pack.clone();
            pack.assets = pack
                .assets
                .iter()
                .filter_map(|id| map.get(id).copied())
                .collect();
            if let Some(p) = self
                .packs
                .iter_mut()
                .find(|p| p.id == pack.id && p.version == pack.version)
            {
                for id in pack.assets {
                    if !p.assets.contains(&id) {
                        p.assets.push(id);
                    }
                }
            } else {
                self.packs.push(pack);
            }
        }
        selected
    }
    pub fn validate(&self) -> Result<()> {
        if self.pages.is_empty() || self.pages.len() > 1000 || self.active_page >= self.pages.len()
        {
            return Err("ページの設定が不正です".into());
        }
        let active = &self.pages[self.active_page];
        if !active.nodes.is_empty() || !active.connectors.is_empty() {
            return Err("編集中のページが重複しています".into());
        }
        self.validate_active()?;
        let mut all = HashSet::new();
        for id in self
            .nodes
            .iter()
            .map(|n| n.id)
            .chain(self.connectors.iter().map(|c| c.id))
            .chain(self.assets.iter().map(|a| a.id))
        {
            all.insert(id);
        }
        for (i, page) in self.pages.iter().enumerate() {
            if page.name.len() > 1024
                || page.name.trim().is_empty()
                || ![page.width, page.height]
                    .iter()
                    .all(|v| v.is_finite() && (16.0..=1e7).contains(v))
            {
                return Err("ページの名前・寸法が不正です".into());
            }
            if i != self.active_page {
                for id in page
                    .nodes
                    .iter()
                    .map(|n| n.id)
                    .chain(page.connectors.iter().map(|c| c.id))
                {
                    if !all.insert(id) {
                        return Err("ページ間で ID が重複しています".into());
                    }
                }
                let view = Document {
                    nodes: page.nodes.clone(),
                    connectors: page.connectors.clone(),
                    assets: self.assets.clone(),
                    font_family: self.font_family.clone(),
                    font_size: self.font_size,
                    grid: self.grid,
                    show_grid: self.show_grid,
                    snap: self.snap,
                    jumps: self.jumps,
                    jump_radius: self.jump_radius,
                    next_id: self.next_id,
                    ..Default::default()
                };
                view.validate_active()?;
            }
        }
        if all.len() > 110_000 {
            return Err("文書全体の要素数が上限を超えています".into());
        }
        if self
            .nodes
            .iter()
            .chain(self.pages.iter().flat_map(|p| &p.nodes))
            .any(|n| n.group.is_some_and(|id| all.contains(&id)))
            || self
                .connectors
                .iter()
                .chain(self.pages.iter().flat_map(|p| &p.connectors))
                .any(|c| c.group.is_some_and(|id| all.contains(&id)))
        {
            return Err("グループの ID が他の要素と重複しています".into());
        }
        let assets: HashSet<_> = self.assets.iter().map(|a| a.id).collect();
        let mut packs = HashSet::new();
        for p in &self.packs {
            if p.id.is_empty()
                || p.id.len() > 1024
                || p.version.is_empty()
                || p.version.len() > 1024
                || p.source.len() > 16384
                || p.terms.len() > 65536
                || p.assets.iter().any(|id| !assets.contains(id))
                || !packs.insert((&p.id, &p.version))
            {
                return Err("素材パックの情報が不正です".into());
            }
        }
        Ok(())
    }
}

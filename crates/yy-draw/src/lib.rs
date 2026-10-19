//! Portable architecture diagrams. Coordinates are independent of display DPI.
pub mod assets;
pub mod catalog;
pub mod drawing;
pub mod editing;
pub mod format;
pub mod geometry;
pub mod jobs;
pub mod library;
mod pages;
pub mod ports;
pub mod render;
pub mod router;

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub type Id = u64;
pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn distance(self, other: Self) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}
impl Rect {
    pub fn center(self) -> Point {
        Point::new(self.x + self.w / 2., self.y + self.h / 2.)
    }
    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x <= self.x + self.w && p.y <= self.y + self.h
    }
    pub fn union(self, b: Self) -> Self {
        let x = self.x.min(b.x);
        let y = self.y.min(b.y);
        Self {
            x,
            y,
            w: (self.x + self.w).max(b.x + b.w) - x,
            h: (self.y + self.h).max(b.y + b.h) - y,
        }
    }
    pub fn expand(self, n: f64) -> Self {
        Self {
            x: self.x - n,
            y: self.y - n,
            w: self.w + 2. * n,
            h: self.h + 2. * n,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shape {
    Rectangle,
    Rounded,
    Ellipse,
    Diamond,
    Triangle,
    Cylinder,
    Cloud,
    Text,
    Frame,
    Icon,
    Junction,
}
pub const SHAPES: &[(Shape, &str)] = &[
    (Shape::Rectangle, "矩形"),
    (Shape::Rounded, "角丸矩形"),
    (Shape::Ellipse, "楕円"),
    (Shape::Diamond, "ひし形"),
    (Shape::Triangle, "三角形"),
    (Shape::Cylinder, "データベース"),
    (Shape::Cloud, "雲"),
    (Shape::Text, "テキスト"),
    (Shape::Frame, "領域枠"),
    (Shape::Junction, "接続点"),
];
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: Id,
    pub shape: Shape,
    pub rect: Rect,
    pub label: String,
    pub fill: u32,
    pub stroke: u32,
    pub line_width: f64,
    pub font_size: f64,
    pub asset: Option<Id>,
    pub group: Option<Id>,
    pub locked: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Endpoint {
    Free(Point),
    Attached { node: Id, side: u8 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Jump {
    Auto,
    Always,
    Never,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Connector {
    pub id: Id,
    pub start: Endpoint,
    pub end: Endpoint,
    pub waypoints: Vec<Point>,
    pub orthogonal: bool,
    pub arrow_start: bool,
    pub arrow_end: bool,
    pub jump: Jump,
    pub color: u32,
    pub width: f64,
    pub dashed: bool,
    pub label: String,
    /// Stored in the CRNR extension chunk to preserve older postcard layouts.
    #[serde(skip)]
    pub corner_radius: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: Id,
    pub name: String,
    pub source: String,
    pub svg: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub nodes: Vec<Node>,
    pub connectors: Vec<Connector>,
    pub assets: Vec<Asset>,
    pub font_family: String,
    pub font_size: f64,
    pub grid: f64,
    pub show_grid: bool,
    pub snap: bool,
    pub jumps: bool,
    pub jump_radius: f64,
    pub next_id: Id,
    pub pages: Vec<Page>,
    pub active_page: usize,
    pub packs: Vec<library::Pack>,
    #[serde(skip)]
    pub extra_chunks: Vec<format::Chunk>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub name: String,
    pub width: f64,
    pub height: f64,
    pub nodes: Vec<Node>,
    pub connectors: Vec<Connector>,
}
impl Default for Page {
    fn default() -> Self {
        Self {
            name: "ページ 1".into(),
            width: 1122.,
            height: 794.,
            nodes: vec![],
            connectors: vec![],
        }
    }
}
impl Default for Document {
    fn default() -> Self {
        Self {
            nodes: vec![],
            connectors: vec![],
            assets: vec![],
            font_family: "UDEV Gothic".into(),
            font_size: 14.6667,
            grid: 8.,
            show_grid: true,
            snap: true,
            jumps: true,
            jump_radius: 5.,
            next_id: 1,
            pages: vec![Page::default()],
            active_page: 0,
            packs: vec![],
            extra_chunks: vec![],
        }
    }
}
impl Document {
    pub fn id(&mut self) -> Id {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
    pub fn snap_point(&self, p: Point) -> Point {
        if self.snap {
            Point::new(
                (p.x / self.grid).round() * self.grid,
                (p.y / self.grid).round() * self.grid,
            )
        } else {
            p
        }
    }
    pub fn node(&self, id: Id) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub fn add_node(&mut self, shape: Shape, point: Point, asset: Option<Id>) -> Id {
        let id = self.id();
        let point = self.snap_point(point);
        let label = asset
            .and_then(|id| self.assets.iter().find(|a| a.id == id))
            .map(|a| a.name.clone())
            .unwrap_or_default();
        self.nodes.push(Node {
            id,
            shape,
            rect: Rect {
                x: point.x - if shape == Shape::Junction { 4. } else { 0. },
                y: point.y - if shape == Shape::Junction { 4. } else { 0. },
                w: if shape == Shape::Junction {
                    8.
                } else if shape == Shape::Icon {
                    80.
                } else {
                    144.
                },
                h: if shape == Shape::Junction {
                    8.
                } else if shape == Shape::Icon {
                    96.
                } else {
                    80.
                },
            },
            label,
            fill: 0xf0f6ff,
            stroke: 0x345782,
            line_width: 1.5,
            font_size: self.font_size,
            asset,
            group: None,
            locked: false,
        });
        id
    }
    pub fn connect(&mut self, start: Endpoint, end: Endpoint, orthogonal: bool) -> Id {
        let id = self.id();
        self.connectors.push(Connector {
            id,
            start,
            end,
            waypoints: vec![],
            orthogonal,
            arrow_start: false,
            arrow_end: true,
            jump: Jump::Auto,
            color: 0x345782,
            width: 1.5,
            dashed: false,
            label: String::new(),
            corner_radius: 0.,
        });
        id
    }
    pub fn endpoint(&self, e: Endpoint) -> Point {
        match e {
            Endpoint::Free(p) => p,
            Endpoint::Attached { node, side } => self
                .node(node)
                .map(|n| ports::point(n, side))
                .unwrap_or_default(),
        }
    }
    pub fn attach(&self, p: Point) -> Endpoint {
        self.attach_near(p, 8.)
    }
    pub fn delete(&mut self, selected: &[Id]) {
        // Keep the visible end of a connection when its attached shape is deleted.
        let points: Vec<_> = self
            .connectors
            .iter()
            .map(|c| (self.endpoint(c.start), self.endpoint(c.end)))
            .collect();
        for (c, (a, b)) in self.connectors.iter_mut().zip(points) {
            if matches!(c.start,Endpoint::Attached{node,..} if selected.contains(&node)) {
                c.start = Endpoint::Free(a)
            }
            if matches!(c.end,Endpoint::Attached{node,..} if selected.contains(&node)) {
                c.end = Endpoint::Free(b)
            }
        }
        self.nodes.retain(|n| !selected.contains(&n.id));
        self.connectors.retain(|c| !selected.contains(&c.id));
    }
    pub fn translate(&mut self, ids: &[Id], dx: f64, dy: f64) {
        for n in &mut self.nodes {
            if ids.contains(&n.id) && !n.locked {
                n.rect.x += dx;
                n.rect.y += dy;
            }
        }
        for c in &mut self.connectors {
            if ids.contains(&c.id) {
                for e in [&mut c.start, &mut c.end] {
                    if let Endpoint::Free(p) = e {
                        p.x += dx;
                        p.y += dy;
                    }
                }
                for p in &mut c.waypoints {
                    p.x += dx;
                    p.y += dy;
                }
            }
        }
    }
    pub fn duplicate(&mut self, ids: &[Id]) -> Vec<Id> {
        let mut map = std::collections::HashMap::new();
        let mut groups = std::collections::HashMap::new();
        let mut copied = Vec::new();
        for mut n in self.nodes.clone() {
            if ids.contains(&n.id) {
                let old = n.id;
                n.id = self.id();
                map.insert(old, n.id);
                if let Some(group) = n.group {
                    let new = *groups.entry(group).or_insert_with(|| self.id());
                    n.group = Some(new);
                }
                n.rect.x += 16.;
                n.rect.y += 16.;
                copied.push(n.id);
                self.nodes.push(n);
            }
        }
        for mut c in self.connectors.clone() {
            let internal = matches!((c.start,c.end),(Endpoint::Attached{node:a,..},Endpoint::Attached{node:b,..}) if map.contains_key(&a)&&map.contains_key(&b));
            if ids.contains(&c.id) || internal {
                c.id = self.id();
                for e in [&mut c.start, &mut c.end] {
                    match e {
                        Endpoint::Attached { node, .. } => {
                            if let Some(id) = map.get(node) {
                                *node = *id;
                            }
                        }
                        Endpoint::Free(p) => {
                            p.x += 16.;
                            p.y += 16.;
                        }
                    }
                }
                for p in &mut c.waypoints {
                    p.x += 16.;
                    p.y += 16.;
                }
                copied.push(c.id);
                self.connectors.push(c);
            }
        }
        copied
    }
    pub fn bounds(&self) -> Rect {
        let mut bounds = None;
        for r in self
            .nodes
            .iter()
            .map(|n| n.rect.expand(n.font_size * 2. + n.line_width))
            .chain(
                router::Router::new(self)
                    .all()
                    .into_iter()
                    .flat_map(|r| r.points)
                    .map(|p| {
                        Rect {
                            x: p.x,
                            y: p.y,
                            w: 0.,
                            h: 0.,
                        }
                        .expand(self.jump_radius + 20.)
                    }),
            )
        {
            bounds = Some(bounds.map_or(r, |b: Rect| b.union(r)));
        }
        bounds
            .unwrap_or(Rect {
                x: 0.,
                y: 0.,
                w: 800.,
                h: 600.,
            })
            .expand(16.)
    }
    fn validate_active(&self) -> Result<()> {
        if self.next_id == 0 || self.next_id > 1_000_000_000_000 {
            return Err("ID の採番状態が不正です".into());
        }
        if self.nodes.len() > 100_000
            || self.connectors.len() > 100_000
            || self.assets.len() > 10_000
        {
            return Err("図の要素数が上限を超えています".into());
        }
        let finite = |v: f64| v.is_finite() && v.abs() <= 1e7;
        if !finite(self.grid)
            || self.grid < 1.
            || self.grid > 1000.
            || !finite(self.jump_radius)
            || !(0.5..=100.).contains(&self.jump_radius)
            || !finite(self.font_size)
            || !(4.0..=512.).contains(&self.font_size)
            || self.font_family.len() > 1024
        {
            return Err("文書の設定が不正です".into());
        }
        let mut ids = HashSet::new();
        for id in self
            .nodes
            .iter()
            .map(|n| n.id)
            .chain(self.connectors.iter().map(|c| c.id))
            .chain(self.assets.iter().map(|a| a.id))
        {
            if id == 0 || id >= self.next_id || !ids.insert(id) {
                return Err("要素の ID が不正です".into());
            }
        }
        for n in &self.nodes {
            if n.group
                .is_some_and(|id| id == 0 || id >= self.next_id || ids.contains(&id))
            {
                return Err("グループの ID が不正です".into());
            }
            let r = n.rect;
            if ![r.x, r.y, r.w, r.h, n.font_size, n.line_width]
                .into_iter()
                .all(finite)
                || r.w < 1.
                || r.h < 1.
                || !(4.0..=512.).contains(&n.font_size)
                || !(0.1..=100.).contains(&n.line_width)
                || n.label.len() > 65536
            {
                return Err("図形の寸法・文字が不正です".into());
            }
            if n.asset
                .is_some_and(|id| !self.assets.iter().any(|a| a.id == id))
            {
                return Err("素材が見つかりません".into());
            }
        }
        for c in &self.connectors {
            for e in [c.start, c.end] {
                match e {
                    Endpoint::Free(p) if !finite(p.x) || !finite(p.y) => {
                        return Err("接続位置が不正です".into());
                    }
                    Endpoint::Attached { node, side }
                        if self.node(node).is_none() || side >= ports::COUNT =>
                    {
                        return Err("接続先が不正です".into());
                    }
                    _ => {}
                }
            }
            if c.waypoints.len() > 1000
                || c.waypoints.iter().any(|p| !finite(p.x) || !finite(p.y))
                || !(0.1..=100.).contains(&c.width)
                || !c.corner_radius.is_finite()
                || !(0.0..=10000.).contains(&c.corner_radius)
                || c.label.len() > 65536
            {
                return Err("接続線が不正です".into());
            }
        }
        for a in &self.assets {
            if a.svg.len() > 8 * 1024 * 1024 || a.name.len() > 4096 || a.source.len() > 16384 {
                return Err("素材が大きすぎます".into());
            }
            render::validate_svg(&a.svg)?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct History {
    undo: Vec<Document>,
    redo: Vec<Document>,
}
impl History {
    pub fn commit(&mut self, before: Document, after: &Document) {
        if &before != after {
            self.undo.push(before);
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }
    pub fn undo(&mut self, doc: &mut Document) {
        if let Some(d) = self.undo.pop() {
            self.redo.push(std::mem::replace(doc, d));
        }
    }
    pub fn redo(&mut self, doc: &mut Document) {
        if let Some(d) = self.redo.pop() {
            self.undo.push(std::mem::replace(doc, d));
        }
    }
}

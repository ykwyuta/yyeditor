use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pencil,
    Eraser,
    Fill,
    Picker,
    Line,
    Rectangle,
    Ellipse,
    Selection,
    Thin,
    RestoreDepth,
}
impl Tool {
    pub const ALL: [Self; 10] = [
        Self::Pencil,
        Self::Eraser,
        Self::Fill,
        Self::Picker,
        Self::Line,
        Self::Rectangle,
        Self::Ellipse,
        Self::Selection,
        Self::Thin,
        Self::RestoreDepth,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Pencil => "鉛筆",
            Self::Eraser => "消しゴム",
            Self::Fill => "塗りつぶし",
            Self::Picker => "スポイト",
            Self::Line => "直線",
            Self::Rectangle => "四角形",
            Self::Ellipse => "楕円",
            Self::Selection => "範囲選択",
            Self::Thin => "厚みを削る (T)",
            Self::RestoreDepth => "厚みを戻す (U)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}
impl Rect {
    pub fn between(a: [i32; 2], b: [i32; 2]) -> Self {
        Self {
            x: a[0].min(b[0]),
            y: a[1].min(b[1]),
            w: a[0].abs_diff(b[0]) + 1,
            h: a[1].abs_diff(b[1]) + 1,
        }
    }
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x
            && y >= self.y
            && x < i64::from(self.x).saturating_add(i64::from(self.w)) as i32
            && y < i64::from(self.y).saturating_add(i64::from(self.h)) as i32
    }
}
pub fn dab(view: &mut View, x: i32, y: i32, color: Color, size: u32, clip: Option<Rect>) {
    let half = (size / 2) as i32;
    for dy in 0..size as i32 {
        for dx in 0..size as i32 {
            put(view, x + dx - half, y + dy - half, color, clip);
        }
    }
}
pub fn put(view: &mut View, x: i32, y: i32, color: Color, clip: Option<Rect>) {
    if x >= 0
        && y >= 0
        && x < view.width as i32
        && y < view.height as i32
        && clip.is_none_or(|r| r.contains(x, y))
    {
        view.layers[view.active].pixels[(y as u32 * view.width + x as u32) as usize] = color;
    }
}
pub fn line(
    view: &mut View,
    a: [i32; 2],
    b: [i32; 2],
    color: Color,
    size: u32,
    clip: Option<Rect>,
) {
    for [x, y] in line_points(a, b) {
        dab(view, x, y, color, size, clip);
    }
}
pub fn line_points(a: [i32; 2], b: [i32; 2]) -> Vec<[i32; 2]> {
    let mut points = vec![];
    let [mut x, mut y] = a;
    let [tx, ty] = b;
    let dx = (tx - x).abs();
    let dy = -(ty - y).abs();
    let sx = if x < tx { 1 } else { -1 };
    let sy = if y < ty { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        points.push([x, y]);
        if x == tx && y == ty {
            break;
        }
        let e = err * 2;
        if e >= dy {
            err += dy;
            x += sx;
        }
        if e <= dx {
            err += dx;
            y += sy;
        }
    }
    points
}
pub fn shape(
    view: &mut View,
    tool: Tool,
    endpoints: [[i32; 2]; 2],
    color: Color,
    size: u32,
    filled: bool,
    clip: Option<Rect>,
) {
    let [a, b] = endpoints;
    if tool == Tool::Line {
        line(view, a, b, color, size, clip);
        return;
    }
    let r = Rect::between(a, b);
    if tool == Tool::Rectangle {
        if filled {
            for y in r.y..r.y + r.h as i32 {
                for x in r.x..r.x + r.w as i32 {
                    put(view, x, y, color, clip);
                }
            }
        } else {
            let [x, y] = [r.x, r.y];
            let [rx, by] = [x + r.w as i32 - 1, y + r.h as i32 - 1];
            for (p, q) in [
                ([x, y], [rx, y]),
                ([rx, y], [rx, by]),
                ([rx, by], [x, by]),
                ([x, by], [x, y]),
            ] {
                line(view, p, q, color, size, clip);
            }
        }
    } else if tool == Tool::Ellipse {
        let rx = r.w as f32 / 2.;
        let ry = r.h as f32 / 2.;
        let cx = r.x as f32 + rx;
        let cy = r.y as f32 + ry;
        let inside = |x: f32, y: f32| ((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2) <= 1.;
        for y in r.y..r.y + r.h as i32 {
            for x in r.x..r.x + r.w as i32 {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                if inside(fx, fy)
                    && (filled
                        || [(fx - 1., fy), (fx + 1., fy), (fx, fy - 1.), (fx, fy + 1.)]
                            .iter()
                            .any(|&(a, b)| !inside(a, b)))
                {
                    dab(view, x, y, color, if filled { 1 } else { size }, clip);
                }
            }
        }
    }
}
pub fn fill(view: &mut View, start: [i32; 2], color: Color, clip: Option<Rect>) {
    let [x, y] = start;
    if x < 0
        || y < 0
        || x >= view.width as i32
        || y >= view.height as i32
        || clip.is_some_and(|r| !r.contains(x, y))
    {
        return;
    }
    let old = view.layers[view.active].pixels[(y as u32 * view.width + x as u32) as usize];
    if old == color {
        return;
    }
    let mut queue = VecDeque::from([(x, y)]);
    put(view, x, y, color, clip);
    while let Some((x, y)) = queue.pop_front() {
        for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
            if nx >= 0
                && ny >= 0
                && nx < view.width as i32
                && ny < view.height as i32
                && clip.is_none_or(|r| r.contains(nx, ny))
                && view.layers[view.active].pixels[(ny as u32 * view.width + nx as u32) as usize]
                    == old
            {
                put(view, nx, ny, color, clip);
                queue.push_back((nx, ny));
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Clip {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<Color>,
}
impl Clip {
    pub fn capture(view: &View, r: Rect) -> Self {
        let mut pixels = vec![CLEAR; (r.w * r.h) as usize];
        for y in 0..r.h {
            for x in 0..r.w {
                let (sx, sy) = (r.x + x as i32, r.y + y as i32);
                if sx >= 0 && sy >= 0 && sx < view.width as i32 && sy < view.height as i32 {
                    pixels[(y * r.w + x) as usize] = view.layers[view.active].pixels
                        [(sy as u32 * view.width + sx as u32) as usize];
                }
            }
        }
        Self {
            width: r.w,
            height: r.h,
            pixels,
        }
    }
    pub fn validate(&self) -> io::Result<()> {
        if self.width == 0
            || self.height == 0
            || self.width > MAX_SIDE
            || self.height > MAX_SIDE
            || self.pixels.len() != (self.width * self.height) as usize
        {
            Err(crate::invalid("クリップボードの画素が不正です"))
        } else {
            Ok(())
        }
    }
    pub fn paste(&self, view: &mut View, at: [i32; 2]) {
        for y in 0..self.height {
            for x in 0..self.width {
                put(
                    view,
                    at[0] + x as i32,
                    at[1] + y as i32,
                    self.pixels[(y * self.width + x) as usize],
                    None,
                );
            }
        }
    }
}
pub fn flip(view: &mut View, horizontal: bool, r: Rect) {
    let clip = Clip::capture(view, r);
    for y in 0..r.h {
        for x in 0..r.w {
            let (sx, sy) = if horizontal {
                (r.w - 1 - x, y)
            } else {
                (x, r.h - 1 - y)
            };
            put(
                view,
                r.x + x as i32,
                r.y + y as i32,
                clip.pixels[(sy * r.w + sx) as usize],
                None,
            );
        }
    }
}
pub fn rotate(view: &mut View, r: Rect) {
    let clip = Clip::capture(view, r);
    for y in 0..r.h {
        for x in 0..r.w {
            put(view, r.x + x as i32, r.y + y as i32, CLEAR, None);
        }
    }
    for y in 0..clip.height {
        for x in 0..clip.width {
            put(
                view,
                r.x + clip.height as i32 - 1 - y as i32,
                r.y + x as i32,
                clip.pixels[(y * clip.width + x) as usize],
                None,
            );
        }
    }
}

pub struct Editor {
    pub project: Project,
    undo: Vec<Project>,
    redo: Vec<Project>,
}
impl Default for Editor {
    fn default() -> Self {
        Self::new(Project::default())
    }
}
impl Editor {
    pub fn new(project: Project) -> Self {
        Self {
            project,
            undo: vec![],
            redo: vec![],
        }
    }
    pub fn change(&mut self, f: impl FnOnce(&mut Project) -> io::Result<()>) -> io::Result<bool> {
        let old = self.project.clone();
        if let Err(e) = f(&mut self.project).and_then(|()| self.project.validate()) {
            self.project = old;
            return Err(e);
        }
        Ok(self.commit(old))
    }
    pub fn commit(&mut self, old: Project) -> bool {
        if old == self.project {
            return false;
        }
        self.undo.push(old);
        self.redo.clear();
        self.trim();
        true
    }
    fn trim(&mut self) {
        let mut bytes: usize = self.undo.iter().chain(&self.redo).map(Project::bytes).sum();
        while self.undo.len() + self.redo.len() > 100 || bytes > 64 * 1024 * 1024 {
            let p = if !self.undo.is_empty() {
                self.undo.remove(0)
            } else {
                self.redo.remove(0)
            };
            bytes = bytes.saturating_sub(p.bytes());
        }
    }
    pub fn undo(&mut self) -> bool {
        if let Some(p) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.project, p));
            true
        } else {
            false
        }
    }
    pub fn redo(&mut self) -> bool {
        if let Some(p) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.project, p));
            true
        } else {
            false
        }
    }
}

use crate::invalid;
use serde::{Deserialize, Serialize};
use std::io;

pub type Color = [u8; 4];
pub const CLEAR: Color = [0; 4];
pub const MAX_SIDE: u32 = 256;
pub const MAX_VOXELS: usize = 2_097_152;
pub const MAX_PIXELS: usize = 4_194_304;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
}
impl Direction {
    pub const ALL: [Self; 6] = [
        Self::Front,
        Self::Back,
        Self::Left,
        Self::Right,
        Self::Top,
        Self::Bottom,
    ];
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|d| *d == self).unwrap()
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Front => "正面",
            Self::Back => "背面",
            Self::Left => "左面",
            Self::Right => "右面",
            Self::Top => "上面",
            Self::Bottom => "下面",
        }
    }
    pub fn size(self, size: [u32; 3]) -> [u32; 2] {
        let [w, h, d] = size;
        match self {
            Self::Front | Self::Back => [w, h],
            Self::Left | Self::Right => [d, h],
            Self::Top | Self::Bottom => [w, d],
        }
    }
    pub fn ray_length(self, size: [u32; 3]) -> u32 {
        match self {
            Self::Front | Self::Back => size[2],
            Self::Left | Self::Right => size[0],
            Self::Top | Self::Bottom => size[1],
        }
    }
    pub fn ray_point(self, [u, v]: [u32; 2], t: u32, [w, h, d]: [u32; 3]) -> [u32; 3] {
        match self {
            Self::Front => [u, h - 1 - v, t],
            Self::Back => [w - 1 - u, h - 1 - v, d - 1 - t],
            Self::Left => [t, h - 1 - v, d - 1 - u],
            Self::Right => [w - 1 - t, h - 1 - v, u],
            Self::Top => [u, h - 1 - t, v],
            Self::Bottom => [u, t, d - 1 - v],
        }
    }
    /// Each view is seen from outside the object. Image y points down.
    pub fn project(self, [x, y, z]: [u32; 3], [w, h, d]: [u32; 3]) -> [u32; 2] {
        match self {
            Self::Front => [x, h - 1 - y],
            Self::Back => [w - 1 - x, h - 1 - y],
            Self::Left => [d - 1 - z, h - 1 - y],
            Self::Right => [z, h - 1 - y],
            Self::Top => [x, z],
            Self::Bottom => [x, d - 1 - z],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub name: String,
    pub visible: bool,
    pub opacity: u8,
    pub pixels: Vec<Color>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub width: u32,
    pub height: u32,
    pub layers: Vec<Layer>,
    pub active: usize,
}
impl View {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            width: w,
            height: h,
            active: 0,
            layers: vec![Layer {
                name: "レイヤー 1".into(),
                visible: true,
                opacity: 255,
                pixels: vec![CLEAR; (w * h) as usize],
            }],
        }
    }
    pub fn composite(&self) -> Vec<Color> {
        let mut result = vec![CLEAR; (self.width * self.height) as usize];
        for layer in self.layers.iter().filter(|l| l.visible) {
            for (dst, src) in result.iter_mut().zip(&layer.pixels) {
                *dst = over(*dst, *src, layer.opacity);
            }
        }
        result
    }
    pub fn resize(&mut self, w: u32, h: u32, scale: bool) {
        for layer in &mut self.layers {
            let mut next = vec![CLEAR; (w * h) as usize];
            for y in 0..h {
                for x in 0..w {
                    let (sx, sy) = if scale {
                        (x * self.width / w, y * self.height / h)
                    } else {
                        (x, y)
                    };
                    if sx < self.width && sy < self.height {
                        next[(y * w + x) as usize] = layer.pixels[(sy * self.width + sx) as usize];
                    }
                }
            }
            layer.pixels = next;
        }
        self.width = w;
        self.height = h;
    }
    pub fn bytes(&self) -> usize {
        self.layers.iter().map(|l| l.pixels.len() * 4).sum()
    }
}

pub fn over(dst: Color, src: Color, opacity: u8) -> Color {
    let sa = u32::from(src[3]) * u32::from(opacity) / 255;
    let da = u32::from(dst[3]);
    let a = sa + da * (255 - sa) / 255;
    if a == 0 {
        return CLEAR;
    }
    let mut out = [0; 4];
    for c in 0..3 {
        out[c] = ((u32::from(src[c]) * sa + u32::from(dst[c]) * da * (255 - sa) / 255) / a) as u8;
    }
    out[3] = a as u8;
    out
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Object {
    pub name: String,
    pub size: [u32; 3],
    pub views: [Option<View>; 6],
    /// 0 = sharp cube, 1 = maximum rounded dot. Original pixels stay editable.
    pub roundness: f32,
    pub alpha_cutoff: u8,
    /// Sorted unique voxel indices removed by thickness editing. Missing in old files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub carved: Vec<u32>,
}
impl Object {
    pub fn new(name: &str, size: [u32; 3]) -> io::Result<Self> {
        validate_size(size)?;
        let mut views = std::array::from_fn(|_| None);
        views[0] = Some(View::new(size[0], size[1]));
        Ok(Self {
            name: name.into(),
            size,
            views,
            roundness: 0.,
            alpha_cutoff: 128,
            carved: vec![],
        })
    }
    pub fn enable(&mut self, direction: Direction) -> &mut View {
        let [w, h] = direction.size(self.size);
        self.views[direction.index()].get_or_insert_with(|| View::new(w, h))
    }
    pub fn resize(&mut self, size: [u32; 3], scale: bool) -> io::Result<()> {
        validate_size(size)?;
        let prospective: usize = Direction::ALL
            .iter()
            .filter_map(|d| {
                self.views[d.index()].as_ref().map(|v| {
                    let [w, h] = d.size(size);
                    (w * h) as usize * v.layers.len()
                })
            })
            .sum();
        if prospective > MAX_PIXELS {
            return Err(invalid("レイヤーの総画素数が上限を超えます"));
        }
        if !self.carved.is_empty() {
            let old = self.size;
            let mut mask = vec![false; old.iter().product::<u32>() as usize];
            for &i in &self.carved {
                if let Some(v) = mask.get_mut(i as usize) {
                    *v = true;
                }
            }
            let mut next = vec![];
            for z in 0..size[2] {
                for y in 0..size[1] {
                    for x in 0..size[0] {
                        let source = if scale {
                            [
                                x * old[0] / size[0],
                                old[1] - 1 - (size[1] - 1 - y) * old[1] / size[1],
                                z * old[2] / size[2],
                            ]
                            .map(|v| v as i32)
                        } else {
                            [
                                x as i32,
                                y as i32 + old[1] as i32 - size[1] as i32,
                                z as i32,
                            ]
                        };
                        if source
                            .iter()
                            .zip(old)
                            .all(|(&v, n)| v >= 0 && (v as u32) < n)
                        {
                            let [sx, sy, sz] = source.map(|v| v as u32);
                            if mask[((sz * old[1] + sy) * old[0] + sx) as usize] {
                                next.push((z * size[1] + y) * size[0] + x);
                            }
                        }
                    }
                }
            }
            self.carved = next;
        }
        for dir in Direction::ALL {
            if let Some(v) = &mut self.views[dir.index()] {
                let [w, h] = dir.size(size);
                v.resize(w, h, scale);
            }
        }
        self.size = size;
        Ok(())
    }
    pub fn bytes(&self) -> usize {
        self.views.iter().flatten().map(View::bytes).sum::<usize>() + self.carved.len() * 4
    }
}
pub fn validate_size(size: [u32; 3]) -> io::Result<()> {
    if size.iter().any(|&v| v == 0 || v > MAX_SIDE)
        || size.iter().map(|&v| v as usize).product::<usize>() > MAX_VOXELS
    {
        return Err(invalid(
            "各辺は1〜256、幅×高さ×厚みは2,097,152以下にしてください",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub version: u32,
    pub objects: Vec<Object>,
    pub active: usize,
    pub palette: Vec<Color>,
}
impl Default for Project {
    fn default() -> Self {
        Self {
            version: 1,
            objects: vec![Object::new("オブジェクト 1", [32, 32, 8]).unwrap()],
            active: 0,
            palette: palette(),
        }
    }
}
impl Project {
    pub fn object(&self) -> &Object {
        &self.objects[self.active]
    }
    pub fn object_mut(&mut self) -> &mut Object {
        &mut self.objects[self.active]
    }
    pub fn bytes(&self) -> usize {
        self.objects.iter().map(Object::bytes).sum()
    }
    pub fn validate(&self) -> io::Result<()> {
        if self.version != 1
            || self.objects.is_empty()
            || self.objects.len() > 64
            || self.active >= self.objects.len()
            || self.palette.len() > 256
            || self
                .objects
                .iter()
                .flat_map(|o| o.views.iter().flatten())
                .map(|v| v.bytes() / 4)
                .sum::<usize>()
                > MAX_PIXELS
            || self.objects.iter().map(|o| o.carved.len()).sum::<usize>() > MAX_VOXELS
        {
            return Err(invalid(
                "プロジェクトのバージョン・オブジェクト数・画素数・パレットが不正です",
            ));
        }
        for o in &self.objects {
            validate_size(o.size)?;
            let voxels = o.size.iter().product::<u32>();
            if o.carved.last().is_some_and(|&i| i >= voxels)
                || o.carved.windows(2).any(|p| p[0] >= p[1])
            {
                return Err(invalid("削った厚みの位置が不正です"));
            }
            if o.name.trim().is_empty() || o.name.chars().count() > 256 {
                return Err(invalid("オブジェクト名は1〜256文字です"));
            }
            if !o.roundness.is_finite()
                || !(0. ..=1.).contains(&o.roundness)
                || o.views.iter().all(Option::is_none)
            {
                return Err(invalid("丸みまたは方向データが不正です"));
            }
            for dir in Direction::ALL {
                if let Some(v) = &o.views[dir.index()]
                    && ([v.width, v.height] != dir.size(o.size)
                        || v.layers.is_empty()
                        || v.layers.len() > 32
                        || v.active >= v.layers.len()
                        || v.layers.iter().any(|l| {
                            l.pixels.len() != (v.width * v.height) as usize
                                || l.name.trim().is_empty()
                                || l.name.chars().count() > 256
                        }))
                {
                    return Err(invalid("方向の寸法・レイヤーが不正です"));
                }
            }
        }
        Ok(())
    }
}
pub fn palette() -> Vec<Color> {
    let colors = [
        0x172033, 0xffffff, 0x64748b, 0xcbd5e1, 0xef4444, 0xfb923c, 0xfacc15, 0xa3e635, 0x22c55e,
        0x14b8a6, 0x22d3ee, 0x3b82f6, 0x6366f1, 0xa855f7, 0xec4899, 0xfda4af, 0x7f1d1d, 0x9a3412,
        0x854d0e, 0x365314, 0x14532d, 0x134e4a, 0x164e63, 0x1e3a8a, 0x312e81, 0x581c87, 0x831843,
        0x713f12, 0xe7c6a5, 0xa36f4a, 0x525252, 0xa3a3a3,
    ];
    colors
        .iter()
        .map(|&c| [(c >> 16) as u8, (c >> 8) as u8, c as u8, 255])
        .collect()
}

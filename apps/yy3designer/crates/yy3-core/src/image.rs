use crate::*;
use std::{
    collections::BTreeMap,
    io::{self, BufReader, Write},
    path::Path,
};

pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<Color>,
}
impl Raster {
    pub fn validate(&self) -> io::Result<()> {
        if self.width == 0
            || self.height == 0
            || self.width > 16384
            || self.height > 16384
            || u64::from(self.width) * u64::from(self.height) > 40_000_000
            || self.pixels.len() != self.width as usize * self.height as usize
        {
            Err(crate::invalid(
                "画像は4,000万画素・各辺16,384以下にしてください",
            ))
        } else {
            Ok(())
        }
    }
}
pub fn read_png(path: &Path) -> io::Result<Raster> {
    let mut decoder = png::Decoder::new_with_limits(
        BufReader::new(std::fs::File::open(path)?),
        png::Limits {
            bytes: 200 * 1024 * 1024,
        },
    );
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(io::Error::other)?;
    let info = reader.info();
    if info.width > 16384
        || info.height > 16384
        || u64::from(info.width) * u64::from(info.height) > 40_000_000
    {
        return Err(crate::invalid("画像が大きすぎます"));
    }
    let mut bytes = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or_else(|| crate::invalid("画像が大きすぎます"))?
    ];
    let info = reader.next_frame(&mut bytes).map_err(io::Error::other)?;
    let pixels = bytes[..info.buffer_size()]
        .chunks_exact(info.color_type.samples())
        .map(|p| match info.color_type {
            png::ColorType::Rgba => [p[0], p[1], p[2], p[3]],
            png::ColorType::Rgb => [p[0], p[1], p[2], 255],
            png::ColorType::Grayscale => [p[0], p[0], p[0], 255],
            png::ColorType::GrayscaleAlpha => [p[0], p[0], p[0], p[1]],
            _ => unreachable!(),
        })
        .collect();
    Ok(Raster {
        width: info.width,
        height: info.height,
        pixels,
    })
}
pub fn write_png(path: &Path, image: &Raster) -> io::Result<()> {
    image.validate()?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut encoder = png::Encoder::new(&mut file, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(io::Error::other)?;
        let bytes: Vec<u8> = image.pixels.iter().flatten().copied().collect();
        writer.write_image_data(&bytes).map_err(io::Error::other)?;
        writer.finish().map_err(io::Error::other)?;
    }
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sampling {
    Nearest,
    Area,
}
pub struct Convert {
    pub sampling: Sampling,
    pub colors: usize,
    pub dither: bool,
}
impl Default for Convert {
    fn default() -> Self {
        Self {
            sampling: Sampling::Area,
            colors: 32,
            dither: false,
        }
    }
}
/// Median-cut palette in RGB; transparent pixels are excluded from the palette.
pub fn quantize(pixels: &[Color], count: usize) -> Vec<Color> {
    let mut histogram = BTreeMap::<[u8; 3], usize>::new();
    for p in pixels.iter().filter(|p| p[3] != 0) {
        *histogram.entry([p[0], p[1], p[2]]).or_default() += 1;
    }
    let mut boxes: Vec<Vec<([u8; 3], usize)>> = vec![histogram.into_iter().collect()];
    let range = |b: &[([u8; 3], usize)]| {
        (0..3)
            .map(|c| {
                let lo = b.iter().map(|(p, _)| p[c]).min().unwrap_or(0);
                let hi = b.iter().map(|(p, _)| p[c]).max().unwrap_or(0);
                (hi - lo, c)
            })
            .max()
            .unwrap()
    };
    while boxes.len() < count.clamp(2, 256) {
        let next = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.len() > 1)
            .max_by_key(|(_, b)| usize::from(range(b).0) * b.iter().map(|(_, n)| n).sum::<usize>())
            .map(|(i, _)| i);
        let Some(i) = next else {
            break;
        };
        let mut b = boxes.swap_remove(i);
        let axis = range(&b).1;
        b.sort_by_key(|(p, _)| p[axis]);
        let total: usize = b.iter().map(|(_, n)| n).sum();
        let mut n = 0;
        let mut cut = 1;
        for (i, (_, v)) in b.iter().enumerate().take(b.len() - 1) {
            n += v;
            cut = i + 1;
            if n * 2 >= total {
                break;
            }
        }
        let tail = b.split_off(cut);
        boxes.push(b);
        boxes.push(tail);
    }
    boxes
        .iter()
        .filter(|b| !b.is_empty())
        .map(|b| {
            let n: usize = b.iter().map(|(_, n)| n).sum();
            let mut color = [0; 4];
            for c in 0..3 {
                color[c] = (b.iter().map(|(p, n)| usize::from(p[c]) * n).sum::<usize>() / n) as u8;
            }
            color[3] = 255;
            color
        })
        .collect()
}
pub fn convert(
    source: &Raster,
    width: u32,
    height: u32,
    settings: &Convert,
) -> io::Result<(Vec<Color>, Vec<Color>)> {
    source.validate()?;
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(crate::invalid("変換後の画像は各辺1〜256です"));
    }
    let mut pixels = vec![CLEAR; (width * height) as usize];
    for y in 0..height {
        for x in 0..width {
            let dst = &mut pixels[(y * width + x) as usize];
            if settings.sampling == Sampling::Nearest {
                *dst = source.pixels[((y * source.height / height) * source.width
                    + x * source.width / width) as usize];
                continue;
            }
            let [x0, x1] = [
                x as f64 * source.width as f64 / width as f64,
                (x + 1) as f64 * source.width as f64 / width as f64,
            ];
            let [y0, y1] = [
                y as f64 * source.height as f64 / height as f64,
                (y + 1) as f64 * source.height as f64 / height as f64,
            ];
            let mut acc = [0.; 4];
            let mut total = 0.;
            for sy in y0.floor() as u32..(y1.ceil() as u32).min(source.height) {
                for sx in x0.floor() as u32..(x1.ceil() as u32).min(source.width) {
                    let weight = ((sx + 1) as f64).min(x1) - (sx as f64).max(x0);
                    let wy = ((sy + 1) as f64).min(y1) - (sy as f64).max(y0);
                    let w = weight * wy;
                    let p = source.pixels[(sy * source.width + sx) as usize];
                    let a = f64::from(p[3]) / 255.;
                    for c in 0..3 {
                        acc[c] += f64::from(p[c]) * a * w;
                    }
                    acc[3] += a * w;
                    total += w;
                }
            }
            if acc[3] > 0. {
                for c in 0..3 {
                    dst[c] = (acc[c] / acc[3]).round() as u8;
                }
                dst[3] = (acc[3] / total * 255.).round() as u8;
            }
        }
    }
    if settings.colors == 0 {
        return Ok((pixels.clone(), quantize(&pixels, 256)));
    }
    let palette = quantize(&pixels, settings.colors);
    if !palette.is_empty() {
        let mut errors = vec![[0f32; 3]; pixels.len()];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let i = y * width as usize + x;
                if pixels[i][3] == 0 {
                    continue;
                }
                let adjusted = std::array::from_fn::<_, 3, _>(|c| {
                    (f32::from(pixels[i][c]) + errors[i][c]).clamp(0., 255.)
                });
                let best = *palette
                    .iter()
                    .min_by(|a, b| {
                        let dist = |p: &&Color| {
                            (0..3)
                                .map(|c| (adjusted[c] - f32::from(p[c])).powi(2))
                                .sum::<f32>()
                        };
                        dist(a).total_cmp(&dist(b))
                    })
                    .unwrap();
                if settings.dither {
                    for (dx, dy, weight) in [(1, 0, 7.), (-1, 1, 3.), (0, 1, 5.), (1, 1, 1.)] {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx >= 0 && nx < width as i32 && ny < height as i32 {
                            let e = &mut errors[ny as usize * width as usize + nx as usize];
                            for c in 0..3 {
                                e[c] += (adjusted[c] - f32::from(best[c])) * weight / 16.;
                            }
                        }
                    }
                }
                let a = pixels[i][3];
                pixels[i] = best;
                pixels[i][3] = a;
            }
        }
    }
    Ok((pixels, palette))
}
pub fn export_view(view: &View, scale: u32, roundness: f32) -> io::Result<Raster> {
    if !(1..=32).contains(&scale) || !roundness.is_finite() || !(0. ..=1.).contains(&roundness) {
        return Err(crate::invalid("拡大率・丸みが不正です"));
    }
    let (w, h) = (view.width * scale, view.height * scale);
    if u64::from(w) * u64::from(h) > 40_000_000 {
        return Err(crate::invalid(
            "出力画像が4,000万画素を超えます。拡大率を下げてください",
        ));
    }
    let base = view.composite();
    let occupied = |x: i32, y: i32| {
        x >= 0
            && y >= 0
            && x < view.width as i32
            && y < view.height as i32
            && base[(y as u32 * view.width + x as u32) as usize][3] != 0
    };
    let mut pixels = vec![CLEAR; (w * h) as usize];
    let radius = roundness * 0.5;
    let half = 0.5 - radius;
    for y in 0..h {
        for x in 0..w {
            let p = base[((y / scale) * view.width + x / scale) as usize];
            let mut covered = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = (x % scale) as f32 / scale as f32
                        + (sx as f32 + 0.5) / (4. * scale as f32)
                        - 0.5;
                    let py = (y % scale) as f32 / scale as f32
                        + (sy as f32 + 0.5) / (4. * scale as f32)
                        - 0.5;
                    let bx = (x / scale) as i32;
                    let by = (y / scale) as i32;
                    let dx = if occupied(bx + if px < 0. { -1 } else { 1 }, by) {
                        0.
                    } else {
                        (px.abs() - half).max(0.)
                    };
                    let dy = if occupied(bx, by + if py < 0. { -1 } else { 1 }) {
                        0.
                    } else {
                        (py.abs() - half).max(0.)
                    };
                    if dx * dx + dy * dy <= radius * radius + 0.00001 {
                        covered += 1;
                    }
                }
            }
            let mut c = p;
            c[3] = (u32::from(p[3]) * covered / 16) as u8;
            pixels[(y * w + x) as usize] = c;
        }
    }
    Ok(Raster {
        width: w,
        height: h,
        pixels,
    })
}

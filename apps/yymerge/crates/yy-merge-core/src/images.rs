//! Bounded static raster review. Colour channels are decoded 8-bit RGBA, not ICC managed.
use crate::{Cancellation, MAX_BYTES, Result, files::Snapshot};
use image::{DynamicImage, ImageDecoder, ImageFormat};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Cursor, Write},
    path::Path,
    sync::Arc,
};
pub const MAX_PIXELS: u64 = 16_000_000;
#[derive(Clone)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}
impl Raster {
    fn validate(&self) -> Result<()> {
        dimensions(self.width, self.height)?;
        if self.rgba.len() as u64 != u64::from(self.width) * u64::from(self.height) * 4 {
            return Err("invalid RGBA buffer length".into());
        }
        Ok(())
    }
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        dimensions(width, height)?;
        if rgba.len() as u64 != u64::from(width) * u64::from(height) * 4 {
            return Err("invalid RGBA buffer length".into());
        }
        Ok(Self {
            width,
            height,
            rgba: rgba.into(),
        })
    }
    fn pixel(&self, x: i32, y: i32) -> Option<&[u8]> {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return None;
        }
        let at = (y as usize * self.width as usize + x as usize) * 4;
        Some(&self.rgba[at..at + 4])
    }
}
#[derive(Clone)]
pub struct Input {
    pub source: Snapshot,
    pub raster: Raster,
}
pub fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "bmp"
                | "gif"
                | "webp"
                | "tif"
                | "tiff"
                | "avif"
                | "ico"
                | "pdf"
        )
    })
}
fn dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0
        || height == 0
        || width > 32768
        || height > 32768
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(
            "image incomplete: nonempty dimensions up to 32768 and 16 million pixels required"
                .into(),
        );
    }
    Ok(())
}
impl Input {
    pub fn open(path: &Path, cancel: &Cancellation) -> Result<Self> {
        if crate::office::Kind::detect(path).is_some() {
            return Err(
                "Excel/PowerPoint image comparison is unsupported; use content comparison".into(),
            );
        }
        let source = Snapshot::binary(path, cancel)?;
        let format = image::guess_format(source.raw()).map_err(|e| format!("image: {e}"))?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(32768);
        limits.max_image_height = Some(32768);
        limits.max_alloc = Some(128 * 1024 * 1024);
        let raster = match format {
            ImageFormat::Png => {
                let decoder = image::codecs::png::PngDecoder::with_limits(
                    Cursor::new(source.raw()),
                    limits.clone(),
                )
                .map_err(|e| e.to_string())?;
                if decoder.is_apng().map_err(|e| e.to_string())? {
                    return Err("animated PNG is unsupported; use a static PNG frame".into());
                }
                decode(decoder, limits, cancel)?
            }
            ImageFormat::WebP => {
                let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(source.raw()))
                    .map_err(|e| e.to_string())?;
                if decoder.has_animation() {
                    return Err("animated WebP is unsupported; use a static frame".into());
                }
                decode(decoder, limits, cancel)?
            }
            ImageFormat::Jpeg | ImageFormat::Bmp => {
                let mut reader = image::ImageReader::with_format(Cursor::new(source.raw()), format);
                reader.limits(limits.clone());
                decode(
                    reader.into_decoder().map_err(|e| e.to_string())?,
                    limits,
                    cancel,
                )?
            }
            _ => {
                return Err("image comparison supports static PNG, JPEG, BMP and WebP only".into());
            }
        };
        source.verify(cancel)?;
        Ok(Self { source, raster })
    }
}
fn decode(
    mut decoder: impl ImageDecoder,
    limits: image::Limits,
    cancel: &Cancellation,
) -> Result<Raster> {
    cancel.check()?;
    let (width, height) = decoder.dimensions();
    dimensions(width, height)?;
    decoder.set_limits(limits).map_err(|e| e.to_string())?;
    if !matches!(
        decoder.color_type(),
        image::ColorType::Rgb8
            | image::ColorType::Rgba8
            | image::ColorType::L8
            | image::ColorType::La8
    ) {
        return Err("image comparison requires at most 8 bits per channel; 16-bit/float images are unsupported".into());
    }
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let mut decoded = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    cancel.check()?;
    decoded.apply_orientation(orientation);
    let rgba = decoded.into_rgba8();
    Raster::new(rgba.width(), rgba.height(), rgba.into_raw())
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub threshold: u8,
    pub offset_x: i32,
    pub offset_y: i32,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub left_size: [u32; 2],
    pub right_size: [u32; 2],
    pub canvas: Bounds,
    pub settings: Settings,
    pub compared_pixels: u64,
    pub changed_pixels: u64,
    pub raw_difference_pixels: u64,
    pub max_channel_delta: u8,
    pub changed_bounds: Option<Bounds>,
    pub equivalent: bool,
}
pub struct Comparison {
    pub summary: Summary,
    pub difference: Raster,
}
pub fn canvas(left: &Raster, right: &Raster, settings: Settings) -> Result<Bounds> {
    left.validate()?;
    right.validate()?;
    if settings.offset_x.unsigned_abs() > 32768 || settings.offset_y.unsigned_abs() > 32768 {
        return Err("image alignment offset exceeds 32768".into());
    }
    let x = settings.offset_x.min(0);
    let y = settings.offset_y.min(0);
    let width = (left.width as i32).max(settings.offset_x + right.width as i32) - x;
    let height = (left.height as i32).max(settings.offset_y + right.height as i32) - y;
    dimensions(width as u32, height as u32)?;
    Ok(Bounds {
        x,
        y,
        width: width as u32,
        height: height as u32,
    })
}
pub fn compare(
    left: &Raster,
    right: &Raster,
    settings: Settings,
    cancel: &Cancellation,
) -> Result<Comparison> {
    cancel.check()?;
    let area = canvas(left, right, settings)?;
    let mut bytes = Vec::with_capacity(area.width as usize * area.height as usize * 4);
    let (mut count, mut raw, mut total, mut max_delta) = (0, 0, 0, 0);
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for y in area.y..area.y + area.height as i32 {
        cancel.check()?;
        for x in area.x..area.x + area.width as i32 {
            let a = left.pixel(x, y);
            let b = right.pixel(x - settings.offset_x, y - settings.offset_y);
            let missing = a.is_none() != b.is_none();
            let delta = match (a, b) {
                (Some(a), Some(b)) if a[3] == 0 && b[3] == 0 => 0,
                (Some(a), Some(b)) => a
                    .iter()
                    .zip(b)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap_or(0),
                _ if missing => 255,
                _ => 0,
            };
            if a.is_some() || b.is_some() {
                total += 1;
            }
            max_delta = max_delta.max(delta);
            if missing || delta > 0 {
                raw += 1;
            }
            if missing || delta > settings.threshold {
                count += 1;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
                bytes.extend_from_slice(if missing {
                    &[255, 144, 0, 255]
                } else {
                    &[255, 0, 160, 255]
                });
            } else if let Some(pixel) = a.or(b) {
                let c = composite(pixel, x, y);
                let grey = ((u16::from(c[0]) + u16::from(c[1]) + u16::from(c[2])) / 6 + 64) as u8;
                bytes.extend_from_slice(&[grey, grey, grey, 255]);
            } else {
                bytes.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    let changed_bounds = (count > 0).then(|| Bounds {
        x: x0,
        y: y0,
        width: (x1 - x0 + 1) as u32,
        height: (y1 - y0 + 1) as u32,
    });
    Ok(Comparison {
        difference: Raster::new(area.width, area.height, bytes)?,
        summary: Summary {
            left_size: [left.width, left.height],
            right_size: [right.width, right.height],
            canvas: area,
            settings,
            compared_pixels: total,
            changed_pixels: count,
            raw_difference_pixels: raw,
            max_channel_delta: max_delta,
            changed_bounds,
            equivalent: count == 0,
        },
    })
}
fn composite(pixel: &[u8], x: i32, y: i32) -> [u8; 3] {
    let background = if (x.div_euclid(8) + y.div_euclid(8)) & 1 == 0 {
        238
    } else {
        205
    };
    let a = u32::from(pixel[3]);
    [0, 1, 2].map(|i| ((u32::from(pixel[i]) * a + background * (255 - a) + 127) / 255) as u8)
}
/// GDI-ready BGRX, with transparency on a checkerboard. Built once per worker result.
pub fn bgrx(image: &Raster, cancel: &Cancellation) -> Result<Arc<[u8]>> {
    cancel.check()?;
    image.validate()?;
    let mut bytes = Vec::with_capacity(image.rgba.len());
    for y in 0..image.height {
        cancel.check()?;
        for x in 0..image.width {
            let rgb = composite(
                image.pixel(x as i32, y as i32).ok_or("missing pixel")?,
                x as i32,
                y as i32,
            );
            bytes.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        }
    }
    Ok(bytes.into())
}
pub fn overlay(
    left: &Raster,
    right: &Raster,
    settings: Settings,
    cancel: &Cancellation,
) -> Result<Raster> {
    cancel.check()?;
    let area = canvas(left, right, settings)?;
    let mut bytes = Vec::with_capacity(area.width as usize * area.height as usize * 4);
    for y in area.y..area.y + area.height as i32 {
        cancel.check()?;
        for x in area.x..area.x + area.width as i32 {
            let a = left.pixel(x, y).map(|p| composite(p, x, y));
            let b = right
                .pixel(x - settings.offset_x, y - settings.offset_y)
                .map(|p| composite(p, x, y));
            let rgb = match (a, b) {
                (Some(a), Some(b)) => {
                    [0, 1, 2].map(|i| ((u16::from(a[i]) + u16::from(b[i])) / 2) as u8)
                }
                (Some(p), None) | (None, Some(p)) => p,
                _ => [238; 3],
            };
            bytes.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    Raster::new(area.width, area.height, bytes)
}
struct Budget<'a> {
    bytes: Vec<u8>,
    cancel: &'a Cancellation,
}
impl Write for Budget<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.cancel.check().map_err(io::Error::other)?;
        if bytes.len() > (MAX_BYTES as usize).saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("image report exceeds 32 MiB"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub fn png(image: &Raster, cancel: &Cancellation) -> Result<Vec<u8>> {
    cancel.check()?;
    image.validate()?;
    let mut out = Budget {
        bytes: Vec::new(),
        cancel,
    };
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut out),
        &image.rgba,
        image.width,
        image.height,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| e.to_string())?;
    cancel.check()?;
    Ok(out.bytes)
}
pub fn html(
    left: &Input,
    right: &Input,
    comparison: &Comparison,
    cancel: &Cancellation,
) -> Result<String> {
    use base64::Engine;
    let mut out = Budget {
        bytes: Vec::new(),
        cancel,
    };
    let mut push = |s: &str| out.write_all(s.as_bytes()).map_err(|e| e.to_string());
    push(
        "<!doctype html><html lang=ja><meta charset=utf-8><meta http-equiv=Content-Security-Policy content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'; base-uri 'none'\"><title>yymerge 画像比較</title><style>body{font-family:'UDEV Gothic',monospace}main{display:flex;gap:12px}section{width:33%;overflow:auto}img{max-width:100%;background:#ddd}pre{white-space:pre-wrap}</style><h1>yymerge 画像比較（読み取り専用）</h1>",
    )?;
    let stats = serde_json::to_string_pretty(&comparison.summary).map_err(|e| e.to_string())?;
    push(&format!(
        "<pre>{stats}</pre><p>デコード後の8ビットRGBAを比較。ICC色管理なし。両側透明のRGBは無視。橙は片側にのみ存在、桃色は閾値を超える差分。合成・位置補正は原本を変更しません。</p><main>"
    ))?;
    for (label, raster) in [
        ("LEFT", &left.raster),
        ("RIGHT", &right.raster),
        ("DIFFERENCE", &comparison.difference),
    ] {
        push(&format!(
            "<section><h2>{label}</h2><img alt={label} src=\"data:image/png;base64,"
        ))?;
        let encoded = png(raster, cancel)?;
        for chunk in encoded.chunks(3 * 4096) {
            cancel.check()?;
            push(&base64::engine::general_purpose::STANDARD.encode(chunk))?;
        }
        push("\"></section>")?;
    }
    push("</main></html>\n")?;
    String::from_utf8(out.bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn raster(width: u32, height: u32, pixel: [u8; 4]) -> Raster {
        Raster::new(
            width,
            height,
            pixel.repeat(width as usize * height as usize),
        )
        .unwrap()
    }
    #[test]
    fn alpha_threshold_missing_pixels_and_bounds_are_explicit() {
        let c = Cancellation::new();
        let a = raster(2, 2, [100, 120, 130, 255]);
        let mut b = a.clone();
        let mut bytes = b.rgba.to_vec();
        bytes[4] = 105;
        b.rgba = bytes.into();
        let result = compare(&a, &b, Default::default(), &c).unwrap();
        assert_eq!(result.summary.changed_pixels, 1);
        assert_eq!(result.summary.max_channel_delta, 5);
        assert_eq!(
            result.summary.changed_bounds,
            Some(Bounds {
                x: 1,
                y: 0,
                width: 1,
                height: 1
            })
        );
        let result = compare(
            &a,
            &b,
            Settings {
                threshold: 5,
                ..Default::default()
            },
            &c,
        )
        .unwrap();
        assert!(result.summary.equivalent);
        assert_eq!(result.summary.raw_difference_pixels, 1);
        let a = raster(1, 1, [255, 0, 0, 0]);
        let b = raster(1, 1, [0, 255, 0, 0]);
        assert!(
            compare(&a, &b, Default::default(), &c)
                .unwrap()
                .summary
                .equivalent
        );
        let b = raster(1, 1, [255, 0, 0, 1]);
        assert_eq!(
            compare(&a, &b, Default::default(), &c)
                .unwrap()
                .summary
                .changed_pixels,
            1
        );
        let b = raster(2, 1, [255, 0, 0, 0]);
        let result = compare(
            &a,
            &b,
            Settings {
                threshold: 255,
                ..Default::default()
            },
            &c,
        )
        .unwrap();
        assert_eq!(result.summary.changed_pixels, 1);
        assert_eq!(result.summary.compared_pixels, 2);
        assert_eq!(&result.difference.rgba[4..8], &[255, 144, 0, 255]);
    }
    #[test]
    fn alignment_counts_union_without_counting_empty_canvas_gaps() {
        let c = Cancellation::new();
        let a = raster(1, 1, [255, 0, 0, 255]);
        let result = compare(
            &a,
            &a,
            Settings {
                offset_x: -2,
                offset_y: 2,
                threshold: 0,
            },
            &c,
        )
        .unwrap();
        assert_eq!(
            result.summary.canvas,
            Bounds {
                x: -2,
                y: 0,
                width: 3,
                height: 3
            }
        );
        assert_eq!(result.summary.compared_pixels, 2);
        assert_eq!(result.summary.changed_pixels, 2);
        assert_eq!(
            result.summary.changed_bounds,
            Some(Bounds {
                x: -2,
                y: 0,
                width: 3,
                height: 3
            })
        );
        let left = Raster::new(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        let right = Raster::new(2, 1, vec![0, 0, 255, 255, 255, 0, 0, 255]).unwrap();
        assert_eq!(
            compare(
                &left,
                &right,
                Settings {
                    offset_x: 1,
                    ..Default::default()
                },
                &c
            )
            .unwrap()
            .summary
            .raw_difference_pixels,
            2
        );
        let overlay = overlay(&left, &right, Default::default(), &c).unwrap();
        assert_eq!(&overlay.rgba[..4], &[127, 0, 127, 255]);
    }
    #[test]
    fn decoders_png_exports_orientation_and_raw_identity_are_independent() {
        let c = Cancellation::new();
        let dir = tempfile::tempdir().unwrap();
        let a = raster(3, 2, [50, 60, 70, 255]);
        let png = png(&a, &c).unwrap();
        let pa = dir.path().join("a.png");
        std::fs::write(&pa, &png).unwrap();
        let mut bmp = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(3, 2, a.rgba.to_vec()).unwrap())
            .write_to(&mut bmp, ImageFormat::Bmp)
            .unwrap();
        let pb = dir.path().join("b.bmp");
        std::fs::write(&pb, bmp.into_inner()).unwrap();
        let a = Input::open(&pa, &c).unwrap();
        let b = Input::open(&pb, &c).unwrap();
        assert!(!a.source.original_identical(&b.source));
        assert!(
            compare(&a.raster, &b.raster, Default::default(), &c)
                .unwrap()
                .summary
                .equivalent
        );
        assert!(a.source.encode("no source edits").is_err());
        let html = html(
            &a,
            &b,
            &compare(&a.raster, &b.raster, Default::default(), &c).unwrap(),
            &c,
        )
        .unwrap();
        assert!(html.contains("data:image/png;base64,"));
        assert!(!html.contains("<script"));
        let path = dir.path().join("orientation.jpg");
        std::fs::write(
            &path,
            include_bytes!("../../../../../demo/yymerge/images/oriented.jpg"),
        )
        .unwrap();
        let input = Input::open(&path, &c).unwrap();
        assert_eq!([input.raster.width, input.raster.height], [3, 2]);
        for (name, bytes) in [
            (
                "demo.png",
                include_bytes!("../../../../../demo/yymerge/images/left.png").as_slice(),
            ),
            (
                "demo.jpg",
                include_bytes!("../../../../../demo/yymerge/images/sample.jpg").as_slice(),
            ),
            (
                "demo.bmp",
                include_bytes!("../../../../../demo/yymerge/images/sample.bmp").as_slice(),
            ),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            let im = Input::open(&path, &c).unwrap();
            assert_eq!([im.raster.width, im.raster.height], [320, 200]);
        }
        std::fs::write(&pa, b"externally changed").unwrap();
        assert!(a.source.verify(&c).is_err());
    }
    #[test]
    fn webp_lossless_lossy_alpha_and_animation_are_handled_explicitly() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cancellation::new();
        let load = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            Input::open(&path, &c)
        };
        let png = load(
            "left.png",
            include_bytes!("../../../../../demo/yymerge/images/left.png"),
        )
        .unwrap();
        let webp = load(
            "LEFT.WEBP",
            include_bytes!("../../../../../demo/yymerge/images/left.webp"),
        )
        .unwrap();
        assert!(is_image(Path::new("LEFT.WEBP")));
        assert_eq!(png.raster.rgba, webp.raster.rgba);
        assert!(
            webp.raster
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] == 180)
        );
        assert!(!png.source.original_identical(&webp.source));
        assert!(
            compare(&png.raster, &webp.raster, Default::default(), &c)
                .unwrap()
                .summary
                .equivalent
        );
        let lossy = load(
            "lossy.webp",
            include_bytes!("../../../../../demo/yymerge/images/sample-lossy.webp"),
        )
        .unwrap();
        assert_eq!([lossy.raster.width, lossy.raster.height], [320, 200]);
        assert!(
            lossy
                .raster
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[3] == 255)
        );
        assert!(
            load(
                "animated.webp",
                include_bytes!("../../../../../demo/yymerge/images/animated.webp")
            )
            .err()
            .unwrap()
            .contains("animated WebP")
        );
        let bytes = include_bytes!("../../../../../demo/yymerge/images/left.webp");
        assert!(load("truncated.webp", &bytes[..bytes.len() / 2]).is_err());
    }
    #[test]
    fn invalid_animated_high_depth_large_and_cancelled_images_fail_closed() {
        let c = Cancellation::new();
        let dir = tempfile::tempdir().unwrap();
        for bytes in [
            b"not an image".as_slice(),
            include_bytes!("../../../../../demo/yymerge/images/animated.png"),
            include_bytes!("../../../../../demo/yymerge/images/sixteen-bit.png"),
        ] {
            let path = dir.path().join("bad.png");
            std::fs::write(&path, bytes).unwrap();
            assert!(Input::open(&path, &c).is_err());
        }
        assert!(Raster::new(0, 1, vec![]).is_err());
        assert!(Raster::new(5000, 5000, vec![]).is_err());
        assert!(Raster::new(2, 1, vec![0; 4]).is_err());
        let a = raster(1, 1, [0; 4]);
        assert!(
            canvas(
                &a,
                &a,
                Settings {
                    offset_x: i32::MIN,
                    ..Default::default()
                }
            )
            .is_err()
        );
        let wrong = Raster {
            width: 1,
            height: 1,
            rgba: Arc::from([]),
        };
        assert!(compare(&wrong, &a, Default::default(), &c).is_err());
        assert!(bgrx(&wrong, &c).is_err());
        assert!(png(&wrong, &c).is_err());
        let mut out = Budget {
            bytes: vec![0; MAX_BYTES as usize],
            cancel: &c,
        };
        assert!(out.write_all(b"x").is_err());
        c.cancel();
        assert!(compare(&a, &a, Default::default(), &c).is_err());
        assert!(overlay(&a, &a, Default::default(), &c).is_err());
        assert!(bgrx(&a, &c).is_err());
        assert!(png(&a, &c).is_err());
    }
}

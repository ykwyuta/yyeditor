//! Batch image I/O is restricted to paths chosen by the user.
use crate::{Output, Params, Result, integer, value};
use image::{DynamicImage, GenericImageView, Rgba, RgbaImage, imageops::FilterType};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

struct Options {
    width: u32,
    height: u32,
    mode: String,
    background: Rgba<u8>,
    format: image::ImageFormat,
    enlarge: bool,
}
fn options(params: &Params) -> Result<Options> {
    let width = integer(params, "width", 1, 8192)?;
    let height = integer(params, "height", 1, 8192)?;
    if u64::from(width) * u64::from(height) > 32_000_000 {
        return Err("出力の枠は3200万画素までです".into());
    }
    let color = value(params, "background", "#FFFFFF")
        .strip_prefix('#')
        .ok_or("背景色は#RRGGBBで指定してください")?;
    if color.len() != 6 || !color.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("背景色は#RRGGBBで指定してください".into());
    }
    let mut rgb = [0u8; 3];
    for (i, c) in rgb.iter_mut().enumerate() {
        *c = u8::from_str_radix(&color[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    let transparent = value(params, "transparent", "なし") == "あり";
    let format = match value(params, "image_format", "PNG") {
        "PNG" => image::ImageFormat::Png,
        "JPEG" => image::ImageFormat::Jpeg,
        "WebP" => image::ImageFormat::WebP,
        "BMP" => image::ImageFormat::Bmp,
        _ => return Err("出力形式が不正です".into()),
    };
    if transparent && !matches!(format, image::ImageFormat::Png | image::ImageFormat::WebP) {
        return Err("透明背景の保存にはPNGまたはWebPを選択してください".into());
    }
    let mode = value(params, "resize_mode", "縦横比維持");
    if !["縦横比維持", "背景補完", "引き伸ばし"].contains(&mode) {
        return Err("サイズの合わせ方が不正です".into());
    }
    Ok(Options {
        width,
        height,
        mode: mode.into(),
        background: Rgba([rgb[0], rgb[1], rgb[2], if transparent { 0 } else { 255 }]),
        format,
        enlarge: value(params, "enlarge", "なし") == "あり",
    })
}
fn transform(source: &DynamicImage, options: &Options) -> DynamicImage {
    let (sw, sh) = source.dimensions();
    let (width, height) = if options.mode == "引き伸ばし" {
        (options.width, options.height)
    } else {
        let ratio = (f64::from(options.width) / f64::from(sw))
            .min(f64::from(options.height) / f64::from(sh));
        let ratio = if options.enlarge {
            ratio
        } else {
            ratio.min(1.0)
        };
        (
            ((f64::from(sw) * ratio).floor() as u32)
                .max(1)
                .min(options.width),
            ((f64::from(sh) * ratio).floor() as u32)
                .max(1)
                .min(options.height),
        )
    };
    let resized = source
        .resize_exact(width, height, FilterType::Lanczos3)
        .to_rgba8();
    let (cw, ch) = if options.mode == "背景補完" {
        (options.width, options.height)
    } else {
        (width, height)
    };
    // PNG/WebP keep source transparency; padding uses the explicitly selected background.
    let needs_canvas = options.mode == "背景補完"
        || matches!(
            options.format,
            image::ImageFormat::Jpeg | image::ImageFormat::Bmp
        );
    if needs_canvas {
        let mut bg = options.background;
        if matches!(
            options.format,
            image::ImageFormat::Jpeg | image::ImageFormat::Bmp
        ) {
            bg.0[3] = 255;
        }
        let mut canvas = RgbaImage::from_pixel(cw, ch, bg);
        image::imageops::overlay(
            &mut canvas,
            &resized,
            i64::from((cw - width) / 2),
            i64::from((ch - height) / 2),
        );
        let image = DynamicImage::ImageRgba8(canvas);
        if matches!(
            options.format,
            image::ImageFormat::Jpeg | image::ImageFormat::Bmp
        ) {
            DynamicImage::ImageRgb8(image.to_rgb8())
        } else {
            image
        }
    } else {
        DynamicImage::ImageRgba8(resized)
    }
}
fn load(path: &Path) -> Result<DynamicImage> {
    if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 {
        return Err("入力画像は64 MiBまでです".into());
    }
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    if !matches!(
        reader.format(),
        Some(
            image::ImageFormat::Png
                | image::ImageFormat::Jpeg
                | image::ImageFormat::WebP
                | image::ImageFormat::Bmp
        )
    ) {
        return Err("入力はPNG/JPEG/WebP/BMPに対応します".into());
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(256 * 1024 * 1024);
    limits.max_image_width = Some(32768);
    limits.max_image_height = Some(32768);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    use image::ImageDecoder;
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let dimensions = decoder.dimensions();
    if u64::from(dimensions.0) * u64::from(dimensions.1) > 32_000_000 {
        return Err("入力画像は3200万画素までです".into());
    }
    let mut image = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    image.apply_orientation(orientation);
    Ok(image)
}
pub fn resize_batch(
    paths: &[PathBuf],
    directory: &Path,
    params: &Params,
    cancelled: &impl Fn() -> bool,
) -> Result<Output> {
    if paths.is_empty() || paths.len() > 1000 {
        return Err("画像は1〜1000個選択してください".into());
    }
    if !directory.is_dir() {
        return Err("出力先のフォルダがありません".into());
    }
    let options = options(params)?;
    let extension = match options.format {
        image::ImageFormat::Png => "png",
        image::ImageFormat::Jpeg => "jpg",
        image::ImageFormat::WebP => "webp",
        _ => "bmp",
    };
    let mut rows = vec![vec![
        "入力".into(),
        "出力".into(),
        "結果".into(),
        "幅".into(),
        "高さ".into(),
    ]];
    let mut succeeded = 0;
    for path in paths {
        if cancelled() {
            break;
        }
        let result = (|| -> Result<_> {
            let source = load(path)?;
            if cancelled() {
                return Err("中止しました".into());
            }
            let image = transform(&source, &options);
            if cancelled() {
                return Err("中止しました".into());
            }
            let stem = path.file_stem().and_then(|v| v.to_str()).unwrap_or("image");
            let mut temp = tempfile::Builder::new()
                .suffix(&format!(".{extension}"))
                .tempfile_in(directory)
                .map_err(|e| e.to_string())?;
            image
                .write_to(temp.as_file_mut(), options.format)
                .map_err(|e| e.to_string())?;
            temp.flush().map_err(|e| e.to_string())?;
            temp.as_file().sync_all().map_err(|e| e.to_string())?;
            if cancelled() {
                return Err("中止しました".into());
            }
            // Atomic no-clobber save; duplicate basenames get numbered output names.
            for suffix in 0..10000 {
                let name = if suffix == 0 {
                    format!("{stem}-resized.{extension}")
                } else {
                    format!("{stem}-resized-{suffix}.{extension}")
                };
                let output = directory.join(name);
                match temp.persist_noclobber(&output) {
                    Ok(_) => return Ok((output, image.dimensions())),
                    Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                        temp = e.file;
                    }
                    Err(e) => return Err(e.error.to_string()),
                }
            }
            Err("同名の出力が多すぎます".into())
        })();
        match result {
            Ok((output, (w, h))) => {
                succeeded += 1;
                rows.push(vec![
                    path.display().to_string(),
                    output.display().to_string(),
                    "成功".into(),
                    w.to_string(),
                    h.to_string(),
                ]);
            }
            Err(error) => rows.push(vec![
                path.display().to_string(),
                String::new(),
                error,
                String::new(),
                String::new(),
            ]),
        }
    }
    let processed = rows.len() - 1;
    Output::csv(
        rows,
        format!(
            "{processed}/{}件処理・{succeeded}件保存{}。入力・既存出力は上書きしません",
            paths.len(),
            if cancelled() {
                "（中止。保存済み画像は維持）"
            } else {
                ""
            }
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ratio_padding_color_transparency_and_no_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("source.png");
        RgbaImage::from_pixel(100, 50, Rgba([255, 0, 0, 255]))
            .save(&input)
            .unwrap();
        let original = std::fs::read(&input).unwrap();
        let mut p = crate::Tool::ResizeImages.defaults();
        p.insert("width".into(), "80".into());
        p.insert("height".into(), "80".into());
        resize_batch(std::slice::from_ref(&input), dir.path(), &p, &|| false).unwrap();
        assert_eq!(
            image::open(dir.path().join("source-resized.png"))
                .unwrap()
                .dimensions(),
            (80, 40)
        );
        p.insert("resize_mode".into(), "背景補完".into());
        p.insert("background".into(), "#123456".into());
        resize_batch(std::slice::from_ref(&input), dir.path(), &p, &|| false).unwrap();
        let padded = image::open(dir.path().join("source-resized-1.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(padded.dimensions(), (80, 80));
        assert_eq!(padded.get_pixel(0, 0).0, [0x12, 0x34, 0x56, 255]);
        assert_eq!(padded.get_pixel(40, 40).0, [255, 0, 0, 255]);
        p.insert("transparent".into(), "あり".into());
        resize_batch(std::slice::from_ref(&input), dir.path(), &p, &|| false).unwrap();
        assert_eq!(
            image::open(dir.path().join("source-resized-2.png"))
                .unwrap()
                .to_rgba8()
                .get_pixel(0, 0)
                .0[3],
            0
        );
        assert_eq!(std::fs::read(input).unwrap(), original);
        p.insert("image_format".into(), "JPEG".into());
        assert!(options(&p).is_err());
        assert!(resize_batch(&[], dir.path(), &p, &|| false).is_err());
    }
}

//! Direct PDF comparison with an embedded Rust rasterizer.
use crate::{
    Cancellation, MAX_BYTES, Result,
    files::Snapshot,
    images::{self, Input, Raster, Settings},
};
use std::path::Path;

const MAX_PAGES: usize = 128;
pub fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}
pub struct Document {
    pub source: Snapshot,
    pub pages: usize,
    pdf: hayro::hayro_syntax::Pdf,
}
pub struct Pair {
    pub left: Document,
    pub right: Document,
}
pub struct Page {
    pub left: Input,
    pub right: Input,
    pub missing: [bool; 2],
}
impl Page {
    pub fn compare(&self, settings: Settings, c: &Cancellation) -> Result<images::Comparison> {
        compare(&self.left, &self.right, self.missing, settings, c)
    }
}
pub fn compare(
    left: &Input,
    right: &Input,
    missing: [bool; 2],
    settings: Settings,
    c: &Cancellation,
) -> Result<images::Comparison> {
    let mut result = images::compare(&left.raster, &right.raster, settings, c)?;
    mark_missing(&mut result.summary, missing);
    if missing.into_iter().any(|m| m) {
        let pixels = std::sync::Arc::make_mut(&mut result.difference.rgba);
        for row in pixels.chunks_mut(result.difference.width as usize * 4) {
            c.check()?;
            for pixel in row.as_chunks_mut::<4>().0 {
                *pixel = [255, 144, 0, 255];
            }
        }
    }
    Ok(result)
}
pub fn mark_missing(summary: &mut images::Summary, missing: [bool; 2]) {
    if missing.into_iter().any(|m| m) {
        summary.equivalent = false;
        summary.changed_pixels = summary.compared_pixels;
        summary.raw_difference_pixels = summary.compared_pixels;
        summary.max_channel_delta = 255;
        summary.changed_bounds = Some(summary.canvas);
    }
}
impl Document {
    fn open(path: &Path, c: &Cancellation) -> Result<Self> {
        if !is_pdf(path) {
            return Err("PDF image comparison requires two PDF files".into());
        }
        let source = Snapshot::binary(path, c)?;
        let pdf = parse_pdf(source.raw().to_vec(), c)?;
        let pages = pdf.pages().len();
        source.verify(c)?;
        Ok(Self { source, pages, pdf })
    }
    fn page(&self, page: usize, c: &Cancellation) -> Result<Input> {
        self.source.verify(c)?;
        if page >= self.pages {
            return Err("PDF page outside document".into());
        }
        let raster = render_page(&self.pdf, page, c)?;
        self.source.verify(c)?;
        Ok(Input {
            source: self.source.clone(),
            raster,
        })
    }
}
fn parse_pdf(bytes: Vec<u8>, c: &Cancellation) -> Result<hayro::hayro_syntax::Pdf> {
    c.check()?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("PDF exceeds 32 MiB".into());
    }
    let pdf = std::panic::catch_unwind(|| hayro::hayro_syntax::Pdf::new(bytes))
        .map_err(|_| "embedded PDF parser failed")?
        .map_err(|e| format!("Cannot read PDF; password entry is unsupported ({e:?})"))?;
    let pages = pdf.pages().len();
    if !(1..=MAX_PAGES).contains(&pages) {
        return Err("PDF image comparison requires 1..128 pages".into());
    }
    c.check()?;
    Ok(pdf)
}
fn render_scale(width: f32, height: f32) -> Result<f32> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err("invalid PDF page dimensions".into());
    }
    let scale = 2400.0 / width.max(height);
    let w = (width * scale) as u32;
    let h = (height * scale) as u32;
    if !scale.is_finite()
        || w == 0
        || h == 0
        || w > 2401
        || h > 2401
        || u64::from(w) * u64::from(h) > images::MAX_PIXELS
    {
        return Err("PDF page exceeds raster dimensions budget".into());
    }
    Ok(scale)
}
fn render_page(pdf: &hayro::hayro_syntax::Pdf, number: usize, c: &Cancellation) -> Result<Raster> {
    c.check()?;
    let page = pdf.pages().get(number).ok_or("PDF page outside document")?;
    let (width, height) = page.render_dimensions();
    let scale = render_scale(width, height)?;
    let warning = std::sync::Arc::new(std::sync::Mutex::new(None));
    let sink = warning.clone();
    let settings = hayro::hayro_interpret::InterpreterSettings {
        warning_sink: std::sync::Arc::new(move |value| {
            if let Ok(mut slot) = sink.lock() {
                slot.get_or_insert(value);
            }
        }),
        ..Default::default()
    };
    let pixmap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        hayro::render(
            page,
            &hayro::RenderCache::new(),
            &settings,
            &Default::default(),
            &hayro::PixmapSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: hayro::vello_cpu::color::palette::css::WHITE,
            },
        )
    }))
    .map_err(|_| "embedded PDF renderer failed")?;
    c.check()?;
    if let Some(warning) = *warning
        .lock()
        .map_err(|_| "PDF warning state unavailable")?
    {
        return Err(format!("PDF rendering incomplete: {warning:?}"));
    }
    Raster::new(
        u32::from(pixmap.width()),
        u32::from(pixmap.height()),
        pixmap.take_rgba8(hayro::vello_cpu::peniko::ImageAlphaType::Alpha),
    )
}
impl Pair {
    pub fn open(left: &Path, right: &Path, c: &Cancellation) -> Result<Self> {
        if !is_pdf(left) || !is_pdf(right) {
            return Err("PDF image comparison requires two PDF files".into());
        }
        Ok(Self {
            left: Document::open(left, c)?,
            right: Document::open(right, c)?,
        })
    }
    pub fn pages(&self) -> usize {
        self.left.pages.max(self.right.pages)
    }
    pub fn page(&self, page: usize, c: &Cancellation) -> Result<Page> {
        self.left.source.verify(c)?;
        self.right.source.verify(c)?;
        if page >= self.pages() {
            return Err("PDF page outside comparison".into());
        }
        let missing = [page >= self.left.pages, page >= self.right.pages];
        let a = if missing[0] {
            None
        } else {
            Some(self.left.page(page, c)?)
        };
        let b = if missing[1] {
            None
        } else {
            Some(self.right.page(page, c)?)
        };
        let blank = |source: &Snapshot, other: &Input| -> Result<Input> {
            Ok(Input {
                source: source.clone(),
                raster: Raster::new(
                    other.raster.width,
                    other.raster.height,
                    vec![0; other.raster.rgba.len()],
                )?,
            })
        };
        let left = match a {
            Some(a) => a,
            None => blank(&self.left.source, b.as_ref().unwrap())?,
        };
        let right = match b {
            Some(b) => b,
            None => blank(&self.right.source, &left)?,
        };
        Ok(Page {
            left,
            right,
            missing,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn direct_pdf_files_are_read_only_and_missing_pages_remain_differences() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("左.pdf");
        let b = dir.path().join("右.PDF");
        let left = include_bytes!("../../../../../demo/yymerge/pdf/left.pdf");
        fs::write(&a, left).unwrap();
        fs::write(
            &b,
            include_bytes!("../../../../../demo/yymerge/pdf/right.pdf"),
        )
        .unwrap();
        let c = Cancellation::new();
        let pair = Pair::open(&a, &b, &c).unwrap();
        assert_eq!([pair.left.pages, pair.right.pages], [2, 3]);
        assert!(
            !pair
                .page(0, &c)
                .unwrap()
                .compare(Settings::default(), &c)
                .unwrap()
                .summary
                .equivalent
        );
        let missing = pair.page(2, &c).unwrap();
        assert_eq!(missing.missing, [true, false]);
        assert!(
            !missing
                .compare(
                    Settings {
                        threshold: 255,
                        ..Default::default()
                    },
                    &c
                )
                .unwrap()
                .summary
                .equivalent
        );
        assert!(pair.left.source.encode("not editable").is_err());
        let equivalent = dir.path().join("equivalent.pdf");
        let mut bytes = left.to_vec();
        bytes.extend_from_slice(b"% metadata-only trailing comment\n");
        fs::write(&equivalent, bytes).unwrap();
        let same = Pair::open(&a, &equivalent, &c)
            .unwrap()
            .page(0, &c)
            .unwrap();
        assert!(!same.left.source.original_identical(&same.right.source));
        assert!(
            same.compare(Settings::default(), &c)
                .unwrap()
                .summary
                .equivalent
        );
        fs::write(&a, b"changed").unwrap();
        assert!(pair.page(2, &c).is_err());
        assert!(Pair::open(&a, &b, &c).is_err());
        assert!(Pair::open(&b, Path::new("other.xlsx"), &c).is_err());
    }
    #[test]
    fn counts_and_missing_pages_are_explicit() {
        let c = Cancellation::new();
        assert_eq!(parse_pdf(pdf_fixture(12), &c).unwrap().pages().len(), 12);
        for bytes in [
            vec![],
            b"not a PDF".to_vec(),
            pdf_fixture(0),
            pdf_fixture(129),
        ] {
            assert!(parse_pdf(bytes, &c).is_err());
        }
        let c = Cancellation::new();
        let r = Raster::new(1, 1, vec![255; 4]).unwrap();
        let mut result = images::compare(
            &r,
            &r,
            Settings {
                threshold: 255,
                ..Default::default()
            },
            &c,
        )
        .unwrap();
        mark_missing(&mut result.summary, [true, false]);
        assert!(!result.summary.equivalent);
        assert_eq!(result.summary.changed_pixels, 1);
    }
    fn pdf_fixture(count: usize) -> Vec<u8> {
        let font = count * 2 + 3;
        let kids = (0..count)
            .map(|i| format!("{} 0 R", i * 2 + 3))
            .collect::<Vec<_>>()
            .join(" ");
        let mut objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            format!("<< /Type /Pages /Kids [{kids}] /Count {count} >>").into_bytes(),
        ];
        for i in 0..count {
            let rotation = if i % 2 == 0 { 0 } else { 90 };
            objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 150 100] /Rotate {rotation} /Resources << /Font << /F1 {font} 0 R >> >> /Contents {} 0 R >>", i * 2 + 4).into_bytes());
            let content = "1 0 0 rg 10 10 30 20 re f 0 0 0 rg BT /F1 14 Tf 50 50 Td (YY PDF) Tj ET";
            objects.push(
                format!(
                    "<< /Length {} >>\nstream\n{content}\nendstream",
                    content.len()
                )
                .into_bytes(),
            );
        }
        objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0];
        for (i, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            bytes.extend_from_slice(object);
            bytes.extend_from_slice(b"\nendobj\n");
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes(),
        );
        for offset in offsets.iter().skip(1) {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len()
            )
            .as_bytes(),
        );
        bytes
    }
    #[test]
    fn embedded_pdf_raster_has_opaque_background_colour_text_and_rotation() {
        let c = Cancellation::new();
        let pdf = parse_pdf(pdf_fixture(2), &c).unwrap();
        let a = render_page(&pdf, 0, &c).unwrap();
        assert_eq!([a.width, a.height], [2400, 1600]);
        let at = (1280 * a.width as usize + 320) * 4;
        assert_eq!(&a.rgba[at..at + 4], &[255, 0, 0, 255]);
        assert_eq!(&a.rgba[..4], &[255; 4]);
        assert!(a.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        assert!(
            a.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[..3] == [0, 0, 0])
        );
        let b = render_page(&pdf, 1, &c).unwrap();
        assert_eq!([b.width, b.height], [1600, 2400]);
        assert!(render_page(&pdf, 2, &c).is_err());
        for (w, h) in [
            (0.0, 10.0),
            (f32::NAN, 10.0),
            (1.0, f32::INFINITY),
            (1e-20, 1e20),
        ] {
            assert!(render_scale(w, h).is_err());
        }
        c.cancel();
        assert!(render_page(&pdf, 0, &c).is_err());
        assert!(parse_pdf(pdf_fixture(1), &c).is_err());
    }
    #[test]
    fn non_pdf_inputs_are_rejected() {
        assert!(
            Pair::open(
                Path::new("a.xlsx"),
                Path::new("b.pptx"),
                &Cancellation::new()
            )
            .is_err()
        );
    }
}

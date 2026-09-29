//! Direct2D / DirectWrite による描画（07 章 2.1）。
//!
//! 描画するのは表示範囲の行だけで、ファイルサイズは描画コストに影響しない。
//! 行ごとの `IDWriteTextLayout` は表示行の位置をキーにキャッシュする。

use std::collections::HashMap;

use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{HSTRING, Interface, Result, w};
use windows_numerics::Vector2;
use yy_config::{Color, Colors};
use yy_layout::{Row, SpanKind};

use crate::util::Context;

/// IME で変換中の文字列（キャレット位置にインライン表示する）。
pub(crate) struct Composition {
    /// 挿入位置（文書のオフセット）。先頭が主カーソルで、複数カーソルでは全位置にプレビューする
    pub offsets: Vec<u64>,
    pub text: String,
    /// 変換中の文字列内のカーソル位置（UTF-16 単位）
    pub cursor: usize,
}

/// 描画に必要な文書側の情報。
pub(crate) struct Frame<'a> {
    /// 内容の版（変わったら行レイアウトのキャッシュを捨てる）
    pub version: u64,
    pub rows: &'a [Row],
    /// 先頭の表示行の論理行番号（0 始まり）
    pub first_line: u64,
    /// 行番号が確定値か（`false` なら推定値として薄く表示）
    pub line_exact: bool,
    /// 行番号欄の桁数
    pub line_digits: usize,
    pub show_line_numbers: bool,
    pub scroll_x: f32,
    /// 空でない選択範囲（昇順）
    pub selections: &'a [std::ops::Range<u64>],
    /// キャレット位置（昇順）
    pub carets: &'a [u64],
    pub caret_visible: bool,
    /// 上書きモード（キャレットを太く表示する）
    pub overwrite: bool,
    pub composition: Option<&'a Composition>,
    /// 矩形選択の表示中の行
    pub rect: &'a [RectPaint],
}

/// 矩形選択の 1 行分の表示情報。
pub(crate) struct RectPaint {
    pub row_start: u64,
    /// 左端の位置と、そこから右の仮想空白の桁数
    pub left: (u64, u32),
    /// 右端の位置と、そこから右の仮想空白の桁数
    pub right: (u64, u32),
    /// この行に表示するキャレット（位置と仮想空白の桁数）
    pub caret: Option<(u64, u32)>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FontMetrics {
    pub line_height: f32,
    pub char_width: f32,
}

struct Brushes {
    foreground: ID2D1SolidColorBrush,
    gutter_background: ID2D1SolidColorBrush,
    line_number: ID2D1SolidColorBrush,
    line_number_estimated: ID2D1SolidColorBrush,
    invalid: ID2D1SolidColorBrush,
    control: ID2D1SolidColorBrush,
    selection: ID2D1SolidColorBrush,
    caret: ID2D1SolidColorBrush,
}

struct Target {
    rt: ID2D1RenderTarget,
    /// ウィンドウに描く場合のみ（サイズ変更に使う）
    hwnd_rt: Option<ID2D1HwndRenderTarget>,
    brushes: Brushes,
}

impl Target {
    fn new(
        rt: ID2D1RenderTarget,
        hwnd_rt: Option<ID2D1HwndRenderTarget>,
        c: &Colors,
    ) -> Result<Target> {
        unsafe {
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_DEFAULT);
            let brush = |c: Color| rt.CreateSolidColorBrush(&color_f(c), None);
            let brushes = Brushes {
                foreground: brush(c.foreground)?,
                gutter_background: brush(c.gutter_background)?,
                line_number: brush(c.line_number)?,
                line_number_estimated: brush(c.line_number_estimated)?,
                invalid: brush(c.invalid_byte)?,
                control: brush(c.control)?,
                selection: brush(c.selection)?,
                caret: brush(c.caret)?,
            };
            Ok(Target {
                rt,
                hwnd_rt,
                brushes,
            })
        }
    }
}

/// 1 行分のレイアウトと、その元の UTF-16 テキスト。
#[derive(Clone)]
struct RowLayout {
    layout: IDWriteTextLayout,
    width: f32,
    wide: std::rc::Rc<[u16]>,
}

struct CachedRow {
    row: RowLayout,
    used: bool,
}

pub(crate) struct Renderer {
    d2d: ID2D1Factory,
    dwrite: IDWriteFactory,
    target: Option<Target>,
    text_format: IDWriteTextFormat,
    number_format: IDWriteTextFormat,
    metrics: FontMetrics,
    cache: HashMap<(u64, u64), CachedRow>,
    /// キャッシュが対応する内容の版
    cache_version: u64,
    colors: Colors,
    font_family: String,
    font_size_pt: f32,
    tab_width: u32,
    dpi: f32,
    /// これまでに描画した行の最大幅（水平スクロールバー用）
    pub max_text_width: f32,
}

const GUTTER_PAD: f32 = 8.0;
const TEXT_PAD: f32 = 4.0;
/// 1 行のレイアウト幅の上限（折り返さないので十分大きくする）
const LAYOUT_MAX_WIDTH: f32 = 1.0e7;

fn color_f(c: Color) -> D2D1_COLOR_F {
    let [r, g, b] = c.to_f32();
    D2D1_COLOR_F { r, g, b, a: 1.0 }
}

impl Renderer {
    pub fn new(
        font_family: &str,
        font_size_pt: f32,
        tab_width: u32,
        colors: Colors,
        dpi: u32,
    ) -> Result<Renderer> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
                .context("D2D1CreateFactory")?;
            let dwrite: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).context("DWriteCreateFactory")?;
            let (text_format, number_format, metrics) =
                create_formats(&dwrite, font_family, font_size_pt, tab_width)?;
            Ok(Renderer {
                d2d,
                dwrite,
                target: None,
                text_format,
                number_format,
                metrics,
                cache: HashMap::new(),
                cache_version: 0,
                colors,
                font_family: font_family.to_owned(),
                font_size_pt,
                tab_width,
                dpi: dpi as f32,
                max_text_width: 0.0,
            })
        }
    }

    pub fn metrics(&self) -> FontMetrics {
        self.metrics
    }

    pub fn font_size_pt(&self) -> f32 {
        self.font_size_pt
    }

    /// フォントサイズを変更する（Ctrl+ホイール等）。
    pub fn set_font_size(&mut self, pt: f32) -> Result<()> {
        let pt = pt.clamp(4.0, 72.0);
        let (t, n, m) =
            unsafe { create_formats(&self.dwrite, &self.font_family, pt, self.tab_width)? };
        self.text_format = t;
        self.number_format = n;
        self.metrics = m;
        self.font_size_pt = pt;
        self.clear_cache();
        Ok(())
    }

    pub fn set_dpi(&mut self, dpi: u32) {
        self.dpi = dpi as f32;
        if let Some(t) = &self.target {
            unsafe { t.rt.SetDpi(self.dpi, self.dpi) };
        }
    }

    pub fn px_to_dip(&self, px: f32) -> f32 {
        px * 96.0 / self.dpi
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.max_text_width = 0.0;
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if let Some(rt) = self.target.as_ref().and_then(|t| t.hwnd_rt.as_ref()) {
            let _ = unsafe {
                rt.Resize(&D2D_SIZE_U {
                    width: width.max(1),
                    height: height.max(1),
                })
            };
        }
    }

    /// 行番号欄の幅（DIP）。
    pub fn gutter_width(&self, digits: usize, show: bool) -> f32 {
        if show {
            (digits.max(3) as f32) * self.metrics.char_width + GUTTER_PAD * 2.0
        } else {
            0.0
        }
    }

    pub fn text_origin_x(&self, digits: usize, show: bool) -> f32 {
        self.gutter_width(digits, show) + TEXT_PAD
    }

    fn ensure_target(&mut self, hwnd: HWND, width: u32, height: u32) -> Result<()> {
        if self.target.is_some() {
            return Ok(());
        }
        unsafe {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN,
                    alphaMode: D2D1_ALPHA_MODE_UNKNOWN,
                },
                dpiX: self.dpi,
                dpiY: self.dpi,
                ..Default::default()
            };
            let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U {
                    width: width.max(1),
                    height: height.max(1),
                },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let hwnd_rt = self
                .d2d
                .CreateHwndRenderTarget(&props, &hwnd_props)
                .context("CreateHwndRenderTarget")?;
            let rt: ID2D1RenderTarget = hwnd_rt.cast()?;
            self.target = Some(Target::new(rt, Some(hwnd_rt), &self.colors)?);
            // ブラシは描画ターゲットに属するため、ブラシを参照するレイアウトも作り直す
            self.cache.clear();
        }
        Ok(())
    }

    /// 内容の版が変わっていたら行レイアウトのキャッシュを捨てる。
    pub fn set_version(&mut self, version: u64) {
        if version != self.cache_version {
            self.cache.clear();
            self.cache_version = version;
        }
    }

    /// 行のレイアウトを作る。`insert` があれば、その位置（`row.text` 内のバイト位置）に
    /// 文字列を差し込んで下線を引く（IME の変換中文字列）。
    fn build_layout(&self, row: &Row, insert: Option<(usize, &str)>) -> Result<RowLayout> {
        let brushes = self.target.as_ref().map(|t| &t.brushes);
        let mut wide: Vec<u16> = Vec::with_capacity(row.text.len() + 16);
        let mut effects: Vec<(u32, u32, &ID2D1SolidColorBrush)> = Vec::new();
        let mut underline = None;
        for span in &row.spans {
            let mut piece = |r: std::ops::Range<usize>, wide: &mut Vec<u16>| {
                let start = wide.len() as u32;
                wide.extend(row.text[r].encode_utf16());
                let len = wide.len() as u32 - start;
                if let Some(b) = brushes {
                    match span.kind {
                        SpanKind::Text => {}
                        SpanKind::Invalid => effects.push((start, len, &b.invalid)),
                        SpanKind::Control => effects.push((start, len, &b.control)),
                    }
                }
            };
            match insert {
                Some((at, text)) if span.range.start <= at && at < span.range.end => {
                    piece(span.range.start..at, &mut wide);
                    let s = wide.len() as u32;
                    wide.extend(text.encode_utf16());
                    underline = Some((s, wide.len() as u32 - s));
                    piece(at..span.range.end, &mut wide);
                }
                _ => piece(span.range.clone(), &mut wide),
            }
        }
        if let Some((at, text)) = insert
            && underline.is_none()
            && at >= row.text.len()
        {
            let s = wide.len() as u32;
            wide.extend(text.encode_utf16());
            underline = Some((s, wide.len() as u32 - s));
        }
        unsafe {
            let layout = self.dwrite.CreateTextLayout(
                &wide,
                &self.text_format,
                LAYOUT_MAX_WIDTH,
                self.metrics.line_height,
            )?;
            for (start, length, brush) in effects {
                layout.SetDrawingEffect(
                    &brush.cast::<windows::core::IUnknown>()?,
                    DWRITE_TEXT_RANGE {
                        startPosition: start,
                        length,
                    },
                )?;
            }
            if let Some((start, length)) = underline {
                layout.SetUnderline(
                    true,
                    DWRITE_TEXT_RANGE {
                        startPosition: start,
                        length,
                    },
                )?;
            }
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            Ok(RowLayout {
                layout,
                width: m.widthIncludingTrailingWhitespace,
                wide: wide.into(),
            })
        }
    }

    fn row_layout(&mut self, row: &Row) -> Result<RowLayout> {
        let key = (row.start, row.next);
        if let Some(c) = self.cache.get_mut(&key) {
            c.used = true;
            return Ok(c.row.clone());
        }
        let rl = self.build_layout(row, None)?;
        // 描画ターゲットがない（ブラシがない）状態で作ったレイアウトは色がないのでキャッシュしない
        if self.target.is_some() {
            self.cache.insert(
                key,
                CachedRow {
                    row: rl.clone(),
                    used: true,
                },
            );
        }
        Ok(rl)
    }

    /// UTF-16 テキストの先頭 `len` 文字分の幅（ヒットテストが使えない環境向けの代替）。
    fn prefix_width(&self, wide: &[u16]) -> f32 {
        if wide.is_empty() {
            return 0.0;
        }
        unsafe {
            let Ok(layout) = self.dwrite.CreateTextLayout(
                wide,
                &self.text_format,
                LAYOUT_MAX_WIDTH,
                self.metrics.line_height,
            ) else {
                return 0.0;
            };
            let mut m = DWRITE_TEXT_METRICS::default();
            match layout.GetMetrics(&mut m) {
                Ok(()) => m.widthIncludingTrailingWhitespace,
                Err(_) => 0.0,
            }
        }
    }

    /// UTF-16 位置 `idx` のキャレットの x 座標。
    fn x_at(&self, rl: &RowLayout, idx: usize) -> f32 {
        let idx = idx.min(rl.wide.len());
        unsafe {
            let (mut x, mut y) = (0.0f32, 0.0f32);
            let mut m = DWRITE_HIT_TEST_METRICS::default();
            match rl
                .layout
                .HitTestTextPosition(idx as u32, false, &mut x, &mut y, &mut m)
            {
                Ok(()) => x,
                // HitTestTextPosition が実装されていない環境（Wine など）では幅を測って求める
                Err(_) => self.prefix_width(&rl.wide[..idx]),
            }
        }
    }

    /// x 座標に最も近い文字境界の UTF-16 位置。
    fn index_at(&self, rl: &RowLayout, x: f32) -> usize {
        unsafe {
            let mut trailing = windows::core::BOOL(0);
            let mut inside = windows::core::BOOL(0);
            let mut m = DWRITE_HIT_TEST_METRICS::default();
            if rl
                .layout
                .HitTestPoint(x, 1.0, &mut trailing, &mut inside, &mut m)
                .is_ok()
            {
                return m.textPosition as usize
                    + if trailing.as_bool() {
                        m.length as usize
                    } else {
                        0
                    };
            }
        }
        // 代替: 文字境界（サロゲートの途中を除く）を二分探索する
        let bounds: Vec<usize> = (0..=rl.wide.len())
            .filter(|&i| i == rl.wide.len() || !(0xDC00..=0xDFFF).contains(&rl.wide[i]))
            .collect();
        let (mut lo, mut hi) = (0usize, bounds.len() - 1);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.prefix_width(&rl.wide[..bounds[mid]]) < x {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo > 0 {
            let right = self.prefix_width(&rl.wide[..bounds[lo]]);
            let left = self.prefix_width(&rl.wide[..bounds[lo - 1]]);
            if x - left < right - x {
                return bounds[lo - 1];
            }
        }
        bounds[lo]
    }

    /// 行 `row` の中でオフセット `offset` の位置の x 座標（本文の左端からの DIP）。
    pub fn caret_x(&mut self, row: &Row, offset: u64) -> f32 {
        let idx = utf16_index(&row.text, row.text_index(offset));
        match self.row_layout(row) {
            Ok(rl) => self.x_at(&rl, idx),
            Err(_) => 0.0,
        }
    }

    /// 行 `row` の中で x 座標（本文の左端からの DIP）に最も近い文字境界のオフセット。
    pub fn hit_test(&mut self, row: &Row, x: f32) -> u64 {
        let Ok(rl) = self.row_layout(row) else {
            return row.start;
        };
        let pos16 = self.index_at(&rl, x);
        row.offset_at(utf8_index(&row.text, pos16))
    }

    /// 1 画面分を描画する。
    ///
    /// 描画ターゲットが失われた（GPU のリセット等）場合は作り直しの準備をして `Ok(false)` を返す。
    /// 呼び出し側は再描画を要求すること。
    pub fn draw(&mut self, hwnd: HWND, width: u32, height: u32, frame: &Frame) -> Result<bool> {
        self.ensure_target(hwnd, width, height)?;
        self.draw_frame(frame)
    }

    /// 画面外のビットマップに描画し、BGRA（premultiplied）の画素列を返す。
    ///
    /// 描画の自動テストと、描画結果の確認用。ウィンドウ用の描画ターゲットには影響しない。
    pub fn render_offscreen(&mut self, width: u32, height: u32, frame: &Frame) -> Result<Vec<u8>> {
        use windows::Win32::Graphics::Imaging::*;
        use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
        unsafe {
            let wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .context("CoCreateInstance(WICImagingFactory)")?;
            let bitmap = wic
                .CreateBitmap(
                    width,
                    height,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapCacheOnDemand,
                )
                .context("CreateBitmap")?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: self.dpi,
                dpiY: self.dpi,
                ..Default::default()
            };
            let rt = self
                .d2d
                .CreateWicBitmapRenderTarget(&bitmap, &props)
                .context("CreateWicBitmapRenderTarget")?;
            let saved = self.target.replace(Target::new(rt, None, &self.colors)?);
            self.cache.clear();
            let result = self.draw_frame(frame);
            self.target = saved;
            self.cache.clear();
            result?;
            let stride = width * 4;
            let mut pixels = vec![0u8; (stride * height) as usize];
            bitmap
                .CopyPixels(std::ptr::null(), stride, &mut pixels)
                .context("CopyPixels")?;
            Ok(pixels)
        }
    }

    fn draw_frame(&mut self, frame: &Frame) -> Result<bool> {
        self.set_version(frame.version);
        for c in self.cache.values_mut() {
            c.used = false;
        }
        let lh = self.metrics.line_height;
        let cw = self.metrics.char_width;
        let size = unsafe { self.target.as_ref().unwrap().rt.GetSize() };
        let gutter = self.gutter_width(frame.line_digits, frame.show_line_numbers);
        let text_x = gutter + TEXT_PAD - frame.scroll_x;

        // 先にレイアウトと装飾（選択範囲の矩形・キャレット位置）を計算する
        let mut layouts = Vec::with_capacity(frame.rows.len());
        let mut sel_rects: Vec<D2D_RECT_F> = Vec::new();
        let mut caret_rects: Vec<D2D_RECT_F> = Vec::new();
        for (i, row) in frame.rows.iter().enumerate() {
            let y = i as f32 * lh;
            let comp = frame.composition.and_then(|c| {
                c.offsets
                    .iter()
                    .find(|&&o| row.shows_caret(o))
                    .map(|&o| (row.text_index(o), o, c))
            });
            let rl = match comp {
                Some((at, _, c)) => self.build_layout(row, Some((at, &c.text)))?,
                None => self.row_layout(row)?,
            };
            self.max_text_width = self.max_text_width.max(rl.width);

            // 選択範囲
            for r in frame.selections {
                if r.end < row.start || r.start > row.end || (r.start == row.end && !row.ends_line)
                {
                    continue;
                }
                let a = utf16_index(&row.text, row.text_index(r.start.max(row.start)));
                let b = utf16_index(&row.text, row.text_index(r.end.min(row.end)));
                if b > a {
                    // 行は折り返さない 1 行のレイアウトなので、両端のキャレット位置から矩形を求める
                    let (xa, xb) = (self.x_at(&rl, a), self.x_at(&rl, b));
                    sel_rects.push(D2D_RECT_F {
                        left: text_x + xa.min(xb),
                        top: y,
                        right: text_x + xa.max(xb),
                        bottom: y + lh,
                    });
                }
                // 改行も選択されている場合は行末に小さな矩形を描く
                if row.ends_line && r.end > row.end && r.start <= row.end {
                    let x = text_x + self.x_at(&rl, utf16_index(&row.text, row.text.len()));
                    sel_rects.push(D2D_RECT_F {
                        left: x,
                        top: y,
                        right: x + cw * 0.6,
                        bottom: y + lh,
                    });
                }
            }

            // キャレット
            if frame.caret_visible {
                let width = if frame.overwrite { cw.max(2.0) } else { 2.0 };
                for &c in frame.carets {
                    if !row.shows_caret(c) {
                        continue;
                    }
                    let mut idx = utf16_index(&row.text, row.text_index(c));
                    if let Some((_, _, comp)) = comp.filter(|(_, o, _)| *o == c) {
                        idx += comp.cursor;
                    }
                    let x = text_x + self.x_at(&rl, idx);
                    caret_rects.push(D2D_RECT_F {
                        left: x - 0.5,
                        top: y + 1.0,
                        right: x - 0.5 + width,
                        bottom: y + lh - 1.0,
                    });
                }
            }
            // 矩形選択（行末より右の仮想空白を含む）
            for rp in frame.rect.iter().filter(|rp| rp.row_start == row.start) {
                let x_of = |(o, v): (u64, u32)| {
                    text_x
                        + self.x_at(&rl, utf16_index(&row.text, row.text_index(o)))
                        + v as f32 * cw
                };
                let (l, r) = (x_of(rp.left), x_of(rp.right));
                if r > l {
                    sel_rects.push(D2D_RECT_F {
                        left: l,
                        top: y,
                        right: r,
                        bottom: y + lh,
                    });
                }
                if let Some(c) = rp.caret
                    && frame.caret_visible
                {
                    let x = x_of(c);
                    caret_rects.push(D2D_RECT_F {
                        left: x - 0.5,
                        top: y + 1.0,
                        right: x + 1.5,
                        bottom: y + lh - 1.0,
                    });
                }
            }
            layouts.push(rl.layout);
        }
        self.cache.retain(|_, c| c.used);

        let target = self.target.as_ref().unwrap();
        let rt = &target.rt;
        let b = &target.brushes;
        let result = unsafe {
            rt.BeginDraw();
            rt.SetTransform(&windows_numerics::Matrix3x2::identity());
            rt.Clear(Some(&color_f(self.colors.background)));

            if frame.show_line_numbers {
                rt.FillRectangle(
                    &D2D_RECT_F {
                        left: 0.0,
                        top: 0.0,
                        right: gutter,
                        bottom: size.height,
                    },
                    &b.gutter_background,
                );
                let mut line = frame.first_line;
                for (i, row) in frame.rows.iter().enumerate() {
                    if i > 0 && row.line_start {
                        line += 1;
                    }
                    if !row.line_start {
                        continue;
                    }
                    let y = i as f32 * lh;
                    let text: Vec<u16> = (line + 1).to_string().encode_utf16().collect();
                    let brush = if frame.line_exact {
                        &b.line_number
                    } else {
                        &b.line_number_estimated
                    };
                    rt.DrawText(
                        &text,
                        &self.number_format,
                        &D2D_RECT_F {
                            left: 0.0,
                            top: y,
                            right: gutter - GUTTER_PAD,
                            bottom: y + lh,
                        },
                        brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }

            rt.PushAxisAlignedClip(
                &D2D_RECT_F {
                    left: gutter,
                    top: 0.0,
                    right: size.width,
                    bottom: size.height,
                },
                D2D1_ANTIALIAS_MODE_ALIASED,
            );
            for r in &sel_rects {
                rt.FillRectangle(r, &b.selection);
            }
            for (i, layout) in layouts.iter().enumerate() {
                rt.DrawTextLayout(
                    Vector2 {
                        X: text_x,
                        Y: i as f32 * lh,
                    },
                    layout,
                    &b.foreground,
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
            }
            for r in &caret_rects {
                rt.FillRectangle(r, &b.caret);
            }
            rt.PopAxisAlignedClip();
            rt.EndDraw(None, None)
        };
        if let Err(e) = result {
            if e.code() == D2DERR_RECREATE_TARGET {
                self.target = None;
                self.cache.clear();
                return Ok(false);
            }
            return Err(e);
        }
        Ok(true)
    }
}

/// UTF-8 のバイト位置を UTF-16 の位置に変換する。
fn utf16_index(text: &str, byte_idx: usize) -> usize {
    text[..byte_idx.min(text.len())].encode_utf16().count()
}

/// UTF-16 の位置を UTF-8 のバイト位置に変換する（サロゲートの途中は前に丸める）。
fn utf8_index(text: &str, u16_idx: usize) -> usize {
    let mut n = 0;
    for (i, c) in text.char_indices() {
        if n >= u16_idx {
            return i;
        }
        n += c.len_utf16();
        if n > u16_idx {
            return i;
        }
    }
    text.len()
}

/// 指定のフォントがインストールされていなければ、等幅の代替フォントを選ぶ。
fn resolve_family(dwrite: &IDWriteFactory, requested: &str) -> String {
    const FALLBACKS: [&str; 5] = [
        "Consolas",
        "BIZ UDゴシック",
        "MS Gothic",
        "Courier New",
        "Segoe UI",
    ];
    unsafe {
        let mut collection = None;
        if dwrite
            .GetSystemFontCollection(&mut collection, false)
            .is_err()
        {
            return requested.to_owned();
        }
        let Some(collection) = collection else {
            return requested.to_owned();
        };
        let exists = |name: &str| {
            let mut index = 0u32;
            let mut found = windows::core::BOOL(0);
            collection
                .FindFamilyName(&HSTRING::from(name), &mut index, &mut found)
                .is_ok()
                && found.as_bool()
        };
        if let Some(name) = std::iter::once(requested)
            .chain(FALLBACKS)
            .find(|n| exists(n))
        {
            return name.to_owned();
        }
        // どれもなければ最初にインストールされているフォント
        if collection.GetFontFamilyCount() > 0
            && let Ok(fam) = collection.GetFontFamily(0)
            && let Ok(names) = fam.GetFamilyNames()
        {
            let mut buf = [0u16; 128];
            if names.GetString(0, &mut buf).is_ok() {
                let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
                return String::from_utf16_lossy(&buf[..n]);
            }
        }
        requested.to_owned()
    }
}

/// 本文用・行番号用のテキスト形式とフォントの寸法を作る。
unsafe fn create_formats(
    dwrite: &IDWriteFactory,
    family: &str,
    size_pt: f32,
    tab_width: u32,
) -> Result<(IDWriteTextFormat, IDWriteTextFormat, FontMetrics)> {
    let size_dip = size_pt * 96.0 / 72.0;
    let family = HSTRING::from(resolve_family(dwrite, family));
    let make = || unsafe {
        dwrite.CreateTextFormat(
            &family,
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size_dip,
            w!("ja-jp"),
        )
    };
    unsafe {
        let text = make().context("CreateTextFormat")?;
        let number = make().context("CreateTextFormat")?;

        // 寸法の計測
        let probe: Vec<u16> = "M".encode_utf16().collect();
        let layout = dwrite
            .CreateTextLayout(&probe, &text, LAYOUT_MAX_WIDTH, LAYOUT_MAX_WIDTH)
            .context("CreateTextLayout")?;
        let mut tm = DWRITE_TEXT_METRICS::default();
        layout.GetMetrics(&mut tm).context("GetMetrics")?;
        let mut lm = [DWRITE_LINE_METRICS::default(); 1];
        let mut count = 0u32;
        layout
            .GetLineMetrics(Some(&mut lm), &mut count)
            .context("GetLineMetrics")?;
        let line_height = lm[0].height.ceil();
        let baseline = lm[0].baseline;
        let char_width = tm.widthIncludingTrailingWhitespace;

        for f in [&text, &number] {
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)
                .context("SetWordWrapping")?;
            // 日本語フォントへのフォールバックで行の高さが変わらないよう固定する
            f.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, line_height, baseline)
                .context("SetLineSpacing")?;
            f.SetIncrementalTabStop(char_width * tab_width.max(1) as f32)
                .context("SetIncrementalTabStop")?;
        }
        number
            .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)
            .context("SetTextAlignment")?;
        Ok((
            text,
            number,
            FontMetrics {
                line_height,
                char_width,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use yy_buffer::Snapshot;
    use yy_layout::{RowConfig, rows_from};

    fn pixel(p: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * width + x) * 4) as usize;
        [p[i + 2], p[i + 1], p[i]]
    }

    /// 行番号欄・本文・不正バイトが実際に描画されることを画素で確認する。
    #[test]
    fn renders_rows_offscreen() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let colors = Colors::default();
        let mut r = Renderer::new("Consolas", 11.0, 4, colors.clone(), 96).unwrap();
        let snap = Snapshot::from_bytes(
            "hello world\n日本語のテキスト\n\tタブ\nbad \u{FFFD}".replace('\u{FFFD}', "\u{1}"),
        );
        let snap = snap.insert(snap.len(), b"\xFF\xFE\n");
        let rows = rows_from(&snap, RowConfig::default(), 0, 10);
        assert_eq!(rows.len(), 5);
        let frame = Frame {
            version: 0,
            rows: &rows,
            first_line: 0,
            line_exact: true,
            line_digits: 1,
            show_line_numbers: true,
            scroll_x: 0.0,
            selections: &[],
            carets: &[],
            caret_visible: false,
            overwrite: false,
            composition: None,
            rect: &[],
        };
        let (w, h) = (400, 120);
        let px = r.render_offscreen(w, h, &frame).unwrap();
        assert_eq!(px.len(), (w * h * 4) as usize);

        let bg = colors.background;
        let gutter_bg = colors.gutter_background;
        assert_eq!(pixel(&px, w, 1, 1), [gutter_bg.r, gutter_bg.g, gutter_bg.b]);
        assert_eq!(pixel(&px, w, w - 1, h - 1), [bg.r, bg.g, bg.b]);

        // 本文の 1 行目に背景色以外の画素（文字）がある
        let gutter = r.gutter_width(1, true) as u32;
        let lh = r.metrics().line_height as u32;
        let inked = |x0: u32, x1: u32, y0: u32, y1: u32, pred: &dyn Fn([u8; 3]) -> bool| {
            (y0..y1).any(|y| (x0..x1).any(|x| pred(pixel(&px, w, x, y))))
        };
        assert!(inked(gutter + 2, w, 0, lh, &|c| c != [bg.r, bg.g, bg.b]));
        // 行番号欄に文字がある
        assert!(inked(0, gutter, 0, lh, &|c| c
            != [gutter_bg.r, gutter_bg.g, gutter_bg.b]));
        // 不正バイト（4 行目の末尾）は赤系で描かれる
        assert!(inked(gutter, w, lh * 3, lh * 4, &|c| c[0] > 150
            && c[1] < 120
            && c[2] < 120));

        if let Some(path) = std::env::var_os("YY_RENDER_DUMP") {
            std::fs::write(path, crate::util::encode_bmp(w, h, &px)).unwrap();
        }
    }

    /// 選択範囲の背景とキャレットが指定の行にだけ描かれることを確認する。
    #[test]
    fn renders_selection_and_caret() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let colors = Colors::default();
        let mut r = Renderer::new("Consolas", 11.0, 4, colors.clone(), 96).unwrap();
        let snap = Snapshot::from_bytes("hello world\nsecond line\n");
        let rows = rows_from(&snap, RowConfig::default(), 0, 10);
        let selections = vec![std::ops::Range {
            start: 0u64,
            end: 5,
        }];
        let carets = [5u64];
        let frame = Frame {
            version: 0,
            rows: &rows,
            first_line: 0,
            line_exact: true,
            line_digits: 1,
            show_line_numbers: false,
            scroll_x: 0.0,
            selections: &selections,
            carets: &carets,
            caret_visible: true,
            overwrite: false,
            composition: None,
            rect: &[],
        };
        let (w, h) = (300, 60);
        let px = r.render_offscreen(w, h, &frame).unwrap();
        let lh = r.metrics().line_height as u32;
        if let Some(path) = std::env::var_os("YY_RENDER_DUMP_SELECTION") {
            std::fs::write(path, crate::util::encode_bmp(w, h, &px)).unwrap();
        }
        let any_in = |y0: u32, y1: u32, c: Color| {
            (y0..y1).any(|y| (0..w).any(|x| pixel(&px, w, x, y) == [c.r, c.g, c.b]))
        };
        assert!(any_in(0, lh, colors.selection), "selection on row 1");
        assert!(
            !any_in(lh + 1, lh * 2, colors.selection),
            "no selection on row 2"
        );
        assert!(any_in(2, lh - 2, colors.caret), "caret on row 1");
    }
}

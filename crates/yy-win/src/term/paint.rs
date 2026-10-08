//! 端末の画面の描画（Direct2D / DirectWrite）。
//!
//! 文字は桁の格子に合わせて置く。ASCII の続きは 1 回の DrawText でまとめて描き（等幅の
//! フォントなら格子に合う）、それ以外（全角・結合文字など）は 1 文字ずつ桁の位置に描く。
//! フォントはエディタと同じく同梱の UDEV Gothic を既定にする。

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{HSTRING, Result, w};
use yy_term::screen::{BASE16, flags, indexed_rgb};
use yy_term::{Attr, Color, CursorShape, Pos, Terminal};

use crate::util::Context;

/// 既定の文字色・背景色
pub(crate) const DEFAULT_FG: (u8, u8, u8) = (204, 204, 204);
pub(crate) const DEFAULT_BG: (u8, u8, u8) = (12, 12, 12);
/// 選択範囲の背景
const SELECTION: (u8, u8, u8) = (38, 79, 120);
/// 画面の端の余白（DIP）
pub(crate) const PADDING: f32 = 4.0;

/// 描画に使う状態。
pub(crate) struct Painter {
    d2d: ID2D1Factory,
    dwrite: IDWriteFactory,
    fonts: Option<IDWriteFontCollection>,
    family: String,
    size_pt: f32,
    dpi: f32,
    /// 標準・太字・斜体・太字斜体
    formats: Vec<IDWriteTextFormat>,
    /// 1 桁の幅と 1 行の高さ（DIP）
    pub cell_w: f32,
    pub cell_h: f32,
    target: Option<(ID2D1HwndRenderTarget, ID2D1SolidColorBrush)>,
}

/// リンクの下線の色
const LINK_COLOR: (u8, u8, u8) = (90, 160, 255);

/// 描く内容。
pub(crate) struct Scene<'a> {
    pub term: &'a Terminal,
    /// さかのぼって表示している行数
    pub back: usize,
    /// 選択範囲（`start` から `end` の手前まで）
    pub selection: Option<(Pos, Pos)>,
    /// マウスの下のリンク（行ごとの範囲。下線を引く）
    pub link: &'a [(Pos, Pos)],
    pub focused: bool,
}

fn rgb_f((r, g, b): (u8, u8, u8)) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    }
}

/// 色を RGB にする。`bold` なら基本の 8 色を明るい色にする。
fn resolve(c: Color, default: (u8, u8, u8), bold: bool) -> (u8, u8, u8) {
    match c {
        Color::Default => default,
        Color::Indexed(i) if bold && i < 8 => indexed_rgb(i + 8, &BASE16),
        Color::Indexed(i) => indexed_rgb(i, &BASE16),
        Color::Rgb(r, g, b) => (r, g, b),
    }
}

/// セルの文字色と背景色。
fn cell_colors(a: &Attr, selected: bool, reverse_video: bool) -> ((u8, u8, u8), (u8, u8, u8)) {
    let mut fg = resolve(a.fg, DEFAULT_FG, a.has(flags::BOLD));
    let mut bg = resolve(a.bg, DEFAULT_BG, false);
    if a.has(flags::INVERSE) != reverse_video {
        std::mem::swap(&mut fg, &mut bg);
    }
    if a.has(flags::DIM) {
        fg = (
            ((u16::from(fg.0) + u16::from(bg.0)) / 2) as u8,
            ((u16::from(fg.1) + u16::from(bg.1)) / 2) as u8,
            ((u16::from(fg.2) + u16::from(bg.2)) / 2) as u8,
        );
    }
    if a.has(flags::HIDDEN) {
        fg = bg;
    }
    if selected {
        bg = SELECTION;
        if a.has(flags::HIDDEN) {
            fg = bg;
        }
    }
    (fg, bg)
}

impl Painter {
    pub(crate) fn new(family: &str, size_pt: f32, dpi: u32) -> Result<Painter> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
                .context("D2D1CreateFactory")?;
            let dwrite: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).context("DWriteCreateFactory")?;
            let fonts = crate::font::collection(&dwrite).ok();
            let mut p = Painter {
                d2d,
                dwrite,
                fonts,
                family: family.to_owned(),
                size_pt,
                dpi: dpi.max(96) as f32,
                formats: Vec::new(),
                cell_w: 8.0,
                cell_h: 16.0,
                target: None,
            };
            p.make_formats()?;
            Ok(p)
        }
    }

    pub(crate) fn size_pt(&self) -> f32 {
        self.size_pt
    }

    pub(crate) fn dpi(&self) -> f32 {
        self.dpi
    }

    /// フォントサイズを変える。
    pub(crate) fn set_size(&mut self, pt: f32) -> Result<()> {
        self.size_pt = pt.clamp(5.0, 72.0);
        self.make_formats()
    }

    pub(crate) fn set_dpi(&mut self, dpi: u32) {
        self.dpi = dpi.max(96) as f32;
        self.target = None;
    }

    fn make_formats(&mut self) -> Result<()> {
        let (family, collection) =
            crate::render::resolve_family(&self.dwrite, self.fonts.as_ref(), &self.family);
        let family = HSTRING::from(family);
        let size_dip = self.size_pt * 96.0 / 72.0;
        let mut formats = Vec::new();
        unsafe {
            for (weight, style) in [
                (DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL),
                (DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_STYLE_NORMAL),
                (DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_ITALIC),
                (DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_STYLE_ITALIC),
            ] {
                let f = self
                    .dwrite
                    .CreateTextFormat(
                        &family,
                        collection.as_ref(),
                        weight,
                        style,
                        DWRITE_FONT_STRETCH_NORMAL,
                        size_dip,
                        w!("ja-jp"),
                    )
                    .context("CreateTextFormat")?;
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                formats.push(f);
            }
            // 1 桁の幅（"0" の送り幅）と行の高さ
            let probe: Vec<u16> = "0000000000".encode_utf16().collect();
            let layout = self
                .dwrite
                .CreateTextLayout(&probe, &formats[0], 1.0e6, 1.0e6)
                .context("CreateTextLayout")?;
            let mut tm = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut tm)?;
            let mut lm = [DWRITE_LINE_METRICS::default(); 1];
            let mut count = 0u32;
            layout.GetLineMetrics(Some(&mut lm), &mut count)?;
            let line_h = lm[0].height.ceil();
            for f in &formats {
                // 日本語のフォールバックで行の高さが変わらないよう固定する
                f.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, line_h, lm[0].baseline)?;
            }
            self.cell_w = tm.widthIncludingTrailingWhitespace / 10.0;
            self.cell_h = line_h;
        }
        self.formats = formats;
        Ok(())
    }

    /// クライアント領域（ピクセル）に入る桁数と行数。
    pub(crate) fn grid_size(&self, width_px: i32, height_px: i32) -> (usize, usize) {
        let scale = 96.0 / self.dpi;
        let w = (width_px as f32 * scale - PADDING * 2.0).max(0.0);
        let h = (height_px as f32 * scale - PADDING * 2.0).max(0.0);
        (
            ((w / self.cell_w) as usize).max(2),
            ((h / self.cell_h) as usize).max(1),
        )
    }

    /// ピクセルの位置を `(行, 桁)` にする（`round` なら桁の境目の近い方）。
    pub(crate) fn cell_at(&self, x_px: i32, y_px: i32, round: bool) -> (isize, isize) {
        let scale = 96.0 / self.dpi;
        let x = (x_px as f32 * scale - PADDING) / self.cell_w;
        let y = (y_px as f32 * scale - PADDING) / self.cell_h;
        let col = if round { x.round() } else { x.floor() };
        (y.floor() as isize, col as isize)
    }

    /// `(行, 桁)` の左上のピクセル位置。
    pub(crate) fn cell_px(&self, row: usize, col: usize) -> (i32, i32) {
        let s = self.dpi / 96.0;
        (
            ((PADDING + col as f32 * self.cell_w) * s) as i32,
            ((PADDING + row as f32 * self.cell_h) * s) as i32,
        )
    }

    pub(crate) fn line_px(&self) -> i32 {
        (self.cell_h * self.dpi / 96.0).ceil() as i32
    }

    pub(crate) fn resize_target(&mut self, width: u32, height: u32) {
        if let Some((t, _)) = &self.target {
            unsafe {
                if t.Resize(&D2D_SIZE_U {
                    width: width.max(1),
                    height: height.max(1),
                })
                .is_err()
                {
                    self.target = None;
                }
            }
        }
    }

    fn ensure_target(&mut self, hwnd: HWND, width: u32, height: u32) -> Result<()> {
        if self.target.is_some() {
            return Ok(());
        }
        unsafe {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
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
            let rt = self
                .d2d
                .CreateHwndRenderTarget(&props, &hwnd_props)
                .context("CreateHwndRenderTarget")?;
            let brush = rt.CreateSolidColorBrush(&rgb_f(DEFAULT_FG), None)?;
            self.target = Some((rt, brush));
        }
        Ok(())
    }

    /// 端末の画面を描く。
    pub(crate) fn paint(
        &mut self,
        hwnd: HWND,
        width: u32,
        height: u32,
        scene: Option<&Scene>,
    ) -> Result<()> {
        self.ensure_target(hwnd, width, height)?;
        let (rt, brush) = self.target.as_ref().expect("target");
        let r = unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&rgb_f(DEFAULT_BG)));
            if let Some(s) = scene {
                self.draw(rt, brush, s);
            }
            rt.EndDraw(None, None)
        };
        if let Err(e) = r {
            // デバイスが失われた: 次の描画で作り直す
            self.target = None;
            if e.code() != D2DERR_RECREATE_TARGET {
                return Err(e);
            }
        }
        Ok(())
    }

    fn draw(&self, rt: &ID2D1HwndRenderTarget, brush: &ID2D1SolidColorBrush, s: &Scene) {
        let term = s.term;
        let reverse = term.modes().reverse_video;
        let back = s.back.min(term.history_len());
        let first = term.screen_line(0) - back as u64;
        let (cw, ch) = (self.cell_w, self.cell_h);
        let sel = s
            .selection
            .map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        let selected = |line: u64, col: usize| {
            sel.is_some_and(|(a, b)| {
                let p = Pos { line, col };
                a <= p && p < b
            })
        };
        let rect = |x: f32, y: f32, w: f32, h: f32| D2D_RECT_F {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        };
        let fill = |r: D2D_RECT_F, c: (u8, u8, u8)| unsafe {
            brush.SetColor(&rgb_f(c));
            rt.FillRectangle(&r, brush);
        };
        let text = |t: &str, x: f32, y: f32, w: f32, a: &Attr, fg: (u8, u8, u8)| unsafe {
            let wide: Vec<u16> = t.encode_utf16().collect();
            let i = usize::from(a.has(flags::BOLD)) | (usize::from(a.has(flags::ITALIC)) << 1);
            brush.SetColor(&rgb_f(fg));
            rt.DrawText(
                &wide,
                &self.formats[i],
                &rect(x, y, w.max(cw), ch),
                brush,
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        };

        for row in 0..term.rows() {
            let line_no = first + row as u64;
            let line = term.view_line(back, row);
            let y = PADDING + row as f32 * ch;
            let cells = &line.cells[..line.cells.len().min(term.cols())];
            // 背景（同じ色の続きをまとめて塗る）
            let mut col = 0;
            while col < cells.len() {
                let (_, bg) = cell_colors(&cells[col].attr, selected(line_no, col), reverse);
                let start = col;
                col += 1;
                while col < cells.len()
                    && cell_colors(&cells[col].attr, selected(line_no, col), reverse).1 == bg
                {
                    col += 1;
                }
                if bg != DEFAULT_BG {
                    fill(
                        rect(
                            PADDING + start as f32 * cw,
                            y,
                            (col - start) as f32 * cw,
                            ch,
                        ),
                        bg,
                    );
                }
            }
            // 文字（ASCII の続きはまとめる）
            let mut run = String::new();
            let mut run_start = 0;
            let mut run_key: Option<(Attr, (u8, u8, u8))> = None;
            let flush = |run: &mut String, start: usize, key: &Option<(Attr, (u8, u8, u8))>| {
                if let Some((a, fg)) = key
                    && !run.trim_end().is_empty()
                {
                    text(
                        run,
                        PADDING + start as f32 * cw,
                        y,
                        run.len() as f32 * cw,
                        a,
                        *fg,
                    );
                }
                run.clear();
            };
            for (c, cell) in cells.iter().enumerate() {
                if cell.is_continuation() {
                    continue;
                }
                let (fg, _) = cell_colors(&cell.attr, selected(line_no, c), reverse);
                let x = PADDING + c as f32 * cw;
                let w = f32::from(cell.width.max(1)) * cw;
                // 下線・取り消し線
                if cell.attr.has(flags::UNDERLINE) || cell.attr.has(flags::DOUBLE_UNDERLINE) {
                    fill(rect(x, y + ch - 2.0, w, 1.0), fg);
                    if cell.attr.has(flags::DOUBLE_UNDERLINE) {
                        fill(rect(x, y + ch - 4.0, w, 1.0), fg);
                    }
                }
                if cell.attr.has(flags::STRIKE) {
                    fill(rect(x, y + ch / 2.0, w, 1.0), fg);
                }
                let ascii = cell.width == 1 && cell.combining.is_none() && cell.ch.is_ascii();
                let key = Some((cell.attr, fg));
                if ascii {
                    if key != run_key || run_start + run.len() != c {
                        flush(&mut run, run_start, &run_key);
                        run_start = c;
                        run_key = key;
                    }
                    run.push(cell.ch);
                } else {
                    flush(&mut run, run_start, &run_key);
                    run_key = None;
                    if cell.ch != ' ' || cell.combining.is_some() {
                        text(&cell.text(), x, y, w, &cell.attr, fg);
                    }
                }
            }
            flush(&mut run, run_start, &run_key);
            // マウスの下のリンクに下線
            for (a, b) in s.link.iter().filter(|(a, _)| a.line == line_no) {
                fill(
                    rect(
                        PADDING + a.col as f32 * cw,
                        y + ch - 2.0,
                        (b.col.saturating_sub(a.col)) as f32 * cw,
                        1.5,
                    ),
                    LINK_COLOR,
                );
            }
        }

        // カーソル（さかのぼって表示しているときは、画面に入っていれば描く）
        let m = term.modes();
        let (crow, ccol) = term.cursor();
        let view_row = crow + back;
        if m.cursor_visible && view_row < term.rows() {
            let line = term.screen_row(crow);
            let cell = line.cells.get(ccol);
            let w = cell.map_or(1, |c| c.width.max(1)) as f32 * cw;
            let x = PADDING + ccol as f32 * cw;
            let y = PADDING + view_row as f32 * ch;
            let color = (220, 220, 220);
            if !s.focused {
                unsafe {
                    brush.SetColor(&rgb_f(color));
                    rt.DrawRectangle(&rect(x + 0.5, y + 0.5, w - 1.0, ch - 1.0), brush, 1.0, None);
                }
            } else {
                match m.cursor_shape {
                    CursorShape::Block => {
                        fill(rect(x, y, w, ch), color);
                        if let Some(c) = cell
                            && (c.ch != ' ' || c.combining.is_some())
                        {
                            text(&c.text(), x, y, w, &c.attr, DEFAULT_BG);
                        }
                    }
                    CursorShape::Underline => fill(rect(x, y + ch - 2.0, w, 2.0), color),
                    CursorShape::Bar => fill(rect(x, y, 2.0, ch), color),
                }
            }
        }
    }
}

/// 描画ターゲットを作り直す必要があるときのエラー
const D2DERR_RECREATE_TARGET: windows::core::HRESULT =
    windows::core::HRESULT(0x8899000C_u32 as i32);

#[cfg(test)]
mod tests {
    use super::*;

    /// 同梱フォントは半角が 1 桁、全角がちょうど 2 桁（格子に合わせて描ける）。
    #[test]
    fn bundled_font_fits_the_grid() {
        let p = Painter::new(crate::font::BUNDLED_FAMILY, 11.0, 96).unwrap();
        assert!(
            p.cell_w > 3.0 && p.cell_h > p.cell_w,
            "{} {}",
            p.cell_w,
            p.cell_h
        );
        let width = |s: &str| unsafe {
            let text: Vec<u16> = s.encode_utf16().collect();
            let layout = p
                .dwrite
                .CreateTextLayout(&text, &p.formats[0], 1.0e6, 1.0e6)
                .unwrap();
            let mut tm = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut tm).unwrap();
            tm.widthIncludingTrailingWhitespace
        };
        let ascii = width("abcdefghij") / 10.0;
        let wide = width("日本語のかな") / 6.0;
        assert!((ascii - p.cell_w).abs() < 0.01, "{ascii} {}", p.cell_w);
        assert!((wide - 2.0 * p.cell_w).abs() < 0.05, "{wide} {}", p.cell_w);
        let (cols, rows) = p.grid_size(800, 600);
        assert!(cols > 50 && rows > 20, "{cols}x{rows}");
    }
}

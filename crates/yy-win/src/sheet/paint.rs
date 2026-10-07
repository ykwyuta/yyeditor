//! 格子の描画（Direct2D / DirectWrite。15 章 12.1）。
//!
//! 見える範囲のセルだけを描く。列見出し・行番号・罫線・値（表示形式を当てた文字列）・選択範囲・
//! アクティブなセルの枠。

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{HSTRING, Result, w};
use windows_numerics::Vector2;

use crate::util::Context;

/// デバイスが失われた（描き直すときに作り直す）
const D2DERR_RECREATE_TARGET: windows::core::HRESULT =
    windows::core::HRESULT(0x8899000C_u32 as i32);

const BG: (u8, u8, u8) = (255, 255, 255);
const FG: (u8, u8, u8) = (0, 0, 0);
const GRID: (u8, u8, u8) = (218, 220, 224);
const HEAD_BG: (u8, u8, u8) = (243, 243, 243);
const HEAD_FG: (u8, u8, u8) = (68, 68, 68);
const HEAD_SEL_BG: (u8, u8, u8) = (210, 223, 214);
const HEAD_SEL_FG: (u8, u8, u8) = (16, 92, 52);
const SEL_FILL: (u8, u8, u8) = (198, 222, 206);
const ACTIVE: (u8, u8, u8) = (33, 115, 70);
const TABLE_HEAD_BG: (u8, u8, u8) = (231, 238, 233);
/// 絞り込み・並べ替えの表示中の行番号
const VIEW_ROW_FG: (u8, u8, u8) = (0, 84, 166);
const BUTTON_BG: (u8, u8, u8) = (252, 252, 252);
const BUTTON_ON_BG: (u8, u8, u8) = (0, 84, 166);

/// 列見出しのボタンの状態。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ButtonState {
    /// 絞り込みの条件がある
    pub filtered: bool,
    /// 並べ替えのキー（`Some(true)` は降順）
    pub sorted: Option<bool>,
}

/// 文字の寄せ方。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Align {
    Left,
    Right,
    Center,
}

/// 描くセル。
#[derive(Clone, Debug)]
pub(crate) struct Cell {
    pub text: String,
    pub align: Align,
    pub color: Option<(u8, u8, u8)>,
    /// 表の見出し（列の名前）
    pub table_head: bool,
}

/// 描く内容（位置はすべて DIP、格子の左上が原点）。
pub(crate) struct Scene<'a> {
    /// 見える列（列番号・左端・幅）
    pub cols: &'a [(u32, f32, f32)],
    /// 見える行（行番号・上端）
    pub rows: &'a [(u64, f32)],
    pub header_w: f32,
    /// `[行][列]`
    pub cells: &'a [Vec<Cell>],
    /// 選択範囲（上・左・下・右。含む）
    pub sel: (u64, u32, u64, u32),
    pub active: (u64, u32),
    /// 編集中（アクティブなセルの中身を描かない）
    pub editing: bool,
    /// 行番号として出す数と、絞り込み・並べ替えの表示中か（`rows` と同じ並び）
    pub row_labels: &'a [(u64, bool)],
    /// ボタンを出す列（列番号・状態）
    pub buttons: &'a [(u32, ButtonState)],
}

fn rgb_f((r, g, b): (u8, u8, u8)) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    }
}

fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: l,
        top: t,
        right: r,
        bottom: b,
    }
}

/// 描画の状態。
pub(crate) struct GridPainter {
    d2d: ID2D1Factory,
    dwrite: IDWriteFactory,
    fonts: Option<IDWriteFontCollection>,
    family: String,
    size_pt: f32,
    dpi: f32,
    /// 左寄せ・右寄せ・中央・見出し（中央）
    formats: Vec<IDWriteTextFormat>,
    /// 数字 1 文字の幅と行の高さ（DIP）
    pub char_w: f32,
    pub row_h: f32,
    target: Option<(ID2D1HwndRenderTarget, ID2D1SolidColorBrush)>,
}

impl GridPainter {
    pub(crate) fn new(family: &str, size_pt: f32, dpi: u32) -> Result<GridPainter> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
                .context("D2D1CreateFactory")?;
            let dwrite: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).context("DWriteCreateFactory")?;
            let fonts = crate::font::collection(&dwrite).ok();
            let mut p = GridPainter {
                d2d,
                dwrite,
                fonts,
                family: family.to_owned(),
                size_pt,
                dpi: dpi.max(96) as f32,
                formats: Vec::new(),
                char_w: 7.0,
                row_h: 20.0,
                target: None,
            };
            p.make_formats()?;
            Ok(p)
        }
    }

    pub(crate) fn dpi(&self) -> f32 {
        self.dpi
    }

    pub(crate) fn size_pt(&self) -> f32 {
        self.size_pt
    }

    pub(crate) fn set_size(&mut self, pt: f32) -> Result<()> {
        self.size_pt = pt.clamp(6.0, 48.0);
        self.make_formats()
    }

    pub(crate) fn set_dpi(&mut self, dpi: u32) {
        self.dpi = dpi.max(96) as f32;
        self.target = None;
    }

    fn make_formats(&mut self) -> Result<()> {
        let (family, collection) = if self.family.is_empty() {
            ("Yu Gothic UI".to_owned(), None)
        } else {
            crate::render::resolve_family(&self.dwrite, self.fonts.as_ref(), &self.family)
        };
        let family = HSTRING::from(family);
        let size_dip = self.size_pt * 96.0 / 72.0;
        let mut formats = Vec::new();
        unsafe {
            for align in [
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_TEXT_ALIGNMENT_TRAILING,
                DWRITE_TEXT_ALIGNMENT_CENTER,
                DWRITE_TEXT_ALIGNMENT_CENTER,
            ] {
                let f = self
                    .dwrite
                    .CreateTextFormat(
                        &family,
                        collection.as_ref(),
                        DWRITE_FONT_WEIGHT_NORMAL,
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        size_dip,
                        w!("ja-jp"),
                    )
                    .context("CreateTextFormat")?;
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                f.SetTextAlignment(align)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                formats.push(f);
            }
            let probe: Vec<u16> = "0000000000".encode_utf16().collect();
            let layout = self
                .dwrite
                .CreateTextLayout(&probe, &formats[0], 1.0e6, 1.0e6)
                .context("CreateTextLayout")?;
            let mut tm = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut tm)?;
            self.char_w = tm.widthIncludingTrailingWhitespace / 10.0;
            self.row_h = (tm.height * 1.45).ceil();
        }
        self.formats = formats;
        Ok(())
    }

    /// 列幅（文字数）を DIP にする（Excel と同じく、数字の幅 × 文字数 ＋ 余白）。
    /// 列見出しのボタンの幅（DIP）。列の右端に置く。
    pub(crate) fn button_w(&self) -> f32 {
        (self.row_h - 4.0).max(8.0)
    }

    pub(crate) fn col_px(&self, chars: f32) -> f32 {
        (chars * self.char_w + 5.0).round()
    }

    /// DIP を列幅（文字数）にする。
    pub(crate) fn col_chars(&self, dip: f32) -> f32 {
        ((dip - 5.0) / self.char_w).max(0.0)
    }

    pub(crate) fn to_dip(&self, px: i32) -> f32 {
        px as f32 * 96.0 / self.dpi
    }

    pub(crate) fn to_px(&self, dip: f32) -> i32 {
        (dip * self.dpi / 96.0).round() as i32
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
            let brush = rt.CreateSolidColorBrush(&rgb_f(FG), None)?;
            self.target = Some((rt, brush));
        }
        Ok(())
    }

    pub(crate) fn paint(&mut self, hwnd: HWND, width: u32, height: u32, s: &Scene) -> Result<()> {
        self.ensure_target(hwnd, width, height)?;
        let (rt, brush) = self.target.as_ref().expect("target");
        let r = unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&rgb_f(BG)));
            self.draw(
                rt,
                brush,
                s,
                self.to_dip(width as i32),
                self.to_dip(height as i32),
            );
            rt.EndDraw(None, None)
        };
        if let Err(e) = r {
            self.target = None;
            if e.code() != D2DERR_RECREATE_TARGET {
                return Err(e);
            }
        }
        Ok(())
    }

    fn text(
        &self,
        rt: &ID2D1HwndRenderTarget,
        brush: &ID2D1SolidColorBrush,
        text: &str,
        fmt: usize,
        r: D2D_RECT_F,
        color: (u8, u8, u8),
    ) {
        if text.is_empty() {
            return;
        }
        let w: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            brush.SetColor(&rgb_f(color));
            rt.DrawText(
                &w,
                &self.formats[fmt],
                &r,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    fn fill(
        &self,
        rt: &ID2D1HwndRenderTarget,
        brush: &ID2D1SolidColorBrush,
        r: D2D_RECT_F,
        c: (u8, u8, u8),
    ) {
        unsafe {
            brush.SetColor(&rgb_f(c));
            rt.FillRectangle(&r, brush);
        }
    }

    /// 線（`(x0, y0, x1, y1)`、太さ 1）。
    fn line(
        &self,
        rt: &ID2D1HwndRenderTarget,
        brush: &ID2D1SolidColorBrush,
        (x0, y0, x1, y1): (f32, f32, f32, f32),
        c: (u8, u8, u8),
    ) {
        unsafe {
            brush.SetColor(&rgb_f(c));
            rt.DrawLine(
                Vector2 { X: x0, Y: y0 },
                Vector2 { X: x1, Y: y1 },
                brush,
                1.0,
                None,
            );
        }
    }

    fn draw(
        &self,
        rt: &ID2D1HwndRenderTarget,
        brush: &ID2D1SolidColorBrush,
        s: &Scene,
        w: f32,
        h: f32,
    ) {
        let hh = self.row_h;
        let hw = s.header_w;
        let (top, left, bottom, right) = s.sel;
        let pad = 3.0;
        // セル
        for (ri, &(row, y)) in s.rows.iter().enumerate() {
            for (ci, &(col, x, cw)) in s.cols.iter().enumerate() {
                let r = rect(hw + x, hh + y, hw + x + cw, hh + y + hh);
                let selected = (top..=bottom).contains(&row) && (left..=right).contains(&col);
                let cell = s.cells.get(ri).and_then(|v| v.get(ci));
                if selected && (row, col) != s.active {
                    self.fill(rt, brush, r, SEL_FILL);
                } else if cell.is_some_and(|c| c.table_head) {
                    self.fill(rt, brush, r, TABLE_HEAD_BG);
                }
                if let Some(c) = cell
                    && !(s.editing && (row, col) == s.active)
                {
                    let fmt = match c.align {
                        Align::Left => 0,
                        Align::Right => 1,
                        Align::Center => 2,
                    };
                    self.text(
                        rt,
                        brush,
                        &c.text,
                        fmt,
                        rect(r.left + pad, r.top, r.right - pad, r.bottom),
                        c.color.unwrap_or(FG),
                    );
                }
            }
        }
        // 罫線
        for &(_, x, cw) in s.cols {
            self.line(
                rt,
                brush,
                (hw + x + cw - 0.5, hh, hw + x + cw - 0.5, h),
                GRID,
            );
        }
        for &(_, y) in s.rows {
            self.line(
                rt,
                brush,
                (hw, hh + y + hh - 0.5, w, hh + y + hh - 0.5),
                GRID,
            );
        }
        // 見出し
        self.fill(rt, brush, rect(0.0, 0.0, w, hh), HEAD_BG);
        self.fill(rt, brush, rect(0.0, 0.0, hw, h), HEAD_BG);
        let bw = self.button_w();
        for &(col, x, cw) in s.cols {
            let r = rect(hw + x, 0.0, hw + x + cw, hh);
            let on = (left..=right).contains(&col);
            if on {
                self.fill(rt, brush, r, HEAD_SEL_BG);
            }
            let button = s.buttons.iter().find(|b| b.0 == col).map(|b| b.1);
            let mut tr = r;
            if let Some(b) = button
                && cw > bw * 2.0
            {
                tr.right -= bw + 2.0;
                let br = rect(r.right - bw - 2.0, 2.0, r.right - 2.0, hh - 2.0);
                let active = b.filtered || b.sorted.is_some();
                self.fill(rt, brush, br, if active { BUTTON_ON_BG } else { BUTTON_BG });
                unsafe {
                    brush.SetColor(&rgb_f(GRID));
                    rt.DrawRectangle(&br, brush, 1.0, None);
                }
                let glyph = match (b.filtered, b.sorted) {
                    (_, Some(false)) => "↑",
                    (_, Some(true)) => "↓",
                    _ => "▼",
                };
                self.text(rt, brush, glyph, 3, br, if active { BG } else { HEAD_FG });
            }
            self.text(
                rt,
                brush,
                &yy_sheet::col_name(col),
                3,
                tr,
                if on { HEAD_SEL_FG } else { HEAD_FG },
            );
            self.line(rt, brush, (r.right - 0.5, 0.0, r.right - 0.5, hh), GRID);
        }
        for (ri, &(row, y)) in s.rows.iter().enumerate() {
            let r = rect(0.0, hh + y, hw, hh + y + hh);
            let on = (top..=bottom).contains(&row);
            if on {
                self.fill(rt, brush, r, HEAD_SEL_BG);
            }
            let (label, in_view) = s.row_labels.get(ri).copied().unwrap_or((row + 1, false));
            self.text(
                rt,
                brush,
                &label.to_string(),
                3,
                r,
                if on {
                    HEAD_SEL_FG
                } else if in_view {
                    VIEW_ROW_FG
                } else {
                    HEAD_FG
                },
            );
            self.line(rt, brush, (0.0, r.bottom - 0.5, hw, r.bottom - 0.5), GRID);
        }
        self.line(rt, brush, (0.0, hh - 0.5, w, hh - 0.5), GRID);
        self.line(rt, brush, (hw - 0.5, 0.0, hw - 0.5, h), GRID);
        // 選択範囲とアクティブなセルの枠
        let col_x = |c: u32| s.cols.iter().find(|x| x.0 == c).map(|x| (x.1, x.2));
        let row_y = |r: u64| s.rows.iter().find(|x| x.0 == r).map(|x| x.1);
        let first_c = s.cols.first().map(|c| c.0).unwrap_or(0);
        let last_c = s.cols.last().map(|c| c.0).unwrap_or(0);
        let first_r = s.rows.first().map(|r| r.0).unwrap_or(0);
        let last_r = s.rows.last().map(|r| r.0).unwrap_or(0);
        if right >= first_c && left <= last_c && bottom >= first_r && top <= last_r {
            let lc = left.max(first_c);
            let rc = right.min(last_c);
            let tr = top.max(first_r);
            let br = bottom.min(last_r);
            if let (Some((lx, _)), Some((rx, rw)), Some(ty), Some(by)) =
                (col_x(lc), col_x(rc), row_y(tr), row_y(br))
            {
                let r = rect(hw + lx, hh + ty, hw + rx + rw, hh + by + hh);
                unsafe {
                    brush.SetColor(&rgb_f(ACTIVE));
                    rt.DrawRectangle(&r, brush, 2.0, None);
                }
            }
        }
        if let (Some((ax, aw)), Some(ay)) = (col_x(s.active.1), row_y(s.active.0)) {
            let r = rect(
                hw + ax + 1.0,
                hh + ay + 1.0,
                hw + ax + aw - 1.0,
                hh + ay + hh - 1.0,
            );
            unsafe {
                brush.SetColor(&rgb_f(ACTIVE));
                rt.DrawRectangle(&r, brush, 2.0, None);
            }
        }
    }
}

//! 式の入力の補助（15 章 12.3）。
//!
//! - 式を入力しているとき、演算子・`(`・`,`・`=` の直後でセルをクリック・ドラッグ（Shift+クリックで範囲を
//!   広げる）・列見出し・行番号をクリックすると、その参照を式に入れる（Excel の「参照」モード）。続けて
//!   クリックすると入れた参照を差し替える。文字を入力すると確定する。セルの編集を文字の入力で始めたとき
//!   は、矢印キー（Shift+矢印で範囲）でも選べる。
//! - 参照を入れられるときにシートのタブをクリックすると、編集を続けたままそのシートを表示し、`Sheet2!`
//!   を入れる（セルをクリックすると `Sheet2!B3`）。確定・取り消しで元のシート・セルに戻る。
//! - F4 で参照の `$` を切り替える。
//! - 関数名を入力している間は候補の一覧（↑↓で選び、Tab・Enter・クリックで入れる。Esc で閉じる）、
//!   関数のかっこの中では引数の書き方（いまの引数を太字）と説明を出す。
//! - 式の中の参照に色の枠を付ける。セルの編集の内容は数式バーにも映す。

use windows::Win32::Foundation::{COLORREF, SIZE};
use windows::Win32::Graphics::Gdi::{
    COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_INFOBK, COLOR_INFOTEXT,
    COLOR_WINDOW, COLOR_WINDOWTEXT, CreateFontIndirectW, DT_CALCRECT, DT_NOPREFIX, DT_WORDBREAK,
    DrawTextW, FillRect, GetDC, GetObjectW, GetSysColor, GetSysColorBrush, GetTextExtentPoint32W,
    GetTextMetricsW, HBRUSH, HDC, LOGFONTW, ReleaseDC, SYS_COLOR_INDEX, SelectObject, SetBkMode,
    SetTextColor, TEXTMETRICW, TRANSPARENT, TextOutW,
};
use yy_formula::{Area, AreaKind, FuncInfo};

use super::paint::{MARK_COLORS, Range4};
use super::*;

pub(super) const ASSIST_CLASS: PCWSTR = w!("YYSheetAssist");

/// 別のシートの参照を選んでいる間の、元のシートの表示（確定・取り消しで戻す）。
pub(super) struct Home {
    pub sheet: usize,
    top: u64,
    left: u32,
    cur: (u64, u32),
    anchor: (u64, u32),
    whole: (bool, bool),
}

/// 式に入れている参照。
pub(super) struct Point {
    /// 入れている EDIT（セルの編集か数式バー）
    pub hwnd: HWND,
    /// 入れた参照の位置（UTF-16）
    start: usize,
    end: usize,
    pub anchor: (u64, u32),
    pub cur: (u64, u32),
    /// 列全体・行全体
    pub whole: (bool, bool),
    /// 参照のシート（表示しているシート）
    pub sheet: usize,
    /// シートのタブをクリックして `Sheet2!` だけを入れた（セルはまだ）
    pub pending: bool,
}

impl Point {
    /// 枠を描く範囲（`Sheet2!` だけならなし）。
    pub fn range(&self) -> Option<Range4> {
        if self.pending {
            return None;
        }
        let (t, b) = (self.anchor.0.min(self.cur.0), self.anchor.0.max(self.cur.0));
        let (l, r) = (self.anchor.1.min(self.cur.1), self.anchor.1.max(self.cur.1));
        Some(match self.whole {
            (true, _) => (0, l, u64::MAX, r),
            (_, true) => (t, 0, b, yy_formula::MAX_COL),
            _ => (t, l, b, r),
        })
    }

    fn area(&self) -> Range4 {
        let (t, b) = (self.anchor.0.min(self.cur.0), self.anchor.0.max(self.cur.0));
        let (l, r) = (self.anchor.1.min(self.cur.1), self.anchor.1.max(self.cur.1));
        (t, l, b, r)
    }
}

/// 候補・引数の書き方の小窓。
enum Mode {
    Hidden,
    List {
        items: Vec<&'static FuncInfo>,
        sel: usize,
        /// 入力中の関数名の始め（文字の位置）
        start: usize,
    },
    Sig {
        info: &'static FuncInfo,
        param: Option<usize>,
    },
}

pub(super) struct Assist {
    pub hwnd: HWND,
    font: HFONT,
    bold: HFONT,
    mode: Mode,
    /// 候補を出している EDIT
    target: HWND,
    /// Esc で閉じた候補（関数名の始めの位置）。名前を入れ終わるまで出さない
    dismissed: Option<usize>,
}

const PAD: i32 = 6;

impl Assist {
    /// 小窓を作る（隠しておく）。
    pub fn create(frame: HWND, instance: HINSTANCE, ui_font: HFONT) -> Assist {
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                ASSIST_CLASS,
                w!(""),
                WS_POPUP | WS_BORDER,
                0,
                0,
                10,
                10,
                Some(frame),
                None,
                Some(instance),
                None,
            )
            .unwrap_or_default();
            let mut lf = LOGFONTW::default();
            GetObjectW(
                HGDIOBJ(ui_font.0),
                std::mem::size_of::<LOGFONTW>() as i32,
                Some(&mut lf as *mut _ as *mut _),
            );
            lf.lfWeight = 700;
            let bold = CreateFontIndirectW(&lf);
            Assist {
                hwnd,
                font: ui_font,
                bold,
                mode: Mode::Hidden,
                target: HWND::default(),
                dismissed: None,
            }
        }
    }

    fn visible(&self) -> bool {
        !matches!(self.mode, Mode::Hidden)
    }

    fn hide(&mut self) {
        if self.visible() {
            self.mode = Mode::Hidden;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    /// 測る（`draw` なら描く）。大きさ（px）を返す。
    fn render(&self, hdc: HDC, draw: bool) -> (i32, i32) {
        unsafe {
            let old = SelectObject(hdc, HGDIOBJ(self.font.0));
            let mut tm = TEXTMETRICW::default();
            let _ = GetTextMetricsW(hdc, &mut tm);
            let line = tm.tmHeight + 2;
            let extent = |s: &str, font: HFONT| -> i32 {
                SelectObject(hdc, HGDIOBJ(font.0));
                let w: Vec<u16> = s.encode_utf16().collect();
                let mut sz = SIZE::default();
                let _ = GetTextExtentPoint32W(hdc, &w, &mut sz);
                sz.cx
            };
            let out = |x: i32, y: i32, s: &str, font: HFONT| {
                SelectObject(hdc, HGDIOBJ(font.0));
                let w: Vec<u16> = s.encode_utf16().collect();
                let _ = TextOutW(hdc, x, y, &w);
            };
            let sys = |i: SYS_COLOR_INDEX| COLORREF(GetSysColor(i));
            // 説明（折り返す）
            let wrap = |s: &str, x: i32, y: i32, w: i32, draw: bool| -> i32 {
                SelectObject(hdc, HGDIOBJ(self.font.0));
                let mut text: Vec<u16> = s.encode_utf16().collect();
                let mut rc = RECT {
                    left: x,
                    top: y,
                    right: x + w,
                    bottom: y + line,
                };
                let flags = DT_WORDBREAK | DT_NOPREFIX;
                DrawTextW(hdc, &mut text, &mut rc, flags | DT_CALCRECT);
                if draw {
                    DrawTextW(hdc, &mut text, &mut rc, flags);
                }
                rc.bottom - rc.top
            };
            if draw {
                SetBkMode(hdc, TRANSPARENT);
            }
            let min_w = 260 * tm.tmHeight / 16;
            let size = match &self.mode {
                Mode::Hidden => (0, 0),
                Mode::List { items, sel, .. } => {
                    let names_w = items
                        .iter()
                        .map(|f| extent(f.name, self.font))
                        .max()
                        .unwrap_or(0);
                    let w = (names_w + PAD * 2).max(min_w);
                    let mut y = PAD / 2;
                    if draw {
                        let bg = GetSysColorBrush(COLOR_WINDOW);
                        FillRect(
                            hdc,
                            &RECT {
                                left: 0,
                                top: 0,
                                right: 4000,
                                bottom: 4000,
                            },
                            bg,
                        );
                    }
                    for (i, f) in items.iter().enumerate() {
                        if draw {
                            if i == *sel {
                                let hl: HBRUSH = GetSysColorBrush(COLOR_HIGHLIGHT);
                                FillRect(
                                    hdc,
                                    &RECT {
                                        left: 0,
                                        top: y,
                                        right: w,
                                        bottom: y + line,
                                    },
                                    hl,
                                );
                                SetTextColor(hdc, sys(COLOR_HIGHLIGHTTEXT));
                            } else {
                                SetTextColor(hdc, sys(COLOR_WINDOWTEXT));
                            }
                            out(PAD, y + 1, f.name, self.font);
                        }
                        y += line;
                    }
                    if let Some(f) = items.get(*sel) {
                        y += PAD / 2;
                        if draw {
                            SetTextColor(hdc, sys(COLOR_GRAYTEXT));
                        }
                        y += wrap(f.desc, PAD, y, w - PAD * 2, draw);
                    }
                    (w, y + PAD / 2)
                }
                Mode::Sig { info, param } => {
                    let parts = info.signature();
                    let font_of = |idx: Option<usize>| {
                        if idx.is_some() && idx == *param {
                            self.bold
                        } else {
                            self.font
                        }
                    };
                    let sig_w: i32 = parts.iter().map(|(s, i)| extent(s, font_of(*i))).sum();
                    let w = (sig_w + PAD * 2).max(min_w);
                    if draw {
                        let bg = GetSysColorBrush(COLOR_INFOBK);
                        FillRect(
                            hdc,
                            &RECT {
                                left: 0,
                                top: 0,
                                right: 4000,
                                bottom: 4000,
                            },
                            bg,
                        );
                        SetTextColor(hdc, sys(COLOR_INFOTEXT));
                        let mut x = PAD;
                        for (s, i) in &parts {
                            out(x, PAD / 2, s, font_of(*i));
                            x += extent(s, font_of(*i));
                        }
                        SetTextColor(hdc, sys(COLOR_GRAYTEXT));
                    }
                    let y = PAD / 2 + line + 2;
                    let h = wrap(info.desc, PAD, y, w - PAD * 2, draw);
                    (w, y + h + PAD / 2)
                }
            };
            SelectObject(hdc, old);
            size
        }
    }
}

/// 文字の番号 → UTF-16 の位置。
fn to_u16(text: &str, chars: usize) -> usize {
    text.chars().take(chars).map(char::len_utf16).sum()
}

/// UTF-16 の位置 → 文字の番号。
fn to_chars(text: &str, u16pos: usize) -> usize {
    let mut u = 0;
    let mut n = 0;
    for c in text.chars() {
        if u >= u16pos {
            break;
        }
        u += c.len_utf16();
        n += 1;
    }
    n
}

/// EDIT の選択（UTF-16 の始め・終わり）。
fn edit_sel(h: HWND) -> (usize, usize) {
    let (mut s, mut e) = (0u32, 0u32);
    unsafe {
        SendMessageW(
            h,
            EM_GETSEL,
            Some(WPARAM(&mut s as *mut u32 as usize)),
            Some(LPARAM(&mut e as *mut u32 as isize)),
        );
    }
    (s as usize, e as usize)
}

/// EDIT の `start..end`（UTF-16）を置き換える（元に戻せる）。
fn edit_replace(h: HWND, start: usize, end: usize, text: &str) {
    let w = HSTRING::from(text);
    unsafe {
        SendMessageW(
            h,
            EM_SETSEL,
            Some(WPARAM(start)),
            Some(LPARAM(end as isize)),
        );
        SendMessageW(
            h,
            EM_REPLACESEL,
            Some(WPARAM(1)),
            Some(LPARAM(w.as_ptr() as isize)),
        );
    }
}

impl App {
    /// 式を入力している EDIT（セルの編集か、フォーカスのある数式バー）。
    pub(super) fn entry_target(&self) -> Option<HWND> {
        if let Some(ed) = &self.editor {
            return Some(ed.hwnd);
        }
        (unsafe { GetFocus() } == self.formula).then_some(self.formula)
    }

    /// 参照を入れられるか（式で、参照を入れている途中か、演算子などの直後）。
    fn can_point(&self, h: HWND) -> bool {
        let text = window_text(h);
        if !text.starts_with('=') {
            return false;
        }
        if self.point.as_ref().is_some_and(|p| p.hwnd == h) {
            return true;
        }
        let (s, _) = edit_sel(h);
        yy_formula::typing(&text, to_chars(&text, s)).can_ref
    }

    /// 参照を入れる（入れている途中なら差し替える）。
    fn point_set(&mut self, h: HWND, anchor: (u64, u32), cur: (u64, u32), whole: (bool, bool)) {
        let (start, end) = match &self.point {
            Some(p) if p.hwnd == h => (p.start, p.end),
            _ => edit_sel(h),
        };
        let mut p = Point {
            hwnd: h,
            start,
            end,
            anchor,
            cur,
            whole,
            sheet: self.sheet,
            pending: false,
        };
        let text = self.point_text(&p);
        edit_replace(h, p.start, p.end, &text);
        p.end = p.start + text.encode_utf16().count();
        self.point = Some(p);
        self.refresh_assist();
        self.invalidate();
    }

    /// 参照の書き方（`A1`・`A1:B3`・`B:D`・`3:5`。別のシートなら `Sheet2!A1`）。
    fn point_text(&self, p: &Point) -> String {
        let prefix = match &self.home {
            Some(h) if h.sheet != p.sheet => {
                yy_formula::sheet_prefix(&self.doc.book.sheets[p.sheet].name)
            }
            _ => String::new(),
        };
        if p.pending {
            return prefix;
        }
        let (t, l, b, r) = p.area();
        let sh = &self.doc.book.sheets[p.sheet];
        let area = match p.whole {
            (true, _) => Area {
                r0: 0,
                c0: l,
                r1: u64::MAX,
                c1: r,
                abs: [false; 4],
                kind: AreaKind::Cols,
            },
            (_, true) => Area {
                r0: sh.source_row(t),
                c0: 0,
                r1: sh.source_row(b),
                c1: u32::MAX,
                abs: [false; 4],
                kind: AreaKind::Rows,
            },
            _ => Area {
                r0: sh.source_row(t),
                c0: l,
                r1: sh.source_row(b),
                c1: r,
                abs: [false; 4],
                kind: if (t, l) == (b, r) {
                    AreaKind::Cell
                } else {
                    AreaKind::Range
                },
            },
        };
        format!("{prefix}{}", yy_formula::area_text(&area))
    }

    /// 参照を入れられるときにシートのタブを選んだ: 編集を続けたままそのシートを表示し、`Sheet2!` を
    /// 入れる。元のシートに戻ったら `Sheet2!` だけの参照は消す。扱ったら `true`。
    pub(super) fn point_sheet(&mut self, i: usize) -> bool {
        let Some(h) = self.entry_target() else {
            return false;
        };
        if i >= self.doc.book.sheets.len() || !self.can_point(h) {
            return false;
        }
        if i != self.sheet {
            if self.home.is_none() {
                self.home = Some(Home {
                    sheet: self.sheet,
                    top: self.top,
                    left: self.left,
                    cur: self.cur,
                    anchor: self.anchor,
                    whole: self.whole,
                });
            }
            let home = self.home.as_ref().map(|x| x.sheet).unwrap_or(self.sheet);
            if i == home {
                self.go_home();
                if let Some(p) = self.point.take_if(|p| p.hwnd == h && p.pending) {
                    edit_replace(h, p.start, p.end, "");
                }
            } else {
                self.sheet = i;
                self.top = 0;
                self.left = 0;
                self.cur = (0, 0);
                self.anchor = (0, 0);
                self.whole = (false, false);
                let (start, end) = match &self.point {
                    Some(p) if p.hwnd == h => (p.start, p.end),
                    _ => edit_sel(h),
                };
                let prefix = yy_formula::sheet_prefix(&self.doc.book.sheets[i].name);
                edit_replace(h, start, end, &prefix);
                self.point = Some(Point {
                    hwnd: h,
                    start,
                    end: start + prefix.encode_utf16().count(),
                    anchor: (0, 0),
                    cur: (0, 0),
                    whole: (false, false),
                    sheet: i,
                    pending: true,
                });
                set_status(
                    "参照するセルをクリックしてください（Enter で確定、Esc で取り消して元のシートに戻ります）",
                );
            }
            self.update_scrollbars();
            self.place_editor();
            self.invalidate();
        }
        unsafe {
            let _ = SetFocus(Some(h));
        }
        true
    }

    /// 元のシートの表示に戻る（別のシートの参照を選んでいたとき）。
    pub(super) fn go_home(&mut self) {
        let Some(h) = self.home.take() else {
            return;
        };
        self.sheet = h.sheet;
        self.top = h.top;
        self.left = h.left;
        self.cur = h.cur;
        self.anchor = h.anchor;
        self.whole = h.whole;
        unsafe {
            SendMessageW(self.tabs, TCM_SETCURSEL, Some(WPARAM(h.sheet)), None);
        }
        self.update_scrollbars();
        self.invalidate();
    }

    /// 格子のクリックで参照を入れる（式の入力中で、入れられる位置のとき）。入れたら `true`。
    pub(super) fn point_click(&mut self, x: i32, y: i32, shift: bool) -> bool {
        let Some(h) = self.entry_target() else {
            return false;
        };
        if !self.can_point(h) {
            return false;
        }
        let (row, col) = self.hit(x, y);
        let (cell, whole) = match (row, col) {
            (Some(r), Some(c)) => ((r, c), (false, false)),
            (None, Some(c)) => ((0, c), (true, false)),
            (Some(r), None) => ((r, 0), (false, true)),
            (None, None) => return false,
        };
        let anchor = match &self.point {
            Some(p)
                if shift
                    && p.hwnd == h
                    && p.whole == whole
                    && p.sheet == self.sheet
                    && !p.pending =>
            {
                p.anchor
            }
            _ => cell,
        };
        self.point_set(h, anchor, cell, whole);
        self.drag = Some(Drag::Point);
        unsafe {
            let _ = SetFocus(Some(h));
        }
        true
    }

    /// 参照を入れながらのドラッグ。
    pub(super) fn point_drag(&mut self, x: i32, y: i32) {
        let Some(p) = &self.point else {
            return;
        };
        let (h, anchor, whole, old) = (p.hwnd, p.anchor, p.whole, p.cur);
        let (row, col) = self.hit(x, y);
        let r = row.unwrap_or(self.top);
        let c = col.unwrap_or(self.cols.last().map(|c| c.0).unwrap_or(0));
        let cur = match whole {
            (true, _) => (0, c),
            (_, true) => (r, 0),
            _ => (r, c),
        };
        if cur != old {
            self.point_set(h, anchor, cur, whole);
            self.reveal(cur);
        }
    }

    /// 矢印キーで参照を選ぶ（文字の入力で始めたセルの編集か、参照を入れている途中）。扱ったら `true`。
    fn point_key(&mut self, h: HWND, dr: i64, dc: i64, extend: bool) -> bool {
        let Some(ed) = &self.editor else {
            return false;
        };
        if ed.hwnd != h {
            return false;
        }
        let (cell, enter_mode) = (ed.cell, ed.enter_mode);
        let active = self
            .point
            .as_ref()
            .filter(|p| p.hwnd == h && p.whole == (false, false) && p.sheet == self.sheet);
        let (anchor, cur) = match active {
            Some(p) => (p.anchor, p.cur),
            None if enter_mode && self.can_point(h) => (cell, cell),
            None => return false,
        };
        let cur = (
            (cur.0 as i64 + dr).max(0) as u64,
            (cur.1 as i64 + dc).clamp(0, yy_formula::MAX_COL as i64) as u32,
        );
        let anchor = if extend { anchor } else { cur };
        self.point_set(h, anchor, cur, (false, false));
        self.reveal(cur);
        true
    }

    /// セルが見えるように動かす（選択は変えない）。
    pub(super) fn reveal(&mut self, cell: (u64, u32)) {
        let (top, left) = (self.top, self.left);
        let saved = self.cur;
        self.cur = cell;
        self.ensure_visible();
        self.cur = saved;
        if (top, left) != (self.top, self.left) {
            self.update_scrollbars();
            self.place_editor();
            self.invalidate();
        }
    }

    /// セルの編集の欄をセルに合わせる（見えなければ外へ出す。入力はそのまま続けられる）。
    pub(super) fn place_editor(&mut self) {
        self.compute_layout();
        let Some(ed) = &self.editor else {
            return;
        };
        let (h, (r, c)) = (ed.hwnd, ed.cell);
        // 別のシートを表示している間は外へ（入力は続けられる。内容は数式バーに映す）
        let rc = if self.home.is_some() {
            None
        } else {
            self.cell_rect_px(r, c)
        };
        unsafe {
            let _ = match rc {
                Some(rc) => MoveWindow(
                    h,
                    rc.left,
                    rc.top,
                    (rc.right - rc.left).max(60),
                    rc.bottom - rc.top,
                    true,
                ),
                None => MoveWindow(h, -30000, -30000, 60, 20, false),
            };
        }
        self.refresh_assist();
    }

    /// F4: 参照の `$` を切り替える。
    fn toggle_abs(&mut self, h: HWND) -> bool {
        let text = window_text(h);
        if !text.starts_with('=') {
            return false;
        }
        let (s, _) = edit_sel(h);
        if let Some((new, _, end)) = yy_formula::toggle_abs(&text, to_chars(&text, s)) {
            self.point = None;
            let pos = to_u16(&new, end);
            unsafe {
                let _ = SetWindowTextW(h, &HSTRING::from(new.as_str()));
                SendMessageW(h, EM_SETSEL, Some(WPARAM(pos)), Some(LPARAM(pos as isize)));
            }
            self.refresh_assist();
        }
        true
    }

    /// 式の入力の EDIT でのキー（メッセージループから）。扱ったら `true`。
    pub(super) fn entry_key(&mut self, h: HWND, vk: VIRTUAL_KEY, shift: bool, ctrl: bool) -> bool {
        // 候補の一覧
        if let Mode::List { items, sel, start } = &self.assist.mode {
            let (n, sel, start) = (items.len(), *sel, *start);
            match vk {
                VK_DOWN | VK_UP => {
                    let next = if vk == VK_DOWN {
                        (sel + 1).min(n - 1)
                    } else {
                        sel.saturating_sub(1)
                    };
                    if let Mode::List { sel, .. } = &mut self.assist.mode {
                        *sel = next;
                    }
                    self.show_assist();
                    return true;
                }
                VK_TAB | VK_RETURN if !ctrl => {
                    self.accept(sel);
                    return true;
                }
                VK_ESCAPE => {
                    self.assist.dismissed = Some(start);
                    self.assist.hide();
                    self.refresh_assist();
                    return true;
                }
                _ => {}
            }
        }
        if vk == VK_F4 && !ctrl && !shift {
            return self.toggle_abs(h);
        }
        let arrow = match vk {
            VK_UP => Some((-1, 0)),
            VK_DOWN => Some((1, 0)),
            VK_LEFT => Some((0, -1)),
            VK_RIGHT => Some((0, 1)),
            _ => None,
        };
        if let Some((dr, dc)) = arrow
            && !ctrl
            && self.point_key(h, dr, dc, shift)
        {
            return true;
        }
        // そのほかのキー（修飾キーを除く）で参照を確定する
        if !matches!(vk, VK_SHIFT | VK_CONTROL | VK_MENU | VK_LSHIFT | VK_RSHIFT)
            && self.point.as_ref().is_some_and(|p| p.hwnd == h)
        {
            self.point = None;
            self.invalidate();
        }
        false
    }

    /// 候補を入れる（`NAME(`）。
    fn accept(&mut self, index: usize) {
        let Mode::List { items, start, .. } = &self.assist.mode else {
            return;
        };
        let Some(f) = items.get(index) else {
            return;
        };
        let (name, start) = (f.name, *start);
        let h = self.assist.target;
        let text = window_text(h);
        let (s, e) = edit_sel(h);
        edit_replace(h, to_u16(&text, start), e.max(s), &format!("{name}("));
        self.point = None;
        self.refresh_assist();
    }

    /// 小窓のクリック（候補を選んで入れる）。
    fn assist_click(&mut self, y: i32) {
        if !matches!(self.assist.mode, Mode::List { .. }) {
            return;
        }
        let line = unsafe {
            let hdc = GetDC(Some(self.assist.hwnd));
            let old = SelectObject(hdc, HGDIOBJ(self.assist.font.0));
            let mut tm = TEXTMETRICW::default();
            let _ = GetTextMetricsW(hdc, &mut tm);
            SelectObject(hdc, old);
            ReleaseDC(Some(self.assist.hwnd), hdc);
            tm.tmHeight + 2
        };
        let i = ((y - PAD / 2) / line.max(1)).max(0) as usize;
        self.accept(i);
    }

    /// 式の入力が終わった（確定・取り消し・ほかへ移った）。
    pub(super) fn entry_reset(&mut self) {
        self.go_home();
        self.point = None;
        self.assist.dismissed = None;
        self.assist.hide();
        if !self.marks.is_empty() {
            self.marks.clear();
            self.invalidate();
        }
    }

    /// 入力の様子に合わせて、参照の枠・候補・引数の書き方を出し直す。
    pub(super) fn refresh_assist(&mut self) {
        let active = unsafe { GetActiveWindow() } == self.frame;
        let target = self.entry_target().filter(|_| active);
        let text = target.map(window_text).unwrap_or_default();
        // セルの編集を数式バーに映す
        if self.editor.is_some() && window_text(self.formula) != text {
            unsafe {
                let _ = SetWindowTextW(self.formula, &HSTRING::from(text.as_str()));
            }
        }
        // 式の参照の枠（絞り込み・並べ替えの表示中は行が合わないので出さない）
        let mut marks = Vec::new();
        if text.starts_with('=') && self.sheet().view.rows.is_none() {
            // 表示しているシートの参照（元のシートなら名前のない参照も）
            let shown = self.sheet().name.clone();
            let on_home = self.home.is_none();
            for (i, r) in yy_formula::refs_in(&text)
                .into_iter()
                .filter(|r| match &r.sheet {
                    None => on_home,
                    Some(n) => yy_formula::eq_text(n, &shown),
                })
                .enumerate()
            {
                let a = r.area;
                let range = match a.kind {
                    AreaKind::Cols => (0, a.c0, u64::MAX, a.c1),
                    AreaKind::Rows => (a.r0, 0, a.r1, yy_formula::MAX_COL),
                    _ => (a.r0, a.c0, a.r1, a.c1),
                };
                marks.push((range, MARK_COLORS[i % MARK_COLORS.len()]));
            }
        }
        if marks != self.marks {
            self.marks = marks;
            self.invalidate();
        }
        let Some(h) = target.filter(|_| text.starts_with('=')) else {
            self.assist.hide();
            return;
        };
        let (s, _) = edit_sel(h);
        let ty = yy_formula::typing(&text, to_chars(&text, s));
        let word_start = ty.word.as_ref().map(|w| w.0);
        if word_start != self.assist.dismissed {
            self.assist.dismissed = None;
        }
        let list = ty
            .word
            .as_ref()
            .filter(|w| Some(w.0) != self.assist.dismissed)
            .map(|(start, w)| (*start, FuncInfo::complete(w)))
            .filter(|(_, items)| !items.is_empty());
        let mode = if let Some((start, items)) = list {
            let sel = match &self.assist.mode {
                Mode::List {
                    items: old, sel, ..
                } if *old == items => *sel,
                _ => 0,
            };
            Mode::List { items, sel, start }
        } else if let Some((name, i)) = &ty.call
            && let Some(info) = FuncInfo::find(name)
        {
            Mode::Sig {
                info,
                param: info.param_index(*i),
            }
        } else {
            Mode::Hidden
        };
        if matches!(mode, Mode::Hidden) {
            self.assist.hide();
            return;
        }
        self.assist.mode = mode;
        self.assist.target = h;
        self.show_assist();
    }

    /// 小窓を EDIT の下に出す（大きさを測り直して描き直す）。
    fn show_assist(&self) {
        let a = &self.assist;
        unsafe {
            let hdc = GetDC(Some(a.hwnd));
            let (w, h) = a.render(hdc, false);
            ReleaseDC(Some(a.hwnd), hdc);
            let mut rc = RECT::default();
            let _ = GetWindowRect(a.target, &mut rc);
            // 枠の分
            let (w, h) = (w + 2, h + 2);
            // 画面の下に収まらなければ上に出す
            let screen_h =
                GetSystemMetrics(SM_CYVIRTUALSCREEN) + GetSystemMetrics(SM_YVIRTUALSCREEN);
            let y = if rc.bottom + 2 + h > screen_h {
                rc.top - 2 - h
            } else {
                rc.bottom + 2
            };
            let _ = SetWindowPos(
                a.hwnd,
                None,
                rc.left,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
            );
            let _ = InvalidateRect(Some(a.hwnd), None, false);
        }
    }
}

/// メッセージを送る前（EDIT のクリックで参照を確定する）。
pub(super) fn before_dispatch(msg: &MSG) {
    if msg.message == WM_LBUTTONDOWN {
        with(|a| {
            if a.point.as_ref().is_some_and(|p| p.hwnd == msg.hwnd) {
                a.point = None;
                a.invalidate();
            }
        });
    }
}

/// メッセージを送った後（式の入力の EDIT への入力・カーソルの移動なら出し直す）。
pub(super) fn after_dispatch(msg: &MSG) {
    if !matches!(
        msg.message,
        WM_KEYDOWN | WM_KEYUP | WM_CHAR | WM_LBUTTONUP | WM_LBUTTONDOWN
    ) {
        return;
    }
    with(|a| {
        let is_entry =
            a.editor.as_ref().is_some_and(|e| e.hwnd == msg.hwnd) || msg.hwnd == a.formula;
        if is_entry {
            a.refresh_assist();
        }
    });
}

pub(super) extern "system" fn assist_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe {
                let hdc = BeginPaint(hwnd, &mut ps);
                with(|a| a.assist.render(hdc, true));
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            with(|a| a.assist_click(y));
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

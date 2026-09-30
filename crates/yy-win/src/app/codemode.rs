//! コード値の表示・編集モード。
//!
//! 16 進数（バイナリ）編集と同じ形で、文書の文字を 1 行 8 文字ずつ、左に各文字の Unicode の
//! 符号位置（コード値）、右に文字を並べて表示・編集する。コード値の欄では 16 進数を入力して
//! 文字を書き換え・挿入し、文字の欄では文字を入力する。選択範囲は文書の選択（バイト位置）を
//! そのまま使うため、検索・置換・Undo も共通になる。

use yy_core::codeview::{self, CodeLayout, CodeRow};
use yy_core::hex::Pane;
use yy_core::{EditKind, Selection, SelectionSet};

use super::*;
use crate::render::CodeFrame;

pub(crate) const ID_CODE_MODE: u16 = 210;

/// コード値表示の状態。
#[derive(Clone, Copy, Debug)]
pub(crate) struct CodeState {
    /// 先頭の表示の行の位置
    pub top: u64,
    /// `Pane::Hex` はコード値の欄、`Pane::Ascii` は文字の欄
    pub pane: Pane,
    /// 上書き（既定）か挿入か
    pub overwrite: bool,
    /// コード値の欄で入力中の値（値, 入力した桁数）
    pub entry: Option<(u32, u8)>,
    /// マウスでドラッグ中の選択の起点
    pub drag_anchor: Option<u64>,
}

impl App {
    fn code_layout(&self) -> CodeLayout {
        let snap = self.doc.snapshot();
        let lines = snap
            .line_count()
            .unwrap_or_else(|| snap.estimated_line_count());
        CodeLayout::for_lines(lines + 1)
    }

    /// コード値表示に切り替える・戻す。
    pub(crate) fn set_code(&mut self, on: bool) {
        ime::cancel(self.view);
        self.composition = None;
        self.rect = None;
        self.drag = None;
        if on {
            self.hex = None;
            // カーソルは 1 つにして、文字の境界に合わせる
            let snap = self.doc.snapshot();
            let p = self.doc.selections().primary();
            let (a, h) = (
                codeview::align(snap, p.anchor),
                codeview::align(snap, p.head),
            );
            self.doc
                .set_selections(SelectionSet::single(Selection::new(a, h)));
        }
        let head = self.doc.selections().primary().head;
        self.code = on.then(|| CodeState {
            top: codeview::row_start_at(self.doc.snapshot(), head),
            pane: Pane::Hex,
            overwrite: true,
            entry: None,
            drag_anchor: None,
        });
        self.scroll_x = 0.0;
        self.renderer.clear_cache();
        self.row_cache.borrow_mut().rows.clear();
        self.update_hex_menu();
        self.after_move();
    }

    /// 表示する行（画面の行数 + 1）。
    fn code_rows(&self) -> Vec<CodeRow> {
        let Some(c) = self.code else {
            return Vec::new();
        };
        codeview::rows_from(self.doc.snapshot(), c.top, self.page_rows() + 1)
    }

    /// 先頭の行が文書の外（編集で短くなった）なら戻す。
    fn code_fix_top(&mut self) {
        let snap = self.doc.snapshot().clone();
        if let Some(c) = &mut self.code {
            let top = codeview::row_start_at(&snap, c.top);
            c.top = top;
        }
    }

    /// カーソルが見えるように表示位置を変える。
    pub(crate) fn code_ensure_visible(&mut self) {
        self.code_fix_top();
        let Some(c) = self.code else { return };
        let snap = self.doc.snapshot().clone();
        let head = self.doc.selections().primary().head;
        let page = self.page_rows();
        let len = snap.len();
        let mut top = c.top;
        let ahead = codeview::rows_from(&snap, top, page * 2);
        match codeview::locate(&ahead, head, len) {
            Some((ri, _)) if head >= top && ri < page => {}
            // 少し下なら、その行が最後に見えるまで進める
            Some((ri, _)) if head >= top => top = ahead[ri + 1 - page].start,
            _ => {
                let row = codeview::row_start_at(&snap, head);
                top = row;
                if head < c.top && c.top - head < 4096 {
                    // 少し上なら、その行を先頭に
                } else {
                    // 離れた位置なら、画面の中ほどに
                    for _ in 0..page / 2 {
                        match codeview::prev_row(&snap, top) {
                            Some(p) => top = p,
                            None => break,
                        }
                    }
                }
            }
        }
        // 横方向（カーソルの桁が見えるように）
        let l = self.code_layout();
        let rows = codeview::rows_from(&snap, top, page + 1);
        let i = codeview::locate(&rows, head, len).map_or(0, |(_, i)| i);
        let col = match c.pane {
            Pane::Hex => l.code_col(i),
            Pane::Ascii => l.char_col(i),
        };
        let cw = self.renderer.metrics().char_width;
        let x = col as f32 * cw;
        let area = self.renderer.px_to_dip(self.view_px.0 as f32);
        if x < self.scroll_x {
            self.scroll_x = (x - cw * 4.0).max(0.0);
        } else if x + cw * 7.0 > self.scroll_x + area {
            self.scroll_x = x + cw * 8.0 - area;
        }
        if let Some(c) = &mut self.code {
            c.top = top;
        }
    }

    /// 表示位置を `rows` 行動かす。
    pub(crate) fn code_scroll_rows(&mut self, rows: i64) {
        let Some(c) = self.code else { return };
        let snap = self.doc.snapshot().clone();
        let page = self.page_rows();
        let mut top = c.top;
        for _ in 0..rows.unsigned_abs() {
            let next = if rows < 0 {
                codeview::prev_row(&snap, top)
            } else if codeview::rows_from(&snap, top, page).len() < page {
                None // 最後の行まで見えている
            } else {
                codeview::next_row(&snap, top)
            };
            match next {
                Some(t) => top = t,
                None => break,
            }
        }
        if top != c.top {
            if let Some(c) = &mut self.code {
                c.top = top;
            }
            self.update_scrollbars();
            self.invalidate();
        }
    }

    /// 位置 `offset` を表示する（表示範囲の外なら画面の中ほどに）。
    pub(crate) fn code_scroll_to(&mut self, offset: u64) {
        let Some(c) = self.code else { return };
        let snap = self.doc.snapshot().clone();
        let page = self.page_rows();
        let rows = codeview::rows_from(&snap, c.top, page);
        let visible = offset >= c.top && rows.last().is_some_and(|r| offset <= r.end());
        if !visible {
            let mut top = codeview::row_start_at(&snap, offset);
            for _ in 0..page / 2 {
                match codeview::prev_row(&snap, top) {
                    Some(p) => top = p,
                    None => break,
                }
            }
            if let Some(c) = &mut self.code {
                c.top = top;
            }
        }
        self.update_scrollbars();
        self.update_status();
        self.invalidate();
    }

    /// 縦のスクロールバー（位置の割合で表す）。
    pub(crate) fn code_update_scrollbars(&mut self) {
        let Some(c) = self.code else { return };
        let len = self.doc.snapshot().len().max(1);
        let rows = self.code_rows();
        let shown = rows.last().map_or(0, |r| r.end()) - c.top;
        let frac = |v: u64| (v as f64 / len as f64 * SCROLL_RANGE as f64) as i32;
        let si = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL | SIF_DISABLENOSCROLL,
            nMin: 0,
            nMax: SCROLL_RANGE - 1,
            nPage: frac(shown).clamp(1, SCROLL_RANGE) as u32,
            nPos: frac(c.top),
            nTrackPos: 0,
        };
        let cw = self.renderer.metrics().char_width;
        let width = (self.code_layout().width() + 4) as f32 * cw;
        let area = self.renderer.px_to_dip(self.view_px.0 as f32);
        let hsi = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL | SIF_DISABLENOSCROLL,
            nMin: 0,
            nMax: width as i32,
            nPage: area as u32,
            nPos: self.scroll_x as i32,
            nTrackPos: 0,
        };
        self.renderer.max_text_width = width;
        unsafe {
            SetScrollInfo(self.view, SB_VERT, &si, true);
            SetScrollInfo(self.view, SB_HORZ, &hsi, true);
        }
    }

    /// スクロールバーのつまみの位置 `pos`（0〜SCROLL_RANGE）に表示位置を合わせる。
    pub(crate) fn code_scroll_to_fraction(&mut self, pos: i32) {
        let snap = self.doc.snapshot().clone();
        let at = (pos as f64 / SCROLL_RANGE as f64 * snap.len() as f64) as u64;
        if let Some(c) = &mut self.code {
            c.top = codeview::row_start_at(&snap, at);
        }
        self.update_status();
        self.invalidate();
    }

    pub(crate) fn code_paint(&mut self) {
        let Some(c) = self.code else { return };
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(self.view, &mut ps);
            let snap = self.doc.snapshot();
            let rows = self.code_rows();
            let end = rows.last().map_or(c.top, |r| r.end());
            let sel = self.doc.selections().primary();
            let matches = match (&self.searcher, self.findbar.visible) {
                (Some(s), true) => {
                    s.matches_in(snap, c.top.saturating_sub(4096)..end, HIGHLIGHT_LIMIT)
                }
                _ => Vec::new(),
            };
            let frame = CodeFrame {
                layout: self.code_layout(),
                rows: &rows,
                len: snap.len(),
                caret: sel.head,
                entry: c.entry.map(|(v, n)| format!("{v:0n$X}", n = n as usize)),
                pane: c.pane,
                caret_visible: self.focused && self.caret_visible,
                overwrite: c.overwrite,
                selection: sel.range(),
                matches: &matches,
                scroll_x: self.scroll_x,
                ambiguous_wide: self.config.editor.ambiguous_wide,
            };
            let r = self
                .renderer
                .draw_code(self.view, self.view_px.0, self.view_px.1, &frame);
            let _ = EndPaint(self.view, &ps);
            match r {
                Ok(true) => {}
                Ok(false) => self.invalidate(),
                Err(e) => eprintln!("draw failed: {}", crate::util::describe_error(&e)),
            }
        }
    }

    /// カーソルを `pos` に動かす（`extend` なら選択を広げる）。入力中のコード値は確定する。
    fn code_move(&mut self, pos: u64, extend: bool) {
        self.code_commit();
        let pos = pos.min(self.doc.snapshot().len());
        let anchor = if extend {
            self.doc.selections().primary().anchor
        } else {
            pos
        };
        self.doc
            .set_selections(SelectionSet::single(Selection::new(anchor, pos)));
        self.after_move();
    }

    /// 上下の行の同じ番号の文字の位置。
    fn code_vertical(&self, head: u64, rows: i64) -> u64 {
        let snap = self.doc.snapshot();
        let len = snap.len();
        let start = codeview::row_start_at(snap, head);
        let cur = codeview::rows_from(snap, start, 1);
        let i = codeview::locate(&cur, head, len).map_or(0, |(_, i)| i);
        let mut r = start;
        for _ in 0..rows.unsigned_abs() {
            let n = if rows > 0 {
                codeview::next_row(snap, r)
            } else {
                codeview::prev_row(snap, r)
            };
            match n {
                Some(n) => r = n,
                None if rows > 0 => return len.max(head),
                None => return 0,
            }
        }
        let row = codeview::rows_from(snap, r, 1);
        let Some(row) = row.first() else { return head };
        match row.cells.get(i) {
            Some(c) => c.start,
            // 行の文字数が足りなければ行の最後の文字（文書の終わりの行なら終わりの位置）
            None if row.end() == len => len,
            None => row.cells.last().map_or(row.start, |c| c.start),
        }
    }

    /// コード値表示のキー操作。処理したら `true`。
    pub(crate) fn code_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        let Some(c) = self.code else { return false };
        let ctrl = key_down(VK_CONTROL);
        let shift = key_down(VK_SHIFT);
        let snap = self.doc.snapshot().clone();
        let len = snap.len();
        let head = self.doc.selections().primary().head;
        let page = self.page_rows().saturating_sub(1).max(1) as i64;
        let target = match vk {
            VK_ESCAPE if c.entry.is_some() => {
                if let Some(c) = &mut self.code {
                    c.entry = None;
                }
                self.invalidate();
                return true;
            }
            VK_LEFT => codeview::prev_char(&snap, head),
            VK_RIGHT => codeview::next_char(&snap, head),
            VK_UP => self.code_vertical(head, -1),
            VK_DOWN => self.code_vertical(head, 1),
            VK_PRIOR => self.code_vertical(head, -page),
            VK_NEXT => self.code_vertical(head, page),
            VK_HOME if ctrl => 0,
            VK_END if ctrl => len,
            VK_HOME => codeview::row_start_at(&snap, head),
            VK_END => {
                let start = codeview::row_start_at(&snap, head);
                let rows = codeview::rows_from(&snap, start, 1);
                match rows.first() {
                    Some(r) if r.end() == len => len,
                    Some(r) => r.cells.last().map_or(r.start, |c| c.start),
                    None => head,
                }
            }
            VK_TAB => {
                self.code_commit();
                if let Some(c) = &mut self.code {
                    c.pane = match c.pane {
                        Pane::Hex => Pane::Ascii,
                        Pane::Ascii => Pane::Hex,
                    };
                }
                self.after_move();
                return true;
            }
            VK_INSERT if !shift && !ctrl => {
                if let Some(c) = &mut self.code {
                    c.overwrite = !c.overwrite;
                }
                self.update_status();
                self.invalidate();
                return true;
            }
            VK_BACK if c.entry.is_some() => {
                // 入力中のコード値の最後の桁を消す
                if let Some(c) = &mut self.code {
                    c.entry = c
                        .entry
                        .and_then(|(v, n)| (n > 1).then_some((v >> 4, n - 1)));
                }
                self.invalidate();
                return true;
            }
            VK_BACK => {
                self.code_delete(true);
                return true;
            }
            VK_DELETE if !shift => {
                self.code_commit();
                self.code_delete(false);
                return true;
            }
            _ => return false,
        };
        self.code_move(target, shift);
        true
    }

    fn code_editable(&mut self) -> bool {
        if self.doc.is_read_only() {
            self.status_msg = "読み取り専用で開いたファイルは編集できません".into();
            self.update_status();
            return false;
        }
        true
    }

    /// 選択範囲（なければカーソルの前後の 1 文字）を削除する。
    pub(crate) fn code_delete(&mut self, backward: bool) {
        if !self.code_editable() {
            return;
        }
        let snap = self.doc.snapshot().clone();
        let sel = self.doc.selections().primary().range();
        let r = if !sel.is_empty() {
            sel
        } else if backward {
            codeview::prev_char(&snap, sel.start)..sel.start
        } else {
            sel.start..codeview::next_char(&snap, sel.start)
        };
        if r.is_empty() {
            return;
        }
        let at = r.start;
        if self.doc.apply_changes(
            vec![yy_core::edit::Change::delete(r)],
            EditKind::Delete,
            |_| SelectionSet::single(Selection::caret(at)),
        ) {
            self.after_edit();
        }
    }

    /// 文字列 `text` を入力する（選択範囲を置き換える。上書きならカーソルの後ろの同じ文字数を
    /// 置き換える）。
    fn code_type(&mut self, text: &str, kind: EditKind) {
        let Some(c) = self.code else { return };
        if text.is_empty() || !self.code_editable() {
            return;
        }
        let snap = self.doc.snapshot().clone();
        let sel = self.doc.selections().primary().range();
        let mut end = sel.end;
        if sel.is_empty() && c.overwrite {
            for _ in text.chars() {
                end = codeview::next_char(&snap, end);
            }
        }
        let caret = sel.start + text.len() as u64;
        if self.doc.apply_changes(
            vec![yy_core::edit::Change::replace_bytes(
                sel.start..end,
                text.as_bytes().to_vec(),
            )],
            kind,
            |_| SelectionSet::single(Selection::caret(caret)),
        ) {
            self.after_edit();
        }
    }

    /// 入力中のコード値を確定して文字を入力する。
    fn code_commit(&mut self) {
        let Some(c) = self.code else { return };
        let Some((v, _)) = c.entry else { return };
        if let Some(c) = &mut self.code {
            c.entry = None;
        }
        match char::from_u32(v) {
            Some(ch) => self.code_type(ch.encode_utf8(&mut [0u8; 4]), EditKind::Typing),
            None => {
                self.status_msg = format!("U+{v:04X} は文字として使えません");
                self.update_status();
                self.invalidate();
            }
        }
    }

    /// 文字の入力（コード値の欄では 0〜9・A〜F と確定の Space・Enter、文字の欄では文字）。
    pub(crate) fn code_char(&mut self, code: u16) {
        let Some(c) = self.code else { return };
        if key_down(VK_CONTROL) && !key_down(VK_MENU) {
            return;
        }
        let text = match code {
            0xD800..=0xDBFF => {
                self.high_surrogate = Some(code);
                return;
            }
            0xDC00..=0xDFFF => match self.high_surrogate.take() {
                Some(hi) => String::from_utf16_lossy(&[hi, code]),
                None => return,
            },
            0x0D => "\r".to_owned(),
            0x09 => return, // Tab は欄の切り替え
            c if c < 0x20 || c == 0x7F => return,
            c => String::from_utf16_lossy(&[c]),
        };
        match c.pane {
            Pane::Hex => {
                if text == "\r" || text == " " {
                    self.code_commit();
                    return;
                }
                let Some(d) = text.chars().next().and_then(|ch| ch.to_digit(16)) else {
                    self.status_msg = "16 進数（0〜9、A〜F）を入力してください".into();
                    self.update_status();
                    return;
                };
                if !self.code_editable() {
                    return;
                }
                match codeview::push_digit(c.entry, d) {
                    Some(e) => {
                        if let Some(c) = &mut self.code {
                            c.entry = Some(e);
                        }
                        if e.1 == codeview::MAX_DIGITS {
                            self.code_commit();
                        } else {
                            self.status_msg.clear();
                            self.update_status();
                            self.invalidate();
                        }
                    }
                    None => {
                        self.status_msg = "コード値は 10FFFF までです".into();
                        self.update_status();
                    }
                }
            }
            Pane::Ascii => {
                let text = if text == "\r" {
                    String::from_utf8_lossy(self.doc.eol().as_bytes()).into_owned()
                } else {
                    text
                };
                self.code_type(&text, EditKind::Typing);
            }
        }
    }

    /// IME で確定した文字列（文字の欄に入力する）。
    pub(crate) fn code_ime_result(&mut self, text: &str) {
        if let Some(c) = &mut self.code {
            c.pane = Pane::Ascii;
            c.entry = None;
        }
        self.code_type(text, EditKind::Typing);
    }

    /// 画面上の位置の文字（位置, 欄）。
    fn code_hit(&self, x: i32, y: i32, extend_end: bool) -> Option<(u64, Pane)> {
        let c = self.code?;
        let xd = self.renderer.px_to_dip(x as f32);
        let yd = self.renderer.px_to_dip(y.max(0) as f32);
        let lh = self.renderer.metrics().line_height.max(1.0);
        let ri = (yd / lh) as usize;
        let snap = self.doc.snapshot();
        let rows = codeview::rows_from(snap, c.top, ri + 1);
        let row = rows.get(ri).or(rows.last())?;
        let col = self.renderer.hex_col_at(xd, self.scroll_x);
        let (pane, i) = self.code_layout().hit(col).unwrap_or((Pane::Hex, 0));
        let pos = match row.cells.get(i) {
            Some(cell) if extend_end => cell.end(),
            Some(cell) => cell.start,
            None if row.end() == snap.len() => snap.len(),
            None => row
                .cells
                .last()
                .map_or(row.start, |c| if extend_end { c.end() } else { c.start }),
        };
        Some((pos, pane))
    }

    /// クリックした位置にカーソルを置く（Shift なら選択を広げる）。
    pub(crate) fn code_mouse_down(&mut self, x: i32, y: i32) {
        self.code_commit();
        let Some((pos, pane)) = self.code_hit(x, y, false) else {
            return;
        };
        let shift = key_down(VK_SHIFT);
        let anchor = if shift {
            self.doc.selections().primary().anchor
        } else {
            pos
        };
        self.doc
            .set_selections(SelectionSet::single(Selection::new(anchor, pos)));
        if let Some(c) = &mut self.code {
            c.pane = pane;
            c.drag_anchor = Some(anchor);
        }
        self.after_move();
    }

    pub(crate) fn code_mouse_move(&mut self, x: i32, y: i32) {
        let Some(anchor) = self.code.and_then(|c| c.drag_anchor) else {
            return;
        };
        let Some((start, _)) = self.code_hit(x, y, false) else {
            return;
        };
        // ドラッグで選ぶときはカーソルのある文字も含める
        let head = if start >= anchor {
            self.code_hit(x, y, true).map_or(start, |h| h.0)
        } else {
            start
        };
        self.doc
            .set_selections(SelectionSet::single(Selection::new(anchor, head)));
        self.after_move();
    }

    /// 選択範囲のコピー（コード値の欄ではコード値の並び、文字の欄では文字列）。
    pub(crate) fn code_copy_text(&self) -> Option<String> {
        let c = self.code?;
        let r = self.doc.selections().primary().range();
        if r.is_empty() || r.end - r.start > MAX_CLIPBOARD_BYTES / 8 {
            return None;
        }
        match c.pane {
            Pane::Hex => {
                let snap = self.doc.snapshot();
                let mut cells = Vec::new();
                let mut top = codeview::row_start_at(snap, r.start);
                loop {
                    let rows = codeview::rows_from(snap, top, 256);
                    for row in &rows {
                        cells.extend(
                            row.cells
                                .iter()
                                .filter(|c| c.start >= r.start && c.end() <= r.end),
                        );
                    }
                    match rows.last() {
                        Some(last) if last.end() < r.end && last.end() > top => top = last.end(),
                        _ => break,
                    }
                }
                Some(codeview::codes_text(&cells))
            }
            Pane::Ascii => self.doc.selected_text(MAX_CLIPBOARD_BYTES).ok().flatten(),
        }
    }

    /// 貼り付け（コード値の欄ではコード値の並びとして読む。読めなければ文字列）。
    pub(crate) fn code_paste(&mut self, text: &str) {
        let Some(c) = self.code else { return };
        self.code_commit();
        let text = match c.pane {
            Pane::Hex => codeview::parse_codes(text).unwrap_or_else(|| text.to_owned()),
            Pane::Ascii => text.to_owned(),
        };
        self.code_type(&text, EditKind::Paste);
    }

    /// 選択範囲を削除する（切り取り）。
    pub(crate) fn code_cut(&mut self) {
        if !self.doc.selections().primary().is_empty() {
            self.code_delete(false);
        }
    }

    /// ステータスバーの位置の表示に加える、カーソルの文字のコード値。
    pub(crate) fn code_status(&self) -> String {
        let snap = self.doc.snapshot();
        let head = self.doc.selections().primary().head;
        let rows = codeview::rows_from(snap, head, 1);
        match rows.first().and_then(|r| r.cells.first()) {
            Some(cell) => match cell.kind {
                codeview::CellKind::Char(ch) => format!("  値 U+{:04X}", ch as u32),
                codeview::CellKind::Byte(b) => format!("  値 \\x{b:02X}"),
            },
            None => String::new(),
        }
    }

    /// IME の変換ウィンドウをカーソルの文字の欄の位置に合わせる。
    pub(crate) fn code_ime_position(&mut self) {
        let rows = self.code_rows();
        let head = self.doc.selections().primary().head;
        let Some((ri, i)) = codeview::locate(&rows, head, self.doc.snapshot().len()) else {
            return;
        };
        let cw = self.renderer.metrics().char_width;
        let lh = self.renderer.metrics().line_height;
        let col = self.code_layout().char_col(i);
        let x = crate::render::TEXT_PAD + col as f32 * cw - self.scroll_x;
        let y = ri as f32 * lh;
        let scale = |v: f32| (v * self.renderer.px_to_dip(1.0).recip()).round() as i32;
        ime::set_position(self.view, scale(x), scale(y), scale(lh));
    }
}

/// コード値表示に切り替える・戻す。
pub(crate) fn cmd_toggle_code(_hwnd: HWND) {
    with_app(|a| a.set_code(a.code.is_none()));
}

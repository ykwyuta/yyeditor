//! 16 進数（バイナリ）編集モード。
//!
//! 文書のバイト列を 1 行 16 バイトの 16 進ダンプで表示・編集する。表示位置はバイト位置から
//! 直接決めるので、行の索引がなくても数 GB のファイルの任意の位置をすぐに表示できる。
//! 選択範囲は文書の選択（バイト位置）をそのまま使うため、検索・置換・Undo も共通になる。

use yy_core::hex::{self, HexLayout, Pane};
use yy_core::{EditKind, Selection, SelectionSet};

use super::*;
use crate::render::HexFrame;

pub(crate) const ID_HEX_MODE: u16 = 207;
pub(crate) const ID_OPEN_BINARY: u16 = 111;

/// 16 進数表示の状態。
#[derive(Clone, Copy, Debug)]
pub(crate) struct HexState {
    /// 先頭の行のオフセット（16 の倍数）
    pub top: u64,
    pub pane: Pane,
    /// カーソルのある 16 進の桁（0 = 上位, 1 = 下位）
    pub nibble: u8,
    /// 上書き（既定）か挿入か
    pub overwrite: bool,
    /// マウスでドラッグ中の選択の起点
    pub drag_anchor: Option<u64>,
}

impl HexState {
    fn new(caret: u64) -> HexState {
        HexState {
            top: caret / 16 * 16,
            pane: Pane::Hex,
            nibble: 0,
            overwrite: true,
            drag_anchor: None,
        }
    }
}

impl App {
    pub(crate) fn hex_layout(&self) -> HexLayout {
        HexLayout::for_len(self.doc.snapshot().len())
    }

    /// 16 進数表示に切り替える・戻す（ファイルを開き直す必要があれば `on` の前に済ませること）。
    pub(crate) fn set_hex(&mut self, on: bool) {
        ime::cancel(self.view);
        self.composition = None;
        self.rect = None;
        self.drag = None;
        let head = self.doc.selections().primary().head;
        self.hex = on.then(|| HexState::new(head));
        self.scroll_x = 0.0;
        self.renderer.clear_cache();
        self.row_cache.borrow_mut().rows.clear();
        unsafe {
            CheckMenuItem(
                self.menu_view,
                ID_HEX_MODE as u32,
                (MF_BYCOMMAND | if on { MF_CHECKED } else { MF_UNCHECKED }).0,
            );
        }
        self.after_move();
    }

    /// 表示中の最後の行の次のオフセットまでの行数。
    fn hex_rows(&self) -> u64 {
        self.page_rows() as u64
    }

    /// 最後の行（文書の終わりの位置を含む行）の先頭。
    fn hex_last_row(&self) -> u64 {
        self.doc.snapshot().len() / 16 * 16
    }

    /// カーソルが見えるように表示位置を変える。
    pub(crate) fn hex_ensure_visible(&mut self) {
        let head = self.doc.selections().primary().head;
        let page = self.hex_rows();
        let last = self.hex_last_row();
        let l = self.hex_layout();
        let Some(h) = &mut self.hex else { return };
        let row = head / 16 * 16;
        if row < h.top {
            h.top = row;
        } else if row >= h.top + page * 16 {
            h.top = row + 16 - page * 16;
        }
        h.top = h
            .top
            .min(last.saturating_sub((page.saturating_sub(1)) * 16));
        // 横方向（カーソルの桁が見えるように）
        let cw = self.renderer.metrics().char_width;
        let i = (head % 16) as usize;
        let col = match h.pane {
            Pane::Hex => l.hex_col(i),
            Pane::Ascii => l.ascii_col(i),
        };
        let x = col as f32 * cw;
        let area = self.text_area_width_hex();
        if x < self.scroll_x {
            self.scroll_x = (x - cw * 4.0).max(0.0);
        } else if x + cw * 3.0 > self.scroll_x + area {
            self.scroll_x = x + cw * 4.0 - area;
        }
    }

    fn text_area_width_hex(&self) -> f32 {
        self.renderer.px_to_dip(self.view_px.0 as f32)
    }

    /// 表示位置を `rows` 行動かす。
    pub(crate) fn hex_scroll_rows(&mut self, rows: i64) {
        let last = self.hex_last_row();
        let page = self.hex_rows();
        let Some(h) = &mut self.hex else { return };
        let max = last.saturating_sub(page.saturating_sub(1) * 16);
        let top = if rows < 0 {
            h.top.saturating_sub(rows.unsigned_abs().saturating_mul(16))
        } else {
            h.top
                .saturating_add((rows as u64).saturating_mul(16))
                .min(max)
        };
        if top != h.top {
            h.top = top;
            self.update_scrollbars();
            self.invalidate();
        }
    }

    /// 表示位置をオフセット `offset` の行にする（行が表示範囲の外なら）。
    pub(crate) fn hex_scroll_to(&mut self, offset: u64) {
        let page = self.hex_rows();
        let last = self.hex_last_row();
        if let Some(h) = &mut self.hex {
            let row = offset.min(last) / 16 * 16;
            if row < h.top || row >= h.top + page * 16 {
                h.top = row.saturating_sub(page / 2 * 16);
            }
            h.top = h.top.min(last.saturating_sub(page.saturating_sub(1) * 16));
        }
        self.update_scrollbars();
        self.update_status();
        self.invalidate();
    }

    /// 縦のスクロールバー（全体の行数が大きいので位置の割合で表す）。
    pub(crate) fn hex_update_scrollbars(&mut self) {
        let Some(h) = self.hex else { return };
        let rows = self.hex_last_row() / 16 + 1;
        let page = self.hex_rows();
        let frac = |v: u64| (v as f64 / rows.max(1) as f64 * SCROLL_RANGE as f64) as i32;
        let si = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL | SIF_DISABLENOSCROLL,
            nMin: 0,
            nMax: SCROLL_RANGE - 1,
            nPage: frac(page).clamp(1, SCROLL_RANGE) as u32,
            nPos: frac(h.top / 16),
            nTrackPos: 0,
        };
        let cw = self.renderer.metrics().char_width;
        let width = (self.hex_layout().width() + 4) as f32 * cw;
        let hsi = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL | SIF_DISABLENOSCROLL,
            nMin: 0,
            nMax: width as i32,
            nPage: self.text_area_width_hex() as u32,
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
    pub(crate) fn hex_scroll_to_fraction(&mut self, pos: i32) {
        let rows = self.hex_last_row() / 16 + 1;
        let row = (pos as f64 / SCROLL_RANGE as f64 * rows as f64) as u64;
        let last = self.hex_last_row();
        let page = self.hex_rows();
        if let Some(h) = &mut self.hex {
            h.top = (row * 16).min(last.saturating_sub(page.saturating_sub(1) * 16));
        }
        self.update_status();
        self.invalidate();
    }

    pub(crate) fn hex_paint(&mut self) {
        let Some(h) = self.hex else { return };
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(self.view, &mut ps);
            let snap = self.doc.snapshot();
            let rows = self.hex_rows() as usize + 1;
            let end = (h.top + rows as u64 * 16).min(snap.len());
            let data = snap.read(h.top.min(end)..end);
            let sel = self.doc.selections().primary();
            let matches = match (&self.searcher, self.findbar.visible) {
                (Some(s), true) => {
                    s.matches_in(snap, h.top.saturating_sub(4096)..end, HIGHLIGHT_LIMIT)
                }
                _ => Vec::new(),
            };
            let frame = HexFrame {
                layout: self.hex_layout(),
                first: h.top,
                data: &data,
                len: snap.len(),
                rows,
                caret: sel.head,
                nibble: h.nibble,
                pane: h.pane,
                caret_visible: self.focused && self.caret_visible,
                overwrite: h.overwrite,
                selection: sel.range(),
                matches: &matches,
                scroll_x: self.scroll_x,
            };
            let r = self
                .renderer
                .draw_hex(self.view, self.view_px.0, self.view_px.1, &frame);
            let _ = EndPaint(self.view, &ps);
            match r {
                Ok(true) => {}
                Ok(false) => self.invalidate(),
                Err(e) => eprintln!("draw failed: {}", crate::util::describe_error(&e)),
            }
        }
    }

    /// カーソルを `pos` に動かす（`extend` なら選択を広げる）。
    fn hex_move(&mut self, pos: u64, extend: bool) {
        let len = self.doc.snapshot().len();
        let pos = pos.min(len);
        let anchor = if extend {
            self.doc.selections().primary().anchor
        } else {
            pos
        };
        self.doc
            .set_selections(SelectionSet::single(Selection::new(anchor, pos)));
        if let Some(h) = &mut self.hex {
            h.nibble = 0;
        }
        self.after_move();
    }

    /// 16 進数表示のキー操作。処理したら `true`。
    pub(crate) fn hex_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        if self.hex.is_none() {
            return false;
        }
        let ctrl = key_down(VK_CONTROL);
        let shift = key_down(VK_SHIFT);
        let len = self.doc.snapshot().len();
        let head = self.doc.selections().primary().head;
        let page = self.hex_rows().saturating_sub(1).max(1) * 16;
        let target = match vk {
            VK_LEFT => head.saturating_sub(1),
            VK_RIGHT => head + 1,
            VK_UP => head.checked_sub(16).unwrap_or(head),
            VK_DOWN if head + 16 <= len => head + 16,
            VK_DOWN => head,
            VK_PRIOR => head.saturating_sub(page),
            VK_NEXT => (head + page).min(len),
            VK_HOME if ctrl => 0,
            VK_END if ctrl => len,
            VK_HOME => head / 16 * 16,
            VK_END => (head / 16 * 16 + 15).min(len),
            VK_TAB => {
                if let Some(h) = &mut self.hex {
                    h.pane = match h.pane {
                        Pane::Hex => Pane::Ascii,
                        Pane::Ascii => Pane::Hex,
                    };
                    h.nibble = 0;
                }
                self.after_move();
                return true;
            }
            VK_INSERT if !shift && !ctrl => {
                if let Some(h) = &mut self.hex {
                    h.overwrite = !h.overwrite;
                }
                self.update_status();
                self.invalidate();
                return true;
            }
            VK_BACK => {
                self.hex_delete(true);
                return true;
            }
            VK_DELETE if !shift => {
                self.hex_delete(false);
                return true;
            }
            _ => return false,
        };
        self.hex_move(target, shift);
        true
    }

    fn hex_editable(&mut self) -> bool {
        if self.doc.is_read_only() {
            self.status_msg = "読み取り専用で開いたファイルは編集できません".into();
            self.update_status();
            return false;
        }
        true
    }

    /// 選択範囲（なければカーソルの前後の 1 バイト）を削除する。
    pub(crate) fn hex_delete(&mut self, backward: bool) {
        if !self.hex_editable() {
            return;
        }
        let len = self.doc.snapshot().len();
        let sel = self.doc.selections().primary().range();
        let Some(r) = hex::delete_range(sel, len, backward) else {
            return;
        };
        let at = r.start;
        let kind = if r.end - r.start == 1 {
            EditKind::Delete
        } else {
            EditKind::Other
        };
        if self
            .doc
            .apply_changes(vec![yy_core::edit::Change::delete(r)], kind, |_| {
                SelectionSet::single(Selection::caret(at))
            })
        {
            if let Some(h) = &mut self.hex {
                h.nibble = 0;
            }
            self.after_edit();
        }
    }

    /// 文字の入力（16 進の欄では 0〜9・A〜F、文字の欄ではその文字のバイト列）。
    pub(crate) fn hex_char(&mut self, code: u16) {
        let Some(h) = self.hex else { return };
        if key_down(VK_CONTROL) && !key_down(VK_MENU) {
            return;
        }
        let Some(c) = char::from_u32(code as u32).filter(|c| !c.is_control()) else {
            return;
        };
        if !self.hex_editable() {
            return;
        }
        let snap = self.doc.snapshot().clone();
        let len = snap.len();
        let sel = self.doc.selections().primary().range();
        // 選択中の入力: 上書きなら選択の先頭から、挿入なら選択を置き換える
        let mut at = sel.start;
        if !sel.is_empty() && !h.overwrite {
            self.hex_delete(false);
            return self.hex_char(code);
        }
        if !sel.is_empty() {
            self.doc
                .set_selections(SelectionSet::single(Selection::caret(at)));
        }
        let (change, caret, nibble) = match h.pane {
            Pane::Hex => {
                let Some(d) = c.to_digit(16) else {
                    self.status_msg = "16 進数（0〜9、A〜F）を入力してください".into();
                    self.update_status();
                    return;
                };
                let byte = (at < len).then(|| snap.byte_at(at)).flatten();
                let nibble = if sel.is_empty() { h.nibble } else { 0 };
                hex::type_nibble(byte, at, nibble, d as u8, h.overwrite)
            }
            Pane::Ascii => {
                let mut buf = [0u8; 4];
                let bytes = c.encode_utf8(&mut buf).as_bytes().to_vec();
                let (change, caret) = hex::type_bytes(len, at, &bytes, h.overwrite);
                (change, caret, 0)
            }
        };
        at = caret;
        if self.doc.apply_changes(vec![change], EditKind::Typing, |_| {
            SelectionSet::single(Selection::caret(at))
        }) {
            if let Some(h) = &mut self.hex {
                h.nibble = nibble;
            }
            self.after_edit();
        }
    }

    /// クリックした位置にカーソルを置く（Shift なら選択を広げる）。
    pub(crate) fn hex_mouse_down(&mut self, x: i32, y: i32) {
        let Some((pos, pane, nibble)) = self.hex_hit(x, y) else {
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
        if let Some(h) = &mut self.hex {
            h.pane = pane;
            h.nibble = if shift { 0 } else { nibble };
            h.drag_anchor = Some(anchor);
        }
        self.after_move();
    }

    pub(crate) fn hex_mouse_move(&mut self, x: i32, y: i32) {
        let Some(anchor) = self.hex.and_then(|h| h.drag_anchor) else {
            return;
        };
        if let Some((pos, _, _)) = self.hex_hit(x, y) {
            // ドラッグで選ぶときはカーソルのあるバイトも含める
            let head = if pos >= anchor {
                (pos + 1).min(self.doc.snapshot().len())
            } else {
                pos
            };
            self.doc
                .set_selections(SelectionSet::single(Selection::new(anchor, head)));
            if let Some(h) = &mut self.hex {
                h.nibble = 0;
            }
            self.after_move();
        }
    }

    /// 画面上の位置のバイト（位置, 欄, 16 進の桁）。
    fn hex_hit(&self, x: i32, y: i32) -> Option<(u64, Pane, u8)> {
        let h = self.hex?;
        let xd = self.renderer.px_to_dip(x as f32);
        let yd = self.renderer.px_to_dip(y.max(0) as f32);
        let lh = self.renderer.metrics().line_height.max(1.0);
        let row = h.top + (yd / lh) as u64 * 16;
        let col = self.renderer.hex_col_at(xd, self.scroll_x);
        let (pane, i, nibble) = self.hex_layout().hit(col).unwrap_or((Pane::Hex, 0, 0));
        let len = self.doc.snapshot().len();
        let pos = (row + i as u64).min(len);
        Some((pos, pane, if pos == len { 0 } else { nibble }))
    }

    /// 選択範囲のコピー（16 進の欄では 16 進表記、文字の欄ではバイト列を文字として）。
    pub(crate) fn hex_copy_text(&self) -> Option<String> {
        let h = self.hex?;
        let r = self.doc.selections().primary().range();
        if r.is_empty() {
            return None;
        }
        if r.end - r.start > MAX_CLIPBOARD_BYTES / 3 {
            return None;
        }
        let bytes = self.doc.snapshot().read(r);
        Some(match h.pane {
            Pane::Hex => hex::to_hex(&bytes),
            Pane::Ascii => String::from_utf8_lossy(&bytes).into_owned(),
        })
    }

    /// 貼り付け（16 進の欄では 16 進表記として読む。読めなければ文字のバイト列）。
    /// 上書きなら同じ長さを置き換え、挿入なら挿入する。
    pub(crate) fn hex_paste(&mut self, text: &str) {
        let Some(h) = self.hex else { return };
        if !self.hex_editable() {
            return;
        }
        let bytes = match h.pane {
            Pane::Hex => hex::parse_hex(text).unwrap_or_else(|| text.as_bytes().to_vec()),
            Pane::Ascii => text.as_bytes().to_vec(),
        };
        if bytes.is_empty() {
            return;
        }
        let len = self.doc.snapshot().len();
        let sel = self.doc.selections().primary().range();
        let (change, caret) = if sel.is_empty() {
            hex::type_bytes(len, sel.start, &bytes, h.overwrite)
        } else {
            let n = bytes.len() as u64;
            (
                yy_core::edit::Change::replace_bytes(sel.clone(), bytes),
                sel.start + n,
            )
        };
        if self.doc.apply_changes(vec![change], EditKind::Paste, |_| {
            SelectionSet::single(Selection::caret(caret))
        }) {
            if let Some(h) = &mut self.hex {
                h.nibble = 0;
            }
            self.after_edit();
        }
    }

    /// 選択範囲を削除する（切り取り）。
    pub(crate) fn hex_cut(&mut self) {
        if !self.doc.selections().primary().is_empty() {
            self.hex_delete(false);
        }
    }

    /// オフセットへ移動する。
    pub(crate) fn hex_goto(&mut self, offset: u64) {
        self.hex_move(offset, false);
    }

    /// ステータスバーの位置の表示。
    pub(crate) fn hex_status(&self) -> String {
        let snap = self.doc.snapshot();
        let sel = self.doc.selections().primary();
        let head = sel.head;
        let mut s = format!("  位置 0x{head:X} ({})", group_digits(head));
        if let Some(b) = snap.byte_at(head) {
            s += &format!("  値 {b:02X}");
        }
        if !sel.is_empty() {
            let n = sel.end() - sel.start();
            s += &format!("  ({} バイト選択)", group_digits(n));
        }
        s
    }
}

/// 16 進数表示に切り替える・戻す。テキストとして読み込んだ文書（UTF-8 以外・BOM 付き）は
/// バイト列のまま開き直す（変更があれば確認する）。
pub(crate) fn cmd_toggle_hex(hwnd: HWND) {
    let Some((on, needs_raw, path)) = with_app(|a| {
        let needs_raw = a.doc.encoding() != Encoding::Utf8 || a.doc.has_bom();
        (
            a.hex.is_some(),
            needs_raw,
            a.doc.path().map(|p| p.to_owned()),
        )
    }) else {
        return;
    };
    if on {
        with_app(|a| a.set_hex(false));
        return;
    }
    if needs_raw {
        let Some(path) = path else {
            info_box(hwnd, "保存してから 16 進数表示にしてください。");
            return;
        };
        if !confirm_discard(hwnd) {
            return;
        }
        if let Some(Err(msg)) = with_app(|a| a.reopen_raw(path)) {
            error_box(hwnd, &format!("ファイルを開けません。\n{msg}"));
            return;
        }
    }
    with_app(|a| a.set_hex(true));
}

/// 「バイナリとして開く」: ファイルを選んでバイト列のまま 16 進数表示で開く。
pub(crate) fn cmd_open_binary(hwnd: HWND, paths: Vec<PathBuf>) {
    for p in paths {
        if let Some(Err(msg)) = with_app(|a| a.open_raw(p)) {
            error_box(hwnd, &format!("ファイルを開けません。\n{msg}"));
        }
    }
}

/// 16 進数表示のオフセットの入力を求めて移動する。
pub(crate) fn cmd_hex_goto(hwnd: HWND) {
    let Some((cur, len)) =
        with_app(|a| (a.doc.selections().primary().head, a.doc.snapshot().len()))
    else {
        return;
    };
    let prompt = format!("オフセット（0x1F は 16 進。最大 0x{len:X}）:");
    let Some(text) =
        crate::goto::prompt_text(hwnd, "オフセットへ移動", &prompt, &format!("0x{cur:X}"))
    else {
        return;
    };
    match hex::parse_offset(&text) {
        Some(off) if off <= len => {
            with_app(|a| a.hex_goto(off));
        }
        _ => info_box(hwnd, "オフセットが正しくありません。"),
    }
}

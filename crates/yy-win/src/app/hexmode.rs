//! 16 進数（バイナリ）編集モード。
//!
//! 文書のバイト列を 1 行 16 バイトの 16 進ダンプで表示・編集する。表示位置はバイト位置から
//! 直接決めるので、行の索引がなくても数 GB のファイルの任意の位置をすぐに表示できる。
//! 選択範囲は文書の選択（バイト位置）をそのまま使うため、検索・置換・Undo も共通になる。

use yy_core::hex::{self, Charset, HexLayout, Pane};
use yy_core::record::{self, RecordLayout};
use yy_core::{EditKind, Selection, SelectionSet};

use super::*;
use crate::render::{HexFrame, RecordFrame};

pub(crate) const ID_HEX_MODE: u16 = 207;
pub(crate) const ID_OPEN_BINARY: u16 = 111;
pub(crate) const ID_RECORD_MODE: u16 = 211;
/// 「文字の欄の文字コード」の項目（`+ 0` は ASCII、`+ 1 + i` は `Encoding::all()` の i 番目）
pub(crate) const ID_HEX_CHARSET_BASE: u16 = 1200;

/// 文字の欄の文字コードの選択肢（先頭は ASCII）。
fn charsets() -> Vec<Charset> {
    std::iter::once(None)
        .chain(Encoding::all().into_iter().map(Some))
        .collect()
}

/// 選択肢での番号。
fn charset_index(charset: Charset) -> usize {
    match charset {
        None => 0,
        Some(e) => Encoding::all()
            .iter()
            .position(|a| a.same_charset(&e))
            .map_or(0, |i| i + 1),
    }
}

fn charset_name(charset: Charset) -> String {
    charset.map_or("ASCII".to_owned(), |e| e.name().to_owned())
}

/// 「文字の欄の文字コード」のメニュー（表示メニューの中）。
pub(crate) fn create_charset_menu() -> Result<HMENU> {
    unsafe {
        let menu = CreatePopupMenu()?;
        for (i, c) in charsets().into_iter().enumerate() {
            let label = match c {
                None => "ASCII（既定）".to_owned(),
                Some(e) => e.label(),
            };
            AppendMenuW(
                menu,
                MF_STRING,
                (ID_HEX_CHARSET_BASE + i as u16) as usize,
                &HSTRING::from(label),
            )?;
        }
        Ok(menu)
    }
}

const FIXED_OVERWRITE_ONLY: &str =
    "固定長表示ではバイトを挿入・削除できません（上書きで書き換えてください）";

/// メニューの項目か。
pub(crate) fn is_charset_command(id: u16) -> bool {
    (ID_HEX_CHARSET_BASE..ID_HEX_CHARSET_BASE + charsets().len() as u16).contains(&id)
}

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
    /// 文字の欄の文字コード
    pub charset: Charset,
    /// 固定長表示のレコード長（バイト）。`None` は 1 行 16 バイトの 16 進ダンプ
    pub record: Option<u64>,
}

impl HexState {
    fn new(caret: u64, charset: Charset, record: Option<u64>) -> HexState {
        let row = record.unwrap_or(16).max(1);
        HexState {
            top: caret / row * row,
            pane: Pane::Hex,
            nibble: 0,
            overwrite: true,
            drag_anchor: None,
            charset,
            record,
        }
    }
}

impl App {
    pub(crate) fn hex_layout(&self) -> HexLayout {
        HexLayout::for_len(self.doc.snapshot().len())
    }

    /// 固定長表示のレイアウト（固定長表示でなければ `None`）。
    pub(crate) fn record_layout(&self) -> Option<RecordLayout> {
        let len = self.hex?.record?;
        let records = self.doc.snapshot().len().div_ceil(len);
        Some(RecordLayout::new(records, len))
    }

    /// 1 行（固定長表示では 1 レコード）のバイト数。
    fn row_bytes(&self) -> u64 {
        self.hex.and_then(|h| h.record).unwrap_or(16).max(1)
    }

    /// 16 進数表示に切り替える・戻す（ファイルを開き直す必要があれば `on` の前に済ませること）。
    /// `charset` は文字の欄の文字コード。
    pub(crate) fn set_hex(&mut self, on: bool, charset: Charset) {
        self.set_hex_mode(on, charset, None);
    }

    /// 16 進数表示（`record` が `Some` なら、そのレコード長の固定長表示）に切り替える・戻す。
    pub(crate) fn set_hex_mode(&mut self, on: bool, charset: Charset, record: Option<u64>) {
        ime::cancel(self.view);
        self.composition = None;
        self.rect = None;
        self.drag = None;
        let head = self.doc.selections().primary().head;
        self.hex = on.then(|| HexState::new(head, charset, record));
        if on {
            self.code = None;
        }
        self.scroll_x = 0.0;
        self.renderer.clear_cache();
        self.row_cache.borrow_mut().rows.clear();
        self.update_hex_menu();
        self.after_move();
    }

    /// 文字の欄の文字コードを変える（メニューの項目 `id`）。
    pub(crate) fn set_hex_charset(&mut self, id: u16) {
        let Some(&charset) = charsets().get((id - ID_HEX_CHARSET_BASE) as usize) else {
            return;
        };
        let Some(h) = &mut self.hex else { return };
        h.charset = charset;
        self.update_hex_menu();
        self.update_status();
        self.invalidate();
    }

    /// 「文字の欄の文字コード」の選択の表示（16 進数表示でなければ選べない）。
    pub(crate) fn update_charset_menu(&self) {
        let n = charsets().len() as u32;
        let base = ID_HEX_CHARSET_BASE as u32;
        unsafe {
            let enable = if self.hex.is_some() {
                MF_ENABLED
            } else {
                MF_GRAYED
            };
            for i in 0..n {
                let _ = EnableMenuItem(self.menu_view, base + i, MF_BYCOMMAND | enable);
            }
            let current = self.hex.map_or(0, |h| charset_index(h.charset)) as u32;
            let _ = CheckMenuRadioItem(
                self.menu_view,
                base,
                base + n - 1,
                base + current,
                MF_BYCOMMAND.0,
            );
        }
    }

    /// 表示中の最後の行の次のオフセットまでの行数。
    fn hex_rows(&self) -> u64 {
        if self.record_layout().is_some() {
            // 目盛りの 1 行と、1 レコード 3 行
            (self.page_rows().saturating_sub(1) / record::LINES).max(1) as u64
        } else {
            self.page_rows() as u64
        }
    }

    /// 最後の行（文書の終わりの位置を含む行）の先頭。
    fn hex_last_row(&self) -> u64 {
        let row = self.row_bytes();
        self.doc.snapshot().len() / row * row
    }

    /// カーソルが見えるように表示位置を変える。
    pub(crate) fn hex_ensure_visible(&mut self) {
        let head = self.doc.selections().primary().head;
        let page = self.hex_rows();
        let last = self.hex_last_row();
        let l = self.hex_layout();
        let rl = self.record_layout();
        let rb = self.row_bytes();
        let Some(h) = &mut self.hex else { return };
        let row = head / rb * rb;
        if row < h.top {
            h.top = row;
        } else if row >= h.top + page * rb {
            h.top = row + rb - page * rb;
        }
        h.top = h
            .top
            .min(last.saturating_sub((page.saturating_sub(1)) * rb));
        // 横方向（カーソルの桁が見えるように）
        let cw = self.renderer.metrics().char_width;
        let i = (head % rb) as usize;
        let col = match (rl, h.pane) {
            (Some(rl), _) => rl.byte_col(i),
            (None, Pane::Hex) => l.hex_col(i),
            (None, Pane::Ascii) => l.ascii_col(i),
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
        let rb = self.row_bytes();
        // 固定長表示ではホイールの 1 行を 1 レコードにする（1 レコードが 3 行のため）
        let rows = if self.record_layout().is_some() {
            rows.signum() * (rows.unsigned_abs().div_ceil(record::LINES as u64)) as i64
        } else {
            rows
        };
        let Some(h) = &mut self.hex else { return };
        let max = last.saturating_sub(page.saturating_sub(1) * rb);
        let top = if rows < 0 {
            h.top.saturating_sub(rows.unsigned_abs().saturating_mul(rb))
        } else {
            h.top
                .saturating_add((rows as u64).saturating_mul(rb))
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
        let rb = self.row_bytes();
        if let Some(h) = &mut self.hex {
            let row = offset.min(last) / rb * rb;
            if row < h.top || row >= h.top + page * rb {
                h.top = row.saturating_sub(page / 2 * rb);
            }
            h.top = h.top.min(last.saturating_sub(page.saturating_sub(1) * rb));
        }
        self.update_scrollbars();
        self.update_status();
        self.invalidate();
    }

    /// 縦のスクロールバー（全体の行数が大きいので位置の割合で表す）。
    pub(crate) fn hex_update_scrollbars(&mut self) {
        let Some(h) = self.hex else { return };
        let rb = self.row_bytes();
        let rows = self.hex_last_row() / rb + 1;
        let page = self.hex_rows();
        let frac = |v: u64| (v as f64 / rows.max(1) as f64 * SCROLL_RANGE as f64) as i32;
        let si = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL | SIF_DISABLENOSCROLL,
            nMin: 0,
            nMax: SCROLL_RANGE - 1,
            nPage: frac(page).clamp(1, SCROLL_RANGE) as u32,
            nPos: frac(h.top / rb),
            nTrackPos: 0,
        };
        let cw = self.renderer.metrics().char_width;
        let cols = match self.record_layout() {
            Some(rl) => rl.width(),
            None => self.hex_layout().width(),
        };
        let width = (cols + 4) as f32 * cw;
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
        let rb = self.row_bytes();
        let rows = self.hex_last_row() / rb + 1;
        let row = (pos as f64 / SCROLL_RANGE as f64 * rows as f64) as u64;
        let last = self.hex_last_row();
        let page = self.hex_rows();
        if let Some(h) = &mut self.hex {
            h.top = (row * rb).min(last.saturating_sub(page.saturating_sub(1) * rb));
        }
        self.update_status();
        self.invalidate();
    }

    pub(crate) fn hex_paint(&mut self) {
        if self.record_layout().is_some() {
            return self.record_paint();
        }
        let Some(h) = self.hex else { return };
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(self.view, &mut ps);
            let snap = self.doc.snapshot();
            let rows = self.hex_rows() as usize + 1;
            let end = (h.top + rows as u64 * 16).min(snap.len());
            // 文字の欄: 前後も読んで、表示する範囲の各バイトの文字を決める
            let mut ctx = h.top.saturating_sub(hex::CONTEXT_BYTES);
            if let Some(e) = h.charset
                && e.records().is_some()
            {
                // EBCDIC はシフト状態（SO・SI）が分かる位置から読む
                let from = h.top.saturating_sub(4096);
                let shift = [0x0E, 0x0F]
                    .iter()
                    .filter_map(|&b| snap.find_prev(from..h.top, b))
                    .max();
                ctx = ctx.min(shift.unwrap_or(from));
            }
            let ahead = (end + hex::LOOKAHEAD_BYTES).min(snap.len());
            let all = snap.read(ctx.min(h.top)..ahead);
            let skip = (h.top.min(end) - ctx.min(h.top)) as usize;
            let all_cells = hex::char_cells(
                h.charset,
                &all,
                ctx.min(h.top),
                self.config.editor.ambiguous_wide,
            );
            let n = (end - h.top.min(end)) as usize;
            let data = all[skip..skip + n].to_vec();
            let cells = &all_cells[skip..skip + n];
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
                cells,
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
        let rb = self.row_bytes();
        let fixed = self.record_layout().is_some();
        let page = self.hex_rows().saturating_sub(1).max(1) * rb;
        let target = match vk {
            VK_LEFT => head.saturating_sub(1),
            VK_RIGHT => head + 1,
            VK_UP => head.checked_sub(rb).unwrap_or(head),
            VK_DOWN if head + rb <= len => head + rb,
            VK_DOWN => head,
            VK_PRIOR => head.saturating_sub(page),
            VK_NEXT => (head + page).min(len),
            VK_HOME if ctrl => 0,
            VK_END if ctrl => len,
            VK_HOME => head / rb * rb,
            VK_END => (head / rb * rb + rb - 1).min(len),
            // 固定長表示は上書きだけ（レコードの長さを変えない）
            VK_INSERT | VK_DELETE if fixed && !shift && !ctrl => {
                self.status_msg = FIXED_OVERWRITE_ONLY.into();
                self.update_status();
                return true;
            }
            VK_BACK if fixed => head.saturating_sub(1),
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
        if self.record_layout().is_some() {
            self.status_msg = FIXED_OVERWRITE_ONLY.into();
            self.update_status();
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
                let Some(bytes) = hex::encode_text(h.charset, c.encode_utf8(&mut buf)) else {
                    self.status_msg = format!("「{c}」は {} で表せません", charset_name(h.charset));
                    self.update_status();
                    return;
                };
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
        if let Some(rl) = self.record_layout() {
            // 目盛りの行の下に 1 レコード 3 行（コード値・16 進数・文字）
            let line = ((yd / lh) as usize).saturating_sub(1);
            let (k, part) = (
                line / record::LINES,
                record::Line::from_index(line % record::LINES),
            );
            let col = self.renderer.hex_col_at(xd, self.scroll_x);
            let i = rl.hit(col).unwrap_or(0);
            let len = self.doc.snapshot().len();
            let pos = (h.top + k as u64 * rl.len as u64 + i as u64).min(len);
            let pane = if part == record::Line::Char {
                Pane::Ascii
            } else {
                Pane::Hex
            };
            let nibble = u8::from(part == record::Line::Hex && col > rl.byte_col(i) && pos < len);
            return Some((pos, pane, nibble));
        }
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
            Pane::Ascii => hex::decode_text(h.charset, &bytes),
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
            Pane::Ascii => match hex::encode_text(h.charset, text) {
                Some(b) => b,
                None => {
                    self.status_msg = format!(
                        "貼り付ける文字列に {} で表せない文字があります",
                        charset_name(h.charset)
                    );
                    self.update_status();
                    return;
                }
            },
        };
        if bytes.is_empty() {
            return;
        }
        let len = self.doc.snapshot().len();
        let sel = self.doc.selections().primary().range();
        // 固定長表示では選択範囲があってもその先頭から上書きする（長さを変えない）
        let (change, caret) = if sel.is_empty() || h.record.is_some() {
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
        let mut s = match self.hex.and_then(|h| h.record) {
            Some(n) => format!(
                "  レコード {}, {} バイト目  位置 0x{head:X}",
                group_digits(head / n + 1),
                group_digits(head % n + 1)
            ),
            None => format!("  位置 0x{head:X} ({})", group_digits(head)),
        };
        if let Some(b) = snap.byte_at(head) {
            s += &format!("  値 {b:02X}");
        }
        if !sel.is_empty() {
            let n = sel.end() - sel.start();
            s += &format!("  ({} バイト選択)", group_digits(n));
        }
        if let Some(h) = self.hex {
            s += &format!("  文字の欄: {}", charset_name(h.charset));
        }
        s
    }

    /// 固定長表示を描画する。
    fn record_paint(&mut self) {
        let (Some(h), Some(rl)) = (self.hex, self.record_layout()) else {
            return;
        };
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(self.view, &mut ps);
            let snap = self.doc.snapshot();
            let len = snap.len();
            let rec = rl.len as u64;
            // 文書の終わりがレコードの境界なら、追加用の空のレコードも出す
            let mut records = Vec::new();
            for k in 0..self.hex_rows() + 1 {
                let off = h.top + k * rec;
                if off > len {
                    break;
                }
                let data = snap.read(off..(off + rec).min(len));
                // レコードごとに読む（EBCDIC のシフト状態はレコードの先頭で戻る）
                let cells =
                    hex::char_cells(h.charset, &data, off, self.config.editor.ambiguous_wide);
                records.push((data, cells));
                if off + rec > len {
                    break;
                }
            }
            let end = (h.top + records.len() as u64 * rec).min(len);
            let sel = self.doc.selections().primary();
            let matches = match (&self.searcher, self.findbar.visible) {
                (Some(s), true) => {
                    s.matches_in(snap, h.top.saturating_sub(4096)..end, HIGHLIGHT_LIMIT)
                }
                _ => Vec::new(),
            };
            let frame = RecordFrame {
                layout: rl,
                first: h.top,
                records: &records,
                ebcdic: h.charset.is_some_and(|e| e.records().is_some()),
                caret: sel.head,
                nibble: h.nibble,
                pane: h.pane,
                caret_visible: self.focused && self.caret_visible,
                selection: sel.range(),
                matches: &matches,
                scroll_x: self.scroll_x,
            };
            let r = self
                .renderer
                .draw_record(self.view, self.view_px.0, self.view_px.1, &frame);
            let _ = EndPaint(self.view, &ps);
            match r {
                Ok(true) => {}
                Ok(false) => self.invalidate(),
                Err(e) => eprintln!("draw failed: {}", crate::util::describe_error(&e)),
            }
        }
    }
}

/// 16 進数表示に切り替える・戻す。テキストとして読み込んだ文書（UTF-8 以外・BOM 付き）は
/// バイト列のまま開き直す（変更があれば確認する）。
pub(crate) fn cmd_toggle_hex(hwnd: HWND) {
    let Some((on, needs_raw, path, encoding)) = with_app(|a| {
        let needs_raw = a.doc.encoding() != Encoding::Utf8 || a.doc.has_bom();
        (
            a.hex.is_some(),
            needs_raw,
            a.doc.path().map(|p| p.to_owned()),
            a.doc.encoding(),
        )
    }) else {
        return;
    };
    if on {
        with_app(|a| a.set_hex(false, None));
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
        if let Some(Err(msg)) = Some(reopen_raw(hwnd, path)) {
            error_box(hwnd, &format!("ファイルを開けません。\n{msg}"));
            return;
        }
    }
    // 文字の欄は、テキストとして読んでいたときの文字コードで表示する
    with_app(|a| a.set_hex(true, Some(encoding)));
}

/// 固定長表示に切り替える・戻す。レコード長は文字コードの固定長レコードの設定（なければ尋ねる）。
pub(crate) fn cmd_toggle_record(hwnd: HWND) {
    let Some((record, hex_charset, needs_raw, path, encoding)) = with_app(|a| {
        let needs_raw = a.doc.encoding() != Encoding::Utf8 || a.doc.has_bom();
        (
            a.hex.and_then(|h| h.record),
            a.hex.map(|h| h.charset),
            needs_raw,
            a.doc.path().map(|p| p.to_owned()),
            a.doc.encoding(),
        )
    }) else {
        return;
    };
    if record.is_some() {
        with_app(|a| a.set_hex(false, None));
        return;
    }
    let default = match encoding.records() {
        Some(yy_encoding::Records::Fixed(n)) => n as u64,
        _ => 80,
    };
    let Some(text) = crate::goto::prompt_text(
        hwnd,
        "固定長表示",
        "レコード長（バイト。1〜65535）:",
        &default.to_string(),
    ) else {
        return;
    };
    let len = match text.trim().parse::<u64>() {
        Ok(n) if (1..=65535).contains(&n) => n,
        _ => {
            info_box(hwnd, "レコード長が正しくありません。");
            return;
        }
    };
    let charset = match hex_charset {
        // 16 進数表示中なら文字の欄の文字コードをそのまま使う
        Some(c) => c,
        None => {
            if needs_raw {
                let Some(path) = path else {
                    info_box(hwnd, "保存してから固定長表示にしてください。");
                    return;
                };
                if !confirm_discard(hwnd) {
                    return;
                }
                if let Some(Err(msg)) = Some(reopen_raw(hwnd, path)) {
                    error_box(hwnd, &format!("ファイルを開けません。\n{msg}"));
                    return;
                }
            }
            Some(encoding)
        }
    };
    with_app(|a| a.set_hex_mode(true, charset, Some(len)));
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

/// 作業中の文書をバイト列のまま開き直す（リモートのファイルは取り寄せ直す）。
fn reopen_raw(hwnd: HWND, path: PathBuf) -> std::result::Result<(), String> {
    let remote = with_app(|a| a.doc.remote().map(|r| r.uri.clone())).flatten();
    match remote.as_deref().and_then(yy_remote::RemoteUri::parse) {
        Some(uri) => crate::remote::open(hwnd, &uri, None, true, crate::remote::OpenAs::Replace),
        None => with_app(|a| a.reopen_raw(path)).unwrap_or(Ok(())),
    }
}

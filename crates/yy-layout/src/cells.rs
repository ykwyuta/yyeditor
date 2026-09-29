//! 区切り文字モードの列揃え表示（04 章 4）。
//!
//! 各行をフィールドに分け、列ごとの幅（表示桁）に合わせて空白で埋め、区切り文字を ` │ ` で
//! 表示する。表示テキストだけを変えるので、文書の内容は変わらない。
//! フィールド内の改行で行が分かれた場合、続きの行はそのフィールドの列の位置から表示する。
//! 行の先頭が引用符の中かどうかはレコードインデックスから求める。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use yy_buffer::Snapshot;
use yy_delimited::{Dialect, LineState, RecordIndex, split_line};

use crate::columns::ColumnConfig;
use crate::row::{Row, Span, SpanKind, append_decoded, push_span};

/// 区切り文字の表示
pub const DELIM_TEXT: &str = " │ ";

/// 列揃え表示の設定と状態。スナップショット（文書の版）ごとに作る。
pub struct CellLayout {
    pub dialect: Dialect,
    /// 列ごとの幅（表示桁）
    pub widths: Vec<u32>,
    pub ccfg: ColumnConfig,
    index: Arc<Mutex<RecordIndex>>,
    /// 行の先頭の状態のキャッシュ（行の先頭の位置 → 状態）
    cache: Mutex<HashMap<u64, LineState>>,
}

impl std::fmt::Debug for CellLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CellLayout")
            .field("dialect", &self.dialect)
            .field("widths", &self.widths)
            .finish()
    }
}

impl CellLayout {
    pub fn new(
        dialect: Dialect,
        widths: Vec<u32>,
        ccfg: ColumnConfig,
        index: Arc<Mutex<RecordIndex>>,
    ) -> CellLayout {
        CellLayout {
            dialect,
            widths,
            ccfg,
            index,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// 論理行の先頭 `start` の状態。
    pub fn line_state(&self, snap: &Snapshot, start: u64) -> LineState {
        if start == 0 {
            return LineState::default();
        }
        if let Some(s) = self.cache.lock().unwrap().get(&start) {
            return *s;
        }
        let s = self.index.lock().unwrap().line_state_at(snap, start);
        let mut c = self.cache.lock().unwrap();
        if c.len() > 4096 {
            c.clear();
        }
        c.insert(start, s);
        s
    }

    /// 次の行の先頭の状態をキャッシュに入れる（連続した行を作るときに使う）。
    fn remember(&self, start: u64, s: LineState) {
        let mut c = self.cache.lock().unwrap();
        if c.len() > 4096 {
            c.clear();
        }
        c.insert(start, s);
    }

    fn width(&self, field: u32) -> u32 {
        self.widths.get(field as usize).copied().unwrap_or(0)
    }

    /// 区切り文字の表示幅。
    pub fn delim_cols(&self) -> u32 {
        self.ccfg.text_width(DELIM_TEXT, 0)
    }

    /// フィールド `field` の左端の桁。
    pub fn column_x(&self, field: u32) -> u32 {
        let d = self.delim_cols();
        (0..field).map(|f| self.width(f) + d).sum()
    }
}

/// スパン列の表示幅（桁）。`units()` と同じ数え方。
fn spans_width(text: &str, spans: &[Span], ccfg: &ColumnConfig, start_col: u32) -> u32 {
    let mut col = start_col;
    for sp in spans {
        match sp.kind {
            SpanKind::Text => {
                for c in text[sp.range.clone()].chars() {
                    col += ccfg.char_width(c, col);
                }
            }
            SpanKind::Control => col += sp.src.len() as u32,
            SpanKind::Break => col += 1,
            SpanKind::Invalid => col += 4 * sp.src.len() as u32,
            SpanKind::Escape => col += 4 * (sp.src.len() / 4) as u32,
            SpanKind::Delim | SpanKind::Pad => col += ccfg.text_width(&text[sp.range.clone()], col),
        }
    }
    col - start_col
}

/// 行の内容 `content`（改行を除く）を列揃えの表示にする。`state` は行の先頭の状態。
/// 戻り値は（表示テキスト, スパン, 次の行の先頭の状態）。
pub(crate) fn layout_line(
    content: &[u8],
    state: LineState,
    cl: &CellLayout,
    show_controls: bool,
) -> (String, Vec<Span>, LineState) {
    let (cells, next) = split_line(content, &cl.dialect, state);
    let mut text = String::with_capacity(content.len() * 2);
    let mut spans: Vec<Span> = Vec::new();
    let mut col = 0u32;
    if state.in_quotes && state.field > 0 {
        // 前の行から続くフィールドはその列の位置から表示する。前の列の区切りの縦線も
        // 同じ文字で描くので、フォントによらず他の行と位置がそろう
        let mut pad = String::new();
        for f in 0..state.field {
            pad.extend(std::iter::repeat_n(' ', cl.width(f) as usize));
            pad.push_str(DELIM_TEXT);
        }
        push_span(&mut text, &mut spans, &pad, SpanKind::Pad, 0..0);
        col = cl.column_x(state.field);
    }
    for c in &cells {
        let first_span = spans.len();
        append_decoded(
            &mut text,
            &mut spans,
            &content[c.range.clone()],
            c.range.start,
            show_controls,
        );
        col += spans_width(&text, &spans[first_span..], &cl.ccfg, col);
        let Some(d) = &c.delim else { break };
        let target = cl.column_x(c.field) + cl.width(c.field);
        if target > col {
            push_span(
                &mut text,
                &mut spans,
                &" ".repeat((target - col) as usize),
                SpanKind::Pad,
                c.range.end..c.range.end,
            );
            col = target;
        }
        push_span(
            &mut text,
            &mut spans,
            DELIM_TEXT,
            SpanKind::Delim,
            d.clone(),
        );
        col += cl.delim_cols();
    }
    (text, spans, next)
}

impl Row {
    /// 区切り文字モードの表示にする（論理行全体が 1 表示行の場合のみ）。
    pub(crate) fn apply_cells(
        &mut self,
        content: &[u8],
        snap: &Snapshot,
        cl: &CellLayout,
        show_controls: bool,
    ) {
        let state = cl.line_state(snap, self.start);
        let (text, spans, next) = layout_line(content, state, cl, show_controls);
        self.text = text;
        self.spans = spans;
        if self.ends_line {
            cl.remember(self.next, next);
        }
    }
}

/// 論理行 `line_start` から `lines` 行の各フィールドの幅（表示桁）で `widths` を広げる。
/// 1 列の幅は `cap` 桁まで。広げたら `true`。
pub fn measure_widths(
    snap: &Snapshot,
    cl: &CellLayout,
    line_start: u64,
    lines: usize,
    max_line_bytes: u64,
    cap: u32,
    widths: &mut Vec<u32>,
) -> bool {
    let len = snap.len();
    let mut pos = line_start;
    let mut state = cl.line_state(snap, pos);
    let mut changed = false;
    for _ in 0..lines {
        if pos > len {
            break;
        }
        let nl = snap.find_next(pos..len.min(pos + max_line_bytes), b'\n');
        let end = nl.unwrap_or(len);
        if nl.is_none() && end - pos >= max_line_bytes {
            // 長大行は測らない
            break;
        }
        let mut line = snap.read(pos..end);
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let (cells, next) = split_line(&line, &cl.dialect, state);
        let last = cells.len().saturating_sub(1);
        for (i, c) in cells.iter().enumerate() {
            // 行の最後のフィールド（続きのある引用符の中を含む）は幅を揃える必要がない
            if i == last {
                break;
            }
            let (t, sp) = crate::row::decode_row(&line[c.range.clone()]);
            let w = spans_width(&t, &sp, &cl.ccfg, 0).min(cap);
            let f = c.field as usize;
            if widths.len() <= f {
                widths.resize(f + 1, 1);
                changed = true;
            }
            if w > widths[f] {
                widths[f] = w;
                changed = true;
            }
        }
        state = next;
        match nl {
            Some(n) => pos = n + 1,
            None => break,
        }
    }
    changed
}

/// 論理行の先頭 `line_start` の行の、位置 `offset` を含むフィールドの範囲（行内の相対位置）と番号。
pub fn cell_at(
    snap: &Snapshot,
    cl: &CellLayout,
    line_start: u64,
    line: &[u8],
    offset: u64,
) -> Option<(std::ops::Range<usize>, u32, Option<std::ops::Range<usize>>)> {
    let state = cl.line_state(snap, line_start);
    let (cells, _) = split_line(line, &cl.dialect, state);
    let rel = offset.checked_sub(line_start)? as usize;
    cells
        .into_iter()
        .find(|c| rel <= c.range.end)
        .map(|c| (c.range, c.field, c.delim))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RowConfig, row_at, rows_from};

    fn snap(text: &str) -> Snapshot {
        Snapshot::from_bytes(text.as_bytes().to_vec())
    }

    fn layout(s: &Snapshot, widths: Vec<u32>) -> Arc<CellLayout> {
        let mut idx = RecordIndex::new(Dialect::csv());
        idx.extend(s, u64::MAX);
        Arc::new(CellLayout::new(
            Dialect::csv(),
            widths,
            ColumnConfig::default(),
            Arc::new(Mutex::new(idx)),
        ))
    }

    #[test]
    fn aligns_columns_and_maps_offsets() {
        let s = snap("a,bb,c\nxxx,y,\"z\nw\",q\n");
        let cl = layout(&s, vec![3, 2, 3]);
        let cfg = RowConfig::default().with_cells(cl);
        let rows = rows_from(&s, &cfg, 0, 4);
        let texts: Vec<&str> = rows.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "a   │ bb │ c",
                "xxx │ y  │ \"z",
                // フィールド 2 の続きは列 2 の位置から
                "    │    │ w\"  │ q",
                ""
            ]
        );
        let r = &rows[0];
        // フィールドの終わりのカーソルは空白の手前
        assert_eq!(r.text_index(1), 1);
        assert_eq!(r.text_index(2), 8);
        // 空白・区切り文字の表示の上はフィールドの終わり・区切り文字の位置
        assert_eq!(r.offset_at(2), 1);
        assert_eq!(r.offset_at(5), 1);
        assert_eq!(r.offset_at(8), 2);
        let r = row_at(&s, &cfg, rows[2].start);
        assert_eq!(r.text_index(r.start), 15);
        assert_eq!(r.offset_at(3), r.start);
    }

    #[test]
    fn measures_widths() {
        let s = snap("id,name\n1,日本語\n22,\"a\nbbbbbbbb\",x\n");
        let cl = layout(&s, Vec::new());
        let mut w = Vec::new();
        assert!(measure_widths(&s, &cl, 0, 100, 8192, 60, &mut w));
        // 行の最後のフィールドは測らない（name 列は 4 行目のフィールド内改行の続き）
        assert_eq!(w, vec![2, 9]);
    }
}

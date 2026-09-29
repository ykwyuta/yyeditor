//! Undo / Redo（06 章 1）。
//!
//! 編集前後のスナップショットを積むだけの方式。スナップショットは `Arc` のルート 1 つなので、
//! 全置換のような巨大な編集でも Undo は O(1)。

use std::time::{Duration, Instant};

use yy_buffer::Snapshot;

use crate::selection::SelectionSet;

/// 編集の種類（連続入力を 1 つの Undo 単位にまとめる判定に使う）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// 文字入力（IME の確定を含む）
    Typing,
    /// BackSpace / Delete による 1 文字削除
    Delete,
    /// 改行の入力
    Newline,
    Paste,
    Cut,
    /// 範囲の削除やその他の一括操作
    Other,
}

/// 文書の状態を識別する番号。編集のたびに新しい値を振る。
pub type Version = u64;

#[derive(Clone)]
pub(crate) struct Entry {
    pub before: Snapshot,
    pub after: Snapshot,
    pub before_version: Version,
    pub after_version: Version,
    pub sels_before: SelectionSet,
    pub sels_after: SelectionSet,
    kind: EditKind,
    last_time: Instant,
    open: bool,
}

/// 連続入力をまとめる時間の上限
const COALESCE_WINDOW: Duration = Duration::from_millis(1500);

pub(crate) struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    max_entries: usize,
}

impl Default for History {
    fn default() -> Self {
        History {
            undo: Vec::new(),
            redo: Vec::new(),
            max_entries: 10_000,
        }
    }
}

pub(crate) struct Record {
    pub before: Snapshot,
    pub after: Snapshot,
    pub before_version: Version,
    pub after_version: Version,
    pub sels_before: SelectionSet,
    pub sels_after: SelectionSet,
    pub kind: EditKind,
}

impl History {
    /// 編集を記録する。直前の記録と同種の連続入力なら 1 つにまとめる。
    pub fn record(&mut self, r: Record) {
        self.redo.clear();
        let now = Instant::now();
        if let Some(last) = self.undo.last_mut()
            && last.open
            && last.kind == r.kind
            && matches!(r.kind, EditKind::Typing | EditKind::Delete)
            && last.after_version == r.before_version
            && now.duration_since(last.last_time) < COALESCE_WINDOW
        {
            last.after = r.after;
            last.after_version = r.after_version;
            last.sels_after = r.sels_after;
            last.last_time = now;
            return;
        }
        if let Some(last) = self.undo.last_mut() {
            last.open = false;
        }
        self.undo.push(Entry {
            before: r.before,
            after: r.after,
            before_version: r.before_version,
            after_version: r.after_version,
            sels_before: r.sels_before,
            sels_after: r.sels_after,
            kind: r.kind,
            last_time: now,
            // 改行の後の入力は新しい単位にする
            open: r.kind != EditKind::Newline,
        });
        if self.undo.len() > self.max_entries {
            let excess = self.undo.len() - self.max_entries;
            self.undo.drain(..excess);
        }
    }

    /// 以後の入力を新しい Undo 単位にする（カーソル移動・保存などで呼ぶ）。
    pub fn seal(&mut self) {
        if let Some(last) = self.undo.last_mut() {
            last.open = false;
        }
    }

    pub fn undo(&mut self) -> Option<&Entry> {
        let mut e = self.undo.pop()?;
        e.open = false;
        self.redo.push(e);
        self.redo.last()
    }

    pub fn redo(&mut self) -> Option<&Entry> {
        let e = self.redo.pop()?;
        self.undo.push(e);
        self.undo.last()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// 行数のカウント結果を履歴中のスナップショットにも反映する
    /// （Undo で戻ったときに数え直さなくて済むように）。
    pub fn fill_line_counts(&mut self, f: &dyn Fn(&yy_buffer::Piece) -> Option<u32>) {
        for e in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            for s in [&mut e.before, &mut e.after] {
                if !s.is_fully_indexed() {
                    *s = s.fill_line_counts(f);
                }
            }
        }
    }
}

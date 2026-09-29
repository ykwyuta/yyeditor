//! 行の開始状態の記録（10 章 5.3）。
//!
//! 一定間隔（ブロック）ごとに、その位置の直後の行頭の状態を記録しておく。任意の行の開始状態は
//! 直前の記録から数ブロック読むだけで求められる。編集後は変更のない先頭部分の記録を残して
//! 続きから読み直す（CSV のレコードインデックスと同じ方式）。
//!
//! まだ読んでいない遠い位置では、同期点（定義の `sync`）の行か、少し手前の行から
//! 初期状態を仮定して読み、暫定の色を付ける。

use std::sync::Arc;

use yy_buffer::Snapshot;

use crate::{LineState, MAX_LINE_BYTES, Syntax, TokenSpan};

/// 既定のブロックの大きさ
pub const BLOCK: u64 = 64 << 10;
/// 前の行の状態が分からない位置で、何バイトまでなら読み直してよいか
const MAX_RESCAN: u64 = 4 * BLOCK;

#[derive(Clone, Debug)]
struct Point {
    /// 行頭
    offset: u64,
    state: LineState,
}

/// 1 行分のトークン。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineTokens {
    /// 行頭の位置
    pub start: u64,
    /// 行の内容（改行を除く）の終わり
    pub end: u64,
    /// 次の行頭（最後の行なら文書の長さ）
    pub next: u64,
    pub spans: Vec<TokenSpan>,
}

pub struct SyntaxIndex {
    syntax: Arc<Syntax>,
    block: u64,
    /// `points[k]` は `k * block` 以降の最初の行頭（`points[0]` は文書の先頭）
    points: Vec<Point>,
    /// 読み終えた行頭
    tail: Point,
    complete: bool,
}

/// `start` から始まる行の（内容の終わり, 次の行頭）。最後の行なら次の行頭は文書の長さ。
fn line_end(snap: &Snapshot, start: u64) -> (u64, u64) {
    let len = snap.len();
    match find_newline(snap, start) {
        Some(nl) => {
            let end = if nl > start && snap.byte_at(nl - 1) == Some(b'\r') {
                nl - 1
            } else {
                nl
            };
            (end, nl + 1)
        }
        None => (len, len),
    }
}

/// `start` 以降の最初の改行。`Snapshot::find_next` は範囲内のピースをすべて集めるため、
/// 文書の終わりまでを一度に探さず、範囲を広げながら探す。
fn find_newline(snap: &Snapshot, start: u64) -> Option<u64> {
    let len = snap.len();
    let mut from = start;
    let mut window = 4096u64;
    while from < len {
        let to = from.saturating_add(window).min(len);
        if let Some(nl) = snap.find_next(from..to, b'\n') {
            return Some(nl);
        }
        from = to;
        window = (window * 2).min(1 << 20);
    }
    None
}

/// `floor..end` の最後の改行（[`find_newline`] と同じく範囲を広げながら探す）。
fn find_newline_back(snap: &Snapshot, floor: u64, end: u64) -> Option<u64> {
    let mut to = end;
    let mut window = 4096u64;
    while to > floor {
        let from = to.saturating_sub(window).max(floor);
        if let Some(nl) = snap.find_prev(from..to, b'\n') {
            return Some(nl);
        }
        to = from;
        window = (window * 2).min(1 << 20);
    }
    None
}

impl SyntaxIndex {
    pub fn new(syntax: Arc<Syntax>) -> SyntaxIndex {
        SyntaxIndex::with_block(syntax, BLOCK)
    }

    pub fn with_block(syntax: Arc<Syntax>, block: u64) -> SyntaxIndex {
        let p = Point {
            offset: 0,
            state: LineState::default(),
        };
        SyntaxIndex {
            syntax,
            block: block.max(1),
            points: vec![p.clone()],
            tail: p,
            complete: false,
        }
    }

    pub fn syntax(&self) -> &Arc<Syntax> {
        &self.syntax
    }

    pub fn is_complete(&self) -> bool {
        self.complete || !self.syntax.multiline
    }

    /// 読み終えた位置。
    pub fn scanned(&self) -> u64 {
        self.tail.offset
    }

    /// 位置 `prefix` より前は変わっていないものとして、それより後ろの記録を捨てる。
    pub fn truncate(&mut self, prefix: u64) {
        let keep = self.points.partition_point(|p| p.offset <= prefix).max(1);
        self.points.truncate(keep);
        self.tail = self.points.last().unwrap().clone();
        self.complete = false;
    }

    /// 1 行を読んで（次の行頭, 次の行の開始状態）を返す。
    fn step(&self, snap: &Snapshot, start: u64, state: &LineState) -> (u64, LineState) {
        let (end, next) = line_end(snap, start);
        let bytes = snap.read(start..end.min(start + MAX_LINE_BYTES as u64));
        let mut spans = Vec::new();
        let st = self.syntax.highlight_line(state, &bytes, &mut spans);
        (next, st)
    }

    /// 読み進める（最大 `max_bytes`）。文書の終わりまで読んだら `true`。
    pub fn extend(&mut self, snap: &Snapshot, max_bytes: u64) -> bool {
        if !self.syntax.multiline {
            self.complete = true;
            return true;
        }
        let len = snap.len();
        let stop = self.tail.offset.saturating_add(max_bytes).min(len);
        while self.tail.offset < stop {
            let (next, st) = self.step(snap, self.tail.offset, &self.tail.state);
            let crossed = next / self.block > self.tail.offset / self.block;
            self.tail = Point {
                offset: next,
                state: st,
            };
            if crossed && next < len {
                self.points.push(self.tail.clone());
            }
            if next >= len {
                break;
            }
        }
        self.complete = self.tail.offset >= len;
        self.complete
    }

    /// 行頭 `line_start` の開始状態。記録から遠くてまだ読んでいなければ `None`。
    pub fn state_at(&self, snap: &Snapshot, line_start: u64) -> Option<LineState> {
        if !self.syntax.multiline {
            return Some(LineState::default());
        }
        let k = self.points.partition_point(|p| p.offset <= line_start) - 1;
        let mut base = &self.points[k];
        if self.tail.offset <= line_start && self.tail.offset > base.offset {
            base = &self.tail;
        }
        if line_start - base.offset > MAX_RESCAN {
            return None;
        }
        let (mut off, mut st) = (base.offset, base.state.clone());
        while off < line_start {
            let (next, s) = self.step(snap, off, &st);
            if next <= off {
                break;
            }
            off = next;
            st = s;
        }
        Some(st)
    }

    /// 状態が分からない位置の暫定の開始状態: 同期点の行か、少し手前の行から初期状態を仮定して読む。
    fn provisional_state(&self, snap: &Snapshot, line_start: u64) -> LineState {
        let floor = line_start.saturating_sub(MAX_RESCAN);
        // 手前の同期点を探す（見つからなければ `floor` の後の最初の行頭）
        let mut start = match find_newline(snap, floor).filter(|&nl| nl < line_start) {
            Some(nl) if floor > 0 => nl + 1,
            _ => floor,
        };
        if self.syntax.sync.is_some() {
            let mut pos = line_start;
            while pos > start {
                let prev = find_newline_back(snap, start, pos - 1)
                    .map_or(start, |p| p + 1)
                    .max(start);
                let (end, _) = line_end(snap, prev);
                let bytes = snap.read(prev..end.min(prev + MAX_LINE_BYTES as u64));
                if self.syntax.is_sync_line(&bytes) {
                    start = prev;
                    break;
                }
                pos = prev;
            }
        }
        let (mut off, mut st) = (start, LineState::default());
        while off < line_start {
            let (next, s) = self.step(snap, off, &st);
            if next <= off {
                break;
            }
            off = next;
            st = s;
        }
        st
    }

    /// 行頭 `first` から、行頭が `until` より前の行までを色付けする。
    /// 開始状態が記録から求められなかった（暫定の色）なら 2 番目の値が `false`。
    pub fn highlight_lines(
        &self,
        snap: &Snapshot,
        first: u64,
        until: u64,
    ) -> (Vec<LineTokens>, bool) {
        let (mut st, exact) = match self.state_at(snap, first) {
            Some(s) => (s, true),
            None => (self.provisional_state(snap, first), false),
        };
        let len = snap.len();
        let mut out = Vec::new();
        let mut off = first;
        loop {
            let (end, next) = line_end(snap, off);
            let bytes = snap.read(off..end.min(off + MAX_LINE_BYTES as u64));
            let mut spans = Vec::new();
            st = self.syntax.highlight_line(&st, &bytes, &mut spans);
            out.push(LineTokens {
                start: off,
                end,
                next,
                spans,
            });
            if next >= len || next <= off || next >= until {
                break;
            }
            off = next;
        }
        (out, exact)
    }
}

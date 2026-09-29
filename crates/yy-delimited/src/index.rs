//! レコードインデックス（04 章 3.2）。
//!
//! 文書の一定間隔（ブロック）の位置ごとに解析状態を記録しておく。任意の行の先頭の状態は、
//! 直前の記録位置から高々 1 ブロック読むだけで求められる。
//! 編集後は、変更のない先頭部分の記録を残して続きから読み直す。

use yy_buffer::Snapshot;

use crate::{Dialect, LineState, Scanner};

/// 記録位置での状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub offset: u64,
    pub scanner: Scanner,
}

/// 解析状態の記録。`points[k].offset == k * block`。
#[derive(Clone, Debug)]
pub struct RecordIndex {
    pub dialect: Dialect,
    block: u64,
    points: Vec<Point>,
    /// 最後の記録位置から読み進めた状態（次の記録位置の手前まで）
    tail: Point,
    /// 文書全体を読み終えたか
    complete: bool,
}

/// 既定のブロックの大きさ
pub const BLOCK: u64 = 64 << 10;

/// 前の行の状態が分からない位置で、何バイトまでなら先頭から読み直してよいか。
const MAX_RESCAN: u64 = 4 * BLOCK;

impl RecordIndex {
    pub fn new(dialect: Dialect) -> RecordIndex {
        RecordIndex::with_block(dialect, BLOCK)
    }

    pub fn with_block(dialect: Dialect, block: u64) -> RecordIndex {
        let p = Point {
            offset: 0,
            scanner: Scanner::new(dialect),
        };
        RecordIndex {
            dialect,
            block,
            points: vec![p],
            tail: p,
            complete: false,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// 読み終えた位置。
    pub fn scanned(&self) -> u64 {
        self.tail.offset
    }

    /// 位置 `prefix` より前は変わっていない（それ以降は変わった）ものとして、記録を捨てる。
    pub fn truncate(&mut self, prefix: u64) {
        let keep = (prefix / self.block) as usize + 1;
        self.points.truncate(keep.max(1));
        self.tail = *self.points.last().unwrap();
        self.complete = false;
    }

    /// 読み進める（最大 `max_bytes`）。文書の終わりまで読んだら `true`。
    pub fn extend(&mut self, snap: &Snapshot, max_bytes: u64) -> bool {
        let len = snap.len();
        let stop = (self.tail.offset + max_bytes).min(len);
        while self.tail.offset < stop {
            let next_point = (self.tail.offset / self.block + 1) * self.block;
            let end = next_point.min(stop);
            for c in snap.chunks(self.tail.offset..end) {
                self.tail.scanner.feed(c);
            }
            self.tail.offset = end;
            if end == next_point {
                self.points.push(self.tail);
            }
        }
        self.complete = self.tail.offset >= len;
        self.complete
    }

    /// 位置 `offset` での解析状態（記録から近くを読む。まだ読んでいない遠い位置なら `None`）。
    pub fn scanner_at(&self, snap: &Snapshot, offset: u64) -> Option<Scanner> {
        let k = ((offset / self.block) as usize).min(self.points.len() - 1);
        let p = self.points[k];
        if offset > p.offset + MAX_RESCAN && offset > self.tail.offset {
            return None;
        }
        let mut s = p.scanner;
        for c in snap.chunks(p.offset..offset.min(snap.len())) {
            s.feed(c);
        }
        Some(s)
    }

    /// 行の先頭 `offset` の状態。まだ読んでいない遠い位置では引用符の外とみなす。
    pub fn line_state_at(&self, snap: &Snapshot, offset: u64) -> LineState {
        self.scanner_at(snap, offset)
            .map(|s| s.line_state())
            .unwrap_or_default()
    }

    /// 位置 `offset` のレコード番号（0 始まり）。分からなければ `None`。
    pub fn record_at(&self, snap: &Snapshot, offset: u64) -> Option<u64> {
        self.scanner_at(snap, offset).map(|s| s.records)
    }

    /// 読み終えていれば、文書のレコード数（末尾が改行でなければ最後の行も数える）。
    pub fn record_count(&self, snap: &Snapshot) -> Option<u64> {
        if !self.complete {
            return None;
        }
        let s = self.tail.scanner;
        let trailing = !snap.is_empty() && snap.byte_at(snap.len() - 1) != Some(b'\n');
        Some(s.records + u64::from(trailing))
    }
}

/// 2 つのスナップショットの内容が先頭から一致している長さ（ピースの比較による下限）。
pub fn common_prefix(a: &Snapshot, b: &Snapshot) -> u64 {
    let pa = a.pieces_in(0..a.len());
    let pb = b.pieces_in(0..b.len());
    let mut off = 0u64;
    for (x, y) in pa.iter().zip(pb.iter()) {
        let (kx, ky) = (x.key(), y.key());
        if kx.source != ky.source || kx.start != ky.start {
            break;
        }
        off += x.len().min(y.len()) as u64;
        if kx.len != ky.len {
            break;
        }
    }
    off.min(a.len()).min(b.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn snap(text: &str) -> Snapshot {
        let v: Arc<Vec<u8>> = Arc::new(text.as_bytes().to_vec());
        let len = v.len() as u64;
        Snapshot::from_source_with_chunk(v, 0..len, 7, true)
    }

    /// 行の先頭の状態を先頭から逐次に読んで求めたもの（オラクル）。
    fn oracle(text: &str) -> Vec<(u64, LineState)> {
        let mut out = vec![(0, LineState::default())];
        let mut s = Scanner::new(Dialect::csv());
        for (i, b) in text.bytes().enumerate() {
            s.feed(&[b]);
            if b == b'\n' {
                out.push((i as u64 + 1, s.line_state()));
            }
        }
        out
    }

    #[test]
    fn line_states_match_sequential_scan() {
        let text = "id,text\n1,\"multi\nline, \"\"q\"\"\n end\"\n2,plain\n3,\"x\ny\"\n".repeat(20);
        let s = snap(&text);
        for block in [5, 16, 64, 1000] {
            let mut idx = RecordIndex::with_block(Dialect::csv(), block);
            while !idx.extend(&s, 37) {}
            for (off, st) in oracle(&text) {
                assert_eq!(idx.line_state_at(&s, off), st, "block {block} at {off}");
            }
            assert_eq!(idx.record_count(&s), Some(80));
        }
    }

    #[test]
    fn truncate_and_rescan_after_edit() {
        let text = "a,\"b\nc\"\nd,e\n".repeat(50);
        let s = snap(&text);
        let mut idx = RecordIndex::with_block(Dialect::csv(), 16);
        idx.extend(&s, u64::MAX);
        // 先頭の引用符を消すと以降の内外が反転する
        let edited = s.delete(2..3);
        let prefix = common_prefix(&s, &edited);
        assert!(prefix <= 2);
        idx.truncate(prefix);
        idx.extend(&edited, u64::MAX);
        let t = String::from_utf8(edited.read(0..edited.len())).unwrap();
        for (off, st) in oracle(&t) {
            assert_eq!(idx.line_state_at(&edited, off), st, "at {off}");
        }
    }
}

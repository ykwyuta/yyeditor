//! 複数箇所の一括編集（09 章 4.2）。
//!
//! すべてのカーソルの編集を 1 つの新しいスナップショットとして適用する。
//! 編集は後ろから適用するので、前の編集で後ろの位置がずれることはない。

use std::ops::Range;
use std::sync::Arc;

use yy_buffer::{MAX_PIECE_LEN, Piece, Snapshot, SourceRef, split_into_pieces};

/// 1 箇所の変更: `range` を削除して、その位置に `insert` を挿入する。
#[derive(Clone, Debug)]
pub struct Change {
    pub range: Range<u64>,
    pub insert: Vec<Piece>,
    pub insert_len: u64,
}

impl Change {
    pub fn delete(range: Range<u64>) -> Change {
        Change {
            range,
            insert: Vec::new(),
            insert_len: 0,
        }
    }

    /// `source[range]` を挿入する変更。同じソースを複数の変更で共有できる。
    pub fn replace(range: Range<u64>, source: &SourceRef, src_range: Range<u64>) -> Change {
        let len = src_range.end - src_range.start;
        Change {
            range,
            insert: split_into_pieces(source, src_range.start, len, MAX_PIECE_LEN, true),
            insert_len: len,
        }
    }

    /// バイト列を挿入する変更（新しいソースを作る）。
    pub fn replace_bytes(range: Range<u64>, bytes: Vec<u8>) -> Change {
        let len = bytes.len() as u64;
        let source: SourceRef = Arc::new(bytes);
        Change::replace(range, &source, 0..len)
    }

    fn delta(&self) -> i128 {
        self.insert_len as i128 - (self.range.end - self.range.start) as i128
    }
}

/// 変更の列を適用した結果。
pub struct Applied {
    pub snapshot: Snapshot,
    /// 各変更の挿入テキストの末尾の、適用後のオフセット（入力の順）
    pub new_ends: Vec<u64>,
    /// 各変更の開始位置の、適用後のオフセット（入力の順）
    pub new_starts: Vec<u64>,
}

/// 互いに重ならない変更の列を適用する。`changes` は開始位置の昇順であること。
pub fn apply(snap: &Snapshot, changes: Vec<Change>) -> Applied {
    for w in changes.windows(2) {
        assert!(
            w[0].range.end <= w[1].range.start,
            "changes must be sorted and non-overlapping"
        );
    }
    let mut new_starts = Vec::with_capacity(changes.len());
    let mut new_ends = Vec::with_capacity(changes.len());
    let mut shift: i128 = 0;
    for c in &changes {
        let start = (c.range.start as i128 + shift) as u64;
        new_starts.push(start);
        new_ends.push(start + c.insert_len);
        shift += c.delta();
    }
    let mut s = snap.clone();
    for c in changes.into_iter().rev() {
        s = s.edit(c.range, c.insert);
    }
    Applied {
        snapshot: s,
        new_ends,
        new_starts,
    }
}

/// 変更の前の位置 `pos` を変更後の位置に写す。変更範囲の内部の位置は挿入テキストの末尾に写す。
pub fn map_offset(changes: &[Change], pos: u64) -> u64 {
    let mut shift: i128 = 0;
    for c in changes {
        if pos < c.range.start || (pos == c.range.start && c.range.start < c.range.end) {
            break;
        }
        if pos >= c.range.end {
            shift += c.delta();
        } else {
            return (c.range.start as i128 + shift) as u64 + c.insert_len;
        }
    }
    (pos as i128 + shift) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_changes_back_to_front() {
        let s = Snapshot::from_bytes("aaa bbb ccc");
        let a = apply(
            &s,
            vec![
                Change::replace_bytes(0..3, b"X".to_vec()),
                Change::delete(4..7),
                Change::replace_bytes(11..11, b"!!".to_vec()),
            ],
        );
        assert_eq!(a.snapshot.read(0..a.snapshot.len()), b"X  ccc!!");
        assert_eq!(a.new_starts, vec![0, 2, 6]);
        assert_eq!(a.new_ends, vec![1, 2, 8]);
    }

    #[test]
    fn maps_offsets() {
        let changes = vec![
            Change::replace_bytes(2..4, b"xyz".to_vec()),
            Change::delete(6..8),
        ];
        assert_eq!(map_offset(&changes, 0), 0);
        assert_eq!(map_offset(&changes, 2), 2);
        assert_eq!(map_offset(&changes, 3), 5);
        assert_eq!(map_offset(&changes, 4), 5);
        assert_eq!(map_offset(&changes, 5), 6);
        assert_eq!(map_offset(&changes, 7), 7);
        assert_eq!(map_offset(&changes, 10), 9);
    }
}

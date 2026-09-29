use std::fmt;
use std::sync::Arc;

/// ピースが参照するバイト列の実体。
///
/// 元ファイルの mmap、変換済み一時ファイル、追記バッファのチャンクなどを表す。
/// 一度公開した範囲の内容は変更してはならない（スナップショットが参照し続けるため）。
pub trait ByteSource: Send + Sync + 'static {
    fn bytes(&self) -> &[u8];
}

impl ByteSource for Vec<u8> {
    fn bytes(&self) -> &[u8] {
        self
    }
}

impl ByteSource for Box<[u8]> {
    fn bytes(&self) -> &[u8] {
        self
    }
}

impl ByteSource for &'static [u8] {
    fn bytes(&self) -> &[u8] {
        self
    }
}

impl ByteSource for String {
    fn bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

pub type SourceRef = Arc<dyn ByteSource>;

/// 1 ピースの最大長。元ファイルはこの単位に分割され、改行数はピース単位で数える。
pub const MAX_PIECE_LEN: u32 = 1 << 20;

/// ピース内の改行（LF）数。巨大ファイルではバックグラウンドで数えるまで `Unknown`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineCount {
    Known(u32),
    Unknown,
}

/// ソース上の連続したバイト範囲への参照。
#[derive(Clone)]
pub struct Piece {
    source: SourceRef,
    start: u64,
    len: u32,
    lf: LineCount,
}

/// インデックス結果をスナップショットのピースに対応付けるためのキー。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PieceKey {
    pub source: usize,
    pub start: u64,
    pub len: u32,
}

impl Piece {
    /// 改行数を数えずにピースを作る（巨大ファイルを開くとき用）。
    pub fn unindexed(source: SourceRef, start: u64, len: u32) -> Piece {
        let p = Piece {
            source,
            start,
            len,
            lf: LineCount::Unknown,
        };
        debug_assert!(p.in_bounds());
        p
    }

    /// 改行数をその場で数えてピースを作る（小さい挿入テキスト用）。
    pub fn indexed(source: SourceRef, start: u64, len: u32) -> Piece {
        let mut p = Piece::unindexed(source, start, len);
        p.lf = LineCount::Known(count_lf(p.bytes()));
        p
    }

    fn in_bounds(&self) -> bool {
        (self.start + self.len as u64) as usize <= self.source.bytes().len()
    }

    pub fn len(&self) -> u32 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn line_count(&self) -> LineCount {
        self.lf
    }

    pub fn bytes(&self) -> &[u8] {
        let s = self.start as usize;
        &self.source.bytes()[s..s + self.len as usize]
    }

    pub fn source(&self) -> &SourceRef {
        &self.source
    }

    pub fn start(&self) -> u64 {
        self.start
    }

    pub fn key(&self) -> PieceKey {
        PieceKey {
            source: Arc::as_ptr(&self.source) as *const u8 as usize,
            start: self.start,
            len: self.len,
        }
    }

    pub(crate) fn with_line_count(&self, lf: u32) -> Piece {
        let mut p = self.clone();
        p.lf = LineCount::Known(lf);
        p
    }

    /// ピース内の `from..to` を切り出す。改行数が既知なら切り出し後も数え直して既知に保つ。
    pub(crate) fn slice(&self, from: u32, to: u32) -> Piece {
        debug_assert!(from < to && to <= self.len);
        let mut p = Piece {
            source: self.source.clone(),
            start: self.start + from as u64,
            len: to - from,
            lf: LineCount::Unknown,
        };
        if let LineCount::Known(total) = self.lf {
            // 短い方を数えて長い方は差し引きで求める
            let n = if from == 0 && (to as u64) * 2 > self.len as u64 {
                total - count_lf(&self.bytes()[to as usize..])
            } else if to == self.len && (from as u64) * 2 < self.len as u64 {
                total - count_lf(&self.bytes()[..from as usize])
            } else {
                count_lf(p.bytes())
            };
            p.lf = LineCount::Known(n);
        }
        p
    }

    /// 隣接ピースとの結合を試みる（同一ソース上で連続し、改行数の状態が揃っている場合）。
    pub(crate) fn try_merge(&self, next: &Piece) -> Option<Piece> {
        if !Arc::ptr_eq(&self.source, &next.source)
            || self.start + self.len as u64 != next.start
            || self.len as u64 + next.len as u64 > MAX_PIECE_LEN as u64
        {
            return None;
        }
        let lf = match (self.lf, next.lf) {
            (LineCount::Known(a), LineCount::Known(b)) => LineCount::Known(a + b),
            (LineCount::Unknown, LineCount::Unknown) => LineCount::Unknown,
            _ => return None,
        };
        Some(Piece {
            source: self.source.clone(),
            start: self.start,
            len: self.len + next.len,
            lf,
        })
    }
}

impl fmt::Debug for Piece {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Piece")
            .field("source", &(Arc::as_ptr(&self.source) as *const u8))
            .field("start", &self.start)
            .field("len", &self.len)
            .field("lf", &self.lf)
            .finish()
    }
}

/// LF の個数を数える。
pub fn count_lf(bytes: &[u8]) -> u32 {
    memchr::memchr_iter(b'\n', bytes).count() as u32
}

/// `pos` を UTF-8 の文字境界まで前方（末尾方向）に進める。最大 3 バイト。
pub fn align_utf8_forward(bytes: &[u8], mut pos: usize) -> usize {
    let limit = (pos + 3).min(bytes.len());
    while pos < limit && is_continuation(bytes[pos]) {
        pos += 1;
    }
    pos
}

#[inline]
pub(crate) fn is_continuation(b: u8) -> bool {
    b & 0xC0 == 0x80
}

/// `source[start..start+len]` を最大 `chunk` バイトのピース列に分割する。
/// 分割位置は UTF-8 の文字境界に合わせる。
pub fn split_into_pieces(
    source: &SourceRef,
    start: u64,
    len: u64,
    chunk: u32,
    indexed: bool,
) -> Vec<Piece> {
    assert!(chunk > 4 && chunk <= MAX_PIECE_LEN);
    let bytes = source.bytes();
    let end = start + len;
    assert!(end as usize <= bytes.len());
    let mut pieces = Vec::with_capacity((len / chunk as u64 + 1) as usize);
    let mut pos = start;
    while pos < end {
        let mut next = (pos + chunk as u64).min(end);
        if next < end {
            // 文字境界まで後退する。見つからなければ（不正な UTF-8）そのまま切る
            let mut aligned = next;
            while aligned > pos && next - aligned < 3 && is_continuation(bytes[aligned as usize]) {
                aligned -= 1;
            }
            if aligned > pos && !is_continuation(bytes[aligned as usize]) {
                next = aligned;
            }
        }
        let plen = (next - pos) as u32;
        let p = if indexed {
            Piece::indexed(source.clone(), pos, plen)
        } else {
            Piece::unindexed(source.clone(), pos, plen)
        };
        pieces.push(p);
        pos = next;
    }
    pieces
}

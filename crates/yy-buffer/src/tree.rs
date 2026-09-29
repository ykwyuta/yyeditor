//! 永続（イミュータブル）ピースツリー。
//!
//! B+木の葉にピースを並べ、各ノードにバイト数・改行数を集約する。
//! ノードは `Arc` で共有され、編集ではルートから葉までの経路だけを複製する。

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use crate::piece::{LineCount, MAX_PIECE_LEN, Piece, SourceRef, count_lf, split_into_pieces};

pub(crate) const MAX_CHILDREN: usize = 32;
pub(crate) const MIN_CHILDREN: usize = 8;

/// ノード（または文書全体）の集約値。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// 総バイト数
    pub bytes: u64,
    /// 改行数が確定しているピースの改行（LF）数の合計
    pub lf: u64,
    /// 改行数が未確定のピースのバイト数の合計
    pub unknown_bytes: u64,
    /// ピース数
    pub pieces: u64,
}

impl Summary {
    fn of_piece(p: &Piece) -> Summary {
        match p.line_count() {
            LineCount::Known(n) => Summary {
                bytes: p.len() as u64,
                lf: n as u64,
                unknown_bytes: 0,
                pieces: 1,
            },
            LineCount::Unknown => Summary {
                bytes: p.len() as u64,
                lf: 0,
                unknown_bytes: p.len() as u64,
                pieces: 1,
            },
        }
    }

    fn add(&mut self, o: &Summary) {
        self.bytes += o.bytes;
        self.lf += o.lf;
        self.unknown_bytes += o.unknown_bytes;
        self.pieces += o.pieces;
    }

    pub fn is_fully_indexed(&self) -> bool {
        self.unknown_bytes == 0
    }
}

enum Node {
    Leaf {
        summary: Summary,
        pieces: Vec<Piece>,
    },
    Internal {
        summary: Summary,
        children: Vec<Arc<Node>>,
    },
}

impl Node {
    fn leaf(pieces: Vec<Piece>) -> Arc<Node> {
        let mut summary = Summary::default();
        for p in &pieces {
            summary.add(&Summary::of_piece(p));
        }
        Arc::new(Node::Leaf { summary, pieces })
    }

    fn internal(children: Vec<Arc<Node>>) -> Arc<Node> {
        let mut summary = Summary::default();
        for c in &children {
            summary.add(c.summary());
        }
        Arc::new(Node::Internal { summary, children })
    }

    fn summary(&self) -> &Summary {
        match self {
            Node::Leaf { summary, .. } | Node::Internal { summary, .. } => summary,
        }
    }

    fn item_count(&self) -> usize {
        match self {
            Node::Leaf { pieces, .. } => pieces.len(),
            Node::Internal { children, .. } => children.len(),
        }
    }
}

/// 要素列を、1 ノードあたり最大 `MAX_CHILDREN` 個になるよう均等に分割する。
fn group<T>(items: Vec<T>, make: impl Fn(Vec<T>) -> Arc<Node>) -> Vec<Arc<Node>> {
    let n = items.len();
    if n == 0 {
        return Vec::new();
    }
    if n <= MAX_CHILDREN {
        return vec![make(items)];
    }
    let k = n.div_ceil(MAX_CHILDREN);
    let base = n / k;
    let rem = n % k;
    let mut out = Vec::with_capacity(k);
    // split_off で末尾を切り出すと残り全体を毎回コピーして O(n^2) になるため、先頭から順に取り出す
    let mut it = items.into_iter();
    for i in 0..k {
        let size = base + usize::from(i < rem);
        out.push(make(it.by_ref().take(size).collect()));
    }
    out
}

fn merge_adjacent(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::with_capacity(pieces.len());
    for p in pieces {
        if let Some(last) = out.last_mut()
            && let Some(m) = last.try_merge(&p)
        {
            *last = m;
            continue;
        }
        out.push(p);
    }
    out
}

fn merge_nodes(a: &Arc<Node>, b: &Arc<Node>) -> Arc<Node> {
    match (&**a, &**b) {
        (Node::Leaf { pieces: pa, .. }, Node::Leaf { pieces: pb, .. }) => {
            let mut v = pa.clone();
            v.extend(pb.iter().cloned());
            Node::leaf(merge_adjacent(v))
        }
        (Node::Internal { children: ca, .. }, Node::Internal { children: cb, .. }) => {
            let mut v = ca.clone();
            v.extend(cb.iter().cloned());
            Node::internal(v)
        }
        _ => unreachable!("siblings must have the same depth"),
    }
}

/// 兄弟ノード列のうち要素数が少ないものを隣と結合する。
fn rebalance(children: Vec<Arc<Node>>) -> Vec<Arc<Node>> {
    let mut out: Vec<Arc<Node>> = Vec::with_capacity(children.len());
    for c in children {
        if let Some(last) = out.last_mut() {
            let (a, b) = (last.item_count(), c.item_count());
            if (a < MIN_CHILDREN || b < MIN_CHILDREN) && a + b <= MAX_CHILDREN {
                *last = merge_nodes(last, &c);
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// `node` の `start..end` を削除して `start` に `insert` を挿入した結果を、
/// `node` と同じ深さのノード列（0 個以上）として返す。
fn edit_node(node: &Arc<Node>, start: u64, end: u64, insert: &[Piece]) -> Vec<Arc<Node>> {
    match &**node {
        Node::Leaf { pieces, .. } => {
            let mut out = Vec::with_capacity(pieces.len() + insert.len() + 1);
            let mut inserted = false;
            let mut ps = 0u64;
            for p in pieces {
                let pe = ps + p.len() as u64;
                if pe <= start {
                    out.push(p.clone());
                } else if ps >= end {
                    if !inserted {
                        out.extend_from_slice(insert);
                        inserted = true;
                    }
                    out.push(p.clone());
                } else {
                    if ps < start {
                        out.push(p.slice(0, (start - ps) as u32));
                    }
                    if !inserted {
                        out.extend_from_slice(insert);
                        inserted = true;
                    }
                    if pe > end {
                        out.push(p.slice((end - ps) as u32, p.len()));
                    }
                }
                ps = pe;
            }
            if !inserted {
                out.extend_from_slice(insert);
            }
            group(merge_adjacent(out), Node::leaf)
        }
        Node::Internal { children, .. } => {
            let mut out = Vec::with_capacity(children.len() + 2);
            let mut target_done = false;
            let mut cs = 0u64;
            for child in children {
                let ce = cs + child.summary().bytes;
                let is_target = !target_done && start <= ce;
                let overlaps = cs < end && ce > start;
                if is_target {
                    target_done = true;
                    let lo = start - cs;
                    let hi = end.min(ce) - cs;
                    out.extend(edit_node(child, lo, hi, insert));
                } else if overlaps {
                    if !(start <= cs && ce <= end) {
                        let lo = start.max(cs) - cs;
                        let hi = end.min(ce) - cs;
                        out.extend(edit_node(child, lo, hi, &[]));
                    }
                    // 完全に削除範囲に含まれる子は捨てる
                } else {
                    out.push(child.clone());
                }
                cs = ce;
            }
            group(rebalance(out), Node::internal)
        }
    }
}

fn fill_node(node: &Arc<Node>, f: &dyn Fn(&Piece) -> Option<u32>) -> Arc<Node> {
    if node.summary().unknown_bytes == 0 {
        return node.clone();
    }
    match &**node {
        Node::Leaf { pieces, .. } => {
            let mut changed = false;
            let new: Vec<Piece> = pieces
                .iter()
                .map(|p| match p.line_count() {
                    LineCount::Unknown => match f(p) {
                        Some(n) => {
                            changed = true;
                            p.with_line_count(n)
                        }
                        None => p.clone(),
                    },
                    LineCount::Known(_) => p.clone(),
                })
                .collect();
            if changed {
                Node::leaf(new)
            } else {
                node.clone()
            }
        }
        Node::Internal { children, .. } => {
            let mut changed = false;
            let new: Vec<Arc<Node>> = children
                .iter()
                .map(|c| {
                    let n = fill_node(c, f);
                    changed |= !Arc::ptr_eq(&n, c);
                    n
                })
                .collect();
            if changed {
                Node::internal(new)
            } else {
                node.clone()
            }
        }
    }
}

fn count_node_lf(node: &Node) -> u64 {
    let s = node.summary();
    if s.unknown_bytes == 0 {
        return s.lf;
    }
    match node {
        Node::Leaf { pieces, .. } => pieces
            .iter()
            .map(|p| match p.line_count() {
                LineCount::Known(n) => n as u64,
                LineCount::Unknown => count_lf(p.bytes()) as u64,
            })
            .sum(),
        Node::Internal { children, .. } => children.iter().map(|c| count_node_lf(c)).sum(),
    }
}

/// 行番号からオフセットを引いた結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineLookup {
    /// 行頭のバイトオフセット
    Found(u64),
    /// 行数がまだ数えられていない範囲にある
    NotIndexed,
    /// 行数を超えている
    OutOfRange,
}

/// オフセットから求めた行番号（0 始まり）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinePosition {
    pub line: u64,
    /// `false` の場合、未確定範囲の改行数を推定して求めた値
    pub exact: bool,
}

/// スナップショットの一部を指すバイト列。
#[derive(Clone, Copy)]
pub struct Slice<'a> {
    /// 文書内での開始オフセット
    pub offset: u64,
    pub bytes: &'a [u8],
}

/// 文書のある時点の内容。複製は O(1) で、元の内容は以後の編集の影響を受けない。
#[derive(Clone)]
pub struct Snapshot {
    root: Arc<Node>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot::empty()
    }
}

impl fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Snapshot")
            .field("summary", self.summary())
            .finish()
    }
}

impl Snapshot {
    pub fn empty() -> Snapshot {
        Snapshot {
            root: Node::leaf(Vec::new()),
        }
    }

    /// ピース列から均衡した木を作る。
    pub fn from_pieces(pieces: Vec<Piece>) -> Snapshot {
        let mut level = group(merge_adjacent(pieces), Node::leaf);
        if level.is_empty() {
            return Snapshot::empty();
        }
        while level.len() > 1 {
            level = group(level, Node::internal);
        }
        Snapshot {
            root: level.pop().unwrap(),
        }
    }

    /// ソースの `range` 全体を参照する文書を作る。
    /// `indexed == false` なら改行数は数えず、後で [`Snapshot::fill_line_counts`] で埋める。
    pub fn from_source(source: SourceRef, range: Range<u64>, indexed: bool) -> Snapshot {
        Snapshot::from_source_with_chunk(source, range, MAX_PIECE_LEN, indexed)
    }

    /// ピースの分割単位を指定して [`Snapshot::from_source`] する（テスト用）。
    pub fn from_source_with_chunk(
        source: SourceRef,
        range: Range<u64>,
        chunk: u32,
        indexed: bool,
    ) -> Snapshot {
        let pieces = split_into_pieces(
            &source,
            range.start,
            range.end - range.start,
            chunk,
            indexed,
        );
        let mut level = group(pieces, Node::leaf);
        if level.is_empty() {
            return Snapshot::empty();
        }
        while level.len() > 1 {
            level = group(level, Node::internal);
        }
        Snapshot {
            root: level.pop().unwrap(),
        }
    }

    /// メモリ上のバイト列から文書を作る（改行数は数える）。
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Snapshot {
        let v: Vec<u8> = bytes.into();
        let len = v.len() as u64;
        Snapshot::from_source(Arc::new(v), 0..len, true)
    }

    pub fn summary(&self) -> &Summary {
        self.root.summary()
    }

    pub fn len(&self) -> u64 {
        self.summary().bytes
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_fully_indexed(&self) -> bool {
        self.summary().is_fully_indexed()
    }

    /// 行数（改行数 + 1）。未確定範囲があれば `None`。
    pub fn line_count(&self) -> Option<u64> {
        let s = self.summary();
        s.is_fully_indexed().then_some(s.lf + 1)
    }

    /// 未確定範囲を既知の範囲の改行密度から推定した行数。
    pub fn estimated_line_count(&self) -> u64 {
        let s = self.summary();
        s.lf + 1 + self.estimate_lf(s.unknown_bytes)
    }

    fn estimate_lf(&self, unknown_bytes: u64) -> u64 {
        if unknown_bytes == 0 {
            return 0;
        }
        let s = self.summary();
        let known = s.bytes - s.unknown_bytes;
        if known >= 4096 {
            (unknown_bytes as f64 * s.lf as f64 / known as f64).round() as u64
        } else {
            unknown_bytes / 64
        }
    }

    pub fn ptr_eq(&self, other: &Snapshot) -> bool {
        Arc::ptr_eq(&self.root, &other.root)
    }

    /// `range` を削除して `range.start` に `insert` を挿入した新しいスナップショットを返す。
    pub fn edit(&self, range: Range<u64>, insert: Vec<Piece>) -> Snapshot {
        let len = self.len();
        assert!(
            range.start <= range.end && range.end <= len,
            "edit range {range:?} out of bounds (len {len})"
        );
        let insert: Vec<Piece> = insert.into_iter().filter(|p| !p.is_empty()).collect();
        if range.is_empty() && insert.is_empty() {
            return self.clone();
        }
        let mut level = edit_node(&self.root, range.start, range.end, &insert);
        if level.is_empty() {
            return Snapshot::empty();
        }
        while level.len() > 1 {
            level = group(level, Node::internal);
        }
        let mut root = level.pop().unwrap();
        loop {
            let only = match &*root {
                Node::Internal { children, .. } if children.len() == 1 => children[0].clone(),
                _ => break,
            };
            root = only;
        }
        Snapshot { root }
    }

    /// バイト列を挿入する（新しいソースを作る。M2 以降は追記バッファに置き換える）。
    pub fn insert(&self, offset: u64, bytes: &[u8]) -> Snapshot {
        if bytes.is_empty() {
            return self.clone();
        }
        let source: SourceRef = Arc::new(bytes.to_vec());
        let pieces = split_into_pieces(&source, 0, bytes.len() as u64, MAX_PIECE_LEN, true);
        self.edit(offset..offset, pieces)
    }

    pub fn delete(&self, range: Range<u64>) -> Snapshot {
        self.edit(range, Vec::new())
    }

    /// `range` に含まれるバイト列を、文書順の連続片の列として返す。
    pub fn slices(&self, range: Range<u64>) -> Vec<Slice<'_>> {
        let range = range.start.min(self.len())..range.end.min(self.len());
        let mut out = Vec::new();
        if !range.is_empty() {
            collect_slices(&self.root, 0, &range, &mut out);
        }
        out
    }

    pub fn chunks(&self, range: Range<u64>) -> impl Iterator<Item = &[u8]> {
        self.slices(range).into_iter().map(|s| s.bytes)
    }

    /// `range` の内容をコピーして返す。
    pub fn read(&self, range: Range<u64>) -> Vec<u8> {
        let mut v = Vec::with_capacity((range.end.saturating_sub(range.start)) as usize);
        for c in self.chunks(range) {
            v.extend_from_slice(c);
        }
        v
    }

    pub fn byte_at(&self, offset: u64) -> Option<u8> {
        self.slices(offset..offset + 1).first().map(|s| s.bytes[0])
    }

    /// `range` 内で最初に現れる `byte` の位置。
    pub fn find_next(&self, range: Range<u64>, byte: u8) -> Option<u64> {
        self.slices(range)
            .into_iter()
            .find_map(|s| memchr::memchr(byte, s.bytes).map(|i| s.offset + i as u64))
    }

    /// `range` 内で最後に現れる `byte` の位置。
    pub fn find_prev(&self, range: Range<u64>, byte: u8) -> Option<u64> {
        self.slices(range)
            .into_iter()
            .rev()
            .find_map(|s| memchr::memrchr(byte, s.bytes).map(|i| s.offset + i as u64))
    }

    /// `range` 内で最初に現れるバイト列 `needle` の位置（ピースの境界をまたぐ一致も見つける）。
    pub fn find_bytes(&self, range: Range<u64>, needle: &[u8]) -> Option<u64> {
        if needle.is_empty() {
            return (range.start <= range.end.min(self.len())).then_some(range.start);
        }
        let finder = memchr::memmem::Finder::new(needle);
        let keep = needle.len() - 1;
        // 直前の片の末尾（最大 needle.len() - 1 バイト）
        let mut carry: Vec<u8> = Vec::new();
        let mut carry_start = 0u64;
        for s in self.slices(range) {
            if !carry.is_empty() {
                let mut w = carry.clone();
                w.extend_from_slice(&s.bytes[..keep.min(s.bytes.len())]);
                if let Some(i) = finder.find(&w) {
                    return Some(carry_start + i as u64);
                }
            }
            if let Some(i) = finder.find(s.bytes) {
                return Some(s.offset + i as u64);
            }
            let mut w = std::mem::take(&mut carry);
            w.extend_from_slice(&s.bytes[s.bytes.len().saturating_sub(keep)..]);
            let n = w.len().min(keep);
            carry = w[w.len() - n..].to_vec();
            carry_start = s.offset + s.bytes.len() as u64 - n as u64;
        }
        None
    }

    /// `range` を覆うピースの列（両端は切り出したもの）。一括編集で木を組み直すときに使う。
    pub fn pieces_in(&self, range: Range<u64>) -> Vec<Piece> {
        let range = range.start.min(self.len())..range.end.min(self.len());
        let mut out = Vec::new();
        if !range.is_empty() {
            collect_pieces(&self.root, 0, &range, &mut out);
        }
        out
    }

    /// 行 `line`（0 始まり）の先頭オフセット。
    ///
    /// `count_unknown` が真なら、未確定のピースもその場で数える（結果は記録しない）。
    pub fn line_start(&self, line: u64, count_unknown: bool) -> LineLookup {
        if line == 0 {
            return LineLookup::Found(0);
        }
        let mut remaining = line;
        let mut offset = 0u64;
        let mut node = &self.root;
        'descend: loop {
            match &**node {
                Node::Internal { children, .. } => {
                    for child in children {
                        let s = child.summary();
                        let child_lf = if s.unknown_bytes == 0 {
                            Some(s.lf)
                        } else if count_unknown {
                            Some(count_node_lf(child))
                        } else {
                            None
                        };
                        match child_lf {
                            Some(n) if n < remaining => {
                                remaining -= n;
                                offset += s.bytes;
                            }
                            _ => {
                                node = child;
                                continue 'descend;
                            }
                        }
                    }
                    return LineLookup::OutOfRange;
                }
                Node::Leaf { pieces, .. } => {
                    for p in pieces {
                        let n = match p.line_count() {
                            LineCount::Known(n) => n as u64,
                            LineCount::Unknown if count_unknown => count_lf(p.bytes()) as u64,
                            LineCount::Unknown => return LineLookup::NotIndexed,
                        };
                        if n < remaining {
                            remaining -= n;
                            offset += p.len() as u64;
                            continue;
                        }
                        let pos = memchr::memchr_iter(b'\n', p.bytes())
                            .nth((remaining - 1) as usize)
                            .expect("line count mismatch");
                        return LineLookup::Found(offset + pos as u64 + 1);
                    }
                    return LineLookup::OutOfRange;
                }
            }
        }
    }

    /// オフセット `offset` を含む行の行番号（0 始まり）。
    pub fn line_of_offset(&self, offset: u64) -> LinePosition {
        let offset = offset.min(self.len());
        let mut lf = 0u64;
        let mut unknown = 0u64;
        let mut base = 0u64;
        let mut node = &self.root;
        'descend: loop {
            match &**node {
                Node::Internal { children, .. } => {
                    for child in children {
                        let s = child.summary();
                        if base + s.bytes <= offset {
                            lf += s.lf;
                            unknown += s.unknown_bytes;
                            base += s.bytes;
                        } else {
                            node = child;
                            continue 'descend;
                        }
                    }
                    break;
                }
                Node::Leaf { pieces, .. } => {
                    for p in pieces {
                        let pe = base + p.len() as u64;
                        if pe <= offset {
                            match p.line_count() {
                                LineCount::Known(n) => lf += n as u64,
                                LineCount::Unknown => unknown += p.len() as u64,
                            }
                            base = pe;
                        } else {
                            lf += count_lf(&p.bytes()[..(offset - base) as usize]) as u64;
                            break;
                        }
                    }
                    break;
                }
            }
        }
        LinePosition {
            line: lf + self.estimate_lf(unknown),
            exact: unknown == 0,
        }
    }

    /// 改行数が未確定のピースを文書順に最大 `limit` 個返す。
    pub fn unindexed_pieces(&self, limit: usize) -> Vec<Piece> {
        let mut out = Vec::new();
        collect_unindexed(&self.root, limit, &mut out);
        out
    }

    /// 未確定ピースの改行数を `f` で埋めた新しいスナップショットを返す。
    pub fn fill_line_counts(&self, f: &dyn Fn(&Piece) -> Option<u32>) -> Snapshot {
        Snapshot {
            root: fill_node(&self.root, f),
        }
    }

    /// 木の不変条件を検査する（テスト用）。木の高さを返す。
    #[doc(hidden)]
    pub fn check_invariants(&self) -> usize {
        fn check(node: &Node, is_root: bool) -> usize {
            assert!(node.item_count() <= MAX_CHILDREN, "node overflow");
            assert!(is_root || node.item_count() > 0, "empty non-root node");
            let mut s = Summary::default();
            let depth = match node {
                Node::Leaf { pieces, .. } => {
                    for p in pieces {
                        assert!(!p.is_empty());
                        if let LineCount::Known(n) = p.line_count() {
                            assert_eq!(n, count_lf(p.bytes()), "stale line count");
                        }
                        s.add(&Summary::of_piece(p));
                    }
                    1
                }
                Node::Internal { children, .. } => {
                    let mut depth = None;
                    for c in children {
                        let d = check(c, false);
                        assert!(depth.is_none_or(|x| x == d), "unbalanced tree");
                        depth = Some(d);
                        s.add(c.summary());
                    }
                    depth.unwrap() + 1
                }
            };
            assert_eq!(&s, node.summary(), "stale summary");
            depth
        }
        check(&self.root, true)
    }
}

fn collect_slices<'a>(node: &'a Node, base: u64, range: &Range<u64>, out: &mut Vec<Slice<'a>>) {
    match node {
        Node::Internal { children, .. } => {
            let mut cs = base;
            for c in children {
                let ce = cs + c.summary().bytes;
                if ce > range.start && cs < range.end {
                    collect_slices(c, cs, range, out);
                }
                if ce >= range.end {
                    break;
                }
                cs = ce;
            }
        }
        Node::Leaf { pieces, .. } => {
            let mut ps = base;
            for p in pieces {
                let pe = ps + p.len() as u64;
                if pe > range.start && ps < range.end {
                    let lo = range.start.max(ps) - ps;
                    let hi = range.end.min(pe) - ps;
                    out.push(Slice {
                        offset: ps + lo,
                        bytes: &p.bytes()[lo as usize..hi as usize],
                    });
                }
                if pe >= range.end {
                    break;
                }
                ps = pe;
            }
        }
    }
}

fn collect_pieces(node: &Node, base: u64, range: &Range<u64>, out: &mut Vec<Piece>) {
    match node {
        Node::Internal { children, .. } => {
            let mut cs = base;
            for c in children {
                let ce = cs + c.summary().bytes;
                if ce > range.start && cs < range.end {
                    collect_pieces(c, cs, range, out);
                }
                if ce >= range.end {
                    break;
                }
                cs = ce;
            }
        }
        Node::Leaf { pieces, .. } => {
            let mut ps = base;
            for p in pieces {
                let pe = ps + p.len() as u64;
                if pe > range.start && ps < range.end {
                    let lo = (range.start.max(ps) - ps) as u32;
                    let hi = (range.end.min(pe) - ps) as u32;
                    if lo == 0 && hi == p.len() {
                        out.push(p.clone());
                    } else {
                        out.push(p.slice(lo, hi));
                    }
                }
                if pe >= range.end {
                    break;
                }
                ps = pe;
            }
        }
    }
}

fn collect_unindexed(node: &Node, limit: usize, out: &mut Vec<Piece>) {
    if out.len() >= limit || node.summary().unknown_bytes == 0 {
        return;
    }
    match node {
        Node::Leaf { pieces, .. } => {
            for p in pieces {
                if out.len() >= limit {
                    return;
                }
                if p.line_count() == LineCount::Unknown {
                    out.push(p.clone());
                }
            }
        }
        Node::Internal { children, .. } => {
            for c in children {
                collect_unindexed(c, limit, out);
            }
        }
    }
}

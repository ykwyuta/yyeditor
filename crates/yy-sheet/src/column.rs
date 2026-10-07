//! 列（チャンクの区間の並び＋編集の差分。15 章 3.3・3.4）。
//!
//! 列はチャンクの一部を指す区間（ピース）の並びで、行番号から区間を二分探索で求める。行の挿入・
//! 削除は区間を分ける・縮めるだけで、チャンクのデータはコピーしない。1 セルの編集は差分に入れ、
//! 差分が一定量を超えたら、その部分のチャンクを作り直す。
//!
//! 列のすべては `Arc` で共有するので、複製（スナップショット）は O(1)。

use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;

use crate::Context;
use crate::chunk::{Builder, CellRef, Chunk, Data, MAX_ROWS};
use crate::value::Value;

/// 差分がこの数を超えたらチャンクに書き戻す。
pub const DELTA_LIMIT: usize = 4096;

/// チャンクの区間。
#[derive(Clone, Debug)]
pub struct Piece {
    pub chunk: Arc<Chunk>,
    pub start: u32,
    pub len: u32,
}

/// 列。
#[derive(Clone, Debug, Default)]
pub struct Column {
    pub name: Arc<str>,
    /// 既定の表示形式（日付の列など）
    pub format: Option<Arc<str>>,
    pieces: Arc<Vec<Piece>>,
    /// 各区間の終わりの行（累積）
    ends: Arc<Vec<u64>>,
    delta: Arc<BTreeMap<u64, Value>>,
}

impl Column {
    pub fn new(name: &str) -> Column {
        Column {
            name: Arc::from(name),
            ..Column::default()
        }
    }

    /// 区間の並びから。
    pub fn from_pieces(name: &str, pieces: Vec<Piece>) -> Column {
        let mut c = Column::new(name);
        c.set_pieces(pieces);
        c
    }

    fn set_pieces(&mut self, pieces: Vec<Piece>) {
        let mut ends = Vec::with_capacity(pieces.len());
        let mut total = 0u64;
        for p in &pieces {
            total += p.len as u64;
            ends.push(total);
        }
        self.pieces = Arc::new(pieces);
        self.ends = Arc::new(ends);
    }

    /// 同じデータ（区間の並びと差分を共有している）か。索引を使い回すときに比べる。
    pub fn same_data(&self, o: &Column) -> bool {
        Arc::ptr_eq(&self.pieces, &o.pieces)
            && Arc::ptr_eq(&self.delta, &o.delta)
            && self.name == o.name
    }

    pub fn rows(&self) -> u64 {
        self.ends.last().copied().unwrap_or(0)
    }

    pub fn pieces(&self) -> &[Piece] {
        &self.pieces
    }

    pub fn delta(&self) -> &BTreeMap<u64, Value> {
        &self.delta
    }

    /// 差分を置き換える（ファイルを開いたとき）。
    pub fn set_delta(&mut self, delta: BTreeMap<u64, Value>) {
        self.delta = Arc::new(delta);
    }

    /// 行を含む区間の番号と、区間の中の位置。
    fn locate(&self, row: u64) -> Option<(usize, u32)> {
        if row >= self.rows() {
            return None;
        }
        let i = self.ends.partition_point(|&e| e <= row);
        let start = if i == 0 { 0 } else { self.ends[i - 1] };
        Some((i, (row - start) as u32))
    }

    /// 行の値（差分を先に見る）。
    pub fn get(&self, ctx: &Context, row: u64) -> io::Result<Value> {
        if let Some(v) = self.delta.get(&row) {
            return Ok(v.clone());
        }
        let Some((i, off)) = self.locate(row) else {
            return Ok(Value::Empty);
        };
        let p = &self.pieces[i];
        let d = p.chunk.data(ctx)?;
        Ok(d.get((p.start + off) as usize).to_value())
    }

    /// `rows` の範囲の値を順に渡す（チャンクごとに展開して読む。差分も反映する）。
    pub fn for_each(
        &self,
        ctx: &Context,
        rows: std::ops::Range<u64>,
        mut f: impl FnMut(u64, CellRef<'_>),
    ) -> io::Result<()> {
        let end = rows.end.min(self.rows());
        let mut row = rows.start;
        while row < end {
            let (i, off) = self.locate(row).expect("in range");
            let p = &self.pieces[i];
            let d = p.chunk.data(ctx)?;
            let n = ((p.len - off) as u64).min(end - row);
            for k in 0..n {
                let r = row + k;
                match self.delta.get(&r) {
                    Some(v) => f(r, CellRef::of(v)),
                    None => f(r, d.get((p.start + off) as usize + k as usize)),
                }
            }
            row += n;
        }
        Ok(())
    }

    /// `rows` の範囲を覆う、展開したチャンクの部分（チャンク・始まり・行数）。差分は含まない。
    pub fn segments(
        &self,
        ctx: &Context,
        rows: std::ops::Range<u64>,
    ) -> io::Result<Vec<(Arc<Data>, usize, usize)>> {
        let end = rows.end.min(self.rows());
        let mut out = Vec::new();
        let mut row = rows.start;
        while row < end {
            let (i, off) = self.locate(row).expect("in range");
            let p = &self.pieces[i];
            let n = ((p.len - off) as u64).min(end - row);
            out.push((p.chunk.data(ctx)?, (p.start + off) as usize, n as usize));
            row += n;
        }
        Ok(out)
    }

    /// 値を入れる（差分に入れ、多くなったらチャンクに書き戻す）。`row` は行数未満であること。
    pub fn set(&mut self, ctx: &Context, row: u64, v: Value) -> io::Result<()> {
        debug_assert!(row < self.rows());
        Arc::make_mut(&mut self.delta).insert(row, v);
        if self.delta.len() > DELTA_LIMIT {
            self.flush(ctx)?;
        }
        Ok(())
    }

    /// 差分をチャンクに書き戻す。
    pub fn flush(&mut self, ctx: &Context) -> io::Result<()> {
        if self.delta.is_empty() {
            return Ok(());
        }
        let delta = std::mem::take(Arc::make_mut(&mut self.delta));
        // 差分のある区間ごとに作り直す
        let mut pieces = (*self.pieces).clone();
        let mut by_piece: BTreeMap<usize, Vec<(u32, Value)>> = BTreeMap::new();
        for (row, v) in delta {
            if let Some((i, off)) = self.locate(row) {
                by_piece.entry(i).or_default().push((off, v));
            }
        }
        for (i, edits) in by_piece {
            let p = &pieces[i];
            let d = p.chunk.data(ctx)?;
            let mut b = Builder::default();
            let mut e = edits.iter().peekable();
            for k in 0..p.len {
                match e.peek() {
                    Some((off, v)) if *off == k => {
                        b.push(CellRef::of(v));
                        e.next();
                    }
                    _ => b.push(d.get((p.start + k) as usize)),
                }
            }
            let chunk = Chunk::create(ctx, b.finish())?;
            pieces[i] = Piece {
                len: chunk.rows,
                chunk,
                start: 0,
            };
        }
        self.set_pieces(pieces);
        Ok(())
    }

    /// 空のチャンク（`n` 行）の区間。
    fn empty_pieces(ctx: &Context, mut n: u64) -> io::Result<Vec<Piece>> {
        let mut out = Vec::new();
        while n > 0 {
            let k = n.min(MAX_ROWS as u64) as usize;
            let chunk = Chunk::create(ctx, Data::Empty(k))?;
            out.push(Piece {
                chunk,
                start: 0,
                len: k as u32,
            });
            n -= k as u64;
        }
        Ok(out)
    }

    /// 末尾に空の行を足す。
    pub fn extend_empty(&mut self, ctx: &Context, n: u64) -> io::Result<()> {
        if n == 0 {
            return Ok(());
        }
        let mut pieces = (*self.pieces).clone();
        pieces.extend(Column::empty_pieces(ctx, n)?);
        self.set_pieces(pieces);
        Ok(())
    }

    /// 区間を `row` で分けた並び（`row` が区間の境目になる）と、その位置の区間の番号。
    fn split_at(&self, row: u64) -> (Vec<Piece>, usize) {
        let mut pieces = (*self.pieces).clone();
        match self.locate(row) {
            None => {
                let n = pieces.len();
                (pieces, n)
            }
            Some((i, 0)) => (pieces, i),
            Some((i, off)) => {
                let p = pieces[i].clone();
                pieces[i].len = off;
                pieces.insert(
                    i + 1,
                    Piece {
                        chunk: p.chunk,
                        start: p.start + off,
                        len: p.len - off,
                    },
                );
                (pieces, i + 1)
            }
        }
    }

    /// `at` の位置に空の行を `n` 行挿入する。
    pub fn insert_rows(&mut self, ctx: &Context, at: u64, n: u64) -> io::Result<()> {
        if n == 0 {
            return Ok(());
        }
        let (mut pieces, i) = self.split_at(at.min(self.rows()));
        let new = Column::empty_pieces(ctx, n)?;
        pieces.splice(i..i, new);
        self.set_pieces(pieces);
        // 差分の行をずらす
        if self.delta.range(at..).next().is_some() {
            let d = std::mem::take(Arc::make_mut(&mut self.delta));
            let shifted = d
                .into_iter()
                .map(|(r, v)| (if r >= at { r + n } else { r }, v))
                .collect();
            self.delta = Arc::new(shifted);
        }
        self.compact_if_fragmented(ctx)
    }

    /// `at` から `n` 行を削除する。
    pub fn delete_rows(&mut self, ctx: &Context, at: u64, n: u64) -> io::Result<()> {
        let end = (at + n).min(self.rows());
        if at >= end {
            return Ok(());
        }
        let (pieces, i) = self.split_at(at);
        let tmp = {
            let mut c = self.clone();
            c.set_pieces(pieces);
            c
        };
        let (mut pieces, j) = tmp.split_at(end);
        pieces.drain(i..j);
        self.set_pieces(pieces);
        let n = end - at;
        if self.delta.range(at..).next().is_some() {
            let d = std::mem::take(Arc::make_mut(&mut self.delta));
            let shifted = d
                .into_iter()
                .filter(|(r, _)| *r < at || *r >= end)
                .map(|(r, v)| (if r >= end { r - n } else { r }, v))
                .collect();
            self.delta = Arc::new(shifted);
        }
        self.compact_if_fragmented(ctx)
    }

    /// 行を `order` の順に並べ替えた列（新しいチャンクに書く）。`order[i]` は新しい `i` 行目の元の行。
    pub fn permuted(&self, ctx: &Context, order: &[u32]) -> io::Result<Column> {
        use rayon::prelude::*;
        let mut me = self.clone();
        me.flush(ctx)?;
        let datas: Vec<Arc<Data>> = me
            .pieces
            .par_iter()
            .map(|p| p.chunk.data(ctx))
            .collect::<io::Result<_>>()?;
        let pieces: Vec<Piece> = order
            .par_chunks(MAX_ROWS)
            .map(|block| {
                let mut b = Builder::default();
                for &r in block {
                    match me.locate(r as u64) {
                        Some((i, off)) => b.push(datas[i].get((me.pieces[i].start + off) as usize)),
                        None => b.push(CellRef::Empty),
                    }
                }
                let c = Chunk::create(ctx, b.finish())?;
                Ok(Piece {
                    len: c.rows,
                    chunk: c,
                    start: 0,
                })
            })
            .collect::<io::Result<_>>()?;
        let mut out = Column::from_pieces(&self.name, pieces);
        out.format = self.format.clone();
        Ok(out)
    }

    /// 小さな区間が増えすぎたら、隣どうしを合わせたチャンクに作り直す。
    fn compact_if_fragmented(&mut self, ctx: &Context) -> io::Result<()> {
        let ideal = self.rows().div_ceil(MAX_ROWS as u64) as usize;
        if self.pieces.len() <= ideal * 2 + 16 {
            return Ok(());
        }
        self.compact(ctx)
    }

    /// 区間を詰め直す（チャンクを `MAX_ROWS` 行ずつに作り直す）。
    pub fn compact(&mut self, ctx: &Context) -> io::Result<()> {
        self.flush(ctx)?;
        let rows = self.rows();
        let mut out = Vec::new();
        let mut b = Builder::default();
        let mut err = None;
        let me = self.clone();
        me.for_each(ctx, 0..rows, |_, v| {
            b.push(v);
            if b.len() == MAX_ROWS {
                match Chunk::create(ctx, std::mem::take(&mut b).finish()) {
                    Ok(c) => out.push(Piece {
                        len: c.rows,
                        chunk: c,
                        start: 0,
                    }),
                    Err(e) => err = Some(e),
                }
            }
        })?;
        if let Some(e) = err {
            return Err(e);
        }
        if !b.is_empty() {
            let c = Chunk::create(ctx, b.finish())?;
            out.push(Piece {
                len: c.rows,
                chunk: c,
                start: 0,
            });
        }
        self.set_pieces(out);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(ctx: &Context, vals: &[f64], chunk: usize) -> Column {
        let mut pieces = Vec::new();
        for c in vals.chunks(chunk) {
            let d = Data::from_values(c.iter().map(|&v| CellRef::Number(v)));
            let ch = Chunk::create(ctx, d).unwrap();
            pieces.push(Piece {
                len: ch.rows,
                chunk: ch,
                start: 0,
            });
        }
        Column::from_pieces("x", pieces)
    }

    fn all(ctx: &Context, c: &Column) -> Vec<Value> {
        let mut v = Vec::new();
        c.for_each(ctx, 0..c.rows(), |_, x| v.push(x.to_value()))
            .unwrap();
        v
    }

    fn nums(v: &[f64]) -> Vec<Value> {
        v.iter().map(|&x| Value::Number(x)).collect()
    }

    #[test]
    fn edits_inserts_and_deletes() {
        let ctx = Context::for_tests();
        let base: Vec<f64> = (0..10).map(|i| i as f64).collect();
        let mut c = col(&ctx, &base, 4);
        assert_eq!(c.rows(), 10);
        assert_eq!(c.get(&ctx, 5).unwrap(), Value::Number(5.0));
        let snap = c.clone();
        c.set(&ctx, 5, "x".into()).unwrap();
        assert_eq!(c.get(&ctx, 5).unwrap(), Value::text("x"));
        assert_eq!(snap.get(&ctx, 5).unwrap(), Value::Number(5.0));
        c.insert_rows(&ctx, 2, 2).unwrap();
        let mut want = nums(&[0.0, 1.0]);
        want.extend([Value::Empty, Value::Empty]);
        want.extend(nums(&[2.0, 3.0, 4.0]));
        want.push(Value::text("x"));
        want.extend(nums(&[6.0, 7.0, 8.0, 9.0]));
        assert_eq!(all(&ctx, &c), want);
        c.delete_rows(&ctx, 1, 4).unwrap();
        let mut want = nums(&[0.0, 3.0, 4.0]);
        want.push(Value::text("x"));
        want.extend(nums(&[6.0, 7.0, 8.0, 9.0]));
        assert_eq!(all(&ctx, &c), want);
        c.flush(&ctx).unwrap();
        assert!(c.delta().is_empty());
        assert_eq!(all(&ctx, &c), want);
        c.compact(&ctx).unwrap();
        assert_eq!(c.pieces().len(), 1);
        assert_eq!(all(&ctx, &c), want);
        // 末尾への挿入・範囲外の削除
        c.insert_rows(&ctx, c.rows(), 1).unwrap();
        assert_eq!(c.rows(), 9);
        c.delete_rows(&ctx, 8, 100).unwrap();
        assert_eq!(c.rows(), 8);
    }

    #[test]
    fn many_edits_flush_to_chunks() {
        let ctx = Context::for_tests();
        let base: Vec<f64> = (0..20_000).map(|i| i as f64).collect();
        let mut c = col(&ctx, &base, MAX_ROWS);
        for r in 0..(DELTA_LIMIT as u64 + 10) {
            c.set(&ctx, r * 2, Value::Number(-1.0)).unwrap();
        }
        assert!(c.delta().len() < DELTA_LIMIT);
        for r in 0..(DELTA_LIMIT as u64 + 10) {
            assert_eq!(c.get(&ctx, r * 2).unwrap(), Value::Number(-1.0));
            assert_eq!(
                c.get(&ctx, r * 2 + 1).unwrap(),
                Value::Number((r * 2 + 1) as f64)
            );
        }
    }
}

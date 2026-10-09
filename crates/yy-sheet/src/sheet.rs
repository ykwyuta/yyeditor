//! ブック・シート・表（15 章 3.1・3.5）。
//!
//! シートは大量のデータの「表」（列指向。A1 から始まり、見出し行を持てる）と、表の外の「自由な
//! セル」（疎な表）からなる。利用者からは 1 枚の格子に見える。
//!
//! ブックの複製（スナップショット）は O(1) で、Undo は操作の前のブックを積むだけ。

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use yy_numfmt::DateSystem;

use crate::Context;
use crate::column::Column;
use crate::formula::{Formula, Formulas};
use crate::query::{ColFilter, SortKey};
use crate::store::Store;
use crate::style::{Style, Styles};
use crate::value::Value;

/// 表。
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub columns: Arc<Vec<Column>>,
    /// データの行数（見出し行を除く）
    pub rows: u64,
    /// 1 行目が見出し（列の名前）
    pub header: bool,
}

impl Table {
    pub fn cols(&self) -> u32 {
        self.columns.len() as u32
    }

    /// 格子の上での行数（見出し行を含む）。
    pub fn grid_rows(&self) -> u64 {
        if self.columns.is_empty() {
            0
        } else {
            self.rows + self.header as u64
        }
    }
}

/// 格子のセルが表のどこに当たるか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// 見出し行の列
    Header(u32),
    /// 表のデータ（表の行・列）
    Data(u64, u32),
    /// 表の外
    Free,
}

/// 絞り込みと並べ替えの表示（15 章 9.1）。表のデータは動かさず、表示する表の行の並びだけを持つ。
#[derive(Clone, Debug, Default)]
pub struct View {
    /// 絞り込みの段階（付けた順。すべてを満たす行を表示する）
    pub filters: Vec<ColFilter>,
    /// 並べ替えのキー（先のキーが優先）
    pub sort: Vec<SortKey>,
    /// 表示する表の行（表示の順）。`None` なら全行を元の順に
    pub rows: Option<Arc<Vec<u32>>>,
    /// 各段階のあとの残りの行数
    pub counts: Vec<u64>,
}

impl View {
    /// 条件もキーもない。
    pub fn is_empty(&self) -> bool {
        self.filters.is_empty() && self.sort.is_empty()
    }

    /// 列 `col` の絞り込みの条件。
    pub fn filter_of(&self, col: u32) -> Option<&ColFilter> {
        self.filters.iter().find(|f| f.col == col)
    }

    /// 列の並びが変わったとき、条件とキーの列を付け替える（`None` を返す列の条件は外す）。
    pub fn remap_cols(&mut self, map: impl Fn(u32) -> Option<u32>) {
        self.filters.retain_mut(|f| match map(f.col) {
            Some(c) => {
                f.col = c;
                true
            }
            None => false,
        });
        self.sort.retain_mut(|k| match map(k.col) {
            Some(c) => {
                k.col = c;
                true
            }
            None => false,
        });
    }
}

/// シート。
#[derive(Clone, Debug, Default)]
pub struct Sheet {
    pub name: Arc<str>,
    pub table: Table,
    /// 表の外のセル（格子の行・列 → 値）
    pub cells: Arc<BTreeMap<(u64, u32), Value>>,
    /// 列幅（文字数。書いていない列は既定）
    pub col_widths: Arc<BTreeMap<u32, f32>>,
    /// 固定する行・列の数
    pub frozen: (u32, u32),
    /// 絞り込み・並べ替え
    pub view: View,
    /// セルの書式（範囲の層）
    pub styles: Styles,
    /// 数式とその結果
    pub formulas: Formulas,
    /// 固定長ファイルの設定（コピーブックのレイアウト・文字コード）
    pub fixed: Option<Arc<crate::fixed::FixedSpec>>,
}

impl Sheet {
    pub fn new(name: &str) -> Sheet {
        Sheet {
            name: Arc::from(name),
            ..Sheet::default()
        }
    }

    /// 表示している表の行数（絞り込み中なら残った行の数）。
    pub fn visible_rows(&self) -> u64 {
        match &self.view.rows {
            Some(r) => r.len() as u64,
            None => self.table.rows,
        }
    }

    /// 格子の上での表の行数（見出し行を含む。絞り込み中なら残った行だけ）。
    pub fn table_grid_rows(&self) -> u64 {
        let t = &self.table;
        if t.columns.is_empty() {
            0
        } else {
            self.visible_rows() + t.header as u64
        }
    }

    /// 絞り込みで隠れている表の行の数。
    fn hidden_rows(&self) -> u64 {
        self.table.rows - self.visible_rows()
    }

    /// 格子の行 → 表の外のセルを置く行（表より下は隠れた行の分だけずれる）。
    fn free_row(&self, row: u64) -> u64 {
        if !self.table.columns.is_empty() && row >= self.table_grid_rows() {
            row + self.hidden_rows()
        } else {
            row
        }
    }

    /// 表の外のセルを置く行 → 格子の行（隠れた行の中なら `None`）。
    fn grid_row_of_free(&self, row: u64) -> Option<u64> {
        let end = self.table_grid_rows();
        if self.table.columns.is_empty() || row < end {
            Some(row)
        } else if row < self.table.grid_rows() {
            None
        } else {
            Some(row - self.hidden_rows())
        }
    }

    /// 格子のセルの書式。
    pub fn style_at(&self, row: u64, col: u32) -> Style {
        if self.styles.is_empty() {
            return Style::default();
        }
        self.styles.at(self.source_row(row), col)
    }

    /// 格子のセルの表示形式（セルの書式、なければ表の列の既定の形式）。
    pub fn format_at(&self, row: u64, col: u32) -> Option<Arc<str>> {
        if !self.styles.is_empty()
            && let Some(f) = self.styles.at(self.source_row(row), col).num_fmt
        {
            return Some(f);
        }
        match self.place(row, col) {
            Place::Data(_, c) => self.table.columns[c as usize].format.clone(),
            _ => None,
        }
    }

    /// 格子の行の、絞り込み・並べ替えをしないときの行（行番号の表示・セルの名前に使う）。
    pub fn source_row(&self, row: u64) -> u64 {
        match self.place(row, 0) {
            Place::Data(r, _) if self.view.rows.is_some() => r + self.table.header as u64,
            Place::Free => self.free_row(row),
            _ => row,
        }
    }

    /// 絞り込み・並べ替えをしないときの格子の位置が、表のどこに当たるか。
    pub fn place_source(&self, row: u64, col: u32) -> Place {
        let t = &self.table;
        if col >= t.cols() || row >= t.grid_rows() {
            return Place::Free;
        }
        if t.header {
            if row == 0 {
                Place::Header(col)
            } else {
                Place::Data(row - 1, col)
            }
        } else {
            Place::Data(row, col)
        }
    }

    /// 絞り込み・並べ替えをしないときの使っている範囲（行数・列数。式の結果を含む）。
    pub fn source_extent(&self) -> (u64, u32) {
        let mut rows = self.table.grid_rows();
        let mut cols = self.table.cols();
        for &(r, c) in self.cells.keys() {
            rows = rows.max(r + 1);
            cols = cols.max(c + 1);
        }
        for &(r, c) in self.formulas.cells.keys() {
            rows = rows.max(r + 1);
            cols = cols.max(c + 1);
        }
        for &(c, r) in self.formulas.results.keys() {
            rows = rows.max(r + 1);
            cols = cols.max(c + 1);
        }
        for sh in self.formulas.shared.iter() {
            rows = rows.max(sh.r1 + 1);
            cols = cols.max(sh.col + 1);
        }
        (rows, cols)
    }

    /// 絞り込み・並べ替えをしないときの、表の列（固定長ではレコードの列）に何かある行数。表の右の列
    /// （固定長の範囲の外。`CBL.MOVE` の式などを書く）だけに値・式がある行は数えない。
    pub fn record_rows(&self) -> u64 {
        let cols = self.table.cols();
        let mut rows = self.table.grid_rows();
        let keys = self
            .cells
            .keys()
            .copied()
            .chain(self.formulas.cells.keys().copied())
            .chain(self.formulas.results.keys().map(|&(c, r)| (r, c)));
        for (r, c) in keys {
            if c < cols {
                rows = rows.max(r + 1);
            }
        }
        for sh in self.formulas.shared.iter() {
            if sh.col < cols {
                rows = rows.max(sh.r1 + 1);
            }
        }
        rows
    }

    /// 格子のセルの式（共有式なら、その行の式）。
    pub fn formula_at(&self, row: u64, col: u32) -> Option<Formula> {
        if self.formulas.cells.is_empty() && self.formulas.shared.is_empty() {
            return None;
        }
        let src = self.source_row(row);
        if let Some(f) = self.formulas.cells.get(&(src, col)) {
            return Some(f.clone());
        }
        let i = self.formulas.shared_at(src, col)?;
        Some(self.formulas.shared[i].formula_at(src))
    }

    /// `top` 行の `c0..=c1` 列を `bottom` 行まで下へコピーする（Excel の Ctrl+D）。式は行ごとに相対参照を
    /// ずらし、行が多ければ共有式にする（1 つの式で持ち、まとめて計算する）。絞り込みをしないときの
    /// 格子の行で指定する。
    pub fn fill_down(
        &mut self,
        ctx: &Context,
        top: u64,
        bottom: u64,
        c0: u32,
        c1: u32,
    ) -> Result<(), String> {
        if bottom <= top {
            return Ok(());
        }
        let count = bottom - top + 1;
        for col in c0..=c1 {
            match self.formula_at(top, col) {
                Some(f) => {
                    if matches!(self.place_source(top, col), Place::Header(_)) {
                        continue;
                    }
                    // 自分の範囲を参照する式（前の行を足していくなど）は共有式にしない
                    let own = yy_formula::Area {
                        r0: top,
                        c0: col,
                        r1: bottom,
                        c1: col,
                        ..yy_formula::Area::cell(top, col)
                    };
                    let mut rs = Vec::new();
                    collect_refs(&f.expr, &mut rs);
                    let name = self.name.clone();
                    let self_ref = rs.iter().any(|(sheet, a)| {
                        sheet
                            .as_deref()
                            .is_none_or(|n| yy_formula::eq_text(n, &name))
                            && yy_formula::spread(a, count).intersects(&own)
                    });
                    // CBL.MOVE は受け取り範囲に値を置くので、共有式にしない（行ごとの式に）
                    if count >= SHARED_MIN && !self_ref && !crate::formula::contains_move(&f.expr) {
                        self.clear_range(ctx, top + 1, bottom, col)
                            .map_err(|e| e.to_string())?;
                        self.formulas.add_shared(crate::shared::Shared {
                            col,
                            r0: top,
                            r1: bottom,
                            formula: f,
                            results: None,
                        });
                    } else if count <= INDIVIDUAL_MAX {
                        for i in 1..count {
                            let e = yy_formula::shift(&f.expr, i as i64);
                            let text = yy_formula::formula_text(&e);
                            self.set_formula_source(ctx, top + i, col, &text)?;
                        }
                    } else {
                        return Err(format!(
                            "自分の列を参照する式は {} 行までしか下へコピーできません",
                            INDIVIDUAL_MAX
                        ));
                    }
                }
                None => {
                    let v = self.get_source(ctx, top, col).map_err(|e| e.to_string())?;
                    for r in top + 1..=bottom {
                        self.set_source(ctx, r, col, v.clone())
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
        }
        Ok(())
    }

    /// 1 列の `r0..=r1` 行（絞り込みをしないときの格子の行）の値を消す（表の列は区間の付け替えで）。
    fn clear_range(&mut self, ctx: &Context, r0: u64, r1: u64, col: u32) -> io::Result<()> {
        let t = &self.table;
        let head = t.header as u64;
        if col < t.cols() && r0 < t.grid_rows() {
            let lo = r0.max(head) - head;
            let hi = r1.min(t.grid_rows() - 1).saturating_sub(head);
            if lo <= hi && r1 >= head {
                let c = &mut Arc::make_mut(&mut self.table.columns)[col as usize];
                c.delete_rows(ctx, lo, hi - lo + 1)?;
                c.insert_rows(ctx, lo, hi - lo + 1)?;
            }
        }
        let gone: Vec<(u64, u32)> = self
            .cells
            .range((r0, 0)..=(r1, u32::MAX))
            .filter(|(k, _)| k.1 == col)
            .map(|(k, _)| *k)
            .collect();
        if !gone.is_empty() {
            let cells = Arc::make_mut(&mut self.cells);
            for k in gone {
                cells.remove(&k);
            }
        }
        self.formulas.touch(r0, col, r1, col);
        Ok(())
    }

    /// 格子の範囲（行 `t..=b`・列 `l..=r`）の集計（ステータスバー）。表の列は区間ごとに並列に読む。
    /// 絞り込み・並べ替えの表示中は `None`（呼ぶ側でセルを数える）。
    pub fn totals(
        &self,
        ctx: &Context,
        t: u64,
        l: u32,
        b: u64,
        r: u32,
    ) -> io::Result<Option<crate::bulk::Totals>> {
        if self.view.rows.is_some() {
            return Ok(None);
        }
        let mut tot = crate::bulk::Totals::default();
        let tb = &self.table;
        let head = tb.header as u64;
        for c in l..=r {
            // 共有式の範囲は結果の列から
            let shared: Vec<&crate::shared::Shared> = self
                .formulas
                .shared
                .iter()
                .filter(|s| s.col == c && s.r1 >= t && s.r0 <= b)
                .collect();
            for s in &shared {
                if let Some(res) = &s.results {
                    let lo = t.max(s.r0) - s.r0;
                    let hi = b.min(s.r1) - s.r0 + 1;
                    let x = crate::bulk::totals(ctx, res, lo..hi)?;
                    tot.count += x.count;
                    tot.numbers += x.numbers;
                    tot.sum += x.sum;
                }
            }
            let in_shared = |row: u64| shared.iter().any(|s| (s.r0..=s.r1).contains(&row));
            for (&(_, row), v) in self.formulas.results.range((c, t)..=(c, b)) {
                if !in_shared(row) {
                    tot.add_value(v);
                }
            }
            let is_result = |row: u64| self.formulas.results.contains_key(&(c, row));
            if c < tb.cols() && t < tb.grid_rows() {
                let col = &tb.columns[c as usize];
                if tb.header && t == 0 && !is_result(0) && !in_shared(0) {
                    tot.count += 1;
                }
                let lo = t.max(head) - head;
                let hi = (b.min(tb.grid_rows() - 1) + 1).saturating_sub(head);
                if lo < hi {
                    let mut x = crate::bulk::totals(ctx, col, lo..hi)?;
                    // 結果・共有式に隠れたセルの分を引く
                    for (&(_, row), _) in
                        self.formulas.results.range((c, lo + head)..(c, hi + head))
                    {
                        x.add(crate::chunk::CellRef::of(&col.get(ctx, row - head)?), -1);
                    }
                    for s in &shared {
                        for row in s.r0.max(lo + head)..=s.r1.min(hi + head - 1) {
                            if !is_result(row) {
                                x.add(crate::chunk::CellRef::of(&col.get(ctx, row - head)?), -1);
                            }
                        }
                    }
                    tot.count = tot.count.wrapping_add(x.count);
                    tot.numbers = tot.numbers.wrapping_add(x.numbers);
                    tot.sum += x.sum;
                }
            }
            // 表の外の自由なセル
            for (&(row, cc), v) in self.cells.range((t, 0)..=(b, u32::MAX)) {
                if cc == c
                    && !is_result(row)
                    && !in_shared(row)
                    && self.place_source(row, c) == Place::Free
                {
                    tot.add_value(v);
                }
            }
        }
        Ok(Some(tot))
    }

    /// 絞り込みをしないときの格子の位置の値。
    pub fn get_source(&self, ctx: &Context, row: u64, col: u32) -> io::Result<Value> {
        if let Some(v) = self.formulas.result(row, col) {
            return Ok(v.clone());
        }
        if let Some(i) = self.formulas.shared_at(row, col) {
            let sh = &self.formulas.shared[i];
            return Ok(match &sh.results {
                Some(c) => c.get(ctx, row - sh.r0)?,
                None => Value::Empty,
            });
        }
        Ok(match self.place_source(row, col) {
            Place::Header(c) => Value::Text(self.table.columns[c as usize].name.clone()),
            Place::Data(r, c) => self.table.columns[c as usize].get(ctx, r)?,
            Place::Free => self.cells.get(&(row, col)).cloned().unwrap_or_default(),
        })
    }

    /// 絞り込みをしないときの格子の位置に値を入れる。
    fn set_source(&mut self, ctx: &Context, row: u64, col: u32, v: Value) -> io::Result<()> {
        let saved = std::mem::take(&mut self.view.rows);
        let r = self.set(ctx, row, col, v);
        self.view.rows = saved;
        r
    }

    /// 絞り込みをしないときの格子の位置に式を入れる。
    fn set_formula_source(
        &mut self,
        ctx: &Context,
        row: u64,
        col: u32,
        text: &str,
    ) -> Result<(), String> {
        let saved = std::mem::take(&mut self.view.rows);
        let r = self.set_formula(ctx, row, col, text);
        self.view.rows = saved;
        r
    }

    /// 格子のセルに式を入れる（値は消す。計算は [`crate::formula::recalc`]）。
    pub fn set_formula(
        &mut self,
        ctx: &Context,
        row: u64,
        col: u32,
        text: &str,
    ) -> Result<(), String> {
        let f = Formula::parse(text)?;
        if matches!(self.place(row, col), Place::Header(_)) {
            return Err("表の見出しには式を入れられません".into());
        }
        self.set(ctx, row, col, Value::Empty)
            .map_err(|e| e.to_string())?;
        let key = (self.source_row(row), col);
        Arc::make_mut(&mut self.formulas.cells).insert(key, f);
        self.formulas.touch(key.0, col, key.0, col);
        Ok(())
    }

    pub fn place(&self, row: u64, col: u32) -> Place {
        let t = &self.table;
        if col >= t.cols() || row >= self.table_grid_rows() {
            return Place::Free;
        }
        let r = if t.header {
            if row == 0 {
                return Place::Header(col);
            }
            row - 1
        } else {
            row
        };
        match &self.view.rows {
            Some(v) => Place::Data(v[r as usize] as u64, col),
            None => Place::Data(r, col),
        }
    }

    /// 格子の行に当たる表の行（表の外なら `None`）。
    pub fn table_row(&self, row: u64) -> Option<u64> {
        match self.place(row, 0) {
            Place::Data(r, _) => Some(r),
            _ => None,
        }
    }

    pub fn get(&self, ctx: &Context, row: u64, col: u32) -> io::Result<Value> {
        if !self.formulas.results.is_empty()
            && let Some(v) = self.formulas.result(self.source_row(row), col)
        {
            return Ok(v.clone());
        }
        if !self.formulas.shared.is_empty() {
            let src = self.source_row(row);
            if let Some(i) = self.formulas.shared_at(src, col) {
                let sh = &self.formulas.shared[i];
                return Ok(match &sh.results {
                    Some(c) => c.get(ctx, src - sh.r0)?,
                    None => Value::Empty,
                });
            }
        }
        Ok(match self.place(row, col) {
            Place::Header(c) => Value::Text(self.table.columns[c as usize].name.clone()),
            Place::Data(r, c) => self.table.columns[c as usize].get(ctx, r)?,
            Place::Free => self
                .cells
                .get(&(self.free_row(row), col))
                .cloned()
                .unwrap_or_default(),
        })
    }

    pub fn set(&mut self, ctx: &Context, row: u64, col: u32, v: Value) -> io::Result<()> {
        let src = self.source_row(row);
        if !self.formulas.cells.is_empty() {
            self.formulas.remove(src, col);
        }
        if !self.formulas.shared.is_empty() {
            self.formulas.split_shared(src, col);
        }
        self.formulas.touch(src, col, src, col);
        match self.place(row, col) {
            Place::Header(c) => {
                let cols = Arc::make_mut(&mut self.table.columns);
                cols[c as usize].name = Arc::from(v.general_text().as_str());
            }
            Place::Data(r, c) => {
                let cols = Arc::make_mut(&mut self.table.columns);
                cols[c as usize].set(ctx, r, v)?;
            }
            Place::Free => {
                let row = self.free_row(row);
                let cells = Arc::make_mut(&mut self.cells);
                if v.is_empty() {
                    cells.remove(&(row, col));
                } else {
                    cells.insert((row, col), v);
                }
            }
        }
        Ok(())
    }

    /// 使っている範囲（格子の行数・列数。絞り込み中なら見えている範囲）。
    pub fn extent(&self) -> (u64, u32) {
        let mut rows = self.table_grid_rows();
        let mut cols = self.table.cols();
        let keys = self
            .cells
            .keys()
            .copied()
            .chain(self.formulas.cells.keys().copied())
            .chain(self.formulas.results.keys().map(|&(c, r)| (r, c)))
            .chain(self.formulas.shared.iter().map(|s| (s.r1, s.col)));
        for (r, c) in keys {
            if let Some(g) = self.grid_row_of_free(r) {
                rows = rows.max(g + 1);
            }
            cols = cols.max(c + 1);
        }
        (rows, cols)
    }

    /// 格子の `at` 行目に `n` 行挿入する（表の中なら表に、表の外の自由なセルはずらす）。
    pub fn insert_rows(&mut self, ctx: &Context, at: u64, n: u64) -> io::Result<()> {
        self.view.rows = None;
        let t = &self.table;
        let first = t.header as u64;
        if !t.columns.is_empty() && at >= first && at <= t.grid_rows() {
            let tr = at - first;
            let cols = Arc::make_mut(&mut self.table.columns);
            for c in cols.iter_mut() {
                c.insert_rows(ctx, tr, n)?;
            }
            self.table.rows += n;
        }
        self.shift_cells(|r, c| (if r >= at { r + n } else { r }, c), |_, _| true);
        self.styles.insert_rows(at, n);
        self.formulas.shift(yy_formula::Edit::InsertRows(at, n));
        Ok(())
    }

    /// 格子の `at` 行目から `n` 行削除する。
    pub fn delete_rows(&mut self, ctx: &Context, at: u64, n: u64) -> io::Result<()> {
        self.view.rows = None;
        let t = &self.table;
        let first = t.header as u64;
        let end = at + n;
        if !t.columns.is_empty() && end > first && at < t.grid_rows() {
            let tr = at.max(first) - first;
            let te = (end.min(t.grid_rows())) - first;
            let cols = Arc::make_mut(&mut self.table.columns);
            for c in cols.iter_mut() {
                c.delete_rows(ctx, tr, te - tr)?;
            }
            self.table.rows -= te - tr;
        }
        self.shift_cells(
            |r, c| (if r >= end { r - n } else { r }, c),
            |r, _| r < at || r >= end,
        );
        self.styles.delete_rows(at, n);
        self.formulas.shift(yy_formula::Edit::DeleteRows(at, n));
        Ok(())
    }

    /// 格子の `at` 列目に `n` 列挿入する。
    pub fn insert_cols(&mut self, ctx: &Context, at: u32, n: u32) -> io::Result<()> {
        let t = &self.table;
        if !t.columns.is_empty() && at <= t.cols() {
            let rows = t.rows;
            let cols = Arc::make_mut(&mut self.table.columns);
            for i in 0..n {
                let mut c = Column::new(&crate::col_name(at + i));
                c.extend_empty(ctx, rows)?;
                cols.insert((at + i) as usize, c);
            }
        }
        self.shift_cells(|r, c| (r, if c >= at { c + n } else { c }), |_, _| true);
        self.view
            .remap_cols(|c| Some(if c >= at { c + n } else { c }));
        self.styles.insert_cols(at, n);
        self.formulas.shift(yy_formula::Edit::InsertCols(at, n));
        let w = std::mem::take(Arc::make_mut(&mut self.col_widths));
        self.col_widths = Arc::new(
            w.into_iter()
                .map(|(c, x)| (if c >= at { c + n } else { c }, x))
                .collect(),
        );
        Ok(())
    }

    /// 格子の `at` 列目から `n` 列削除する。
    pub fn delete_cols(&mut self, at: u32, n: u32) {
        let end = at + n;
        let t = &self.table;
        if at < t.cols() {
            let e = end.min(t.cols());
            let cols = Arc::make_mut(&mut self.table.columns);
            cols.drain(at as usize..e as usize);
            if cols.is_empty() {
                self.table.rows = 0;
            }
        }
        self.shift_cells(
            |r, c| (r, if c >= end { c - n } else { c }),
            |_, c| c < at || c >= end,
        );
        self.view.remap_cols(|c| {
            if c < at {
                Some(c)
            } else if c >= end {
                Some(c - n)
            } else {
                None
            }
        });
        if self.table.columns.is_empty() {
            self.view = View::default();
        }
        self.styles.delete_cols(at, n);
        self.formulas.shift(yy_formula::Edit::DeleteCols(at, n));
        let w = std::mem::take(Arc::make_mut(&mut self.col_widths));
        self.col_widths = Arc::new(
            w.into_iter()
                .filter(|(c, _)| *c < at || *c >= end)
                .map(|(c, x)| (if c >= end { c - n } else { c }, x))
                .collect(),
        );
    }

    /// 絞り込み・並べ替えに使う表（15 章 9）。表のデータのセルに数式があれば、その結果の値に置き換えた
    /// 写し（列は必要なものだけ写す）。数式がなければ表そのもの。
    pub fn query_table(&self, ctx: &Context) -> io::Result<Table> {
        let t = &self.table;
        if t.columns.is_empty() || self.formulas.is_empty() {
            return Ok(t.clone());
        }
        let header = t.header as u64;
        // 表のデータの行（絞り込みをしないときの格子の行）
        let (first, end) = (header, header + t.rows);
        let mut cols: Option<Vec<Column>> = None;
        let mut touched: Vec<u32> = Vec::new();
        let mut put = |c: u32, r: u64, v: Value| -> io::Result<()> {
            let cols = cols.get_or_insert_with(|| (*t.columns).clone());
            if !touched.contains(&c) {
                touched.push(c);
            }
            cols[c as usize].set(ctx, r - first, v)
        };
        for c in 0..t.cols() {
            for (&(_, r), v) in self.formulas.results.range((c, first)..(c, end)) {
                put(c, r, v.clone())?;
            }
        }
        for sh in self.formulas.shared.iter() {
            let Some(res) = &sh.results else {
                continue;
            };
            if sh.col >= t.cols() {
                continue;
            }
            let (a, b) = (sh.r0.max(first), sh.r1.min(end.saturating_sub(1)));
            for r in a..=b {
                put(sh.col, r, res.get(ctx, r - sh.r0)?)?;
            }
        }
        let Some(mut cols) = cols else {
            return Ok(t.clone());
        };
        for c in touched {
            cols[c as usize].flush(ctx)?;
        }
        Ok(Table {
            columns: Arc::new(cols),
            rows: t.rows,
            header: t.header,
        })
    }

    /// 表の外の自由なセル（手で入れた・貼り付けたデータ）を表に取り込む（15 章 9。絞り込み・並べ替えは
    /// 表の列に対して行うため）。取り込んだら `true`。
    ///
    /// * 表がなければ: 1 行目を見出しにして、使っている範囲（A1 から）を表にする。1 行目が空なら取り込まない。
    /// * 表があれば: 表の右の `col` 列目までの、表の行の範囲にある自由なセルを、表の列として足す
    ///   （絞り込み・並べ替えの最中は行がずれるので取り込まない）。
    pub fn absorb_free_cells(&mut self, ctx: &Context, col: u32) -> io::Result<bool> {
        if self.table.columns.is_empty() {
            let (rows, cols) = self.extent();
            if rows < 2 || !self.cells.range((0, 0)..(1, 0)).any(|_| true) {
                return Ok(false);
            }
            let data_rows = rows - 1;
            let cells = Arc::make_mut(&mut self.cells);
            let mut columns = Vec::with_capacity(cols as usize);
            for c in 0..cols {
                let name = cells
                    .remove(&(0, c))
                    .map(|v| v.general_text())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| crate::col_name(c));
                let mut column = Column::new(&name);
                column.extend_empty(ctx, data_rows)?;
                let keys: Vec<(u64, u32)> = cells
                    .range((1, 0)..(rows, 0))
                    .filter(|((_, cc), _)| *cc == c)
                    .map(|(k, _)| *k)
                    .collect();
                for k in keys {
                    if let Some(v) = cells.remove(&k) {
                        column.set(ctx, k.0 - 1, v)?;
                    }
                }
                column.flush(ctx)?;
                columns.push(column);
            }
            self.table = Table {
                columns: Arc::new(columns),
                rows: data_rows,
                header: true,
            };
            self.view = View::default();
            return Ok(true);
        }
        let start = self.table.cols();
        if col < start || !self.view.is_empty() {
            return Ok(false);
        }
        let header = self.table.header as u64;
        let grid = self.table.grid_rows();
        let rows = self.table.rows;
        let has = self
            .cells
            .range((0, 0)..(grid, 0))
            .any(|((_, c), _)| *c >= start && *c <= col);
        if !has {
            return Ok(false);
        }
        let cells = Arc::make_mut(&mut self.cells);
        let mut added = Vec::new();
        for c in start..=col {
            let name = if header == 1 {
                cells
                    .remove(&(0, c))
                    .map(|v| v.general_text())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| crate::col_name(c))
            } else {
                crate::col_name(c)
            };
            let mut column = Column::new(&name);
            column.extend_empty(ctx, rows)?;
            let keys: Vec<(u64, u32)> = cells
                .range((header, 0)..(grid, 0))
                .filter(|((_, cc), _)| *cc == c)
                .map(|(k, _)| *k)
                .collect();
            for k in keys {
                if let Some(v) = cells.remove(&k) {
                    column.set(ctx, k.0 - header, v)?;
                }
            }
            column.flush(ctx)?;
            added.push(column);
        }
        Arc::make_mut(&mut self.table.columns).extend(added);
        Ok(true)
    }

    fn shift_cells(
        &mut self,
        map: impl Fn(u64, u32) -> (u64, u32),
        keep: impl Fn(u64, u32) -> bool,
    ) {
        if self.cells.is_empty() {
            return;
        }
        let cells = std::mem::take(Arc::make_mut(&mut self.cells));
        let moved = cells
            .into_iter()
            .filter(|((r, c), _)| keep(*r, *c))
            .map(|((r, c), v)| (map(r, c), v))
            .collect();
        self.cells = Arc::new(moved);
    }
}

/// ブック。
#[derive(Clone, Debug)]
pub struct Workbook {
    pub sheets: Vec<Sheet>,
    pub date_system: DateSystem,
}

/// 下へコピーで共有式にする行数（これより少なければ 1 つずつの式）。
const SHARED_MIN: u64 = 16;
/// 共有式にできない式を 1 つずつ下へコピーできる行数。
const INDIVIDUAL_MAX: u64 = 100_000;

/// 式の参照（シート名・範囲）を集める。
fn collect_refs(e: &yy_formula::Expr, out: &mut Vec<(Option<Arc<str>>, yy_formula::Area)>) {
    use yy_formula::Expr;
    match e {
        Expr::Ref(r) => out.push((r.sheet.clone(), r.area)),
        Expr::Neg(x) | Expr::Plus(x) | Expr::Percent(x) | Expr::Paren(x) => collect_refs(x, out),
        Expr::Bin(_, l, r) => {
            collect_refs(l, out);
            collect_refs(r, out);
        }
        Expr::Call(_, args) => args.iter().for_each(|a| collect_refs(a, out)),
        _ => {}
    }
}

impl Workbook {
    /// シートの行・列を挿入・削除し、ブックのすべての式の参照を付け替える。
    pub fn edit_rows_cols(
        &mut self,
        ctx: &Context,
        sheet: usize,
        edit: yy_formula::Edit,
    ) -> io::Result<()> {
        let s = &mut self.sheets[sheet];
        match edit {
            yy_formula::Edit::InsertRows(at, n) => s.insert_rows(ctx, at, n)?,
            yy_formula::Edit::DeleteRows(at, n) => s.delete_rows(ctx, at, n)?,
            yy_formula::Edit::InsertCols(at, n) => s.insert_cols(ctx, at, n)?,
            yy_formula::Edit::DeleteCols(at, n) => s.delete_cols(at, n),
        }
        crate::formula::adjust_refs(self, sheet, edit);
        Ok(())
    }
}

impl Default for Workbook {
    fn default() -> Self {
        Workbook {
            sheets: vec![Sheet::new("Sheet1")],
            date_system: DateSystem::D1900,
        }
    }
}

/// Undo の深さ。
const UNDO_DEPTH: usize = 1000;

/// 開いている文書（ブックと、Undo・保存の状態）。
pub struct Document {
    pub ctx: Arc<Context>,
    pub book: Workbook,
    undo: Vec<Workbook>,
    redo: Vec<Workbook>,
    generation: u64,
    pub path: Option<PathBuf>,
    /// 開いた（保存した）独自形式のファイル
    pub(crate) file: Option<Arc<Store>>,
    /// そのファイルの最後の目次の位置
    pub(crate) last_dir: u64,
    /// 保存してから変えた
    pub dirty: bool,
    /// 編集のあとの再計算を呼ぶ側に任せる（UI がバックグラウンドで行う）
    pub defer_recalc: bool,
    /// 任された再計算がまだ（[`Document::take_recalc`]）
    pub recalc_pending: bool,
}

impl Document {
    pub fn new(ctx: Arc<Context>) -> Document {
        Document {
            ctx,
            book: Workbook::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            generation: 0,
            path: None,
            file: None,
            last_dir: 0,
            dirty: false,
            defer_recalc: false,
            recalc_pending: false,
        }
    }

    pub fn with_book(ctx: Arc<Context>, book: Workbook) -> Document {
        Document {
            book,
            ..Document::new(ctx)
        }
    }

    /// 1 つの操作として変える（失敗したら元に戻す）。
    pub fn edit(
        &mut self,
        f: impl FnOnce(&mut Workbook, &Context) -> io::Result<()>,
    ) -> io::Result<()> {
        let before = self.book.clone();
        if let Err(e) = f(&mut self.book, &self.ctx) {
            self.book = before;
            return Err(e);
        }
        if self.defer_recalc {
            self.recalc_pending |= self.book.sheets.iter().any(|s| !s.formulas.is_empty());
        } else {
            crate::formula::recalc(&mut self.book, &self.ctx);
        }
        self.undo.push(before);
        self.generation += 1;
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
        Ok(())
    }

    /// 任された再計算を引き取る（計算するブックの写し。計算したら [`Document::put_recalc`]）。
    pub fn take_recalc(&mut self) -> Option<Workbook> {
        std::mem::take(&mut self.recalc_pending).then(|| self.book.clone())
    }

    /// 計算したブックを戻す。
    pub fn put_recalc(&mut self, book: Workbook) {
        self.book = book;
    }

    /// 変更の番号（編集・元に戻す・やり直すのたびに増える。直前の編集が変わっていないかを確かめる）。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some(b) => {
                self.generation += 1;
                self.redo.push(std::mem::replace(&mut self.book, b));
                self.dirty = true;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some(b) => {
                self.generation += 1;
                self.undo.push(std::mem::replace(&mut self.book, b));
                self.dirty = true;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{CellRef, Chunk, Data};
    use crate::column::Piece;

    fn sheet(ctx: &Context) -> Sheet {
        let mut cols = Vec::new();
        for (name, vals) in [
            ("地域", ["東京", "大阪", "名古屋"]),
            ("店", ["新宿", "梅田", "栄"]),
        ] {
            let d = Data::from_values(vals.iter().map(|s| CellRef::Text(s)));
            let ch = Chunk::create(ctx, d).unwrap();
            cols.push(Column::from_pieces(
                name,
                vec![Piece {
                    len: ch.rows,
                    chunk: ch,
                    start: 0,
                }],
            ));
        }
        let mut s = Sheet::new("S");
        s.table = Table {
            columns: Arc::new(cols),
            rows: 3,
            header: true,
        };
        s
    }

    /// 手で入れた・貼り付けたデータ（表の外の自由なセル）を表に取り込み、絞り込めるようにする。
    #[test]
    fn absorbs_free_cells_into_a_table() {
        use crate::query::{Cmp, ColFilter, Cond};
        let ctx = Context::for_tests();
        let mut s = Sheet::new("S");
        let put = |s: &mut Sheet, r: u64, c: u32, v: Value| s.set(&ctx, r, c, v).unwrap();
        put(&mut s, 0, 0, Value::text("品名"));
        put(&mut s, 0, 1, Value::text("数"));
        for (r, (n, q)) in [("りんご", 3.0), ("みかん", 10.0), ("ぶどう", 7.0)]
            .iter()
            .enumerate()
        {
            put(&mut s, r as u64 + 1, 0, Value::text(n));
            put(&mut s, r as u64 + 1, 1, Value::Number(*q));
        }
        // 見出しのない列（C）と、表の下の自由なセル
        put(&mut s, 2, 2, Value::text("メモ"));
        assert_eq!(s.table.cols(), 0);
        let before: Vec<Value> = (0..4).map(|r| s.get(&ctx, r, 1).unwrap()).collect();
        assert!(s.absorb_free_cells(&ctx, 1).unwrap());
        assert_eq!(s.table.cols(), 3);
        assert_eq!(s.table.rows, 3);
        assert!(s.table.header);
        assert_eq!(&*s.table.columns[0].name, "品名");
        assert_eq!(&*s.table.columns[2].name, "C");
        assert!(s.cells.is_empty());
        // 格子の見え方は変わらない
        let after: Vec<Value> = (0..4).map(|r| s.get(&ctx, r, 1).unwrap()).collect();
        assert_eq!(before, after);
        assert_eq!(s.get(&ctx, 2, 2).unwrap(), Value::text("メモ"));
        // 絞り込める
        let (bits, counts) = crate::query::filter(
            &ctx,
            &s.table,
            &[ColFilter {
                col: 1,
                cond: Cond::Number {
                    op: Cmp::Gt,
                    value: 5.0,
                },
            }],
        )
        .unwrap();
        assert_eq!(counts, [2]);
        assert_eq!(bits.count_ones(), 2);
        // 二度目は何もしない
        assert!(!s.absorb_free_cells(&ctx, 1).unwrap());
        // 表の右に足した列も取り込む
        put(&mut s, 0, 4, Value::text("産地"));
        put(&mut s, 1, 4, Value::text("青森"));
        put(&mut s, 9, 4, Value::text("表の下"));
        assert!(s.absorb_free_cells(&ctx, 4).unwrap());
        assert_eq!(s.table.cols(), 5);
        assert_eq!(&*s.table.columns[3].name, "D");
        assert_eq!(&*s.table.columns[4].name, "産地");
        assert_eq!(s.get(&ctx, 1, 4).unwrap(), Value::text("青森"));
        assert_eq!(s.get(&ctx, 9, 4).unwrap(), Value::text("表の下"));
        assert_eq!(s.cells.len(), 1);
        // 1 行目が空なら取り込まない
        let mut e = Sheet::new("E");
        e.set(&ctx, 3, 0, Value::text("x")).unwrap();
        e.set(&ctx, 4, 0, Value::text("y")).unwrap();
        assert!(!e.absorb_free_cells(&ctx, 0).unwrap());
    }

    /// 数式のセルは、絞り込み・並べ替えで結果の値として扱う（個別の式も共有式も）。
    #[test]
    fn queries_use_formula_results() {
        use crate::query::{Cmp, ColFilter, Cond};
        let ctx = Context::for_tests();
        let mut s = Sheet::new("S");
        s.set(&ctx, 0, 0, Value::text("数")).unwrap();
        s.set(&ctx, 0, 1, Value::text("倍")).unwrap();
        s.set(&ctx, 0, 2, Value::text("足す")).unwrap();
        for r in 1..=4u64 {
            s.set(&ctx, r, 0, Value::Number(r as f64)).unwrap();
            s.set_formula(&ctx, r, 1, &format!("=A{}*10", r + 1))
                .unwrap();
        }
        // C 列は共有式（C2:C5 = A+100）
        s.formulas.add_shared(crate::shared::Shared {
            col: 2,
            r0: 1,
            r1: 4,
            formula: Formula::parse("=A2+100").unwrap(),
            results: None,
        });
        assert!(s.absorb_free_cells(&ctx, 2).unwrap());
        let mut book = Workbook {
            sheets: vec![s],
            date_system: DateSystem::D1900,
        };
        crate::formula::recalc(&mut book, &ctx);
        let s = &book.sheets[0];
        assert_eq!(s.get(&ctx, 3, 1).unwrap(), Value::Number(30.0));
        assert_eq!(s.get(&ctx, 3, 2).unwrap(), Value::Number(103.0));
        // 表そのものの列は空（式は表の外に持つ）
        assert_eq!(s.table.columns[1].get(&ctx, 2).unwrap(), Value::Empty);
        let q = s.query_table(&ctx).unwrap();
        assert_eq!(q.columns[1].get(&ctx, 2).unwrap(), Value::Number(30.0));
        assert_eq!(q.columns[2].get(&ctx, 3).unwrap(), Value::Number(104.0));
        let gt = |col: u32, value: f64| ColFilter {
            col,
            cond: Cond::Number { op: Cmp::Gt, value },
        };
        let (_, counts) = crate::query::filter(&ctx, &q, &[gt(1, 25.0)]).unwrap();
        assert_eq!(counts, [2]);
        let (_, counts) = crate::query::filter(&ctx, &q, &[gt(2, 101.5)]).unwrap();
        assert_eq!(counts, [3]);
        // 並べ替えも結果の値で（降順なら 4 行目が先）
        let order = crate::query::sort(
            &ctx,
            &q,
            &[crate::query::SortKey { col: 1, desc: true }],
            None,
        )
        .unwrap();
        assert_eq!(order[0], 3);
        // 数式がなければ表そのもの
        let plain = Sheet::new("P");
        assert_eq!(plain.query_table(&ctx).unwrap().cols(), 0);
    }

    #[test]
    fn grid_mapping_edits_and_undo() {
        let ctx = Context::for_tests();
        let mut doc = Document::with_book(
            ctx.clone(),
            Workbook {
                sheets: vec![sheet(&ctx)],
                date_system: DateSystem::D1900,
            },
        );
        let s = &doc.book.sheets[0];
        assert_eq!(s.get(&ctx, 0, 0).unwrap(), Value::text("地域"));
        assert_eq!(s.get(&ctx, 2, 1).unwrap(), Value::text("梅田"));
        assert_eq!(s.extent(), (4, 2));
        doc.edit(|b, ctx| {
            let s = &mut b.sheets[0];
            s.set(ctx, 1, 0, Value::Number(1.0))?;
            s.set(ctx, 0, 1, "店舗".into())?;
            s.set(ctx, 10, 5, "メモ".into())
        })
        .unwrap();
        let s = &doc.book.sheets[0];
        assert_eq!(s.get(&ctx, 1, 0).unwrap(), Value::Number(1.0));
        assert_eq!(s.get(&ctx, 0, 1).unwrap(), Value::text("店舗"));
        assert_eq!(s.extent(), (11, 6));
        doc.edit(|b, ctx| b.sheets[0].insert_rows(ctx, 2, 2))
            .unwrap();
        let s = &doc.book.sheets[0];
        assert_eq!(s.table.rows, 5);
        assert_eq!(s.get(&ctx, 2, 0).unwrap(), Value::Empty);
        assert_eq!(s.get(&ctx, 4, 0).unwrap(), Value::text("大阪"));
        assert_eq!(s.get(&ctx, 12, 5).unwrap(), Value::text("メモ"));
        doc.edit(|b, ctx| {
            b.sheets[0].insert_cols(ctx, 1, 1)?;
            b.sheets[0].delete_rows(ctx, 1, 1)
        })
        .unwrap();
        let s = &doc.book.sheets[0];
        assert_eq!(s.table.cols(), 3);
        assert_eq!(s.get(&ctx, 0, 2).unwrap(), Value::text("店舗"));
        assert_eq!(s.get(&ctx, 3, 2).unwrap(), Value::text("梅田"));
        assert_eq!(s.get(&ctx, 11, 6).unwrap(), Value::text("メモ"));
        assert!(doc.undo());
        assert!(doc.undo());
        assert!(doc.undo());
        let s = &doc.book.sheets[0];
        assert_eq!(s.get(&ctx, 1, 0).unwrap(), Value::text("東京"));
        assert!(doc.redo());
        assert_eq!(
            doc.book.sheets[0].get(&ctx, 1, 0).unwrap(),
            Value::Number(1.0)
        );
        doc.edit(|b, _| {
            b.sheets[0].delete_cols(0, 1);
            Ok(())
        })
        .unwrap();
        assert!(!doc.can_redo());
        assert_eq!(doc.book.sheets[0].table.cols(), 1);
    }
}

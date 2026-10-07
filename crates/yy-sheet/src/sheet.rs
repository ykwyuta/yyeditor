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
                    if count >= SHARED_MIN && !self_ref {
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

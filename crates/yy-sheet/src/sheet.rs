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
        (rows, cols)
    }

    /// 格子のセルの式。
    pub fn formula_at(&self, row: u64, col: u32) -> Option<&Formula> {
        if self.formulas.cells.is_empty() {
            return None;
        }
        self.formulas.cells.get(&(self.source_row(row), col))
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
        if !self.formulas.cells.is_empty() {
            let key = (self.source_row(row), col);
            if self.formulas.cells.contains_key(&key) {
                Arc::make_mut(&mut self.formulas.cells).remove(&key);
            }
        }
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
            .chain(self.formulas.results.keys().map(|&(c, r)| (r, c)));
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
    pub path: Option<PathBuf>,
    /// 開いた（保存した）独自形式のファイル
    pub(crate) file: Option<Arc<Store>>,
    /// そのファイルの最後の目次の位置
    pub(crate) last_dir: u64,
    /// 保存してから変えた
    pub dirty: bool,
}

impl Document {
    pub fn new(ctx: Arc<Context>) -> Document {
        Document {
            ctx,
            book: Workbook::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            path: None,
            file: None,
            last_dir: 0,
            dirty: false,
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
        crate::formula::recalc(&mut self.book, &self.ctx);
        self.undo.push(before);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
        Ok(())
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

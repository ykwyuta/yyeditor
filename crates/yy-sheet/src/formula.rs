//! 数式の保管と再計算（15 章 7.4）。
//!
//! 式はシートごとに（絞り込みをしないときの行, 列）→ 式で持ち、計算の結果（スピルした分を含む）は
//! 別に持つ。セルの値を読むと、結果があれば結果を返す。編集のたびに、式どうしの依存の順（参照する
//! 範囲にほかの式があれば、そちらを先に）ですべての式を計算し直す。循環する式は `#CALC!`。
//!
//! 評価は `yy-formula` に、ブックの読み方（[`yy_formula::Grid`]）を渡して行う。表の列は列ごとに
//! まとめて読む。

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::sync::Arc;

use yy_formula::{Cell, Edit, Expr, Grid, Val};

use crate::Context;
use crate::chunk::CellRef;
use crate::sheet::{Place, Workbook};
use crate::value::{CellError, Value};

/// 式。
#[derive(Clone, Debug)]
pub struct Formula {
    /// 入力した文字列（`=` から）
    pub text: Arc<str>,
    pub expr: Arc<Expr>,
}

impl Formula {
    /// 解析する。
    pub fn parse(text: &str) -> Result<Formula, String> {
        let expr = yy_formula::parse(text).map_err(|e| e.to_string())?;
        Ok(Formula {
            text: Arc::from(yy_formula::formula_text(&expr).as_str()),
            expr: Arc::new(expr),
        })
    }
}

/// シートの式と結果。
#[derive(Clone, Debug, Default)]
pub struct Formulas {
    /// （行, 列）→ 式
    pub cells: Arc<BTreeMap<(u64, u32), Formula>>,
    /// （列, 行）→ 結果（スピルした分を含む）
    pub results: Arc<BTreeMap<(u32, u64), Value>>,
    /// 式のセル → スピルした大きさ（行数, 列数）
    pub spills: Arc<BTreeMap<(u64, u32), (u64, u32)>>,
}

impl Formulas {
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.results.is_empty()
    }

    pub fn result(&self, row: u64, col: u32) -> Option<&Value> {
        if self.results.is_empty() {
            return None;
        }
        self.results.get(&(col, row))
    }

    /// 行・列の挿入と削除で、式のセルの位置をずらす（参照の付け替えは [`adjust_refs`]）。
    pub(crate) fn shift(&mut self, edit: Edit) {
        if self.cells.is_empty() {
            return;
        }
        let cells = std::mem::take(Arc::make_mut(&mut self.cells));
        let moved = cells
            .into_iter()
            .filter_map(|((r, c), f)| {
                let pos = match edit {
                    Edit::InsertRows(at, n) => Some((if r >= at { r + n } else { r }, c)),
                    Edit::DeleteRows(at, n) => {
                        if r >= at && r < at + n {
                            None
                        } else {
                            Some((if r >= at + n { r - n } else { r }, c))
                        }
                    }
                    Edit::InsertCols(at, n) => Some((r, if c >= at { c + n } else { c })),
                    Edit::DeleteCols(at, n) => {
                        if c >= at && c < at + n {
                            None
                        } else {
                            Some((r, if c >= at + n { c - n } else { c }))
                        }
                    }
                };
                pos.map(|p| (p, f))
            })
            .collect();
        self.cells = Arc::new(moved);
    }
}

/// シート `sheet` の行・列の編集に合わせて、ブックのすべての式の参照を付け替える。
pub fn adjust_refs(book: &mut Workbook, sheet: usize, edit: Edit) {
    let name = book.sheets[sheet].name.clone();
    for (i, s) in book.sheets.iter_mut().enumerate() {
        if s.formulas.cells.is_empty() {
            continue;
        }
        let hits = |r: Option<&str>| match r {
            None => i == sheet,
            Some(n) => yy_formula::eq_text(n, &name),
        };
        let cells = Arc::make_mut(&mut s.formulas.cells);
        for f in cells.values_mut() {
            let mut e = (*f.expr).clone();
            if yy_formula::adjust(&mut e, edit, &hits) {
                f.text = Arc::from(yy_formula::formula_text(&e).as_str());
                f.expr = Arc::new(e);
            }
        }
    }
}

pub(crate) fn to_val(v: &Value) -> Val {
    match v {
        Value::Empty => Val::Empty,
        Value::Number(n) => Val::Num(*n),
        Value::Text(s) => Val::Text(s.clone()),
        Value::Bool(b) => Val::Bool(*b),
        Value::Error(e) => Val::Err(yy_formula::Error::from_code(e.code()).expect("same order")),
    }
}

fn to_cell(v: CellRef<'_>) -> Cell<'_> {
    match v {
        CellRef::Empty => Cell::Empty,
        CellRef::Number(n) => Cell::Num(n),
        CellRef::Text(s) => Cell::Text(s),
        CellRef::Bool(b) => Cell::Bool(b),
        CellRef::Error(e) => Cell::Err(yy_formula::Error::from_code(e.code()).expect("same order")),
    }
}

pub(crate) fn from_val(v: &Val) -> Value {
    match v {
        Val::Empty => Value::Empty,
        Val::Num(n) => Value::Number(*n),
        Val::Text(s) => Value::Text(s.clone()),
        Val::Bool(b) => Value::Bool(*b),
        Val::Err(e) => Value::Error(CellError::from_code(e.code()).expect("same order")),
        Val::Array(a) => a.data.first().map(from_val).unwrap_or_default(),
    }
}

/// ブックの読み方（計算中の結果を重ねる）。
struct BookGrid<'a> {
    book: &'a Workbook,
    ctx: &'a Context,
    results: &'a [BTreeMap<(u32, u64), Value>],
}

impl BookGrid<'_> {
    /// 式・結果を除いた値（絞り込みをしないときの位置）。
    fn base(&self, sheet: usize, row: u64, col: u32) -> Value {
        let s = &self.book.sheets[sheet];
        match s.place_source(row, col) {
            Place::Header(c) => Value::Text(s.table.columns[c as usize].name.clone()),
            Place::Data(r, c) => s.table.columns[c as usize]
                .get(self.ctx, r)
                .unwrap_or(Value::Error(CellError::Ref)),
            Place::Free => s.cells.get(&(row, col)).cloned().unwrap_or_default(),
        }
    }
}

impl Grid for BookGrid<'_> {
    fn sheet(&self, name: &str) -> Option<usize> {
        self.book
            .sheets
            .iter()
            .position(|s| yy_formula::eq_text(&s.name, name))
    }

    fn get(&self, sheet: usize, row: u64, col: u32) -> Val {
        if let Some(v) = self.results[sheet].get(&(col, row)) {
            return to_val(v);
        }
        to_val(&self.base(sheet, row, col))
    }

    fn used(&self, sheet: usize) -> (u64, u32) {
        let s = &self.book.sheets[sheet];
        let (mut rows, mut cols) = s.source_extent();
        for &(c, r) in self.results[sheet].keys() {
            rows = rows.max(r + 1);
            cols = cols.max(c + 1);
        }
        (rows, cols)
    }

    fn scan(&self, sheet: usize, col: u32, rows: Range<u64>, f: &mut dyn FnMut(u64, Cell<'_>)) {
        let s = &self.book.sheets[sheet];
        let res: HashMap<u64, &Value> = self.results[sheet]
            .range((col, rows.start)..(col, rows.end))
            .map(|(&(_, r), v)| (r, v))
            .collect();
        let t = &s.table;
        let head = t.header as u64;
        let mut row = rows.start;
        // 表の列: 列をまとめて読む
        if col < t.cols() && row < t.grid_rows() {
            if t.header && row == 0 {
                match res.get(&0) {
                    Some(v) => f(0, to_val(v).cell()),
                    None => f(0, Cell::Text(&t.columns[col as usize].name)),
                }
                row = 1;
            }
            let end = rows.end.min(t.grid_rows());
            if row < end {
                let c = &t.columns[col as usize];
                let r = if res.is_empty() {
                    c.for_each(self.ctx, row - head..end - head, |tr, v| {
                        f(tr + head, to_cell(v))
                    })
                } else {
                    c.for_each(self.ctx, row - head..end - head, |tr, v| {
                        let gr = tr + head;
                        match res.get(&gr) {
                            Some(x) => f(gr, to_val(x).cell()),
                            None => f(gr, to_cell(v)),
                        }
                    })
                };
                if r.is_err() {
                    for gr in row..end {
                        f(gr, Cell::Err(yy_formula::Error::Ref));
                    }
                }
                row = end;
            }
        }
        // 表の外: 自由なセル（疎）と結果
        for r in row..rows.end {
            match res.get(&r) {
                Some(v) => f(r, to_val(v).cell()),
                None => match s.cells.get(&(r, col)) {
                    Some(v) => f(r, to_val(v).cell()),
                    None => f(r, Cell::Empty),
                },
            }
        }
    }
}

/// 式の参照する範囲（シートの番号・範囲）。
fn refs(e: &Expr, sheet: usize, book: &Workbook, out: &mut Vec<(usize, yy_formula::Area)>) {
    match e {
        Expr::Ref(r) => {
            let s = match &r.sheet {
                None => Some(sheet),
                Some(n) => book
                    .sheets
                    .iter()
                    .position(|x| yy_formula::eq_text(&x.name, n)),
            };
            if let Some(s) = s {
                out.push((s, r.area));
            }
        }
        Expr::Neg(x) | Expr::Plus(x) | Expr::Percent(x) | Expr::Paren(x) => {
            refs(x, sheet, book, out)
        }
        Expr::Bin(_, l, r) => {
            refs(l, sheet, book, out);
            refs(r, sheet, book, out);
        }
        Expr::Call(_, args) => args.iter().for_each(|a| refs(a, sheet, book, out)),
        _ => {}
    }
}

/// ブックのすべての式を計算し直す。
pub fn recalc(book: &mut Workbook, ctx: &Context) {
    if book.sheets.iter().all(|s| s.formulas.is_empty()) {
        return;
    }
    // 式の一覧と、（シート, 列, 行）→ 番号
    let mut list: Vec<(usize, u64, u32, Arc<Expr>)> = Vec::new();
    let mut index: Vec<BTreeMap<(u32, u64), usize>> = vec![BTreeMap::new(); book.sheets.len()];
    for (si, s) in book.sheets.iter().enumerate() {
        for (&(r, c), f) in s.formulas.cells.iter() {
            index[si].insert((c, r), list.len());
            list.push((si, r, c, f.expr.clone()));
        }
    }
    // 依存（参照する範囲にある式・前回スピルした範囲が重なる式）
    let n = list.len();
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, (si, _, _, e)) in list.iter().enumerate() {
        let mut rs = Vec::new();
        refs(e, *si, book, &mut rs);
        for (s, a) in rs {
            let max_c = index[s].keys().next_back().map_or(0, |k| k.0);
            for c in a.c0..=a.c1.min(max_c) {
                for (_, &j) in index[s].range((c, a.r0)..=(c, a.r1)) {
                    deps[i].push(j);
                }
            }
            for (&(r, c), &(h, w)) in book.sheets[s].formulas.spills.iter() {
                let spill = yy_formula::Area {
                    r0: r,
                    c0: c,
                    r1: r + h - 1,
                    c1: c + w - 1,
                    ..yy_formula::Area::cell(r, c)
                };
                if a.intersects(&spill)
                    && let Some(&j) = index[s].get(&(c, r))
                {
                    deps[i].push(j);
                }
            }
        }
        deps[i].retain(|&j| j != i);
    }
    // 依存の順（Kahn）。残ったものは循環
    let mut indeg = vec![0usize; n];
    let mut users: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, d) in deps.iter().enumerate() {
        indeg[i] = d.len();
        for &j in d {
            users[j].push(i);
        }
    }
    let mut order: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut k = 0;
    while k < order.len() {
        let i = order[k];
        k += 1;
        for &u in &users[i] {
            indeg[u] -= 1;
            if indeg[u] == 0 {
                order.push(u);
            }
        }
    }
    let mut cyclic = vec![true; n];
    for &i in &order {
        cyclic[i] = false;
    }
    let mut results: Vec<BTreeMap<(u32, u64), Value>> = vec![BTreeMap::new(); book.sheets.len()];
    let mut spills: Vec<BTreeMap<(u64, u32), (u64, u32)>> =
        vec![BTreeMap::new(); book.sheets.len()];
    for (i, &(si, r, c, _)) in list.iter().enumerate() {
        if cyclic[i] {
            results[si].insert((c, r), Value::Error(CellError::Calc));
        }
    }
    let sys = book.date_system;
    for &i in &order {
        let (si, r, c, ref e) = list[i];
        let v = {
            let grid = BookGrid {
                book,
                ctx,
                results: &results,
            };
            yy_formula::eval(
                e,
                &yy_formula::Context {
                    grid: &grid,
                    sheet: si,
                    sys,
                },
            )
        };
        match v {
            Val::Array(a) if a.rows * a.cols > 1 => {
                // あふれる先に値・式・ほかのスピルがあれば #SPILL!
                let grid = BookGrid {
                    book,
                    ctx,
                    results: &results,
                };
                let blocked = (0..a.rows).any(|dr| {
                    (0..a.cols).any(|dc| {
                        if dr == 0 && dc == 0 {
                            return false;
                        }
                        let (rr, cc) = (r + dr as u64, c + dc as u32);
                        index[si].contains_key(&(cc, rr))
                            || results[si].contains_key(&(cc, rr))
                            || !grid.base(si, rr, cc).is_empty()
                    })
                });
                if blocked {
                    results[si].insert((c, r), Value::Error(CellError::Spill));
                } else {
                    for dr in 0..a.rows {
                        for dc in 0..a.cols {
                            results[si]
                                .insert((c + dc as u32, r + dr as u64), from_val(a.get(dr, dc)));
                        }
                    }
                    spills[si].insert((r, c), (a.rows as u64, a.cols as u32));
                }
            }
            v => {
                results[si].insert((c, r), from_val(&v));
            }
        }
    }
    for (si, s) in book.sheets.iter_mut().enumerate() {
        s.formulas.results = Arc::new(std::mem::take(&mut results[si]));
        s.formulas.spills = Arc::new(std::mem::take(&mut spills[si]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, Data};
    use crate::column::{Column, Piece};
    use crate::sheet::{Document, Table};

    fn val(d: &Document, row: u64, col: u32) -> Value {
        d.book.sheets[0].get(&d.ctx, row, col).unwrap()
    }

    fn put(d: &mut Document, row: u64, col: u32, v: Value) {
        d.edit(|b, ctx| b.sheets[0].set(ctx, row, col, v)).unwrap();
    }

    fn formula(d: &mut Document, row: u64, col: u32, f: &str) {
        d.edit(|b, ctx| {
            b.sheets[0]
                .set_formula(ctx, row, col, f)
                .map_err(std::io::Error::other)
        })
        .unwrap();
    }

    #[test]
    fn recalculates_in_dependency_order() {
        let ctx = Context::for_tests();
        let mut d = Document::with_book(ctx, Workbook::default());
        put(&mut d, 0, 0, 2.0.into());
        put(&mut d, 1, 0, 3.0.into());
        // C1 が先に入っても B1 のあとに計算する
        formula(&mut d, 0, 2, "=b1*2");
        formula(&mut d, 0, 1, "=A1*A2+1");
        assert_eq!(val(&d, 0, 1), 7.0.into());
        assert_eq!(val(&d, 0, 2), 14.0.into());
        assert_eq!(&*d.book.sheets[0].formula_at(0, 2).unwrap().text, "=B1*2");
        put(&mut d, 0, 0, 3.0.into());
        assert_eq!(val(&d, 0, 2), 20.0.into());
        // 値で上書きすれば式は消える
        put(&mut d, 0, 1, 1.0.into());
        assert!(d.book.sheets[0].formula_at(0, 1).is_none());
        assert_eq!(val(&d, 0, 2), 2.0.into());
        // Undo で式に戻る
        assert!(d.undo());
        assert_eq!(val(&d, 0, 2), 20.0.into());
        // 循環
        formula(&mut d, 5, 0, "=A7+1");
        formula(&mut d, 6, 0, "=A6");
        assert_eq!(val(&d, 5, 0), Value::Error(CellError::Calc));
    }

    #[test]
    fn spills_and_blocks() {
        let ctx = Context::for_tests();
        let mut d = Document::with_book(ctx, Workbook::default());
        formula(&mut d, 0, 3, "=TEXTSPLIT(\"a,b,c\",\",\")");
        assert_eq!(val(&d, 0, 3), "a".into());
        assert_eq!(val(&d, 0, 5), "c".into());
        assert_eq!(d.book.sheets[0].extent(), (1, 6));
        // スピルした先を参照する式
        formula(&mut d, 1, 0, "=F1&\"!\"");
        assert_eq!(val(&d, 1, 0), "c!".into());
        // あふれる先に値があれば #SPILL!
        put(&mut d, 0, 4, 1.0.into());
        assert_eq!(val(&d, 0, 3), Value::Error(CellError::Spill));
        assert_eq!(val(&d, 0, 5), Value::Empty);
        assert_eq!(val(&d, 1, 0), "!".into());
    }

    #[test]
    fn references_follow_row_and_col_edits() {
        let ctx = Context::for_tests();
        let mut d = Document::with_book(ctx, Workbook::default());
        put(&mut d, 0, 0, 5.0.into());
        formula(&mut d, 2, 1, "=A1*2");
        d.edit(|b, ctx| b.edit_rows_cols(ctx, 0, Edit::InsertRows(0, 2)))
            .unwrap();
        let f = d.book.sheets[0].formula_at(4, 1).unwrap();
        assert_eq!(&*f.text, "=A3*2");
        assert_eq!(val(&d, 4, 1), 10.0.into());
        d.edit(|b, ctx| b.edit_rows_cols(ctx, 0, Edit::DeleteRows(2, 1)))
            .unwrap();
        assert_eq!(val(&d, 3, 1), Value::Error(CellError::Ref));
    }

    fn col(ctx: &Context, name: &str, vals: &[Value]) -> Column {
        let ch = Chunk::create(ctx, Data::from_values(vals.iter().map(CellRef::of))).unwrap();
        Column::from_pieces(
            name,
            vec![Piece {
                len: ch.rows,
                chunk: ch,
                start: 0,
            }],
        )
    }

    #[test]
    fn table_columns_and_files() {
        let ctx = Context::for_tests();
        let mut book = Workbook::default();
        let s = &mut book.sheets[0];
        s.table = Table {
            columns: Arc::new(vec![
                col(&ctx, "地域", &["東京".into(), "大阪".into(), "東京".into()]),
                col(&ctx, "売上", &[100.0.into(), 200.0.into(), 50.0.into()]),
            ]),
            rows: 3,
            header: true,
        };
        let mut d = Document::with_book(ctx.clone(), book);
        formula(&mut d, 0, 3, "=SUMIFS(B:B,A:A,\"東京\")");
        formula(&mut d, 1, 3, "=XLOOKUP(\"大阪\",A:A,B:B)");
        formula(&mut d, 2, 3, "=COUNTIFS(A2:A4,\"東京\",B2:B4,\">60\")");
        assert_eq!(val(&d, 0, 3), 150.0.into());
        assert_eq!(val(&d, 1, 3), 200.0.into());
        assert_eq!(val(&d, 2, 3), 1.0.into());
        // 表のセルを直せば計算し直す
        put(&mut d, 2, 1, 250.0.into());
        assert_eq!(val(&d, 1, 3), 250.0.into());
        // 保存して開けば式と結果が戻る
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.yys");
        crate::yys::save(&mut d, &path, &mut |_, _| true).unwrap();
        let d2 = crate::yys::open(ctx.clone(), &path).unwrap();
        assert_eq!(val(&d2, 0, 3), 150.0.into());
        assert_eq!(
            &*d2.book.sheets[0].formula_at(1, 3).unwrap().text,
            "=XLOOKUP(\"大阪\",A:A,B:B)"
        );
    }
}

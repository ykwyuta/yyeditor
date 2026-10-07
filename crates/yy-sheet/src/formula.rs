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
    /// 式のセル → あふれようとして止められた大きさ（`#SPILL!`。止めた値が消えたら計算し直す）
    pub blocked: Arc<BTreeMap<(u64, u32), (u64, u32)>>,
    /// 前回の計算から変わったセル（計算し直す式を決める。保存しない）
    pub(crate) touched: Touched,
}

/// 変わったセル（範囲）の記録。多すぎればすべて計算し直す。
#[derive(Clone, Debug, Default)]
pub(crate) struct Touched {
    pub(crate) all: bool,
    pub(crate) rects: Vec<yy_formula::Area>,
}

/// これを超えて範囲が変わったら、すべての式を計算し直す。
const TOUCH_LIMIT: usize = 4096;

impl Formulas {
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.results.is_empty()
    }

    /// セル（範囲）が変わったことを記録する。
    pub(crate) fn touch(&mut self, r0: u64, c0: u32, r1: u64, c1: u32) {
        let t = &mut self.touched;
        if t.all {
            return;
        }
        if t.rects.len() >= TOUCH_LIMIT {
            t.all = true;
            t.rects.clear();
            return;
        }
        t.rects.push(yy_formula::Area {
            r0,
            c0,
            r1,
            c1,
            ..yy_formula::Area::cell(r0, c0)
        });
    }

    /// すべての式を計算し直すようにする（行・列の挿入と削除、表の作り直しのあと）。
    pub fn touch_all(&mut self) {
        self.touched.all = true;
        self.touched.rects.clear();
    }

    /// 式を消す（結果とスピルした分も消し、参照していた式を計算し直すようにする）。
    pub(crate) fn remove(&mut self, row: u64, col: u32) {
        if Arc::make_mut(&mut self.cells).remove(&(row, col)).is_none() {
            return;
        }
        let (h, w) = self.spills.get(&(row, col)).copied().unwrap_or((1, 1));
        let results = Arc::make_mut(&mut self.results);
        for c in col..col + w {
            for r in row..row + h {
                results.remove(&(c, r));
            }
        }
        Arc::make_mut(&mut self.spills).remove(&(row, col));
        Arc::make_mut(&mut self.blocked).remove(&(row, col));
        self.touch(row, col, row + h - 1, col + w - 1);
    }

    pub fn result(&self, row: u64, col: u32) -> Option<&Value> {
        if self.results.is_empty() {
            return None;
        }
        self.results.get(&(col, row))
    }

    /// 行・列の挿入と削除で、式のセルの位置をずらす（参照の付け替えは [`adjust_refs`]）。
    pub(crate) fn shift(&mut self, edit: Edit) {
        self.touch_all();
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

/// 覚えておく `XLOOKUP` の索引の数。
const LOOKUP_KEEP: usize = 8;

/// 覚えている `XLOOKUP` の索引。
#[derive(Debug)]
pub(crate) struct LookupEntry {
    /// 列（データを共有しているかで比べる。持っている間はデータも消えない）
    column: crate::Column,
    header: bool,
    rows: Range<u64>,
    index: Arc<yy_formula::ExactIndex>,
    bytes: u64,
}

/// ブックの読み方（計算中の結果を重ねる）。
struct BookGrid<'a> {
    book: &'a Workbook,
    ctx: &'a Context,
    results: &'a [BTreeMap<(u32, u64), Value>],
    /// シートごとの（列, 行）→ 式の番号
    formulas: &'a [BTreeMap<(u32, u64), usize>],
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
    fn stable(&self, sheet: usize, a: &yy_formula::Area) -> bool {
        (a.c0..=a.c1).all(|c| {
            self.formulas[sheet]
                .range((c, a.r0)..=(c, a.r1))
                .next()
                .is_none()
                && self.results[sheet]
                    .range((c, a.r0)..=(c, a.r1))
                    .next()
                    .is_none()
        })
    }

    fn exact_index(
        &self,
        sheet: usize,
        col: u32,
        rows: Range<u64>,
    ) -> Option<Arc<yy_formula::ExactIndex>> {
        // 表の列（式・結果を含まない）だけ。データが同じなら前に作った索引を使う
        let t = &self.book.sheets[sheet].table;
        if col >= t.cols() || rows.end > t.grid_rows() || rows.is_empty() {
            return None;
        }
        let area = yy_formula::Area {
            r0: rows.start,
            c0: col,
            r1: rows.end - 1,
            c1: col,
            ..yy_formula::Area::cell(rows.start, col)
        };
        if !self.stable(sheet, &area) {
            return None;
        }
        let column = &t.columns[col as usize];
        {
            let mut lookups = self.ctx.lookups.lock().unwrap();
            if let Some(i) = lookups
                .iter()
                .position(|e| e.column.same_data(column) && e.header == t.header && e.rows == rows)
            {
                let e = lookups.remove(i);
                let ix = e.index.clone();
                lookups.push(e);
                return Some(ix);
            }
        }
        let index = Arc::new(yy_formula::ExactIndex::build(
            self,
            sheet,
            col,
            rows.clone(),
        ));
        let bytes = index.heap_bytes() as u64;
        let mut lookups = self.ctx.lookups.lock().unwrap();
        let budget = &self.ctx.budget;
        // 予算（索引の分）に入るまで古いものを捨てる
        while !lookups.is_empty()
            && (lookups.len() >= LOOKUP_KEEP || !budget.fits(crate::budget::Part::Index, bytes))
        {
            let old = lookups.remove(0);
            budget.sub(crate::budget::Part::Index, old.bytes);
        }
        budget.add(crate::budget::Part::Index, bytes);
        lookups.push(LookupEntry {
            column: column.clone(),
            header: t.header,
            rows,
            index: index.clone(),
            bytes,
        });
        Some(index)
    }

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

fn rect(r: u64, c: u32, (h, w): (u64, u32)) -> yy_formula::Area {
    yy_formula::Area {
        r0: r,
        c0: c,
        r1: r + h - 1,
        c1: c + w - 1,
        ..yy_formula::Area::cell(r, c)
    }
}

/// 式を計算し直す。前回から変わったセルに関わる式（と、それに依存する式）だけを、依存の順に計算する。
/// 行・列の挿入と削除のあとなどは、すべての式を計算する。
pub fn recalc(book: &mut Workbook, ctx: &Context) {
    let clear_touched = |book: &mut Workbook| {
        for s in book.sheets.iter_mut() {
            s.formulas.touched = Touched::default();
        }
    };
    if book.sheets.iter().all(|s| s.formulas.is_empty()) {
        clear_touched(book);
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
    let mut ref_lists: Vec<Vec<(usize, yy_formula::Area)>> = Vec::with_capacity(n);
    for (i, (si, _, _, e)) in list.iter().enumerate() {
        let mut rs = Vec::new();
        refs(e, *si, book, &mut rs);
        for &(s, a) in &rs {
            let max_c = index[s].keys().next_back().map_or(0, |k| k.0);
            for c in a.c0..=a.c1.min(max_c) {
                for (_, &j) in index[s].range((c, a.r0)..=(c, a.r1)) {
                    deps[i].push(j);
                }
            }
            for (&(r, c), &size) in book.sheets[s].formulas.spills.iter() {
                if a.intersects(&rect(r, c, size))
                    && let Some(&j) = index[s].get(&(c, r))
                {
                    deps[i].push(j);
                }
            }
        }
        deps[i].retain(|&j| j != i);
        ref_lists.push(rs);
    }
    let mut users: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, d) in deps.iter().enumerate() {
        for &j in d {
            users[j].push(i);
        }
    }
    // 計算し直す式
    let full = book.sheets.iter().any(|s| s.formulas.touched.all)
        || book
            .sheets
            .iter()
            .all(|s| s.formulas.results.is_empty() && s.formulas.blocked.is_empty());
    let mut dirty = vec![full; n];
    if !full {
        let touched = |s: usize, a: &yy_formula::Area| {
            book.sheets[s]
                .formulas
                .touched
                .rects
                .iter()
                .any(|t| t.intersects(a))
        };
        for (i, &(si, r, c, _)) in list.iter().enumerate() {
            let f = &book.sheets[si].formulas;
            let own = f
                .spills
                .get(&(r, c))
                .or_else(|| f.blocked.get(&(r, c)))
                .map_or(yy_formula::Area::cell(r, c), |&size| rect(r, c, size));
            dirty[i] = touched(si, &own)
                || ref_lists[i].iter().any(|(s, a)| touched(*s, a))
                // まだ結果のない式（新しく入れた式）
                || !f.results.contains_key(&(c, r));
        }
        let mut stack: Vec<usize> = (0..n).filter(|&i| dirty[i]).collect();
        while let Some(i) = stack.pop() {
            for &u in &users[i] {
                if !dirty[u] {
                    dirty[u] = true;
                    stack.push(u);
                }
            }
        }
    }
    clear_touched(book);
    if !dirty.iter().any(|&d| d) {
        return;
    }
    // 依存の順（Kahn）。残ったものは循環
    let mut indeg: Vec<usize> = deps.iter().map(Vec::len).collect();
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
    // 計算し直す式の前回の結果を消す（ほかの式の結果は残す）
    let mut results: Vec<BTreeMap<(u32, u64), Value>> = Vec::with_capacity(book.sheets.len());
    let mut spills: Vec<BTreeMap<(u64, u32), (u64, u32)>> = Vec::new();
    let mut blocked: Vec<BTreeMap<(u64, u32), (u64, u32)>> = Vec::new();
    for s in &book.sheets {
        if full {
            results.push(BTreeMap::new());
            spills.push(BTreeMap::new());
            blocked.push(BTreeMap::new());
        } else {
            results.push((*s.formulas.results).clone());
            spills.push((*s.formulas.spills).clone());
            blocked.push((*s.formulas.blocked).clone());
        }
    }
    if !full {
        for (i, &(si, r, c, _)) in list.iter().enumerate() {
            if !dirty[i] {
                continue;
            }
            let (h, w) = spills[si].remove(&(r, c)).unwrap_or((1, 1));
            blocked[si].remove(&(r, c));
            for cc in c..c + w {
                for rr in r..r + h {
                    results[si].remove(&(cc, rr));
                }
            }
        }
    }
    for (i, &(si, r, c, _)) in list.iter().enumerate() {
        if cyclic[i] && dirty[i] {
            results[si].insert((c, r), Value::Error(CellError::Calc));
        }
    }
    let sys = book.date_system;
    let cache = yy_formula::Cache::default();
    for &i in order.iter().filter(|&&i| dirty[i]) {
        let (si, r, c, ref e) = list[i];
        let v = {
            let grid = BookGrid {
                book,
                ctx,
                results: &results,
                formulas: &index,
            };
            yy_formula::eval(
                e,
                &yy_formula::Context {
                    grid: &grid,
                    sheet: si,
                    sys,
                    cache: Some(&cache),
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
                    formulas: &index,
                };
                let is_blocked = (0..a.rows).any(|dr| {
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
                let size = (a.rows as u64, a.cols as u32);
                if is_blocked {
                    results[si].insert((c, r), Value::Error(CellError::Spill));
                    blocked[si].insert((r, c), size);
                } else {
                    for dr in 0..a.rows {
                        for dc in 0..a.cols {
                            results[si]
                                .insert((c + dc as u32, r + dr as u64), from_val(a.get(dr, dc)));
                        }
                    }
                    spills[si].insert((r, c), size);
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
        s.formulas.blocked = Arc::new(std::mem::take(&mut blocked[si]));
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

#[cfg(test)]
mod incremental_tests {
    use super::*;
    use crate::sheet::Document;

    /// 計算の回数を数える代わりに、結果の Arc が変わったかで計算し直したかを見る。
    #[test]
    fn recalculates_only_affected_formulas() {
        let ctx = Context::for_tests();
        let mut d = Document::with_book(ctx, Workbook::default());
        let set = |d: &mut Document, r: u64, c: u32, v: Value| {
            d.edit(|b, ctx| b.sheets[0].set(ctx, r, c, v)).unwrap();
        };
        let formula = |d: &mut Document, r: u64, c: u32, f: &str| {
            d.edit(|b, ctx| {
                b.sheets[0]
                    .set_formula(ctx, r, c, f)
                    .map_err(std::io::Error::other)
            })
            .unwrap();
        };
        set(&mut d, 0, 0, 1.0.into());
        set(&mut d, 0, 1, 10.0.into());
        formula(&mut d, 1, 0, "=A1*2");
        formula(&mut d, 1, 1, "=B1*2");
        formula(&mut d, 2, 0, "=A2+B2");
        let get = |d: &Document, r, c| d.book.sheets[0].get(&d.ctx, r, c).unwrap();
        assert_eq!(get(&d, 2, 0), 22.0.into());
        // 関係のないセルを変えても結果は変わらない（計算もしない）
        let before = d.book.sheets[0].formulas.results.clone();
        set(&mut d, 9, 9, 5.0.into());
        assert!(Arc::ptr_eq(&before, &d.book.sheets[0].formulas.results));
        // A1 を変えると A2 と A3 だけ（B2 はそのまま）
        set(&mut d, 0, 0, 4.0.into());
        assert_eq!(get(&d, 1, 0), 8.0.into());
        assert_eq!(get(&d, 2, 0), 28.0.into());
        assert_eq!(get(&d, 1, 1), 20.0.into());
        // スピルを止めていた値を消すとスピルする
        formula(&mut d, 5, 0, "=TEXTSPLIT(\"x,y\",\",\")");
        set(&mut d, 5, 1, 1.0.into());
        assert_eq!(get(&d, 5, 0), Value::Error(CellError::Spill));
        set(&mut d, 5, 1, Value::Empty);
        assert_eq!(get(&d, 5, 1), "y".into());
        // 式を消すと結果も消え、参照していた式は計算し直す
        formula(&mut d, 6, 0, "=B6&\"!\"");
        assert_eq!(get(&d, 6, 0), "y!".into());
        set(&mut d, 5, 0, Value::Empty);
        assert_eq!(get(&d, 5, 1), Value::Empty);
        assert_eq!(get(&d, 6, 0), "!".into());
    }
}

#[cfg(test)]
mod deferred_tests {
    use super::*;
    use crate::sheet::Document;

    #[test]
    fn deferred_recalc_is_handed_to_the_caller() {
        let ctx = Context::for_tests();
        let mut d = Document::with_book(ctx.clone(), Workbook::default());
        d.defer_recalc = true;
        // 式がなければ任せるものもない
        d.edit(|b, ctx| b.sheets[0].set(ctx, 0, 0, 2.0.into()))
            .unwrap();
        assert!(d.take_recalc().is_none());
        d.edit(|b, ctx| {
            b.sheets[0]
                .set_formula(ctx, 0, 1, "=A1*3")
                .map_err(std::io::Error::other)
        })
        .unwrap();
        assert_eq!(d.book.sheets[0].get(&ctx, 0, 1).unwrap(), Value::Empty);
        let mut book = d.take_recalc().unwrap();
        assert!(d.take_recalc().is_none());
        recalc(&mut book, &ctx);
        d.put_recalc(book);
        assert_eq!(d.book.sheets[0].get(&ctx, 0, 1).unwrap(), 6.0.into());
        // Undo で計算済みの前の状態に戻る
        assert!(d.undo());
        assert!(d.book.sheets[0].formula_at(0, 1).is_none());
    }
}

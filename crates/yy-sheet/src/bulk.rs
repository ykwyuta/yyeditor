//! 大量のデータの操作（15 章 9.4）: 重複の削除・列の値の置換（正規表現）・列の型の変更・検索。
//!
//! どれもチャンク（区間）ごとに全コアで並列に処理し、変える列は新しいチャンクに作り直す。検索は、
//! 文字列のチャンクではまず辞書（行数よりずっと少ない）を調べ、当たる文字列のないチャンクを読み飛ばす。

use std::collections::HashSet;
use std::io;
use std::sync::Arc;

use rayon::prelude::*;
use regex_automata::meta::Regex;
use regex_automata::util::syntax;
use yy_numfmt::{DateSystem, Parsed};

use crate::Context;
use crate::chunk::{Builder, CellRef, Chunk, Data, FxMap};
use crate::column::{Column, Piece};
use crate::query::Key;
use crate::sheet::Table;
use crate::value::Value;

/// 表の行のうち `rows`（新しい順）だけを残した表。
pub fn pick(ctx: &Context, table: &Table, rows: &[u32]) -> io::Result<Table> {
    let columns = table
        .columns
        .iter()
        .map(|c| c.permuted(ctx, rows))
        .collect::<io::Result<Vec<_>>>()?;
    Ok(Table {
        columns: Arc::new(columns),
        rows: rows.len() as u64,
        header: table.header,
    })
}

// ---- 重複の削除 ----------------------------------------------------------------------

/// 列の値の番号（大文字・小文字を区別しない。空は 0）。文字列のチャンクは辞書の文字列ごとに 1 回だけ
/// 番号を引く。
fn codes(ctx: &Context, col: &Column, rows: usize) -> io::Result<Vec<u32>> {
    let mut map: FxMap<Key, u32> = FxMap::default();
    let mut code = |k: Option<Key>| -> u32 {
        match k {
            None => 0,
            Some(k) => {
                let next = map.len() as u32 + 1;
                *map.entry(k).or_insert(next)
            }
        }
    };
    let mut out = vec![0u32; rows];
    let mut row = 0usize;
    for p in col.pieces() {
        let d = p.chunk.data(ctx)?;
        let range = p.start as usize..(p.start + p.len) as usize;
        match &*d {
            Data::Text { dict, ids, .. } => {
                let per_id: Vec<u32> = dict
                    .iter()
                    .map(|t| code(Key::of(CellRef::Text(t))))
                    .collect();
                for i in range {
                    out[row] = match d.get(i) {
                        CellRef::Text(_) => per_id[ids[i] as usize],
                        v => code(Key::of(v)),
                    };
                    row += 1;
                }
            }
            _ => {
                for i in range {
                    out[row] = code(Key::of(d.get(i)));
                    row += 1;
                }
            }
        }
    }
    for (&r, v) in col.delta() {
        if (r as usize) < rows {
            out[r as usize] = code(Key::of(CellRef::of(v)));
        }
    }
    Ok(out)
}

/// `cols` の値の組が前の行と同じ行を除いた、残す行（元の順）。Excel と同じく大文字・小文字は区別しない。
pub fn unique_rows(ctx: &Context, table: &Table, cols: &[u32]) -> io::Result<Vec<u32>> {
    let rows = table.rows as usize;
    let per_col: Vec<Vec<u32>> = cols
        .par_iter()
        .filter_map(|&c| table.columns.get(c as usize))
        .map(|c| codes(ctx, c, rows))
        .collect::<io::Result<_>>()?;
    let mut keep = Vec::new();
    if per_col.len() <= 4 {
        let mut seen: HashSet<u128, std::hash::BuildHasherDefault<crate::chunk::FxHasher>> =
            HashSet::default();
        for r in 0..rows {
            let k = per_col
                .iter()
                .fold(0u128, |acc, c| acc << 32 | c[r] as u128);
            if seen.insert(k) {
                keep.push(r as u32);
            }
        }
    } else {
        let mut seen: HashSet<Vec<u32>> = HashSet::new();
        for r in 0..rows {
            if seen.insert(per_col.iter().map(|c| c[r]).collect()) {
                keep.push(r as u32);
            }
        }
    }
    Ok(keep)
}

// ---- 値を変える（置換・型の変更） --------------------------------------------------------

/// 列の各値を `f` で変えた列（変えた数も返す）。`f` が `None` を返した値はそのまま。
pub fn map_column(
    ctx: &Context,
    col: &Column,
    f: &(dyn Fn(CellRef<'_>) -> Option<Value> + Sync),
) -> io::Result<(Column, u64)> {
    let mut col = col.clone();
    col.flush(ctx)?;
    let parts: Vec<(Piece, u64)> = col
        .pieces()
        .par_iter()
        .map(|p| -> io::Result<(Piece, u64)> {
            let d = p.chunk.data(ctx)?;
            let mut b = Builder::default();
            let mut changed = 0;
            for i in p.start as usize..(p.start + p.len) as usize {
                let v = d.get(i);
                match f(v) {
                    Some(nv) => {
                        changed += 1;
                        b.push(CellRef::of(&nv));
                    }
                    None => b.push(v),
                }
            }
            if changed == 0 {
                return Ok((p.clone(), 0));
            }
            let c = Chunk::create(ctx, b.finish())?;
            Ok((
                Piece {
                    len: c.rows,
                    chunk: c,
                    start: 0,
                },
                changed,
            ))
        })
        .collect::<io::Result<_>>()?;
    let total = parts.iter().map(|p| p.1).sum();
    let mut out = Column::from_pieces(&col.name, parts.into_iter().map(|p| p.0).collect());
    out.format = col.format.clone();
    Ok((out, total))
}

/// 置換の指定。
#[derive(Clone, Debug)]
pub struct Replace {
    pub find: String,
    pub with: String,
    /// 正規表現（置換後の文字列の `$1`・`${name}` で組を使える）
    pub regex: bool,
    /// 大文字・小文字を区別する
    pub case: bool,
    /// セル全体が一致するものだけ
    pub whole: bool,
}

/// 置換の準備（正規表現を作る）。
pub struct Replacer {
    re: Regex,
    with: String,
    regex: bool,
}

impl Replacer {
    pub fn new(r: &Replace) -> Result<Replacer, String> {
        if r.find.is_empty() {
            return Err("検索する文字列が空です".into());
        }
        let pat = if r.regex {
            r.find.clone()
        } else {
            regex_syntax::escape(&r.find)
        };
        let pat = if r.whole { format!("^(?:{pat})$") } else { pat };
        let re = Regex::builder()
            .syntax(
                syntax::Config::new()
                    .case_insensitive(!r.case)
                    .unicode(true),
            )
            .build(&pat)
            .map_err(|e| format!("正規表現が正しくありません: {e}"))?;
        Ok(Replacer {
            re,
            with: r.with.clone(),
            regex: r.regex,
        })
    }

    /// 文字列を置換する（一致しなければ `None`）。
    pub fn apply(&self, s: &str) -> Option<String> {
        if !self.re.is_match(s) {
            return None;
        }
        let mut out = String::with_capacity(s.len());
        let mut last = 0;
        let mut caps = self.re.create_captures();
        let mut start = 0;
        while start <= s.len() {
            self.re
                .search_captures(&regex_automata::Input::new(s).range(start..), &mut caps);
            let Some(m) = caps.get_match() else {
                break;
            };
            out.push_str(&s[last..m.start()]);
            if self.regex {
                caps.interpolate_string_into(s, &self.with, &mut out);
            } else {
                out.push_str(&self.with);
            }
            last = m.end();
            start = if m.is_empty() {
                // 空の一致は 1 文字進める
                match s[m.end()..].chars().next() {
                    Some(c) => m.end() + c.len_utf8(),
                    None => s.len() + 1,
                }
            } else {
                m.end()
            };
        }
        out.push_str(&s[last.min(s.len())..]);
        Some(out)
    }

    /// 検索だけ（当たるか）。
    pub fn is_match(&self, s: &str) -> bool {
        self.re.is_match(s)
    }
}

/// 表の `cols` 列の文字列の値を置換した表と、置換したセルの数。
pub fn replace(
    ctx: &Context,
    table: &Table,
    cols: &[u32],
    r: &Replace,
) -> Result<(Table, u64), String> {
    let rep = Replacer::new(r)?;
    let mut columns = (*table.columns).clone();
    let mut total = 0;
    for &c in cols {
        let Some(col) = table.columns.get(c as usize) else {
            continue;
        };
        let f = |v: CellRef<'_>| match v {
            CellRef::Text(s) => rep.apply(s).map(|t| Value::text(&t)),
            _ => None,
        };
        let (nc, n) = map_column(ctx, col, &f).map_err(|e| e.to_string())?;
        if n > 0 {
            columns[c as usize] = nc;
            total += n;
        }
    }
    Ok((
        Table {
            columns: Arc::new(columns),
            ..table.clone()
        },
        total,
    ))
}

/// 列の型の変更先。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Convert {
    /// 数値・日付に読める文字列を数値に
    Number,
    /// 数値・真偽値を文字列（標準の表記）に
    Text,
}

/// 列の型を変える（変えたセルの数も返す）。
pub fn convert(
    ctx: &Context,
    col: &Column,
    to: Convert,
    sys: DateSystem,
) -> io::Result<(Column, u64)> {
    let f = move |v: CellRef<'_>| match (to, v) {
        (Convert::Number, CellRef::Text(s)) => match yy_numfmt::parse_input(s, sys) {
            Parsed::Number(n, _) => Some(Value::Number(n)),
            Parsed::Bool(b) => Some(Value::Bool(b)),
            Parsed::Text => None,
        },
        (Convert::Text, CellRef::Number(_) | CellRef::Bool(_)) => {
            Some(Value::text(&v.to_value().general_text()))
        }
        _ => None,
    };
    map_column(ctx, col, &f)
}

// ---- 集計（ステータスバー） ------------------------------------------------------------

/// 値の個数・数値の個数・数値の合計。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Totals {
    pub count: u64,
    pub numbers: u64,
    pub sum: f64,
}

impl Totals {
    pub fn add(&mut self, v: CellRef<'_>, sign: i64) {
        match v {
            CellRef::Empty => {}
            CellRef::Number(x) => {
                self.count = self.count.wrapping_add_signed(sign);
                self.numbers = self.numbers.wrapping_add_signed(sign);
                self.sum += x * sign as f64;
            }
            _ => self.count = self.count.wrapping_add_signed(sign),
        }
    }

    pub fn add_value(&mut self, v: &Value) {
        self.add(CellRef::of(v), 1);
    }

    fn merge(mut self, o: Totals) -> Totals {
        self.count += o.count;
        self.numbers += o.numbers;
        self.sum += o.sum;
        self
    }
}

/// 列の `rows`（表の行）の集計。区間ごとに並列に、数値のチャンクは配列をそのまま足す。
pub fn totals(ctx: &Context, col: &Column, rows: std::ops::Range<u64>) -> io::Result<Totals> {
    let mut starts = Vec::with_capacity(col.pieces().len());
    let mut row = 0u64;
    for p in col.pieces() {
        starts.push(row);
        row += p.len as u64;
    }
    let base = col
        .pieces()
        .par_iter()
        .zip(starts.par_iter())
        .filter(|(p, s)| **s < rows.end && **s + p.len as u64 > rows.start)
        .map(|(p, &s)| -> io::Result<Totals> {
            let d = p.chunk.data(ctx)?;
            let lo = rows.start.saturating_sub(s) as usize;
            let hi = ((rows.end - s) as usize).min(p.len as usize);
            let (a, b) = (p.start as usize + lo, p.start as usize + hi);
            let mut t = Totals::default();
            match &*d {
                Data::Number {
                    vals,
                    present: None,
                } => {
                    t.count = (b - a) as u64;
                    t.numbers = t.count;
                    t.sum = vals[a..b].iter().sum();
                }
                Data::Empty(_) => {}
                d => (a..b).for_each(|i| t.add(d.get(i), 1)),
            }
            Ok(t)
        })
        .try_reduce(Totals::default, |x, y| Ok(x.merge(y)))?;
    // 直したセル: チャンクの値を引いて、直した値を足す
    let mut t = base;
    for (&r, v) in col.delta().range(rows) {
        t.add(CellRef::of(&col.get_base(ctx, r)?), -1);
        t.add(CellRef::of(v), 1);
    }
    Ok(t)
}

// ---- 検索 --------------------------------------------------------------------------

/// 列の中で、`from` 行（表の行）以降で最初に当たる行。
fn find_in_column(
    ctx: &Context,
    col: &Column,
    from: u64,
    rep: &Replacer,
) -> io::Result<Option<u64>> {
    // 差分（直したセル）を先に見る
    let delta_hit = col
        .delta()
        .range(from..)
        .find(|(_, v)| cell_matches(CellRef::of(v), rep))
        .map(|(r, _)| *r);
    let mut row = 0u64;
    let mut starts = Vec::with_capacity(col.pieces().len());
    for p in col.pieces() {
        starts.push(row);
        row += p.len as u64;
    }
    // 区間ごとに並列に探し、最初のものを取る
    let hit = col
        .pieces()
        .par_iter()
        .zip(starts.par_iter())
        .filter(|(p, s)| **s + p.len as u64 > from)
        .map(|(p, &s)| -> io::Result<Option<u64>> {
            let d = p.chunk.data(ctx)?;
            let lo = from.saturating_sub(s) as usize;
            let range = p.start as usize + lo..(p.start + p.len) as usize;
            // 文字列のチャンクは辞書で当たる文字列を先に決める
            if let Data::Text { dict, ids, .. } = &*d {
                let hits: HashSet<u32> = dict
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| rep.is_match(t))
                    .map(|(i, _)| i as u32)
                    .collect();
                if hits.is_empty() {
                    return Ok(None);
                }
                return Ok(range
                    .into_iter()
                    .find(|&i| matches!(d.get(i), CellRef::Text(_)) && hits.contains(&ids[i]))
                    .map(|i| s + (i - p.start as usize) as u64));
            }
            Ok(range
                .into_iter()
                .find(|&i| cell_matches(d.get(i), rep))
                .map(|i| s + (i - p.start as usize) as u64))
        })
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .filter(|r| !col.delta().contains_key(r))
        .min();
    Ok(match (hit, delta_hit) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    })
}

fn cell_matches(v: CellRef<'_>, rep: &Replacer) -> bool {
    match v {
        CellRef::Empty => false,
        CellRef::Text(s) => rep.is_match(s),
        v => rep.is_match(&v.to_value().general_text()),
    }
}

/// 表で `from`（表の行・列）の次から、行の順（同じ行は列の順）に探す。末尾まで来たら先頭に戻る。
pub fn find_next(
    ctx: &Context,
    table: &Table,
    from: (u64, u32),
    r: &Replace,
) -> Result<Option<(u64, u32)>, String> {
    let rep = Replacer::new(r)?;
    let (fr, fc) = from;
    let search = |start_row: u64, first_col: u32| -> Result<Option<(u64, u32)>, String> {
        // 列ごとに最初に当たる行を求め、（行, 列）の小さいものを取る
        let hits: Vec<Option<(u64, u32)>> = (0..table.cols())
            .into_par_iter()
            .map(|c| {
                let begin = if c >= first_col {
                    start_row
                } else {
                    start_row + 1
                };
                find_in_column(ctx, &table.columns[c as usize], begin, &rep)
                    .map(|h| h.map(|r| (r, c)))
            })
            .collect::<io::Result<_>>()
            .map_err(|e| e.to_string())?;
        Ok(hits.into_iter().flatten().min())
    };
    if let Some(h) = search(fr, fc + 1)? {
        return Ok(Some(h));
    }
    // 先頭に戻る
    Ok(search(0, 0)?.filter(|&h| h <= (fr, fc)).or(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(ctx: &Context, cols: Vec<Vec<Value>>, chunk: usize) -> Table {
        let rows = cols[0].len() as u64;
        let columns = cols
            .iter()
            .enumerate()
            .map(|(i, vals)| {
                let pieces = vals
                    .chunks(chunk)
                    .map(|part| {
                        let ch =
                            Chunk::create(ctx, Data::from_values(part.iter().map(CellRef::of)))
                                .unwrap();
                        Piece {
                            len: ch.rows,
                            chunk: ch,
                            start: 0,
                        }
                    })
                    .collect();
                Column::from_pieces(&format!("c{i}"), pieces)
            })
            .collect();
        Table {
            columns: Arc::new(columns),
            rows,
            header: false,
        }
    }

    fn t(s: &str) -> Value {
        Value::text(s)
    }

    fn cells(ctx: &Context, tb: &Table, c: usize) -> Vec<Value> {
        (0..tb.rows)
            .map(|r| tb.columns[c].get(ctx, r).unwrap())
            .collect()
    }

    #[test]
    fn removes_duplicates_case_insensitively() {
        let ctx = Context::for_tests();
        let tb = table(
            &ctx,
            vec![
                vec![t("a"), t("A"), t("b"), t("a"), Value::Empty, Value::Empty],
                vec![
                    1.0.into(),
                    1.0.into(),
                    1.0.into(),
                    2.0.into(),
                    3.0.into(),
                    3.0.into(),
                ],
            ],
            2,
        );
        assert_eq!(unique_rows(&ctx, &tb, &[0]).unwrap(), vec![0, 2, 4]);
        assert_eq!(unique_rows(&ctx, &tb, &[0, 1]).unwrap(), vec![0, 2, 3, 4]);
        let kept = pick(&ctx, &tb, &[0, 2, 4]).unwrap();
        assert_eq!(kept.rows, 3);
        assert_eq!(cells(&ctx, &kept, 0), vec![t("a"), t("b"), Value::Empty]);
    }

    #[test]
    fn replaces_text_with_options() {
        let ctx = Context::for_tests();
        let tb = table(
            &ctx,
            vec![vec![
                t("東京都"),
                t("tokyo"),
                t("Tokyo-2"),
                5.0.into(),
                t("大阪府"),
            ]],
            2,
        );
        let r = |find: &str, with: &str, regex: bool, case: bool, whole: bool| Replace {
            find: find.into(),
            with: with.into(),
            regex,
            case,
            whole,
        };
        let (nt, n) = replace(&ctx, &tb, &[0], &r("tokyo", "TKY", false, false, false)).unwrap();
        assert_eq!(n, 2);
        assert_eq!(cells(&ctx, &nt, 0)[2], t("TKY-2"));
        let (nt, n) = replace(&ctx, &tb, &[0], &r("tokyo", "x", false, true, true)).unwrap();
        assert_eq!(n, 1);
        assert_eq!(cells(&ctx, &nt, 0)[1], t("x"));
        let (nt, n) =
            replace(&ctx, &tb, &[0], &r("(.)(都|府)$", "$1", true, false, false)).unwrap();
        assert_eq!(n, 2);
        assert_eq!(cells(&ctx, &nt, 0)[0], t("東京"));
        assert_eq!(cells(&ctx, &nt, 0)[4], t("大阪"));
        assert_eq!(cells(&ctx, &nt, 0)[3], 5.0.into());
        assert!(Replacer::new(&r("(", "", true, false, false)).is_err());
        let rp = Replacer::new(&r("x*", "-", true, false, false)).unwrap();
        assert_eq!(rp.apply("ab").as_deref(), Some("-a-b-"));
    }

    #[test]
    fn converts_and_finds() {
        let ctx = Context::for_tests();
        let tb = table(
            &ctx,
            vec![
                vec![t("12"), t("x"), t("2026/10/7"), 3.0.into()],
                vec![t("apple"), t("banana"), t("cherry"), t("Banana split")],
            ],
            2,
        );
        let (c, n) = convert(&ctx, &tb.columns[0], Convert::Number, DateSystem::D1900).unwrap();
        assert_eq!(n, 2);
        assert_eq!(c.get(&ctx, 2).unwrap(), 46302.0.into());
        let (c, n) = convert(&ctx, &tb.columns[0], Convert::Text, DateSystem::D1900).unwrap();
        assert_eq!(n, 1);
        assert_eq!(c.get(&ctx, 3).unwrap(), t("3"));
        let q = |s: &str| Replace {
            find: s.into(),
            with: String::new(),
            regex: false,
            case: false,
            whole: false,
        };
        assert_eq!(
            find_next(&ctx, &tb, (0, 0), &q("banana")).unwrap(),
            Some((1, 1))
        );
        assert_eq!(
            find_next(&ctx, &tb, (1, 1), &q("banana")).unwrap(),
            Some((3, 1))
        );
        // 末尾から先頭に戻る
        assert_eq!(
            find_next(&ctx, &tb, (3, 1), &q("banana")).unwrap(),
            Some((1, 1))
        );
        assert_eq!(find_next(&ctx, &tb, (0, 0), &q("3")).unwrap(), Some((3, 0)));
        assert_eq!(find_next(&ctx, &tb, (0, 0), &q("zzz")).unwrap(), None);
    }
}

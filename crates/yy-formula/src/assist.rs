//! 式の入力の補助（15 章 12.3）: 関数の一覧と書き方、入力中の位置の解析（関数名の途中か・どの関数の
//! 何番目の引数か・参照を入れられるか）、式の中の参照（色の枠）、`$` の切り替え（F4）。
//!
//! 位置はすべて文字（`char`）の番号で数える。

use crate::parse::{Area, AreaKind};
use crate::{MAX_COL, col_name};

/// 関数の説明と書き方。
#[derive(Debug, PartialEq, Eq)]
pub struct FuncInfo {
    pub name: &'static str,
    pub desc: &'static str,
    /// 引数（`[` で始まるものは省略できる）
    pub params: &'static [&'static str],
    /// 繰り返す引数（`params` の中の繰り返しの始めの番号・数）。最後の引数の後に「…」を付ける
    pub repeat: Option<(usize, usize)>,
}

/// 使える関数（名前の順）。
pub const FUNCTIONS: &[FuncInfo] = &[
    FuncInfo {
        name: "ABS",
        desc: "数値の絶対値を返します。",
        params: &["数値"],
        repeat: None,
    },
    FuncInfo {
        name: "CONCAT",
        desc: "文字列をつなげます（範囲はセルの順に）。",
        params: &["文字列1", "[文字列2]"],
        repeat: Some((1, 1)),
    },
    FuncInfo {
        name: "COUNT",
        desc: "数値のセルの数を返します（範囲の中の文字列・空・真偽値・エラーは数えません）。",
        params: &["値1", "[値2]"],
        repeat: Some((1, 1)),
    },
    FuncInfo {
        name: "COUNTIFS",
        desc: "すべての条件に合うセルの数を返します。",
        params: &["条件範囲1", "条件1", "[条件範囲2", "条件2]"],
        repeat: Some((2, 2)),
    },
    FuncInfo {
        name: "PRODUCT",
        desc: "数値の積を返します。",
        params: &["数値1", "[数値2]"],
        repeat: Some((1, 1)),
    },
    FuncInfo {
        name: "SUM",
        desc: "数値の合計を返します（範囲の中の文字列・真偽値・空は無視します）。",
        params: &["数値1", "[数値2]"],
        repeat: Some((1, 1)),
    },
    FuncInfo {
        name: "SUMIFS",
        desc: "すべての条件に合う行の、合計範囲の数値の合計を返します。",
        params: &["合計範囲", "条件範囲1", "条件1", "[条件範囲2", "条件2]"],
        repeat: Some((3, 2)),
    },
    FuncInfo {
        name: "TEXTJOIN",
        desc: "区切り文字をはさんで文字列をつなげます。",
        params: &["区切り文字", "空のセルを無視", "文字列1", "[文字列2]"],
        repeat: Some((3, 1)),
    },
    FuncInfo {
        name: "TEXTSPLIT",
        desc: "文字列を区切り文字で分け、行・列に並べます。",
        params: &[
            "文字列",
            "列の区切り",
            "[行の区切り]",
            "[空を無視]",
            "[一致モード]",
            "[埋める値]",
        ],
        repeat: None,
    },
    FuncInfo {
        name: "XLOOKUP",
        desc: "検索範囲から検索値を探し、戻り範囲の同じ位置の値を返します。",
        params: &[
            "検索値",
            "検索範囲",
            "戻り範囲",
            "[見つからない場合]",
            "[一致モード]",
            "[検索モード]",
        ],
        repeat: None,
    },
];

impl FuncInfo {
    /// 名前から（大文字・小文字を区別しない。`_xlfn.` と `CONCATENATE` も）。
    pub fn find(name: &str) -> Option<&'static FuncInfo> {
        let up = name.to_ascii_uppercase();
        let up = up.strip_prefix("_XLFN.").unwrap_or(&up);
        let up = if up == "CONCATENATE" { "CONCAT" } else { up };
        FUNCTIONS.iter().find(|f| f.name == up)
    }

    /// 名前が `prefix` で始まる関数（大文字・小文字を区別しない）。
    pub fn complete(prefix: &str) -> Vec<&'static FuncInfo> {
        if prefix.is_empty() {
            return Vec::new();
        }
        let up = prefix.to_ascii_uppercase();
        FUNCTIONS
            .iter()
            .filter(|f| f.name.starts_with(&up))
            .collect()
    }

    /// `i` 番目（0 始まり）の引数に当たる `params` の番号（多すぎれば `None`）。
    pub fn param_index(&self, i: usize) -> Option<usize> {
        if i < self.params.len() {
            return Some(i);
        }
        let (start, len) = self.repeat?;
        Some(start + (i - start) % len)
    }

    /// 書き方を部品に分けたもの（文字列と、引数なら `params` の番号）。
    /// 例: `SUMIFS(` `合計範囲` `, ` … `, …)`。
    pub fn signature(&self) -> Vec<(String, Option<usize>)> {
        let mut out = vec![(format!("{}(", self.name), None)];
        for (i, p) in self.params.iter().enumerate() {
            if i > 0 {
                out.push((", ".into(), None));
            }
            out.push(((*p).into(), Some(i)));
        }
        out.push((
            if self.repeat.is_some() { ", …)" } else { ")" }.into(),
            None,
        ));
        out
    }
}

/// 入力中の位置の解析の結果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Typing {
    /// 入力中の関数名の始めの位置と、それまでの文字
    pub word: Option<(usize, String)>,
    /// 囲んでいる関数の名前（大文字）と、何番目（0 始まり）の引数か
    pub call: Option<(String, usize)>,
    /// 参照を入れられる位置か（演算子・`(`・`,`・`=` の直後）
    pub can_ref: bool,
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

/// `text` の `caret`（文字の番号）での入力の様子。`=` で始まらなければ何もない。
pub fn typing(text: &str, caret: usize) -> Typing {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    if chars.first() != Some(&'=') {
        return Typing::default();
    }
    // かっこの深さ（関数名・引数の番号。`{` は配列定数）
    enum Frame {
        Call(Option<String>, usize),
        Brace,
    }
    let mut stack: Vec<Frame> = Vec::new();
    let mut in_str = false;
    let mut in_quote = false;
    let mut i = 1;
    while i < caret {
        let c = chars[i];
        if in_str {
            if c == '"' {
                in_str = false;
            }
        } else if in_quote {
            if c == '\'' {
                in_quote = false;
            }
        } else {
            match c {
                '"' => in_str = true,
                '\'' => in_quote = true,
                '(' => {
                    let mut s = i;
                    while s > 1 && is_name_char(chars[s - 1]) {
                        s -= 1;
                    }
                    let name: String = chars[s..i].iter().collect();
                    let name = name
                        .chars()
                        .next()
                        .filter(|c| c.is_alphabetic())
                        .map(|_| name.to_ascii_uppercase());
                    stack.push(Frame::Call(name, 0));
                }
                '{' => stack.push(Frame::Brace),
                ')' | '}' => {
                    stack.pop();
                }
                ',' => {
                    if let Some(Frame::Call(_, n)) = stack.last_mut() {
                        *n += 1;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    let call = stack.iter().rev().find_map(|f| match f {
        Frame::Call(Some(name), n) => Some((name.clone(), *n)),
        _ => None,
    });
    if in_str || in_quote {
        return Typing {
            call,
            ..Typing::default()
        };
    }
    // 入力中の名前
    let mut ws = caret;
    while ws > 1 && is_name_char(chars[ws - 1]) {
        ws -= 1;
    }
    let before =
        |at: usize| -> Option<char> { chars[1..at].iter().rev().find(|c| **c != ' ').copied() };
    let after_op = |at: usize| -> bool {
        match before(at) {
            None => true,
            Some(c) => "(,+-*/^&<>=:".contains(c),
        }
    };
    let word = (ws < caret && chars[ws].is_alphabetic() && {
        // 直前が演算子など（`Sheet1!` や `A1:` の続きではない）
        let prev = if ws > 1 { Some(chars[ws - 1]) } else { None };
        match prev {
            None => true,
            Some(c) => "(,+-*/^&<>= ".contains(c),
        }
    })
    .then(|| (ws, chars[ws..caret].iter().collect::<String>()));
    // `A1:` の後も（範囲の終わり）
    let can_ref = after_op(caret);
    Typing {
        word,
        call,
        can_ref,
    }
}

/// 式の中の参照（位置・シート名・範囲）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefSpan {
    pub start: usize,
    pub end: usize,
    /// `Sheet2!A1` のシート名
    pub sheet: Option<String>,
    pub area: Area,
}

/// 参照の部品（`$A$1`・`$A`・`$1`）。
enum Part {
    Cell(u64, u32, bool, bool),
    Col(u32, bool),
    Row(u64, bool),
}

/// `chars[i..]` の参照の部品と、その終わり。
fn part(chars: &[char], i: usize) -> Option<(Part, usize)> {
    let mut j = i;
    let col_abs = chars.get(j) == Some(&'$');
    if col_abs {
        j += 1;
    }
    let ls = j;
    while j < chars.len() && j - ls < 3 && chars[j].is_ascii_alphabetic() {
        j += 1;
    }
    let letters: String = chars[ls..j].iter().collect();
    let row_abs_at = j;
    let row_abs = chars.get(j) == Some(&'$');
    if row_abs {
        j += 1;
    }
    let ds = j;
    while j < chars.len() && chars[j].is_ascii_digit() && j - ds < 8 {
        j += 1;
    }
    let digits: String = chars[ds..j].iter().collect();
    let col = (!letters.is_empty())
        .then(|| {
            letters.bytes().try_fold(0u32, |n, b| {
                Some(n * 26 + (b.to_ascii_uppercase() - b'A') as u32 + 1)
            })
        })
        .flatten()
        .map(|n| n - 1)
        .filter(|&c| c <= MAX_COL);
    let row = digits
        .parse::<u64>()
        .ok()
        .filter(|&r| r >= 1)
        .map(|r| r - 1);
    match (letters.is_empty(), col, digits.is_empty(), row) {
        (false, Some(c), false, Some(r)) => Some((Part::Cell(r, c, row_abs, col_abs), j)),
        (false, Some(c), true, _) if !row_abs => Some((Part::Col(c, col_abs), row_abs_at)),
        (true, _, false, Some(r)) if !col_abs || row_abs => {
            // `$1` は行（`$` は行の絶対）
            Some((Part::Row(r, col_abs || row_abs), j))
        }
        _ => None,
    }
}

/// 式の中の参照をすべて（文字列・名前の一部は除く）。
pub fn refs_in(text: &str) -> Vec<RefSpan> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    if chars.first() != Some(&'=') {
        return out;
    }
    let mut i = 1;
    let mut sheet: Option<(String, usize)> = None;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += 1;
            }
            i += 1;
            continue;
        }
        if c == '\'' {
            let s = i + 1;
            i += 1;
            let mut name = String::new();
            // `''` は名前の中の `'`
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        name.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                name.push(chars[i]);
                i += 1;
            }
            i += 1;
            if chars.get(i) == Some(&'!') {
                sheet = Some((name, s - 1));
                i += 1;
            }
            continue;
        }
        let boundary = i == 1 || !(is_name_char(chars[i - 1]) || chars[i - 1] == '$');
        if !(boundary || sheet.as_ref().is_some_and(|s| s.1 < i)) {
            i += 1;
            continue;
        }
        if !(c == '$' || c.is_ascii_alphanumeric()) {
            sheet = None;
            i += 1;
            continue;
        }
        // 名前（シート名か関数名か）を先に読む
        let mut j = i;
        while j < chars.len() && is_name_char(chars[j]) {
            j += 1;
        }
        if chars.get(j) == Some(&'!') && chars[i] != '$' {
            sheet = Some((chars[i..j].iter().collect(), i));
            i = j + 1;
            continue;
        }
        let start = sheet.as_ref().map(|s| s.1).unwrap_or(i);
        let mut found = None;
        if let Some((p1, e1)) = part(&chars, i) {
            let second = (chars.get(e1) == Some(&':'))
                .then(|| part(&chars, e1 + 1))
                .flatten();
            let (area, end) = match (p1, second) {
                (Part::Cell(r, c, ar, ac), None) => (
                    Some(Area {
                        r0: r,
                        c0: c,
                        r1: r,
                        c1: c,
                        abs: [ar, ac, ar, ac],
                        kind: AreaKind::Cell,
                    }),
                    e1,
                ),
                (Part::Cell(r0, c0, ar0, ac0), Some((Part::Cell(r1, c1, ar1, ac1), e2))) => (
                    Some(Area {
                        r0: r0.min(r1),
                        c0: c0.min(c1),
                        r1: r0.max(r1),
                        c1: c0.max(c1),
                        abs: [ar0, ac0, ar1, ac1],
                        kind: AreaKind::Range,
                    }),
                    e2,
                ),
                (Part::Col(c0, a0), Some((Part::Col(c1, a1), e2))) => (
                    Some(Area {
                        r0: 0,
                        c0: c0.min(c1),
                        r1: u64::MAX,
                        c1: c0.max(c1),
                        abs: [false, a0, false, a1],
                        kind: AreaKind::Cols,
                    }),
                    e2,
                ),
                (Part::Row(r0, a0), Some((Part::Row(r1, a1), e2))) => (
                    Some(Area {
                        r0: r0.min(r1),
                        c0: 0,
                        r1: r0.max(r1),
                        c1: u32::MAX,
                        abs: [a0, false, a1, false],
                        kind: AreaKind::Rows,
                    }),
                    e2,
                ),
                _ => (None, e1),
            };
            // 名前の続き・関数名なら参照ではない
            let ends_ok =
                !matches!(chars.get(end), Some(c) if is_name_char(*c) || *c == '(' || *c == '$');
            if let Some(a) = area.filter(|_| ends_ok) {
                found = Some((a, end));
            }
        }
        match found {
            Some((area, end)) => {
                out.push(RefSpan {
                    start,
                    end,
                    sheet: sheet.take().map(|s| s.0),
                    area,
                });
                i = end;
            }
            None => {
                sheet = None;
                i = j.max(i + 1);
            }
        }
    }
    out
}

/// シート名を参照の前に付ける形に（`Sheet2!`。名前に記号・空白がある、数字で始まる、参照に見える
/// ときは `'売上 2026'!`。`'` は `''`）。
pub fn sheet_prefix(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let plain = chars
        .first()
        .is_some_and(|c| c.is_alphabetic() || *c == '_')
        && chars.iter().all(|c| c.is_alphanumeric() || *c == '_')
        && !matches!(part(&chars, 0), Some((_, end)) if end == chars.len())
        && !name.eq_ignore_ascii_case("TRUE")
        && !name.eq_ignore_ascii_case("FALSE");
    if plain {
        format!("{name}!")
    } else {
        format!("'{}'!", name.replace('\'', "''"))
    }
}

/// 範囲を式の参照の書き方に（`A1`・`A1:B3`・`A:C`・`1:3`）。
pub fn area_text(a: &Area) -> String {
    let d = |abs: bool| if abs { "$" } else { "" };
    let cell =
        |r: u64, c: u32, ar: bool, ac: bool| format!("{}{}{}{}", d(ac), col_name(c), d(ar), r + 1);
    match a.kind {
        AreaKind::Cell => cell(a.r0, a.c0, a.abs[0], a.abs[1]),
        AreaKind::Range => format!(
            "{}:{}",
            cell(a.r0, a.c0, a.abs[0], a.abs[1]),
            cell(a.r1, a.c1, a.abs[2], a.abs[3])
        ),
        AreaKind::Cols => format!(
            "{}{}:{}{}",
            d(a.abs[1]),
            col_name(a.c0),
            d(a.abs[3]),
            col_name(a.c1)
        ),
        AreaKind::Rows => format!("{}{}:{}{}", d(a.abs[0]), a.r0 + 1, d(a.abs[2]), a.r1 + 1),
    }
}

/// 位置 `caret` にある（またはすぐ前で終わる）参照の `$` を、Excel の F4 と同じ順に切り替える
/// （`A1` → `$A$1` → `A$1` → `$A1` → `A1`）。新しい文字列と、参照の始め・終わり。
pub fn toggle_abs(text: &str, caret: usize) -> Option<(String, usize, usize)> {
    let span = refs_in(text)
        .into_iter()
        .find(|r| r.start <= caret && caret <= r.end)?;
    let mut a = span.area;
    let next = |row: bool, col: bool| match (row, col) {
        (false, false) => (true, true),
        (true, true) => (true, false),
        (true, false) => (false, true),
        (false, true) => (false, false),
    };
    match a.kind {
        AreaKind::Cols => {
            let c = !a.abs[1];
            a.abs = [false, c, false, c];
        }
        AreaKind::Rows => {
            let r = !a.abs[0];
            a.abs = [r, false, r, false];
        }
        _ => {
            let (r, c) = next(a.abs[0], a.abs[1]);
            a.abs = [r, c, r, c];
        }
    }
    let chars: Vec<char> = text.chars().collect();
    // シート名は残す
    let prefix_end = match &span.sheet {
        Some(_) => {
            let bang = chars[span.start..span.end].iter().position(|c| *c == '!')?;
            span.start + bang + 1
        }
        None => span.start,
    };
    let new_ref = area_text(&a);
    let mut out: String = chars[..prefix_end].iter().collect();
    out.push_str(&new_ref);
    let end = prefix_end + new_ref.chars().count();
    out.extend(&chars[span.end..]);
    Some((out, span.start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caret_at_end(s: &str) -> Typing {
        typing(s, s.chars().count())
    }

    #[test]
    fn typing_function_names_and_arguments() {
        let t = caret_at_end("=SU");
        assert_eq!(t.word, Some((1, "SU".into())));
        assert!(t.call.is_none());
        let t = caret_at_end("=1+sumifs(A:A,B:B,\"東京\",C");
        assert_eq!(t.call, Some(("SUMIFS".into(), 3)));
        assert_eq!(t.word, Some((23, "C".into())));
        // 内側の関数
        let t = caret_at_end("=SUMIFS(A:A,B:B,XLOOKUP(1,");
        assert_eq!(t.call, Some(("XLOOKUP".into(), 1)));
        // 閉じれば外側
        let t = caret_at_end("=SUMIFS(A:A,B:B,XLOOKUP(1,C:C,D:D),");
        assert_eq!(t.call, Some(("SUMIFS".into(), 3)));
        // 文字列の中の , ( は数えない
        let t = caret_at_end("=CONCAT(\"a,(b\",");
        assert_eq!(t.call, Some(("CONCAT".into(), 1)));
        assert!(t.can_ref);
        let t = caret_at_end("=CONCAT(\"a,");
        assert!(!t.can_ref);
        assert!(t.word.is_none());
        // 配列定数の , は引数ではない
        let t = caret_at_end("=XLOOKUP({1,2,3},");
        assert_eq!(t.call, Some(("XLOOKUP".into(), 1)));
        // 式でなければ何もない
        assert_eq!(caret_at_end("SU"), Typing::default());
    }

    #[test]
    fn typing_reference_positions() {
        for s in ["=", "=A1+", "=SUM(", "=X(1, ", "=A1:", "=1*(", "=a&"] {
            assert!(caret_at_end(s).can_ref, "{s}");
        }
        for s in ["=A1", "=1", "=X(1)", "=\"a"] {
            assert!(!caret_at_end(s).can_ref, "{s}");
        }
        // 途中の位置
        assert!(typing("=A1+B2", 4).can_ref);
        assert!(!typing("=A1+B2", 3).can_ref);
        // シート名の後の名前は関数名ではない
        assert!(caret_at_end("=Sheet2!A").word.is_none());
    }

    #[test]
    fn completion_and_signature() {
        let names: Vec<_> = FuncInfo::complete("co").iter().map(|f| f.name).collect();
        assert_eq!(names, ["CONCAT", "COUNT", "COUNTIFS"]);
        let names: Vec<_> = FuncInfo::complete("su").iter().map(|f| f.name).collect();
        assert_eq!(names, ["SUM", "SUMIFS"]);
        assert!(FuncInfo::complete("").is_empty());
        assert_eq!(FuncInfo::find("concatenate").unwrap().name, "CONCAT");
        assert_eq!(FuncInfo::find("_xlfn.XLOOKUP").unwrap().name, "XLOOKUP");
        let f = FuncInfo::find("SUMIFS").unwrap();
        assert_eq!(f.param_index(0), Some(0));
        assert_eq!(f.param_index(4), Some(4));
        assert_eq!(f.param_index(5), Some(3));
        assert_eq!(f.param_index(6), Some(4));
        assert_eq!(f.param_index(7), Some(3));
        let c = FuncInfo::find("COUNTIFS").unwrap();
        assert_eq!(c.param_index(4), Some(2));
        assert_eq!(c.param_index(5), Some(3));
        let x = FuncInfo::find("XLOOKUP").unwrap();
        assert_eq!(x.param_index(6), None);
        let sig: String = f.signature().into_iter().map(|p| p.0).collect();
        assert_eq!(
            sig,
            "SUMIFS(合計範囲, 条件範囲1, 条件1, [条件範囲2, 条件2], …)"
        );
        // 名前の順（一覧の並び）
        assert!(FUNCTIONS.windows(2).all(|w| w[0].name < w[1].name));
    }

    #[test]
    fn references_in_formula() {
        let r = refs_in("=SUMIFS(B:B,$C$2:c10,\"A1\",Sheet2!A1)+ab12+LOG10(1)+3:5+X1Y");
        let texts: Vec<String> = r.iter().map(|s| area_text(&s.area)).collect();
        assert_eq!(texts, ["B:B", "$C$2:C10", "A1", "AB12", "3:5"]);
        assert_eq!(r[2].sheet.as_deref(), Some("Sheet2"));
        assert_eq!((r[0].start, r[0].end), (8, 11));
        assert_eq!(r[2].start, 26);
        let r = refs_in("='My Sheet'!B2*2");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].sheet.as_deref(), Some("My Sheet"));
        assert_eq!(r[0].start, 1);
        assert!(refs_in("A1").is_empty());
    }

    #[test]
    fn shift_by_rows_and_columns() {
        let e = crate::parse("=A1+$B2+C$3+$D$4+SUM(E:E)+SUM(5:5)").unwrap();
        let t = crate::formula_text(&crate::shift_by(&e, 2, 1));
        assert_eq!(t, "=B3+$B4+D$3+$D$4+SUM(F:F)+SUM(7:7)");
        let t = crate::formula_text(&crate::shift_by(&e, 0, -1));
        assert_eq!(t, "=#REF!+$B2+B$3+$D$4+SUM(D:D)+SUM(5:5)");
    }

    #[test]
    fn sheet_prefixes() {
        assert_eq!(sheet_prefix("Sheet2"), "Sheet2!");
        assert_eq!(sheet_prefix("売上"), "売上!");
        assert_eq!(sheet_prefix("売上 2026"), "'売上 2026'!");
        assert_eq!(sheet_prefix("2026"), "'2026'!");
        assert_eq!(sheet_prefix("AB12"), "'AB12'!");
        assert_eq!(sheet_prefix("It's"), "'It''s'!");
        // 作った参照を読める（式の解析と参照の取り出し）
        for name in ["Sheet2", "売上 2026", "It's", "AB12"] {
            let f = format!("={}B3+1", sheet_prefix(name));
            crate::parse(&f).unwrap();
            let r = refs_in(&f);
            assert_eq!(r.len(), 1, "{f}");
            assert_eq!(r[0].sheet.as_deref(), Some(name));
            assert_eq!(area_text(&r[0].area), "B3");
        }
    }

    #[test]
    fn f4_cycles_dollars() {
        let mut s = "=A1+B2".to_string();
        let mut seen = Vec::new();
        for _ in 0..4 {
            let (t, a, b) = toggle_abs(&s, 2).unwrap();
            assert_eq!(a, 1);
            seen.push(t.chars().skip(a).take(b - a).collect::<String>());
            s = t;
        }
        assert_eq!(seen, ["$A$1", "A$1", "$A1", "A1"]);
        assert_eq!(toggle_abs("=SUM(B:C)", 8).unwrap().0, "=SUM($B:$C)");
        assert_eq!(toggle_abs("=A1:B2", 6).unwrap().0, "=$A$1:$B$2");
        assert_eq!(toggle_abs("=Sheet2!A1", 9).unwrap().0, "=Sheet2!$A$1");
        assert!(toggle_abs("=1+2", 2).is_none());
    }
}

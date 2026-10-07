//! 数式の解析（15 章 7.1）。
//!
//! 優先順位（Excel と同じ。上ほど先に結び付く）: 参照の `:`、単項の `-`・`+`、`%`、`^`、`*`・`/`、
//! `+`・`-`、`&`、比較。`-2^2` は `4`。

use std::fmt;
use std::sync::Arc;

use crate::Error;

/// 範囲（両端を含む）。列全体は行が `0..=u64::MAX`、行全体は列が `0..=u32::MAX`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub r0: u64,
    pub c0: u32,
    pub r1: u64,
    pub c1: u32,
    /// `$` の付いた部分（行の始め・列の始め・行の終わり・列の終わり）
    pub abs: [bool; 4],
    /// 書き方
    pub kind: AreaKind,
}

/// 範囲の書き方。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaKind {
    Cell,
    Range,
    /// `A:C`
    Cols,
    /// `1:3`
    Rows,
}

impl Area {
    pub fn cell(r: u64, c: u32) -> Area {
        Area {
            r0: r,
            c0: c,
            r1: r,
            c1: c,
            abs: [false; 4],
            kind: AreaKind::Cell,
        }
    }

    pub fn rows(&self) -> u64 {
        self.r1 - self.r0 + 1
    }

    pub fn cols(&self) -> u32 {
        self.c1 - self.c0 + 1
    }

    pub fn contains(&self, r: u64, c: u32) -> bool {
        (self.r0..=self.r1).contains(&r) && (self.c0..=self.c1).contains(&c)
    }

    pub fn intersects(&self, o: &Area) -> bool {
        self.r0 <= o.r1 && o.r0 <= self.r1 && self.c0 <= o.c1 && o.c0 <= self.c1
    }
}

/// 参照。
#[derive(Clone, Debug, PartialEq)]
pub struct Ref {
    /// シートの名前（`None` は式のあるシート）
    pub sheet: Option<Arc<str>>,
    pub area: Area,
}

/// 二項演算子。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// 関数。
#[derive(Clone, Debug, PartialEq)]
pub enum Func {
    Abs,
    Product,
    Sum,
    Count,
    Sumifs,
    Countifs,
    Xlookup,
    Concat,
    Textjoin,
    Textsplit,
    Max,
    Min,
    Average,
    Median,
    /// `PERCENTILE`・`PERCENTILE.INC`（`inc`）と `PERCENTILE.EXC`。`name` は書いたときの名前
    Percentile {
        inc: bool,
        name: &'static str,
    },
    /// `ROUNDUP`（`true`）・`ROUNDDOWN`（`false`）
    RoundAway(bool),
    /// `LOW-VALUE()`（COBOL の LOW-VALUE。項目をすべて X'00' にする）
    LowValue,
    /// `HIGH-VALUE()`（COBOL の HIGH-VALUE。項目をすべて X'FF' にする）
    HighValue,
    /// 未登録（`#NAME?`）
    Unknown(Arc<str>),
}

impl Func {
    fn from_name(name: &str) -> Func {
        let up = name.to_ascii_uppercase();
        let up = up.strip_prefix("_XLFN.").unwrap_or(&up);
        match up {
            "ABS" => Func::Abs,
            "PRODUCT" => Func::Product,
            "SUM" => Func::Sum,
            "COUNT" => Func::Count,
            "SUMIFS" => Func::Sumifs,
            "COUNTIFS" => Func::Countifs,
            "XLOOKUP" => Func::Xlookup,
            "CONCAT" | "CONCATENATE" => Func::Concat,
            "TEXTJOIN" => Func::Textjoin,
            "TEXTSPLIT" => Func::Textsplit,
            "MAX" => Func::Max,
            "MIN" => Func::Min,
            "AVERAGE" => Func::Average,
            "MEDIAN" => Func::Median,
            "PERCENTILE" => Func::Percentile {
                inc: true,
                name: "PERCENTILE",
            },
            "PERCENTILE.INC" => Func::Percentile {
                inc: true,
                name: "PERCENTILE.INC",
            },
            "PERCENTILE.EXC" => Func::Percentile {
                inc: false,
                name: "PERCENTILE.EXC",
            },
            "ROUNDUP" => Func::RoundAway(true),
            "ROUNDDOWN" => Func::RoundAway(false),
            "LOW-VALUE" | "LOW-VALUES" => Func::LowValue,
            "HIGH-VALUE" | "HIGH-VALUES" => Func::HighValue,
            _ => Func::Unknown(Arc::from(name)),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Func::Abs => "ABS",
            Func::Product => "PRODUCT",
            Func::Sum => "SUM",
            Func::Count => "COUNT",
            Func::Sumifs => "SUMIFS",
            Func::Countifs => "COUNTIFS",
            Func::Xlookup => "XLOOKUP",
            Func::Concat => "CONCAT",
            Func::Textjoin => "TEXTJOIN",
            Func::Textsplit => "TEXTSPLIT",
            Func::Max => "MAX",
            Func::Min => "MIN",
            Func::Average => "AVERAGE",
            Func::Median => "MEDIAN",
            Func::Percentile { name, .. } => name,
            Func::RoundAway(true) => "ROUNDUP",
            Func::RoundAway(false) => "ROUNDDOWN",
            Func::LowValue => "LOW-VALUE",
            Func::HighValue => "HIGH-VALUE",
            Func::Unknown(n) => n,
        }
    }
}

/// 式。
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(f64),
    Text(Arc<str>),
    Bool(bool),
    Err(Error),
    Ref(Ref),
    /// 配列定数（行ごと）
    Array(Vec<Vec<Expr>>),
    Neg(Box<Expr>),
    /// 単項の `+`（値は変えない）
    Plus(Box<Expr>),
    Percent(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
    /// かっこ（書き戻すときのため）
    Paren(Box<Expr>),
    /// 省略した引数（`XLOOKUP(a,b,c,,1)` の 4 つ目）
    Missing,
}

/// 解析の誤り。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub pos: usize,
    pub msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}（{} 文字目）", self.msg, self.pos + 1)
    }
}

impl std::error::Error for ParseError {}

/// 数式を解析する（先頭の `=` はあってもなくてもよい）。
pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let s: Vec<char> = src.chars().collect();
    let mut p = Parser { s: &s, i: 0 };
    p.skip_ws();
    if p.peek() == Some('=') {
        p.i += 1;
    }
    let e = p.expr()?;
    p.skip_ws();
    if p.i < s.len() {
        return Err(p.err("式の終わりに余分な文字があります"));
    }
    Ok(e)
}

struct Parser<'a> {
    s: &'a [char],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, msg: &str) -> ParseError {
        ParseError {
            pos: self.i,
            msg: msg.to_owned(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn peek_at(&self, k: usize) -> Option<char> {
        self.s.get(self.i + k).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: char) -> bool {
        self.skip_ws();
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Result<Expr, ParseError> {
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Expr, ParseError> {
        let mut l = self.concat()?;
        loop {
            self.skip_ws();
            let op = match (self.peek(), self.peek_at(1)) {
                (Some('<'), Some('>')) => (BinOp::Ne, 2),
                (Some('<'), Some('=')) => (BinOp::Le, 2),
                (Some('>'), Some('=')) => (BinOp::Ge, 2),
                (Some('<'), _) => (BinOp::Lt, 1),
                (Some('>'), _) => (BinOp::Gt, 1),
                (Some('='), _) => (BinOp::Eq, 1),
                _ => return Ok(l),
            };
            self.i += op.1;
            let r = self.concat()?;
            l = Expr::Bin(op.0, Box::new(l), Box::new(r));
        }
    }

    fn concat(&mut self) -> Result<Expr, ParseError> {
        let mut l = self.additive()?;
        while self.eat('&') {
            let r = self.additive()?;
            l = Expr::Bin(BinOp::Concat, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn additive(&mut self) -> Result<Expr, ParseError> {
        let mut l = self.multiplicative()?;
        loop {
            let op = if self.eat('+') {
                BinOp::Add
            } else if self.eat('-') {
                BinOp::Sub
            } else {
                return Ok(l);
            };
            let r = self.multiplicative()?;
            l = Expr::Bin(op, Box::new(l), Box::new(r));
        }
    }

    fn multiplicative(&mut self) -> Result<Expr, ParseError> {
        let mut l = self.power()?;
        loop {
            let op = if self.eat('*') {
                BinOp::Mul
            } else if self.eat('/') {
                BinOp::Div
            } else {
                return Ok(l);
            };
            let r = self.power()?;
            l = Expr::Bin(op, Box::new(l), Box::new(r));
        }
    }

    fn power(&mut self) -> Result<Expr, ParseError> {
        let mut l = self.percent()?;
        while self.eat('^') {
            let r = self.percent()?;
            l = Expr::Bin(BinOp::Pow, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn percent(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.unary()?;
        while self.eat('%') {
            e = Expr::Percent(Box::new(e));
        }
        Ok(e)
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        if self.eat('-') {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        if self.eat('+') {
            return Ok(Expr::Plus(Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        self.skip_ws();
        let Some(c) = self.peek() else {
            return Err(self.err("式が途中で終わっています"));
        };
        match c {
            '(' => {
                self.i += 1;
                let e = self.expr()?;
                if !self.eat(')') {
                    return Err(self.err("かっこが閉じていません"));
                }
                Ok(Expr::Paren(Box::new(e)))
            }
            '"' => self.string().map(Expr::Text),
            '{' => self.array(),
            '#' => {
                let start = self.i;
                while let Some(c) = self.peek() {
                    if c.is_alphanumeric() || matches!(c, '#' | '/' | '!' | '?') {
                        self.i += 1;
                        if matches!(c, '!' | '?') {
                            break;
                        }
                        // #N/A は A で終わる
                        let t: String = self.s[start..self.i].iter().collect();
                        if t.eq_ignore_ascii_case("#N/A") {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                let t: String = self.s[start..self.i].iter().collect();
                Error::parse(&t)
                    .map(Expr::Err)
                    .ok_or_else(|| self.err("エラー値の書き方が正しくありません"))
            }
            _ => {
                if let Some(r) = self.reference()? {
                    return Ok(Expr::Ref(r));
                }
                if c.is_ascii_digit() || c == '.' {
                    return self.number();
                }
                self.ident()
            }
        }
    }

    fn string(&mut self) -> Result<Arc<str>, ParseError> {
        self.i += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(self.err("文字列が閉じていません")),
                Some('"') if self.peek_at(1) == Some('"') => {
                    out.push('"');
                    self.i += 2;
                }
                Some('"') => {
                    self.i += 1;
                    return Ok(Arc::from(out));
                }
                Some(c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    fn number(&mut self) -> Result<Expr, ParseError> {
        let start = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '.') {
            self.i += 1;
        }
        if matches!(self.peek(), Some('e' | 'E'))
            && matches!(self.peek_at(1), Some(c) if c.is_ascii_digit() || c == '+' || c == '-')
        {
            self.i += 2;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.i += 1;
            }
        }
        let t: String = self.s[start..self.i].iter().collect();
        t.parse::<f64>().map(Expr::Num).map_err(|_| ParseError {
            pos: start,
            msg: format!("数値「{t}」が正しくありません"),
        })
    }

    fn array(&mut self) -> Result<Expr, ParseError> {
        self.i += 1;
        let mut rows = vec![Vec::new()];
        loop {
            let e = self.unary()?;
            if !matches!(
                e,
                Expr::Num(_) | Expr::Text(_) | Expr::Bool(_) | Expr::Err(_) | Expr::Neg(_)
            ) {
                return Err(self.err("配列定数には定数だけを書けます"));
            }
            rows.last_mut().expect("row").push(e);
            if self.eat(',') {
                continue;
            }
            if self.eat(';') {
                rows.push(Vec::new());
                continue;
            }
            if self.eat('}') {
                break;
            }
            return Err(self.err("配列定数が閉じていません"));
        }
        let w = rows[0].len();
        if rows.iter().any(|r| r.len() != w) {
            return Err(self.err("配列定数の各行の数が揃っていません"));
        }
        Ok(Expr::Array(rows))
    }

    fn ident(&mut self) -> Result<Expr, ParseError> {
        let start = self.i;
        while matches!(self.peek(), Some(c) if c.is_alphanumeric() || c == '_' || c == '.') {
            self.i += 1;
        }
        if self.i == start {
            return Err(self.err("式の書き方が正しくありません"));
        }
        let mut name: String = self.s[start..self.i].iter().collect();
        // COBOL の表意定数の関数（`LOW-VALUE(`・`HIGH-VALUES(`）は名前に `-` を含む
        if matches!(name.to_ascii_uppercase().as_str(), "LOW" | "HIGH") {
            for tail in ["-VALUES", "-VALUE"] {
                let n = tail.chars().count();
                let next: String = self.s[self.i..(self.i + n).min(self.s.len())]
                    .iter()
                    .collect();
                if next.eq_ignore_ascii_case(tail) && self.peek_at(n) == Some('(') {
                    name.push_str(&next);
                    self.i += n;
                    break;
                }
            }
        }
        if self.eat('(') {
            let mut args = Vec::new();
            if !self.eat(')') {
                loop {
                    self.skip_ws();
                    if matches!(self.peek(), Some(',') | Some(')')) {
                        args.push(Expr::Missing);
                    } else {
                        args.push(self.expr()?);
                    }
                    if self.eat(',') {
                        continue;
                    }
                    if self.eat(')') {
                        break;
                    }
                    return Err(self.err("関数のかっこが閉じていません"));
                }
            }
            return Ok(Expr::Call(Func::from_name(&name), args));
        }
        match name.to_ascii_uppercase().as_str() {
            "TRUE" => Ok(Expr::Bool(true)),
            "FALSE" => Ok(Expr::Bool(false)),
            _ => Ok(Expr::Err(Error::Name)),
        }
    }

    /// 参照を読む（参照でなければ位置を戻して `None`）。
    fn reference(&mut self) -> Result<Option<Ref>, ParseError> {
        let start = self.i;
        let sheet = self.sheet_prefix()?;
        let Some(first) = self.part() else {
            self.i = start;
            if sheet.is_some() {
                return Err(self.err("シート名のあとに参照がありません"));
            }
            return Ok(None);
        };
        // 関数名（`LOG10(` など）なら参照ではない
        if sheet.is_none() && self.peek() == Some('(') {
            self.i = start;
            return Ok(None);
        }
        let save = self.i;
        let second = if self.peek() == Some(':') {
            self.i += 1;
            match self.part() {
                Some(p) => Some(p),
                None => {
                    self.i = save;
                    None
                }
            }
        } else {
            None
        };
        let area = match (first, second) {
            (Part::Cell(r, c, ar, ac), None) => Area {
                r0: r,
                c0: c,
                r1: r,
                c1: c,
                abs: [ar, ac, ar, ac],
                kind: AreaKind::Cell,
            },
            (Part::Cell(r0, c0, ar0, ac0), Some(Part::Cell(r1, c1, ar1, ac1))) => {
                let (r0, ar0, r1, ar1) = if r0 <= r1 {
                    (r0, ar0, r1, ar1)
                } else {
                    (r1, ar1, r0, ar0)
                };
                let (c0, ac0, c1, ac1) = if c0 <= c1 {
                    (c0, ac0, c1, ac1)
                } else {
                    (c1, ac1, c0, ac0)
                };
                Area {
                    r0,
                    c0,
                    r1,
                    c1,
                    abs: [ar0, ac0, ar1, ac1],
                    kind: AreaKind::Range,
                }
            }
            (Part::Col(c0, a0), Some(Part::Col(c1, a1))) => Area {
                r0: 0,
                c0: c0.min(c1),
                r1: u64::MAX,
                c1: c0.max(c1),
                abs: [false, a0, false, a1],
                kind: AreaKind::Cols,
            },
            (Part::Row(r0, a0), Some(Part::Row(r1, a1))) => Area {
                r0: r0.min(r1),
                c0: 0,
                r1: r0.max(r1),
                c1: u32::MAX,
                abs: [a0, false, a1, false],
                kind: AreaKind::Rows,
            },
            _ => {
                self.i = start;
                if sheet.is_some() {
                    return Err(self.err("参照の書き方が正しくありません"));
                }
                return Ok(None);
            }
        };
        // 名前の続き（`A1B`）なら参照ではない
        if matches!(self.peek(), Some(c) if c.is_alphanumeric() || c == '_') {
            self.i = start;
            return Ok(None);
        }
        Ok(Some(Ref { sheet, area }))
    }

    /// `Sheet1!`・`'売上 2026'!`。
    fn sheet_prefix(&mut self) -> Result<Option<Arc<str>>, ParseError> {
        let start = self.i;
        if self.peek() == Some('\'') {
            self.i += 1;
            let mut name = String::new();
            loop {
                match self.peek() {
                    None => return Err(self.err("シート名の ' が閉じていません")),
                    Some('\'') if self.peek_at(1) == Some('\'') => {
                        name.push('\'');
                        self.i += 2;
                    }
                    Some('\'') => {
                        self.i += 1;
                        break;
                    }
                    Some(c) => {
                        name.push(c);
                        self.i += 1;
                    }
                }
            }
            if self.peek() != Some('!') {
                return Err(self.err("シート名のあとに ! がありません"));
            }
            self.i += 1;
            return Ok(Some(Arc::from(name)));
        }
        let mut j = self.i;
        while matches!(self.s.get(j), Some(c) if c.is_alphanumeric() || *c == '_' || *c == '.') {
            j += 1;
        }
        if j > self.i && self.s.get(j) == Some(&'!') {
            let name: String = self.s[self.i..j].iter().collect();
            self.i = j + 1;
            return Ok(Some(Arc::from(name)));
        }
        self.i = start;
        Ok(None)
    }

    /// 参照の片側（`$A$1`・`A`・`1`）。
    fn part(&mut self) -> Option<Part> {
        let start = self.i;
        let abs_c = self.peek() == Some('$');
        if abs_c {
            self.i += 1;
        }
        let cs = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        let letters: String = self.s[cs..self.i].iter().collect();
        let abs_r = self.peek() == Some('$');
        if abs_r {
            self.i += 1;
        }
        let rs = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.i += 1;
        }
        let digits: String = self.s[rs..self.i].iter().collect();
        let col = (!letters.is_empty() && letters.len() <= 3)
            .then(|| parse_col(&letters))
            .flatten();
        let row = digits
            .parse::<u64>()
            .ok()
            .filter(|r| *r >= 1)
            .map(|r| r - 1);
        let part = match (letters.is_empty(), digits.is_empty()) {
            (false, false) => col.zip(row).map(|(c, r)| Part::Cell(r, c, abs_r, abs_c)),
            (false, true) if !abs_r => col.map(|c| Part::Col(c, abs_c)),
            // 行だけ（`$1` の `$` は列の位置で読んでいる）
            (true, false) if !abs_r => row.map(|r| Part::Row(r, abs_c)),
            _ => None,
        };
        if part.is_none() {
            self.i = start;
        }
        part
    }
}

enum Part {
    /// 行・列・行が絶対・列が絶対
    Cell(u64, u32, bool, bool),
    Col(u32, bool),
    Row(u64, bool),
}

/// 列の名前 → 番号（`A` → 0。`XFD` まで）。
fn parse_col(s: &str) -> Option<u32> {
    let mut n: u32 = 0;
    for b in s.bytes() {
        n = n * 26 + (b.to_ascii_uppercase() - b'A' + 1) as u32;
    }
    (1..=16_384).contains(&n).then(|| n - 1)
}

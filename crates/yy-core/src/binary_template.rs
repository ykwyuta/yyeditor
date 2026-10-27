//! Bounded, read-only C-style binary template interpreter. No host code or file I/O.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use yy_buffer::Snapshot;

const MAX_SOURCE: usize = 1024 * 1024;
const MAX_NODES: usize = 20_000;
const MAX_STEPS: usize = 200_000;
const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub message: String,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}行: {}", self.line, self.message)
    }
}
impl std::error::Error for Error {}
type Result<T> = std::result::Result<T, Error>;
fn error(line: usize, message: impl Into<String>) -> Error {
    Error {
        line,
        message: message.into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}

/// A field and its exact byte span in the document, including array/structure containers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub ty: String,
    pub range: Range<u64>,
    pub value: String,
    pub endian: Endian,
    pub children: Vec<Field>,
}

#[derive(Clone, Debug)]
struct Token {
    text: String,
    line: usize,
}
fn lex(source: &str) -> Result<Vec<Token>> {
    if source.len() > MAX_SOURCE {
        return Err(error(1, "テンプレートは1 MiB以下にしてください"));
    }
    let b = source.as_bytes();
    let (mut i, mut line) = (0, 1);
    let mut tokens = Vec::new();
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            line += usize::from(b[i] == b'\n');
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i..].starts_with(b"/*") {
            i += 2;
            while i < b.len() && !b[i..].starts_with(b"*/") {
                line += usize::from(b[i] == b'\n');
                i += 1;
            }
            if i == b.len() {
                return Err(error(line, "コメントが閉じていません"));
            }
            i += 2;
            continue;
        }
        let start = i;
        if b[i].is_ascii_alphanumeric() || b[i] == b'_' {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
        } else if b"{}[]();,.+-*/%<>=!&|^~".contains(&b[i]) {
            i += 1;
            if i < b.len()
                && b[i].is_ascii()
                && [
                    "==", "!=", "<=", ">=", "&&", "||", "<<", ">>", "++", "--", "+=", "-=",
                ]
                .contains(&&source[start..=i])
            {
                i += 1;
            }
        } else {
            return Err(error(
                line,
                format!("未対応の文字: {}", source[i..].chars().next().unwrap()),
            ));
        }
        tokens.push(Token {
            text: source[start..i].into(),
            line,
        });
    }
    tokens.push(Token {
        text: String::new(),
        line,
    });
    Ok(tokens)
}

#[derive(Clone, Debug)]
enum Expr {
    Num(i128),
    Var(String),
    Index(Box<Expr>, Box<Expr>),
    Member(Box<Expr>, String),
    Call(String, Vec<Expr>),
    Unary(String, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
}
#[derive(Clone, Debug)]
struct Stmt {
    line: usize,
    kind: Kind,
}
#[derive(Clone, Debug)]
enum Kind {
    Decl {
        ty: String,
        name: String,
        count: Option<Expr>,
        local: Option<Expr>,
    },
    Block(Vec<Stmt>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    While(Expr, Box<Stmt>),
    For(Box<Stmt>, Expr, Box<Stmt>, Box<Stmt>),
    Assign(String, String, Expr),
    Call(String, Vec<Expr>),
    Empty,
}
#[derive(Clone, Debug)]
struct Record {
    union: bool,
    body: Vec<Stmt>,
}
#[derive(Clone, Debug)]
pub struct Template {
    statements: Vec<Stmt>,
    records: HashMap<String, Record>,
    aliases: HashMap<String, String>,
}
struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
    records: HashMap<String, Record>,
    aliases: HashMap<String, String>,
}
impl Parser {
    fn peek(&self) -> &str {
        &self.tokens[self.pos].text
    }
    fn line(&self) -> usize {
        self.tokens[self.pos].line
    }
    fn take(&mut self) -> String {
        let s = self.peek().to_owned();
        if !s.is_empty() {
            self.pos += 1;
        }
        s
    }
    fn eat(&mut self, s: &str) -> bool {
        if self.peek() == s {
            self.take();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, s: &str) -> Result<()> {
        if self.eat(s) {
            Ok(())
        } else {
            Err(error(
                self.line(),
                format!("'{s}'が必要です（現在: '{}'）", self.peek()),
            ))
        }
    }
    fn ident(&mut self) -> Result<String> {
        let line = self.line();
        let s = self.take();
        if s.as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        {
            Ok(s)
        } else {
            Err(error(line, "識別子が必要です"))
        }
    }
    fn expr(&mut self, min: u8) -> Result<Expr> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(error(self.line(), "式の入れ子が深すぎます"));
        }
        let mut lhs = if ["-", "+", "!", "~"].contains(&self.peek()) {
            let op = self.take();
            Expr::Unary(op, Box::new(self.expr(12)?))
        } else if self.eat("(") {
            let e = self.expr(0)?;
            self.expect(")")?;
            e
        } else if self.peek().starts_with(|c: char| c.is_ascii_digit()) {
            let line = self.line();
            let raw = self.take().replace('_', "");
            let raw = raw.trim_end_matches(['u', 'U', 'l', 'L']);
            let n = if let Some(s) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
                i128::from_str_radix(s, 16)
            } else {
                raw.parse()
            }
            .map_err(|_| error(line, "整数が不正です"))?;
            Expr::Num(n)
        } else {
            let name = self.ident()?;
            if self.eat("(") {
                Expr::Call(name, self.args()?)
            } else {
                Expr::Var(name)
            }
        };
        let mut operators = 0;
        loop {
            operators += 1;
            if operators > MAX_DEPTH {
                return Err(error(self.line(), "式が長すぎます"));
            }
            if self.eat(".") {
                lhs = Expr::Member(Box::new(lhs), self.ident()?);
                continue;
            }
            if self.eat("[") {
                let i = self.expr(0)?;
                self.expect("]")?;
                lhs = Expr::Index(Box::new(lhs), Box::new(i));
                continue;
            }
            let prec = match self.peek() {
                "||" => 1,
                "&&" => 2,
                "|" => 3,
                "^" => 4,
                "&" => 5,
                "==" | "!=" => 6,
                "<" | "<=" | ">" | ">=" => 7,
                "<<" | ">>" => 8,
                "+" | "-" => 9,
                "*" | "/" | "%" => 10,
                _ => break,
            };
            if prec < min {
                break;
            }
            let op = self.take();
            let rhs = self.expr(prec + 1)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        self.depth -= 1;
        Ok(lhs)
    }
    fn args(&mut self) -> Result<Vec<Expr>> {
        let mut args = Vec::new();
        if self.eat(")") {
            return Ok(args);
        }
        loop {
            args.push(self.expr(0)?);
            if self.eat(")") {
                break;
            }
            self.expect(",")?;
        }
        Ok(args)
    }
    fn stmt(&mut self) -> Result<Stmt> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(error(self.line(), "文の入れ子が深すぎます"));
        }
        let line = self.line();
        let kind = if self.eat(";") {
            Kind::Empty
        } else if self.eat("{") {
            let mut body = Vec::new();
            while !self.eat("}") {
                if self.peek().is_empty() {
                    return Err(error(line, "'}'が必要です"));
                }
                body.push(self.stmt()?);
            }
            Kind::Block(body)
        } else if self.eat("if") {
            self.expect("(")?;
            let e = self.expr(0)?;
            self.expect(")")?;
            let yes = Box::new(self.stmt()?);
            let no = if self.eat("else") {
                Some(Box::new(self.stmt()?))
            } else {
                None
            };
            Kind::If(e, yes, no)
        } else if self.eat("while") {
            self.expect("(")?;
            let e = self.expr(0)?;
            self.expect(")")?;
            Kind::While(e, Box::new(self.stmt()?))
        } else if self.eat("for") {
            self.expect("(")?;
            let init = Box::new(self.simple(";")?);
            let e = self.expr(0)?;
            self.expect(";")?;
            let next = Box::new(self.simple(")")?);
            Kind::For(init, e, next, Box::new(self.stmt()?))
        } else {
            self.depth -= 1;
            return self.simple(";");
        };
        self.depth -= 1;
        Ok(Stmt { line, kind })
    }
    fn simple(&mut self, end: &str) -> Result<Stmt> {
        let line = self.line();
        if self.eat(end) {
            return Ok(Stmt {
                line,
                kind: Kind::Empty,
            });
        }
        let local = self.eat("local");
        let typedef = self.eat("typedef");
        let mut ty = self.ident()?;
        if ty == "struct" || ty == "union" {
            let union = ty == "union";
            let tag = if self.peek() != "{" {
                Some(self.ident()?)
            } else {
                None
            };
            if self.eat("{") {
                let mut body = Vec::new();
                while !self.eat("}") {
                    if self.peek().is_empty() {
                        return Err(error(line, "構造体の'}'が必要です"));
                    }
                    body.push(self.stmt()?);
                }
                let name = if typedef {
                    self.ident()?
                } else {
                    tag.clone()
                        .ok_or_else(|| error(line, "構造体名が必要です"))?
                };
                if self.records.contains_key(&name) {
                    return Err(error(line, "型名が重複しています"));
                }
                self.records.insert(name.clone(), Record { union, body });
                if self.eat(end) {
                    return Ok(Stmt {
                        line,
                        kind: Kind::Empty,
                    });
                }
                ty = name;
            } else {
                ty = tag.ok_or_else(|| error(line, "構造体名が必要です"))?;
            }
        }
        if ty == "unsigned" || ty == "signed" {
            ty += &format!(" {}", self.ident()?);
        }
        if typedef {
            let name = self.ident()?;
            self.expect(end)?;
            if primitive(&ty).is_none()
                && !self.records.contains_key(&ty)
                && !self.aliases.contains_key(&ty)
            {
                return Err(error(line, "未定義の型です"));
            }
            if self.aliases.insert(name, ty).is_some() {
                return Err(error(line, "型名が重複しています"));
            }
            return Ok(Stmt {
                line,
                kind: Kind::Empty,
            });
        }
        let kind = if self.eat("(") {
            Kind::Call(ty, self.args()?)
        } else if ["=", "+=", "-=", "++", "--"].contains(&self.peek()) {
            let op = self.take();
            let e = if op == "++" || op == "--" {
                Expr::Num(1)
            } else {
                self.expr(0)?
            };
            Kind::Assign(ty, op, e)
        } else {
            let name = self.ident()?;
            let count = if self.eat("[") {
                let e = self.expr(0)?;
                self.expect("]")?;
                Some(e)
            } else {
                None
            };
            let init = if local {
                self.expect("=")?;
                Some(self.expr(0)?)
            } else {
                None
            };
            Kind::Decl {
                ty,
                name,
                count,
                local: init,
            }
        };
        self.expect(end)?;
        Ok(Stmt { line, kind })
    }
}

/// Compile a supported C-style template. Unsupported syntax is an error, never ignored.
pub fn compile(source: &str) -> Result<Template> {
    let mut p = Parser {
        tokens: lex(source)?,
        pos: 0,
        depth: 0,
        records: HashMap::new(),
        aliases: HashMap::new(),
    };
    let mut statements = Vec::new();
    while !p.peek().is_empty() {
        statements.push(p.stmt()?);
    }
    Ok(Template {
        statements,
        records: p.records,
        aliases: p.aliases,
    })
}

#[derive(Clone, Copy)]
enum Scalar {
    Unsigned(usize),
    Signed(usize),
    Float(usize),
    Char,
}
impl Scalar {
    fn size(self) -> usize {
        match self {
            Self::Unsigned(n) | Self::Signed(n) | Self::Float(n) => n,
            Self::Char => 1,
        }
    }
}
fn primitive(ty: &str) -> Option<Scalar> {
    Some(match ty {
        "uchar" | "ubyte" | "uint8" | "uint8_t" | "unsigned char" | "byte" => Scalar::Unsigned(1),
        "ushort" | "uint16" | "uint16_t" | "unsigned short" => Scalar::Unsigned(2),
        "uint" | "uint32" | "uint32_t" | "unsigned int" | "ulong" | "unsigned long" => {
            Scalar::Unsigned(4)
        }
        "uint64" | "uint64_t" | "uquad" => Scalar::Unsigned(8),
        "char" => Scalar::Char,
        "int8" | "int8_t" | "signed char" => Scalar::Signed(1),
        "short" | "int16" | "int16_t" | "signed short" => Scalar::Signed(2),
        "int" | "long" | "int32" | "int32_t" | "signed int" | "signed long" => Scalar::Signed(4),
        "int64" | "int64_t" | "quad" => Scalar::Signed(8),
        "float" => Scalar::Float(4),
        "double" => Scalar::Float(8),
        _ => return None,
    })
}

struct Runner<'a> {
    template: &'a Template,
    data: &'a Snapshot,
    pos: u64,
    endian: Endian,
    vars: HashMap<String, i128>,
    scope: String,
    steps: usize,
    nodes: usize,
    depth: usize,
    line: usize,
}
impl Template {
    /// Interpret against the current immutable snapshot without copying the entire file.
    pub fn run(&self, data: &Snapshot) -> Result<Vec<Field>> {
        let mut r = Runner {
            template: self,
            data,
            pos: 0,
            endian: Endian::Little,
            vars: HashMap::new(),
            scope: String::new(),
            steps: 0,
            nodes: 0,
            depth: 0,
            line: 1,
        };
        let mut fields = Vec::new();
        r.block(&self.statements, &mut fields)?;
        Ok(fields)
    }
}
impl Runner<'_> {
    fn fail(&self, message: impl Into<String>) -> Error {
        error(self.line, message)
    }
    fn tick(&mut self) -> Result<()> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err(self.fail("実行回数の上限に達しました"));
        }
        Ok(())
    }
    fn key(&self, name: &str) -> String {
        if self.scope.is_empty() {
            name.into()
        } else {
            format!("{}.{name}", self.scope)
        }
    }
    fn lookup(&self, name: &str) -> Result<i128> {
        let mut scope = self.scope.as_str();
        loop {
            let key = if scope.is_empty() {
                name.to_owned()
            } else {
                format!("{scope}.{name}")
            };
            if let Some(v) = self.vars.get(&key) {
                return Ok(*v);
            }
            if scope.is_empty() {
                break;
            }
            scope = scope.rsplit_once('.').map_or("", |(p, _)| p);
        }
        Err(self.fail(format!("未定義の整数: {name}")))
    }
    fn path(&mut self, e: &Expr) -> Result<String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.fail("変数参照が深すぎます"));
        }
        let result = self.path_inner(e);
        self.depth -= 1;
        result
    }
    fn path_inner(&mut self, e: &Expr) -> Result<String> {
        match e {
            Expr::Var(s) => Ok(s.clone()),
            Expr::Member(e, s) => Ok(format!("{}.{s}", self.path(e)?)),
            Expr::Index(e, i) => {
                let p = self.path(e)?;
                let i = self.eval(i)?;
                Ok(format!("{p}[{i}]"))
            }
            _ => Err(self.fail("変数名が必要です")),
        }
    }
    fn eval(&mut self, e: &Expr) -> Result<i128> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.fail("式の入れ子が深すぎます"));
        }
        let result = self.eval_inner(e);
        self.depth -= 1;
        result
    }
    fn eval_inner(&mut self, e: &Expr) -> Result<i128> {
        self.tick()?;
        match e {
            Expr::Num(n) => Ok(*n),
            Expr::Var(_) | Expr::Member(..) | Expr::Index(..) => {
                let path = self.path(e)?;
                self.lookup(&path)
            }
            Expr::Unary(op, e) => {
                let n = self.eval(e)?;
                match op.as_str() {
                    "+" => Ok(n),
                    "-" => n
                        .checked_neg()
                        .ok_or_else(|| self.fail("整数オーバーフロー")),
                    "!" => Ok(i128::from(n == 0)),
                    "~" => Ok(!n),
                    _ => unreachable!(),
                }
            }
            Expr::Binary(op, a, b) => {
                let a = self.eval(a)?;
                if op == "&&" && a == 0 {
                    return Ok(0);
                }
                if op == "||" && a != 0 {
                    return Ok(1);
                }
                let b = self.eval(b)?;
                let v = match op.as_str() {
                    "+" => a.checked_add(b),
                    "-" => a.checked_sub(b),
                    "*" => a.checked_mul(b),
                    "/" => a.checked_div(b),
                    "%" => a.checked_rem(b),
                    "<<" if (0..128).contains(&b) => a.checked_shl(b as u32),
                    ">>" if (0..128).contains(&b) => a.checked_shr(b as u32),
                    "&" => Some(a & b),
                    "|" => Some(a | b),
                    "^" => Some(a ^ b),
                    "==" => Some(i128::from(a == b)),
                    "!=" => Some(i128::from(a != b)),
                    "<" => Some(i128::from(a < b)),
                    "<=" => Some(i128::from(a <= b)),
                    ">" => Some(i128::from(a > b)),
                    ">=" => Some(i128::from(a >= b)),
                    "&&" => Some(i128::from(b != 0)),
                    "||" => Some(i128::from(b != 0)),
                    _ => None,
                };
                v.ok_or_else(|| self.fail("不正な整数演算（ゼロ除算・桁あふれ・シフト量）"))
            }
            Expr::Call(name, args) => match (name.as_str(), args.as_slice()) {
                ("FTell", []) => Ok(self.pos.into()),
                ("FileSize", []) => Ok(self.data.len().into()),
                ("FEof", []) => Ok(i128::from(self.pos >= self.data.len())),
                _ => Err(self.fail(format!("未対応の式関数: {name}"))),
            },
        }
    }
    fn block(&mut self, body: &[Stmt], out: &mut Vec<Field>) -> Result<()> {
        for stmt in body {
            self.exec(stmt, out)?;
        }
        Ok(())
    }
    fn exec(&mut self, stmt: &Stmt, out: &mut Vec<Field>) -> Result<()> {
        self.line = stmt.line;
        self.tick()?;
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.fail("実行時の入れ子が深すぎます"));
        }
        match &stmt.kind {
            Kind::Empty => (),
            Kind::Block(body) => self.block(body, out)?,
            Kind::If(e, yes, no) => {
                if self.eval(e)? != 0 {
                    self.exec(yes, out)?;
                } else if let Some(no) = no {
                    self.exec(no, out)?;
                }
            }
            Kind::While(e, body) => {
                while self.eval(e)? != 0 {
                    self.exec(body, out)?;
                }
            }
            Kind::For(init, e, next, body) => {
                self.exec(init, out)?;
                while self.eval(e)? != 0 {
                    self.exec(body, out)?;
                    self.exec(next, out)?;
                }
            }
            Kind::Assign(name, op, e) => {
                let rhs = self.eval(e)?;
                let old = self.lookup(name)?;
                // Assignment is intentionally restricted to local variables, never file-backed fields.
                let key = self.key(&format!("@{name}"));
                if !self.vars.contains_key(&key) {
                    return Err(self.fail("代入はlocal変数にのみ対応します"));
                }
                let v = match op.as_str() {
                    "=" => Some(rhs),
                    "+=" | "++" => old.checked_add(rhs),
                    "-=" | "--" => old.checked_sub(rhs),
                    _ => None,
                }
                .ok_or_else(|| self.fail("整数オーバーフロー"))?;
                self.vars.insert(self.key(name), v);
            }
            Kind::Call(name, args) => match (name.as_str(), args.as_slice()) {
                ("LittleEndian", []) => self.endian = Endian::Little,
                ("BigEndian", []) => self.endian = Endian::Big,
                ("FSeek" | "FSkip", [e]) => {
                    let n = self.eval(e)?;
                    let n = if name == "FSkip" {
                        i128::from(self.pos)
                            .checked_add(n)
                            .ok_or_else(|| self.fail("位置の桁あふれ"))?
                    } else {
                        n
                    };
                    let pos = u64::try_from(n).map_err(|_| self.fail("位置が不正です"))?;
                    if pos > self.data.len() {
                        return Err(self.fail("位置がファイル範囲外です"));
                    }
                    self.pos = pos;
                }
                _ => return Err(self.fail(format!("未対応の関数: {name}"))),
            },
            Kind::Decl {
                ty,
                name,
                count,
                local,
            } => {
                if let Some(init) = local {
                    if count.is_some() || primitive(ty).is_none() {
                        return Err(self.fail("localは整数スカラーを指定してください"));
                    }
                    let n = self.eval(init)?;
                    self.vars.insert(self.key(name), n);
                    self.vars.insert(self.key(&format!("@{name}")), 1);
                } else {
                    let field = if let Some(count) = count {
                        let n = self.eval(count)?;
                        let n =
                            usize::try_from(n).map_err(|_| self.fail("配列の長さが不正です"))?;
                        if n > MAX_NODES.saturating_sub(self.nodes) {
                            return Err(self.fail("配列・フィールド数の上限は20000です"));
                        }
                        let start = self.pos;
                        let mut children = Vec::new();
                        for i in 0..n {
                            children.push(self.field(ty, &format!("{name}[{i}]"))?);
                        }
                        let value = if ty == "char" {
                            let bytes = self.data.read(start..self.pos);
                            format!("{:?}", String::from_utf8_lossy(&bytes))
                        } else {
                            format!("{n}要素")
                        };
                        self.node()?;
                        Field {
                            name: name.clone(),
                            ty: format!("{ty}[{n}]"),
                            range: start..self.pos,
                            value,
                            endian: self.endian,
                            children,
                        }
                    } else {
                        self.field(ty, name)?
                    };
                    out.push(field);
                }
            }
        }
        self.depth -= 1;
        Ok(())
    }
    fn node(&mut self) -> Result<()> {
        self.nodes += 1;
        self.tick()?;
        if self.nodes > MAX_NODES {
            return Err(self.fail("フィールド数の上限は20000です"));
        }
        Ok(())
    }
    fn field(&mut self, ty: &str, name: &str) -> Result<Field> {
        self.node()?;
        let mut resolved = ty;
        for _ in 0..MAX_DEPTH {
            if let Some(alias) = self.template.aliases.get(resolved) {
                resolved = alias;
            } else {
                break;
            }
        }
        let start = self.pos;
        let endian = self.endian;
        if let Some(scalar) = primitive(resolved) {
            let size = scalar.size();
            let end = start
                .checked_add(size as u64)
                .ok_or_else(|| self.fail("位置の桁あふれ"))?;
            if end > self.data.len() {
                return Err(self.fail(format!(
                    "{name}: 0x{start:X}から{size}バイトが必要です（ファイル長: {}）",
                    self.data.len()
                )));
            }
            let bytes = self.data.read(start..end);
            let mut padded = [0u8; 8];
            let n = match endian {
                Endian::Little => {
                    padded[..size].copy_from_slice(&bytes);
                    u64::from_le_bytes(padded)
                }
                Endian::Big => {
                    padded[8 - size..].copy_from_slice(&bytes);
                    u64::from_be_bytes(padded)
                }
            };
            let (value, integer) = match scalar {
                Scalar::Unsigned(_) => (format!("{n} (0x{n:X})"), Some(i128::from(n))),
                Scalar::Char => (
                    format!("{:?} ({n})", char::from(bytes[0])),
                    Some(i128::from(n)),
                ),
                Scalar::Signed(_) => {
                    let signed = ((n << (64 - size * 8)) as i64) >> (64 - size * 8);
                    (signed.to_string(), Some(i128::from(signed)))
                }
                Scalar::Float(4) => (f32::from_bits(n as u32).to_string(), None),
                Scalar::Float(_) => (f64::from_bits(n).to_string(), None),
            };
            if let Some(n) = integer {
                self.vars.insert(self.key(name), n);
            }
            self.pos = end;
            Ok(Field {
                name: name.into(),
                ty: ty.into(),
                range: start..end,
                value,
                endian,
                children: Vec::new(),
            })
        } else if let Some(record) = self.template.records.get(resolved) {
            self.depth += 1;
            if self.depth > MAX_DEPTH {
                return Err(self.fail("再帰構造の上限に達しました"));
            }
            let old_scope = self.scope.clone();
            self.scope = self.key(name);
            let mut children = Vec::new();
            let mut end = start;
            for stmt in &record.body {
                if record.union {
                    self.pos = start;
                }
                self.exec(stmt, &mut children)?;
                end = end.max(self.pos);
            }
            self.scope = old_scope;
            self.pos = end;
            self.depth -= 1;
            Ok(Field {
                name: name.into(),
                ty: ty.into(),
                range: start..end,
                value: String::new(),
                endian,
                children,
            })
        } else {
            Err(self.fail(format!("未定義の型: {ty}")))
        }
    }
}

pub const EXAMPLE: &str = "// 先頭4バイトと残りのデータ\r\nLittleEndian();\r\nchar magic[4];\r\nuchar data[FileSize() - FTell()];\r\n";

#[cfg(test)]
mod tests {
    use super::*;
    fn run(src: &str, bytes: &[u8]) -> Result<Vec<Field>> {
        compile(src)?.run(&Snapshot::from_bytes(bytes.to_vec()))
    }
    #[test]
    fn nested_dynamic_arrays_and_endian() {
        let f = run("typedef struct { ushort count; uchar payload[count]; } PACKET; BigEndian(); PACKET header; ushort tail;", &[0, 3, 10, 20, 30, 0x12, 0x34]).unwrap();
        assert_eq!(f[0].range, 0..5);
        assert_eq!(f[0].children[1].children[2].value, "30 (0x1E)");
        assert_eq!(f[1].value, "4660 (0x1234)");
    }
    #[test]
    fn loops_conditionals_locals_and_member_references() {
        let f = run("struct H { uchar count; }; H h; for(local int i=0; i<h.count; i++) { if(i%2==0) ushort item; else uchar item; }", &[3, 1, 0, 9, 2, 0]).unwrap();
        assert_eq!(f.len(), 4);
        assert_eq!(f[3].range, 4..6);
    }
    #[test]
    fn union_seek_alias_float_and_signed() {
        let f = run("typedef uint32 U; union V { U bits; float number; }; V v; FSeek(0); int8 negative; FSkip(3); BigEndian(); short last;", &[0xff, 0, 0x80, 0x3f, 0xff, 0xfe]).unwrap();
        assert_eq!(f[0].range, 0..4);
        assert_eq!(f[0].children[1].range, 0..4);
        assert_eq!(f[1].value, "-1");
        assert_eq!(f[2].value, "-2");
    }
    #[test]
    fn comments_and_array_index() {
        let f = run(
            "/* hello\nworld */ uchar lengths[2]; // test\nchar text[lengths[1]];",
            &[1, 3, b'A', b'B', b'C'],
        )
        .unwrap();
        assert_eq!(f[1].value, "\"ABC\"");
    }
    #[test]
    fn bundled_bmp_and_wav_templates() {
        let mut bmp = vec![0; 54];
        bmp[..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&54u32.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&320i32.to_le_bytes());
        let f = run(
            include_str!("../../../demo/binary-templates/bmp.yybt"),
            &bmp,
        )
        .unwrap();
        assert_eq!(f[0].range, 0..14);
        assert_eq!(f[1].range, 14..54);
        assert_eq!(f[1].children[1].value, "320");
        let mut wav = b"RIFF\x28\0\0\0WAVEfmt \x10\0\0\0\x01\0\x02\0".to_vec();
        wav.extend_from_slice(&44100u32.to_le_bytes());
        wav.extend_from_slice(&176400u32.to_le_bytes());
        wav.extend_from_slice(&[4, 0, 16, 0]);
        wav.extend_from_slice(b"data\x03\0\0\0abc\0");
        let f = run(
            include_str!("../../../demo/binary-templates/wav.yybt"),
            &wav,
        )
        .unwrap();
        assert_eq!(f.len(), 5);
        assert_eq!(f[3].children[4].value, "44100 (0xAC44)");
        assert_eq!(f[4].range, 36..48);
        assert!(
            run(
                include_str!("../../../demo/binary-templates/wav.yybt"),
                &wav[..47]
            )
            .is_err()
        );
    }
    #[test]
    fn truncated_invalid_and_unsupported_are_errors() {
        for src in [
            "uint a;",
            "FSeek(-1);",
            "FSeek(99);",
            "uchar a[1/0];",
            "uchar a[-1];",
            "uchar a[20001];",
            "uchar a; a=2;",
            "Printf(1);",
            "#include x",
            "/*",
            "if(1){",
            "uchar 値;",
            "(値",
        ] {
            assert!(run(src, &[1, 2]).is_err(), "{src}");
        }
        assert_eq!(compile("// comment\nuint n\n").unwrap_err().line, 3);
    }
    #[test]
    fn runaway_loops_recursion_and_expression_depth_are_bounded() {
        assert!(
            run("while(1) {}", &[])
                .unwrap_err()
                .message
                .contains("上限")
        );
        assert!(run("struct R { R next; }; R r;", &[]).is_err());
        assert!(
            compile(&format!(
                "uchar x[{}1{}];",
                "(".repeat(100),
                ")".repeat(100)
            ))
            .is_err()
        );
    }
    #[test]
    fn short_circuit_and_zero_length_array() {
        let f = run(
            "if(0 && missing) uint absent; uchar empty[0]; while(!FEof()) uchar rest;",
            &[7],
        )
        .unwrap();
        assert_eq!(f[0].range, 0..0);
        assert_eq!(f[1].value, "7 (0x7)");
    }
}

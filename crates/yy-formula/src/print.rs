//! 式を文字列に戻す（行・列の挿入と削除で参照を付け替えたあとに見せる）。

use std::fmt::{self, Write};

use crate::col_name;
use crate::parse::{Area, AreaKind, BinOp, Expr, Ref};

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Num(n) => f.write_str(&yy_numfmt::general(*n)),
            Expr::Text(s) => {
                f.write_char('"')?;
                f.write_str(&s.replace('"', "\"\""))?;
                f.write_char('"')
            }
            Expr::Bool(b) => f.write_str(if *b { "TRUE" } else { "FALSE" }),
            Expr::Err(e) => f.write_str(e.text()),
            Expr::Ref(r) => write!(f, "{r}"),
            Expr::Array(rows) => {
                f.write_char('{')?;
                for (i, row) in rows.iter().enumerate() {
                    if i > 0 {
                        f.write_char(';')?;
                    }
                    for (j, e) in row.iter().enumerate() {
                        if j > 0 {
                            f.write_char(',')?;
                        }
                        write!(f, "{e}")?;
                    }
                }
                f.write_char('}')
            }
            Expr::Neg(e) => write!(f, "-{e}"),
            Expr::Plus(e) => write!(f, "+{e}"),
            Expr::Percent(e) => write!(f, "{e}%"),
            Expr::Bin(op, l, r) => {
                let o = match op {
                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                    BinOp::Mul => "*",
                    BinOp::Div => "/",
                    BinOp::Pow => "^",
                    BinOp::Concat => "&",
                    BinOp::Eq => "=",
                    BinOp::Ne => "<>",
                    BinOp::Lt => "<",
                    BinOp::Le => "<=",
                    BinOp::Gt => ">",
                    BinOp::Ge => ">=",
                };
                write!(f, "{l}{o}{r}")
            }
            Expr::Call(func, args) => {
                f.write_str(func.name())?;
                f.write_char('(')?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_char(',')?;
                    }
                    write!(f, "{a}")?;
                }
                f.write_char(')')
            }
            Expr::Paren(e) => write!(f, "({e})"),
            Expr::Missing => Ok(()),
        }
    }
}

impl fmt::Display for Ref {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(s) = &self.sheet {
            let plain = s.chars().all(|c| c.is_alphanumeric() || c == '_')
                && !s.chars().next().is_some_and(|c| c.is_ascii_digit());
            if plain {
                write!(f, "{s}!")?;
            } else {
                write!(f, "'{}'!", s.replace('\'', "''"))?;
            }
        }
        write!(f, "{}", self.area)
    }
}

impl fmt::Display for Area {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = |abs: bool| if abs { "$" } else { "" };
        let cell = |r: u64, c: u32, ar: bool, ac: bool| {
            format!("{}{}{}{}", d(ac), col_name(c), d(ar), r + 1)
        };
        match self.kind {
            AreaKind::Cell => f.write_str(&cell(self.r0, self.c0, self.abs[0], self.abs[1])),
            AreaKind::Range => write!(
                f,
                "{}:{}",
                cell(self.r0, self.c0, self.abs[0], self.abs[1]),
                cell(self.r1, self.c1, self.abs[2], self.abs[3])
            ),
            AreaKind::Cols => write!(
                f,
                "{}{}:{}{}",
                d(self.abs[1]),
                col_name(self.c0),
                d(self.abs[3]),
                col_name(self.c1)
            ),
            AreaKind::Rows => write!(
                f,
                "{}{}:{}{}",
                d(self.abs[0]),
                self.r0 + 1,
                d(self.abs[2]),
                self.r1 + 1
            ),
        }
    }
}

/// 数式の文字列（先頭の `=` 付き）。
pub fn formula_text(e: &Expr) -> String {
    format!("={e}")
}

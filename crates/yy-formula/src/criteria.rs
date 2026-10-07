//! `SUMIFS`・`COUNTIFS` の条件（15 章 7.3.1）。
//!
//! - `"東京"`: 等しい（大文字・小文字を区別しない。`*`・`?` のワイルドカード、`~` で打ち消し）
//! - `">=100"`・`"<>東京"`・`"<>"`（空でない）・`"="`（空）・`""`（空）
//! - 数値（`100` や `"100"`・`"2026/10/1"` のように数値・日付に読める文字列）は、数値の 100 と、
//!   数値に読める文字列の両方に合う。

use std::cmp::Ordering;

use yy_numfmt::{DateSystem, Parsed};

use crate::eval::num_cmp;
use crate::{Cell, Error, cmp_text, eq_text, wildcard_match};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug, PartialEq)]
enum Target {
    Num(f64),
    Text(String),
    Bool(bool),
    Err(Error),
    /// 空（`""`・`"<>"`。`true` は `"="`: 空の文字列は合わない）
    Empty(bool),
}

/// 解釈した条件。
#[derive(Clone, Debug)]
pub(crate) struct Criterion {
    op: Op,
    target: Target,
    /// 文字列の等しい・等しくないでワイルドカードを使う
    wildcard: bool,
    sys: DateSystem,
}

impl Criterion {
    pub(crate) fn new(c: Cell<'_>, sys: DateSystem) -> Criterion {
        let mk = |op, target, wildcard| Criterion {
            op,
            target,
            wildcard,
            sys,
        };
        let s = match c {
            Cell::Empty => return mk(Op::Eq, Target::Empty(false), false),
            Cell::Num(n) => return mk(Op::Eq, Target::Num(n), false),
            Cell::Bool(b) => return mk(Op::Eq, Target::Bool(b), false),
            Cell::Err(e) => return mk(Op::Eq, Target::Err(e), false),
            Cell::Text(s) => s,
        };
        let (op, rest) = if let Some(r) = s.strip_prefix(">=") {
            (Op::Ge, r)
        } else if let Some(r) = s.strip_prefix("<=") {
            (Op::Le, r)
        } else if let Some(r) = s.strip_prefix("<>") {
            (Op::Ne, r)
        } else if let Some(r) = s.strip_prefix('=') {
            (Op::Eq, r)
        } else if let Some(r) = s.strip_prefix('>') {
            (Op::Gt, r)
        } else if let Some(r) = s.strip_prefix('<') {
            (Op::Lt, r)
        } else {
            (Op::Eq, s)
        };
        if rest.is_empty() {
            return mk(op, Target::Empty(s == "="), false);
        }
        if let Parsed::Number(n, _) = yy_numfmt::parse_input(rest, sys) {
            return mk(op, Target::Num(n), false);
        }
        if rest.eq_ignore_ascii_case("TRUE") {
            return mk(op, Target::Bool(true), false);
        }
        if rest.eq_ignore_ascii_case("FALSE") {
            return mk(op, Target::Bool(false), false);
        }
        if let Some(e) = Error::parse(rest) {
            return mk(op, Target::Err(e), false);
        }
        let wildcard = rest.contains(['*', '?', '~']);
        mk(op, Target::Text(rest.to_owned()), wildcard)
    }

    /// セルが条件に合うか。
    pub(crate) fn test(&self, c: Cell<'_>) -> bool {
        let ord_ok = |o: Ordering| match self.op {
            Op::Eq => o == Ordering::Equal,
            Op::Ne => o != Ordering::Equal,
            Op::Lt => o == Ordering::Less,
            Op::Le => o != Ordering::Greater,
            Op::Gt => o == Ordering::Greater,
            Op::Ge => o != Ordering::Less,
        };
        match &self.target {
            Target::Empty(strict) => {
                let blank = matches!(c, Cell::Empty);
                let empty_text = matches!(c, Cell::Text(s) if s.is_empty());
                match self.op {
                    Op::Eq => blank || (!strict && empty_text),
                    Op::Ne => !blank,
                    _ => false,
                }
            }
            Target::Num(n) => {
                let v = match c {
                    Cell::Num(x) => Some(x),
                    // 等しい・等しくないは数値に読める文字列も比べる
                    Cell::Text(s) if matches!(self.op, Op::Eq | Op::Ne) => {
                        match yy_numfmt::parse_input(s, self.sys) {
                            Parsed::Number(x, _) => Some(x),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                match v {
                    Some(x) => ord_ok(num_cmp(x, *n)),
                    None => self.op == Op::Ne,
                }
            }
            Target::Bool(b) => match c {
                Cell::Bool(x) => ord_ok(x.cmp(b)),
                _ => self.op == Op::Ne,
            },
            Target::Err(e) => match c {
                Cell::Err(x) => (x == *e) == (self.op == Op::Eq),
                _ => self.op == Op::Ne,
            },
            Target::Text(t) => match c {
                Cell::Text(s) => {
                    if self.wildcard && matches!(self.op, Op::Eq | Op::Ne) {
                        wildcard_match(t, s) == (self.op == Op::Eq)
                    } else if matches!(self.op, Op::Eq | Op::Ne) {
                        eq_text(s, t) == (self.op == Op::Eq)
                    } else {
                        ord_ok(cmp_text(s, t))
                    }
                }
                _ => self.op == Op::Ne,
            },
        }
    }
}

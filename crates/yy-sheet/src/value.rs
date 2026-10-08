//! セルの値。

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// エラー値（Excel と同じ）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CellError {
    Null,
    Div0,
    Value,
    Ref,
    Name,
    Num,
    NA,
    Spill,
    Calc,
}

impl CellError {
    pub const ALL: [CellError; 9] = [
        CellError::Null,
        CellError::Div0,
        CellError::Value,
        CellError::Ref,
        CellError::Name,
        CellError::Num,
        CellError::NA,
        CellError::Spill,
        CellError::Calc,
    ];

    pub fn text(self) -> &'static str {
        match self {
            CellError::Null => "#NULL!",
            CellError::Div0 => "#DIV/0!",
            CellError::Value => "#VALUE!",
            CellError::Ref => "#REF!",
            CellError::Name => "#NAME?",
            CellError::Num => "#NUM!",
            CellError::NA => "#N/A",
            CellError::Spill => "#SPILL!",
            CellError::Calc => "#CALC!",
        }
    }

    /// `#N/A` などの表記から（大文字・小文字を区別しない）。
    pub fn parse(s: &str) -> Option<CellError> {
        CellError::ALL
            .into_iter()
            .find(|e| e.text().eq_ignore_ascii_case(s.trim()))
    }

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(c: u8) -> Option<CellError> {
        CellError::ALL.get(c as usize).copied()
    }
}

impl fmt::Display for CellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text())
    }
}

/// セルの値。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Value {
    #[default]
    Empty,
    Number(f64),
    Text(Arc<str>),
    Bool(bool),
    Error(CellError),
}

impl Value {
    pub fn text(s: &str) -> Value {
        Value::Text(Arc::from(s))
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Value::Empty)
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// 「標準」の表示形式での文字列（日付などの表示形式は当てない）。
    pub fn general_text(&self) -> String {
        match self {
            Value::Empty => String::new(),
            Value::Number(n) => yy_numfmt::general(*n),
            Value::Text(s) => figurative_label(s).unwrap_or(s).to_string(),
            Value::Bool(true) => "TRUE".into(),
            Value::Bool(false) => "FALSE".into(),
            Value::Error(e) => e.text().into(),
        }
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Value {
        Value::Number(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Value {
        Value::text(v)
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Value {
        Value::Bool(v)
    }
}

/// `CBL.LOW-VALUE()`・`CBL.HIGH-VALUE()` の結果（印の文字列）なら、表示する名前（`LOW-VALUE`・`HIGH-VALUE`）。
pub fn figurative_label(s: &str) -> Option<&'static str> {
    match yy_formula::figurative(s)? {
        0x00 => Some("LOW-VALUE"),
        _ => Some("HIGH-VALUE"),
    }
}

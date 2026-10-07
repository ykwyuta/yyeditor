//! IBM COBOL のコピーブック互換のレイアウト定義と、固定長レコードの項目の読み書き（15 章 6.5）。
//!
//! - [`parse`] はコピーブック（固定形式・自由形式）を読み、最初のレコード（01）の基本項目を
//!   バイト位置の順に並べた [`Layout`] を作る。`OCCURS` は添字付きの項目に展開し、`REDEFINES`
//!   する項目は使わない（マルチレイアウトは対象外。元の定義でバイトを読み書きする）。
//! - [`Codec`] は項目のバイト列と値（10 進数・文字列）を相互に変換する。文字コードは MS932 か
//!   EBCDIC（CCSID 930・939・1390・1399・290・1027・037・500・1047）。

mod codec;
mod copybook;
mod edit;
mod num;
mod pic;

pub use codec::{Charset, Codec, Decoded, Input, Issues, hex_text, parse_hex};
pub use copybook::parse;
pub use num::Decimal;

/// 符号の位置（`DISPLAY` の数字項目）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignPos {
    /// 最後の桁のゾーンに含める（既定）
    Trailing,
    /// 最初の桁のゾーンに含める（`SIGN LEADING`）
    Leading,
    /// 最後に `+`・`-` の文字（`SIGN TRAILING SEPARATE`）
    TrailingSeparate,
    /// 最初に `+`・`-` の文字（`SIGN LEADING SEPARATE`）
    LeadingSeparate,
}

/// 編集の記号（数字編集項目の `PIC`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sym {
    Nine,
    Z,
    Star,
    Comma,
    Dot,
    /// `V`（位置を取らない小数点）
    V,
    /// `P`（位置を取らない桁）
    P,
    Plus,
    Minus,
    Cr,
    Db,
    /// 通貨記号（`$`・`\`・`¥`）
    Cur(char),
    /// 空白の挿入
    B,
    /// `0` の挿入
    Zero,
    Slash,
}

/// 項目の型。
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// 英数字（`PIC X`・`A`・英数字編集）。`justified` は `JUSTIFIED RIGHT`
    Alnum { justified: bool },
    /// 2 バイト文字（`PIC G`・`PIC N`〔`DISPLAY-1`〕）。SO / SI なし
    Dbcs,
    /// `PIC N USAGE NATIONAL`（UTF-16 ビッグエンディアン）
    National,
    /// ゾーン 10 進数（`PIC S9(n)V9(m)`・`DISPLAY`）
    Zoned {
        digits: u32,
        scale: i32,
        signed: bool,
        sign: SignPos,
    },
    /// パック 10 進数（`COMP-3`・`PACKED-DECIMAL`）
    Packed {
        digits: u32,
        scale: i32,
        signed: bool,
    },
    /// 2 進数（`COMP`・`COMP-4`・`BINARY`・`COMP-5`。2・4・8 バイト）。`native` は `COMP-5`（桁数で
    /// 切り詰めない）
    Binary {
        digits: u32,
        scale: i32,
        signed: bool,
        bytes: usize,
        native: bool,
    },
    /// 浮動小数点（`COMP-1` は 4 バイト、`COMP-2` は 8 バイト）。EBCDIC では IBM の 16 進浮動小数点、
    /// MS932 では IEEE 754
    Float { double: bool },
    /// 数字編集（`PIC ZZ,ZZ9.99-` など）
    Edited {
        syms: Vec<Sym>,
        digits: u32,
        scale: i32,
        blank_zero: bool,
    },
}

impl Kind {
    /// 数値の項目か。
    pub fn is_numeric(&self) -> bool {
        !matches!(self, Kind::Alnum { .. } | Kind::Dbcs | Kind::National)
    }

    /// 数値の桁数・小数部の桁数（数値の項目）。
    pub fn digits_scale(&self) -> Option<(u32, i32)> {
        match self {
            Kind::Zoned { digits, scale, .. }
            | Kind::Packed { digits, scale, .. }
            | Kind::Binary { digits, scale, .. }
            | Kind::Edited { digits, scale, .. } => Some((*digits, *scale)),
            _ => None,
        }
    }
}

/// レイアウトの基本項目（1 列になる）。
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// 列の名前（同じ名前があれば `名前 OF 親`、`OCCURS` は `名前(1)`・`名前(1,2)`）
    pub name: String,
    /// レコードの先頭からの位置（0 始まり）
    pub offset: usize,
    pub len: usize,
    pub kind: Kind,
    /// 型の説明（`S9(7)V99 COMP-3` など。一覧に出す）
    pub describe: String,
}

/// レイアウト（1 レコードの形）。
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Layout {
    /// レコード（01）の名前
    pub record: String,
    /// レコード長（バイト）
    pub record_len: usize,
    pub fields: Vec<Field>,
    /// 読めたが注意のいること（使わない `REDEFINES`・2 つ目の 01・`OCCURS DEPENDING ON` など）
    pub warnings: Vec<String>,
}

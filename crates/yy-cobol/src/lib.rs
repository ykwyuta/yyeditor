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

pub use codec::{Charset, Codec, Decoded, Input, Issues, Misfit, hex_text, parse_hex};
pub use copybook::{add_field, check_type, parse, retype};
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
    /// 英数字（`PIC X`・`A`・英数字編集）。`justified` は `JUSTIFIED RIGHT`、`alpha` は英字だけの
    /// 項目（`PIC A`）
    Alnum { justified: bool, alpha: bool },
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

/// 数値の型の説明（`十進 5 けたの符号なし整数` など）。
fn number_words(digits: u32, scale: i32, signed: bool) -> String {
    let sign = if signed {
        "符号あり"
    } else {
        "符号なし"
    };
    if scale == 0 {
        format!("十進 {digits} けたの{sign}整数")
    } else if scale < 0 {
        format!(
            "十進 {digits} けたに 10 の {} 乗を掛けた{sign}整数（P）",
            -scale
        )
    } else if scale as u32 >= digits {
        format!("十進小数部 {scale} けた（有効 {digits} けた）の{sign}数")
    } else {
        format!(
            "十進整数部 {} けた、小数部 {scale} けたの{sign}数",
            digits as i32 - scale
        )
    }
}

impl Field {
    /// 型の意味（ヘルプ・セルの書式設定に出す。例: `数字 7 文字で十進整数部 5 けた、小数部 2 けたの
    /// 符号なし数を表す`）。
    pub fn meaning(&self) -> String {
        meaning(&self.kind, self.len)
    }
}

/// 型の意味（`len` は項目のバイト数）。
pub fn meaning(kind: &Kind, len: usize) -> String {
    match kind {
        Kind::Alnum { alpha: true, .. } => format!("英字 {len} 文字（英字と空白だけ）を表す"),
        Kind::Alnum { justified, .. } => format!(
            "英数字 {len} バイトの文字列を表す（漢字は 1 文字 2 バイト。足りない分は空白{}）",
            if *justified {
                "を左に詰める〔右寄せ〕"
            } else {
                "で埋める"
            }
        ),
        Kind::Dbcs => format!(
            "2 バイト文字（全角）{} 文字の文字列を表す（SO / SI なし）",
            len / 2
        ),
        Kind::National => format!("UTF-16 の {} 文字の文字列を表す", len / 2),
        Kind::Zoned {
            digits,
            scale,
            signed,
            sign,
        } => {
            let n = number_words(*digits, *scale, *signed);
            match (signed, sign) {
                (false, _) => format!("数字 {digits} 文字で{n}を表す"),
                (true, SignPos::Trailing) => {
                    format!("{n}を表す（符号は最後のけたのゾーンに含める）")
                }
                (true, SignPos::Leading) => {
                    format!("{n}を表す（符号は最初のけたのゾーンに含める）")
                }
                (true, SignPos::TrailingSeparate) => {
                    format!("数字 {digits} 文字と最後の符号 1 文字（+ -）で{n}を表す")
                }
                (true, SignPos::LeadingSeparate) => {
                    format!("最初の符号 1 文字（+ -）と数字 {digits} 文字で{n}を表す")
                }
            }
        }
        Kind::Packed {
            digits,
            scale,
            signed,
        } => format!(
            "パック 10 進数（1 バイトに 2 けた、最後の半バイトが符号）で{}を表す",
            number_words(*digits, *scale, *signed)
        ),
        Kind::Binary {
            digits,
            scale,
            signed,
            bytes,
            native,
        } => {
            let n = number_words(*digits, *scale, *signed);
            if *native {
                let bits = 8 * *bytes as u32;
                let (lo, hi): (i128, i128) = if *signed {
                    (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
                } else {
                    (0, (1i128 << bits) - 1)
                };
                format!(
                    "{bytes} バイトの 2 進数で{}{}を表す（PIC のけた数で切り詰めず、{}〜{} まで）",
                    if *signed {
                        "符号あり"
                    } else {
                        "符号なし"
                    },
                    if *scale == 0 { "整数" } else { "数" },
                    Decimal::new(lo, *scale),
                    Decimal::new(hi, *scale)
                )
            } else {
                format!("{bytes} バイトの 2 進数で{n}を表す")
            }
        }
        Kind::Float { double: false } => {
            "4 バイトの単精度浮動小数点数を表す（EBCDIC は IBM の 16 進形式、MS932 は IEEE 754）"
                .into()
        }
        Kind::Float { double: true } => {
            "8 バイトの倍精度浮動小数点数を表す（EBCDIC は IBM の 16 進形式、MS932 は IEEE 754）"
                .into()
        }
        Kind::Edited {
            syms,
            digits,
            scale,
            blank_zero,
        } => {
            let signed = syms
                .iter()
                .any(|s| matches!(s, Sym::Plus | Sym::Minus | Sym::Cr | Sym::Db));
            let sample = Decimal::new(
                if signed { -1 } else { 1 } * (12345678901234567890i128 % 10i128.pow(*digits)),
                *scale,
            );
            let (text, _) = edit::format(syms, sample, false);
            format!(
                "{}を {len} 文字に編集して表す（例: {}）{}",
                number_words(*digits, *scale, signed),
                text.trim_start(),
                if *blank_zero { "。0 は空白" } else { "" }
            )
        }
    }
}

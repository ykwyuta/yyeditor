//! Excel 互換の表示形式（15 章 8）。
//!
//! * [`date`] … 日付のシリアル値（1900 年・1904 年の日付、1900 年 2 月 29 日の扱い）
//! * [`general`] … 「標準」の文字列（有効数字 15 桁）
//! * [`input`] … 入力された文字列の解釈（数値・日付・時刻・百分率・通貨・真偽値と、付ける表示形式）

pub mod date;
mod general;
pub mod input;

pub use date::{DateSystem, DateTime};
pub use general::{general, general_into};
pub use input::{Parsed, parse_input};

/// 表示形式を当てた文字列（仮: 15 章 8 の書式記号は次の段階で作る）。
pub fn format_number(_code: &str, v: f64, _sys: DateSystem) -> String {
    general(v)
}

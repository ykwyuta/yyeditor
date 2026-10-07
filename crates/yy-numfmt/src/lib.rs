//! Excel 互換の表示形式（15 章 8）。
//!
//! * [`date`] … 日付のシリアル値（1900 年・1904 年の日付、1900 年 2 月 29 日の扱い）
//! * [`general`] … 「標準」の文字列（有効数字 15 桁）
//! * [`input`] … 入力された文字列の解釈（数値・日付・時刻・百分率・通貨・真偽値と、付ける表示形式）
//! * [`format`] … 表示形式（書式記号）の解析と表示

pub mod date;
pub mod format;
mod general;
pub mod input;

pub use date::{DateSystem, DateTime};
pub use general::{general, general_into};
pub use input::{Parsed, parse_input};

pub use format::{FmtColor, FmtValue, Format, Formatted, format_number, general_fit};

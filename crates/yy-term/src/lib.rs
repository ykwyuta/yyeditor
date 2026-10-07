//! yyterm の端末エミュレーションの中核（12 章）。
//!
//! OS に依存しない部分（制御シーケンスの解釈、画面とスクロールバック、選択範囲の文字列、
//! キー・貼り付け・マウスの操作の送り方）だけを持つ。シェルの起動（ConPTY・SSH）と描画は
//! `yy-win` が行う。

pub mod keys;
pub mod screen;
pub mod term;

pub use keys::{Button, Key, Mods, MouseEvent};
pub use screen::{Attr, Cell, Color, Line};
pub use term::{CursorShape, Modes, MouseMode, Pos, Terminal};

#[cfg(test)]
mod tests;

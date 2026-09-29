//! yyeditor のテキストバッファ。
//!
//! 元ファイル（mmap）や追記バッファを参照する「ピース」を永続 B+木に並べた
//! ピースツリーを提供する。詳細は `docs/proposal/02-buffer-large-file.md` を参照。

mod piece;
mod tree;

pub use piece::{
    ByteSource, LineCount, MAX_PIECE_LEN, Piece, PieceKey, SourceRef, align_utf8_forward, count_lf,
    split_into_pieces,
};
pub use tree::{LineLookup, LinePosition, Slice, Snapshot, Summary};

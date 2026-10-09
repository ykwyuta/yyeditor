//! yybrowser の広告ブロックの中核（20 章）。OS に依存しない。
//!
//! * [`lists`] … フィルタリストのキャッシュ（本文と更新の記録）・更新の間隔（`! Expires:`）・
//!   ダウンロードしたものの確かめ
//! * [`engine`] … adblock クレート（Brave。MPL-2.0）の包み: 要求の照合・広告の枠を隠す CSS・
//!   ページに差し込むスクリプト

pub mod engine;
pub mod lists;

pub use engine::{AdBlocker, PageCosmetic};

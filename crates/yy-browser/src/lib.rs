//! yybrowser の中核（19 章）。OS に依存しない。
//!
//! * [`proxy`] … プロキシの設定・検証・WebView2（Chromium）の起動引数・データのフォルダの名前
//! * [`profiles`] … プロキシのプロファイルの一覧（`browser.toml`）
//! * [`input`] … アドレスバーの入力 → URL

pub mod input;
pub mod profiles;
pub mod proxy;

pub use profiles::ProfileList;
pub use proxy::{ProxyMode, ProxyProfile};

//! yybrowser の中核（19 章）。OS に依存しない。
//!
//! * [`proxy`] … プロキシの設定・検証・WebView2（Chromium）の起動引数・データのフォルダの名前
//! * [`profiles`] … プロキシのプロファイルの一覧（`browser.toml`）
//! * [`rules`] … ドメインごとのプロキシ（PAC）・ホストの転送・開発者用証明書
//! * [`input`] … アドレスバーの入力 → URL

pub mod input;
pub mod profiles;
pub mod proxy;
pub mod rules;

pub use profiles::ProfileList;
pub use proxy::{ProxyMode, ProxyProfile};
pub use rules::{DevCert, HostMap, ProxyRule};

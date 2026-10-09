//! yybrowser の中核（19 章）。OS に依存しない。
//!
//! * [`proxy`] … プロキシの設定・検証・WebView2（Chromium）の起動引数・データのフォルダの名前
//! * [`profiles`] … プロキシのプロファイルの一覧（`browser.toml`）
//! * [`bookmarks`] … ブックマーク（bookmarks.toml・Edge や Chrome の HTML の読み込みと書き出し）
//! * [`form`] … プロキシの設定の画面の部品（プルダウンの選択 ↔ 保存する文字列）
//! * [`rules`] … ドメインごとのプロキシ（PAC）・ホストの転送・開発者用証明書
//! * [`input`] … アドレスバーの入力 → URL

pub mod bookmarks;
pub mod form;
pub mod input;
pub mod profiles;
pub mod proxy;
pub mod rules;

pub use profiles::{AdblockConfig, FilterList, ProfileList};
pub use proxy::{ProxyMode, ProxyProfile};
pub use rules::{DevCert, HostMap, ProxyRule};

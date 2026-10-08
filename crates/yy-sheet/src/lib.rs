//! スプレッドシート（yysheet）の中核（15 章）。
//!
//! * [`value`] … セルの値
//! * [`chunk`] … 列のチャンク（型ごとに詰めた形・統計・ファイルに置く形）
//! * [`column`] … 列（チャンクの区間の並び＋編集の差分）
//! * [`sheet`] … 表・シート・ブック・Undo
//! * [`store`] … チャンクを置くファイルと、展開したチャンクのキャッシュ
//! * [`budget`] … メモリの予算（既定 8 GB）
//! * [`yys`] … 独自形式のファイル（`.yys`）
//! * [`csv`] … CSV（RFC 4180）の並列の取り込みと書き出し
//! * [`query`] … 絞り込み（複数段階）・値の一覧・並べ替え（複数のキー）

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub mod autofill;
pub mod budget;
pub mod bulk;
pub mod chunk;
pub mod column;
pub mod csv;
pub mod fixed;
pub mod formula;
pub mod query;
pub mod shared;
pub mod sheet;
pub mod store;
pub mod style;
pub mod value;
pub mod yys;

pub use budget::Budget;
pub use column::Column;
pub use sheet::{Document, Place, Sheet, Table, View, Workbook};
pub use value::{CellError, Value};

use store::{ChunkCache, Store};

/// 文書をまたいで共有する状態（メモリの予算・チャンクのキャッシュ・作業ファイル）。
#[derive(Debug)]
pub struct Context {
    pub budget: Budget,
    pub cache: ChunkCache,
    work_dir: PathBuf,
    work: Mutex<Option<Arc<Store>>>,
    /// `XLOOKUP` の完全一致の索引（列のデータ・行の範囲ごと。新しいものが後ろ）
    pub(crate) lookups: Mutex<Vec<formula::LookupEntry>>,
}

impl Context {
    /// `limit` はメモリの上限、`work_dir` は作業ファイルのフォルダ。
    pub fn new(limit: u64, work_dir: PathBuf) -> Arc<Context> {
        Arc::new(Context {
            budget: Budget::new(limit),
            cache: ChunkCache::default(),
            work_dir,
            work: Mutex::new(None),
            lookups: Mutex::new(Vec::new()),
        })
    }

    /// 既定（8 GB・OS の一時フォルダ）。
    pub fn with_defaults() -> Arc<Context> {
        Context::new(budget::DEFAULT_LIMIT, std::env::temp_dir())
    }

    #[cfg(test)]
    pub(crate) fn for_tests() -> Arc<Context> {
        Context::with_defaults()
    }

    /// 作業ファイル（初めて使うときに作る）。
    pub fn work(&self) -> io::Result<Arc<Store>> {
        let mut w = self.work.lock().unwrap();
        if let Some(s) = &*w {
            return Ok(s.clone());
        }
        let s = Store::work(&self.work_dir)?;
        *w = Some(s.clone());
        Ok(s)
    }
}

/// 列の名前（`A`・`Z`・`AA`…）。
pub fn col_name(mut c: u32) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).expect("ascii")
}

/// 列の名前から番号（`A` = 0）。
pub fn parse_col(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 4 {
        return None;
    }
    let mut n: u32 = 0;
    for b in s.bytes() {
        let b = b.to_ascii_uppercase();
        if !b.is_ascii_uppercase() {
            return None;
        }
        n = n * 26 + (b - b'A') as u32 + 1;
    }
    Some(n - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_names() {
        for (i, n) in [
            (0, "A"),
            (25, "Z"),
            (26, "AA"),
            (51, "AZ"),
            (52, "BA"),
            (701, "ZZ"),
            (702, "AAA"),
            (16_383, "XFD"),
        ] {
            assert_eq!(col_name(i), n);
            assert_eq!(parse_col(n), Some(i));
        }
        assert_eq!(parse_col("a"), Some(0));
        assert_eq!(parse_col("A1"), None);
    }
}

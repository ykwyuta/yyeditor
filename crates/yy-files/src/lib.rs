//! yyfilemanager の中核（18 章）。OS に依存しない。
//!
//! * [`fs`] … ファイル操作の口（手元のファイルシステムと、試験用の故障を起こせる実装）
//! * [`scan`] … フォルダの走査と目録
//! * [`hash`] … ファイルのハッシュ（BLAKE3。全体と、先頭・末尾だけ）
//! * [`index`] … 目録とハッシュのキャッシュ（索引）
//! * [`sync`] … 共有フォルダへの同期（計画・実行・ジャーナル・レジューム）
//! * [`dupes`] … 完全に同じファイル
//! * [`similar`] … 似た名前のファイルと新しい版の判定
//! * [`purge`] … 確かめてからの一括削除（隔離フォルダ・取り消し）
//! * [`search`] … ファイルの検索（名前・属性・中身）
//! * [`jobs`] … 同期ジョブの保存・置き場所・画面なしでの実行

pub mod dupes;
pub mod fs;
pub mod hash;
pub mod index;
pub mod jobs;
pub mod office;
pub mod pattern;
pub mod purge;
pub mod scan;
pub mod search;
pub mod similar;
pub mod sync;

pub use fs::{DirEntry, Fs, Local, Meta};
pub use scan::{Catalog, FileEntry, ScanOptions, scan};

/// 中止したことを表すエラー（`ErrorKind::Interrupted` は読み直しの合図に使われるので使わない）。
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("中止しました")
    }
}

impl std::error::Error for Cancelled {}

/// 中止のエラー。
pub fn cancelled() -> std::io::Error {
    std::io::Error::other(Cancelled)
}

/// 中止したことによるエラーか。
pub fn is_cancelled(e: &std::io::Error) -> bool {
    e.get_ref().is_some_and(|i| i.is::<Cancelled>())
}

/// 相対パス（`/` 区切り）を `root` の下のパスにする。
pub fn join(root: &std::path::Path, rel: &str) -> std::path::PathBuf {
    let mut p = root.to_path_buf();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(part);
    }
    p
}

/// 相対パスの比べるための形（大文字・小文字と Unicode の正規化（NFC・NFD）の違いをそろえる）。
pub fn rel_key(rel: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    rel.nfc().collect::<String>().to_lowercase()
}

/// 大きさを読みやすく（`1.2 MB`）。
pub fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_joins() {
        assert_eq!(rel_key("A/Ｂ.TXT"), "a/ｂ.txt");
        // NFD（が + ゛）と NFC（が）は同じ
        assert_eq!(rel_key("か\u{3099}.txt"), rel_key("が.txt"));
        let p = join(std::path::Path::new("/r"), "a/b.txt");
        assert_eq!(p, std::path::Path::new("/r/a/b.txt"));
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
    }
}

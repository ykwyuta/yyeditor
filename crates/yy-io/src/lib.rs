//! ファイル入出力。
//!
//! 巨大ファイルは読み込まずにメモリマップし、ピースツリーから直接参照する。
//! 詳細は `docs/proposal/02-buffer-large-file.md` 4 章を参照。

// mmap の作成は unsafe を必要とする（ファイルが外部で変更されないことを共有モードで保証する）
#![allow(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use memmap2::Mmap;
use yy_buffer::{ByteSource, Snapshot, SourceRef};

/// 読み取り専用でメモリマップしたファイル。
pub struct MmapSource {
    map: Mmap,
}

impl ByteSource for MmapSource {
    fn bytes(&self) -> &[u8] {
        &self.map
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bom {
    None,
    Utf8,
}

impl Bom {
    pub fn len(self) -> u64 {
        match self {
            Bom::None => 0,
            Bom::Utf8 => 3,
        }
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}

/// 開いたファイルのハンドル。保持している間、他プロセスからの書き込みを拒否する。
pub struct FileGuard(#[allow(dead_code)] File);

/// 開いたファイル。
pub struct OpenedFile {
    pub path: PathBuf,
    pub file_len: u64,
    pub bom: Bom,
    /// 内容（BOM を除く）。改行数は未確定の状態で返す
    pub snapshot: Snapshot,
    pub guard: FileGuard,
}

/// 書き込みを拒否する共有モードでファイルを開く。
///
/// Windows では `FILE_SHARE_READ | FILE_SHARE_DELETE` を指定する。
/// 書き込み共有を許すと mmap 中に内容が変わり未定義動作になるため許可しない。
/// `FILE_SHARE_DELETE` は保存時に元ファイルを退避（rename）するために必要（06 章 3.4）。
fn open_locked(path: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_DELETE: u32 = 0x4;
        opts.share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE);
    }
    opts.open(path)
}

/// ファイルを開いてメモリマップし、改行数未確定のスナップショットを作る。
///
/// ファイルサイズに依存せず短時間で終わる（ピース分割のみ）。改行数は
/// バックグラウンドで数えて [`Snapshot::fill_line_counts`] で埋める。
pub fn open_file(path: &Path) -> io::Result<OpenedFile> {
    let file = open_locked(path)?;
    let meta = file.metadata()?;
    if meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "フォルダは開けません",
        ));
    }
    let file_len = meta.len();
    if file_len == 0 {
        return Ok(OpenedFile {
            path: path.to_owned(),
            file_len,
            bom: Bom::None,
            snapshot: Snapshot::empty(),
            guard: FileGuard(file),
        });
    }
    // SAFETY: 書き込み共有を拒否してファイルを開いているため、マップ中に内容は変化しない。
    // （Windows 以外では他プロセスによる変更を防げないが、Windows 以外は開発・テスト用途）
    let map = unsafe { Mmap::map(&file)? };
    let bom = if map.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Bom::Utf8
    } else {
        Bom::None
    };
    let len = map.len() as u64;
    let source: SourceRef = Arc::new(MmapSource { map });
    let snapshot = Snapshot::from_source(source, bom.len()..len, false);
    Ok(OpenedFile {
        path: path.to_owned(),
        file_len,
        bom,
        snapshot,
        guard: FileGuard(file),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use yy_buffer::LineLookup;

    /// 書き込んだ後にハンドルを閉じたファイルを作る。
    /// `open_file` は書き込み共有を拒否するため、書き込みハンドルが残っていると開けない。
    fn temp_file(dir: &tempfile::TempDir, data: &[u8]) -> PathBuf {
        let p = dir.path().join("test.txt");
        std::fs::write(&p, data).unwrap();
        p
    }

    #[test]
    fn opens_and_strips_utf8_bom() {
        let d = tempfile::tempdir().unwrap();
        let o = open_file(&temp_file(&d, b"\xEF\xBB\xBFhello\nworld\n")).unwrap();
        assert_eq!(o.bom, Bom::Utf8);
        assert_eq!(o.file_len, 15);
        assert_eq!(o.snapshot.read(0..o.snapshot.len()), b"hello\nworld\n");
        assert!(!o.snapshot.is_fully_indexed());
        assert_eq!(o.snapshot.line_start(1, true), LineLookup::Found(6));
    }

    #[test]
    fn opens_empty_file() {
        let d = tempfile::tempdir().unwrap();
        let o = open_file(&temp_file(&d, b"")).unwrap();
        assert!(o.snapshot.is_empty());
        assert_eq!(o.snapshot.line_count(), Some(1));
    }

    /// 他のプロセス（ここでは自分）が書き込み用に開いているファイルは開けない。
    #[cfg(windows)]
    #[test]
    fn denies_write_sharing() {
        let d = tempfile::tempdir().unwrap();
        let p = temp_file(&d, b"x");
        let _writer = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        assert!(open_file(&p).is_err());
    }

    #[test]
    fn rejects_directory_and_missing_file() {
        let d = tempfile::tempdir().unwrap();
        assert!(open_file(d.path()).is_err());
        assert!(open_file(&d.path().join("missing.txt")).is_err());
    }

    #[test]
    fn large_file_is_split_into_pieces() {
        let d = tempfile::tempdir().unwrap();
        let line = b"0123456789abcdef0123456789abcdef0123456789abcdef012345678\n";
        let mut data = Vec::new();
        while data.len() < 5 << 20 {
            data.extend_from_slice(line);
        }
        let o = open_file(&temp_file(&d, &data)).unwrap();
        assert!(o.snapshot.summary().pieces >= 5);
        assert_eq!(o.snapshot.read(0..o.snapshot.len()), data);
    }
}

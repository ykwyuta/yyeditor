//! ファイル入出力。
//!
//! 巨大ファイルは読み込まずにメモリマップし、ピースツリーから直接参照する。
//! 保存は一時ファイルに書き出してから置き換える（06 章 3）。
//! 詳細は `docs/proposal/02-buffer-large-file.md` 4 章を参照。

// mmap の作成は unsafe を必要とする（ファイルが外部で変更されないことを共有モードで保証する）
#![allow(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use memmap2::Mmap;
use yy_buffer::{ByteSource, Snapshot, SourceRef};

/// 読み取り専用でメモリマップしたファイル。
///
/// ファイルハンドル（書き込み共有を拒否）もここで保持するため、マップが参照されている間は
/// 内容が変わらない。保存で元ファイルを退避した場合は、最後の参照が消えたときに
/// 退避したファイルを削除する。
pub struct MmapSource {
    map: ManuallyDrop<Mmap>,
    file: ManuallyDrop<File>,
    path: PathBuf,
    delete_on_drop: Mutex<Option<PathBuf>>,
}

impl MmapSource {
    /// 開いたときのパス（退避後は元の名前のまま）。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// このマップが使われなくなったら `path` のファイルを削除する。
    fn delete_when_dropped(&self, path: PathBuf) {
        *self.delete_on_drop.lock().unwrap() = Some(path);
    }
}

impl ByteSource for MmapSource {
    fn bytes(&self) -> &[u8] {
        &self.map
    }
}

impl Drop for MmapSource {
    fn drop(&mut self) {
        // SAFETY: drop 中に一度だけ解放し、以後は触らない。
        // ファイルを削除する前にマップとハンドルを閉じる必要がある（Windows）。
        unsafe {
            ManuallyDrop::drop(&mut self.map);
            ManuallyDrop::drop(&mut self.file);
        }
        if let Some(p) = self.delete_on_drop.get_mut().unwrap().take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bom {
    None,
    Utf8,
}

impl Bom {
    pub fn len(self) -> u64 {
        self.bytes().len() as u64
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn bytes(self) -> &'static [u8] {
        match self {
            Bom::None => b"",
            Bom::Utf8 => b"\xEF\xBB\xBF",
        }
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
    /// メモリマップ（空のファイルでは `None`）
    pub source: Option<Arc<MmapSource>>,
    /// 空のファイルを開いた場合のハンドル
    pub guard: Option<FileGuard>,
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
            source: None,
            guard: Some(FileGuard(file)),
        });
    }
    // SAFETY: 書き込み共有を拒否してファイルを開いているため、マップ中に内容は変化しない。
    // （Windows 以外では他プロセスによる変更を防げないが、Windows 以外は開発・テスト用途）
    let map = unsafe { Mmap::map(&file)? };
    let bom = if map.starts_with(Bom::Utf8.bytes()) {
        Bom::Utf8
    } else {
        Bom::None
    };
    let len = map.len() as u64;
    let source = Arc::new(MmapSource {
        map: ManuallyDrop::new(map),
        file: ManuallyDrop::new(file),
        path: path.to_owned(),
        delete_on_drop: Mutex::new(None),
    });
    let source_ref: SourceRef = source.clone();
    let snapshot = Snapshot::from_source(source_ref, bom.len()..len, false);
    Ok(OpenedFile {
        path: path.to_owned(),
        file_len,
        bom,
        snapshot,
        source: Some(source),
        guard: None,
    })
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 保存先と同じフォルダに置く一時ファイル名。
fn sibling_name(target: &Path, tag: &str) -> PathBuf {
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_owned(),
        _ => PathBuf::from("."),
    };
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(".{name}.{tag}-{}-{n}", std::process::id()))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// 文書の内容を `target` に保存する（06 章 3.3〜3.4）。
///
/// 1. 同じフォルダの一時ファイルに書き出して永続化する
/// 2. `target` が現在メモリマップしているファイル（`current`）なら、それを退避名に変更する
///    （マップは退避したファイルを参照し続けるので、Undo 履歴も有効なまま）
/// 3. 一時ファイルを `target` に変更する
///
/// 途中で失敗した場合は元のファイルを元の名前に戻す。退避したファイルは `current` の
/// 最後の参照がなくなったときに削除される。
pub fn save_snapshot(
    snap: &Snapshot,
    target: &Path,
    bom: Bom,
    current: Option<&MmapSource>,
) -> io::Result<()> {
    let tmp = sibling_name(target, "yytmp");
    let write = || -> io::Result<()> {
        let file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        if let Ok(meta) = std::fs::metadata(target) {
            // 権限（Unix のモード等）を引き継ぐ
            let _ = file.set_permissions(meta.permissions());
        }
        let mut w = BufWriter::with_capacity(1 << 20, file);
        w.write_all(bom.bytes())?;
        for chunk in snap.chunks(0..snap.len()) {
            w.write_all(chunk)?;
        }
        let file = w.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    let retreat = match current {
        Some(c) if same_file(c.path(), target) => {
            let r = sibling_name(target, "yyorig");
            if let Err(e) = std::fs::rename(target, &r) {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
            Some((c, r))
        }
        _ => None,
    };
    if let Err(e) = std::fs::rename(&tmp, target) {
        if let Some((_, r)) = &retreat {
            let _ = std::fs::rename(r, target);
        }
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Some((c, r)) = retreat {
        // マップ中のファイルでも名前は消せる（Unix の unlink / Windows の削除保留）。
        // マップは内容を参照し続けるので、ここで消しておけば異常終了しても退避ファイルが残らない。
        // 消せない環境では、マップが使われなくなったときに消す。
        if std::fs::remove_file(&r).is_err() {
            c.delete_when_dropped(r);
        }
    }
    remove_stale_retreats(target);
    Ok(())
}

/// 以前の異常終了などで残った、`target` の退避ファイル（`.名前.yyorig-*`）を消す。
/// 他のプロセスが使用中でも、名前を消すだけなので内容の参照には影響しない。
fn remove_stale_retreats(target: &Path) {
    let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else {
        return;
    };
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let prefix = format!(".{}.yyorig-", name.to_string_lossy());
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
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

    fn dir_entries(d: &tempfile::TempDir) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn saves_new_file_with_bom() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("new.txt");
        let snap = Snapshot::from_bytes("こんにちは\r\n");
        save_snapshot(&snap, &target, Bom::Utf8, None).unwrap();
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"\xEF\xBB\xBF\xE3\x81\x93\xE3\x82\x93\xE3\x81\xAB\xE3\x81\xA1\xE3\x81\xAF\r\n"
        );
        assert_eq!(dir_entries(&d), vec!["new.txt"]);
    }

    /// 開いている（マップ中の）ファイルに上書き保存しても、古いスナップショットは
    /// 退避したファイルの内容を参照し続け、退避ファイル自体は残らない。
    #[test]
    fn overwrite_keeps_old_snapshot_until_dropped() {
        let d = tempfile::tempdir().unwrap();
        let p = temp_file(&d, b"original content\n");
        let o = open_file(&p).unwrap();
        let old = o.snapshot.clone();
        let edited = old.insert(0, b"edited: ");
        save_snapshot(&edited, &p, o.bom, o.source.as_deref()).unwrap();

        assert_eq!(std::fs::read(&p).unwrap(), b"edited: original content\n");
        // 古い内容は退避ファイルから読める
        assert_eq!(old.read(0..old.len()), b"original content\n");
        assert_eq!(edited.read(0..edited.len()), b"edited: original content\n");
        // 退避したファイルの名前はすでに消えている（内容はマップから読める）
        if !cfg!(windows) {
            assert_eq!(dir_entries(&d), vec!["test.txt"]);
        }

        drop(o);
        drop(old);
        drop(edited);
        assert_eq!(dir_entries(&d), vec!["test.txt"]);

        // 保存したファイルを開き直せる
        let o2 = open_file(&p).unwrap();
        assert_eq!(
            o2.snapshot.read(0..o2.snapshot.len()),
            b"edited: original content\n"
        );
    }

    #[test]
    fn removes_stale_retreat_files() {
        let d = tempfile::tempdir().unwrap();
        let p = temp_file(&d, b"x");
        std::fs::write(d.path().join(".test.txt.yyorig-1-0"), b"old").unwrap();
        std::fs::write(d.path().join(".other.txt.yyorig-1-0"), b"keep").unwrap();
        save_snapshot(&Snapshot::from_bytes("y"), &p, Bom::None, None).unwrap();
        assert_eq!(dir_entries(&d), vec![".other.txt.yyorig-1-0", "test.txt"]);
    }

    #[test]
    fn failed_save_leaves_no_temp_file() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("missing-dir").join("x.txt");
        let snap = Snapshot::from_bytes("x");
        assert!(save_snapshot(&snap, &target, Bom::None, None).is_err());
        assert!(dir_entries(&d).is_empty());
    }
}

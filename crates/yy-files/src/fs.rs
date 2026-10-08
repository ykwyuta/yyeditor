//! ファイル操作の口（18 章 2）。
//!
//! 同期・削除などの処理はすべてこの口を通してファイルを読み書きする。手元のファイルシステム（[`Local`]。
//! Windows では共有フォルダも同じ）と、試験用に、決めた回数の操作のあとで「回線が切れる」「アプリが
//! 落ちる」をまねる [`Faulty`] がある。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// ファイル・フォルダの情報。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Meta {
    pub size: u64,
    /// 更新日時（UNIX 時刻からのナノ秒）
    pub mtime: i64,
    /// 作成日時（分からなければ 0）
    pub ctime: i64,
    pub dir: bool,
    pub readonly: bool,
    pub hidden: bool,
    /// シンボリック リンク・ジャンクションなどの再解析点（たどらない）
    pub link: bool,
    /// ファイルの ID（同じ ID ならハードリンク。分からなければ `None`）
    pub file_id: Option<u128>,
}

/// フォルダの項目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub meta: Meta,
}

/// 読むファイル。
pub trait ReadFile: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadFile for T {}

/// 書くファイル。
pub trait WriteFile: Write + Send {
    /// 書いた内容をディスクに書き出す（共有フォルダでは送り先のサーバーまで）。
    fn sync(&mut self) -> io::Result<()>;
}

/// ファイル操作。
pub trait Fs: Send + Sync {
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>>;
    /// 情報（リンクはたどらない）。
    fn metadata(&self, path: &Path) -> io::Result<Meta>;
    fn open_read(&self, path: &Path) -> io::Result<Box<dyn ReadFile>>;
    /// 書くために開く（なければ作る）。大きさを `keep` バイトに切り詰めて、その終わりから書く。
    fn open_write(&self, path: &Path, keep: u64) -> io::Result<Box<dyn WriteFile>>;
    /// 名前を変える（`to` があれば置き換える）。
    fn rename_replace(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// 名前を変える（`to` があればエラー）。
    fn rename_new(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    /// 空のフォルダを消す。
    fn remove_dir(&self, path: &Path) -> io::Result<()>;
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    /// 更新日時を合わせる（ナノ秒）。
    fn set_mtime(&self, path: &Path, mtime: i64) -> io::Result<()>;
    fn set_readonly(&self, path: &Path, readonly: bool) -> io::Result<()>;
}

/// 時刻を UNIX 時刻からのナノ秒にする。
pub fn to_nanos(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos().min(i64::MAX as u128) as i64,
        Err(e) => -(e.duration().as_nanos().min(i64::MAX as u128) as i64),
    }
}

/// UNIX 時刻からのナノ秒を時刻にする。
pub fn from_nanos(n: i64) -> SystemTime {
    if n >= 0 {
        UNIX_EPOCH + Duration::from_nanos(n as u64)
    } else {
        UNIX_EPOCH - Duration::from_nanos(n.unsigned_abs())
    }
}

fn meta_of(m: &fs::Metadata) -> Meta {
    let mtime = m.modified().map(to_nanos).unwrap_or(0);
    let ctime = m.created().map(to_nanos).unwrap_or(0);
    #[cfg(windows)]
    let (hidden, link) = {
        use std::os::windows::fs::MetadataExt;
        let a = m.file_attributes();
        // FILE_ATTRIBUTE_HIDDEN・FILE_ATTRIBUTE_REPARSE_POINT
        (a & 0x2 != 0, a & 0x400 != 0 || m.file_type().is_symlink())
    };
    #[cfg(not(windows))]
    let (hidden, link) = (false, m.file_type().is_symlink());
    #[cfg(unix)]
    let file_id = {
        use std::os::unix::fs::MetadataExt;
        Some(((m.dev() as u128) << 64) | m.ino() as u128)
    };
    #[cfg(not(unix))]
    let file_id = None;
    Meta {
        size: if m.is_dir() { 0 } else { m.len() },
        mtime,
        ctime,
        dir: m.is_dir(),
        readonly: m.permissions().readonly(),
        hidden,
        link,
        file_id,
    }
}

/// 手元のファイルシステム（Windows では共有フォルダも）。
#[derive(Clone, Copy, Debug, Default)]
pub struct Local;

struct LocalWrite(File);

impl Write for LocalWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl WriteFile for LocalWrite {
    fn sync(&mut self) -> io::Result<()> {
        self.0.sync_data()
    }
}

impl Fs for Local {
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for e in fs::read_dir(path)? {
            let e = e?;
            // 項目ごとの情報が読めなければ飛ばす（消えた・権限がない）
            let Ok(m) = fs::symlink_metadata(e.path()) else {
                continue;
            };
            out.push(DirEntry {
                name: e.file_name().to_string_lossy().into_owned(),
                meta: meta_of(&m),
            });
        }
        Ok(out)
    }

    fn metadata(&self, path: &Path) -> io::Result<Meta> {
        Ok(meta_of(&fs::symlink_metadata(path)?))
    }

    fn open_read(&self, path: &Path) -> io::Result<Box<dyn ReadFile>> {
        Ok(Box::new(File::open(path)?))
    }

    fn open_write(&self, path: &Path, keep: u64) -> io::Result<Box<dyn WriteFile>> {
        let mut f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        if f.metadata()?.len() != keep {
            f.set_len(keep)?;
        }
        f.seek(SeekFrom::Start(keep))?;
        Ok(Box::new(LocalWrite(f)))
    }

    fn rename_replace(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn rename_new(&self, from: &Path, to: &Path) -> io::Result<()> {
        if fs::symlink_metadata(to).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} は既にあります", to.display()),
            ));
        }
        fs::rename(from, to)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            // 読み取り専用のファイル（Windows）は属性を外してから
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                let mut p = fs::metadata(path)?.permissions();
                if !p.readonly() {
                    return Err(e);
                }
                #[allow(clippy::permissions_set_readonly_false)]
                p.set_readonly(false);
                fs::set_permissions(path, p)?;
                fs::remove_file(path)
            }
            r => r,
        }
    }

    fn remove_dir(&self, path: &Path) -> io::Result<()> {
        fs::remove_dir(path)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }

    fn set_mtime(&self, path: &Path, mtime: i64) -> io::Result<()> {
        let f = OpenOptions::new().write(true).open(path)?;
        f.set_modified(from_nanos(mtime))
    }

    fn set_readonly(&self, path: &Path, readonly: bool) -> io::Result<()> {
        let mut p = fs::metadata(path)?.permissions();
        if p.readonly() == readonly {
            return Ok(());
        }
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(readonly);
        fs::set_permissions(path, p)
    }
}

// ---- 試験用 ------------------------------------------------------------------------------

/// [`Faulty`] が「アプリが落ちた」ことを表すエラー。
#[derive(Debug)]
pub struct Crashed;

impl std::fmt::Display for Crashed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("（試験）アプリが落ちました")
    }
}

impl std::error::Error for Crashed {}

/// 落ちたことによるエラーか。
pub fn is_crash(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|i| i.is::<Crashed>())
}

/// 試験用: 決めた回数の操作のあとで、回線が切れる（その操作だけ失敗する）か、アプリが落ちる（その後の
/// 操作はすべて失敗する。書き込みは途中まで残る）。
pub struct Faulty {
    inner: Local,
    state: Arc<FaultState>,
}

#[derive(Default)]
struct FaultState {
    /// 落ちるまでに許す変更の操作の数（`u64::MAX` は落ちない）
    crash_after: AtomicU64,
    /// 何回目ごとの書き込みで回線が切れるか（0 は切れない）
    drop_every: AtomicU64,
    ops: AtomicU64,
    writes: AtomicU64,
}

impl Faulty {
    /// `crash_after` 回の変更の操作のあとで落ち、`drop_every` 回目ごとの書き込みで回線が切れる。
    pub fn new(crash_after: u64, drop_every: u64) -> Faulty {
        let state = FaultState::default();
        state.crash_after.store(crash_after, Ordering::SeqCst);
        state.drop_every.store(drop_every, Ordering::SeqCst);
        Faulty {
            inner: Local,
            state: Arc::new(state),
        }
    }

    /// 落ちたか。
    pub fn crashed(&self) -> bool {
        self.state.ops.load(Ordering::SeqCst) >= self.state.crash_after.load(Ordering::SeqCst)
    }

    /// 変更の操作を数える（落ちていればエラー）。
    fn op(&self) -> io::Result<()> {
        let n = self.state.ops.fetch_add(1, Ordering::SeqCst);
        if n >= self.state.crash_after.load(Ordering::SeqCst) {
            return Err(io::Error::other(Crashed));
        }
        Ok(())
    }
}

struct FaultyWrite {
    inner: Box<dyn WriteFile>,
    state: Arc<FaultState>,
}

impl Write for FaultyWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = &self.state;
        let n = s.ops.fetch_add(1, Ordering::SeqCst);
        if n >= s.crash_after.load(Ordering::SeqCst) {
            // 落ちる直前の書き込みは途中まで残る
            if n == s.crash_after.load(Ordering::SeqCst) && buf.len() > 1 {
                let _ = self.inner.write(&buf[..buf.len() / 2]);
                let _ = self.inner.flush();
            }
            return Err(io::Error::other(Crashed));
        }
        let every = s.drop_every.load(Ordering::SeqCst);
        let w = s.writes.fetch_add(1, Ordering::SeqCst) + 1;
        if every > 0 && w % every == 0 {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "（試験）回線が切れました",
            ));
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl WriteFile for FaultyWrite {
    fn sync(&mut self) -> io::Result<()> {
        if self.state.ops.load(Ordering::SeqCst) >= self.state.crash_after.load(Ordering::SeqCst) {
            return Err(io::Error::other(Crashed));
        }
        self.inner.sync()
    }
}

impl Fs for Faulty {
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        if self.crashed() {
            return Err(io::Error::other(Crashed));
        }
        self.inner.read_dir(path)
    }
    fn metadata(&self, path: &Path) -> io::Result<Meta> {
        if self.crashed() {
            return Err(io::Error::other(Crashed));
        }
        self.inner.metadata(path)
    }
    fn open_read(&self, path: &Path) -> io::Result<Box<dyn ReadFile>> {
        if self.crashed() {
            return Err(io::Error::other(Crashed));
        }
        self.inner.open_read(path)
    }
    fn open_write(&self, path: &Path, keep: u64) -> io::Result<Box<dyn WriteFile>> {
        self.op()?;
        Ok(Box::new(FaultyWrite {
            inner: self.inner.open_write(path, keep)?,
            state: self.state.clone(),
        }))
    }
    fn rename_replace(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.op()?;
        self.inner.rename_replace(from, to)
    }
    fn rename_new(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.op()?;
        self.inner.rename_new(from, to)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.op()?;
        self.inner.remove_file(path)
    }
    fn remove_dir(&self, path: &Path) -> io::Result<()> {
        self.op()?;
        self.inner.remove_dir(path)
    }
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.op()?;
        self.inner.create_dir_all(path)
    }
    fn set_mtime(&self, path: &Path, mtime: i64) -> io::Result<()> {
        self.op()?;
        self.inner.set_mtime(path, mtime)
    }
    fn set_readonly(&self, path: &Path, readonly: bool) -> io::Result<()> {
        self.op()?;
        self.inner.set_readonly(path, readonly)
    }
}

/// 回線が切れたなど、待って続ければ直るかもしれないエラーか（18 章 4.4）。
pub fn is_transient(e: &io::Error) -> bool {
    use io::ErrorKind::*;
    if matches!(
        e.kind(),
        ConnectionReset
            | ConnectionAborted
            | NotConnected
            | BrokenPipe
            | TimedOut
            | UnexpectedEof
            | Interrupted
            | NetworkDown
            | NetworkUnreachable
            | HostUnreachable
    ) {
        return true;
    }
    // Windows の共有フォルダのエラー: ERROR_BAD_NETPATH (53)・ERROR_NETWORK_BUSY (54)・
    // ERROR_UNEXP_NET_ERR (59)・ERROR_NETNAME_DELETED (64)・ERROR_BAD_NET_NAME (67)・
    // ERROR_SEM_TIMEOUT (121)・ERROR_NETWORK_UNREACHABLE (1231)・ERROR_CONNECTION_ABORTED (1236)
    cfg!(windows)
        && matches!(
            e.raw_os_error(),
            Some(53 | 54 | 59 | 64 | 67 | 121 | 1231 | 1236)
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_operations() {
        let d = tempfile::tempdir().unwrap();
        let fs = Local;
        let p = d.path().join("a.txt");
        let mut w = fs.open_write(&p, 0).unwrap();
        w.write_all(b"hello world").unwrap();
        w.sync().unwrap();
        drop(w);
        // 切り詰めて続きを書く
        let mut w = fs.open_write(&p, 5).unwrap();
        w.write_all(b"!!").unwrap();
        drop(w);
        assert_eq!(std::fs::read(&p).unwrap(), b"hello!!");
        let t = 1_700_000_000_123_456_700i64;
        fs.set_mtime(&p, t).unwrap();
        let m = fs.metadata(&p).unwrap();
        assert_eq!(m.size, 7);
        // ファイルシステムの時刻の細かさ（100 ns 以下）
        assert!((m.mtime - t).abs() < 1_000, "{} {t}", m.mtime);
        let q = d.path().join("b.txt");
        fs.rename_new(&p, &q).unwrap();
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(
            fs.rename_new(&q, &p).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        fs.rename_replace(&q, &p).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello!!");
        fs.set_readonly(&p, true).unwrap();
        assert!(fs.metadata(&p).unwrap().readonly);
        fs.remove_file(&p).unwrap();
        assert!(fs.read_dir(d.path()).unwrap().is_empty());
        assert_eq!(
            from_nanos(to_nanos(UNIX_EPOCH - Duration::from_secs(5))),
            UNIX_EPOCH - Duration::from_secs(5)
        );
    }

    #[test]
    fn faulty_crashes_and_drops() {
        let d = tempfile::tempdir().unwrap();
        let fs = Faulty::new(2, 0);
        let p = d.path().join("a");
        let mut w = fs.open_write(&p, 0).unwrap(); // 1
        w.write_all(b"1234").unwrap(); // 2
        let e = w.write_all(b"abcdef").unwrap_err(); // 3: 半分だけ書いて落ちる
        assert!(is_crash(&e));
        assert!(fs.crashed());
        assert!(is_crash(&fs.metadata(&p).unwrap_err()));
        assert_eq!(std::fs::read(&p).unwrap(), b"1234abc");
        let fs = Faulty::new(u64::MAX, 2);
        let mut w = fs.open_write(&p, 0).unwrap();
        w.write_all(b"a").unwrap();
        let e = w.write_all(b"b").unwrap_err();
        assert!(is_transient(&e));
        w.write_all(b"c").unwrap();
    }
}

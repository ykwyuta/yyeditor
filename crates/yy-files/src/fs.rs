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

/// 書き換えるファイル（差分の送り方で、変わったブロックだけを書く）。
pub trait PatchFile: Write + Seek + Send {
    fn sync(&mut self) -> io::Result<()>;
    fn set_len(&mut self, len: u64) -> io::Result<()>;
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
    /// ファイルを写す（`to` があれば置き換える）。同じ共有フォルダの中では、Windows の `CopyFileEx` が
    /// SMB のサーバー側コピーになり、回線を使わない。
    fn copy_file(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// 書き換えるために開く（大きさはそのまま）。
    fn open_patch(&self, path: &Path) -> io::Result<Box<dyn PatchFile>>;
    /// アクセス権を写す（Windows は DACL の明示的な項目と継承の止め方。ほかの OS は属性のモード）。
    fn copy_acl(&self, from: &Path, to: &Path) -> io::Result<()>;
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

struct LocalPatch(File);

impl Write for LocalPatch {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Seek for LocalPatch {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0.seek(pos)
    }
}

impl PatchFile for LocalPatch {
    fn sync(&mut self) -> io::Result<()> {
        self.0.sync_data()
    }
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.0.set_len(len)
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

    fn copy_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        // 読み取り専用の写しは書き換えられないので外す（Windows の CopyFileEx は属性も写す）
        fs::copy(from, to)?;
        self.set_readonly(to, false)
    }

    fn open_patch(&self, path: &Path) -> io::Result<Box<dyn PatchFile>> {
        Ok(Box::new(LocalPatch(
            OpenOptions::new().read(true).write(true).open(path)?,
        )))
    }

    fn copy_acl(&self, from: &Path, to: &Path) -> io::Result<()> {
        copy_acl_local(from, to)
    }
}

/// アクセス権を写す（Windows）。送り元の DACL の明示的な項目を写し、送り元が継承を止めていれば送り先も
/// 止める（継承する項目は送り先の親から受け継ぐ）。所有者・監査（SACL）は写さない（特権が要るため）。
#[cfg(windows)]
#[allow(unsafe_code)] // Win32 のセキュリティ API（このクレートで unsafe を使うのはここと試験だけ）
fn copy_acl_local(from: &Path, to: &Path) -> io::Result<()> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        GetNamedSecurityInfoW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows::core::HSTRING;
    let src = HSTRING::from(from.as_os_str());
    let dst = HSTRING::from(to.as_os_str());
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe {
        let r = GetNamedSecurityInfoW(
            &src,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut dacl),
            None,
            &mut sd,
        );
        if r.0 != 0 {
            return Err(io::Error::from_raw_os_error(r.0 as i32));
        }
        let mut control = 0u16;
        let mut rev = 0u32;
        let protected = GetSecurityDescriptorControl(sd, &mut control, &mut rev).is_ok()
            && control & SE_DACL_PROTECTED.0 != 0;
        let info = DACL_SECURITY_INFORMATION
            | if protected {
                PROTECTED_DACL_SECURITY_INFORMATION
            } else {
                UNPROTECTED_DACL_SECURITY_INFORMATION
            };
        let r = SetNamedSecurityInfoW(
            &dst,
            SE_FILE_OBJECT,
            info,
            None,
            None,
            Some(dacl as *const ACL),
            None,
        );
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        if r.0 != 0 {
            return Err(io::Error::from_raw_os_error(r.0 as i32));
        }
    }
    Ok(())
}

/// アクセス権を写す（Windows 以外: 属性のモード）。
#[cfg(not(windows))]
fn copy_acl_local(from: &Path, to: &Path) -> io::Result<()> {
    fs::set_permissions(to, fs::metadata(from)?.permissions())
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

/// 試験用の書き込み: 落ちる・回線が切れるをまねる。
fn faulty_write(s: &FaultState, inner: &mut dyn Write, buf: &[u8]) -> io::Result<usize> {
    let n = s.ops.fetch_add(1, Ordering::SeqCst);
    if n >= s.crash_after.load(Ordering::SeqCst) {
        // 落ちる直前の書き込みは途中まで残る
        if n == s.crash_after.load(Ordering::SeqCst) && buf.len() > 1 {
            let _ = inner.write(&buf[..buf.len() / 2]);
            let _ = inner.flush();
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
    inner.write(buf)
}

impl Write for FaultyWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        faulty_write(&self.state, &mut self.inner, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct FaultyPatch {
    inner: Box<dyn PatchFile>,
    state: Arc<FaultState>,
}

impl Write for FaultyPatch {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        faulty_write(&self.state, &mut self.inner, buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Seek for FaultyPatch {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl PatchFile for FaultyPatch {
    fn sync(&mut self) -> io::Result<()> {
        if self.state.ops.load(Ordering::SeqCst) >= self.state.crash_after.load(Ordering::SeqCst) {
            return Err(io::Error::other(Crashed));
        }
        self.inner.sync()
    }
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        let n = self.state.ops.fetch_add(1, Ordering::SeqCst);
        if n >= self.state.crash_after.load(Ordering::SeqCst) {
            return Err(io::Error::other(Crashed));
        }
        self.inner.set_len(len)
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
    fn copy_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.op()?;
        self.inner.copy_file(from, to)
    }
    fn copy_acl(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.op()?;
        self.inner.copy_acl(from, to)
    }
    fn open_patch(&self, path: &Path) -> io::Result<Box<dyn PatchFile>> {
        self.op()?;
        Ok(Box::new(FaultyPatch {
            inner: self.inner.open_patch(path)?,
            state: self.state.clone(),
        }))
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
    #[cfg_attr(windows, allow(unsafe_code))]
    fn copies_access_rights() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a.txt"), d.path().join("b.txt"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&a, fs::Permissions::from_mode(0o640)).unwrap();
            Local.copy_acl(&a, &b).unwrap();
            assert_eq!(
                std::fs::metadata(&b).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::{HLOCAL, LocalFree};
            use windows::Win32::Security::Authorization::*;
            use windows::Win32::Security::*;
            use windows::core::{HSTRING, PWSTR};
            // 継承を止めた DACL（Administrators にフル、Everyone に読み取り）を送り元に付ける
            let sddl = HSTRING::from("D:P(A;;FA;;;BA)(A;;FR;;;WD)");
            let dacl_of = |p: &Path| -> String {
                unsafe {
                    let mut sd = PSECURITY_DESCRIPTOR::default();
                    let r = GetNamedSecurityInfoW(
                        &HSTRING::from(p.as_os_str()),
                        SE_FILE_OBJECT,
                        DACL_SECURITY_INFORMATION,
                        None,
                        None,
                        None,
                        None,
                        &mut sd,
                    );
                    assert_eq!(r.0, 0);
                    let mut s = PWSTR::null();
                    ConvertSecurityDescriptorToStringSecurityDescriptorW(
                        sd,
                        SDDL_REVISION_1,
                        DACL_SECURITY_INFORMATION,
                        &mut s,
                        None,
                    )
                    .unwrap();
                    let out = s.to_string().unwrap();
                    let _ = LocalFree(Some(HLOCAL(s.0 as *mut _)));
                    let _ = LocalFree(Some(HLOCAL(sd.0)));
                    out
                }
            };
            unsafe {
                let mut sd = PSECURITY_DESCRIPTOR::default();
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    &sddl,
                    SDDL_REVISION_1,
                    &mut sd,
                    None,
                )
                .unwrap();
                let mut present = windows::core::BOOL(0);
                let mut defaulted = windows::core::BOOL(0);
                let mut dacl: *mut ACL = std::ptr::null_mut();
                GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted).unwrap();
                let r = SetNamedSecurityInfoW(
                    &HSTRING::from(a.as_os_str()),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    None,
                    None,
                    Some(dacl as *const ACL),
                    None,
                );
                assert_eq!(r.0, 0);
                let _ = LocalFree(Some(HLOCAL(sd.0)));
            }
            assert_ne!(dacl_of(&a), dacl_of(&b));
            Local.copy_acl(&a, &b).unwrap();
            assert_eq!(dacl_of(&a), dacl_of(&b));
            assert!(dacl_of(&b).starts_with("D:P"), "{}", dacl_of(&b));
        }
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

//! SSH 接続先で動くエージェント（11 章 6）。
//!
//! yyeditor が SSH の exec チャネルで起動し、標準入出力で [`yy_proto`] の要求に答える。
//! ポートは待ち受けず、チャネルが閉じたら（標準入力が EOF になったら）終わる。
//! 編集中の状態はすべて端末が持つので、エージェントはファイルの読み出しと保存だけを行う。

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use yy_proto::{
    Block, DirEntry, FileId, FileInfo, FileKind, RemoteError, Request, Response, VERSION,
};

/// 1 回の読み出しで返す上限
const READ_MAX: u32 = 4 * yy_proto::CHUNK;
/// フォルダの一覧の応答の大きさの目安の上限（フレームの上限より小さく）
const DIR_BYTES_MAX: usize = 6 << 20;

/// 要求に順に答える。`input` が閉じられたら終わる。
pub fn serve(input: impl Read, output: impl Write) -> io::Result<()> {
    let mut input = BufReader::with_capacity(1 << 20, input);
    let mut output = BufWriter::with_capacity(1 << 20, output);
    let mut agent = Agent::default();
    while let Some((id, req)) = yy_proto::read_frame::<_, Request>(&mut input)? {
        let resp = agent.handle(req);
        yy_proto::write_frame(&mut output, id, &resp)?;
        // 続けて届いている要求があれば、まとめて書き出す
        if input.buffer().is_empty() {
            output.flush()?;
        }
    }
    output.flush()
}

#[derive(Default)]
struct Agent {
    next: u32,
    files: HashMap<u32, File>,
    uploads: HashMap<u32, Upload>,
}

/// 受け取り中の保存内容。
struct Upload {
    /// 保存先（シンボリックリンクはリンク先）
    target: PathBuf,
    /// 保存先と同じフォルダの一時ファイル
    temp: PathBuf,
    file: Option<File>,
}

impl Drop for Upload {
    fn drop(&mut self) {
        // 置き換えに使わなかった一時ファイルは残さない
        if self.file.is_some() {
            let _ = fs::remove_file(&self.temp);
        }
    }
}

impl Agent {
    fn handle(&mut self, req: Request) -> Response {
        match self.try_handle(req) {
            Ok(r) => r,
            Err(e) => Response::Error(RemoteError::from(&e)),
        }
    }

    fn id(&mut self) -> u32 {
        self.next = self.next.wrapping_add(1);
        self.next
    }

    fn try_handle(&mut self, req: Request) -> io::Result<Response> {
        Ok(match req {
            Request::Hello { .. } => Response::Hello {
                version: VERSION,
                agent_version: env!("CARGO_PKG_VERSION").to_owned(),
                os: std::env::consts::OS.to_owned(),
                arch: std::env::consts::ARCH.to_owned(),
                home: std::env::var_os("HOME")
                    .map(|h| path_bytes(Path::new(&h)))
                    .unwrap_or_else(|| b"/".to_vec()),
            },
            Request::RealPath { path } => {
                Response::Path(path_bytes(&fs::canonicalize(to_path(&path))?))
            }
            Request::Stat { path } => Response::Info(stat(&to_path(&path))?),
            Request::ReadDir { path } => Response::Dir(read_dir(&to_path(&path))?),
            Request::Open { path } => {
                let path = to_path(&path);
                let file = File::open(&path)?;
                let info = stat(&path)?;
                if info.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "フォルダは開けません",
                    ));
                }
                let handle = self.id();
                self.files.insert(handle, file);
                Response::Opened { handle, info }
            }
            Request::Read {
                handle,
                offset,
                len,
            } => {
                let file = self.files.get(&handle).ok_or_else(bad_handle)?;
                let mut buf = vec![0u8; len.min(READ_MAX) as usize];
                let n = read_at(file, &mut buf, offset)?;
                buf.truncate(n);
                Response::Data(Block::pack(&buf))
            }
            Request::Close { handle } => {
                self.files.remove(&handle);
                Response::Done
            }
            Request::BeginUpload { path } => {
                let upload = begin_upload(&to_path(&path))?;
                let id = self.id();
                self.uploads.insert(id, upload);
                Response::Upload(id)
            }
            Request::Write { upload, data } => {
                let u = self.uploads.get_mut(&upload).ok_or_else(bad_handle)?;
                let file = u.file.as_mut().ok_or_else(bad_handle)?;
                if let Err(e) = file.write_all(&data.unpack()?) {
                    // 書けなかった内容で置き換えないよう、受け取りをやめる
                    self.uploads.remove(&upload);
                    return Err(e);
                }
                Response::Done
            }
            Request::Commit {
                upload,
                expected,
                force,
            } => {
                let u = self.uploads.get_mut(&upload).ok_or_else(bad_handle)?;
                match commit(u, expected, force) {
                    Ok(Some(info)) => {
                        self.uploads.remove(&upload);
                        Response::Committed(info)
                    }
                    Ok(None) => Response::Conflict(stat(&u.target).ok()),
                    Err(e) => {
                        self.uploads.remove(&upload);
                        return Err(e);
                    }
                }
            }
            Request::Abort { upload } => {
                self.uploads.remove(&upload);
                Response::Done
            }
            Request::MakeDir { path } => {
                fs::create_dir(to_path(&path))?;
                Response::Done
            }
            Request::Rename { from, to } => {
                rename(&to_path(&from), &to_path(&to))?;
                Response::Done
            }
            Request::Remove { path, recursive } => {
                remove(&to_path(&path), recursive)?;
                Response::Done
            }
            Request::Copy { from, to } => {
                copy(&to_path(&from), &to_path(&to))?;
                Response::Done
            }
            Request::Hash { path, len } => Response::Hash(hash_prefix(&to_path(&path), len)?),
        })
    }
}

/// ファイルの先頭から `len` バイトの SHA-256。
fn hash_prefix(path: &Path, len: u64) -> io::Result<Vec<u8>> {
    use sha2::{Digest, Sha256};
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    if size < len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("ファイルが短すぎます（{size} / {len} バイト）"),
        ));
    }
    let mut r = BufReader::with_capacity(1 << 20, file.take(len));
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().to_vec())
}

fn bad_handle() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "不正なハンドルです")
}

#[cfg(unix)]
fn to_path(p: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(p))
}

#[cfg(not(unix))]
fn to_path(p: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(p).into_owned())
}

#[cfg(unix)]
fn path_bytes(p: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(p: &Path) -> Vec<u8> {
    p.to_string_lossy().as_bytes().to_vec()
}

#[cfg(unix)]
fn name_bytes(n: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    n.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn name_bytes(n: &std::ffi::OsStr) -> Vec<u8> {
    n.to_string_lossy().as_bytes().to_vec()
}

fn mtime_ns(m: &fs::Metadata) -> i64 {
    match m.modified() {
        Ok(t) => match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_nanos().min(i64::MAX as u128) as i64,
            Err(e) => -(e.duration().as_nanos().min(i64::MAX as u128) as i64),
        },
        Err(_) => 0,
    }
}

#[cfg(unix)]
fn info_of(m: &fs::Metadata, link: bool) -> FileInfo {
    use std::os::unix::fs::MetadataExt;
    FileInfo {
        kind: kind_of(m),
        link,
        mode: m.mode() & 0o7777,
        nlink: m.nlink(),
        id: FileId {
            dev: m.dev(),
            ino: m.ino(),
            len: m.len(),
            mtime_ns: mtime_ns(m),
        },
    }
}

#[cfg(not(unix))]
fn info_of(m: &fs::Metadata, link: bool) -> FileInfo {
    FileInfo {
        kind: kind_of(m),
        link,
        mode: if m.permissions().readonly() {
            0o444
        } else {
            0o644
        },
        nlink: 1,
        id: FileId {
            dev: 0,
            ino: 0,
            len: m.len(),
            mtime_ns: mtime_ns(m),
        },
    }
}

fn kind_of(m: &fs::Metadata) -> FileKind {
    if m.is_file() {
        FileKind::File
    } else if m.is_dir() {
        FileKind::Dir
    } else {
        FileKind::Other
    }
}

/// ファイルの情報（シンボリックリンクはリンク先）。
fn stat(path: &Path) -> io::Result<FileInfo> {
    let link = fs::symlink_metadata(path)?.file_type().is_symlink();
    Ok(info_of(&fs::metadata(path)?, link))
}

fn read_dir(path: &Path) -> io::Result<Vec<DirEntry>> {
    let mut out = Vec::new();
    let mut bytes = 0;
    for e in fs::read_dir(path)? {
        let e = e?;
        let name = name_bytes(&e.file_name());
        let link = e.file_type().is_ok_and(|t| t.is_symlink());
        let info = fs::metadata(e.path()).ok().map(|m| info_of(&m, link));
        bytes += name.len() + 64;
        if bytes > DIR_BYTES_MAX {
            break;
        }
        out.push(DirEntry { name, info });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[cfg(unix)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        match file.read_at(&mut buf[done..], offset + done as u64) {
            Ok(0) => break,
            Ok(n) => done += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(done)
}

#[cfg(windows)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        match file.seek_read(&mut buf[done..], offset + done as u64) {
            Ok(0) => break,
            Ok(n) => done += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(done)
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 保存先と、同じフォルダの一時ファイルを用意する。
fn begin_upload(path: &Path) -> io::Result<Upload> {
    // シンボリックリンクはリンク自体を残してリンク先を置き換える（11 章 7.1）
    let target = match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => fs::canonicalize(path)?,
        _ => path.to_owned(),
    };
    let existing = fs::metadata(&target).ok();
    if existing.as_ref().is_some_and(|m| m.is_dir()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "フォルダには保存できません",
        ));
    }
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_owned(),
        _ => PathBuf::from("."),
    };
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut attempts = 0;
    let (temp, file) = loop {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = dir.join(format!(".{name}.yytmp-{}-{n}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(f) => break (temp, f),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempts < 100 => attempts += 1,
            Err(e) => return Err(e),
        }
    };
    let upload = Upload {
        target,
        temp,
        file: Some(file),
    };
    if let Some(m) = existing {
        // 元のファイルの権限・所有者を引き継ぐ（所有者は変えられる場合だけ）
        let file = upload.file.as_ref().expect("just created");
        file.set_permissions(m.permissions())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let _ = std::os::unix::fs::fchown(file, Some(m.uid()), Some(m.gid()));
        }
    }
    Ok(upload)
}

/// 受け取った内容で保存先を置き換える。競合したら `None`（一時ファイルは残す）。
fn commit(u: &mut Upload, expected: Option<FileId>, force: bool) -> io::Result<Option<FileInfo>> {
    let file = u.file.as_ref().ok_or_else(bad_handle)?;
    file.sync_all()?;
    let current = fs::metadata(&u.target).ok();
    if !force {
        let ok = match (&current, expected) {
            // 開いたときから変わっていない（消えていれば作り直す）
            (Some(m), Some(id)) => info_of(m, false).id == id,
            (None, Some(_)) => true,
            // 新しく作るつもりなのに既にある
            (Some(_), None) => false,
            (None, None) => true,
        };
        if !ok {
            return Ok(None);
        }
    }
    let file = u.file.take().expect("checked above");
    let hard_linked = current
        .as_ref()
        .is_some_and(|m| info_of(m, false).nlink > 1);
    let replaced = if hard_linked {
        // rename するとハードリンクが切れるので、元のファイルに書き写す
        drop(file);
        copy_into(&u.temp, &u.target)
    } else {
        drop(file);
        fs::rename(&u.temp, &u.target)
    };
    if let Err(e) = replaced {
        let _ = fs::remove_file(&u.temp);
        return Err(e);
    }
    if hard_linked {
        let _ = fs::remove_file(&u.temp);
    }
    sync_dir(&u.target);
    Ok(Some(stat(&u.target)?))
}

/// 名前を変える・移動する（上書きしない）。
fn rename(from: &Path, to: &Path) -> io::Result<()> {
    fs::symlink_metadata(from)?;
    if fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} は既にあります", to.display()),
        ));
    }
    if is_within(to, from) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "フォルダをその中には移動できません",
        ));
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        // 別のファイルシステムへの移動は、コピーしてから元を消す
        Err(e) if is_cross_device(&e) => {
            if let Err(e) = copy_tree(from, to) {
                let _ = remove(to, true);
                return Err(e);
            }
            remove(from, true)
        }
        Err(e) => Err(e),
    }
}

/// 中身ごとコピーする（上書きしない）。途中で失敗したら、作りかけのコピーを消す。
fn copy(from: &Path, to: &Path) -> io::Result<()> {
    fs::symlink_metadata(from)?;
    if fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} は既にあります", to.display()),
        ));
    }
    if is_within(to, from) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "フォルダをその中へはコピーできません",
        ));
    }
    copy_tree(from, to).inspect_err(|_| {
        let _ = remove(to, true);
    })
}

/// `path` が `dir` 自身か、その中か（パスの文字列で比べる）。
fn is_within(path: &Path, dir: &Path) -> bool {
    let (Ok(p), Ok(d)) = (
        fs::canonicalize(path.parent().unwrap_or(path)),
        fs::canonicalize(dir),
    ) else {
        return path.starts_with(dir);
    };
    p.starts_with(&d)
}

#[cfg(unix)]
fn is_cross_device(e: &io::Error) -> bool {
    // EXDEV
    e.raw_os_error() == Some(18)
}

#[cfg(not(unix))]
fn is_cross_device(e: &io::Error) -> bool {
    // ERROR_NOT_SAME_DEVICE
    e.raw_os_error() == Some(17)
}

/// ファイル・フォルダを中身ごとコピーする（権限も。シンボリックリンクはリンクとして）。
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(fs::read_link(from)?, to);
        #[cfg(not(unix))]
        return fs::copy(from, to).map(|_| ());
    }
    if meta.is_dir() {
        fs::create_dir(to)?;
        for e in fs::read_dir(from)? {
            let e = e?;
            copy_tree(&e.path(), &to.join(e.file_name()))?;
        }
        fs::set_permissions(to, meta.permissions())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

/// ファイル・フォルダを消す。
fn remove(path: &Path, recursive: bool) -> io::Result<()> {
    if path.parent().is_none() || path == Path::new("/") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ルートは消せません",
        ));
    }
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        if recursive {
            fs::remove_dir_all(path)
        } else {
            fs::remove_dir(path)
        }
    } else {
        fs::remove_file(path)
    }
}

fn copy_into(from: &Path, to: &Path) -> io::Result<()> {
    let mut src = File::open(from)?;
    let mut dst = OpenOptions::new().write(true).truncate(true).open(to)?;
    io::copy(&mut src, &mut dst)?;
    dst.sync_all()
}

/// rename を永続化する（フォルダの fsync）。
fn sync_dir(target: &Path) {
    #[cfg(unix)]
    if let Some(dir) = target.parent() {
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = target;
}

/// 自分自身（実行ファイル）の SHA-256（16 進数）。端末は配置したファイルとの一致を確かめる。
pub fn self_sha256() -> io::Result<String> {
    let mut f = File::open(std::env::current_exe()?)?;
    sha256_of(&mut f)
}

/// 読み出した内容の SHA-256（16 進数の小文字）。
pub fn sha256_of(r: &mut dyn Read) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// 配置先のフォルダの古い版を消す（11 章 6.2）。
///
/// エージェントは `<配置先>/<版>-<ハッシュ>/yy-agent` に置かれる。自分のフォルダに使った印を付け、
/// 同じ配置先にある他の版のうち `max_age` より長く使われていないものを消す。
/// `yy-agent` を含まないフォルダには触らない。
pub fn clean_old_versions(exe: &Path, max_age: Duration) {
    let Some(own) = exe.parent() else { return };
    let Some(root) = own.parent() else { return };
    let _ = fs::write(own.join(".last-used"), b"");
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for e in entries.flatten() {
        let dir = e.path();
        if dir == own || !dir.join("yy-agent").is_file() {
            continue;
        }
        let used = fs::metadata(dir.join(".last-used"))
            .or_else(|_| fs::metadata(dir.join("yy-agent")))
            .and_then(|m| m.modified());
        let old = used
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > max_age);
        if old {
            let _ = fs::remove_dir_all(&dir);
        }
    }
}

// エージェントは Linux で動かす（Windows では開いているファイルを rename で置き換えられない）
#[cfg(all(test, unix))]
mod tests;

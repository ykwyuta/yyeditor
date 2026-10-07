//! SFTP（版 3）のクライアント（13 章 3）。
//!
//! SSH の `sftp` サブシステム（[`crate::Transport::subsystem`]）の上で話す。接続先には
//! OpenSSH の sftp-server などがあればよく、エージェントは使わない。要求には ID を付けて送り、
//! 応答は読み出し用のスレッドが ID ごとの待ち手に渡すので、読み書きの要求を並べて
//! （応答を待たずに）送り、回線の遅延を隠せる（巨大なファイルの転送に使う）。
//!
//! パスはバイト列で扱う（接続先のファイル名が UTF-8 とは限らない）。

use std::collections::HashMap;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender, bounded};

use crate::{Process, Transport};

// 要求・応答の種類
const SSH_FXP_INIT: u8 = 1;
const SSH_FXP_VERSION: u8 = 2;
const SSH_FXP_OPEN: u8 = 3;
const SSH_FXP_CLOSE: u8 = 4;
const SSH_FXP_READ: u8 = 5;
const SSH_FXP_WRITE: u8 = 6;
const SSH_FXP_LSTAT: u8 = 7;
const SSH_FXP_FSTAT: u8 = 8;
const SSH_FXP_SETSTAT: u8 = 9;
const SSH_FXP_OPENDIR: u8 = 11;
const SSH_FXP_READDIR: u8 = 12;
const SSH_FXP_REMOVE: u8 = 13;
const SSH_FXP_MKDIR: u8 = 14;
const SSH_FXP_RMDIR: u8 = 15;
const SSH_FXP_REALPATH: u8 = 16;
const SSH_FXP_STAT: u8 = 17;
const SSH_FXP_RENAME: u8 = 18;
const SSH_FXP_STATUS: u8 = 101;
const SSH_FXP_HANDLE: u8 = 102;
const SSH_FXP_DATA: u8 = 103;
const SSH_FXP_NAME: u8 = 104;
const SSH_FXP_ATTRS: u8 = 105;
const SSH_FXP_EXTENDED: u8 = 200;

// 状態の番号
const SSH_FX_OK: u32 = 0;
const SSH_FX_EOF: u32 = 1;
const SSH_FX_NO_SUCH_FILE: u32 = 2;
const SSH_FX_PERMISSION_DENIED: u32 = 3;
const SSH_FX_NO_CONNECTION: u32 = 6;
const SSH_FX_CONNECTION_LOST: u32 = 7;
const SSH_FX_OP_UNSUPPORTED: u32 = 8;

// 属性
const ATTR_SIZE: u32 = 1;
const ATTR_UIDGID: u32 = 2;
const ATTR_PERMISSIONS: u32 = 4;
const ATTR_ACMODTIME: u32 = 8;
const ATTR_EXTENDED: u32 = 0x8000_0000;

/// ファイルを開くときの指定。
pub mod open {
    pub const READ: u32 = 1;
    pub const WRITE: u32 = 2;
    pub const APPEND: u32 = 4;
    pub const CREATE: u32 = 8;
    pub const TRUNCATE: u32 = 0x10;
    pub const EXCLUSIVE: u32 = 0x20;
}

/// 1 回の読み書きの大きさ（OpenSSH などが受け付ける大きさ）
pub const CHUNK: u32 = 32 * 1024;
/// 応答を待たずに送っておく読み書きの数（32 KiB × 64 = 2 MiB）
pub const WINDOW: usize = 64;
/// 受け付ける応答の大きさの上限
const PACKET_LIMIT: usize = 4 << 20;

/// ファイルの属性。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attrs {
    pub size: Option<u64>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub permissions: Option<u32>,
    pub atime: Option<u32>,
    pub mtime: Option<u32>,
}

impl Attrs {
    pub fn is_dir(&self) -> bool {
        self.permissions.is_some_and(|p| p & 0o170000 == 0o040000)
    }

    pub fn is_symlink(&self) -> bool {
        self.permissions.is_some_and(|p| p & 0o170000 == 0o120000)
    }

    /// `drwxr-xr-x` の形。
    pub fn mode_string(&self) -> String {
        let Some(p) = self.permissions else {
            return String::new();
        };
        let kind = match p & 0o170000 {
            0o040000 => 'd',
            0o120000 => 'l',
            0o010000 => 'p',
            0o140000 => 's',
            0o060000 => 'b',
            0o020000 => 'c',
            _ => '-',
        };
        let mut s = String::from(kind);
        for shift in [6, 3, 0] {
            let bits = (p >> shift) & 7;
            s.push(if bits & 4 != 0 { 'r' } else { '-' });
            s.push(if bits & 2 != 0 { 'w' } else { '-' });
            s.push(if bits & 1 != 0 { 'x' } else { '-' });
        }
        s
    }
}

/// ディレクトリの 1 項目。
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: Vec<u8>,
    pub attrs: Attrs,
}

/// 開いたファイル・ディレクトリ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handle(Vec<u8>);

/// 応答。
#[derive(Debug)]
enum Reply {
    Status { code: u32, message: String },
    Handle(Vec<u8>),
    Data(Vec<u8>),
    Name(Vec<Entry>),
    Attrs(Attrs),
}

/// SFTP の状態をエラーにする（接続が切れた場合は再接続して続けられる種類にする）。
fn status_error(code: u32, message: &str, what: &str) -> io::Error {
    let kind = match code {
        SSH_FX_NO_SUCH_FILE => io::ErrorKind::NotFound,
        SSH_FX_PERMISSION_DENIED => io::ErrorKind::PermissionDenied,
        SSH_FX_NO_CONNECTION | SSH_FX_CONNECTION_LOST => io::ErrorKind::ConnectionAborted,
        SSH_FX_OP_UNSUPPORTED => io::ErrorKind::Unsupported,
        SSH_FX_EOF => io::ErrorKind::UnexpectedEof,
        _ => io::ErrorKind::Other,
    };
    let name = match code {
        SSH_FX_EOF => "EOF",
        SSH_FX_NO_SUCH_FILE => "NO_SUCH_FILE",
        SSH_FX_PERMISSION_DENIED => "PERMISSION_DENIED",
        4 => "FAILURE",
        5 => "BAD_MESSAGE",
        SSH_FX_NO_CONNECTION => "NO_CONNECTION",
        SSH_FX_CONNECTION_LOST => "CONNECTION_LOST",
        SSH_FX_OP_UNSUPPORTED => "OP_UNSUPPORTED",
        _ => "?",
    };
    let detail = if message.is_empty() {
        String::new()
    } else {
        format!("「{message}」")
    };
    io::Error::new(
        kind,
        format!("{what}: SFTP の状態 {code}（{name}）{detail}"),
    )
}

fn bad_reply(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{what}: SFTP の応答が不正です"),
    )
}

// ---- 符号化 ---------------------------------------------------------------------

struct Packet(Vec<u8>);

impl Packet {
    fn new(kind: u8) -> Packet {
        Packet(vec![0, 0, 0, 0, kind])
    }
    fn u32(mut self, v: u32) -> Packet {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn u64(mut self, v: u64) -> Packet {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn bytes(mut self, b: &[u8]) -> Packet {
        self = self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
        self
    }
    fn attrs(mut self, a: &Attrs) -> Packet {
        let mut flags = 0;
        if a.size.is_some() {
            flags |= ATTR_SIZE;
        }
        if a.uid.is_some() && a.gid.is_some() {
            flags |= ATTR_UIDGID;
        }
        if a.permissions.is_some() {
            flags |= ATTR_PERMISSIONS;
        }
        if a.atime.is_some() && a.mtime.is_some() {
            flags |= ATTR_ACMODTIME;
        }
        self = self.u32(flags);
        if let Some(s) = a.size {
            self = self.u64(s);
        }
        if let (Some(u), Some(g)) = (a.uid, a.gid) {
            self = self.u32(u).u32(g);
        }
        if let Some(p) = a.permissions {
            self = self.u32(p);
        }
        if let (Some(at), Some(mt)) = (a.atime, a.mtime) {
            self = self.u32(at).u32(mt);
        }
        self
    }
    fn finish(mut self) -> Vec<u8> {
        let len = (self.0.len() - 4) as u32;
        self.0[..4].copy_from_slice(&len.to_be_bytes());
        self.0
    }
}

/// 読み取り位置つきの応答の中身。
struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|b| u64::from_be_bytes(b.try_into().unwrap()))
    }
    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn attrs(&mut self) -> Option<Attrs> {
        let flags = self.u32()?;
        let mut a = Attrs::default();
        if flags & ATTR_SIZE != 0 {
            a.size = Some(self.u64()?);
        }
        if flags & ATTR_UIDGID != 0 {
            a.uid = Some(self.u32()?);
            a.gid = Some(self.u32()?);
        }
        if flags & ATTR_PERMISSIONS != 0 {
            a.permissions = Some(self.u32()?);
        }
        if flags & ATTR_ACMODTIME != 0 {
            a.atime = Some(self.u32()?);
            a.mtime = Some(self.u32()?);
        }
        if flags & ATTR_EXTENDED != 0 {
            let n = self.u32()?;
            for _ in 0..n {
                self.bytes()?;
                self.bytes()?;
            }
        }
        Some(a)
    }
}

fn parse_reply(kind: u8, c: &mut Cursor) -> Option<Reply> {
    Some(match kind {
        SSH_FXP_STATUS => {
            let code = c.u32()?;
            // 古いサーバーは説明を付けない
            let message = c
                .bytes()
                .map(|m| String::from_utf8_lossy(m).into_owned())
                .unwrap_or_default();
            Reply::Status { code, message }
        }
        SSH_FXP_HANDLE => Reply::Handle(c.bytes()?.to_vec()),
        SSH_FXP_DATA => Reply::Data(c.bytes()?.to_vec()),
        SSH_FXP_NAME => {
            let n = c.u32()?;
            let mut v = Vec::with_capacity(n.min(4096) as usize);
            for _ in 0..n {
                let name = c.bytes()?.to_vec();
                let _long = c.bytes()?;
                let attrs = c.attrs()?;
                v.push(Entry { name, attrs });
            }
            Reply::Name(v)
        }
        SSH_FXP_ATTRS => Reply::Attrs(c.attrs()?),
        _ => return None,
    })
}

// ---- 送受信 ---------------------------------------------------------------------

struct Shared {
    writer: Mutex<Option<BufWriter<Box<dyn Write + Send>>>>,
    waiting: Mutex<Waiting>,
    next: AtomicU32,
}

#[derive(Default)]
struct Waiting {
    map: HashMap<u32, Sender<Reply>>,
    closed: Option<String>,
}

impl Shared {
    fn closed_error(&self) -> io::Error {
        let reason = self.waiting.lock().unwrap().closed.clone();
        io::Error::new(
            io::ErrorKind::ConnectionAborted,
            reason.unwrap_or_else(|| "SFTP の接続が切れました".to_owned()),
        )
    }

    fn close(&self, reason: String) {
        let mut w = self.waiting.lock().unwrap();
        if w.closed.is_none() {
            w.closed = Some(reason);
        }
        // 待っている要求を起こす（送り口を捨てると受け取り側がエラーになる）
        w.map.clear();
        drop(w);
        self.writer.lock().unwrap().take();
    }
}

/// 送った要求への応答を待つもの。
pub struct Pending {
    rx: Receiver<Reply>,
    shared: Arc<Shared>,
}

impl Pending {
    fn wait(self) -> io::Result<Reply> {
        self.rx.recv().map_err(|_| self.shared.closed_error())
    }
}

/// SFTP のクライアント。
pub struct Sftp {
    shared: Arc<Shared>,
    version: u32,
    extensions: Vec<(String, String)>,
}

impl Drop for Sftp {
    fn drop(&mut self) {
        // 送り口を閉じると sftp-server は終わる
        self.shared.close("SFTP を閉じました".into());
    }
}

impl Sftp {
    /// `sftp` サブシステムを起動して始める。
    pub fn connect(t: &dyn Transport) -> io::Result<Sftp> {
        Sftp::start(t.subsystem("sftp")?)
    }

    /// 起動したサブシステム（または sftp-server）と、初期化のやり取りをする。
    pub fn start(p: Process) -> io::Result<Sftp> {
        let (stdin, stdout, finish) = p.into_parts();
        let mut reader = BufReader::with_capacity(256 << 10, stdout);
        let mut writer = BufWriter::with_capacity(256 << 10, stdin);
        writer.write_all(&Packet::new(SSH_FXP_INIT).u32(3).finish())?;
        writer.flush()?;
        let (kind, body) = read_packet(&mut reader).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("SFTP を始められませんでした（接続先に sftp-server がないか、許可されていません）: {e}"),
            )
        })?;
        if kind != SSH_FXP_VERSION {
            return Err(bad_reply("SFTP の初期化"));
        }
        let mut c = Cursor { b: &body, pos: 0 };
        let version = c.u32().ok_or_else(|| bad_reply("SFTP の初期化"))?;
        let mut extensions = Vec::new();
        while let (Some(n), Some(d)) = (c.bytes(), c.bytes()) {
            extensions.push((
                String::from_utf8_lossy(n).into_owned(),
                String::from_utf8_lossy(d).into_owned(),
            ));
        }
        let shared = Arc::new(Shared {
            writer: Mutex::new(Some(writer)),
            waiting: Mutex::new(Waiting::default()),
            next: AtomicU32::new(1),
        });
        let s = shared.clone();
        std::thread::spawn(move || {
            let reason = loop {
                match read_packet(&mut reader) {
                    Ok((kind, body)) => {
                        let mut c = Cursor { b: &body, pos: 0 };
                        let Some(id) = c.u32() else {
                            break "SFTP の応答が不正です".to_owned();
                        };
                        let Some(reply) = parse_reply(kind, &mut c) else {
                            break format!("SFTP の応答（種類 {kind}）を読めません");
                        };
                        let tx = s.waiting.lock().unwrap().map.remove(&id);
                        if let Some(tx) = tx {
                            let _ = tx.send(reply);
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                        break "SFTP の接続が切れました".to_owned();
                    }
                    Err(e) => break format!("SFTP の接続が切れました: {e}"),
                }
            };
            s.close(reason);
            // 読み口を閉じてから終わりを待つ（相手が書き込みで止まっていても終わるように）
            drop(reader);
            let _ = finish();
        });
        Ok(Sftp {
            shared,
            version,
            extensions,
        })
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    /// サーバーの拡張（`posix-rename@openssh.com` など）。
    pub fn extensions(&self) -> &[(String, String)] {
        &self.extensions
    }

    fn has_extension(&self, name: &str) -> bool {
        self.extensions.iter().any(|(n, _)| n == name)
    }

    pub fn is_closed(&self) -> bool {
        self.shared.waiting.lock().unwrap().closed.is_some()
    }

    /// 要求を送る（`body` は ID の後ろ）。
    fn send(&self, kind: u8, build: impl FnOnce(Packet) -> Packet) -> io::Result<Pending> {
        let id = self.shared.next.fetch_add(1, Ordering::Relaxed);
        let packet = build(Packet::new(kind).u32(id)).finish();
        let (tx, rx) = bounded(1);
        {
            let mut w = self.shared.waiting.lock().unwrap();
            if w.closed.is_some() {
                drop(w);
                return Err(self.shared.closed_error());
            }
            w.map.insert(id, tx);
        }
        let mut writer = self.shared.writer.lock().unwrap();
        let r = match writer.as_mut() {
            Some(w) => w.write_all(&packet).and_then(|()| w.flush()),
            None => Err(io::Error::from(io::ErrorKind::BrokenPipe)),
        };
        drop(writer);
        if let Err(e) = r {
            self.shared.close(format!("SFTP の接続が切れました: {e}"));
            return Err(self.shared.closed_error());
        }
        Ok(Pending {
            rx,
            shared: self.shared.clone(),
        })
    }

    fn call(&self, kind: u8, build: impl FnOnce(Packet) -> Packet) -> io::Result<Reply> {
        self.send(kind, build)?.wait()
    }

    fn status(r: Reply, what: &str) -> io::Result<()> {
        match r {
            Reply::Status {
                code: SSH_FX_OK, ..
            } => Ok(()),
            Reply::Status { code, message } => Err(status_error(code, &message, what)),
            _ => Err(bad_reply(what)),
        }
    }

    fn what(op: &str, path: &[u8]) -> String {
        format!("{op} {}", crate::display(path))
    }

    /// 絶対パスにする（`.` でホーム）。
    pub fn realpath(&self, path: &[u8]) -> io::Result<Vec<u8>> {
        let what = Sftp::what("パスの解決", path);
        match self.call(SSH_FXP_REALPATH, |p| p.bytes(path))? {
            Reply::Name(mut v) if !v.is_empty() => Ok(v.remove(0).name),
            Reply::Status { code, message } => Err(status_error(code, &message, &what)),
            _ => Err(bad_reply(&what)),
        }
    }

    fn attrs_of(&self, kind: u8, path: &[u8]) -> io::Result<Attrs> {
        let what = Sftp::what("属性の取得", path);
        match self.call(kind, |p| p.bytes(path))? {
            Reply::Attrs(a) => Ok(a),
            Reply::Status { code, message } => Err(status_error(code, &message, &what)),
            _ => Err(bad_reply(&what)),
        }
    }

    /// 属性（シンボリックリンクはたどる）。
    pub fn stat(&self, path: &[u8]) -> io::Result<Attrs> {
        self.attrs_of(SSH_FXP_STAT, path)
    }

    /// 属性（シンボリックリンクはたどらない）。
    pub fn lstat(&self, path: &[u8]) -> io::Result<Attrs> {
        self.attrs_of(SSH_FXP_LSTAT, path)
    }

    /// あれば属性、なければ `None`。
    pub fn try_stat(&self, path: &[u8]) -> io::Result<Option<Attrs>> {
        match self.stat(path) {
            Ok(a) => Ok(Some(a)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn fstat(&self, h: &Handle) -> io::Result<Attrs> {
        match self.call(SSH_FXP_FSTAT, |p| p.bytes(&h.0))? {
            Reply::Attrs(a) => Ok(a),
            Reply::Status { code, message } => Err(status_error(code, &message, "属性の取得")),
            _ => Err(bad_reply("属性の取得")),
        }
    }

    /// フォルダの中身（`.` と `..` を除く）。
    pub fn read_dir(&self, path: &[u8]) -> io::Result<Vec<Entry>> {
        let what = Sftp::what("フォルダの読み出し", path);
        let h = match self.call(SSH_FXP_OPENDIR, |p| p.bytes(path))? {
            Reply::Handle(h) => Handle(h),
            Reply::Status { code, message } => return Err(status_error(code, &message, &what)),
            _ => return Err(bad_reply(&what)),
        };
        let mut out = Vec::new();
        let r = loop {
            match self.call(SSH_FXP_READDIR, |p| p.bytes(&h.0)) {
                Ok(Reply::Name(v)) => {
                    out.extend(v.into_iter().filter(|e| e.name != b"." && e.name != b".."));
                }
                Ok(Reply::Status {
                    code: SSH_FX_EOF, ..
                }) => break Ok(()),
                Ok(Reply::Status { code, message }) => {
                    break Err(status_error(code, &message, &what));
                }
                Ok(_) => break Err(bad_reply(&what)),
                Err(e) => break Err(e),
            }
        };
        let _ = self.close(&h);
        r.map(|()| out)
    }

    pub fn open(&self, path: &[u8], flags: u32, attrs: &Attrs) -> io::Result<Handle> {
        let what = Sftp::what("ファイルを開く", path);
        match self.call(SSH_FXP_OPEN, |p| p.bytes(path).u32(flags).attrs(attrs))? {
            Reply::Handle(h) => Ok(Handle(h)),
            Reply::Status { code, message } => Err(status_error(code, &message, &what)),
            _ => Err(bad_reply(&what)),
        }
    }

    pub fn close(&self, h: &Handle) -> io::Result<()> {
        Sftp::status(
            self.call(SSH_FXP_CLOSE, |p| p.bytes(&h.0))?,
            "ファイルを閉じる",
        )
    }

    /// 読み出しを送る（応答は [`ReadReply::wait`]）。
    pub fn send_read(&self, h: &Handle, offset: u64, len: u32) -> io::Result<ReadReply> {
        Ok(ReadReply(self.send(SSH_FXP_READ, |p| {
            p.bytes(&h.0).u64(offset).u32(len)
        })?))
    }

    /// 書き込みを送る（応答は [`WriteReply::wait`]）。
    pub fn send_write(&self, h: &Handle, offset: u64, data: &[u8]) -> io::Result<WriteReply> {
        Ok(WriteReply(self.send(SSH_FXP_WRITE, |p| {
            p.bytes(&h.0).u64(offset).bytes(data)
        })?))
    }

    /// 書いた内容を接続先のディスクに書き出す（`fsync@openssh.com` があれば）。書き出したら `true`。
    pub fn fsync(&self, h: &Handle) -> io::Result<bool> {
        if !self.has_extension("fsync@openssh.com") {
            return Ok(false);
        }
        let r = self.call(SSH_FXP_EXTENDED, |p| {
            p.bytes(b"fsync@openssh.com").bytes(&h.0)
        })?;
        Sftp::status(r, "fsync").map(|()| true)
    }

    pub fn mkdir(&self, path: &[u8]) -> io::Result<()> {
        let what = Sftp::what("フォルダの作成", path);
        Sftp::status(
            self.call(SSH_FXP_MKDIR, |p| p.bytes(path).attrs(&Attrs::default()))?,
            &what,
        )
    }

    pub fn rmdir(&self, path: &[u8]) -> io::Result<()> {
        let what = Sftp::what("フォルダの削除", path);
        Sftp::status(self.call(SSH_FXP_RMDIR, |p| p.bytes(path))?, &what)
    }

    pub fn remove(&self, path: &[u8]) -> io::Result<()> {
        let what = Sftp::what("ファイルの削除", path);
        Sftp::status(self.call(SSH_FXP_REMOVE, |p| p.bytes(path))?, &what)
    }

    /// 名前を変える。`overwrite` なら `to` があっても置き換える（`posix-rename@openssh.com`
    /// があればそれで、なければ消してから）。
    pub fn rename(&self, from: &[u8], to: &[u8], overwrite: bool) -> io::Result<()> {
        let what = format!(
            "名前の変更 {} → {}",
            crate::display(from),
            crate::display(to)
        );
        if overwrite && self.has_extension("posix-rename@openssh.com") {
            let r = self.call(SSH_FXP_EXTENDED, |p| {
                p.bytes(b"posix-rename@openssh.com").bytes(from).bytes(to)
            })?;
            return Sftp::status(r, &what);
        }
        if overwrite && self.try_stat(to)?.is_some() {
            self.remove(to)?;
        }
        Sftp::status(
            self.call(SSH_FXP_RENAME, |p| p.bytes(from).bytes(to))?,
            &what,
        )
    }

    pub fn setstat(&self, path: &[u8], attrs: &Attrs) -> io::Result<()> {
        let what = Sftp::what("属性の設定", path);
        Sftp::status(
            self.call(SSH_FXP_SETSTAT, |p| p.bytes(path).attrs(attrs))?,
            &what,
        )
    }

    /// フォルダを中身ごと消す。
    pub fn remove_all(&self, path: &[u8]) -> io::Result<()> {
        let a = self.lstat(path)?;
        if !a.is_dir() {
            return self.remove(path);
        }
        for e in self.read_dir(path)? {
            let child = crate::join_remote(path, &e.name);
            if e.attrs.is_dir() && !e.attrs.is_symlink() {
                self.remove_all(&child)?;
            } else {
                self.remove(&child)?;
            }
        }
        self.rmdir(path)
    }
}

/// 送った読み出しの応答。
pub struct ReadReply(Pending);

impl ReadReply {
    /// 読んだデータ（ファイルの終わりなら空）。
    pub fn wait(self) -> io::Result<Vec<u8>> {
        match self.0.wait()? {
            Reply::Data(d) => Ok(d),
            Reply::Status {
                code: SSH_FX_EOF, ..
            } => Ok(Vec::new()),
            Reply::Status { code, message } => Err(status_error(code, &message, "読み出し")),
            _ => Err(bad_reply("読み出し")),
        }
    }
}

/// 送った書き込みの応答。
pub struct WriteReply(Pending);

impl WriteReply {
    pub fn wait(self) -> io::Result<()> {
        Sftp::status(self.0.wait()?, "書き込み")
    }
}

fn read_packet(r: &mut dyn Read) -> io::Result<(u8, Vec<u8>)> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > PACKET_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("SFTP の応答の長さが不正です（{len}）"),
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let kind = body[0];
    body.remove(0);
    Ok((kind, body))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::local::{LocalTransport, sftp_server};
    use std::os::unix::ffi::OsStrExt;

    fn client() -> Option<Sftp> {
        if sftp_server().is_none() {
            eprintln!("sftp-server がないため飛ばします");
            return None;
        }
        Some(Sftp::connect(&LocalTransport::new()).unwrap())
    }

    #[test]
    fn file_operations() {
        let Some(s) = client() else { return };
        assert_eq!(s.version(), 3);
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().as_os_str().as_bytes().to_vec();
        let p = |n: &str| crate::join_remote(&d, n.as_bytes());
        // 書いて読む（要求を並べる）
        let h = s
            .open(
                &p("a.bin"),
                open::WRITE | open::CREATE | open::TRUNCATE,
                &Attrs::default(),
            )
            .unwrap();
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let pending: Vec<_> = data
            .chunks(CHUNK as usize)
            .enumerate()
            .map(|(i, c)| s.send_write(&h, (i * CHUNK as usize) as u64, c).unwrap())
            .collect();
        for w in pending {
            w.wait().unwrap();
        }
        s.close(&h).unwrap();
        assert_eq!(std::fs::read(dir.path().join("a.bin")).unwrap(), data);
        let a = s.stat(&p("a.bin")).unwrap();
        assert_eq!(a.size, Some(300_000));
        assert!(!a.is_dir());
        let h = s.open(&p("a.bin"), open::READ, &Attrs::default()).unwrap();
        assert_eq!(
            s.send_read(&h, 299_990, 100).unwrap().wait().unwrap(),
            &data[299_990..]
        );
        assert!(
            s.send_read(&h, 300_000, 100)
                .unwrap()
                .wait()
                .unwrap()
                .is_empty()
        );
        s.close(&h).unwrap();
        // フォルダ・名前の変更・属性・削除
        s.mkdir(&p("sub")).unwrap();
        assert!(s.stat(&p("sub")).unwrap().is_dir());
        s.rename(&p("a.bin"), &p("sub/b.bin"), false).unwrap();
        std::fs::write(dir.path().join("c.txt"), "c").unwrap();
        s.rename(&p("c.txt"), &p("sub/b.bin"), true).unwrap();
        assert_eq!(std::fs::read(dir.path().join("sub/b.bin")).unwrap(), b"c");
        s.setstat(
            &p("sub/b.bin"),
            &Attrs {
                atime: Some(1_000_000_000),
                mtime: Some(1_000_000_000),
                ..Attrs::default()
            },
        )
        .unwrap();
        assert_eq!(s.stat(&p("sub/b.bin")).unwrap().mtime, Some(1_000_000_000));
        let names: Vec<_> = s
            .read_dir(&d)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, [b"sub".to_vec()]);
        let e = s.stat(&p("none")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert!(e.to_string().contains("NO_SUCH_FILE"), "{e}");
        assert!(s.try_stat(&p("none")).unwrap().is_none());
        s.remove_all(&p("sub")).unwrap();
        assert!(s.read_dir(&d).unwrap().is_empty());
        assert_eq!(
            s.realpath(&d).unwrap(),
            std::fs::canonicalize(dir.path())
                .unwrap()
                .as_os_str()
                .as_bytes()
        );
    }

    #[test]
    fn mode_strings() {
        let a = Attrs {
            permissions: Some(0o040755),
            ..Attrs::default()
        };
        assert_eq!(a.mode_string(), "drwxr-xr-x");
        assert!(a.is_dir());
        let a = Attrs {
            permissions: Some(0o100640),
            ..Attrs::default()
        };
        assert_eq!(a.mode_string(), "-rw-r-----");
    }
}

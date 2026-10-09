//! 手元と接続先の間のファイル・フォルダのコピー（ワークスペースのコピー・貼り付け）。
//!
//! コピー元とコピー先は、それぞれ手元のパスか接続先のパス（[`Loc`]）。どの組み合わせでもよい。
//! 同じ接続先の中のコピーはエージェントが接続先で行い、転送しない。別の接続先の間は、手元の
//! 一時ファイルを経由する。
//!
//! - 上書きはしない（コピー先が既にあればエラー）。途中で失敗・中止したら、作りかけのコピーを消す。
//! - ファイルを指すシンボリックリンクは指す先の内容をコピーし、フォルダを指すリンクは
//!   たどらずに飛ばす（循環を避けるため。飛ばした数は [`CopyStats::skipped`]）。
//! - 接続先の名前が UTF-8 でなければ、手元の名前では置き換え文字になる。

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use yy_proto::FileKind;

use crate::sftp::{Attrs, Sftp};
use crate::{RemoteFs, Session, SftpFs, UploadOutcome};

/// コピー元・コピー先の場所。
#[derive(Clone)]
pub enum Loc {
    Local(PathBuf),
    /// 接続先のセッション（エージェント）と、その中の絶対パス
    Remote(Arc<Session>, Vec<u8>),
    /// 接続先の SFTP（エージェントを置かない）と、その中の絶対パス
    Sftp(Arc<SftpFs>, Vec<u8>),
}

/// コピーした量。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CopyStats {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    /// 飛ばしたもの（フォルダを指すシンボリックリンク、特殊ファイル）
    pub skipped: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
    /// コピーしない（フォルダを指すリンク、特殊ファイル、リンク切れ）
    Skip,
}

/// フォルダの中の名前（手元は OS の文字列、接続先はバイト列）。
enum Name {
    Os(OsString),
    Bytes(Vec<u8>),
}

/// Convert one remote filename to a portable local name. Never interpret it as a path.
pub fn local_file_name(name: &[u8]) -> io::Result<OsString> {
    let name = String::from_utf8_lossy(name);
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|n| {
                matches!(
                    n,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.ends_with(['.', ' '])
        || reserved
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "安全なローカルファイル名に変換できません",
        ));
    }
    Ok(OsString::from(name.into_owned()))
}

fn remote_file_name(name: &[u8]) -> io::Result<()> {
    if name.is_empty() || name == b"." || name == b".." || name.contains(&b'/') || name.contains(&0)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "接続先から不正なファイル名を受け取りました",
        ));
    }
    Ok(())
}

impl Loc {
    /// 接続先なら、そのファイル操作とパス。
    fn remote_fs(&self) -> Option<(&dyn RemoteFs, &[u8])> {
        match self {
            Loc::Local(_) => None,
            Loc::Remote(s, p) => Some((s.as_ref() as &dyn RemoteFs, p.as_slice())),
            Loc::Sftp(s, p) => Some((s.as_ref() as &dyn RemoteFs, p.as_slice())),
        }
    }

    fn join(&self, name: &Name) -> io::Result<Loc> {
        Ok(match self {
            Loc::Local(p) => Loc::Local(p.join(match name {
                Name::Os(n) => n.clone(),
                Name::Bytes(b) => local_file_name(b)?,
            })),
            Loc::Remote(_, p) | Loc::Sftp(_, p) => {
                let n = match name {
                    Name::Os(n) => n.to_string_lossy().into_owned().into_bytes(),
                    Name::Bytes(b) => b.clone(),
                };
                remote_file_name(&n)?;
                let path = yy_proto::join_path(p, &n);
                match self {
                    Loc::Remote(s, _) => Loc::Remote(s.clone(), path),
                    Loc::Sftp(s, _) => Loc::Sftp(s.clone(), path),
                    Loc::Local(_) => unreachable!(),
                }
            }
        })
    }

    fn kind(&self) -> io::Result<Kind> {
        match self {
            Loc::Local(p) => {
                let m = fs::symlink_metadata(p)?;
                Ok(if m.file_type().is_symlink() {
                    match fs::metadata(p) {
                        Ok(t) if t.is_file() => Kind::File,
                        _ => Kind::Skip,
                    }
                } else if m.is_dir() {
                    Kind::Dir
                } else if m.is_file() {
                    Kind::File
                } else {
                    Kind::Skip
                })
            }
            _ => {
                let (fs, p) = self.remote_fs().expect("remote");
                let i = fs.stat(p)?;
                Ok(remote_kind(i.kind, i.link))
            }
        }
    }

    fn exists(&self) -> io::Result<bool> {
        let r = match self.remote_fs() {
            None => match self {
                Loc::Local(p) => fs::symlink_metadata(p).map(|_| ()),
                _ => unreachable!(),
            },
            Some((fs, p)) => fs.stat(p).map(|_| ()),
        };
        match r {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn children(&self) -> io::Result<Vec<(Name, Kind)>> {
        match self {
            Loc::Local(p) => {
                let mut out = Vec::new();
                for e in fs::read_dir(p)? {
                    let e = e?;
                    let kind = Loc::Local(e.path()).kind().unwrap_or(Kind::Skip);
                    out.push((Name::Os(e.file_name()), kind));
                }
                Ok(out)
            }
            _ => {
                let (fs, p) = self.remote_fs().expect("remote");
                Ok(fs
                    .read_dir(p)?
                    .into_iter()
                    .map(|e| {
                        remote_file_name(&e.name)?;
                        let kind = e
                            .info
                            .as_ref()
                            .map_or(Kind::Skip, |i| remote_kind(i.kind, i.link));
                        Ok((Name::Bytes(e.name), kind))
                    })
                    .collect::<io::Result<Vec<_>>>()?)
            }
        }
    }

    fn make_dir(&self) -> io::Result<()> {
        match self {
            Loc::Local(p) => fs::create_dir(p),
            _ => {
                let (fs, p) = self.remote_fs().expect("remote");
                fs.make_dir(p)
            }
        }
    }

    fn remove_all(&self) -> io::Result<()> {
        match self {
            Loc::Local(p) => {
                if fs::symlink_metadata(p)?.is_dir() {
                    fs::remove_dir_all(p)
                } else {
                    fs::remove_file(p)
                }
            }
            _ => {
                let (fs, p) = self.remote_fs().expect("remote");
                fs.remove(p, true)
            }
        }
    }
}

fn remote_kind(kind: FileKind, link: bool) -> Kind {
    match kind {
        FileKind::File => Kind::File,
        FileKind::Dir if !link => Kind::Dir,
        _ => Kind::Skip,
    }
}

/// 中止したことを表すエラー。`ErrorKind::Interrupted` は `io::copy` などが「やり直し」と
/// みなして読み直し続けるので使わない。
#[derive(Debug)]
struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("コピーを中止しました")
    }
}

impl std::error::Error for Cancelled {}

fn cancelled() -> io::Error {
    io::Error::other(Cancelled)
}

/// [`copy`] を中止したことによるエラーか。
pub fn is_cancelled(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<Cancelled>())
}

/// 進みを数えて知らせる。
struct Ticker<'a> {
    stats: CopyStats,
    step: &'a mut dyn FnMut(&CopyStats) -> bool,
}

impl Ticker<'_> {
    fn tick(&mut self) -> io::Result<()> {
        if (self.step)(&self.stats) {
            Ok(())
        } else {
            Err(cancelled())
        }
    }

    fn add_bytes(&mut self, n: u64) -> io::Result<()> {
        self.stats.bytes += n;
        self.tick()
    }
}

/// コピーする量を数える（コピーと同じく、フォルダを指すリンクはたどらない）。
pub fn measure(loc: &Loc, step: &mut dyn FnMut(&CopyStats) -> bool) -> io::Result<CopyStats> {
    let mut t = Ticker {
        stats: CopyStats::default(),
        step,
    };
    let kind = loc.kind()?;
    measure_node(loc, kind, &mut t)?;
    Ok(t.stats)
}

fn measure_node(loc: &Loc, kind: Kind, t: &mut Ticker) -> io::Result<()> {
    match kind {
        Kind::Skip => t.stats.skipped += 1,
        Kind::File => {
            t.stats.files += 1;
            t.stats.bytes += match loc.remote_fs() {
                Some((fs, p)) => fs.stat(p)?.len(),
                None => match loc {
                    Loc::Local(p) => fs::metadata(p)?.len(),
                    _ => unreachable!(),
                },
            };
        }
        Kind::Dir => {
            t.stats.dirs += 1;
            for (name, kind) in loc.children()? {
                measure_node(&loc.join(&name)?, kind, t)?;
            }
        }
    }
    t.tick()
}

/// 手元と接続先の間（または別の接続先の間）で、内容を転送するコピーか。
pub fn crosses_network(from: &Loc, to: &Loc) -> bool {
    match (from, to) {
        (Loc::Local(_), Loc::Local(_)) => false,
        (Loc::Remote(a, _), Loc::Remote(b, _)) => !Arc::ptr_eq(a, b),
        (Loc::Sftp(a, _), Loc::Sftp(b, _)) => !Arc::ptr_eq(a, b),
        _ => true,
    }
}

/// `from` を `to` にコピーする（`to` は新しく作る名前）。`step(これまでの量)` が `false` を
/// 返したら中止する。
pub fn copy(
    from: &Loc,
    to: &Loc,
    step: &mut dyn FnMut(&CopyStats) -> bool,
) -> io::Result<CopyStats> {
    if to.exists()? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "コピー先に同じ名前があります",
        ));
    }
    // 同じ接続先の中は、接続先でコピーする
    if let (Loc::Remote(a, f), Loc::Remote(b, t)) = (from, to)
        && Arc::ptr_eq(a, b)
    {
        a.copy(f, t)?;
        return Ok(CopyStats::default());
    }
    let mut ticker = Ticker {
        stats: CopyStats::default(),
        step,
    };
    let kind = from.kind()?;
    if kind == Kind::Skip {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "フォルダを指すリンクや特殊なファイルはコピーできません",
        ));
    }
    match copy_node(from, to, kind, &mut ticker) {
        Ok(()) => Ok(ticker.stats),
        Err(e) => {
            // 作りかけのコピーを消す（コピー先は新しく作ったものなので、消してよい）
            let _ = to.remove_all();
            Err(e)
        }
    }
}

fn copy_node(from: &Loc, to: &Loc, kind: Kind, t: &mut Ticker) -> io::Result<()> {
    match kind {
        Kind::Skip => {
            t.stats.skipped += 1;
            t.tick()
        }
        Kind::File => {
            copy_file(from, to, t)?;
            t.stats.files += 1;
            t.tick()
        }
        Kind::Dir => {
            to.make_dir()?;
            t.stats.dirs += 1;
            t.tick()?;
            for (name, kind) in from.children()? {
                copy_node(&from.join(&name)?, &to.join(&name)?, kind, t)?;
            }
            Ok(())
        }
    }
}

/// 読んだ量を数える `Read`。
struct Counting<'a, 'b, R> {
    inner: R,
    ticker: &'a mut Ticker<'b>,
}

impl<R: Read> Read for Counting<'_, '_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.ticker.add_bytes(n as u64)?;
        Ok(n)
    }
}

fn create_new(p: &std::path::Path) -> io::Result<File> {
    fs::OpenOptions::new().write(true).create_new(true).open(p)
}

fn copy_file(from: &Loc, to: &Loc, t: &mut Ticker) -> io::Result<()> {
    match (from, to) {
        (Loc::Local(f), Loc::Local(d)) => {
            let mut src = Counting {
                inner: File::open(f)?,
                ticker: t,
            };
            let mut dst = BufWriter::with_capacity(1 << 20, create_new(d)?);
            io::copy(&mut src, &mut dst)?;
            dst.into_inner().map_err(|e| e.into_error())?.sync_all()?;
            // 権限（読み取り専用など）も合わせる
            if let Ok(m) = fs::metadata(f) {
                let _ = fs::set_permissions(d, m.permissions());
            }
            Ok(())
        }
        (Loc::Local(f), _) => {
            let mut src = Counting {
                inner: File::open(f)?,
                ticker: t,
            };
            write_remote(to, &mut src)
        }
        (_, Loc::Local(d)) => {
            let mut dst = BufWriter::with_capacity(1 << 20, create_new(d)?);
            read_remote(from, &mut dst, t)?;
            dst.into_inner().map_err(|e| e.into_error())?.sync_all()
        }
        _ => {
            // 接続先の間（別の接続先・エージェントと SFTP）は、手元の一時ファイルを経由する
            let tmp = std::env::temp_dir().join(format!(
                "yyeditor-copy-{}-{}.tmp",
                std::process::id(),
                t.stats.files
            ));
            let r = (|| {
                let mut w = BufWriter::with_capacity(1 << 20, File::create(&tmp)?);
                read_remote(from, &mut w, t)?;
                w.flush()?;
                drop(w);
                write_remote(to, &mut File::open(&tmp)?)
            })();
            let _ = fs::remove_file(&tmp);
            r
        }
    }
}

/// 接続先のファイルを読んで `out` に書く。
fn read_remote(from: &Loc, out: &mut dyn Write, t: &mut Ticker) -> io::Result<()> {
    match from {
        Loc::Remote(s, f) => download(s, f, out, t),
        Loc::Sftp(s, f) => sftp_download(s.sftp(), f, out, t),
        Loc::Local(_) => unreachable!(),
    }
}

/// `src` を接続先のファイル（新しく作る）に書く。
fn write_remote(to: &Loc, src: &mut dyn Read) -> io::Result<()> {
    match to {
        Loc::Remote(s, d) => upload(s, src, d),
        Loc::Sftp(s, d) => sftp_upload(s.sftp(), src, d),
        Loc::Local(_) => unreachable!(),
    }
}

/// 1 回に読み書きする大きさと、返事を待たずに送る数（SFTP）。
const SFTP_CHUNK: u32 = 32 * 1024;
const SFTP_INFLIGHT: usize = 16;

/// SFTP でファイルを読む（読み出しを先に何個も送って、往復の待ちを減らす）。
fn sftp_download(s: &Sftp, path: &[u8], out: &mut dyn Write, t: &mut Ticker) -> io::Result<()> {
    use crate::sftp::open;
    let h = s.open(path, open::READ, &Attrs::default())?;
    let r = (|| {
        let mut queue = std::collections::VecDeque::new();
        let mut next = 0u64;
        loop {
            while queue.len() < SFTP_INFLIGHT {
                queue.push_back((next, s.send_read(&h, next, SFTP_CHUNK)?));
                next += u64::from(SFTP_CHUNK);
            }
            let Some((offset, reply)) = queue.pop_front() else {
                return Ok(());
            };
            let data = reply.wait()?;
            if data.is_empty() {
                // ファイルの終わり（残りの返事は読み捨てる）
                for (_, r) in queue.drain(..) {
                    let _ = r.wait();
                }
                return Ok(());
            }
            out.write_all(&data)?;
            t.add_bytes(data.len() as u64)?;
            if (data.len() as u32) < SFTP_CHUNK {
                // 短い読み出し: 続きをその位置から読み直す
                let rest = offset + data.len() as u64;
                for (_, r) in queue.drain(..) {
                    let _ = r.wait();
                }
                next = rest;
            }
        }
    })();
    let _ = s.close(&h);
    r
}

/// SFTP でファイルを書く（新しく作る。あればエラー）。
fn sftp_upload(s: &Sftp, src: &mut dyn Read, path: &[u8]) -> io::Result<()> {
    use crate::sftp::open;
    let h = s.open(
        path,
        open::WRITE | open::CREATE | open::EXCLUSIVE,
        &Attrs::default(),
    )?;
    let r = (|| {
        let mut queue = std::collections::VecDeque::new();
        let mut offset = 0u64;
        let mut buf = vec![0u8; SFTP_CHUNK as usize];
        loop {
            let n = read_full(src, &mut buf)?;
            if n == 0 {
                break;
            }
            queue.push_back(s.send_write(&h, offset, &buf[..n])?);
            offset += n as u64;
            if queue.len() >= SFTP_INFLIGHT
                && let Some(w) = queue.pop_front()
            {
                w.wait()?;
            }
        }
        for w in queue {
            w.wait()?;
        }
        Ok(())
    })();
    let _ = s.close(&h);
    r
}

/// `buf` が埋まるか終わりまで読む。
fn read_full(src: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match src.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

fn upload(s: &Arc<Session>, src: &mut dyn Read, path: &[u8]) -> io::Result<()> {
    match s.upload(src, path, None, &mut |_| true)? {
        UploadOutcome::Saved(_) => Ok(()),
        UploadOutcome::Conflict { .. } => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} は既にあります", yy_proto::display_path(path)),
        )),
    }
}

fn download(s: &Session, path: &[u8], out: &mut dyn Write, t: &mut Ticker) -> io::Result<()> {
    let mut last = 0u64;
    let mut stop = false;
    let r = s.download(path, out, &mut |done, _| {
        let delta = done.saturating_sub(last);
        last = done;
        if t.add_bytes(delta).is_err() {
            stop = true;
        }
        !stop
    });
    if stop {
        return Err(cancelled());
    }
    r.map(|_| ())
}

/// 場所のファイルの情報（なければ `None`）。手元のファイルは `None`（接続先のファイルの変更を
/// 見分けるためのもの）。
pub fn info(loc: &Loc) -> io::Result<Option<yy_proto::FileInfo>> {
    match loc.remote_fs() {
        Some((fs, p)) => fs.try_stat(p),
        None => Ok(None),
    }
}

/// 手元のファイル `local` の内容で、ファイル `to` を置き換える（なければ作る）。上書きしない
/// [`copy`] と違い、保存のためのもの。
///
/// 同じフォルダに一時の名前で書いてから名前を変えるので、途中で切れても元のファイルは残る
/// （エージェントは接続先で同じことをする）。元のファイルの権限は引き継ぐ。
pub fn replace(
    local: &std::path::Path,
    to: &Loc,
    step: &mut dyn FnMut(&CopyStats) -> bool,
) -> io::Result<()> {
    let mut ticker = Ticker {
        stats: CopyStats::default(),
        step,
    };
    let mut src = Counting {
        inner: File::open(local)?,
        ticker: &mut ticker,
    };
    match to {
        Loc::Local(d) => {
            let name = d.file_name().unwrap_or_default().to_string_lossy();
            let tmp = d.with_file_name(format!(".{name}.yy-{}.tmp", std::process::id()));
            let r = (|| {
                let mut w = BufWriter::with_capacity(1 << 20, create_new(&tmp)?);
                io::copy(&mut src, &mut w)?;
                w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
                if let Ok(m) = fs::metadata(d) {
                    let _ = fs::set_permissions(&tmp, m.permissions());
                }
                fs::rename(&tmp, d)
            })();
            if r.is_err() {
                let _ = fs::remove_file(&tmp);
            }
            r
        }
        Loc::Remote(s, d) => match s.upload(&mut src, d, None, &mut |_| true)? {
            UploadOutcome::Saved(_) => Ok(()),
            // 既にある（置き換える）
            UploadOutcome::Conflict { pending, .. } => pending.force().map(|_| ()),
        },
        Loc::Sftp(s, d) => {
            let sftp = s.sftp();
            let tmp = sibling_temp(d);
            let r = (|| {
                // 前に残った一時ファイルがあれば消す
                if sftp.try_stat(&tmp)?.is_some() {
                    sftp.remove(&tmp)?;
                }
                sftp_upload(sftp, &mut src, &tmp)?;
                if let Some(perm) = sftp.try_stat(d)?.and_then(|a| a.permissions) {
                    let _ = sftp.setstat(
                        &tmp,
                        &Attrs {
                            permissions: Some(perm & 0o7777),
                            ..Attrs::default()
                        },
                    );
                }
                sftp.rename(&tmp, d, true)
            })();
            if r.is_err() {
                let _ = sftp.remove(&tmp);
            }
            r
        }
    }
}

/// `path` と同じフォルダの一時の名前（`.名前.yy-プロセス番号.tmp`）。
fn sibling_temp(path: &[u8]) -> Vec<u8> {
    let cut = path.iter().rposition(|&b| b == b'/').map_or(0, |i| i + 1);
    let mut out = path[..cut].to_vec();
    out.push(b'.');
    out.extend_from_slice(&path[cut..]);
    out.extend_from_slice(format!(".yy-{}.tmp", std::process::id()).as_bytes());
    out
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn remote_names_cannot_escape_local_destination() {
        let root = tempfile::tempdir().unwrap();
        let destination = Loc::Local(root.path().join("downloads"));
        for name in [
            "..\\outside.txt",
            "../outside.txt",
            "C:\\outside.txt",
            "\\\\host\\share\\x",
            "x:stream",
            "CON.txt",
            "LPT1",
            "COM¹",
            "x.",
            "x ",
            ".",
            "..",
            "",
            "a\0b",
        ] {
            assert!(
                destination
                    .join(&Name::Bytes(name.as_bytes().to_vec()))
                    .is_err(),
                "{name:?}"
            );
        }
        let Loc::Local(path) = destination
            .join(&Name::Bytes("日本語.txt".as_bytes().to_vec()))
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(path, root.path().join("downloads/日本語.txt"));
        assert!(fs::read_dir(root.path()).unwrap().next().is_none());
    }

    #[test]
    fn remote_entries_must_be_single_posix_names() {
        for name in [b"../x".as_slice(), b"/absolute", b"..", b".", b"a\0b", b""] {
            assert!(remote_file_name(name).is_err());
        }
        assert!(remote_file_name(b"valid\\posix.txt").is_ok());
        assert!(local_file_name(b"valid\\posix.txt").is_err());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::local::{LocalTransport, sftp_server};
    use std::os::unix::ffi::OsStrExt;

    fn tree(root: &std::path::Path) -> Vec<u8> {
        fs::create_dir_all(root.join("sub/深い")).unwrap();
        // 読み出しを何個も先に送る大きさ（32 KiB × 16 を超える）と、端の大きさ
        let big: Vec<u8> = (0..1_234_567u32).map(|i| (i * 7 % 251) as u8).collect();
        fs::write(root.join("big.bin"), &big).unwrap();
        fs::write(root.join("sub/empty"), b"").unwrap();
        fs::write(root.join("sub/深い/メモ.txt"), "こんにちは\n").unwrap();
        fs::write(root.join("exact.bin"), vec![1u8; SFTP_CHUNK as usize]).unwrap();
        std::os::unix::fs::symlink(root.join("sub"), root.join("link-to-dir")).unwrap();
        big
    }

    #[test]
    fn copies_over_sftp_both_ways() {
        if sftp_server().is_none() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("remote-src");
        let big = tree(&src);
        let t = LocalTransport::new();
        let sftp = Arc::new(SftpFs::connect(&t).unwrap());
        let remote =
            |p: &std::path::Path| Loc::Sftp(sftp.clone(), p.as_os_str().as_bytes().to_vec());
        // 量（フォルダを指すリンクは数えない）
        let st = measure(&remote(&src), &mut |_| true).unwrap();
        assert_eq!(st.files, 4);
        assert_eq!(st.skipped, 1);
        assert_eq!(
            st.bytes,
            big.len() as u64 + "こんにちは\n".len() as u64 + u64::from(SFTP_CHUNK)
        );
        // 接続先 → 手元（ダウンロード）
        let dst = tmp.path().join("Downloads").join("remote-src");
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        let st = copy(&remote(&src), &Loc::Local(dst.clone()), &mut |_| true).unwrap();
        assert_eq!((st.files, st.skipped), (4, 1));
        assert_eq!(fs::read(dst.join("big.bin")).unwrap(), big);
        assert_eq!(fs::read(dst.join("sub/empty")).unwrap(), b"");
        assert_eq!(
            fs::read_to_string(dst.join("sub/深い/メモ.txt")).unwrap(),
            "こんにちは\n"
        );
        assert_eq!(
            fs::read(dst.join("exact.bin")).unwrap().len(),
            SFTP_CHUNK as usize
        );
        assert!(!dst.join("link-to-dir").exists());
        // 1 つのファイル
        let one = tmp.path().join("one.bin");
        copy(
            &remote(&src.join("big.bin")),
            &Loc::Local(one.clone()),
            &mut |_| true,
        )
        .unwrap();
        assert_eq!(fs::read(&one).unwrap(), big);
        // 手元 → 接続先（アップロード）
        let up = tmp.path().join("uploaded");
        copy(&Loc::Local(dst.clone()), &remote(&up), &mut |_| true).unwrap();
        assert_eq!(fs::read(up.join("big.bin")).unwrap(), big);
        assert_eq!(
            fs::read_to_string(up.join("sub/深い/メモ.txt")).unwrap(),
            "こんにちは\n"
        );
        // あれば上書きしない
        let e = copy(
            &remote(&src.join("big.bin")),
            &Loc::Local(one.clone()),
            &mut |_| true,
        )
        .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        // 中止したら作りかけを消す
        let stopped = tmp.path().join("stopped");
        let e = copy(&remote(&src), &Loc::Local(stopped.clone()), &mut |s| {
            s.bytes < 100_000
        })
        .unwrap_err();
        assert!(is_cancelled(&e), "{e}");
        assert!(!stopped.exists());
    }

    #[test]
    fn replaces_files_keeping_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("new.csv");
        let big: Vec<u8> = (0..700_000u32).map(|i| (i % 253) as u8).collect();
        fs::write(&src, &big).unwrap();
        // 手元
        let local = tmp.path().join("local.csv");
        fs::write(&local, b"old").unwrap();
        fs::set_permissions(&local, fs::Permissions::from_mode(0o640)).unwrap();
        replace(&src, &Loc::Local(local.clone()), &mut |_| true).unwrap();
        assert_eq!(fs::read(&local).unwrap(), big);
        assert_eq!(
            fs::metadata(&local).unwrap().permissions().mode() & 0o777,
            0o640
        );
        // 一時ファイルは残らない
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 2);
        if sftp_server().is_none() {
            return;
        }
        let t = LocalTransport::new();
        let sftp = Arc::new(SftpFs::connect(&t).unwrap());
        let remote =
            |p: &std::path::Path| Loc::Sftp(sftp.clone(), p.as_os_str().as_bytes().to_vec());
        // 既にあるファイルを置き換える（権限はそのまま）
        let dir = tmp.path().join("remote");
        fs::create_dir(&dir).unwrap();
        let there = dir.join("データ.csv");
        fs::write(&there, b"old contents").unwrap();
        fs::set_permissions(&there, fs::Permissions::from_mode(0o600)).unwrap();
        let before = info(&remote(&there)).unwrap().unwrap();
        replace(&src, &remote(&there), &mut |_| true).unwrap();
        assert_eq!(fs::read(&there).unwrap(), big);
        assert_eq!(
            fs::metadata(&there).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let after = info(&remote(&there)).unwrap().unwrap();
        assert_ne!(before.id, after.id);
        assert_eq!(after.len(), big.len() as u64);
        // ないファイルは作る
        let fresh = dir.join("new.csv");
        assert!(info(&remote(&fresh)).unwrap().is_none());
        replace(&src, &remote(&fresh), &mut |_| true).unwrap();
        assert_eq!(fs::read(&fresh).unwrap(), big);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        // 中止したら元のファイルが残る
        fs::write(&there, b"keep").unwrap();
        let e = replace(&src, &remote(&there), &mut |s| s.bytes < 100_000).unwrap_err();
        assert!(is_cancelled(&e), "{e}");
        assert_eq!(fs::read(&there).unwrap(), b"keep");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
    }

    #[test]
    fn temp_names_sit_next_to_the_file() {
        assert_eq!(
            String::from_utf8(sibling_temp(b"/home/u/a.csv")).unwrap(),
            format!("/home/u/.a.csv.yy-{}.tmp", std::process::id())
        );
        assert_eq!(
            String::from_utf8(sibling_temp(b"a.csv")).unwrap(),
            format!(".a.csv.yy-{}.tmp", std::process::id())
        );
    }
}

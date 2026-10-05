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

use crate::{Session, UploadOutcome};

/// コピー元・コピー先の場所。
#[derive(Clone)]
pub enum Loc {
    Local(PathBuf),
    /// 接続先のセッションと、その中の絶対パス
    Remote(Arc<Session>, Vec<u8>),
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

impl Loc {
    fn join(&self, name: &Name) -> Loc {
        match self {
            Loc::Local(p) => Loc::Local(p.join(match name {
                Name::Os(n) => n.clone(),
                Name::Bytes(b) => OsString::from(String::from_utf8_lossy(b).into_owned()),
            })),
            Loc::Remote(s, p) => {
                let n = match name {
                    Name::Os(n) => n.to_string_lossy().into_owned().into_bytes(),
                    Name::Bytes(b) => b.clone(),
                };
                Loc::Remote(s.clone(), yy_proto::join_path(p, &n))
            }
        }
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
            Loc::Remote(s, p) => {
                let i = s.stat(p)?;
                Ok(remote_kind(i.kind, i.link))
            }
        }
    }

    fn exists(&self) -> io::Result<bool> {
        let r = match self {
            Loc::Local(p) => fs::symlink_metadata(p).map(|_| ()),
            Loc::Remote(s, p) => s.stat(p).map(|_| ()),
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
            Loc::Remote(s, p) => Ok(s
                .read_dir(p)?
                .into_iter()
                .map(|e| {
                    let kind = e
                        .info
                        .as_ref()
                        .map_or(Kind::Skip, |i| remote_kind(i.kind, i.link));
                    (Name::Bytes(e.name), kind)
                })
                .collect()),
        }
    }

    fn make_dir(&self) -> io::Result<()> {
        match self {
            Loc::Local(p) => fs::create_dir(p),
            Loc::Remote(s, p) => s.make_dir(p),
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
            Loc::Remote(s, p) => s.remove(p, true),
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
                copy_node(&from.join(&name), &to.join(&name), kind, t)?;
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
        (Loc::Local(f), Loc::Remote(s, d)) => {
            let mut src = Counting {
                inner: File::open(f)?,
                ticker: t,
            };
            upload(s, &mut src, d)
        }
        (Loc::Remote(s, f), Loc::Local(d)) => {
            let mut dst = BufWriter::with_capacity(1 << 20, create_new(d)?);
            download(s, f, &mut dst, t)?;
            dst.into_inner().map_err(|e| e.into_error())?.sync_all()
        }
        (Loc::Remote(a, f), Loc::Remote(b, d)) => {
            // 別の接続先の間は、手元の一時ファイルを経由する
            let tmp = std::env::temp_dir().join(format!(
                "yyeditor-copy-{}-{}.tmp",
                std::process::id(),
                t.stats.files
            ));
            let r = (|| {
                let mut w = BufWriter::with_capacity(1 << 20, File::create(&tmp)?);
                download(a, f, &mut w, t)?;
                w.flush()?;
                drop(w);
                upload(b, &mut File::open(&tmp)?, d)
            })();
            let _ = fs::remove_file(&tmp);
            r
        }
    }
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

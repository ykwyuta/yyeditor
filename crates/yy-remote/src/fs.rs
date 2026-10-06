//! 接続先のファイル操作の共通の口（一覧・情報・フォルダの作成・名前の変更・削除・照合）。
//!
//! エージェント（[`Session`]）と SFTP（[`SftpFs`]）のどちらでも同じように使える。エディタは
//! 常にエージェント、ターミナル（yyterm）とファイル転送（yysftp）はエージェントを使うかを
//! 選べる（使わなければ SFTP。接続先に何も置かない）。

use std::io;

use crate::Session;
use crate::sftp::{Attrs, Entry, Sftp};
use yy_proto::{DirEntry, FileId, FileInfo, FileKind};

/// 接続先のファイル操作。パスは接続先のバイト列のまま。
pub trait RemoteFs: Send + Sync {
    /// 何で操作しているか（記録・表示用。「エージェント」「SFTP」）
    fn kind(&self) -> &'static str;
    /// ホームフォルダ
    fn home(&self) -> &[u8];
    /// 絶対パスに直す（`~` と `~/`・相対パスはホームから）
    fn real_path(&self, path: &[u8]) -> io::Result<Vec<u8>>;
    /// 情報（シンボリックリンクはリンク先）
    fn stat(&self, path: &[u8]) -> io::Result<FileInfo>;
    /// フォルダの項目（`.`・`..` を除く。項目の情報はリンク先、リンクは `link`）
    fn read_dir(&self, path: &[u8]) -> io::Result<Vec<DirEntry>>;
    /// フォルダを作る（親のフォルダは既にあること）
    fn make_dir(&self, path: &[u8]) -> io::Result<()>;
    /// 名前を変える（`to` が既にあればエラー）
    fn rename(&self, from: &[u8], to: &[u8]) -> io::Result<()>;
    /// 消す（フォルダは `recursive` なら中身ごと）
    fn remove(&self, path: &[u8], recursive: bool) -> io::Result<()>;
    /// 先頭から `len` バイトの SHA-256（接続先で計算できなければ `None`）
    fn hash(&self, _path: &[u8], _len: u64) -> io::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn is_closed(&self) -> bool;

    /// `~` で始まるパス・相対パスをホームからのパスにする。
    fn expand_home(&self, path: &[u8]) -> Vec<u8> {
        if path == b"~" || path.is_empty() {
            self.home().to_vec()
        } else if let Some(rest) = path.strip_prefix(b"~/") {
            yy_proto::join_path(self.home(), rest)
        } else {
            yy_proto::join_path(self.home(), path)
        }
    }
}

impl dyn RemoteFs + '_ {
    /// 情報（なければ `None`）。
    pub fn try_stat(&self, path: &[u8]) -> io::Result<Option<FileInfo>> {
        match self.stat(path) {
            Ok(i) => Ok(Some(i)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// 情報を SFTP の形で（なければ `None`）。
    pub fn try_attrs(&self, path: &[u8]) -> io::Result<Option<Attrs>> {
        Ok(self.try_stat(path)?.map(|i| attrs_of(Some(&i))))
    }

    /// 情報を SFTP の形で。
    pub fn attrs(&self, path: &[u8]) -> io::Result<Attrs> {
        Ok(attrs_of(Some(&self.stat(path)?)))
    }

    /// フォルダの項目を SFTP の形で（シンボリックリンクはリンクとして）。
    pub fn entries(&self, path: &[u8]) -> io::Result<Vec<Entry>> {
        Ok(self
            .read_dir(path)?
            .into_iter()
            .map(|e| Entry {
                attrs: if e.info.as_ref().is_none_or(|i| i.link) {
                    link_attrs(e.info.as_ref())
                } else {
                    attrs_of(e.info.as_ref())
                },
                name: e.name,
            })
            .collect())
    }
}

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

/// エージェントの情報を SFTP の形にする（`None` は読めないリンク）。
pub fn attrs_of(info: Option<&FileInfo>) -> Attrs {
    let Some(i) = info else {
        return link_attrs(None);
    };
    let kind = match i.kind {
        FileKind::Dir => S_IFDIR,
        FileKind::File => S_IFREG,
        FileKind::Other => 0,
    };
    Attrs {
        size: Some(i.len()),
        permissions: Some(kind | (i.mode & 0o7777)),
        mtime: Some(u32::try_from(i.id.mtime_ns.max(0) / 1_000_000_000).unwrap_or(u32::MAX)),
        ..Attrs::default()
    }
}

/// シンボリックリンクの項目（リンク先の大きさ・日時は見せる）。
fn link_attrs(target: Option<&FileInfo>) -> Attrs {
    let mut a = target.map_or_else(Attrs::default, |i| attrs_of(Some(i)));
    a.permissions = Some(S_IFLNK | 0o777);
    a
}

/// SFTP の情報をエージェントの形にする。
fn info_of(a: &Attrs, link: bool) -> FileInfo {
    let p = a.permissions.unwrap_or(0);
    FileInfo {
        kind: match p & 0o170000 {
            S_IFDIR => FileKind::Dir,
            S_IFREG => FileKind::File,
            _ => FileKind::Other,
        },
        link,
        mode: p & 0o7777,
        nlink: 1,
        id: FileId {
            dev: 0,
            ino: 0,
            len: a.size.unwrap_or(0),
            mtime_ns: i64::from(a.mtime.unwrap_or(0)) * 1_000_000_000,
        },
    }
}

impl RemoteFs for Session {
    fn kind(&self) -> &'static str {
        "エージェント"
    }
    fn home(&self) -> &[u8] {
        Session::home(self)
    }
    fn real_path(&self, path: &[u8]) -> io::Result<Vec<u8>> {
        Session::real_path(self, path)
    }
    fn stat(&self, path: &[u8]) -> io::Result<FileInfo> {
        Session::stat(self, path)
    }
    fn read_dir(&self, path: &[u8]) -> io::Result<Vec<DirEntry>> {
        Session::read_dir(self, path)
    }
    fn make_dir(&self, path: &[u8]) -> io::Result<()> {
        Session::make_dir(self, path)
    }
    fn rename(&self, from: &[u8], to: &[u8]) -> io::Result<()> {
        Session::rename(self, from, to)
    }
    fn remove(&self, path: &[u8], recursive: bool) -> io::Result<()> {
        Session::remove(self, path, recursive)
    }
    fn hash(&self, path: &[u8], len: u64) -> io::Result<Option<Vec<u8>>> {
        Session::hash(self, path, len).map(Some)
    }
    fn is_closed(&self) -> bool {
        Session::is_closed(self)
    }
}

/// SFTP でのファイル操作（接続先に何も置かない）。
pub struct SftpFs {
    sftp: Sftp,
    home: Vec<u8>,
}

impl SftpFs {
    /// SFTP を始めてホームを調べる。
    pub fn connect(t: &dyn crate::Transport) -> io::Result<SftpFs> {
        let sftp = Sftp::connect(t)?;
        let home = sftp.realpath(b".")?;
        Ok(SftpFs { sftp, home })
    }

    pub fn sftp(&self) -> &Sftp {
        &self.sftp
    }
}

impl RemoteFs for SftpFs {
    fn kind(&self) -> &'static str {
        "SFTP"
    }
    fn home(&self) -> &[u8] {
        &self.home
    }
    fn real_path(&self, path: &[u8]) -> io::Result<Vec<u8>> {
        self.sftp.realpath(&self.expand_home(path))
    }
    fn stat(&self, path: &[u8]) -> io::Result<FileInfo> {
        let a = self.sftp.stat(path)?;
        let link = self.sftp.lstat(path).is_ok_and(|l| l.is_symlink());
        Ok(info_of(&a, link))
    }
    fn read_dir(&self, path: &[u8]) -> io::Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for e in self.sftp.read_dir(path)? {
            let info = if e.attrs.is_symlink() {
                // リンク先（読めなければ `None`）
                let full = yy_proto::join_path(path, &e.name);
                match self.sftp.stat(&full) {
                    Ok(a) => Some(info_of(&a, true)),
                    Err(_) => None,
                }
            } else {
                Some(info_of(&e.attrs, false))
            };
            out.push(DirEntry { name: e.name, info });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }
    fn make_dir(&self, path: &[u8]) -> io::Result<()> {
        self.sftp.mkdir(path)
    }
    fn rename(&self, from: &[u8], to: &[u8]) -> io::Result<()> {
        if self.sftp.lstat(to).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} は既にあります", crate::display(to)),
            ));
        }
        self.sftp.rename(from, to, false)
    }
    fn remove(&self, path: &[u8], recursive: bool) -> io::Result<()> {
        if recursive {
            return self.sftp.remove_all(path);
        }
        let a = self.sftp.lstat(path)?;
        if a.is_dir() {
            self.sftp.rmdir(path)
        } else {
            self.sftp.remove(path)
        }
    }
    fn is_closed(&self) -> bool {
        self.sftp.is_closed()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::local::{LocalTransport, sftp_server};
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn sftp_fs_works_like_the_agent() {
        if sftp_server().is_none() {
            eprintln!("sftp-server がないため飛ばします");
            return;
        }
        let t = LocalTransport::new();
        let fs: Box<dyn RemoteFs> = Box::new(SftpFs::connect(&t).unwrap());
        let fs: &dyn RemoteFs = fs.as_ref();
        assert_eq!(fs.kind(), "SFTP");
        assert!(!fs.home().is_empty());
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().as_os_str().as_bytes();
        let sub = yy_proto::join_path(d, b"sub");
        fs.make_dir(&sub).unwrap();
        std::fs::write(dir.path().join("b.txt"), b"12345").unwrap();
        std::os::unix::fs::symlink(dir.path().join("sub"), dir.path().join("link")).unwrap();
        std::os::unix::fs::symlink("/nonexistent/x", dir.path().join("broken")).unwrap();
        let entries = fs.read_dir(d).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_slice()).collect();
        assert_eq!(names, [&b"b.txt"[..], b"broken", b"link", b"sub"]);
        assert_eq!(entries[0].info.as_ref().unwrap().len(), 5);
        assert!(entries[1].info.is_none());
        let link = entries[2].info.as_ref().unwrap();
        assert!(link.is_dir() && link.link);
        assert!(entries[3].info.as_ref().unwrap().is_dir());
        // SFTP の形にしたときはリンクはリンク
        let e = fs.entries(d).unwrap();
        assert!(e[2].attrs.is_symlink() && e[3].attrs.is_dir());
        assert_eq!(e[0].attrs.size, Some(5));
        // 名前の変更は上書きしない
        let b = yy_proto::join_path(d, b"b.txt");
        let c = yy_proto::join_path(d, b"c.txt");
        fs.rename(&b, &c).unwrap();
        std::fs::write(dir.path().join("b.txt"), b"x").unwrap();
        let err = fs.rename(&b, &c).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(fs.try_stat(b"/nonexistent/zz").unwrap().is_none());
        assert_eq!(fs.attrs(&c).unwrap().size, Some(5));
        fs.remove(&b, false).unwrap();
        std::fs::write(dir.path().join("sub").join("x"), b"x").unwrap();
        fs.remove(&sub, true).unwrap();
        assert!(!dir.path().join("sub").exists());
        // ~ はホームから
        assert_eq!(fs.real_path(b"~").unwrap(), fs.home());
        assert_eq!(fs.hash(&c, 5).unwrap(), None);
    }
}

//! yyeditor と、SSH 接続先で動くエージェント（`yy-agent`）の間のプロトコル（11 章 8）。
//!
//! エージェントの標準入出力を 1 本の通り道として、端末からの要求とエージェントの応答を
//! フレームに包んでやり取りする。フレームは
//!
//! ```text
//! | 長さ u32 LE | 要求 ID u32 LE | 本体（postcard） |
//! ```
//!
//! で、長さは要求 ID と本体を合わせたバイト数。応答には要求と同じ ID を付けるので、
//! 端末は応答を待たずに次の要求を送れる（読み出しや書き込みを並べて回線の遅延を隠す）。
//!
//! 端末とエージェントは同じ版を組で使う（[`VERSION`] が違えばエージェントを置き直す）ので、
//! 異なる版の間の互換性は持たせない。

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// プロトコルの版。メッセージの形を変えたら増やす。
pub const VERSION: u32 = 3;

/// エージェントが起動直後に出す印。ログインシェルの初期化ファイルが標準出力に文字を出しても、
/// 端末はこれより前を読み捨てて同期する（11 章 6.2）。
pub const MAGIC: &[u8] = b"\0YYAGENT1\n";

/// 1 フレームの大きさの上限（壊れた入力で巨大な領域を確保しない）。
pub const MAX_FRAME: usize = 8 << 20;

/// 読み出し・書き込みを分ける単位。
pub const CHUNK: u32 = 1 << 20;

/// 接続先のファイルの同一性（11 章 6.3）。開いた時点と保存する時点でこれが変わっていれば、
/// 外部で変更されたとみなす。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId {
    pub dev: u64,
    pub ino: u64,
    pub len: u64,
    /// 更新時刻（UNIX 時刻からのナノ秒）
    pub mtime_ns: i64,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    File,
    Dir,
    Other,
}

/// ファイルの情報。シンボリックリンクはリンク先の情報（`link` が `true`）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    pub kind: FileKind,
    pub link: bool,
    /// パーミッション（Unix のモードの下位 12 ビット）
    pub mode: u32,
    /// ハードリンクの数
    pub nlink: u64,
    pub id: FileId,
}

impl FileInfo {
    pub fn len(&self) -> u64 {
        self.id.len
    }

    pub fn is_empty(&self) -> bool {
        self.id.len == 0
    }

    pub fn is_dir(&self) -> bool {
        self.kind == FileKind::Dir
    }
}

/// フォルダの項目。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// 名前（接続先のバイト列のまま。UTF-8 とは限らない）
    pub name: Vec<u8>,
    /// リンク先が読めないシンボリックリンクは `None`
    pub info: Option<FileInfo>,
}

/// 端末からの要求。パスは接続先のバイト列のまま（UTF-8 とは限らない）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// 最初の要求。応答は [`Response::Hello`]
    Hello { version: u32 },
    /// 絶対パスに直す（`..` やリンクを解決する）。応答は [`Response::Path`]
    RealPath { path: Vec<u8> },
    /// 応答は [`Response::Info`]
    Stat { path: Vec<u8> },
    /// 応答は [`Response::Dir`]（名前順）
    ReadDir { path: Vec<u8> },
    /// 読み出し用に開く。応答は [`Response::Opened`]
    Open { path: Vec<u8> },
    /// 開いたファイルの `offset` から最大 `len` バイト。応答は [`Response::Data`]
    Read { handle: u32, offset: u64, len: u32 },
    /// 応答は [`Response::Done`]
    Close { handle: u32 },
    /// `path` へ保存する内容の受け取りを始める（保存先と同じフォルダの一時ファイルに書く）。
    /// 応答は [`Response::Upload`]
    BeginUpload { path: Vec<u8> },
    /// 応答は [`Response::Done`]
    Write { upload: u32, data: Block },
    /// 書き終えた内容で保存先を置き換える（11 章 7.1）。
    ///
    /// `force` でなければ、保存先が `expected` と違う（外部で変更された）とき、または
    /// `expected` が `None`（新しく作るつもり）なのに保存先があるときは置き換えずに
    /// [`Response::Conflict`] を返す。そのあとも受け取った内容は残るので、`force` を付けて
    /// もう一度送れば置き換える。成功すれば [`Response::Committed`]
    Commit {
        upload: u32,
        expected: Option<FileId>,
        force: bool,
    },
    /// 受け取った内容を捨てる。応答は [`Response::Done`]
    Abort { upload: u32 },
    /// フォルダを作る（親のフォルダは既にあること。既にあればエラー）。応答は [`Response::Done`]
    MakeDir { path: Vec<u8> },
    /// 名前を変える・移動する。`to` が既にあればエラー（上書きしない）。別のファイルシステムへの
    /// 移動はコピーしてから元を消す。応答は [`Response::Done`]
    Rename { from: Vec<u8>, to: Vec<u8> },
    /// ファイル・フォルダを消す（ごみ箱はない）。フォルダは `recursive` なら中身ごと、でなければ
    /// 空のときだけ。シンボリックリンクはリンクだけを消す。応答は [`Response::Done`]
    Remove { path: Vec<u8>, recursive: bool },
    /// ファイル・フォルダを中身ごとコピーする（接続先の中で。転送はしない）。`to` が既にあれば
    /// エラー。フォルダをその中へはコピーしない。応答は [`Response::Done`]
    Copy { from: Vec<u8>, to: Vec<u8> },
}

/// エージェントからの応答。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Response {
    Hello {
        version: u32,
        agent_version: String,
        /// `std::env::consts::OS` / `ARCH`
        os: String,
        arch: String,
        home: Vec<u8>,
    },
    Path(Vec<u8>),
    Info(FileInfo),
    Dir(Vec<DirEntry>),
    Opened {
        handle: u32,
        info: FileInfo,
    },
    Data(Block),
    Upload(u32),
    Done,
    /// 置き換えた。保存したファイルの情報
    Committed(FileInfo),
    /// 置き換えなかった。保存先の現在の情報（なければ `None`）
    Conflict(Option<FileInfo>),
    Error(RemoteError),
}

/// エージェントで起きたエラー。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RemoteError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    InvalidInput,
    /// 空き容量の不足
    StorageFull,
    Other,
}

impl From<&io::Error> for RemoteError {
    fn from(e: &io::Error) -> Self {
        let kind = match e.kind() {
            io::ErrorKind::NotFound => ErrorKind::NotFound,
            io::ErrorKind::PermissionDenied => ErrorKind::PermissionDenied,
            io::ErrorKind::AlreadyExists => ErrorKind::AlreadyExists,
            io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => ErrorKind::InvalidInput,
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => ErrorKind::StorageFull,
            _ => ErrorKind::Other,
        };
        RemoteError {
            kind,
            message: e.to_string(),
        }
    }
}

impl From<RemoteError> for io::Error {
    fn from(e: RemoteError) -> Self {
        let kind = match e.kind {
            ErrorKind::NotFound => io::ErrorKind::NotFound,
            ErrorKind::PermissionDenied => io::ErrorKind::PermissionDenied,
            ErrorKind::AlreadyExists => io::ErrorKind::AlreadyExists,
            ErrorKind::InvalidInput => io::ErrorKind::InvalidInput,
            ErrorKind::StorageFull => io::ErrorKind::StorageFull,
            ErrorKind::Other => io::ErrorKind::Other,
        };
        io::Error::new(kind, e.message)
    }
}

/// ファイルの内容の一部。縮むものは deflate で圧縮して送る（11 章 8.1）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// 元の大きさ
    pub len: u32,
    pub compressed: bool,
    pub data: Vec<u8>,
}

impl Block {
    /// 送る形にする。圧縮して 1 割以上縮まなければ圧縮しない。
    pub fn pack(raw: &[u8]) -> Block {
        let len = u32::try_from(raw.len()).expect("block is smaller than 4 GiB");
        if raw.len() >= 512 {
            let z = miniz_oxide::deflate::compress_to_vec(raw, 1);
            if z.len() < raw.len() / 10 * 9 {
                return Block {
                    len,
                    compressed: true,
                    data: z,
                };
            }
        }
        Block {
            len,
            compressed: false,
            data: raw.to_vec(),
        }
    }

    /// 元のバイト列に戻す。
    pub fn unpack(self) -> io::Result<Vec<u8>> {
        if !self.compressed {
            return Ok(self.data);
        }
        let out = miniz_oxide::inflate::decompress_to_vec_with_limit(&self.data, self.len as usize)
            .map_err(|_| invalid("圧縮されたデータが壊れています"))?;
        if out.len() != self.len as usize {
            return Err(invalid("圧縮されたデータの長さが合いません"));
        }
        Ok(out)
    }
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// フレームを 1 つ書く（フラッシュはしない）。
pub fn write_frame<W: Write + ?Sized, T: Serialize>(w: &mut W, id: u32, msg: &T) -> io::Result<()> {
    let body = postcard::to_allocvec(msg).map_err(|e| invalid(&e.to_string()))?;
    let len = body.len() + 4;
    if len > MAX_FRAME {
        return Err(invalid("フレームが大きすぎます"));
    }
    let mut buf = Vec::with_capacity(8 + body.len());
    buf.extend_from_slice(&(len as u32).to_le_bytes());
    buf.extend_from_slice(&id.to_le_bytes());
    buf.extend_from_slice(&body);
    w.write_all(&buf)
}

/// フレームを 1 つ読む。相手が閉じていれば（フレームの境目で EOF なら）`None`。
pub fn read_frame<R: Read + ?Sized, T: DeserializeOwned>(
    r: &mut R,
) -> io::Result<Option<(u32, T)>> {
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < head.len() {
        match r.read(&mut head[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_le_bytes(head) as usize;
    if !(4..=MAX_FRAME).contains(&len) {
        return Err(invalid("フレームの長さが不正です"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let id = u32::from_le_bytes(buf[..4].try_into().expect("4 bytes"));
    let msg = postcard::from_bytes(&buf[4..]).map_err(|e| invalid(&e.to_string()))?;
    Ok(Some((id, msg)))
}

/// `r` を [`MAGIC`] の直後まで読み進める。それより前に読み捨てた内容（ログインシェルの出力など。
/// 最大 64 KiB）を返す。印が現れないまま閉じられたらエラー。
pub fn skip_to_magic<R: Read + ?Sized>(r: &mut R) -> io::Result<Vec<u8>> {
    const LIMIT: usize = 64 << 10;
    let mut seen = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match r.read(&mut byte) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!(
                        "エージェントが起動しませんでした: {}",
                        String::from_utf8_lossy(&seen).trim()
                    ),
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
        seen.push(byte[0]);
        if seen.ends_with(MAGIC) {
            seen.truncate(seen.len() - MAGIC.len());
            return Ok(seen);
        }
        if seen.len() > LIMIT + MAGIC.len() {
            seen.drain(..seen.len() - LIMIT);
        }
    }
}

/// 接続先のパスの表示用の文字列（UTF-8 でないバイトは置き換える）。
pub fn display_path(path: &[u8]) -> String {
    String::from_utf8_lossy(path).into_owned()
}

/// POSIX のパスをつなぐ。`name` が絶対パスならそれを返す。
pub fn join_path(dir: &[u8], name: &[u8]) -> Vec<u8> {
    if name.starts_with(b"/") || dir.is_empty() {
        return name.to_vec();
    }
    let mut p = dir.to_vec();
    if !p.ends_with(b"/") {
        p.push(b'/');
    }
    p.extend_from_slice(name);
    p
}

/// POSIX のパスの親フォルダ（ルートはルートのまま）。
pub fn parent_path(path: &[u8]) -> Vec<u8> {
    let trimmed = match path {
        [rest @ .., b'/'] if !rest.is_empty() => rest,
        p => p,
    };
    match trimmed.iter().rposition(|&b| b == b'/') {
        Some(0) => b"/".to_vec(),
        Some(i) => trimmed[..i].to_vec(),
        None => b".".to_vec(),
    }
}

/// POSIX のパスの最後の要素。
pub fn file_name(path: &[u8]) -> &[u8] {
    let trimmed = match path {
        [rest @ .., b'/'] if !rest.is_empty() => rest,
        p => p,
    };
    match trimmed.iter().rposition(|&b| b == b'/') {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let msgs = [
            Request::Hello { version: VERSION },
            Request::Read {
                handle: 3,
                offset: 1 << 40,
                len: CHUNK,
            },
            Request::Write {
                upload: 1,
                data: Block::pack(&vec![b'a'; 100_000]),
            },
            Request::Commit {
                upload: 1,
                expected: Some(FileId {
                    dev: 1,
                    ino: 2,
                    len: 3,
                    mtime_ns: -4,
                }),
                force: false,
            },
        ];
        let mut buf = Vec::new();
        for (i, m) in msgs.iter().enumerate() {
            write_frame(&mut buf, i as u32, m).unwrap();
        }
        let mut r = &buf[..];
        for (i, m) in msgs.iter().enumerate() {
            let (id, got): (u32, Request) = read_frame(&mut r).unwrap().unwrap();
            assert_eq!(id, i as u32);
            assert_eq!(&got, m);
        }
        assert!(read_frame::<_, Request>(&mut r).unwrap().is_none());
    }

    #[test]
    fn truncated_frame_is_an_error() {
        let mut buf = Vec::new();
        write_frame(&mut buf, 7, &Response::Done).unwrap();
        buf.pop();
        let mut r = &buf[..buf.len()];
        assert!(read_frame::<_, Response>(&mut r).is_err());
        let mut r = &buf[..2];
        assert!(read_frame::<_, Response>(&mut r).is_err());
    }

    #[test]
    fn oversized_length_is_rejected() {
        let mut buf = ((MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
        buf.extend_from_slice(&[0; 16]);
        assert!(read_frame::<_, Response>(&mut &buf[..]).is_err());
    }

    #[test]
    fn blocks_compress_only_when_useful() {
        let text = b"hello world\n".repeat(1000);
        let b = Block::pack(&text);
        assert!(b.compressed && b.data.len() < text.len() / 4);
        assert_eq!(b.unpack().unwrap(), text);

        let mut x = 0x2545_f491_4f6c_dd1du64;
        let noise: Vec<u8> = (0..4096)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 32) as u8
            })
            .collect();
        let b = Block::pack(&noise);
        assert!(!b.compressed);
        assert_eq!(b.unpack().unwrap(), noise);

        let mut broken = Block::pack(&text);
        broken.len += 1;
        assert!(broken.unpack().is_err());
    }

    #[test]
    fn magic_skips_shell_noise() {
        let mut input = b"Welcome!\nlast login: today\n".to_vec();
        input.extend_from_slice(MAGIC);
        input.extend_from_slice(b"rest");
        let mut r = &input[..];
        let noise = skip_to_magic(&mut r).unwrap();
        assert_eq!(noise, b"Welcome!\nlast login: today\n");
        assert_eq!(r, b"rest");

        let mut r = &b"bash: yy-agent: not found\n"[..];
        let e = skip_to_magic(&mut r).unwrap_err();
        assert!(e.to_string().contains("not found"), "{e}");
    }

    #[test]
    fn posix_paths() {
        assert_eq!(join_path(b"/home/a", b"x.txt"), b"/home/a/x.txt");
        assert_eq!(join_path(b"/", b"etc"), b"/etc");
        assert_eq!(join_path(b"/home", b"/etc"), b"/etc");
        assert_eq!(parent_path(b"/home/a/x.txt"), b"/home/a");
        assert_eq!(parent_path(b"/home/"), b"/");
        assert_eq!(parent_path(b"/"), b"/");
        assert_eq!(file_name(b"/home/a/x.txt"), b"x.txt");
        assert_eq!(file_name(b"/home/a/"), b"a");
    }
}

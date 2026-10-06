//! 接続先との 1 つのセッション（エージェント 1 つ）。
//!
//! ファイルの一覧・情報・読み出し（取り寄せ）・保存（送り出し）を提供する。どの操作も
//! 呼び出したスレッドで応答を待つので、UI スレッドからではなくバックグラウンドで呼ぶこと。

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::Arc;

use yy_proto::{Block, DirEntry, FileId, FileInfo, Request, Response, VERSION};

use crate::deploy::{self, AgentFiles, AgentImage};
use crate::rpc::{Client, Reply};
use crate::{ConnectLog, Connector, HostSpec, Prompter, Transport};

/// 応答を待たずに送っておく読み出し・書き込みの数
const WINDOW: usize = 8;

pub struct Session {
    client: Client,
    transport: Arc<dyn Transport>,
    home: Vec<u8>,
    agent_version: String,
}

/// 保存の結果。
pub enum UploadOutcome {
    /// 置き換えた（保存したファイルの情報）
    Saved(FileInfo),
    /// 外部で変更されていた、または新しく作るはずのファイルが既にあった。送った内容は
    /// 接続先に残っているので、[`PendingUpload::force`] で置き換えるか、捨てる（drop）。
    Conflict {
        current: Option<FileInfo>,
        pending: PendingUpload,
    },
}

/// 置き換えずに残した保存内容。drop すると捨てる。
pub struct PendingUpload {
    session: Arc<Session>,
    id: u32,
    expected: Option<FileId>,
    done: bool,
}

impl PendingUpload {
    /// 競合を無視して置き換える。
    pub fn force(mut self) -> io::Result<FileInfo> {
        self.done = true;
        let r = self.session.client.call(&Request::Commit {
            upload: self.id,
            expected: self.expected,
            force: true,
        })?;
        match r {
            Response::Committed(info) => Ok(info),
            other => Err(unexpected(other)),
        }
    }
}

impl Drop for PendingUpload {
    fn drop(&mut self) {
        if !self.done {
            let _ = self
                .session
                .client
                .send(&Request::Abort { upload: self.id });
        }
    }
}

fn unexpected(r: Response) -> io::Error {
    match r {
        Response::Error(e) => e.into(),
        other => io::Error::other(format!("エージェントの応答が不正です: {other:?}")),
    }
}

fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "中止しました")
}

impl Session {
    /// `spec` に接続し、エージェントを配置して起動する。各段階を `log` に記録する。
    pub fn connect(
        connector: &dyn Connector,
        spec: &HostSpec,
        prompter: &dyn Prompter,
        files: &AgentFiles,
        log: &ConnectLog,
    ) -> io::Result<Session> {
        let transport = connector.connect(spec, prompter, log)?;
        Session::start(transport, files, spec.agent_dir.as_deref(), log)
    }

    /// 接続済みの `transport` でエージェントを配置して起動する。
    pub fn start(
        transport: Arc<dyn Transport>,
        files: &AgentFiles,
        agent_dir: Option<&str>,
        log: &ConnectLog,
    ) -> io::Result<Session> {
        let t = transport.as_ref();
        let platform = step(log, "接続先の環境の確認", || deploy::probe(t))?;
        log.note(format!(
            "接続先の環境: {} {}、ホーム {}",
            platform.os,
            platform.arch,
            yy_proto::display_path(&platform.home)
        ));
        let image = step(log, "エージェントの用意", || {
            AgentImage::load(files, &platform.arch)
        })?;
        log.note(format!(
            "エージェント: {}（SHA-256 {}…）",
            image.local.display(),
            &image.sha256[..16]
        ));
        let exe = step(log, "エージェントの配置", || {
            deploy::install(t, &image, &platform, agent_dir, log)
        })?;
        let process = step(log, "エージェントの起動", || {
            deploy::launch(t, &exe)
        })?;
        let (stdin, stdout, finish) = process.into_parts();
        let client = Client::new(stdout, stdin, finish);
        let r = step(log, "エージェントとの通信", || {
            client.call(&Request::Hello { version: VERSION })
        })?;
        let Response::Hello {
            version,
            agent_version,
            home,
            ..
        } = r
        else {
            return Err(unexpected(r));
        };
        log.note(format!(
            "エージェントが起動しました（版 {agent_version}、プロトコル {version}）"
        ));
        if version != VERSION {
            let e = io::Error::other(format!(
                "エージェントのプロトコルの版が違います（{version}。期待する版は {VERSION}）"
            ));
            log.note(format!("失敗: {e}"));
            return Err(e);
        }
        Ok(Session {
            client,
            transport,
            home,
            agent_version,
        })
    }

    /// 接続先のホームフォルダ。
    pub fn home(&self) -> &[u8] {
        &self.home
    }

    pub fn agent_version(&self) -> &str {
        &self.agent_version
    }

    /// 接続（SSH またはエージェント）が切れたか。
    pub fn is_closed(&self) -> bool {
        self.client.is_closed() || self.transport.is_closed()
    }

    /// 絶対パスに直す（`~` と `~/` で始まるパスはホームから）。
    pub fn real_path(&self, path: &[u8]) -> io::Result<Vec<u8>> {
        let path = self.expand_home(path);
        match self.client.call(&Request::RealPath { path })? {
            Response::Path(p) => Ok(p),
            r => Err(unexpected(r)),
        }
    }

    /// `~` で始まるパスをホームからのパスにする。相対パスもホームからとみなす。
    pub fn expand_home(&self, path: &[u8]) -> Vec<u8> {
        if path == b"~" || path.is_empty() {
            self.home.clone()
        } else if let Some(rest) = path.strip_prefix(b"~/") {
            yy_proto::join_path(&self.home, rest)
        } else {
            yy_proto::join_path(&self.home, path)
        }
    }

    pub fn stat(&self, path: &[u8]) -> io::Result<FileInfo> {
        match self.client.call(&Request::Stat {
            path: path.to_vec(),
        })? {
            Response::Info(i) => Ok(i),
            r => Err(unexpected(r)),
        }
    }

    pub fn read_dir(&self, path: &[u8]) -> io::Result<Vec<DirEntry>> {
        match self.client.call(&Request::ReadDir {
            path: path.to_vec(),
        })? {
            Response::Dir(d) => Ok(d),
            r => Err(unexpected(r)),
        }
    }

    /// フォルダを作る（親のフォルダは既にあること）。
    pub fn make_dir(&self, path: &[u8]) -> io::Result<()> {
        check_done(self.client.call(&Request::MakeDir {
            path: path.to_vec(),
        })?)
    }

    /// 名前を変える・移動する（`to` が既にあればエラー。上書きしない）。
    pub fn rename(&self, from: &[u8], to: &[u8]) -> io::Result<()> {
        check_done(self.client.call(&Request::Rename {
            from: from.to_vec(),
            to: to.to_vec(),
        })?)
    }

    /// ファイル・フォルダを消す（フォルダは `recursive` なら中身ごと）。ごみ箱はない。
    pub fn remove(&self, path: &[u8], recursive: bool) -> io::Result<()> {
        check_done(self.client.call(&Request::Remove {
            path: path.to_vec(),
            recursive,
        })?)
    }

    /// 接続先の中でファイル・フォルダを中身ごとコピーする（`to` が既にあればエラー）。
    pub fn copy(&self, from: &[u8], to: &[u8]) -> io::Result<()> {
        check_done(self.client.call(&Request::Copy {
            from: from.to_vec(),
            to: to.to_vec(),
        })?)
    }

    /// 空のファイルを作る（既にあればエラー）。
    pub fn create_file(self: &Arc<Self>, path: &[u8]) -> io::Result<FileInfo> {
        match self.upload(&mut io::empty(), path, None, &mut |_| true)? {
            UploadOutcome::Saved(info) => Ok(info),
            UploadOutcome::Conflict { .. } => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} は既にあります", yy_proto::display_path(path)),
            )),
        }
    }

    /// ファイル全体を `out` に取り寄せる。`progress(済み, 全体)` が `false` を返したら中止する。
    /// 取り寄せている間に外部で変更されたらエラー。開いたときのファイルの情報を返す。
    pub fn download(
        &self,
        path: &[u8],
        out: &mut dyn Write,
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> io::Result<FileInfo> {
        let (handle, info) = match self.client.call(&Request::Open {
            path: path.to_vec(),
        })? {
            Response::Opened { handle, info } => (handle, info),
            r => return Err(unexpected(r)),
        };
        let result = self.read_all(handle, info.len(), out, progress);
        let _ = self.client.send(&Request::Close { handle });
        result?;
        // 読んでいる間に変わっていないか
        let now = self.stat(path)?;
        if now.id != info.id {
            return Err(io::Error::other(
                "読み込み中にファイルが変更されました。もう一度開いてください",
            ));
        }
        Ok(info)
    }

    fn read_all(
        &self,
        handle: u32,
        len: u64,
        out: &mut dyn Write,
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> io::Result<()> {
        let chunk = u64::from(yy_proto::CHUNK);
        let mut next = 0u64;
        let mut done = 0u64;
        let mut inflight: VecDeque<(u64, Reply)> = VecDeque::new();
        if !progress(0, len) {
            return Err(cancelled());
        }
        loop {
            while inflight.len() < WINDOW && next < len {
                let n = chunk.min(len - next) as u32;
                let reply = self.client.send(&Request::Read {
                    handle,
                    offset: next,
                    len: n,
                })?;
                inflight.push_back((next, reply));
                next += u64::from(n);
            }
            let Some((offset, reply)) = inflight.pop_front() else {
                break;
            };
            let data = match reply.wait()? {
                Response::Data(b) => b.unpack()?,
                r => return Err(unexpected(r)),
            };
            let expected = chunk.min(len - offset) as usize;
            if data.len() != expected {
                return Err(io::Error::other(
                    "読み込み中にファイルが変更されました。もう一度開いてください",
                ));
            }
            out.write_all(&data)?;
            done += data.len() as u64;
            if !progress(done, len) {
                return Err(cancelled());
            }
        }
        Ok(())
    }

    /// `input` の内容で `path` を置き換える（11 章 7.1）。`expected` は開いたときのファイルの
    /// 情報（新しく作る場合は `None`）。`progress(送った量)` が `false` を返したら中止する。
    pub fn upload(
        self: &Arc<Self>,
        input: &mut dyn Read,
        path: &[u8],
        expected: Option<FileId>,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> io::Result<UploadOutcome> {
        let id = match self.client.call(&Request::BeginUpload {
            path: path.to_vec(),
        })? {
            Response::Upload(id) => id,
            r => return Err(unexpected(r)),
        };
        // 途中で失敗したら捨てる
        let mut pending = PendingUpload {
            session: self.clone(),
            id,
            expected,
            done: false,
        };
        let mut inflight: VecDeque<Reply> = VecDeque::new();
        let mut buf = vec![0u8; yy_proto::CHUNK as usize];
        let mut sent = 0u64;
        loop {
            let n = read_full(input, &mut buf)?;
            if n > 0 {
                if inflight.len() >= WINDOW {
                    check_done(inflight.pop_front().expect("non-empty").wait()?)?;
                }
                inflight.push_back(self.client.send(&Request::Write {
                    upload: id,
                    data: Block::pack(&buf[..n]),
                })?);
                sent += n as u64;
                if !progress(sent) {
                    return Err(cancelled());
                }
            }
            if n < buf.len() {
                break;
            }
        }
        for r in inflight {
            check_done(r.wait()?)?;
        }
        match self.client.call(&Request::Commit {
            upload: id,
            expected,
            force: false,
        })? {
            Response::Committed(info) => {
                pending.done = true;
                Ok(UploadOutcome::Saved(info))
            }
            Response::Conflict(current) => Ok(UploadOutcome::Conflict { current, pending }),
            r => {
                pending.done = true;
                Err(unexpected(r))
            }
        }
    }
}

/// `f` を行い、失敗したら何をしていたかと一緒に記録する。
fn step<T>(log: &ConnectLog, what: &str, f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    f().inspect_err(|e| log.note(format!("{what}に失敗しました: {e}")))
}

fn check_done(r: Response) -> io::Result<()> {
    match r {
        Response::Done => Ok(()),
        r => Err(unexpected(r)),
    }
}

/// `buf` が埋まるか EOF まで読む。
fn read_full(r: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

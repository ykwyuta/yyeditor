//! SSH 接続先のファイルの編集（11 章）。
//!
//! 接続先に置いたエージェント（`yy-agent`）と [`yy_proto`] で話し、ファイルの一覧・読み出し・
//! 保存を行う。SSH の実装そのもの（暗号処理）はこのクレートに含めず、[`Connector`] と
//! [`Transport`] の trait 越しに使う（実装は `yy-ssh`）。このクレートは OS や非同期実行に
//! 依存しないので、UI からも、SSH を使わないテスト（[`local`]）からも使える。

pub mod deploy;
pub mod known_hosts;
#[cfg(unix)]
pub mod local;
pub mod rpc;
pub mod session;
pub mod ssh_config;
pub mod uri;

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use deploy::AgentFiles;
pub use session::{Session, UploadOutcome};
pub use ssh_config::HostSpec;
pub use uri::RemoteUri;
pub use yy_proto::{DirEntry, FileId, FileInfo, FileKind};

/// SSH 接続（またはテスト用の代わり）。コマンドを起動して標準入出力でやり取りできる。
pub trait Transport: Send + Sync {
    /// 接続先のログインシェルでコマンドを起動する（コマンドはバイト列。パスが UTF-8 とは限らない）。
    fn exec(&self, command: &[u8]) -> io::Result<Process>;
    /// 接続が切れているか。
    fn is_closed(&self) -> bool;
}

/// 起動したコマンド。
pub struct Process {
    /// 閉じる（drop する）と相手には EOF が届く
    pub stdin: Box<dyn Write + Send>,
    pub stdout: Box<dyn Read + Send>,
    finish: Box<dyn FnOnce() -> io::Result<Exit> + Send>,
}

/// 終わったコマンドの結果。
#[derive(Debug, Default)]
pub struct Exit {
    /// 終了コード（シグナルで終わった場合などは `None`）
    pub status: Option<u32>,
    /// 標準エラー出力（先頭の最大 64 KiB）
    pub stderr: Vec<u8>,
}

/// 標準エラー出力を保持する上限
pub const STDERR_LIMIT: usize = 64 << 10;

impl Process {
    pub fn new(
        stdin: Box<dyn Write + Send>,
        stdout: Box<dyn Read + Send>,
        finish: Box<dyn FnOnce() -> io::Result<Exit> + Send>,
    ) -> Process {
        Process {
            stdin,
            stdout,
            finish,
        }
    }

    /// 入出力を分ける。`finish` はコマンドが終わるまで待って結果を返す。
    #[allow(clippy::type_complexity)]
    pub fn into_parts(
        self,
    ) -> (
        Box<dyn Write + Send>,
        Box<dyn Read + Send>,
        Box<dyn FnOnce() -> io::Result<Exit> + Send>,
    ) {
        (self.stdin, self.stdout, self.finish)
    }
}

/// 終わったコマンドの出力。
#[derive(Debug, Default)]
pub struct Output {
    pub status: Option<u32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }

    /// 標準エラー出力（なければ標準出力）を説明に使う形にする。
    pub fn message(&self) -> String {
        let text = if self.stderr.trim_ascii().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        String::from_utf8_lossy(text.trim_ascii()).into_owned()
    }
}

/// コマンドを実行し、`input` を標準入力に渡して終わるまで待つ。
pub fn run(t: &dyn Transport, command: &[u8], input: &[u8]) -> io::Result<Output> {
    let (mut stdin, mut stdout, finish) = t.exec(command)?.into_parts();
    // 書き込みと読み出しを並べる（出力が詰まって入力を受け取らなくなるのを防ぐ）
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).map(|_| out)
    });
    let written = stdin.write_all(input).and_then(|()| stdin.flush());
    drop(stdin);
    let stdout = reader
        .join()
        .map_err(|_| io::Error::other("出力の読み込みに失敗しました"))??;
    let exit = finish()?;
    // 入力を書き切れなかった場合（コマンドが先に終わった）は、終了コードで判断する
    if let Err(e) = written
        && exit.status == Some(0)
    {
        return Err(e);
    }
    Ok(Output {
        status: exit.status,
        stdout,
        stderr: exit.stderr,
    })
}

/// [`Connector`] を作るときの設定。
#[derive(Clone, Debug)]
pub struct ConnectorOptions {
    /// yyeditor 自身のホスト鍵の記録（承認した鍵を書き込む）
    pub known_hosts: PathBuf,
    /// 読むだけのホスト鍵の記録（`~/.ssh/known_hosts` など）
    pub extra_known_hosts: Vec<PathBuf>,
    /// 死活確認の間隔
    pub keepalive: std::time::Duration,
}

/// 設定から [`Connector`] を作る関数（UI は SSH の実装を知らずに、起動時に受け取る）。
pub type ConnectorFactory = Arc<dyn Fn(&ConnectorOptions) -> Arc<dyn Connector> + Send + Sync>;

/// SSH の接続を作るもの（実装は `yy-ssh`）。
pub trait Connector: Send + Sync {
    /// `spec` に接続して認証する。ホスト鍵の確認や、パスワードなどの入力は `prompter` に尋ねる。
    fn connect(&self, spec: &HostSpec, prompter: &dyn Prompter) -> io::Result<Arc<dyn Transport>>;
}

/// ホスト鍵の確認の種類。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKeyCheck {
    /// 初めて接続するホスト（承認すれば記録する）
    Unknown,
    /// 記録と違う鍵（中間者攻撃のおそれ。接続しない）
    Changed { file: PathBuf, line: usize },
}

/// ホスト鍵の確認の内容。
#[derive(Clone, Debug)]
pub struct HostKeyQuestion {
    pub host: String,
    pub port: u16,
    /// 鍵の種類（`ssh-ed25519` など）
    pub algorithm: String,
    /// `SHA256:` で始まる指紋
    pub fingerprint: String,
    pub check: HostKeyCheck,
}

/// 接続中に利用者へ尋ねること。UI が実装する（呼ばれるのは接続を行うスレッド）。
pub trait Prompter: Send + Sync {
    /// 初めてのホストの鍵を承認するか。`Changed` では警告を出して `false` を返すこと。
    fn confirm_host_key(&self, q: &HostKeyQuestion) -> bool;
    /// パスワード。`None` なら中止
    fn password(&self, user_host: &str) -> Option<String>;
    /// 秘密鍵のパスフレーズ。`None` ならこの鍵を使わない
    fn passphrase(&self, key: &Path) -> Option<String>;
    /// keyboard-interactive 認証の質問（`(質問, 入力を表示するか)` の並び）への答え。
    /// `None` なら中止
    fn keyboard_interactive(
        &self,
        user_host: &str,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>>;
}

/// 利用者に尋ねずに断る [`Prompter`]（テストや自動処理用）。
pub struct NoPrompt;

impl Prompter for NoPrompt {
    fn confirm_host_key(&self, _: &HostKeyQuestion) -> bool {
        false
    }
    fn password(&self, _: &str) -> Option<String> {
        None
    }
    fn passphrase(&self, _: &Path) -> Option<String> {
        None
    }
    fn keyboard_interactive(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &[(String, bool)],
    ) -> Option<Vec<String>> {
        None
    }
}

/// シェルの引数として安全に渡せるよう単一引用符で囲む。
pub fn shell_quote(arg: &[u8]) -> Vec<u8> {
    let mut out = vec![b'\''];
    for &b in arg {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(shell_quote(b"/home/a b"), b"'/home/a b'");
        assert_eq!(shell_quote(b"it's"), b"'it'\\''s'");
    }
}

//! git コマンドを動かすところ（手元・SSH の接続先）。

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use yy_remote::{RemoteUri, Transport};

use crate::auth::{ASKPASS_ENV, ASKPASS_SCRIPT, Credentials, SECRET_ENV, USER_ENV, auth_args};
use crate::{Found, GitError, Limits, Output, Result};

/// 1 つのリポジトリで git を動かすもの。
pub trait Runner: Send + Sync {
    /// 作業ツリーの起点で `git <args>` を動かす（`input` は標準入力）。0 以外で終われば [`GitError`]。
    fn git(&self, args: &[String], input: Option<&[u8]>) -> Result<Output>;
    /// 資格情報を渡して git を動かす（askpass で答える。設定済みの資格情報ヘルパーは外す。[`crate::auth`]）。
    fn git_auth(&self, args: &[String], input: Option<&[u8]>, cred: &Credentials)
    -> Result<Output>;
    /// 作業ツリーのファイル（起点からの相対パス）。なければ `Ok(None)`。
    fn read(&self, rel: &str) -> Result<Option<Vec<u8>>>;
    /// 場所の説明（手元のフォルダ・`ssh://…`）。
    fn location(&self) -> String;
}

/// 実行したコマンドの表示（`git status …`）。
fn shown(args: &[String]) -> String {
    format!("git {}", args.join(" "))
}

/// 失敗の説明（標準エラー出力、なければ標準出力、それもなければ終了コード）。
fn failure(stderr: &str, stdout: &[u8], code: Option<i64>) -> String {
    let mut message = stderr.trim().to_string();
    if message.is_empty() {
        message = String::from_utf8_lossy(stdout).trim().to_string();
    }
    if message.is_empty() {
        message = format!(
            "終了コード {}",
            code.map_or("（不明）".to_string(), |c| c.to_string())
        );
    }
    message
}

// ---- 手元 -------------------------------------------------------------------------------------

/// 手元の git（`git -C 起点 …`）。
#[derive(Clone, Debug)]
pub struct LocalRunner {
    root: PathBuf,
    program: PathBuf,
    /// 資格情報を答える askpass（既定は自分の実行ファイル。[`crate::askpass_main`]）
    askpass: Option<PathBuf>,
}

impl LocalRunner {
    /// git は `YYEDITOR_GIT`、なければ PATH の `git`。
    pub fn new(root: &Path) -> LocalRunner {
        LocalRunner::with_program(
            root,
            &std::env::var_os("YYEDITOR_GIT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("git")),
        )
    }

    pub fn with_program(root: &Path, program: &Path) -> LocalRunner {
        LocalRunner {
            root: root.to_path_buf(),
            program: program.to_path_buf(),
            askpass: std::env::current_exe().ok(),
        }
    }

    /// askpass のプログラムを変える（試験用）。
    pub fn with_askpass(mut self, askpass: &Path) -> LocalRunner {
        self.askpass = Some(askpass.to_path_buf());
        self
    }

    fn run(
        &self,
        args: &[String],
        input: Option<&[u8]>,
        cred: Option<&Credentials>,
    ) -> Result<Output> {
        let shown = shown(args);
        let mut c = Command::new(&self.program);
        c.arg("-C")
            .arg(&self.root)
            .args(["-c", "core.quotepath=off", "-c", "color.ui=false"]);
        if let Some(cred) = cred {
            let Some(askpass) = &self.askpass else {
                return Err(GitError {
                    command: shown,
                    message: "資格情報を渡すプログラム（askpass）がありません".into(),
                });
            };
            c.args(auth_args())
                .env("GIT_ASKPASS", askpass)
                .env("SSH_ASKPASS", askpass)
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env(ASKPASS_ENV, "1")
                .env(USER_ENV, &cred.user)
                .env(SECRET_ENV, &cred.secret);
            if std::env::var_os("DISPLAY").is_none() {
                c.env("DISPLAY", ":0");
            }
        }
        c.args(args)
            // パスワードを端末で尋ねない（固まらない）。資格情報マネージャーの画面は出てよい
            .env("GIT_TERMINAL_PROMPT", "0")
            // エディタを開かない（コミットのメッセージは渡す）
            .env("GIT_EDITOR", "true")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // コンソールの窓を出さない
            c.creation_flags(0x0800_0000);
        }
        let mut child = c.spawn().map_err(|e| GitError {
            command: shown.clone(),
            message: if e.kind() == std::io::ErrorKind::NotFound {
                "git が見つかりません。Git for Windows をインストールし、PATH に git.exe のフォルダを入れてください".into()
            } else {
                format!("git を起動できません: {e}")
            },
        })?;
        if let Some(input) = input
            && let Some(mut stdin) = child.stdin.take()
        {
            let _ = stdin.write_all(input);
        }
        let out = child.wait_with_output().map_err(|e| GitError {
            command: shown.clone(),
            message: e.to_string(),
        })?;
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        if !out.status.success() {
            return Err(GitError {
                command: shown,
                message: failure(&stderr, &out.stdout, out.status.code().map(i64::from)),
            });
        }
        Ok(Output {
            stdout: out.stdout,
            stderr,
        })
    }
}

impl Runner for LocalRunner {
    fn git(&self, args: &[String], input: Option<&[u8]>) -> Result<Output> {
        self.run(args, input, None)
    }

    fn git_auth(
        &self,
        args: &[String],
        input: Option<&[u8]>,
        cred: &Credentials,
    ) -> Result<Output> {
        self.run(args, input, Some(cred))
    }

    fn read(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        let path = self.root.join(rel);
        match std::fs::read(&path) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(GitError {
                command: format!("{} を読む", path.display()),
                message: e.to_string(),
            }),
        }
    }

    fn location(&self) -> String {
        self.root.display().to_string()
    }
}

// ---- SSH の接続先 -------------------------------------------------------------------------------

/// シェル（sh・bash・zsh・fish）の 1 つの語にする（`'…'`。中の `'` は `'"'"'`）。
pub fn shell_quote(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 2);
    out.push(b'\'');
    for &b in s {
        if b == b'\'' {
            out.extend_from_slice(b"'\"'\"'");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

/// 接続先の git（SSH の exec で `env … git -C 起点 …`）。
pub struct RemoteRunner {
    transport: Arc<dyn Transport>,
    root: Vec<u8>,
}

impl RemoteRunner {
    pub fn new(transport: Arc<dyn Transport>, root: &[u8]) -> RemoteRunner {
        RemoteRunner {
            transport,
            root: root.to_vec(),
        }
    }

    fn exec(&self, command: &[u8], input: &[u8], shown: &str) -> Result<yy_remote::Output> {
        yy_remote::run(self.transport.as_ref(), command, input).map_err(|e| GitError {
            command: shown.to_string(),
            message: format!("接続先でコマンドを動かせません: {e}"),
        })
    }
}

/// 接続先で git を動かすコマンド（バイト列）。
fn remote_git_command(root: &[u8], args: &[String]) -> Vec<u8> {
    let mut cmd: Vec<u8> =
        b"env GIT_TERMINAL_PROMPT=0 GIT_EDITOR=true GIT_MERGE_AUTOEDIT=no git -C ".to_vec();
    cmd.extend(shell_quote(root));
    cmd.extend_from_slice(b" -c core.quotepath=off -c color.ui=false");
    for a in args {
        cmd.push(b' ');
        cmd.extend(shell_quote(a.as_bytes()));
    }
    cmd
}

/// 接続先で、資格情報を askpass で渡して git を動かすコマンド（`sh -c '…'`）。標準入力の 1 行目が
/// ユーザー名、2 行目がパスワードなどで、残りを git に渡す。askpass のスクリプトは一時フォルダに作って
/// 終わったら消す。
fn remote_auth_command(root: &[u8], args: &[String]) -> Vec<u8> {
    let mut git: Vec<u8> =
        b"GIT_ASKPASS=\"$d/askpass\" SSH_ASKPASS=\"$d/askpass\" SSH_ASKPASS_REQUIRE=force \
DISPLAY=\"${DISPLAY:-:0}\" GIT_TERMINAL_PROMPT=0 GIT_EDITOR=true GIT_MERGE_AUTOEDIT=no git -C "
            .to_vec();
    git.extend(shell_quote(root));
    git.extend_from_slice(b" -c core.quotepath=off -c color.ui=false -c credential.helper=");
    for a in args {
        git.push(b' ');
        git.extend(shell_quote(a.as_bytes()));
    }
    let mut script: Vec<u8> = b"d=$(mktemp -d 2>/dev/null || mktemp -d -t yygit) || exit 125\n\
trap 'rm -rf \"$d\"' EXIT\n\
umask 077\n\
printf '%s' "
        .to_vec();
    script.extend(shell_quote(ASKPASS_SCRIPT.as_bytes()));
    script.extend_from_slice(
        b" > \"$d/askpass\" && chmod 700 \"$d/askpass\" || exit 125\n\
IFS= read -r YYGIT_USER\n\
IFS= read -r YYGIT_SECRET\n\
export YYGIT_USER YYGIT_SECRET\n",
    );
    script.extend(git);
    script.push(b'\n');
    let mut cmd = b"sh -c ".to_vec();
    cmd.extend(shell_quote(&script));
    cmd
}

impl RemoteRunner {
    fn finish(&self, shown: String, out: yy_remote::Output) -> Result<Output> {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        if !out.success() {
            let message = if out.status == Some(127) {
                format!(
                    "接続先に git がありません（git をインストールし、ログインシェルの PATH に入れてください）\n{}",
                    stderr.trim()
                )
            } else {
                failure(&stderr, &out.stdout, out.status.map(i64::from))
            };
            return Err(GitError {
                command: shown,
                message,
            });
        }
        Ok(Output {
            stdout: out.stdout,
            stderr,
        })
    }
}

impl Runner for RemoteRunner {
    fn git_auth(
        &self,
        args: &[String],
        input: Option<&[u8]>,
        cred: &Credentials,
    ) -> Result<Output> {
        let shown = shown(args);
        // 改行は渡せない（1 行ずつ読む）
        let line = |s: &str| s.replace(['\r', '\n'], "");
        let mut stdin = format!("{}\n{}\n", line(&cred.user), line(&cred.secret)).into_bytes();
        stdin.extend_from_slice(input.unwrap_or(b""));
        let out = self.exec(&remote_auth_command(&self.root, args), &stdin, &shown)?;
        self.finish(shown, out)
    }

    fn git(&self, args: &[String], input: Option<&[u8]>) -> Result<Output> {
        let shown = shown(args);
        let out = self.exec(
            &remote_git_command(&self.root, args),
            input.unwrap_or(b""),
            &shown,
        )?;
        self.finish(shown, out)
    }

    fn read(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        let path = yy_proto::join_path(&self.root, rel.as_bytes());
        let mut cmd = b"test -f ".to_vec();
        cmd.extend(shell_quote(&path));
        cmd.extend_from_slice(b" && cat -- ");
        cmd.extend(shell_quote(&path));
        let shown = format!("cat {}", String::from_utf8_lossy(&path));
        let out = self.exec(&cmd, b"", &shown)?;
        Ok(out.success().then_some(out.stdout))
    }

    fn location(&self) -> String {
        String::from_utf8_lossy(&self.root).into_owned()
    }
}

/// 接続先のフォルダ `folders`（同じ接続先の `ssh://…`）の下の Git のリポジトリ。フォルダを含む上の
/// フォルダがリポジトリならそれも入れる。探し方は手元と同じ（深さ・`node_modules` などを見ない・
/// シンボリック リンクをたどらない）で、接続先の `find` で 1 回に探す。`Found::root` は `ssh://…`。
pub fn discover_remote(
    transport: &Arc<dyn Transport>,
    folders: &[RemoteUri],
    limits: Limits,
) -> Vec<Found> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for folder in folders {
        let base = &folder.path;
        let base_name = {
            let trimmed: &[u8] = match base.as_slice() {
                [rest @ .., b'/'] if !rest.is_empty() => rest,
                b => b,
            };
            let name = trimmed.rsplit(|&b| b == b'/').next().unwrap_or(trimmed);
            let name = String::from_utf8_lossy(name).into_owned();
            if name.is_empty() {
                "/".to_string()
            } else {
                name
            }
        };
        let target = folder.target().to_string();
        let uri = |path: &[u8]| {
            PathBuf::from(
                RemoteUri {
                    path: path.to_vec(),
                    ..folder.clone()
                }
                .to_string(),
            )
        };
        // find R -maxdepth N ( -name node_modules -o … ) -prune -o -name .git -prune
        //   ( -type d -exec test -f {}/HEAD ; -exec printf 'D%s\n' {} ; -o -type f -exec printf 'L%s\n' {} ; )
        let mut cmd = b"find ".to_vec();
        cmd.extend(shell_quote(base));
        cmd.extend_from_slice(format!(" -maxdepth {} '('", limits.depth + 1).as_bytes());
        let skip: Vec<&str> = crate::SKIP_DIRS
            .iter()
            .copied()
            .filter(|s| *s != ".git")
            .collect();
        for (i, s) in skip.iter().enumerate() {
            if i > 0 {
                cmd.extend_from_slice(b" -o");
            }
            cmd.extend_from_slice(b" -name ");
            cmd.extend(shell_quote(s.as_bytes()));
        }
        cmd.extend_from_slice(
            b" ')' -prune -o -name .git -prune '(' -type d -exec test -f '{}/HEAD' ';' \
              -exec printf 'D%s\\n' '{}' ';' -o -type f -exec printf 'L%s\\n' '{}' ';' ')' 2>/dev/null",
        );
        let found = yy_remote::run(transport.as_ref(), &cmd, b"")
            .map(|o| o.stdout)
            .unwrap_or_default();
        let mut here = false;
        for line in found.split(|&b| b == b'\n') {
            let (linked, path) = match line.split_first() {
                Some((b'D', p)) => (false, p),
                Some((b'L', p)) => (true, p),
                _ => continue,
            };
            let Some(root) = path.strip_suffix(b"/.git") else {
                continue;
            };
            let root = if root.is_empty() { b"/" as &[u8] } else { root };
            if !seen.insert((target.clone(), root.to_vec())) {
                continue;
            }
            let rel = root
                .strip_prefix(base.as_slice())
                .map(|r| r.strip_prefix(b"/").unwrap_or(r))
                .filter(|r| !r.is_empty());
            if rel.is_none() {
                here = true;
            }
            out.push(Found {
                label: match rel {
                    Some(r) => format!("{base_name}/{} [{target}]", String::from_utf8_lossy(r)),
                    None => format!("{base_name} [{target}]"),
                },
                root: uri(root),
                linked,
            });
        }
        // 上のフォルダ（フォルダを含むリポジトリ）
        if !here {
            let r = RemoteRunner::new(transport.clone(), base);
            if let Ok(o) = r.git(&["rev-parse".into(), "--show-toplevel".into()], None) {
                let top = o.stdout.trim_ascii_end().to_vec();
                if !top.is_empty() && seen.insert((target.clone(), top.clone())) {
                    let name =
                        String::from_utf8_lossy(top.rsplit(|&b| b == b'/').next().unwrap_or(&top))
                            .into_owned();
                    out.push(Found {
                        label: format!("{name}（{base_name} を含む）[{target}]"),
                        root: uri(&top),
                        linked: false,
                    });
                }
            }
        }
    }
    out.sort_by_key(|f| f.label.to_lowercase());
    out
}

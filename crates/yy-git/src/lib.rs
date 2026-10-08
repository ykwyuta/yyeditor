//! Git の連携（17 章）。
//!
//! - [`discover`]: ワークスペースのフォルダの下（と、フォルダを含む上のフォルダ）から `.git` を探し、
//!   リポジトリ（作業ツリーの起点）の一覧を作る。どれを表示するかは使う人が選ぶ。
//! - [`Git`]: 1 つのリポジトリで `git` コマンドを動かす（状態・ステージ・コミット・ブランチ・プル・
//!   プッシュ・ファイルの版の取り出し）。Git の実装は持たず、入っている `git`（Git for Windows など）を
//!   使うので、資格情報・フック・設定は `git` と同じになる。

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub mod auth;
mod runner;
mod status;
pub use auth::{AuthKind, Credentials, RemoteInfo, askpass_main, auth_failure, remote_info};
pub use runner::{LocalRunner, RemoteRunner, Runner, discover_remote, shell_quote};
pub use status::{Change, Group, Status, parse_status};

// ---- リポジトリを探す ---------------------------------------------------------------------------

/// 下のフォルダをたどらない名前。
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    ".svn",
    ".hg",
    "$RECYCLE.BIN",
    "System Volume Information",
];

/// 探す範囲の上限。
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// 起点のフォルダからの深さ
    pub depth: usize,
    /// 見るフォルダの数の合計
    pub dirs: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            depth: 8,
            dirs: 50_000,
        }
    }
}

/// 見つけたリポジトリ。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Found {
    /// 作業ツリーの起点（`.git` のあるフォルダ）
    pub root: PathBuf,
    /// 一覧に出す名前（ワークスペースのフォルダの名前からの相対パス。例: `app/libs/core`）
    pub label: String,
    /// `.git` がファイル（サブモジュール・`git worktree` で作った作業ツリー）
    pub linked: bool,
}

/// `dir` に `.git`（フォルダかファイル）があるか。ファイルなら `Some(true)`。
fn git_entry(dir: &Path) -> Option<bool> {
    let m = std::fs::symlink_metadata(dir.join(".git")).ok()?;
    if m.is_dir() {
        // 中身のない .git フォルダは数えない
        dir.join(".git").join("HEAD").exists().then_some(false)
    } else {
        Some(true)
    }
}

/// ワークスペースのフォルダ `roots` の下にある Git のリポジトリの一覧（名前の順）。フォルダを含む上の
/// フォルダがリポジトリならそれも入れる（ワークスペースのフォルダがリポジトリの中のフォルダのとき）。
/// `ssh://` などのリモートのフォルダは見ない。
pub fn discover(roots: &[PathBuf], limits: Limits) -> Vec<Found> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut budget = limits.dirs;
    for root in roots {
        if root.to_string_lossy().contains("://") || !root.is_dir() {
            continue;
        }
        let base_name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());
        // 上のフォルダ
        if git_entry(root).is_none() {
            let mut up = root.parent();
            while let Some(dir) = up {
                if let Some(linked) = git_entry(dir) {
                    if seen.insert(dir.to_path_buf()) {
                        out.push(Found {
                            root: dir.to_path_buf(),
                            label: format!(
                                "{}（{base_name} を含む）",
                                dir.file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| dir.to_string_lossy().into_owned())
                            ),
                            linked,
                        });
                    }
                    break;
                }
                up = dir.parent();
            }
        }
        // 下のフォルダ（幅優先）
        let mut queue = std::collections::VecDeque::from([(root.clone(), 0usize)]);
        while let Some((dir, depth)) = queue.pop_front() {
            if budget == 0 {
                break;
            }
            budget -= 1;
            if let Some(linked) = git_entry(&dir)
                && seen.insert(dir.clone())
            {
                let rel = dir
                    .strip_prefix(root)
                    .ok()
                    .filter(|r| !r.as_os_str().is_empty())
                    .map(|r| {
                        r.components()
                            .map(|c| c.as_os_str().to_string_lossy())
                            .collect::<Vec<_>>()
                            .join("/")
                    });
                out.push(Found {
                    label: match rel {
                        Some(r) => format!("{base_name}/{r}"),
                        None => base_name.clone(),
                    },
                    root: dir.clone(),
                    linked,
                });
            }
            if depth >= limits.depth {
                continue;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut subs: Vec<PathBuf> = rd
                .flatten()
                .filter(|e| {
                    // シンボリック リンク・ジャンクションはたどらない（循環を防ぐ）
                    e.file_type().is_ok_and(|t| t.is_dir())
                        && !SKIP_DIRS.iter().any(|s| e.file_name() == OsStr::new(s))
                })
                .map(|e| e.path())
                .collect();
            subs.sort();
            queue.extend(subs.into_iter().map(|p| (p, depth + 1)));
        }
    }
    out.sort_by_key(|a| a.label.to_lowercase());
    out
}

// ---- git コマンド -------------------------------------------------------------------------------

/// git コマンドの失敗。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitError {
    /// 実行したコマンド（`git commit …`）
    pub command: String,
    /// 理由（git のエラーの出力など）
    pub message: String,
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n{}", self.command, self.message.trim_end())
    }
}

impl std::error::Error for GitError {}

pub type Result<T> = std::result::Result<T, GitError>;

/// git コマンドの結果。
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// ブランチ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// `main`・`origin/main`
    pub name: String,
    /// 今のブランチ
    pub current: bool,
    /// リモート追跡ブランチ（`refs/remotes`）
    pub remote: bool,
    /// 上流（`origin/main`。なければ空）
    pub upstream: String,
}

/// 1 つのリポジトリの git コマンド（手元か、SSH の接続先。[`Runner`]）。
#[derive(Clone)]
pub struct Git {
    runner: Arc<dyn Runner>,
}

impl fmt::Debug for Git {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Git({})", self.runner.location())
    }
}

impl Git {
    /// 手元のリポジトリ（作業ツリーの起点 `root`）。
    pub fn new(root: &Path) -> Git {
        Git {
            runner: Arc::new(LocalRunner::new(root)),
        }
    }

    /// SSH の接続先のリポジトリ（作業ツリーの起点は接続先の絶対パス `root`）。接続先の git を動かす。
    pub fn remote(transport: Arc<dyn yy_remote::Transport>, root: &[u8]) -> Git {
        Git {
            runner: Arc::new(RemoteRunner::new(transport, root)),
        }
    }

    pub fn with_runner(runner: Arc<dyn Runner>) -> Git {
        Git { runner }
    }

    /// 場所の説明（手元のフォルダ・`ssh://…`）。
    pub fn location(&self) -> String {
        self.runner.location()
    }

    /// git コマンドを動かす（`input` は標準入力に渡す）。0 以外で終われば [`GitError`]。
    pub fn run_with<I, S>(&self, args: I, input: Option<&[u8]>) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let args: Vec<String> = args.into_iter().map(|a| a.as_ref().to_string()).collect();
        self.runner.git(&args, input)
    }

    pub fn run<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.run_with(args, None)
    }

    /// 資格情報（あれば）を渡して git を動かす（[`auth`]）。
    pub fn run_auth<I, S>(&self, args: I, cred: Option<&Credentials>) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let args: Vec<String> = args.into_iter().map(|a| a.as_ref().to_string()).collect();
        match cred {
            Some(c) => self.runner.git_auth(&args, None, c),
            None => self.runner.git(&args, None),
        }
    }

    /// 資格情報を渡して、標準入力も渡す（試験用: `git credential fill`）。
    pub fn run_auth_with(&self, args: &[&str], input: &[u8], cred: &Credentials) -> Result<Output> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        self.runner.git_auth(&args, Some(input), cred)
    }

    /// プル・プッシュ・フェッチの相手の URL（今のブランチのリモート、なければ `origin`）。リモートが
    /// なければ `None`。
    pub fn default_remote_url(&self) -> Option<String> {
        let o = self.run(["ls-remote", "--get-url"]).ok()?;
        let url = o.text().trim().to_string();
        // リモートがなければ、名前（origin）がそのまま返る
        (url.contains("://") || url.contains(':') || url.contains('/')).then_some(url)
    }

    /// 作業ツリーのファイル（起点からの相対パス）の中身。なければ `Ok(None)`。
    pub fn read_worktree(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.runner.read(path)
    }

    /// 状態（ブランチ・上流との差・変更の一覧）。
    pub fn status(&self) -> Result<Status> {
        let out = self.run([
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ])?;
        parse_status(&out.stdout).map_err(|message| GitError {
            command: "git status".into(),
            message,
        })
    }

    fn with_paths<'a>(args: &[&'a str], paths: &'a [String]) -> Vec<&'a str> {
        let mut v: Vec<&str> = args.to_vec();
        v.push("--");
        v.extend(paths.iter().map(String::as_str));
        v
    }

    /// ステージする（削除も）。
    pub fn stage(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        self.run(Self::with_paths(&["add", "-A"], paths))
            .map(|_| ())
    }

    /// すべてステージする。
    pub fn stage_all(&self) -> Result<()> {
        self.run(["add", "-A"]).map(|_| ())
    }

    /// まだコミットがないか。
    fn unborn(&self) -> bool {
        self.run(["rev-parse", "--verify", "-q", "HEAD"]).is_err()
    }

    /// ステージを外す。
    pub fn unstage(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        if self.unborn() {
            self.run(Self::with_paths(&["rm", "--cached", "-r", "-q"], paths))
                .map(|_| ())
        } else {
            self.run(Self::with_paths(&["reset", "-q"], paths))
                .map(|_| ())
        }
    }

    /// すべてのステージを外す。
    pub fn unstage_all(&self) -> Result<()> {
        if self.unborn() {
            self.run(["rm", "--cached", "-r", "-q", "."]).map(|_| ())
        } else {
            self.run(["reset", "-q"]).map(|_| ())
        }
    }

    /// 作業ツリーの変更を捨てる。`tracked` はステージした版（なければ HEAD）に戻し、`untracked` は消す。
    pub fn discard(&self, tracked: &[String], untracked: &[String]) -> Result<()> {
        if !tracked.is_empty() {
            self.run(Self::with_paths(&["checkout", "-q"], tracked))?;
        }
        if !untracked.is_empty() {
            self.run(Self::with_paths(&["clean", "-f", "-q"], untracked))?;
        }
        Ok(())
    }

    /// コミットする（`amend` なら直前のコミットを直す）。コミットの短い説明を返す。
    pub fn commit(&self, message: &str, amend: bool) -> Result<String> {
        let mut args = vec!["commit", "-q", "-F", "-", "--cleanup=strip"];
        if amend {
            args.push("--amend");
        }
        self.run_with(args, Some(message.as_bytes()))?;
        Ok(self
            .run(["log", "-1", "--format=%h %s"])
            .map(|o| o.text().trim().to_string())
            .unwrap_or_default())
    }

    /// 直前のコミットのメッセージ。
    pub fn last_message(&self) -> Result<String> {
        Ok(self
            .run(["log", "-1", "--format=%B"])?
            .text()
            .trim_end()
            .to_string())
    }

    /// ファイルの版の中身。`rev` は `HEAD`、空ならインデックス（ステージした版）。なければ `Ok(None)`。
    pub fn show(&self, rev: &str, path: &str) -> Result<Option<Vec<u8>>> {
        let spec = format!("{rev}:{path}");
        if self.run(["cat-file", "-e", &spec]).is_err() {
            return Ok(None);
        }
        Ok(Some(self.run(["cat-file", "blob", &spec])?.stdout))
    }

    /// ブランチの一覧（手元のブランチ、続いてリモート追跡ブランチ）。
    pub fn branches(&self) -> Result<Vec<Branch>> {
        let out = self.run([
            "for-each-ref",
            "--format=%(refname)%00%(HEAD)%00%(upstream:short)",
            "refs/heads",
            "refs/remotes",
        ])?;
        let mut v = Vec::new();
        for line in out.text().lines() {
            let mut f = line.split('\0');
            let (Some(full), Some(head), up) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            let (name, remote) = if let Some(n) = full.strip_prefix("refs/heads/") {
                (n, false)
            } else if let Some(n) = full.strip_prefix("refs/remotes/") {
                // origin/HEAD は一覧に出さない
                if n.ends_with("/HEAD") {
                    continue;
                }
                (n, true)
            } else {
                continue;
            };
            v.push(Branch {
                name: name.to_string(),
                current: head == "*",
                remote,
                upstream: up.unwrap_or("").to_string(),
            });
        }
        Ok(v)
    }

    /// ブランチの名前として使えるか（`git check-ref-format --branch`）。
    pub fn valid_branch_name(&self, name: &str) -> bool {
        !name.trim().is_empty()
            && !name.starts_with('-')
            && self.run(["check-ref-format", "--branch", name]).is_ok()
    }

    /// ブランチを切り替える。リモート追跡ブランチ（`origin/feat`）なら、同じ名前の手元のブランチを作って
    /// 追跡する（あればそれに切り替える）。
    pub fn switch(&self, branch: &Branch) -> Result<()> {
        if !branch.remote {
            return self.run(["checkout", "-q", &branch.name]).map(|_| ());
        }
        let local = branch
            .name
            .split_once('/')
            .map_or(branch.name.as_str(), |x| x.1);
        let exists = self
            .run([
                "rev-parse",
                "--verify",
                "-q",
                &format!("refs/heads/{local}"),
            ])
            .is_ok();
        if exists {
            self.run(["checkout", "-q", local]).map(|_| ())
        } else {
            self.run(["checkout", "-q", "--track", &branch.name])
                .map(|_| ())
        }
    }

    /// 新しいブランチを作って切り替える。
    pub fn create_branch(&self, name: &str) -> Result<()> {
        if !self.valid_branch_name(name) {
            return Err(GitError {
                command: format!("git checkout -b {name}"),
                message: format!("ブランチの名前「{name}」は使えません"),
            });
        }
        self.run(["checkout", "-q", "-b", name]).map(|_| ())
    }

    /// リモートの名前（`origin` を先に）。
    pub fn remotes(&self) -> Result<Vec<String>> {
        let mut v: Vec<String> = self
            .run(["remote"])?
            .text()
            .lines()
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .collect();
        v.sort_by_key(|r| r != "origin");
        Ok(v)
    }

    /// フェッチ（消えたリモートのブランチは消す）。
    pub fn fetch(&self) -> Result<Output> {
        self.fetch_with(None)
    }

    /// 資格情報（あれば）を渡してフェッチ。
    pub fn fetch_with(&self, cred: Option<&Credentials>) -> Result<Output> {
        self.run_auth(["fetch", "--prune"], cred)
    }

    /// プル。
    pub fn pull(&self) -> Result<Output> {
        self.pull_with(None)
    }

    /// 資格情報（あれば）を渡してプル。
    pub fn pull_with(&self, cred: Option<&Credentials>) -> Result<Output> {
        self.run_auth(["pull"], cred)
    }

    /// プッシュ。上流がなければ最初のリモート（`origin`）の同じ名前のブランチへ、上流として設定する。
    pub fn push(&self, status: &Status) -> Result<Output> {
        self.push_with(status, None)
    }

    /// 資格情報（あれば）を渡してプッシュ。
    pub fn push_with(&self, status: &Status, cred: Option<&Credentials>) -> Result<Output> {
        if status.upstream.is_some() {
            return self.run_auth(["push"], cred);
        }
        let Some(branch) = &status.branch else {
            return Err(GitError {
                command: "git push".into(),
                message: "ブランチにいない（HEAD が切り離されている）ので、プッシュできません"
                    .into(),
            });
        };
        let Some(remote) = self.remotes()?.into_iter().next() else {
            return Err(GitError {
                command: "git push".into(),
                message: "リモートがありません（git remote add で追加してください）".into(),
            });
        };
        self.run_auth(["push", "-u", &remote, branch], cred)
    }
}

#[cfg(test)]
mod tests;

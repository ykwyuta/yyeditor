//! プル・プッシュ・フェッチの資格情報（パスワード・トークン・SSH の鍵のパスフレーズ）。
//!
//! git が資格情報を尋ねる（認証に失敗した）ら、UI が利用者に尋ねて資格情報マネージャーに保存し、同じ操作を
//! 資格情報を渡してやり直す。渡し方は askpass: git（HTTPS のユーザー名・パスワード）と ssh（鍵の
//! パスフレーズ・パスワード）は `GIT_ASKPASS`・`SSH_ASKPASS` のプログラムに問いを渡して答えを読むので、
//! そのプログラムが環境変数 [`USER_ENV`]・[`SECRET_ENV`]（その git の起動にだけ渡す）から答える。
//!
//! - 手元: askpass は yyeditor.exe 自身（[`ASKPASS_ENV`] を付けて起動されたら [`askpass_main`] で答えて
//!   終わる）。
//! - 接続先: `sh -c` の中で一時フォルダに askpass のスクリプト（[`ASKPASS_SCRIPT`]、0700）を作り、資格情報は
//!   コマンドの文字列ではなく標準入力で渡して（`ps` に出ない）、終わったら消す。
//!
//! やり直すときは `-c credential.helper=` で設定済みの資格情報ヘルパーを外す（受け付けられない資格情報を
//! 出し続けるヘルパーがあっても、askpass に尋ねさせる）。

/// askpass として起動されたことを示す環境変数。
pub const ASKPASS_ENV: &str = "YYGIT_ASKPASS";
/// ユーザー名を渡す環境変数。
pub const USER_ENV: &str = "YYGIT_USER";
/// パスワード・トークン・パスフレーズを渡す環境変数。
pub const SECRET_ENV: &str = "YYGIT_SECRET";

/// 資格情報。
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub user: String,
    /// パスワード・トークン・パスフレーズ
    pub secret: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credentials({}, ***)", self.user)
    }
}

/// askpass の答え（問いがユーザー名ならユーザー名、それ以外はパスワードなど）。
pub fn askpass_answer<'a>(prompt: &str, user: &'a str, secret: &'a str) -> &'a str {
    if prompt
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("username")
    {
        user
    } else {
        secret
    }
}

/// askpass として起動されたなら答えを標準出力に書いて `true`（呼んだ側はすぐ終わる）。yyeditor の
/// `main` の初めに呼ぶ。
pub fn askpass_main() -> bool {
    if std::env::var_os(ASKPASS_ENV).is_none() {
        return false;
    }
    let prompt = std::env::args().nth(1).unwrap_or_default();
    let user = std::env::var(USER_ENV).unwrap_or_default();
    let secret = std::env::var(SECRET_ENV).unwrap_or_default();
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{}", askpass_answer(&prompt, &user, &secret));
    let _ = out.flush();
    true
}

/// 接続先の askpass のスクリプト（`$1` が問い）。
pub const ASKPASS_SCRIPT: &str = "#!/bin/sh\ncase \"$1\" in\n  [Uu]sername*) printf '%s\\n' \"$YYGIT_USER\" ;;\n  *) printf '%s\\n' \"$YYGIT_SECRET\" ;;\nesac\n";

/// 認証の失敗の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthKind {
    /// HTTPS など（ユーザー名とパスワード・トークン）
    Password,
    /// SSH（鍵のパスフレーズか、SSH のパスワード）
    Ssh,
}

/// git のエラーの出力が、資格情報がない・違うためのものか。
pub fn auth_failure(message: &str) -> Option<AuthKind> {
    let m = message.to_ascii_lowercase();
    let any = |pats: &[&str]| pats.iter().any(|p| m.contains(p));
    if any(&[
        "permission denied (publickey",
        "permission denied (password",
        "permission denied (keyboard-interactive",
        "permission denied, please try again",
        "read_passphrase",
        "incorrect passphrase",
        "bad passphrase",
        "too many authentication failures",
    ]) {
        return Some(AuthKind::Ssh);
    }
    if any(&[
        "terminal prompts disabled",
        "could not read username",
        "could not read password",
        "authentication failed",
        "invalid username or password",
        "http basic: access denied",
        "returned error: 401",
        "returned error: 403",
        "invalid credentials",
        "bad credentials",
        "incorrect username or password",
    ]) {
        return Some(AuthKind::Password);
    }
    None
}

/// リモートの URL から分かること。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteInfo {
    /// 資格情報マネージャーに保存する名前（`git/https://github.com`・`git/ssh/git@github.com`）
    pub key: String,
    /// 表示する名前（`https://github.com`・`git@github.com`）
    pub label: String,
    /// URL に書いてあったユーザー名（なければ空）
    pub user: String,
    pub kind: AuthKind,
}

/// リモートの URL（`https://user@host/path`・`git@host:path`・`ssh://git@host:22/path`）を読む。
pub fn remote_info(url: &str) -> RemoteInfo {
    let url = url.trim();
    if let Some((scheme, rest)) = url.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        let authority = rest.split('/').next().unwrap_or(rest);
        let (user, host) = match authority.rsplit_once('@') {
            Some((u, h)) => (u.split(':').next().unwrap_or(u).to_string(), h.to_string()),
            None => (String::new(), authority.to_string()),
        };
        if scheme == "ssh" || scheme.starts_with("git+ssh") || scheme == "ssh+git" {
            let label = if user.is_empty() {
                host.clone()
            } else {
                format!("{user}@{host}")
            };
            return RemoteInfo {
                key: format!("git/ssh/{label}"),
                label,
                user,
                kind: AuthKind::Ssh,
            };
        }
        let label = format!("{scheme}://{host}");
        return RemoteInfo {
            key: format!("git/{label}"),
            label,
            user,
            kind: AuthKind::Password,
        };
    }
    // scp 形式（user@host:path）
    let target = url.split(':').next().unwrap_or(url);
    let user = target
        .rsplit_once('@')
        .map(|(u, _)| u.to_string())
        .unwrap_or_default();
    RemoteInfo {
        key: format!("git/ssh/{target}"),
        label: target.to_string(),
        user,
        kind: AuthKind::Ssh,
    }
}

/// 資格情報を渡す git の追加の引数（設定済みの資格情報ヘルパーを外す）。
pub(crate) fn auth_args() -> [&'static str; 2] {
    ["-c", "credential.helper="]
}

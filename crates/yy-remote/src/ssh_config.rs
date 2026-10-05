//! 接続設定の解決（11 章 4.4）。
//!
//! 入力された接続先（`[ユーザー@]ホスト[:ポート]` または設定の名前）に、yyeditor の接続設定と
//! `~/.ssh/config` の内容を重ねて、接続に使う値を決める。`~/.ssh/config` はファイルを読むだけで、
//! OpenSSH のプログラムは使わない（16 で決定）。
//!
//! `~/.ssh/config` のうち解釈するのは `Host`・`HostName`・`User`・`Port`・`IdentityFile`・
//! `ProxyJump` だけで、`Match` ブロックは読み飛ばす。値は OpenSSH と同じく最初に現れたものが
//! 優先される（`IdentityFile` は重ねる）。外部プログラムを起動する `ProxyCommand` には対応しない。

use std::path::{Path, PathBuf};

use crate::uri::Target;

/// 接続に使う値。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostSpec {
    /// 接続先として入力された名前（履歴やタイトルに使う）
    pub target: Target,
    /// 実際に接続するホスト名
    pub hostname: String,
    pub port: u16,
    pub user: String,
    /// 試す秘密鍵（存在するものだけを使う）
    pub identity_files: Vec<PathBuf>,
    /// 踏み台（M9.4 で対応）
    pub proxy_jump: Option<String>,
    /// エージェントの配置先（`None` なら `~/.yyeditor/agent`）
    pub agent_dir: Option<String>,
}

impl HostSpec {
    /// ホスト鍵の記録・照合に使う名前（OpenSSH と同じく、22 番以外は `[host]:port`）。
    pub fn known_hosts_name(&self) -> String {
        if self.port == 22 {
            self.hostname.clone()
        } else {
            format!("[{}]:{}", self.hostname, self.port)
        }
    }

    /// `ユーザー@ホスト` の表示。
    pub fn user_host(&self) -> String {
        format!("{}@{}", self.user, self.hostname)
    }
}

/// yyeditor の接続設定（`config.toml` の `[remote.host.<名前>]`）の 1 項目。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostOverride {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    pub agent_dir: Option<String>,
}

/// 接続先を解決するための材料。
#[derive(Clone, Debug, Default)]
pub struct Resolver {
    /// yyeditor の接続設定（名前 → 値）
    pub hosts: Vec<(String, HostOverride)>,
    /// すべての接続先に共通のエージェントの配置先
    pub agent_dir: Option<String>,
    /// `~/.ssh/config` の内容（読まない設定なら空）
    pub ssh_config: String,
    /// 端末のホームフォルダ（`~` の展開に使う）
    pub local_home: Option<PathBuf>,
    /// 既定のユーザー名（端末にログインしているユーザー）
    pub local_user: String,
}

impl Resolver {
    /// 端末の環境（ホームフォルダ・ユーザー名・`~/.ssh/config`）から作る。
    pub fn from_environment(read_ssh_config: bool) -> Resolver {
        let local_home = std::env::home_dir();
        let ssh_config = match (&local_home, read_ssh_config) {
            (Some(h), true) => {
                std::fs::read_to_string(h.join(".ssh").join("config")).unwrap_or_default()
            }
            _ => String::new(),
        };
        let local_user = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default();
        Resolver {
            hosts: Vec::new(),
            agent_dir: None,
            ssh_config,
            local_home,
            local_user,
        }
    }

    /// 接続先を解決する。
    pub fn resolve(&self, target: &Target) -> HostSpec {
        let own = self
            .hosts
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(&target.host))
            .map(|(_, h)| h.clone())
            .unwrap_or_default();
        let sc = parse_for_host(&self.ssh_config, &target.host);
        let hostname = own
            .hostname
            .clone()
            .or_else(|| sc.hostname.clone())
            .unwrap_or_else(|| target.host.clone());
        let hostname = hostname.replace("%h", &target.host);
        let user = target
            .user
            .clone()
            .or_else(|| own.user.clone())
            .or_else(|| sc.user.clone())
            .unwrap_or_else(|| self.local_user.clone());
        let port = target.port.or(own.port).or(sc.port).unwrap_or(22);
        let mut identity_files: Vec<PathBuf> = Vec::new();
        let expand = |s: &str| self.expand(s, &hostname, &user);
        if let Some(f) = &own.identity_file {
            identity_files.push(expand(f));
        }
        identity_files.extend(sc.identity_files.iter().map(|f| expand(f)));
        if identity_files.is_empty()
            && let Some(h) = &self.local_home
        {
            for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
                identity_files.push(h.join(".ssh").join(name));
            }
        }
        HostSpec {
            target: target.clone(),
            hostname,
            port,
            user,
            identity_files,
            proxy_jump: own.proxy_jump.clone().or(sc.proxy_jump),
            agent_dir: own.agent_dir.clone().or_else(|| self.agent_dir.clone()),
        }
    }

    /// `~` と `%d`・`%h`・`%r`・`%u`・`%%` を展開する。
    fn expand(&self, s: &str, host: &str, user: &str) -> PathBuf {
        let home = self
            .local_home
            .as_deref()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut out = String::new();
        let mut rest = s;
        if let Some(r) = rest.strip_prefix("~/").or_else(|| rest.strip_prefix("~\\")) {
            out.push_str(&home);
            out.push(std::path::MAIN_SEPARATOR);
            rest = r;
        } else if rest == "~" {
            return PathBuf::from(home);
        }
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('d') => out.push_str(&home),
                Some('h') => out.push_str(host),
                Some('r') => out.push_str(user),
                Some('u') => out.push_str(&self.local_user),
                Some('%') => out.push('%'),
                Some(o) => {
                    out.push('%');
                    out.push(o);
                }
                None => out.push('%'),
            }
        }
        PathBuf::from(out)
    }
}

/// `~/.ssh/config` から読み取った値。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SshConfigEntry {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_files: Vec<String>,
    pub proxy_jump: Option<String>,
}

/// `~/.ssh/config` の内容のうち、`host` に当てはまる値を集める。
pub fn parse_for_host(text: &str, host: &str) -> SshConfigEntry {
    let mut out = SshConfigEntry::default();
    // 最初の Host 行より前は全ホストに当てはまる
    let mut active = true;
    for line in text.lines() {
        let Some((key, value)) = split_line(line) else {
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "host" => active = host_matches(&value, host),
            // Match の条件は評価しない（読み飛ばす）
            "match" => active = false,
            _ if !active => {}
            "hostname" => {
                out.hostname.get_or_insert(first_word(&value));
            }
            "user" => {
                out.user.get_or_insert(first_word(&value));
            }
            "port" => {
                if out.port.is_none() {
                    out.port = first_word(&value).parse().ok();
                }
            }
            "identityfile" => out.identity_files.push(unquote(value.trim())),
            "proxyjump" if out.proxy_jump.is_none() => {
                let v = first_word(&value);
                out.proxy_jump = Some(v).filter(|v| !v.eq_ignore_ascii_case("none"));
            }
            _ => {}
        }
    }
    out
}

/// 1 行を `(キーワード, 値)` に分ける（`Key value` と `Key=value` の両方）。
fn split_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let key = &line[..end];
    let rest = line[end..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim();
    Some((key.to_owned(), rest.to_owned()))
}

fn unquote(s: &str) -> String {
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s)
        .to_owned()
}

fn first_word(s: &str) -> String {
    let s = s.trim();
    if let Some(q) = s.strip_prefix('"') {
        return q.split('"').next().unwrap_or("").to_owned();
    }
    s.split_whitespace().next().unwrap_or("").to_owned()
}

/// `Host` 行のパターン（空白区切り、`*`・`?`、`!` で否定）に `host` が当てはまるか。
fn host_matches(patterns: &str, host: &str) -> bool {
    let mut matched = false;
    for p in patterns.split_whitespace() {
        let p = p.trim_matches('"');
        if let Some(neg) = p.strip_prefix('!') {
            if wildcard(neg, host) {
                return false;
            }
        } else if wildcard(p, host) {
            matched = true;
        }
    }
    matched
}

/// `*` と `?` のワイルドカード（大文字小文字を区別しない）。
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// 端末のホームフォルダの `.ssh/known_hosts`（読むだけ）。
pub fn user_known_hosts(home: &Path) -> PathBuf {
    home.join(".ssh").join("known_hosts")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
# comment
IdentityFile ~/.ssh/global_key

Host build-server build
    HostName build01.example.co.jp
    User yamada
    Port 2222
    IdentityFile "~/.ssh/id_build"
    ProxyJump bastion

Host *.example.co.jp !secret.example.co.jp
    User=ops
    Port 2200

Match host foo
    User matched

Host *
    User fallback
    HostName %h.internal
"#;

    fn resolver() -> Resolver {
        Resolver {
            ssh_config: CONFIG.into(),
            local_home: Some(PathBuf::from("/home/me")),
            local_user: "me".into(),
            ..Resolver::default()
        }
    }

    #[test]
    fn first_value_wins() {
        let e = parse_for_host(CONFIG, "build");
        assert_eq!(e.hostname.as_deref(), Some("build01.example.co.jp"));
        assert_eq!(e.user.as_deref(), Some("yamada"));
        assert_eq!(e.port, Some(2222));
        assert_eq!(e.identity_files, ["~/.ssh/global_key", "~/.ssh/id_build"]);
        assert_eq!(e.proxy_jump.as_deref(), Some("bastion"));

        let e = parse_for_host(CONFIG, "web.example.co.jp");
        assert_eq!(e.user.as_deref(), Some("ops"));
        assert_eq!(e.port, Some(2200));
        let e = parse_for_host(CONFIG, "secret.example.co.jp");
        assert_eq!(e.user.as_deref(), Some("fallback"));
        assert_eq!(e.port, None);
        // Match ブロックは使わない
        assert_eq!(
            parse_for_host(CONFIG, "foo").user.as_deref(),
            Some("fallback")
        );
    }

    #[test]
    fn resolves_with_overrides() {
        let r = resolver();
        let s = r.resolve(&Target::parse("build").unwrap());
        assert_eq!(s.hostname, "build01.example.co.jp");
        assert_eq!((s.user.as_str(), s.port), ("yamada", 2222));
        assert_eq!(
            s.identity_files,
            [
                PathBuf::from("/home/me/.ssh/global_key"),
                PathBuf::from("/home/me/.ssh/id_build")
            ]
        );
        assert_eq!(s.known_hosts_name(), "[build01.example.co.jp]:2222");

        // 入力した値が最優先、次に yyeditor の接続設定
        let mut r = resolver();
        r.hosts.push((
            "Build".into(),
            HostOverride {
                user: Some("override".into()),
                agent_dir: Some("/work/agent".into()),
                ..HostOverride::default()
            },
        ));
        let s = r.resolve(&Target::parse("root@build:22").unwrap());
        assert_eq!((s.user.as_str(), s.port), ("root", 22));
        let s = r.resolve(&Target::parse("build").unwrap());
        assert_eq!(s.user, "override");
        assert_eq!(s.agent_dir.as_deref(), Some("/work/agent"));

        let s = r.resolve(&Target::parse("other").unwrap());
        assert_eq!(s.hostname, "other.internal");
        assert_eq!(s.user, "fallback");
    }

    #[test]
    fn defaults_without_ssh_config() {
        let r = Resolver {
            local_home: Some(PathBuf::from("/home/me")),
            local_user: "me".into(),
            ..Resolver::default()
        };
        let s = r.resolve(&Target::parse("10.0.0.5").unwrap());
        assert_eq!(
            (s.hostname.as_str(), s.user.as_str(), s.port),
            ("10.0.0.5", "me", 22)
        );
        assert_eq!(s.identity_files.len(), 3);
        assert_eq!(s.known_hosts_name(), "10.0.0.5");
    }

    #[test]
    fn wildcards() {
        assert!(wildcard("*", "x"));
        assert!(wildcard("*.example.com", "a.b.EXAMPLE.com"));
        assert!(wildcard("h?st", "host"));
        assert!(!wildcard("h?st", "hoost"));
        assert!(wildcard("a*b*c", "aXXbYYc"));
        assert!(!wildcard("a*b*c", "aXXbYY"));
    }
}

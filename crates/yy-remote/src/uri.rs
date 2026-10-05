//! 接続先のファイルの場所の表し方（11 章 5.3）。
//!
//! `ssh://[ユーザー@]ホスト[:ポート]/絶対パス` の形で、履歴・ブックマーク・タイトルに使う。
//! ホストは接続設定の名前（`~/.ssh/config` の `Host` など）でもよい。パスのうち `%`、制御文字、
//! UTF-8 として正しくないバイトは `%XX` で表す（それ以外の文字はそのまま読めるように残す）。

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RemoteUri {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
    /// 接続先の絶対パス（バイト列）
    pub path: Vec<u8>,
}

pub const SCHEME: &str = "ssh://";

impl RemoteUri {
    /// `ssh://` で始まる文字列を読む。形が正しくなければ `None`。
    pub fn parse(s: &str) -> Option<RemoteUri> {
        let rest = s.strip_prefix(SCHEME)?;
        let slash = rest.find('/')?;
        let (target, path) = rest.split_at(slash);
        let t = Target::parse(target)?;
        Some(RemoteUri {
            user: t.user,
            host: t.host,
            port: t.port,
            path: decode(path)?,
        })
    }

    /// 接続先の名前（`ユーザー@ホスト:ポート` のうち指定があるもの）。
    pub fn target(&self) -> Target {
        Target {
            user: self.user.clone(),
            host: self.host.clone(),
            port: self.port,
        }
    }

    /// 同じ接続先・同じパスか（ホスト名の大文字小文字は区別しない。パスは区別する）。
    pub fn same(&self, other: &RemoteUri) -> bool {
        self.target().same(&other.target()) && self.path == other.path
    }
}

impl fmt::Display for RemoteUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{SCHEME}{}{}", self.target(), encode(&self.path))
    }
}

/// 接続先（`[ユーザー@]ホスト[:ポート]`）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl Target {
    /// `[ユーザー@]ホスト[:ポート]` を読む。IPv6 アドレスは `[::1]:22` のように角括弧で囲む。
    pub fn parse(s: &str) -> Option<Target> {
        let s = s.trim();
        let (user, hostport) = match s.rsplit_once('@') {
            Some((u, h)) if !u.is_empty() => (Some(u.to_owned()), h),
            Some(_) => return None,
            None => (None, s),
        };
        let (host, port) = if let Some(inner) = hostport.strip_prefix('[') {
            let (h, after) = inner.split_once(']')?;
            let port = match after.strip_prefix(':') {
                Some(p) => Some(p.parse().ok()?),
                None if after.is_empty() => None,
                None => return None,
            };
            (h.to_owned(), port)
        } else {
            match hostport.split_once(':') {
                Some((h, p)) => (h.to_owned(), Some(p.parse().ok()?)),
                None => (hostport.to_owned(), None),
            }
        };
        let valid = !host.is_empty()
            && host
                .chars()
                .all(|c| !c.is_whitespace() && !c.is_control() && !"/@".contains(c));
        valid.then_some(Target { user, host, port })
    }

    pub fn same(&self, other: &Target) -> bool {
        self.user == other.user
            && self.port == other.port
            && self.host.eq_ignore_ascii_case(&other.host)
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(u) = &self.user {
            write!(f, "{u}@")?;
        }
        if self.host.contains(':') {
            write!(f, "[{}]", self.host)?;
        } else {
            f.write_str(&self.host)?;
        }
        if let Some(p) = self.port {
            write!(f, ":{p}")?;
        }
        Ok(())
    }
}

fn encode(path: &[u8]) -> String {
    let mut out = String::new();
    for chunk in path.utf8_chunks() {
        for c in chunk.valid().chars() {
            if c == '%' || c.is_control() {
                let mut b = [0; 4];
                for byte in c.encode_utf8(&mut b).bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            } else {
                out.push(c);
            }
        }
        for b in chunk.invalid() {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for s in [
            "ssh://build/home/a/x.txt",
            "ssh://yamada@build01.example.co.jp:2222/var/log/日本語 ファイル.log",
            "ssh://[::1]:22/etc/hosts",
            "ssh://h/100%25/a",
        ] {
            let u = RemoteUri::parse(s).unwrap();
            assert_eq!(u.to_string(), s);
        }
        let u = RemoteUri::parse("ssh://u@h:22/a%FFb").unwrap();
        assert_eq!(u.path, b"/a\xffb");
        assert_eq!(u.user.as_deref(), Some("u"));
        assert_eq!(u.port, Some(22));
        let raw = RemoteUri {
            user: None,
            host: "h".into(),
            port: None,
            path: b"/x\xe3\x81/\ttab".to_vec(),
        };
        assert_eq!(raw.to_string(), "ssh://h/x%E3%81/%09tab");
        assert_eq!(RemoteUri::parse(&raw.to_string()).unwrap(), raw);
    }

    #[test]
    fn rejects_malformed() {
        for s in [
            "C:\\x.txt",
            "ssh://host",
            "ssh:///path",
            "ssh://h:port/x",
            "ssh://@h/x",
            "ssh://h/%ZZ",
        ] {
            assert!(RemoteUri::parse(s).is_none(), "{s}");
        }
    }

    #[test]
    fn targets() {
        let t = Target::parse("me@host:2200").unwrap();
        assert_eq!(
            (t.user.as_deref(), t.host.as_str(), t.port),
            (Some("me"), "host", Some(2200))
        );
        assert!(
            Target::parse("Host")
                .unwrap()
                .same(&Target::parse("host").unwrap())
        );
        assert!(Target::parse("a b").is_none());
        assert!(Target::parse("").is_none());
        let a = RemoteUri::parse("ssh://H/a").unwrap();
        assert!(a.same(&RemoteUri::parse("ssh://h/a").unwrap()));
        assert!(!a.same(&RemoteUri::parse("ssh://h/A").unwrap()));
    }
}

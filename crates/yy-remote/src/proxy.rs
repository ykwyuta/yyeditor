//! プロキシの設定（11 章 4.5）。
//!
//! 接続先（または最初の踏み台）への TCP 接続に使う HTTP（CONNECT）・SOCKS のプロキシを、
//! `scheme://[ユーザー[:パスワード]@]ホスト[:ポート]` の形で書く。`~/.ssh/config` の
//! `ProxyCommand` は外部のプログラムを起動しないと使えないため、よく使われる形
//! （`ssh -W %h:%p 踏み台`・`nc -X`・`ncat --proxy`・`connect -S`/`-H`）だけを同じ意味の
//! 踏み台・プロキシに読み替える（[`translate_command`]）。

use std::fmt;

/// プロキシの種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyKind {
    /// HTTP の CONNECT
    Http,
    /// SOCKS5（ホスト名はプロキシ側で解決する）
    Socks5,
    /// SOCKS4（ホスト名は 4a の形でプロキシ側に解決させる）
    Socks4,
}

impl ProxyKind {
    fn scheme(self) -> &'static str {
        match self {
            ProxyKind::Http => "http",
            ProxyKind::Socks5 => "socks5",
            ProxyKind::Socks4 => "socks4",
        }
    }

    fn default_port(self) -> u16 {
        match self {
            ProxyKind::Http => 8080,
            ProxyKind::Socks5 | ProxyKind::Socks4 => 1080,
        }
    }
}

/// プロキシ。
#[derive(Clone, PartialEq, Eq)]
pub struct Proxy {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    /// 認証のユーザー名（HTTP の Basic 認証・SOCKS5 のユーザー名認証。SOCKS4 では USERID）
    pub user: Option<String>,
    /// パスワード（ユーザー名だけを書いた場合は接続のときに尋ねる）
    pub password: Option<String>,
}

// パスワードを表示しない
impl fmt::Debug for Proxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Proxy({self})")
    }
}

impl fmt::Display for Proxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://", self.kind.scheme())?;
        if let Some(u) = &self.user {
            write!(f, "{u}@")?;
        }
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// `none`（または空）か。直接接続する指定。
pub fn is_none(s: &str) -> bool {
    let s = s.trim();
    s.is_empty() || s.eq_ignore_ascii_case("none")
}

impl Proxy {
    /// `http://proxy:8080`・`socks5://user@proxy:1080` などを読む。`none` と空は `Ok(None)`。
    pub fn parse(s: &str) -> Result<Option<Proxy>, String> {
        if is_none(s) {
            return Ok(None);
        }
        let s = s.trim();
        let bad = || {
            format!(
                "プロキシの指定（{s}）を読めません。http://ホスト:ポート や socks5://ホスト:ポート の形で書いてください"
            )
        };
        let (scheme, rest) = s.split_once("://").ok_or_else(bad)?;
        let kind = match scheme.to_ascii_lowercase().as_str() {
            "http" => ProxyKind::Http,
            "socks" | "socks5" | "socks5h" => ProxyKind::Socks5,
            "socks4" | "socks4a" => ProxyKind::Socks4,
            _ => return Err(bad()),
        };
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        let (userinfo, hostport) = match rest.rsplit_once('@') {
            Some((u, h)) => (Some(u), h),
            None => (None, rest),
        };
        let (user, password) = match userinfo {
            Some(u) => {
                let (name, pass) = match u.split_once(':') {
                    Some((n, p)) => (n, Some(decode(p).ok_or_else(bad)?)),
                    None => (u, None),
                };
                (Some(decode(name).ok_or_else(bad)?), pass)
            }
            None => (None, None),
        };
        let (host, port) = split_host_port(hostport).ok_or_else(bad)?;
        Ok(Some(Proxy {
            kind,
            host,
            port: port.unwrap_or(kind.default_port()),
            user: user.filter(|u| !u.is_empty()),
            password,
        }))
    }
}

/// `host`・`host:port`・`[v6]`・`[v6]:port` を分ける。
fn split_host_port(s: &str) -> Option<(String, Option<u16>)> {
    let (host, port) = if let Some(inner) = s.strip_prefix('[') {
        let (h, after) = inner.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => Some(p.parse().ok()?),
            None if after.is_empty() => None,
            None => return None,
        };
        (h, port)
    } else {
        match s.split_once(':') {
            Some((h, p)) => (h, Some(p.parse().ok()?)),
            None => (s, None),
        }
    };
    let valid = !host.is_empty()
        && host
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && !"/@".contains(c));
    valid.then(|| (host.to_owned(), port))
}

/// `%XX` を戻す。
fn decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `ProxyCommand` を読み替えた結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// 踏み台（`ProxyJump` と同じ形の文字列）
    Jump(String),
    Proxy(Proxy),
}

/// `ProxyCommand` を同じ意味の踏み台・プロキシに読み替える。読み替えられなければ説明を返す。
pub fn translate_command(command: &str) -> Result<Route, String> {
    let unsupported = || {
        format!(
            "ProxyCommand（{}）には対応していません。yyeditor は外部のプログラムを起動しないため、\
             ProxyJump か、yyeditor の設定の proxy を使ってください",
            command.trim()
        )
    };
    let mut words = split_words(command).ok_or_else(unsupported)?;
    if words.first().is_some_and(|w| w == "exec") {
        words.remove(0);
    }
    let Some((program, args)) = words.split_first() else {
        return Err(unsupported());
    };
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    let route = match name {
        "ssh" => ssh_w(args),
        "nc" | "netcat" => nc(args),
        "ncat" => ncat(args),
        "connect" | "connect-proxy" => connect(args),
        _ => None,
    };
    route.ok_or_else(unsupported)
}

/// 空白で区切る（単一・二重引用符を外す）。
fn split_words(s: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut cur: Option<String> = None;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.get_or_insert_default().push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                cur.get_or_insert_default();
            }
            None if c.is_whitespace() => words.extend(cur.take()),
            None => cur.get_or_insert_default().push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    words.extend(cur);
    Some(words)
}

/// 末尾の `%h %p` を確かめる。
fn ends_with_target(rest: &[&str]) -> bool {
    rest == ["%h", "%p"]
}

/// `ssh [-q] [-p ポート] [-l ユーザー] -W %h:%p [ユーザー@]踏み台`
fn ssh_w(args: &[String]) -> Option<Route> {
    let mut host = None;
    let mut port = None;
    let mut user = None;
    let mut forward = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-W" => {
                let w = it.next()?;
                if w != "%h:%p" && w != "[%h]:%p" {
                    return None;
                }
                forward = true;
            }
            "-p" => port = Some(it.next()?.parse::<u16>().ok()?),
            "-l" => user = Some(it.next()?.clone()),
            a if a.starts_with('-') => {
                // 引数を取らない、接続の向き先を変えないものだけ
                if !a[1..].chars().all(|c| "qTAaxCNn46v".contains(c)) || a.len() < 2 {
                    return None;
                }
            }
            a if host.is_none() => host = Some(a.to_owned()),
            _ => return None,
        }
    }
    let host = host?;
    if !forward || host.contains(',') {
        return None;
    }
    let target = crate::uri::Target::parse(host.strip_prefix("ssh://").unwrap_or(&host))?;
    let user = user.or(target.user);
    let port = port.or(target.port);
    let mut out = String::new();
    if let Some(u) = user {
        out.push_str(&u);
        out.push('@');
    }
    if target.host.contains(':') {
        out.push_str(&format!("[{}]", target.host));
    } else {
        out.push_str(&target.host);
    }
    if let Some(p) = port {
        out.push_str(&format!(":{p}"));
    }
    Some(Route::Jump(out))
}

fn proxy_at(kind: ProxyKind, addr: &str, user: Option<String>) -> Option<Route> {
    let (user_in_addr, addr) = match addr.rsplit_once('@') {
        Some((u, a)) => (Some(u.to_owned()), a),
        None => (None, addr),
    };
    let (host, port) = split_host_port(addr)?;
    Some(Route::Proxy(Proxy {
        kind,
        host,
        port: port.unwrap_or(kind.default_port()),
        user: user.or(user_in_addr),
        password: None,
    }))
}

/// `nc [-X 5|4|connect] -x プロキシ[:ポート] [-P ユーザー] %h %p`（OpenBSD の nc）
fn nc(args: &[String]) -> Option<Route> {
    let mut kind = ProxyKind::Socks5;
    let mut addr = None;
    let mut user = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-X" => {
                kind = match it.next()?.to_ascii_lowercase().as_str() {
                    "5" => ProxyKind::Socks5,
                    "4" => ProxyKind::Socks4,
                    "connect" => ProxyKind::Http,
                    _ => return None,
                }
            }
            "-x" => addr = Some(it.next()?.clone()),
            "-P" => user = Some(it.next()?.clone()),
            "-w" => {
                it.next()?;
            }
            "-4" | "-6" | "-v" | "-N" => {}
            a if a.starts_with('-') => return None,
            a => rest.push(a),
        }
    }
    if !ends_with_target(&rest) {
        return None;
    }
    proxy_at(kind, &addr?, user)
}

/// `ncat --proxy プロキシ:ポート [--proxy-type http|socks4|socks5] [--proxy-auth ユーザー[:パスワード]] %h %p`
fn ncat(args: &[String]) -> Option<Route> {
    let mut kind = ProxyKind::Http;
    let mut addr = None;
    let mut auth: Option<String> = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (key, inline) = match a.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k, Some(v.to_owned())),
            _ => (a.as_str(), None),
        };
        let mut value = || inline.clone().or_else(|| it.next().cloned());
        match key {
            "--proxy" => addr = Some(value()?),
            "--proxy-type" => {
                kind = match value()?.to_ascii_lowercase().as_str() {
                    "http" => ProxyKind::Http,
                    "socks4" => ProxyKind::Socks4,
                    "socks5" => ProxyKind::Socks5,
                    _ => return None,
                }
            }
            "--proxy-auth" => auth = Some(value()?),
            "-4" | "-6" | "-v" => {}
            a if a.starts_with('-') => return None,
            a => rest.push(a),
        }
    }
    if !ends_with_target(&rest) {
        return None;
    }
    let mut route = proxy_at(kind, &addr?, None)?;
    if let (Route::Proxy(p), Some(auth)) = (&mut route, auth) {
        match auth.split_once(':') {
            Some((u, pw)) => {
                p.user = Some(u.to_owned());
                p.password = Some(pw.to_owned());
            }
            None => p.user = Some(auth),
        }
    }
    Some(route)
}

/// `connect [-4|-5] -S [ユーザー@]プロキシ[:ポート] %h %p`・`connect -H [ユーザー@]プロキシ[:ポート] %h %p`
fn connect(args: &[String]) -> Option<Route> {
    let mut socks = ProxyKind::Socks5;
    let mut target: Option<(bool, String)> = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-S" => target = Some((true, it.next()?.clone())),
            "-H" => target = Some((false, it.next()?.clone())),
            "-4" => socks = ProxyKind::Socks4,
            "-5" => socks = ProxyKind::Socks5,
            "-a" | "-w" => {
                it.next()?;
            }
            a if a.starts_with('-') => return None,
            a => rest.push(a),
        }
    }
    if !ends_with_target(&rest) {
        return None;
    }
    let (is_socks, addr) = target?;
    proxy_at(if is_socks { socks } else { ProxyKind::Http }, &addr, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy(s: &str) -> Proxy {
        Proxy::parse(s).unwrap().unwrap()
    }

    #[test]
    fn parses_proxy_urls() {
        let p = proxy("http://proxy.example.com:3128");
        assert_eq!(
            (p.kind, p.host.as_str(), p.port),
            (ProxyKind::Http, "proxy.example.com", 3128)
        );
        assert_eq!(p.user, None);
        let p = proxy("socks5h://me%40corp:p%3Ass@[::1]");
        assert_eq!(
            (p.kind, p.host.as_str(), p.port),
            (ProxyKind::Socks5, "::1", 1080)
        );
        assert_eq!(p.user.as_deref(), Some("me@corp"));
        assert_eq!(p.password.as_deref(), Some("p:ss"));
        // パスワードは表示しない
        assert_eq!(p.to_string(), "socks5://me@corp@[::1]:1080");
        assert!(!format!("{p:?}").contains("p:ss"));
        assert_eq!(proxy("socks4://gw").kind, ProxyKind::Socks4);
        assert_eq!(proxy("http://gw/").port, 8080);
        assert_eq!(Proxy::parse("none"), Ok(None));
        assert_eq!(Proxy::parse(" "), Ok(None));
        assert!(Proxy::parse("proxy:8080").is_err());
        assert!(Proxy::parse("ftp://proxy").is_err());
        assert!(Proxy::parse("http://proxy:port").is_err());
    }

    #[test]
    fn translates_proxy_commands() {
        let t = |s: &str| translate_command(s);
        assert_eq!(t("ssh -W %h:%p bastion"), Ok(Route::Jump("bastion".into())));
        assert_eq!(
            t("exec /usr/bin/ssh -q -p 2222 -l ops -W [%h]:%p gw.example.com"),
            Ok(Route::Jump("ops@gw.example.com:2222".into()))
        );
        assert_eq!(
            t("ssh.exe -W %h:%p admin@jump:22"),
            Ok(Route::Jump("admin@jump:22".into()))
        );
        assert_eq!(
            t("nc -X 5 -x socks.example.com:1080 %h %p"),
            Ok(Route::Proxy(proxy("socks5://socks.example.com:1080")))
        );
        assert_eq!(
            t("nc -X connect -x proxy:3128 -P alice %h %p"),
            Ok(Route::Proxy(proxy("http://alice@proxy:3128")))
        );
        assert_eq!(
            t("/usr/bin/nc -x gw %h %p"),
            Ok(Route::Proxy(proxy("socks5://gw:1080")))
        );
        assert_eq!(
            t("ncat --proxy proxy:8080 --proxy-type=socks4 %h %p"),
            Ok(Route::Proxy(proxy("socks4://proxy:8080")))
        );
        assert_eq!(
            t("ncat --proxy proxy:3128 --proxy-auth 'bob:s ecret' %h %p"),
            Ok(Route::Proxy(proxy("http://bob:s%20ecret@proxy:3128")))
        );
        assert_eq!(
            t(r#""C:\Program Files\Git\mingw64\bin\connect.exe" -H proxy.local:8080 %h %p"#),
            Ok(Route::Proxy(proxy("http://proxy.local:8080")))
        );
        assert_eq!(
            t("connect -4 -S user@socks %h %p"),
            Ok(Route::Proxy(proxy("socks4://user@socks:1080")))
        );
        // 読み替えられないもの
        for s in [
            "ssh -W %h:%p -o ProxyCommand=foo bastion",
            "ssh -W other:22 bastion",
            "ssh bastion nc %h %p",
            "nc -x proxy %h 22",
            "socat - PROXY:proxy:%h:%p",
            "aws ssm start-session --target %h",
            "ssh -W %h:%p 'unterminated",
        ] {
            let e = t(s).unwrap_err();
            assert!(e.contains("ProxyCommand"), "{s}: {e}");
        }
    }
}

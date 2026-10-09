//! ドメインごとのプロキシ（19 章 3.5）と、ホストの転送・開発者用証明書（19 章 3.6）。
//!
//! * ドメインごとのプロキシは、プロファイルから自動構成のスクリプト（PAC）を作り、`data:` の URL で
//!   Chromium に渡す（`--proxy-pac-url`）。最初に当てはまった規則を使い、どれにも当てはまらなければ
//!   プロファイルのやり方（直接・指定のプロキシ）に従う。
//! * ホストの転送は `--host-resolver-rules` の `MAP`（名前の解決を差し替える。ポートも替えられる）。
//!   転送するホストはプロキシを通さない（通すとプロキシ側で名前を解決してしまう）。
//! * 開発者用証明書は、転送先のサーバーが出す自己署名の証明書の SHA-256 の指紋を覚えておき、
//!   一致したときだけ証明書のエラーを許す（OS の証明書ストアは変えない）。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// ドメインごとのプロキシの規則（`*.corp.example.jp = 10.0.0.1:8080`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyRule {
    /// ホストの型（`example.com` はそのドメインとサブドメイン、`*` は任意の文字列、
    /// `10.0.0.0/8` は IPv4 アドレスの範囲）
    pub pattern: String,
    /// `direct`（直接）か、プロキシ（`host:port`・`socks5://host:port`・`https://host:port`）
    pub proxy: String,
}

/// ホストの転送（`www.example.com = 127.0.0.1:8443`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostMap {
    /// ホスト（`*` を使える）。`:443` のようにポートを付けると、そのポートだけ
    pub host: String,
    /// 転送先（`127.0.0.1:8443`・`[::1]:8443`。ポートを省くと元のポート）
    pub address: String,
    /// 開発者用証明書の SHA-256 の指紋（`AB:CD:…`）。空なら証明書のエラーは許さない
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cert_sha256: String,
}

/// ホスト名・型に使える文字か。
fn host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '*')
}

/// `10.0.0.0/8` を（アドレス, 長さ）に。
fn parse_cidr(s: &str) -> Option<([u8; 4], u8)> {
    let (a, n) = s.split_once('/')?;
    let ip: std::net::Ipv4Addr = a.parse().ok()?;
    let n: u8 = n.parse().ok()?;
    (n <= 32).then_some((ip.octets(), n))
}

/// 規則の型を確かめる。
fn check_pattern(p: &str) -> Result<(), String> {
    if p.is_empty() {
        return Err("ホストの型が空です".into());
    }
    if p.contains('/') {
        return parse_cidr(p)
            .map(|_| ())
            .ok_or_else(|| format!("「{p}」は IPv4 の範囲（例: 10.0.0.0/8）として読めません"));
    }
    if !p.chars().all(host_char) {
        return Err(format!("ホストの型「{p}」に使えない文字があります"));
    }
    Ok(())
}

/// 規則のプロキシ（`direct` かプロキシ 1 つ）を確かめる。
fn check_rule_proxy(s: &str) -> Result<(), String> {
    if s.eq_ignore_ascii_case("direct") {
        return Ok(());
    }
    crate::proxy::check_one(s)
}

/// 転送するホスト（`host[:port]`）を（ホスト, ポート）に分ける。
pub fn split_host_port(s: &str) -> Result<(&str, Option<u16>), String> {
    let (h, p) = match s.rsplit_once(':') {
        Some((h, p)) => {
            let port = match p.parse::<u16>() {
                Ok(n) if n > 0 => n,
                _ => return Err(format!("「{s}」のポートは 1〜65535 です")),
            };
            (h, Some(port))
        }
        None => (s, None),
    };
    if h.is_empty() || !h.chars().all(host_char) {
        return Err(format!("ホスト「{s}」が正しくありません"));
    }
    Ok((h, p))
}

/// 転送先（`ip[:port]`・`[v6]:port`・`ホスト名[:port]`）を確かめる。
fn check_address(s: &str) -> Result<(), String> {
    if let Some(v6) = s.strip_prefix('[') {
        let (a, rest) = v6
            .split_once(']')
            .ok_or_else(|| format!("転送先「{s}」の IPv6 アドレスが読めません"))?;
        a.parse::<std::net::Ipv6Addr>()
            .map_err(|_| format!("転送先「{s}」の IPv6 アドレスが読めません"))?;
        return match rest {
            "" => Ok(()),
            r => match r.strip_prefix(':').map(str::parse::<u16>) {
                Some(Ok(n)) if n > 0 => Ok(()),
                _ => Err(format!("転送先「{s}」のポートは 1〜65535 です")),
            },
        };
    }
    let (h, _) = split_host_port(s).map_err(|e| format!("転送先: {e}"))?;
    if h.contains('*') {
        return Err(format!("転送先「{s}」に * は使えません"));
    }
    Ok(())
}

/// 指紋を `AB:CD:…`（大文字・コロン区切り）にそろえる。64 桁の 16 進でなければ `None`。
pub fn normalize_fingerprint(s: &str) -> Option<String> {
    let hex: String = s
        .chars()
        .filter(|c| !matches!(c, ':' | ' ' | '-'))
        .collect();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let up = hex.to_ascii_uppercase();
    Some(
        up.as_bytes()
            .chunks(2)
            .map(|c| std::str::from_utf8(c).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(":"),
    )
}

/// DER の SHA-256 の指紋（`AB:CD:…`）。
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// 証明書（PEM なら最初の `CERTIFICATE`、そうでなければ DER）の指紋。
pub fn cert_fingerprint(data: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(data).ok();
    match text.and_then(|t| t.find("-----BEGIN CERTIFICATE-----").map(|i| &t[i..])) {
        Some(t) => {
            let body = t.strip_prefix("-----BEGIN CERTIFICATE-----")?;
            let end = body.find("-----END CERTIFICATE-----")?;
            let der = base64_decode(&body[..end])?;
            Some(fingerprint(&der))
        }
        None if data.first() == Some(&0x30) => Some(fingerprint(data)),
        None => None,
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64 を読む（空白・改行は飛ばす）。
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            break;
        }
        let v = B64.iter().position(|&b| b == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Base64 にする。
pub(crate) fn base64_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for ch in data.chunks(3) {
        let n = ch.len();
        let v = (ch[0] as u32) << 16
            | (*ch.get(1).unwrap_or(&0) as u32) << 8
            | *ch.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= n {
                s.push(B64[(v >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// `*` だけを使う型の照合（大文字・小文字は区別しない）。
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p = pattern.to_ascii_lowercase();
    let t = text.to_ascii_lowercase();
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == t;
    }
    let mut rest = t.as_str();
    let first = parts[0];
    let Some(r) = rest.strip_prefix(first) else {
        return false;
    };
    rest = r;
    let last = parts[parts.len() - 1];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

impl ProxyRule {
    pub fn validate(&self) -> Result<(), String> {
        check_pattern(self.pattern.trim())?;
        check_rule_proxy(self.proxy.trim())
    }

    /// ホストがこの規則に当てはまるか（PAC と同じ考え方。試験・表示用）。
    pub fn matches(&self, host: &str) -> bool {
        let p = self.pattern.trim();
        if let Some((net, n)) = parse_cidr(p) {
            let Ok(ip) = host.parse::<std::net::Ipv4Addr>() else {
                return false;
            };
            let mask = if n == 0 { 0 } else { u32::MAX << (32 - n) };
            return u32::from(ip) & mask == u32::from_be_bytes(net) & mask;
        }
        if p.contains('*') {
            return glob_match(p, host);
        }
        let h = host.to_ascii_lowercase();
        let p = p.to_ascii_lowercase();
        h == p || h.ends_with(&format!(".{p}"))
    }
}

impl HostMap {
    pub fn validate(&self) -> Result<(), String> {
        split_host_port(self.host.trim())?;
        check_address(self.address.trim())?;
        if !self.cert_sha256.trim().is_empty() && normalize_fingerprint(&self.cert_sha256).is_none()
        {
            return Err(format!(
                "「{}」の証明書の指紋は SHA-256（16 進 64 桁）で入れてください",
                self.host.trim()
            ));
        }
        Ok(())
    }

    /// ホスト（とポート）が当てはまるか。
    pub fn matches(&self, host: &str, port: u16) -> bool {
        match split_host_port(self.host.trim()) {
            Ok((h, p)) => glob_match(h, host) && p.is_none_or(|p| p == port),
            Err(_) => false,
        }
    }

    /// Chromium の `MAP` の規則（`MAP www.example.com 127.0.0.1:8443`）。
    fn map_rule(&self) -> String {
        format!("MAP {} {}", self.host.trim(), self.address.trim())
    }

    /// 開発者用証明書の指紋（そろえたもの）。
    pub fn pinned(&self) -> Option<String> {
        normalize_fingerprint(&self.cert_sha256)
    }
}

/// `#` から行末は注釈。
fn strip_comment(line: &str) -> &str {
    line.split_once('#').map_or(line, |(a, _)| a).trim()
}

/// 1 行を「左 = 右」に分ける（`=` がなければ空白）。
fn split_pair(line: &str) -> Option<(&str, &str)> {
    line.split_once('=')
        .or_else(|| line.split_once(char::is_whitespace))
        .map(|(a, b)| (a.trim(), b.trim()))
}

/// 規則の一覧（1 行に 1 つ: `*.corp.example.jp = 10.0.0.1:8080`）を読む。
pub fn parse_rules(text: &str) -> Result<Vec<ProxyRule>, String> {
    let mut v = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = strip_comment(raw);
        if line.is_empty() {
            continue;
        }
        let (pattern, proxy) = split_pair(line).ok_or_else(|| {
            format!(
                "{} 行目「{line}」が読めません（例: *.example.jp = 10.0.0.1:8080）",
                i + 1
            )
        })?;
        let r = ProxyRule {
            pattern: pattern.to_owned(),
            proxy: proxy.to_owned(),
        };
        r.validate().map_err(|e| format!("{} 行目: {e}", i + 1))?;
        v.push(r);
    }
    Ok(v)
}

/// 規則の一覧を書く（[`parse_rules`] の逆）。
pub fn format_rules(rules: &[ProxyRule]) -> String {
    rules
        .iter()
        .map(|r| format!("{} = {}", r.pattern, r.proxy))
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// 転送の一覧（1 行に 1 つ: `www.example.com = 127.0.0.1:8443 cert=AB:CD:…`）を読む。
pub fn parse_hosts(text: &str) -> Result<Vec<HostMap>, String> {
    let mut v = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = strip_comment(raw);
        if line.is_empty() {
            continue;
        }
        let (host, rest) = split_pair(line).ok_or_else(|| {
            format!(
                "{} 行目「{line}」が読めません（例: www.example.com = 127.0.0.1:8443）",
                i + 1
            )
        })?;
        let mut words = rest.split_whitespace();
        let address = words.next().unwrap_or_default();
        let mut cert = String::new();
        for w in words {
            match w.split_once('=') {
                Some((k, val)) if k.eq_ignore_ascii_case("cert") => cert = val.to_owned(),
                _ => return Err(format!("{} 行目: 「{w}」が読めません（cert=指紋）", i + 1)),
            }
        }
        let m = HostMap {
            host: host.to_owned(),
            address: address.to_owned(),
            cert_sha256: normalize_fingerprint(&cert).unwrap_or(cert),
        };
        m.validate().map_err(|e| format!("{} 行目: {e}", i + 1))?;
        v.push(m);
    }
    Ok(v)
}

/// 転送の一覧を書く（[`parse_hosts`] の逆）。
pub fn format_hosts(hosts: &[HostMap]) -> String {
    hosts
        .iter()
        .map(|m| {
            let mut s = format!("{} = {}", m.host, m.address);
            if !m.cert_sha256.is_empty() {
                s.push_str(&format!(" cert={}", m.cert_sha256));
            }
            s
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// Chromium の `--host-resolver-rules` の値（なければ `None`）。
pub fn host_resolver_rules(hosts: &[HostMap]) -> Option<String> {
    (!hosts.is_empty()).then(|| {
        hosts
            .iter()
            .map(HostMap::map_rule)
            .collect::<Vec<_>>()
            .join(",")
    })
}

/// プロキシ 1 つ（`[scheme://]host:port`）→ PAC の返す値（`PROXY h:p` など）。
pub(crate) fn pac_token(spec: &str) -> String {
    let spec = spec.trim();
    if spec.eq_ignore_ascii_case("direct") {
        return "DIRECT".into();
    }
    let (scheme, hp) = match spec.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => ("http".to_owned(), spec),
    };
    let kind = match scheme.as_str() {
        "https" => "HTTPS",
        "socks" | "socks4" => "SOCKS",
        "socks5" => "SOCKS5",
        _ => "PROXY",
    };
    format!("{kind} {hp}")
}

/// PAC の条件式（`host` が型に当てはまる）。型は確かめ済み（引用符などは入らない）。
fn pac_condition(pattern: &str) -> String {
    let p = pattern.trim().to_ascii_lowercase();
    if let Some((net, n)) = parse_cidr(&p) {
        let mask = if n == 0 { 0 } else { u32::MAX << (32 - n) };
        let m = mask.to_be_bytes();
        return format!(
            "(isIp(host) && isInNet(host, \"{}.{}.{}.{}\", \"{}.{}.{}.{}\"))",
            net[0], net[1], net[2], net[3], m[0], m[1], m[2], m[3]
        );
    }
    if p.contains('*') {
        format!("shExpMatch(host, \"{p}\")")
    } else {
        format!("(host == \"{p}\" || dnsDomainIs(host, \".{p}\"))")
    }
}

/// どれにも当てはまらないときの返し方。
pub(crate) enum Fallback<'a> {
    Direct,
    /// 指定のプロキシ（`host:port` か `http=h:p;https=h:p`）と、除くホスト
    Manual {
        server: &'a str,
        bypass: &'a [&'a str],
    },
}

/// PAC のスクリプトを作る。転送するホスト → 直接、規則（上から順に）、最後にやり方どおり。
pub(crate) fn pac_script(hosts: &[HostMap], rules: &[ProxyRule], fallback: Fallback) -> String {
    let mut s = String::from(
        "// yybrowser が作った自動構成のスクリプト\n\
         function isIp(h) { return /^\\d+\\.\\d+\\.\\d+\\.\\d+$/.test(h); }\n\
         function FindProxyForURL(url, host) {\n  host = host.toLowerCase();\n",
    );
    for m in hosts {
        if let Ok((h, _)) = split_host_port(m.host.trim()) {
            s.push_str(&format!(
                "  if ({}) return \"DIRECT\";\n",
                pac_condition_exact(h)
            ));
        }
    }
    for r in rules {
        s.push_str(&format!(
            "  if ({}) return \"{}\";\n",
            pac_condition(&r.pattern),
            pac_token(&r.proxy)
        ));
    }
    match fallback {
        Fallback::Direct => s.push_str("  return \"DIRECT\";\n"),
        Fallback::Manual { server, bypass } => {
            for b in bypass {
                if *b == "<local>" {
                    s.push_str("  if (isPlainHostName(host)) return \"DIRECT\";\n");
                } else if check_pattern(b).is_ok() {
                    let b = b
                        .strip_prefix('.')
                        .map_or(b.to_string(), |d| format!("*.{d}"));
                    s.push_str(&format!(
                        "  if ({}) return \"DIRECT\";\n",
                        pac_condition(&b)
                    ));
                }
            }
            let server = server.trim();
            if server.contains('=') {
                // スキームごと（socks= はほかに当てはまらないときに使う）
                let mut other = "DIRECT".to_owned();
                for part in server.split(';').map(str::trim).filter(|p| !p.is_empty()) {
                    if let Some((k, v)) = part.split_once('=') {
                        let tok = pac_token(v);
                        match k.trim() {
                            "socks" => {
                                other = if v.contains("://") {
                                    tok
                                } else {
                                    format!("SOCKS {}", v.trim())
                                }
                            }
                            scheme => s.push_str(&format!(
                                "  if (url.substring(0, {}) == \"{scheme}:\") return \"{tok}\";\n",
                                scheme.len() + 1
                            )),
                        }
                    }
                }
                s.push_str(&format!("  return \"{other}\";\n"));
            } else {
                s.push_str(&format!("  return \"{}\";\n", pac_token(server)));
            }
        }
    }
    s.push_str("}\n");
    s
}

/// 転送するホストの条件（`*` があれば型、なければそのホストだけ）。
fn pac_condition_exact(h: &str) -> String {
    let h = h.to_ascii_lowercase();
    if h.contains('*') {
        format!("shExpMatch(host, \"{h}\")")
    } else {
        format!("host == \"{h}\"")
    }
}

/// PAC を `data:` の URL にする。
pub(crate) fn pac_data_url(script: &str) -> String {
    format!(
        "data:application/x-ns-proxy-autoconfig;base64,{}",
        base64_encode(script.as_bytes())
    )
}

/// URL のスキーム・ホスト（小文字）・ポート（省略なら既定のポート）。http・https 以外は `None`。
pub fn url_host_port(url: &str) -> Option<(String, String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let default = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let auth = rest.split(['/', '?', '#']).next()?;
    let auth = auth.rsplit_once('@').map_or(auth, |(_, a)| a);
    let (host, port) = if let Some(v6) = auth.strip_prefix('[') {
        let (h, r) = v6.split_once(']')?;
        (format!("[{h}]"), r.strip_prefix(':'))
    } else {
        match auth.rsplit_once(':') {
            Some((h, p)) => (h.to_owned(), Some(p)),
            None => (auth.to_owned(), None),
        }
    };
    let port = match port {
        Some(p) if !p.is_empty() => p.parse().ok()?,
        _ => default,
    };
    (!host.is_empty()).then(|| (scheme, host.to_ascii_lowercase(), port))
}

/// フィルタリストなどをダウンロードする経路（20 章 3.2。WinINet で使う）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// 直接
    Direct,
    /// WinINet のプロキシの指定（`host:port`・`http=h:p https=h:p`・`socks=h:p`）
    Proxy(String),
    /// OS と同じ（インターネット オプション）
    System,
    /// WinINet では使えないプロキシなので OS と同じにする（理由）
    Unsupported(String),
}

/// プロキシ 1 つ（`[scheme://]host:port`）を WinINet の書き方に。
fn wininet_one(spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    let (scheme, hp) = match spec.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => ("http".to_owned(), spec),
    };
    match scheme.as_str() {
        "http" => Ok(hp.to_owned()),
        "socks" | "socks4" => Ok(format!("socks={hp}")),
        _ => Err(format!(
            "ダウンロードでは {scheme} のプロキシ（{spec}）を使えないので、OS と同じ経路にします"
        )),
    }
}

/// URL をダウンロードする経路。プロファイルの規則が当てはまればそれ、なければやり方に従う。
pub fn download_route(profile: &crate::ProxyProfile, url: &str) -> Route {
    use crate::ProxyMode;
    let host = url_host_port(url).map(|(_, h, _)| h).unwrap_or_default();
    let one = |spec: &str| -> Route {
        if spec.trim().eq_ignore_ascii_case("direct") {
            return Route::Direct;
        }
        match wininet_one(spec) {
            Ok(p) => Route::Proxy(p),
            Err(e) => Route::Unsupported(e),
        }
    };
    if let Some(r) = profile.rules.iter().find(|r| r.matches(&host)) {
        return one(&r.proxy);
    }
    match profile.mode {
        ProxyMode::Direct => Route::Direct,
        ProxyMode::System | ProxyMode::Pac => Route::System,
        ProxyMode::Manual => {
            let bypassed = profile
                .bypass
                .split([';', ',', '\n'])
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .any(|b| {
                    if b == "<local>" {
                        !host.contains('.')
                    } else {
                        let b = b
                            .strip_prefix('.')
                            .map_or(b.to_string(), |d| format!("*.{d}"));
                        ProxyRule {
                            pattern: b,
                            proxy: "direct".into(),
                        }
                        .matches(&host)
                    }
                });
            if bypassed {
                return Route::Direct;
            }
            let server = profile.server.trim();
            if !server.contains('=') {
                return one(server);
            }
            // スキームごと: WinINet は「http=h:p https=h:p socks=h:p」（空白区切り）
            let mut parts = Vec::new();
            for part in server.split(';').map(str::trim).filter(|p| !p.is_empty()) {
                let Some((k, v)) = part.split_once('=') else {
                    continue;
                };
                let k = k.trim();
                match wininet_one(v) {
                    Ok(p) if k == "socks" => {
                        parts.push(format!("socks={}", p.trim_start_matches("socks=")))
                    }
                    Ok(p) if !p.starts_with("socks=") => parts.push(format!("{k}={p}")),
                    Ok(_) | Err(_) => {
                        return Route::Unsupported(format!(
                            "ダウンロードでは「{part}」を使えないので、OS と同じ経路にします"
                        ));
                    }
                }
            }
            Route::Proxy(parts.join(" "))
        }
    }
}

/// 開発者用証明書（PEM）。
#[derive(Clone, Debug)]
pub struct DevCert {
    pub cert_pem: String,
    pub key_pem: String,
    /// 証明書の SHA-256 の指紋（`AB:CD:…`）
    pub sha256: String,
}

/// 証明書のファイルの名前に使う、ホストから作った名前（`*` → `_wildcard`）。
pub fn cert_file_stem(host: &str) -> String {
    let h = split_host_port(host.trim()).map_or(host, |(h, _)| h);
    h.replace('*', "_wildcard")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 開発者用証明書を作る（自己署名。ホスト名を別名に入れ、サーバーの認証に使う）。
#[cfg(feature = "devcert")]
pub fn generate_dev_cert(host: &str) -> Result<DevCert, String> {
    use rcgen::{CertificateParams, DnType, ExtendedKeyUsagePurpose, KeyPair};
    let (h, _) = split_host_port(host.trim())?;
    let key = KeyPair::generate().map_err(|e| e.to_string())?;
    let mut p = CertificateParams::new(vec![h.to_owned()]).map_err(|e| e.to_string())?;
    p.distinguished_name.push(DnType::CommonName, h);
    p.distinguished_name
        .push(DnType::OrganizationName, "yybrowser developer certificate");
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    // 有効期間はおよそ前年から 2 年後まで（長すぎると嫌うサーバー・ツールがある）
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let year = 1970 + (secs / 31_556_952) as i32;
    p.not_before = rcgen::date_time_ymd(year - 1, 1, 1);
    p.not_after = rcgen::date_time_ymd(year + 2, 1, 1);
    let cert = p.self_signed(&key).map_err(|e| e.to_string())?;
    Ok(DevCert {
        sha256: fingerprint(cert.der()),
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    })
}

/// 開発者用証明書を作ってフォルダに書く（`<ホスト>.crt`・`<ホスト>.key`）。書いた 2 つのパスを返す。
pub fn write_dev_cert(
    dir: &std::path::Path,
    host: &str,
    cert: &DevCert,
) -> std::io::Result<(std::path::PathBuf, std::path::PathBuf)> {
    std::fs::create_dir_all(dir)?;
    let stem = cert_file_stem(host);
    let crt = dir.join(format!("{stem}.crt"));
    let key = dir.join(format!("{stem}.key"));
    std::fs::write(&crt, &cert.cert_pem)?;
    std::fs::write(&key, &cert.key_pem)?;
    Ok((crt, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_rules() {
        let rules = parse_rules(
            "# 社内\n*.corp.example.jp = 10.0.0.1:8080\nexample.org direct\n\n10.0.0.0/8 = socks5://127.0.0.1:1080 # 範囲\n",
        )
        .unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[1].pattern, "example.org");
        assert_eq!(rules[1].proxy, "direct");
        assert_eq!(parse_rules(&format_rules(&rules)).unwrap(), rules);
        assert!(rules[0].matches("a.corp.example.jp"));
        assert!(!rules[0].matches("corp.example.jp"));
        assert!(rules[1].matches("example.org"));
        assert!(rules[1].matches("www.Example.org"));
        assert!(!rules[1].matches("badexample.org"));
        assert!(rules[2].matches("10.1.2.3"));
        assert!(!rules[2].matches("11.1.2.3"));
        for bad in [
            "a.example = ",
            "a b c",
            "a\"b = direct",
            "x = 1.2.3.4",
            "10.0.0.0/40 = direct",
            "x = ftp://h:1",
        ] {
            assert!(parse_rules(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parses_and_formats_hosts() {
        let fp = "ab".repeat(32);
        let hosts = parse_hosts(&format!(
            "www.example.com = 127.0.0.1:8443 cert={fp}\napi.example.com:443 = [::1]:9443\n*.dev.example = 127.0.0.1\n"
        ))
        .unwrap();
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0].cert_sha256, normalize_fingerprint(&fp).unwrap());
        assert!(hosts[0].cert_sha256.starts_with("AB:AB:"));
        assert_eq!(parse_hosts(&format_hosts(&hosts)).unwrap(), hosts);
        assert!(hosts[0].matches("WWW.example.com", 443));
        assert!(hosts[0].matches("www.example.com", 80));
        assert!(hosts[1].matches("api.example.com", 443));
        assert!(!hosts[1].matches("api.example.com", 80));
        assert!(hosts[2].matches("a.dev.example", 443));
        assert!(!hosts[2].matches("dev.example", 443));
        assert_eq!(
            host_resolver_rules(&hosts).unwrap(),
            "MAP www.example.com 127.0.0.1:8443,MAP api.example.com:443 [::1]:9443,MAP *.dev.example 127.0.0.1"
        );
        for bad in [
            "www.example.com",
            "www.example.com = ",
            "www.example.com = 127.0.0.1:0",
            "www.example.com = *.x:1",
            "www.example.com = 127.0.0.1:8443 cert=xyz",
            "www.example.com = 127.0.0.1:8443 extra",
            "www.example.com:99999 = 127.0.0.1",
            "a\"b = 127.0.0.1",
            "h = [::1",
        ] {
            assert!(parse_hosts(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn globs_and_fingerprints() {
        assert!(glob_match("*.example.com", "a.b.example.com"));
        assert!(!glob_match("*.example.com", "example.com"));
        assert!(glob_match("a*c*e", "abcde"));
        assert!(!glob_match("a*c*e", "abcd"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("ab*ba", "aba"));
        assert_eq!(
            normalize_fingerprint(&"0f".repeat(32)).unwrap(),
            vec!["0F"; 32].join(":")
        );
        assert!(normalize_fingerprint("00:11").is_none());
        // PEM と DER の指紋が同じ
        let der = [0x30u8, 0x03, 0x02, 0x01, 0x05];
        let pem = format!(
            "junk\n-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            base64_encode(&der)
        );
        assert_eq!(cert_fingerprint(pem.as_bytes()), Some(fingerprint(&der)));
        assert_eq!(cert_fingerprint(&der), Some(fingerprint(&der)));
        assert_eq!(cert_fingerprint(b"hello"), None);
        for n in 0..10usize {
            let data: Vec<u8> = (0..n as u8).map(|b| b.wrapping_mul(37)).collect();
            assert_eq!(base64_decode(&base64_encode(&data)).unwrap(), data);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(cert_file_stem("*.dev.example:443"), "_wildcard.dev.example");
    }

    #[test]
    fn chooses_download_routes() {
        use crate::{ProxyMode, ProxyProfile};
        let url = "https://easylist.to/easylist/easylist.txt";
        assert_eq!(
            download_route(&ProxyProfile::new("d", ProxyMode::Direct), url),
            Route::Direct
        );
        assert_eq!(
            download_route(&ProxyProfile::new("s", ProxyMode::System), url),
            Route::System
        );
        let mut m = ProxyProfile::new("m", ProxyMode::Manual);
        m.server = "127.0.0.1:8888".into();
        assert_eq!(
            download_route(&m, url),
            Route::Proxy("127.0.0.1:8888".into())
        );
        m.bypass = "<local>;easylist.to".into();
        assert_eq!(download_route(&m, url), Route::Direct);
        m.bypass.clear();
        m.server = "socks5://127.0.0.1:1080".into();
        assert!(matches!(download_route(&m, url), Route::Unsupported(_)));
        m.server = "socks4://127.0.0.1:1080".into();
        assert_eq!(
            download_route(&m, url),
            Route::Proxy("socks=127.0.0.1:1080".into())
        );
        m.server = "http=p:80;https=p:443;socks=s:1080".into();
        assert_eq!(
            download_route(&m, url),
            Route::Proxy("http=p:80 https=p:443 socks=s:1080".into())
        );
        // 規則が先
        m.rules = parse_rules("easylist.to = direct\nadtidy.org = 10.0.0.1:8080").unwrap();
        assert_eq!(download_route(&m, url), Route::Direct);
        assert_eq!(
            download_route(&m, "https://filters.adtidy.org/x.txt"),
            Route::Proxy("10.0.0.1:8080".into())
        );
    }

    #[test]
    fn reads_hosts_from_urls() {
        let t = |u: &str| url_host_port(u).map(|(s, h, p)| format!("{s} {h} {p}"));
        assert_eq!(
            t("https://WWW.example.com/a?b").as_deref(),
            Some("https www.example.com 443")
        );
        assert_eq!(t("http://user:pw@h:8080").as_deref(), Some("http h 8080"));
        assert_eq!(
            t("https://[::1]:9443/").as_deref(),
            Some("https [::1] 9443")
        );
        assert_eq!(t("http://h:/x").as_deref(), Some("http h 80"));
        assert_eq!(t("about:blank"), None);
        assert_eq!(t("file:///c:/x"), None);
        assert_eq!(t("https://h:99999/"), None);
    }

    #[test]
    fn builds_pac_scripts() {
        let hosts = parse_hosts("www.example.com = 127.0.0.1:8443").unwrap();
        let rules = parse_rules(
            "*.corp.example = 10.0.0.1:8080\nexample.org = direct\n10.0.0.0/8 = socks5://s:1080",
        )
        .unwrap();
        let s = pac_script(
            &hosts,
            &rules,
            Fallback::Manual {
                server: "http=p:80;https=p:443;socks=s:1080",
                bypass: &["<local>", ".internal"],
            },
        );
        let expect = [
            "if (host == \"www.example.com\") return \"DIRECT\";",
            "if (shExpMatch(host, \"*.corp.example\")) return \"PROXY 10.0.0.1:8080\";",
            "if ((host == \"example.org\" || dnsDomainIs(host, \".example.org\"))) return \"DIRECT\";",
            "isInNet(host, \"10.0.0.0\", \"255.0.0.0\"))) return \"SOCKS5 s:1080\";",
            "if (isPlainHostName(host)) return \"DIRECT\";",
            "if (shExpMatch(host, \"*.internal\")) return \"DIRECT\";",
            "if (url.substring(0, 5) == \"http:\") return \"PROXY p:80\";",
            "if (url.substring(0, 6) == \"https:\") return \"PROXY p:443\";",
            "return \"SOCKS s:1080\";",
        ];
        let mut at = 0;
        for e in expect {
            let i = s[at..].find(e).unwrap_or_else(|| panic!("{e}\n{s}")) + at;
            at = i + e.len();
        }
        let d = pac_script(&[], &rules, Fallback::Direct);
        assert!(d.trim_end().ends_with("return \"DIRECT\";\n}"), "{d}");
        assert!(pac_data_url("x").starts_with("data:application/x-ns-proxy-autoconfig;base64,"));
        assert_eq!(pac_token("https://h:1"), "HTTPS h:1");
        assert_eq!(pac_token("socks4://h:1"), "SOCKS h:1");
    }

    #[cfg(feature = "devcert")]
    #[test]
    fn generates_dev_certificates() {
        let c = generate_dev_cert("www.example.com:443").unwrap();
        assert!(c.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(c.key_pem.contains("PRIVATE KEY"));
        assert_eq!(
            cert_fingerprint(c.cert_pem.as_bytes()),
            Some(c.sha256.clone())
        );
        let dir = tempfile::tempdir().unwrap();
        let (crt, key) = write_dev_cert(dir.path(), "www.example.com:443", &c).unwrap();
        assert!(crt.ends_with("www.example.com.crt"));
        assert_eq!(std::fs::read_to_string(key).unwrap(), c.key_pem);
        assert!(generate_dev_cert("bad host").is_err());
    }
}

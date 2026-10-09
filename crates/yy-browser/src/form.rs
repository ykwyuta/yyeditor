//! プロキシの設定の画面の部品（19 章 3.2）。プルダウンで選んだもの（種類・ホスト・ポート）と、保存する
//! 文字列（`socks5://127.0.0.1:1080` など）を行き来する。OS に依存しないので Linux で試験する。

use crate::rules::{HostMap, ProxyRule, split_host_port};

/// プロキシの種類（プルダウン）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyKind {
    Http,
    Https,
    Socks5,
    Socks4,
}

impl ProxyKind {
    pub const ALL: [ProxyKind; 4] = [
        ProxyKind::Http,
        ProxyKind::Https,
        ProxyKind::Socks5,
        ProxyKind::Socks4,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ProxyKind::Http => "HTTP",
            ProxyKind::Https => "HTTPS",
            ProxyKind::Socks5 => "SOCKS5",
            ProxyKind::Socks4 => "SOCKS4",
        }
    }

    fn scheme(self) -> &'static str {
        match self {
            ProxyKind::Http => "http",
            ProxyKind::Https => "https",
            ProxyKind::Socks5 => "socks5",
            ProxyKind::Socks4 => "socks4",
        }
    }

    /// よく使うポート（ポートの欄が空のとき）。
    pub fn default_port(self) -> u16 {
        match self {
            ProxyKind::Http => 8080,
            ProxyKind::Https => 443,
            ProxyKind::Socks5 | ProxyKind::Socks4 => 1080,
        }
    }
}

/// プロキシ 1 つ（種類・ホスト・ポート）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    /// 保存する書き方（HTTP は `host:port`、ほかは `scheme://host:port`。IPv6 は `[ ]` で囲む）。
    pub fn to_spec(&self) -> String {
        let host = self.host.trim();
        let host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_owned()
        };
        match self.kind {
            ProxyKind::Http => format!("{host}:{}", self.port),
            k => format!("{}://{host}:{}", k.scheme(), self.port),
        }
    }

    /// 保存してある書き方を読む（スキームごとの指定 `http=…;https=…` などは `None`）。
    pub fn parse(spec: &str) -> Option<Endpoint> {
        let spec = spec.trim();
        if spec.contains('=') || spec.contains(';') {
            return None;
        }
        let (scheme, rest) = match spec.split_once("://") {
            Some((s, r)) => (s.to_ascii_lowercase(), r),
            None => ("http".to_owned(), spec),
        };
        let kind = match scheme.as_str() {
            "http" => ProxyKind::Http,
            "https" => ProxyKind::Https,
            "socks5" => ProxyKind::Socks5,
            "socks" | "socks4" => ProxyKind::Socks4,
            _ => return None,
        };
        let (host, port) = split_address(rest)?;
        Some(Endpoint {
            kind,
            host,
            port: port?,
        })
    }
}

/// `host:port`・`[v6]:port`・`host` を（ホスト, ポート）に。IPv6 の `[ ]` は外す。
pub fn split_address(s: &str) -> Option<(String, Option<u16>)> {
    let s = s.trim();
    if let Some(v6) = s.strip_prefix('[') {
        let (h, rest) = v6.split_once(']')?;
        let port = match rest.strip_prefix(':') {
            Some(p) => Some(p.parse::<u16>().ok().filter(|n| *n > 0)?),
            None if rest.is_empty() => None,
            None => return None,
        };
        return Some((h.to_owned(), port));
    }
    match s.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => Some((
            h.to_owned(),
            Some(p.parse::<u16>().ok().filter(|n| *n > 0)?),
        )),
        Some(_) => Some((s.to_owned(), None)), // 括弧のない IPv6
        None if s.is_empty() => None,
        None => Some((s.to_owned(), None)),
    }
}

/// 住所（ホストとポート）を書く（IPv6 は `[ ]` で囲む）。
pub fn join_address(host: &str, port: Option<u16>) -> String {
    let host = host.trim();
    let h = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    match port {
        Some(p) => format!("{h}:{p}"),
        None => h,
    }
}

/// ドメインごとのプロキシの対象の選び方（プルダウン）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// `example.com`（そのドメインとサブドメイン）
    DomainAndSubdomains,
    /// `*.example.com`（サブドメインだけ）
    SubdomainsOnly,
    /// `10.0.0.0/8`
    IpRange,
    /// そのほかの `*` を使う型
    Pattern,
}

impl Target {
    pub const ALL: [Target; 4] = [
        Target::DomainAndSubdomains,
        Target::SubdomainsOnly,
        Target::IpRange,
        Target::Pattern,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Target::DomainAndSubdomains => "このドメインとサブドメイン",
            Target::SubdomainsOnly => "サブドメインだけ",
            Target::IpRange => "IP アドレスの範囲",
            Target::Pattern => "* を使う型",
        }
    }

    /// 入力欄の薄い字の例。
    pub fn example(self) -> &'static str {
        match self {
            Target::DomainAndSubdomains | Target::SubdomainsOnly => "例: corp.example.jp",
            Target::IpRange => "例: 10.0.0.0/8",
            Target::Pattern => "例: ads*.example.com",
        }
    }
}

/// 規則の型を（選び方, 入力欄の値）に分ける。
pub fn split_pattern(p: &str) -> (Target, String) {
    let p = p.trim();
    if p.contains('/') {
        return (Target::IpRange, p.to_owned());
    }
    if let Some(d) = p.strip_prefix("*.")
        && !d.contains('*')
    {
        return (Target::SubdomainsOnly, d.to_owned());
    }
    if p.contains('*') {
        return (Target::Pattern, p.to_owned());
    }
    (Target::DomainAndSubdomains, p.to_owned())
}

/// （選び方, 入力欄の値）から規則の型を作る。
pub fn join_pattern(t: Target, value: &str) -> String {
    let v = value
        .trim()
        .trim_start_matches("*.")
        .trim_start_matches('.');
    match t {
        Target::SubdomainsOnly => format!("*.{v}"),
        Target::DomainAndSubdomains => v.to_owned(),
        Target::IpRange | Target::Pattern => value.trim().to_owned(),
    }
}

/// 規則の経路: 直接か、プロキシ。
pub fn rule_route(r: &ProxyRule) -> Option<Endpoint> {
    if r.proxy.trim().eq_ignore_ascii_case("direct") {
        None
    } else {
        Endpoint::parse(&r.proxy)
    }
}

/// 一覧に出す規則の説明（`corp.example.jp とサブドメイン → HTTP 10.0.0.1:8080`）。
pub fn describe_rule(r: &ProxyRule) -> String {
    let (t, v) = split_pattern(&r.pattern);
    let target = match t {
        Target::DomainAndSubdomains => format!("{v} とサブドメイン"),
        Target::SubdomainsOnly => format!("{v} のサブドメイン"),
        Target::IpRange | Target::Pattern => v,
    };
    let route = match rule_route(r) {
        None => "直接".to_owned(),
        Some(e) => format!("{} {}", e.kind.label(), join_address(&e.host, Some(e.port))),
    };
    format!("{target}　→　{route}")
}

/// 転送の対象のポートの選び方（プルダウン）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortChoice {
    All,
    Https,
    Http,
    Other,
}

impl PortChoice {
    pub const ALL: [PortChoice; 4] = [
        PortChoice::All,
        PortChoice::Https,
        PortChoice::Http,
        PortChoice::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PortChoice::All => "すべて",
            PortChoice::Https => "443（https）",
            PortChoice::Http => "80（http）",
            PortChoice::Other => "ほかのポート",
        }
    }

    pub fn of(port: Option<u16>) -> PortChoice {
        match port {
            None => PortChoice::All,
            Some(443) => PortChoice::Https,
            Some(80) => PortChoice::Http,
            Some(_) => PortChoice::Other,
        }
    }

    /// ポート（「ほかのポート」なら `other`）。
    pub fn port(self, other: Option<u16>) -> Option<u16> {
        match self {
            PortChoice::All => None,
            PortChoice::Https => Some(443),
            PortChoice::Http => Some(80),
            PortChoice::Other => other,
        }
    }
}

/// 転送を画面の欄に分ける: (ホスト, 対象のポート, 転送先のホスト, 転送先のポート)。
pub fn split_host_map(m: &HostMap) -> (String, Option<u16>, String, Option<u16>) {
    let (h, p) = split_host_port(m.host.trim())
        .map(|(h, p)| (h.to_owned(), p))
        .unwrap_or_else(|_| (m.host.trim().to_owned(), None));
    let (dh, dp) = split_address(&m.address).unwrap_or_default();
    (h, p, dh, dp)
}

/// 一覧に出す転送の説明（`www.example.com:443　→　127.0.0.1:8443　🔒 開発者用証明書`）。
pub fn describe_host_map(m: &HostMap) -> String {
    let mut s = format!("{}　→　{}", m.host.trim(), m.address.trim());
    if !m.cert_sha256.trim().is_empty() {
        s.push_str("　🔒 開発者用証明書");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_round_trip() {
        for (spec, kind, host, port) in [
            ("127.0.0.1:8888", ProxyKind::Http, "127.0.0.1", 8888),
            ("http://proxy:8080", ProxyKind::Http, "proxy", 8080),
            ("https://p.example:443", ProxyKind::Https, "p.example", 443),
            (
                "socks5://127.0.0.1:1080",
                ProxyKind::Socks5,
                "127.0.0.1",
                1080,
            ),
            ("socks://h:1080", ProxyKind::Socks4, "h", 1080),
            ("[::1]:3128", ProxyKind::Http, "::1", 3128),
        ] {
            let e = Endpoint::parse(spec).unwrap_or_else(|| panic!("{spec}"));
            assert_eq!(
                (e.kind, e.host.as_str(), e.port),
                (kind, host, port),
                "{spec}"
            );
            assert_eq!(Endpoint::parse(&e.to_spec()), Some(e));
        }
        assert_eq!(
            Endpoint {
                kind: ProxyKind::Socks5,
                host: "::1".into(),
                port: 1080
            }
            .to_spec(),
            "socks5://[::1]:1080"
        );
        for bad in ["http=a:1;https=b:2", "host", "ftp://h:21", "h:0", ""] {
            assert!(Endpoint::parse(bad).is_none(), "{bad}");
        }
        // 作ったものは確かめを通る
        let p = crate::ProxyProfile {
            name: "x".into(),
            mode: crate::ProxyMode::Manual,
            server: Endpoint {
                kind: ProxyKind::Socks5,
                host: "127.0.0.1".into(),
                port: 1080,
            }
            .to_spec(),
            ..Default::default()
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn patterns_round_trip() {
        for (p, t, v) in [
            (
                "corp.example.jp",
                Target::DomainAndSubdomains,
                "corp.example.jp",
            ),
            (
                "*.corp.example.jp",
                Target::SubdomainsOnly,
                "corp.example.jp",
            ),
            ("10.0.0.0/8", Target::IpRange, "10.0.0.0/8"),
            ("ads*.example.com", Target::Pattern, "ads*.example.com"),
        ] {
            assert_eq!(split_pattern(p), (t, v.to_owned()), "{p}");
            assert_eq!(join_pattern(t, v), p);
        }
        // 入力に「*.」を付けてしまっても直す
        assert_eq!(join_pattern(Target::DomainAndSubdomains, "*.a.jp"), "a.jp");
        assert_eq!(join_pattern(Target::SubdomainsOnly, ".a.jp"), "*.a.jp");
        let r = ProxyRule {
            pattern: "corp.example.jp".into(),
            proxy: "10.0.0.1:8080".into(),
        };
        assert_eq!(
            describe_rule(&r),
            "corp.example.jp とサブドメイン　→　HTTP 10.0.0.1:8080"
        );
        let d = ProxyRule {
            pattern: "*.x.jp".into(),
            proxy: "direct".into(),
        };
        assert_eq!(describe_rule(&d), "x.jp のサブドメイン　→　直接");
        assert!(rule_route(&d).is_none());
    }

    #[test]
    fn host_maps_split_and_describe() {
        let m = HostMap {
            host: "www.example.com:443".into(),
            address: "127.0.0.1:8443".into(),
            cert_sha256: "AB".into(),
        };
        assert_eq!(
            split_host_map(&m),
            (
                "www.example.com".into(),
                Some(443),
                "127.0.0.1".into(),
                Some(8443)
            )
        );
        assert!(describe_host_map(&m).ends_with("🔒 開発者用証明書"));
        let v6 = HostMap {
            host: "a.example".into(),
            address: "[::1]".into(),
            cert_sha256: String::new(),
        };
        assert_eq!(
            split_host_map(&v6),
            ("a.example".into(), None, "::1".into(), None)
        );
        assert_eq!(join_address("::1", Some(9443)), "[::1]:9443");
        assert_eq!(join_address("127.0.0.1", None), "127.0.0.1");
        assert_eq!(PortChoice::of(Some(443)), PortChoice::Https);
        assert_eq!(PortChoice::of(Some(8443)), PortChoice::Other);
        assert_eq!(PortChoice::Other.port(Some(8443)), Some(8443));
        assert_eq!(PortChoice::All.port(Some(1)), None);
        assert_eq!(split_address("h:0"), None);
        assert_eq!(split_address("h"), Some(("h".into(), None)));
    }
}

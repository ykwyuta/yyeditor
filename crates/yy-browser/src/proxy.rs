//! プロキシの設定（19 章 3）。
//!
//! WebView2 の環境を作るときの Chromium の起動引数（`AdditionalBrowserArguments`）で渡す。OS の設定は変えない。

use serde::{Deserialize, Serialize};

use crate::rules::{self, Fallback, HostMap, ProxyRule};

/// プロキシのやり方。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    /// OS（インターネット オプション）と同じ
    #[default]
    System,
    /// 使わない（直接つなぐ）
    Direct,
    /// 指定したプロキシ
    Manual,
    /// 自動構成（PAC）
    Pac,
}

impl ProxyMode {
    pub const ALL: [ProxyMode; 4] = [
        ProxyMode::System,
        ProxyMode::Direct,
        ProxyMode::Manual,
        ProxyMode::Pac,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ProxyMode::System => "OS と同じ",
            ProxyMode::Direct => "使わない（直接）",
            ProxyMode::Manual => "指定",
            ProxyMode::Pac => "自動構成（PAC）",
        }
    }
}

/// 名前を付けたプロキシの設定（プロファイル）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyProfile {
    pub name: String,
    #[serde(default)]
    pub mode: ProxyMode,
    /// 指定のときのプロキシ（`host:port`・`socks5://host:port`・`http=h:p;https=h:p`）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub server: String,
    /// 除くホスト（`;` 区切り。`<local>`・`*.example.jp`・`192.168.0.0/16`）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bypass: String,
    /// 自動構成のときの PAC の URL
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pac_url: String,
    /// ドメインごとのプロキシ（上から順に。「使わない」「指定」のときだけ。19 章 3.5）
    #[serde(default, rename = "rule", skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<ProxyRule>,
    /// ホストの転送と開発者用証明書（19 章 3.6）
    #[serde(default, rename = "host", skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<HostMap>,
}

/// 指定のプロキシに使えるスキーム。
const SCHEMES: [&str; 5] = ["http", "https", "socks", "socks4", "socks5"];

/// 1 つのプロキシ（`[scheme://]host:port`）を確かめる。
pub(crate) fn check_one(s: &str) -> Result<(), String> {
    let (scheme, rest) = match s.split_once("://") {
        Some((sc, r)) => (sc.to_ascii_lowercase(), r),
        None => ("http".to_owned(), s),
    };
    if !SCHEMES.contains(&scheme.as_str()) {
        return Err(format!(
            "プロキシのスキーム「{scheme}」は使えません（http・https・socks4・socks5）"
        ));
    }
    let (host, port) = match rest.strip_prefix('[') {
        // IPv6: [::1]:8080
        Some(v6) => {
            let (h, p) = v6
                .split_once("]:")
                .ok_or_else(|| format!("「{s}」の IPv6 アドレスとポートが読めません"))?;
            (h, p)
        }
        None => rest
            .rsplit_once(':')
            .ok_or_else(|| format!("「{s}」にポートがありません（例: 127.0.0.1:8080）"))?,
    };
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
    {
        return Err(format!("「{s}」のホスト名が正しくありません"));
    }
    match port.parse::<u32>() {
        Ok(1..=65535) => Ok(()),
        _ => Err(format!("「{s}」のポートは 1〜65535 です")),
    }
}

/// 起動引数に入れてはいけない文字（引用符・改行など）がないか。
fn check_plain(what: &str, s: &str) -> Result<(), String> {
    if s.chars().any(|c| c == '"' || c.is_control()) {
        return Err(format!("{what}に引用符や改行は使えません"));
    }
    Ok(())
}

impl ProxyProfile {
    pub fn new(name: &str, mode: ProxyMode) -> ProxyProfile {
        ProxyProfile {
            name: name.to_owned(),
            mode,
            ..ProxyProfile::default()
        }
    }

    /// 入力を確かめる（保存する前・起動引数を作る前）。
    pub fn validate(&self) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("プロファイルの名前を入力してください".into());
        }
        check_plain("名前", name)?;
        check_plain("プロキシ", &self.server)?;
        check_plain("除くホスト", &self.bypass)?;
        check_plain("PAC の URL", &self.pac_url)?;
        for r in &self.rules {
            r.validate()
                .map_err(|e| format!("ドメインごとのプロキシ「{}」: {e}", r.pattern))?;
        }
        for h in &self.hosts {
            h.validate()
                .map_err(|e| format!("ホストの転送「{}」: {e}", h.host))?;
        }
        if !self.rules.is_empty() && matches!(self.mode, ProxyMode::System | ProxyMode::Pac) {
            return Err(
                "ドメインごとのプロキシは、やり方が「使わない（直接）」か「指定」のときに使えます"
                    .into(),
            );
        }
        match self.mode {
            ProxyMode::System | ProxyMode::Direct => Ok(()),
            ProxyMode::Manual => {
                let s = self.server.trim();
                if s.is_empty() {
                    return Err(
                        "プロキシ（例: 127.0.0.1:8080・socks5://127.0.0.1:1080）を入力してください"
                            .into(),
                    );
                }
                if s.contains('=') {
                    // スキームごと: http=h:p;https=h:p
                    for part in s.split(';').map(str::trim).filter(|p| !p.is_empty()) {
                        let (k, v) = part.split_once('=').ok_or_else(|| {
                            format!("「{part}」が読めません（例: http=h:p;https=h:p）")
                        })?;
                        if !matches!(k.trim(), "http" | "https" | "ftp" | "socks") {
                            return Err(format!("「{k}」は使えません（http・https・ftp・socks）"));
                        }
                        check_one(v.trim())?;
                    }
                    Ok(())
                } else {
                    check_one(s)
                }
            }
            ProxyMode::Pac => {
                let u = self.pac_url.trim();
                let lower = u.to_ascii_lowercase();
                if !(lower.starts_with("http://")
                    || lower.starts_with("https://")
                    || lower.starts_with("file:///"))
                {
                    return Err("PAC の URL は http://・https://・file:/// で始めてください".into());
                }
                Ok(())
            }
        }
    }

    /// Chromium の起動引数（`AdditionalBrowserArguments`）。OS と同じで転送もなければ空。
    ///
    /// ドメインごとのプロキシがあれば PAC を作って `data:` の URL で渡す。転送するホストは
    /// `--host-resolver-rules` で差し替え、プロキシを通さない（PAC では直接、指定では除くホスト）。
    pub fn browser_args(&self) -> Result<String, String> {
        self.validate()?;
        let mut bypass: Vec<String> = self
            .bypass
            .split([';', ',', '\n'])
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(str::to_owned)
            .collect();
        let mut args = Vec::new();
        if !self.rules.is_empty() {
            let refs: Vec<&str> = bypass.iter().map(String::as_str).collect();
            let fallback = match self.mode {
                ProxyMode::Manual => Fallback::Manual {
                    server: &self.server,
                    bypass: &refs,
                },
                _ => Fallback::Direct,
            };
            let script = rules::pac_script(&self.hosts, &self.rules, fallback);
            args.push(format!(
                "--proxy-pac-url=\"{}\"",
                rules::pac_data_url(&script)
            ));
        } else {
            match self.mode {
                ProxyMode::System => {}
                ProxyMode::Direct => args.push("--no-proxy-server".into()),
                ProxyMode::Manual => {
                    for h in &self.hosts {
                        if let Ok((host, _)) = rules::split_host_port(h.host.trim())
                            && !bypass.iter().any(|b| b == host)
                        {
                            bypass.push(host.to_owned());
                        }
                    }
                    args.push(format!("--proxy-server=\"{}\"", self.server.trim()));
                    if !bypass.is_empty() {
                        args.push(format!("--proxy-bypass-list=\"{}\"", bypass.join(";")));
                    }
                }
                ProxyMode::Pac => args.push(format!("--proxy-pac-url=\"{}\"", self.pac_url.trim())),
            }
        }
        if let Some(map) = rules::host_resolver_rules(&self.hosts) {
            args.push(format!("--host-resolver-rules=\"{map}\""));
        }
        Ok(args.join(" "))
    }

    /// ホスト（とポート）の転送（あれば）。
    pub fn host_map(&self, host: &str, port: u16) -> Option<&HostMap> {
        self.hosts.iter().find(|m| m.matches(host, port))
    }

    /// 状態表示に出す説明（`検証用（127.0.0.1:8888）`）。
    pub fn describe(&self) -> String {
        let detail = match self.mode {
            ProxyMode::System => "OS と同じ".to_owned(),
            ProxyMode::Direct => "直接".to_owned(),
            ProxyMode::Manual => self.server.trim().to_owned(),
            ProxyMode::Pac => format!("PAC {}", self.pac_url.trim()),
        };
        let mut extra = Vec::new();
        if !self.rules.is_empty() {
            extra.push(format!("規則 {}", self.rules.len()));
        }
        if !self.hosts.is_empty() {
            extra.push(format!("転送 {}", self.hosts.len()));
        }
        let detail = if extra.is_empty() {
            detail
        } else {
            format!("{detail}・{}", extra.join("・"))
        };
        if detail == self.name.trim() {
            detail
        } else {
            format!("{}（{detail}）", self.name.trim())
        }
    }

    /// データのフォルダの名前（プロファイルごとに分ける。19 章 3.3）。使えない文字を除き、名前の
    /// 大文字・小文字の違いでフォルダが重ならないよう、名前から作った短い印を付ける。
    pub fn data_folder_name(&self) -> String {
        folder_name(&self.name)
    }
}

/// 名前からフォルダの名前を作る。
pub fn folder_name(name: &str) -> String {
    let readable: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(24)
        .collect();
    // FNV-1a（名前そのもの。大文字・小文字も区別する）
    let mut h: u32 = 0x811c_9dc5;
    for b in name.trim().as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{}-{h:08x}", readable.trim_matches('_'))
}

/// コマンドラインの `--proxy <URL>`（その場限りのプロファイル）。
pub fn adhoc(server: &str) -> Result<ProxyProfile, String> {
    let p = match server.trim() {
        "direct" | "none" | "直接" => {
            ProxyProfile::new("直接（コマンドライン）", ProxyMode::Direct)
        }
        "system" | "os" => ProxyProfile::new("OS と同じ（コマンドライン）", ProxyMode::System),
        s if s.to_ascii_lowercase().ends_with(".pac") => ProxyProfile {
            name: format!("PAC {s}"),
            mode: ProxyMode::Pac,
            pac_url: s.to_owned(),
            ..ProxyProfile::default()
        },
        s => ProxyProfile {
            name: s.to_owned(),
            mode: ProxyMode::Manual,
            server: s.to_owned(),
            ..ProxyProfile::default()
        },
    };
    p.validate()?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manual(server: &str, bypass: &str) -> ProxyProfile {
        ProxyProfile {
            name: "p".into(),
            mode: ProxyMode::Manual,
            server: server.into(),
            bypass: bypass.into(),
            ..ProxyProfile::default()
        }
    }

    #[test]
    fn builds_chromium_switches() {
        assert_eq!(
            ProxyProfile::new("os", ProxyMode::System)
                .browser_args()
                .unwrap(),
            ""
        );
        assert_eq!(
            ProxyProfile::new("d", ProxyMode::Direct)
                .browser_args()
                .unwrap(),
            "--no-proxy-server"
        );
        assert_eq!(
            manual("127.0.0.1:8888", "<local>; *.example.co.jp ,192.168.0.0/16")
                .browser_args()
                .unwrap(),
            "--proxy-server=\"127.0.0.1:8888\" --proxy-bypass-list=\"<local>;*.example.co.jp;192.168.0.0/16\""
        );
        assert_eq!(
            manual("socks5://127.0.0.1:1080", "")
                .browser_args()
                .unwrap(),
            "--proxy-server=\"socks5://127.0.0.1:1080\""
        );
        assert!(
            manual("http=proxy.local:8080;https=proxy.local:8443", "")
                .browser_args()
                .is_ok()
        );
        assert!(manual("[::1]:3128", "").browser_args().is_ok());
        let pac = ProxyProfile {
            name: "pac".into(),
            mode: ProxyMode::Pac,
            pac_url: "http://wpad.local/proxy.pac".into(),
            ..ProxyProfile::default()
        };
        assert_eq!(
            pac.browser_args().unwrap(),
            "--proxy-pac-url=\"http://wpad.local/proxy.pac\""
        );
    }

    #[test]
    fn rules_and_host_maps_become_pac_and_resolver_rules() {
        let hosts = crate::rules::parse_hosts("www.example.com = 127.0.0.1:8443").unwrap();
        // 転送だけ: 指定なら除くホストに足す・直接ならそのまま・OS と同じでも転送は効く
        let mut p = manual("127.0.0.1:8888", "<local>");
        p.hosts = hosts.clone();
        assert_eq!(
            p.browser_args().unwrap(),
            "--proxy-server=\"127.0.0.1:8888\" --proxy-bypass-list=\"<local>;www.example.com\" \
             --host-resolver-rules=\"MAP www.example.com 127.0.0.1:8443\""
        );
        let mut os = ProxyProfile::new("os", ProxyMode::System);
        os.hosts = hosts.clone();
        assert_eq!(
            os.browser_args().unwrap(),
            "--host-resolver-rules=\"MAP www.example.com 127.0.0.1:8443\""
        );
        assert!(os.host_map("www.example.com", 443).is_some());
        assert!(os.host_map("example.com", 443).is_none());
        // 規則があれば PAC（data: の URL）
        p.rules = crate::rules::parse_rules("*.corp.example = 10.0.0.1:8080").unwrap();
        let a = p.browser_args().unwrap();
        assert!(
            a.starts_with("--proxy-pac-url=\"data:application/x-ns-proxy-autoconfig;base64,"),
            "{a}"
        );
        assert!(!a.contains("--proxy-server"), "{a}");
        assert!(a.ends_with("--host-resolver-rules=\"MAP www.example.com 127.0.0.1:8443\""));
        assert!(p.describe().contains("規則 1・転送 1"), "{}", p.describe());
        // 規則は「OS と同じ」「PAC」とは組み合わせられない
        os.rules = p.rules.clone();
        assert!(os.validate().is_err());
        // TOML に書いて読める
        let text = toml::to_string(&p).unwrap();
        assert!(
            text.contains("[[rule]]") && text.contains("[[host]]"),
            "{text}"
        );
        assert_eq!(toml::from_str::<ProxyProfile>(&text).unwrap(), p);
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "",
            "127.0.0.1",
            "127.0.0.1:0",
            "127.0.0.1:70000",
            "ftp://host:21",
            "host name:80",
            "host:80\" --disable-web-security \"",
            "http=host:80;gopher=h:1",
            "[::1:80",
        ] {
            assert!(manual(bad, "").validate().is_err(), "{bad}");
        }
        assert!(manual("h:80", "a\nb").validate().is_err());
        assert!(
            ProxyProfile::new("  ", ProxyMode::Direct)
                .validate()
                .is_err()
        );
        let pac = ProxyProfile {
            name: "pac".into(),
            mode: ProxyMode::Pac,
            pac_url: "javascript:alert(1)".into(),
            ..ProxyProfile::default()
        };
        assert!(pac.validate().is_err());
    }

    #[test]
    fn names_folders_and_parses_command_line() {
        let a = folder_name("検証用 Fiddler");
        assert!(a.starts_with("検証用_Fiddler-"), "{a}");
        assert_ne!(folder_name("Proxy"), folder_name("proxy"));
        assert_eq!(folder_name("x"), folder_name(" x "));
        assert!(!folder_name("a/b\\c:*?").contains(['/', '\\', ':', '*', '?']));
        assert!(folder_name(&"長".repeat(100)).chars().count() <= 24 + 9);
        assert_eq!(adhoc("direct").unwrap().mode, ProxyMode::Direct);
        assert_eq!(
            adhoc("socks5://127.0.0.1:1080").unwrap().mode,
            ProxyMode::Manual
        );
        assert_eq!(adhoc("http://wpad/proxy.pac").unwrap().mode, ProxyMode::Pac);
        assert!(adhoc("nonsense").is_err());
        assert_eq!(
            manual("127.0.0.1:8888", "").describe(),
            "p（127.0.0.1:8888）"
        );
    }
}

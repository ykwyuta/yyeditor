//! ポートフォワーディング（ターミナルだけが使う。12 章 4.3）。
//!
//! OpenSSH と同じ 3 種類: 手元のポートに来た接続を接続先から転送する（`-L`・`LocalForward`）、
//! 接続先のポートに来た接続を手元から転送する（`-R`・`RemoteForward`）、手元のポートを SOCKS の
//! プロキシにする（`-D`・`DynamicForward`）。指定は `~/.ssh/config`・yyeditor の接続設定・
//! ターミナルの「SSH で接続」の入力から集める（[`crate::HostSpec::forwards`]）。
//!
//! 始めるのは [`crate::Transport::forward`]。エディタとファイル転送は呼ばない（無視する）。
//! 始められなかったフォワーディングは警告にするだけで、接続はそのまま使う。

use std::fmt;
use std::sync::Arc;

/// ポートフォワーディングの 1 つ。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Forward {
    /// 手元の `bind:port` に来た接続を、接続先から `host:host_port` へ（`-L`）
    Local {
        bind: Option<String>,
        port: u16,
        host: String,
        host_port: u16,
    },
    /// 接続先の `bind:port` に来た接続を、手元から `host:host_port` へ（`-R`）。
    /// `port` が 0 なら接続先が空いているポートを選ぶ
    Remote {
        bind: Option<String>,
        port: u16,
        host: String,
        host_port: u16,
    },
    /// 手元の `bind:port` を SOCKS（4・4a・5）のプロキシにして、接続先から接続する（`-D`）
    Dynamic { bind: Option<String>, port: u16 },
}

/// フォワーディングの中の出来事（転送先に接続できなかったなど）を知らせる関数。
pub type ForwardNote = Arc<dyn Fn(&str) + Send + Sync>;

/// 動いているフォワーディング。drop すると止める（待ち受けを閉じる）。
pub struct ActiveForward {
    /// 表示用の説明（`-L 127.0.0.1:8080 → localhost:80` など。接続先が選んだポートを含む）
    pub description: String,
    /// 待ち受けているポート（`-L`・`-D` は手元、`-R` は接続先。0 を求めたときは選ばれたもの）
    pub port: u16,
    stop: Option<Box<dyn FnOnce() + Send>>,
}

impl ActiveForward {
    pub fn new(description: String, port: u16, stop: Box<dyn FnOnce() + Send>) -> ActiveForward {
        ActiveForward {
            description,
            port,
            stop: Some(stop),
        }
    }
}

impl Drop for ActiveForward {
    fn drop(&mut self) {
        if let Some(s) = self.stop.take() {
            s();
        }
    }
}

impl fmt::Debug for ActiveForward {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.description)
    }
}

/// `[bind:]port` の bind（IPv6 は `[…]`）を表示する。
fn show_bind(bind: &Option<String>) -> String {
    match bind {
        Some(b) if b.contains(':') => format!("[{b}]:"),
        Some(b) => format!("{b}:"),
        None => String::new(),
    }
}

fn show_host(h: &str) -> String {
    if h.contains(':') {
        format!("[{h}]")
    } else {
        h.to_owned()
    }
}

impl fmt::Display for Forward {
    /// OpenSSH の引数の形（`-L 8080:localhost:80` など）。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Forward::Local {
                bind,
                port,
                host,
                host_port,
            } => write!(
                f,
                "-L {}{port}:{}:{host_port}",
                show_bind(bind),
                show_host(host)
            ),
            Forward::Remote {
                bind,
                port,
                host,
                host_port,
            } => write!(
                f,
                "-R {}{port}:{}:{host_port}",
                show_bind(bind),
                show_host(host)
            ),
            Forward::Dynamic { bind, port } => write!(f, "-D {}{port}", show_bind(bind)),
        }
    }
}

/// `:` で区切る（`[…]` の中の `:` では区切らない）。
fn split_colons(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_bracket = false;
    for c in s.chars() {
        match c {
            '[' if !in_bracket && cur.is_empty() => in_bracket = true,
            ']' if in_bracket => in_bracket = false,
            ':' if !in_bracket => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    if in_bracket {
        return Err("「[」が閉じていません".into());
    }
    out.push(cur);
    Ok(out)
}

fn port_of(s: &str, what: &str) -> Result<u16, String> {
    s.trim()
        .parse::<u16>()
        .map_err(|_| format!("{what}（{s}）を読めません"))
}

/// 待ち受けるアドレスの指定（空なら既定、`*` はすべて）。
fn bind_of(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

impl Forward {
    /// OpenSSH のオプションの形を読む。`kind` は `L`・`R`・`D`、`spec` は
    /// `[bind:]port:host:hostport`（`D` は `[bind:]port`）。`host` の IPv6 は `[…]`。
    pub fn parse_option(kind: char, spec: &str) -> Result<Forward, String> {
        let spec = spec.trim();
        let err = |e: String| format!("-{kind} {spec}: {e}");
        if spec.contains('/') && !spec.contains(':') {
            return Err(err("UNIX ドメインソケットの転送には対応していません".into()));
        }
        let parts = split_colons(spec).map_err(err)?;
        match kind.to_ascii_uppercase() {
            'D' => {
                let (bind, port) = match parts.as_slice() {
                    [p] => (None, p),
                    [b, p] => (bind_of(b), p),
                    _ => {
                        return Err(err(
                            "[待ち受けるアドレス:]ポート の形で指定してください".into()
                        ));
                    }
                };
                Ok(Forward::Dynamic {
                    bind,
                    port: port_of(port, "ポート").map_err(err)?,
                })
            }
            k @ ('L' | 'R') => {
                let (bind, port, host, host_port) = match parts.as_slice() {
                    [p, h, hp] => (None, p, h, hp),
                    [b, p, h, hp] => (bind_of(b), p, h, hp),
                    [_] | [_, _] if k == 'R' => {
                        return Err(err(
                            "接続先での SOCKS（-R ポート）には対応していません。-R ポート:ホスト:ポート の形で指定してください"
                                .into(),
                        ));
                    }
                    _ => {
                        return Err(err(
                            "[待ち受けるアドレス:]ポート:転送先のホスト:転送先のポート の形で指定してください"
                                .into(),
                        ));
                    }
                };
                if host.trim().is_empty() {
                    return Err(err("転送先のホストがありません".into()));
                }
                let port = port_of(port, "待ち受けるポート").map_err(err)?;
                let host_port = port_of(host_port, "転送先のポート").map_err(err)?;
                let host = host.trim().to_owned();
                if k == 'L' {
                    if port == 0 {
                        return Err(err("手元の待ち受けるポートに 0 は使えません".into()));
                    }
                    Ok(Forward::Local {
                        bind,
                        port,
                        host,
                        host_port,
                    })
                } else {
                    Ok(Forward::Remote {
                        bind,
                        port,
                        host,
                        host_port,
                    })
                }
            }
            _ => Err(format!("-{kind}: フォワーディングの種類を読めません")),
        }
    }

    /// `~/.ssh/config` の形を読む（`LocalForward [bind:]port host:hostport`・
    /// `RemoteForward …`・`DynamicForward [bind:]port`）。`keyword` は小文字。
    pub fn parse_config(keyword: &str, value: &str) -> Result<Forward, String> {
        let kind = match keyword {
            "localforward" => 'L',
            "remoteforward" => 'R',
            "dynamicforward" => 'D',
            _ => return Err(format!("{keyword}: フォワーディングの指定ではありません")),
        };
        let words: Vec<&str> = value.split_whitespace().collect();
        let spec = match (kind, words.as_slice()) {
            ('D', [one]) => (*one).to_owned(),
            (_, [listen, target]) => format!("{listen}:{target}"),
            // 1 語で書いた形（`-L` と同じ）も受け付ける
            (_, [one]) => (*one).to_owned(),
            _ => {
                return Err(format!("{keyword} {value}: 指定を読めません"));
            }
        };
        Forward::parse_option(kind, &spec)
    }

    /// 文字列 1 つで書いた指定（yyeditor の設定やターミナルの入力。`L 8080:localhost:80`、
    /// `-L 8080:localhost:80`、`D 1080` など）を読む。
    pub fn parse_text(s: &str) -> Result<Forward, String> {
        let s = s.trim();
        let s = s.strip_prefix('-').unwrap_or(s);
        let mut chars = s.chars();
        let Some(kind) = chars.next() else {
            return Err("フォワーディングの指定が空です".into());
        };
        let rest = chars.as_str().trim_start_matches(['=', ' ', '\t']);
        if !matches!(kind.to_ascii_uppercase(), 'L' | 'R' | 'D') {
            return Err(format!(
                "{s}: L・R・D のどれかで始めてください（例: L 8080:localhost:80）"
            ));
        }
        Forward::parse_option(kind, rest)
    }

    /// 手元で待ち受けるアドレス（`Local`・`Dynamic`）。既定は 127.0.0.1、`*` はすべて。
    pub fn local_bind(&self) -> String {
        let b = match self {
            Forward::Local { bind, .. } | Forward::Dynamic { bind, .. } => bind.as_deref(),
            Forward::Remote { .. } => None,
        };
        match b {
            None | Some("localhost") => "127.0.0.1".into(),
            Some("*") => "0.0.0.0".into(),
            Some(b) => b.to_owned(),
        }
    }

    /// 接続先で待ち受けるアドレス（`Remote`。OpenSSH と同じく既定は `localhost`、`*` は `""`）。
    pub fn remote_bind(&self) -> String {
        match self {
            Forward::Remote { bind, .. } => match bind.as_deref() {
                None => "localhost".into(),
                Some("*") => String::new(),
                Some(b) => b.to_owned(),
            },
            _ => String::new(),
        }
    }
}

/// フォワーディングの指定を並べて読む（読めないものは説明を返す）。
pub fn parse_all(items: &[String]) -> (Vec<Forward>, Vec<String>) {
    let mut ok = Vec::new();
    let mut errors = Vec::new();
    for i in items {
        match Forward::parse_text(i) {
            Ok(f) => ok.push(f),
            Err(e) => errors.push(e),
        }
    }
    (ok, errors)
}

/// ターミナルの「SSH で接続」の入力を、接続先とフォワーディングに分ける
/// （`yamada@build01 -L 8080:localhost:80 -D1080` など）。
pub fn split_target_and_forwards(input: &str) -> Result<(String, Vec<Forward>), String> {
    let mut words = input.split_whitespace();
    let mut target = None;
    let mut forwards = Vec::new();
    while let Some(w) = words.next() {
        if let Some(rest) = w.strip_prefix('-') {
            let mut chars = rest.chars();
            let kind = chars.next().unwrap_or(' ');
            if !matches!(kind, 'L' | 'R' | 'D') {
                return Err(format!(
                    "{w}: 使えるオプションは -L・-R・-D（ポートフォワーディング）だけです"
                ));
            }
            let spec = match chars.as_str() {
                "" => words
                    .next()
                    .ok_or_else(|| format!("-{kind} の後に指定がありません"))?
                    .to_owned(),
                s => s.to_owned(),
            };
            forwards.push(Forward::parse_option(kind, &spec)?);
        } else if target.is_none() {
            target = Some(w.to_owned());
        } else {
            return Err(format!("{w}: 接続先は 1 つだけ指定してください"));
        }
    }
    target
        .map(|t| (t, forwards))
        .ok_or_else(|| "接続先がありません".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(bind: Option<&str>, port: u16, host: &str, hp: u16) -> Forward {
        Forward::Local {
            bind: bind.map(Into::into),
            port,
            host: host.into(),
            host_port: hp,
        }
    }

    #[test]
    fn parses_options() {
        assert_eq!(
            Forward::parse_option('L', "8080:localhost:80").unwrap(),
            l(None, 8080, "localhost", 80)
        );
        assert_eq!(
            Forward::parse_option('L', "0.0.0.0:8080:db.internal:5432").unwrap(),
            l(Some("0.0.0.0"), 8080, "db.internal", 5432)
        );
        assert_eq!(
            Forward::parse_option('L', "[::1]:8080:[fe80::1]:22").unwrap(),
            l(Some("::1"), 8080, "fe80::1", 22)
        );
        assert_eq!(
            Forward::parse_option('R', "9000:localhost:3000").unwrap(),
            Forward::Remote {
                bind: None,
                port: 9000,
                host: "localhost".into(),
                host_port: 3000
            }
        );
        assert_eq!(
            Forward::parse_option('D', "1080").unwrap(),
            Forward::Dynamic {
                bind: None,
                port: 1080
            }
        );
        assert_eq!(
            Forward::parse_option('D', "*:1080").unwrap(),
            Forward::Dynamic {
                bind: Some("*".into()),
                port: 1080
            }
        );
        for (k, bad) in [
            ('L', "8080"),
            ('L', "x:localhost:80"),
            ('L', "8080:localhost:99999"),
            ('L', "0:localhost:80"),
            ('L', "8080::80"),
            ('R', "8080"),
            ('D', "a:b:c"),
            ('L', "/tmp/sock"),
            ('L', "[::1:8080:h:1"),
        ] {
            assert!(Forward::parse_option(k, bad).is_err(), "{k} {bad}");
        }
    }

    #[test]
    fn parses_config_and_text() {
        assert_eq!(
            Forward::parse_config("localforward", "8080 localhost:80").unwrap(),
            l(None, 8080, "localhost", 80)
        );
        assert_eq!(
            Forward::parse_config("localforward", "127.0.0.1:8080 [::1]:80").unwrap(),
            l(Some("127.0.0.1"), 8080, "::1", 80)
        );
        assert!(matches!(
            Forward::parse_config("dynamicforward", "1080").unwrap(),
            Forward::Dynamic { port: 1080, .. }
        ));
        assert!(matches!(
            Forward::parse_config("remoteforward", "2222 localhost:22").unwrap(),
            Forward::Remote { port: 2222, .. }
        ));
        assert_eq!(
            Forward::parse_text("L 8080:localhost:80").unwrap(),
            l(None, 8080, "localhost", 80)
        );
        assert_eq!(
            Forward::parse_text("-L8080:localhost:80").unwrap(),
            l(None, 8080, "localhost", 80)
        );
        assert!(Forward::parse_text("X 1").is_err());
        let (ok, bad) = parse_all(&["D 1080".into(), "L nope".into()]);
        assert_eq!(ok.len(), 1);
        assert_eq!(bad.len(), 1);
    }

    #[test]
    fn displays_like_openssh() {
        assert_eq!(
            l(None, 8080, "localhost", 80).to_string(),
            "-L 8080:localhost:80"
        );
        assert_eq!(
            l(Some("::1"), 8080, "fe80::1", 22).to_string(),
            "-L [::1]:8080:[fe80::1]:22"
        );
        let r = Forward::parse_option('R', "*:9000:localhost:3000").unwrap();
        assert_eq!(r.to_string(), "-R *:9000:localhost:3000");
        assert_eq!(r.remote_bind(), "");
        let d = Forward::parse_option('D', "1080").unwrap();
        assert_eq!(d.to_string(), "-D 1080");
        assert_eq!(d.local_bind(), "127.0.0.1");
        assert_eq!(
            Forward::parse_option('R', "9000:h:1")
                .unwrap()
                .remote_bind(),
            "localhost"
        );
    }

    #[test]
    fn splits_the_terminal_input() {
        let (t, f) =
            split_target_and_forwards("yamada@build01 -L 8080:localhost:80 -D1080").unwrap();
        assert_eq!(t, "yamada@build01");
        assert_eq!(f.len(), 2);
        assert_eq!(split_target_and_forwards("build01").unwrap().1, vec![]);
        assert!(split_target_and_forwards("-L 8080:localhost:80").is_err());
        assert!(split_target_and_forwards("a b").is_err());
        assert!(split_target_and_forwards("a -X 1").is_err());
        assert!(split_target_and_forwards("a -L").is_err());
    }
}

//! アドレスバーの入力 → URL（19 章 4）。

/// 入力を開く URL にする。URL らしくなければ検索の URL（`search_url` の `%s` を検索語に置き換える）。
pub fn to_url(input: &str, search_url: &str) -> Option<String> {
    let t = input.trim();
    if t.is_empty() {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    // スキームのあるもの
    for scheme in [
        "http://",
        "https://",
        "file:///",
        "about:",
        "edge://",
        "data:",
        "view-source:",
    ] {
        if lower.starts_with(scheme) {
            return Some(t.to_owned());
        }
    }
    // Windows のパス（C:\…・\\server\share）
    let b = t.as_bytes();
    if (b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/'))
        || t.starts_with("\\\\")
    {
        return Some(file_url(t));
    }
    if !t.contains(char::is_whitespace) && looks_like_host(t) {
        return Some(format!("http{}://{t}", if is_local(t) { "" } else { "s" }));
    }
    Some(search_url.replace("%s", &encode(t)))
}

/// ホスト（とポート・パス）らしいか: `example.com`・`localhost:8080/x`・`192.168.0.1`・`[::1]:80`。
fn looks_like_host(t: &str) -> bool {
    let host_port = t.split(['/', '?', '#']).next().unwrap_or("");
    if host_port.starts_with('[') {
        return host_port.contains(']');
    }
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (host_port, None),
    };
    if port.is_some_and(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
        return false;
    }
    if host.is_empty() || host.starts_with('.') || host.ends_with('.') {
        return false;
    }
    let ok_chars = host
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_'));
    ok_chars && (host.contains('.') || host.eq_ignore_ascii_case("localhost") || port.is_some())
}

/// 手元のホスト（http:// で開く）。
fn is_local(t: &str) -> bool {
    let host = t.split(['/', ':', '?', '#']).next().unwrap_or("");
    host.eq_ignore_ascii_case("localhost")
        || host.starts_with("127.")
        || host.starts_with("192.168.")
        || host.starts_with("10.")
        || t.starts_with("[::1]")
        || !host.contains('.')
}

fn file_url(path: &str) -> String {
    let p = path.replace('\\', "/");
    let p = p.trim_start_matches('/');
    let body: String = p
        .split('/')
        .map(|seg| {
            seg.bytes()
                .map(|b| match b {
                    b' ' => "%20".to_owned(),
                    b'#' => "%23".to_owned(),
                    b'%' => "%25".to_owned(),
                    b'?' => "%3F".to_owned(),
                    b if b < 0x80 => (b as char).to_string(),
                    b => format!("%{b:02X}"),
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/");
    if path.starts_with("\\\\") {
        format!("file://{body}")
    } else {
        format!("file:///{body}")
    }
}

/// 検索語を URL に入れる形にする（UTF-8 のパーセント符号化。空白は `+`）。
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_owned(),
            b => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "https://www.bing.com/search?q=%s";

    #[test]
    fn turns_input_into_urls() {
        let u = |s: &str| to_url(s, S).unwrap();
        assert_eq!(u("https://example.com/a?b=c"), "https://example.com/a?b=c");
        assert_eq!(u("  example.com  "), "https://example.com");
        assert_eq!(
            u("www.example.co.jp/path"),
            "https://www.example.co.jp/path"
        );
        assert_eq!(u("localhost:8080/app"), "http://localhost:8080/app");
        assert_eq!(u("192.168.0.10"), "http://192.168.0.10");
        assert_eq!(u("intranet:8080"), "http://intranet:8080");
        assert_eq!(u("[::1]:3000"), "http://[::1]:3000");
        assert_eq!(u("about:blank"), "about:blank");
        assert_eq!(u(r"C:\work\a b.html"), "file:///C:/work/a%20b.html");
        assert_eq!(u(r"\\nas\share\x.html"), "file://nas/share/x.html");
        // 検索語
        assert_eq!(
            u("rust 言語"),
            "https://www.bing.com/search?q=rust+%E8%A8%80%E8%AA%9E"
        );
        assert_eq!(u("hello"), "https://www.bing.com/search?q=hello");
        assert_eq!(u("a.b c"), "https://www.bing.com/search?q=a.b+c");
        assert_eq!(u("what:is"), "https://www.bing.com/search?q=what%3Ais");
        assert_eq!(u(".hidden"), "https://www.bing.com/search?q=.hidden");
        assert!(to_url("   ", S).is_none());
    }
}

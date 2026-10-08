//! 画面の文字列のリンク（URL・ファイルのパス）と、プロンプトから分かる作業フォルダ（12 章）。
//!
//! - URL: `http://`・`https://`・`ftp://`・`file://` で始まるもの（文の終わりの `.`・`,`・閉じ括弧などは
//!   含めない。括弧は対になっていれば含める）。
//! - パス: `/`・`~/`・`./`・`../`・`C:\`・`\\サーバー\` で始まるもの、`/` か `\` を含んで拡張子のあるもの
//!   （`src/main.rs`）、拡張子のあるファイル名（`README.md`）。後ろの行・桁（`:12`・`:12:5`・`(12,5)`）も
//!   読む。本当にあるかは、開くときに確かめる。

/// リンクの行き先。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Url(String),
    Path {
        path: String,
        line: Option<u32>,
        col: Option<u32>,
    },
}

/// 文字列の中のリンク（`start`・`end` は文字の番号。`end` は含まない）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub start: usize,
    pub end: usize,
    pub target: LinkTarget,
}

const SCHEMES: [&str; 4] = ["https://", "http://", "ftp://", "file://"];

/// URL に入らない文字。
fn url_stop(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(
            c,
            '<' | '>'
                | '"'
                | '`'
                | '{'
                | '}'
                | '|'
                | '\\'
                | '^'
                | '「'
                | '」'
                | '『'
                | '』'
                | '、'
                | '。'
                | '（'
                | '）'
                | '　'
        )
}

/// 後ろの句読点・対のない閉じ括弧を外した終わり。
fn trim_end(chars: &[char], start: usize, mut end: usize) -> usize {
    while end > start {
        let c = chars[end - 1];
        let unbalanced = |open: char, close: char| {
            let s = &chars[start..end];
            s.iter().filter(|&&x| x == close).count() > s.iter().filter(|&&x| x == open).count()
        };
        let drop = match c {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' => true,
            ')' => unbalanced('(', ')'),
            ']' => unbalanced('[', ']'),
            _ => false,
        };
        if !drop {
            break;
        }
        end -= 1;
    }
    end
}

/// `text` の中の URL とパス（前から順に、重ならない）。
pub fn find(text: &str) -> Vec<Found> {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    let mut out = Vec::new();
    // URL
    let mut taken = vec![false; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        let scheme = SCHEMES.iter().find(|s| {
            let s: Vec<char> = s.chars().collect();
            lower.len() >= i + s.len() && lower[i..i + s.len()] == s[..]
        });
        let starts_word = i == 0 || !chars[i - 1].is_ascii_alphanumeric();
        if let (Some(s), true) = (scheme, starts_word) {
            let mut end = i + s.len();
            while end < chars.len() && !url_stop(chars[end]) {
                end += 1;
            }
            let end = trim_end(&chars, i, end);
            if end > i + s.len() {
                let url: String = chars[i..end].iter().collect();
                out.push(Found {
                    start: i,
                    end,
                    target: LinkTarget::Url(url),
                });
                taken[i..end].iter_mut().for_each(|t| *t = true);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    // パス（空白・引用符などで区切った語）
    let sep = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\'' | '`' | '<' | '>' | '|' | '[' | ']' | '{' | '}' | '　' | '「' | '」'
            )
    };
    let mut i = 0;
    while i < chars.len() {
        if sep(chars[i]) || taken[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && !sep(chars[i]) && !taken[i] {
            i += 1;
        }
        if let Some(f) = path_token(&chars, start, i) {
            out.push(f);
        }
    }
    out.sort_by_key(|f| f.start);
    out
}

/// 語 `chars[start..end]` がパスなら、そのリンク。
fn path_token(chars: &[char], mut start: usize, end: usize) -> Option<Found> {
    // 前の飾り（`(`・`--file=` など）
    while start < end && matches!(chars[start], '(' | ',' | ';') {
        start += 1;
    }
    if let Some(eq) = chars[start..end].iter().position(|&c| c == '=') {
        let after = start + eq + 1;
        if after < end && matches!(chars[after], '/' | '~' | '.') {
            start = after;
        }
    }
    let mut end = trim_end(chars, start, end);
    if end <= start {
        return None;
    }
    let word: String = chars[start..end].iter().collect();
    if word.contains("://") {
        return None;
    }
    // 行・桁: `(12,5)`・`(12)`
    let (mut path, mut line, mut col) = (word.clone(), None, None);
    if let Some(open) = word.rfind('(')
        && word.ends_with(')')
    {
        let inner = &word[open + 1..word.len() - 1];
        let mut it = inner.split(',');
        if let Some(l) = it.next().and_then(|s| s.trim().parse().ok()) {
            line = Some(l);
            col = it.next().and_then(|s| s.trim().parse().ok());
            path = word[..open].to_string();
        }
    }
    // 行・桁: `:12`・`:12:5`
    if line.is_none() {
        let mut nums = Vec::new();
        let mut rest = path.as_str();
        while let Some(i) = rest.rfind(':') {
            let n = &rest[i + 1..];
            if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) || nums.len() == 2 {
                break;
            }
            nums.push(n.parse::<u32>().ok()?);
            rest = &rest[..i];
        }
        if !nums.is_empty() {
            nums.reverse();
            line = nums.first().copied();
            col = nums.get(1).copied();
            path = rest.to_string();
        }
    }
    if !looks_like_path(&path, line.is_some()) {
        return None;
    }
    // 下線は、行・桁を含めた語の全体
    if path.is_empty() {
        end = start;
    }
    Some(Found {
        start,
        end,
        target: LinkTarget::Path { path, line, col },
    })
}

/// パスらしいか（`with_line` なら行番号が付いていた）。
fn looks_like_path(p: &str, with_line: bool) -> bool {
    if p.is_empty() || !p.chars().any(|c| c.is_alphanumeric()) {
        return false;
    }
    let b = p.as_bytes();
    let drive = b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/');
    if drive || p.starts_with("\\\\") {
        return true;
    }
    if p.starts_with("//") {
        return false;
    }
    if p.starts_with('/') {
        // `/` だけ・オプションのような `/s` は除く
        return p.len() > 2;
    }
    if p.starts_with("~/") || p.starts_with("./") || p.starts_with("../") {
        return true;
    }
    let name = p.rsplit(['/', '\\']).next().unwrap_or(p);
    let ext = name
        .rsplit_once('.')
        .map(|(stem, e)| {
            !stem.is_empty()
                && (1..=8).contains(&e.len())
                && e.chars().all(|c| c.is_ascii_alphanumeric())
                && e.chars().any(|c| c.is_ascii_alphabetic())
        })
        .unwrap_or(false);
    let has_sep = p.contains('/') || p.contains('\\');
    // 数字だけの区切り（日付 2026/10/08・分数 1/2）は除く
    let digits_only = p
        .split(['/', '\\', '.', '-'])
        .all(|s| s.bytes().all(|b| b.is_ascii_digit()));
    if digits_only {
        return false;
    }
    (has_sep && (ext || with_line)) || (ext && (with_line || name.len() > 3))
}

/// プロンプトの行から作業フォルダを読む（`yamada@build:~/src$ `・`PS C:\work> `・`C:\work>`）。
/// `~` で始まることがある（展開は呼んだ側）。プロンプトでなければ `None`。
pub fn prompt_cwd(line: &str) -> Option<String> {
    let l = line.trim_start();
    // PowerShell
    if let Some(rest) = l.strip_prefix("PS ")
        && let Some(gt) = rest.find('>')
    {
        let p = rest[..gt].trim();
        let p = p
            .strip_prefix("Microsoft.PowerShell.Core\\FileSystem::")
            .unwrap_or(p);
        if is_windows_abs(p) {
            return Some(p.to_string());
        }
    }
    // コマンド プロンプト
    if is_windows_abs(l)
        && let Some(gt) = l.find('>')
    {
        let p = &l[..gt];
        if is_windows_abs(p) {
            return Some(p.to_string());
        }
    }
    // bash などの `ユーザー@ホスト:パス$`
    let at = l.find('@')?;
    let user_ok = l[..at]
        .rsplit(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == '[')
        .next()
        .is_some_and(|u| {
            !u.is_empty() && u.chars().all(|c| c.is_alphanumeric() || "._-".contains(c))
        });
    if !user_ok {
        return None;
    }
    let after = &l[at + 1..];
    let colon = after.find(':')?;
    let host = &after[..colon];
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_alphanumeric() || "._-".contains(c))
    {
        return None;
    }
    let rest = &after[colon + 1..];
    if !(rest.starts_with('/') || rest.starts_with('~')) {
        return None;
    }
    // パスの終わりはプロンプトの記号（後ろから探す。パスに空白があってもよい）
    let end = ["$ ", "# ", "% ", "> "]
        .iter()
        .filter_map(|m| rest.rfind(m))
        .max()
        .or_else(|| {
            let t = rest.trim_end();
            t.ends_with(['$', '#', '%', '>']).then(|| t.len() - 1)
        })?;
    let p = rest[..end].trim_end();
    (!p.is_empty()).then(|| p.to_string())
}

fn is_windows_abs(p: &str) -> bool {
    let b = p.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\')
        || p.starts_with("\\\\")
}

/// `file://ホスト/パス`（OSC 7）のパス（百分率の符号を戻す。Windows の `/C:/x` は `C:/x`）。
pub fn file_url_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let slash = rest.find('/')?;
    let raw = &rest[slash..];
    let mut bytes = Vec::with_capacity(raw.len());
    let b = raw.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&raw[i + 1..i + 3], 16)
        {
            bytes.push(v);
            i += 3;
            continue;
        }
        bytes.push(b[i]);
        i += 1;
    }
    let s = String::from_utf8_lossy(&bytes).into_owned();
    let s = match s.as_bytes() {
        [b'/', d, b':', ..] if d.is_ascii_alphabetic() => s[1..].to_string(),
        _ => s,
    };
    Some(s)
}

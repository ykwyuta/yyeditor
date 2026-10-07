//! 受け入れた証明書の記録（TOFU。SSH の known_hosts と同じ考え方）。
//!
//! 1 行に 1 つ: `ホスト:ポート sha256:指紋 主体`（主体は読む人のための注記）。`#` で始まる行と空行は
//! 無視する。IPv6 のアドレスは `[::1]:992` と書く。

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

/// 記録と照らした結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// 同じ指紋が記録されている
    Match,
    /// 記録がない
    Unknown,
    /// 違う指紋が記録されている（`line` は 1 から数えた行）
    Changed { line: usize, recorded: String },
}

/// 記録のキー（`ホスト:ポート`。ホストは小文字）。
pub fn key(host: &str, port: u16) -> String {
    let h = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if h.contains(':') {
        format!("[{h}]:{port}")
    } else {
        format!("{h}:{port}")
    }
}

/// 記録と照らす（ファイルがなければ記録なし）。
pub fn lookup(path: &Path, key: &str, sha256: &str) -> io::Result<Lookup> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Lookup::Unknown),
        Err(e) => return Err(e),
    };
    let mut changed = None;
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let (Some(k), Some(fp)) = (it.next(), it.next()) else {
            continue;
        };
        if !k.eq_ignore_ascii_case(key) {
            continue;
        }
        let fp = fp.strip_prefix("sha256:").unwrap_or(fp);
        if fp.eq_ignore_ascii_case(sha256) {
            return Ok(Lookup::Match);
        }
        changed.get_or_insert(Lookup::Changed {
            line: i + 1,
            recorded: fp.to_owned(),
        });
    }
    Ok(changed.unwrap_or(Lookup::Unknown))
}

/// 記録に加える（フォルダがなければ作る）。
pub fn add(path: &Path, key: &str, sha256: &str, subject: &str) -> io::Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let new = !path.exists();
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    if new {
        writeln!(
            f,
            "# yyterm の 3270 の TLS で受け入れた証明書（ホスト:ポート sha256:指紋 主体）"
        )?;
    }
    let subject: String = subject
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    writeln!(f, "{key} sha256:{sha256} {subject}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_looks_up() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("known_certs");
        let k = key("MVS01.example", 992);
        assert_eq!(k, "mvs01.example:992");
        assert_eq!(key("::1", 992), "[::1]:992");
        assert_eq!(lookup(&p, &k, "AA:BB").unwrap(), Lookup::Unknown);
        add(&p, &k, "AA:BB", "CN=mvs01").unwrap();
        add(&p, "other:23", "CC:DD", "CN=other").unwrap();
        assert_eq!(lookup(&p, &k, "aa:bb").unwrap(), Lookup::Match);
        assert_eq!(
            lookup(&p, &k, "EE:FF").unwrap(),
            Lookup::Changed {
                line: 2,
                recorded: "AA:BB".into()
            }
        );
        assert_eq!(lookup(&p, "x:1", "AA:BB").unwrap(), Lookup::Unknown);
    }
}

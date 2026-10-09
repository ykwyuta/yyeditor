//! ホスト鍵の記録と照合（11 章 4.3）。
//!
//! 書式は OpenSSH の `known_hosts` と同じ。yyeditor 自身の記録（`%APPDATA%\yyeditor\known_hosts`）
//! には書き込み、利用者の `~/.ssh/known_hosts` は読むだけにする。
//!
//! 対応する書式: カンマ区切りのホスト名、`[host]:port`、ワイルドカード（`*`・`?`）と否定（`!`）、
//! ハッシュ化されたホスト名（`|1|salt|hash`）、`@revoked`。`@cert-authority` の行は使わない。
//! 鍵の比較は種類（`ssh-ed25519` など）と鍵のデータ（Base64）の文字列で行う。

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use data_encoding::BASE64;
use hmac::{Hmac, KeyInit, Mac};

use crate::ssh_config::wildcard;

/// 照合の結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKeyStatus {
    /// 記録と一致した
    Known,
    /// 記録がない（同じ種類の鍵が記録されていない）
    Unknown,
    /// 同じ種類の別の鍵が記録されている、または失効した鍵
    Changed { file: PathBuf, line: usize },
}

/// `files` の記録と照合する。`name` は [`crate::HostSpec::known_hosts_name`]。
pub fn check(files: &[PathBuf], name: &str, algorithm: &str, key_b64: &str) -> HostKeyStatus {
    let mut changed = None;
    let mut known = false;
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let Some(entry) = Entry::parse(line) else {
                continue;
            };
            if !entry.matches_host(name) {
                continue;
            }
            let same_key = entry.key == key_b64 && entry.algorithm == algorithm;
            match entry.marker {
                Marker::Revoked if same_key => {
                    return HostKeyStatus::Changed {
                        file: file.clone(),
                        line: i + 1,
                    };
                }
                Marker::Revoked | Marker::CertAuthority => {}
                Marker::None if same_key => known = true,
                Marker::None if entry.algorithm == algorithm => {
                    changed.get_or_insert((file.clone(), i + 1));
                }
                Marker::None => {}
            }
        }
    }
    if known {
        return HostKeyStatus::Known;
    }
    match changed {
        Some((file, line)) => HostKeyStatus::Changed { file, line },
        None => HostKeyStatus::Unknown,
    }
}

/// 鍵を記録する（ファイルの末尾に 1 行加える）。
pub fn learn(file: &Path, name: &str, algorithm: &str, key_b64: &str) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let needs_newline = std::fs::read(file)
        .map(|b| !b.is_empty() && !b.ends_with(b"\n"))
        .unwrap_or(false);
    let mut f = OpenOptions::new().create(true).append(true).open(file)?;
    let mut line = String::new();
    if needs_newline {
        line.push('\n');
    }
    line.push_str(&format!("{name} {algorithm} {key_b64}\n"));
    f.write_all(line.as_bytes())?;
    f.sync_all()
}

/// 鍵の指紋（`SHA256:` ＋ Base64。末尾の `=` は付けない。OpenSSH の表示と同じ形）。
pub fn fingerprint(key_b64: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    let blob = BASE64.decode(key_b64.as_bytes()).ok()?;
    let digest = Sha256::digest(&blob);
    Some(format!(
        "SHA256:{}",
        BASE64.encode(&digest).trim_end_matches('=')
    ))
}

#[derive(Debug, PartialEq, Eq)]
enum Marker {
    None,
    Revoked,
    CertAuthority,
}

struct Entry<'a> {
    marker: Marker,
    hosts: &'a str,
    algorithm: &'a str,
    key: &'a str,
}

impl<'a> Entry<'a> {
    fn parse(line: &'a str) -> Option<Entry<'a>> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let mut words = line.split_whitespace();
        let mut first = words.next()?;
        let marker = match first {
            "@revoked" => Marker::Revoked,
            "@cert-authority" => Marker::CertAuthority,
            m if m.starts_with('@') => return None,
            _ => Marker::None,
        };
        if marker != Marker::None {
            first = words.next()?;
        }
        Some(Entry {
            marker,
            hosts: first,
            algorithm: words.next()?,
            key: words.next()?,
        })
    }

    fn matches_host(&self, name: &str) -> bool {
        let mut matched = false;
        for p in self.hosts.split(',') {
            if let Some(hashed) = p.strip_prefix("|1|") {
                if hashed_matches(hashed, name) {
                    matched = true;
                }
            } else if let Some(neg) = p.strip_prefix('!') {
                if wildcard(neg, name) {
                    return false;
                }
            } else if wildcard(p, name) {
                matched = true;
            }
        }
        matched
    }
}

/// `salt|hash`（どちらも Base64）が `name` の HMAC-SHA1 か。
fn hashed_matches(hashed: &str, name: &str) -> bool {
    let Some((salt, hash)) = hashed.split_once('|') else {
        return false;
    };
    let (Ok(salt), Ok(hash)) = (
        BASE64.decode(salt.as_bytes()),
        BASE64.decode(hash.as_bytes()),
    ) else {
        return false;
    };
    let Ok(mut mac) = <Hmac<sha1::Sha1> as KeyInit>::new_from_slice(&salt) else {
        return false;
    };
    mac.update(name.as_bytes());
    mac.verify_slice(&hash).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";
    const OTHER: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIA1jM5GTzu6MSsNdGy9zDaw0Wo6mWMvy3ZmpOHz3hdr3";

    fn write(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn matches_plain_and_patterns() {
        let (_d, f) = write(&format!(
            "# comment\nhost1,10.0.0.1 ssh-ed25519 {ED}\n[host2]:2222 ssh-ed25519 {ED} c\n\
             *.example.com,!bad.example.com ssh-ed25519 {ED}\n"
        ));
        let files = [f.clone()];
        assert_eq!(
            check(&files, "10.0.0.1", "ssh-ed25519", ED),
            HostKeyStatus::Known
        );
        assert_eq!(
            check(&files, "[host2]:2222", "ssh-ed25519", ED),
            HostKeyStatus::Known
        );
        assert_eq!(
            check(&files, "host2", "ssh-ed25519", ED),
            HostKeyStatus::Unknown
        );
        assert_eq!(
            check(&files, "a.example.com", "ssh-ed25519", ED),
            HostKeyStatus::Known
        );
        assert_eq!(
            check(&files, "bad.example.com", "ssh-ed25519", ED),
            HostKeyStatus::Unknown
        );
        assert_eq!(
            check(&files, "host1", "ssh-ed25519", OTHER),
            HostKeyStatus::Changed { file: f, line: 2 }
        );
        // 別の種類の鍵しか記録がなければ未知
        assert_eq!(
            check(&files, "host1", "ecdsa-sha2-nistp256", OTHER),
            HostKeyStatus::Unknown
        );
    }

    #[test]
    fn hashed_names() {
        // ssh-keygen -H で作った host1 の記録（salt は固定値）
        let salt = [7u8; 20];
        let mut mac = <Hmac<sha1::Sha1> as KeyInit>::new_from_slice(&salt).unwrap();
        mac.update(b"host1");
        let hash = mac.finalize().into_bytes();
        let line = format!(
            "|1|{}|{} ssh-ed25519 {ED}\n",
            BASE64.encode(&salt),
            BASE64.encode(&hash)
        );
        let (_d, f) = write(&line);
        assert_eq!(
            check(std::slice::from_ref(&f), "host1", "ssh-ed25519", ED),
            HostKeyStatus::Known
        );
        assert_eq!(
            check(&[f], "host2", "ssh-ed25519", ED),
            HostKeyStatus::Unknown
        );
    }

    #[test]
    fn revoked_and_unparsable_lines() {
        let (_d, f) = write(&format!(
            "garbage\n@cert-authority * ssh-ed25519 {OTHER}\n@revoked * ssh-ed25519 {ED}\n"
        ));
        assert!(matches!(
            check(std::slice::from_ref(&f), "any", "ssh-ed25519", ED),
            HostKeyStatus::Changed { line: 3, .. }
        ));
        assert_eq!(
            check(&[f], "any", "ssh-ed25519", OTHER),
            HostKeyStatus::Unknown
        );
    }

    #[test]
    fn learns_and_reads_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let own = dir.path().join("sub").join("known_hosts");
        let user = dir.path().join("user_known_hosts");
        std::fs::write(&user, format!("old ssh-ed25519 {OTHER}")).unwrap();
        let files = [own.clone(), user.clone(), dir.path().join("missing")];
        assert_eq!(
            check(&files, "new", "ssh-ed25519", ED),
            HostKeyStatus::Unknown
        );
        learn(&own, "new", "ssh-ed25519", ED).unwrap();
        learn(&own, "[new]:2222", "ssh-ed25519", ED).unwrap();
        assert_eq!(
            check(&files, "new", "ssh-ed25519", ED),
            HostKeyStatus::Known
        );
        assert_eq!(
            check(&files, "[new]:2222", "ssh-ed25519", ED),
            HostKeyStatus::Known
        );
        assert_eq!(
            check(&files, "old", "ssh-ed25519", OTHER),
            HostKeyStatus::Known
        );
        // 改行で終わっていないファイルにも行として加える
        learn(&user, "x", "ssh-ed25519", ED).unwrap();
        assert_eq!(check(&[user], "x", "ssh-ed25519", ED), HostKeyStatus::Known);
    }

    #[test]
    fn fingerprints_like_openssh() {
        // ssh-keygen -lf の表示と同じ形
        let fp = fingerprint(ED).unwrap();
        assert!(fp.starts_with("SHA256:") && !fp.ends_with('='));
        assert_eq!(fp.len(), "SHA256:".len() + 43);
        assert!(fingerprint("not base64!").is_none());
    }

    #[test]
    fn revocation_overrides_matches_in_any_order() {
        let (_a, accepted) = write(&format!("host ssh-ed25519 {ED}\n"));
        let (_r, revoked) = write(&format!("@revoked * ssh-ed25519 {ED}\n"));
        for files in [
            vec![accepted.clone(), revoked.clone()],
            vec![revoked.clone(), accepted.clone()],
        ] {
            assert!(matches!(
                check(&files, "host", "ssh-ed25519", ED),
                HostKeyStatus::Changed { .. }
            ));
        }
        for text in [
            format!("host ssh-ed25519 {ED}\n@revoked host ssh-ed25519 {ED}\n"),
            format!("@revoked host ssh-ed25519 {ED}\nhost ssh-ed25519 {ED}\n"),
        ] {
            let (_d, file) = write(&text);
            assert!(matches!(
                check(&[file], "host", "ssh-ed25519", ED),
                HostKeyStatus::Changed { .. }
            ));
        }
    }
}

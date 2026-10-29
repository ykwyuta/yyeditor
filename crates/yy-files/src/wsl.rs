//! WSL UNC paths and validated Linux permission data. No command execution.
use std::io;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub distro: String,
    pub linux: String,
}
impl Location {
    pub fn parse(raw: &str) -> Option<Self> {
        let normalized = raw.replace('\\', "/");
        let rest = if normalized
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("//?/UNC/"))
        {
            &normalized[8..]
        } else {
            normalized.strip_prefix("//")?
        };
        let (server, tail) = rest.split_once('/')?;
        if !server.eq_ignore_ascii_case("wsl.localhost") && !server.eq_ignore_ascii_case("wsl$") {
            return None;
        }
        let (distro, tail) = tail.split_once('/').unwrap_or((tail, ""));
        if !valid_distro(distro) {
            return None;
        }
        let mut parts = Vec::new();
        for part in tail.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                _ if part.contains('\0') => return None,
                _ => parts.push(part),
            }
        }
        Some(Self {
            distro: distro.into(),
            linux: format!("/{}", parts.join("/")),
        })
    }
    pub fn unc(&self) -> String {
        format!(
            "\\\\wsl.localhost\\{}{}",
            self.distro,
            self.linux.replace('/', "\\")
        )
    }
    pub fn credential_key(&self, user: &str) -> String {
        let distro = self.distro.to_lowercase();
        // Length-prefixed components avoid collisions; no password is part of the key.
        format!(
            "wsl-sudo/{}:{}/{}:{}",
            distro.len(),
            distro,
            user.len(),
            user
        )
    }
}
pub fn valid_distro(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && !name.starts_with('-')
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
        && !matches!(name, "." | "..")
}
pub fn decode_distributions(bytes: &[u8]) -> io::Result<Vec<String>> {
    let text = if bytes.starts_with(&[0xff, 0xfe]) || bytes.iter().take(128).any(|b| *b == 0) {
        let bytes = bytes.strip_prefix(&[0xff, 0xfe]).unwrap_or(bytes);
        if bytes.len() % 2 != 0 {
            return Err(io::Error::other("WSLの一覧の文字コードが不正です"));
        }
        String::from_utf16(
            &bytes
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>(),
        )
        .map_err(io::Error::other)?
    } else {
        String::from_utf8(bytes.to_vec()).map_err(io::Error::other)?
    };
    let mut names = Vec::new();
    for line in text.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if !valid_distro(line) {
            return Err(io::Error::other("WSLディストリビューション名が不正です"));
        }
        if !names.iter().any(|s: &String| s.eq_ignore_ascii_case(line)) {
            names.push(line.to_owned());
        }
    }
    Ok(names)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permissions {
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    pub owner: String,
    pub group: String,
    pub kind: String,
    pub fingerprint: String,
}
impl Permissions {
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        let fields: Vec<_> = bytes.split(|b| *b == 0).collect();
        if fields.len() != 8 || !fields[7].is_empty() {
            return Err(io::Error::other("WSLの権限情報が不正です"));
        }
        let text = |i: usize| String::from_utf8(fields[i].to_vec()).map_err(io::Error::other);
        let number = |i| text(i)?.parse().map_err(io::Error::other);
        Ok(Self {
            uid: number(0)?,
            gid: number(1)?,
            mode: mode(&text(2)?)?,
            owner: text(3)?,
            group: text(4)?,
            kind: text(5)?,
            fingerprint: text(6)?,
        })
    }
    pub fn symlink(&self) -> bool {
        self.kind == "symbolic link"
    }
}
pub fn mode(raw: &str) -> io::Result<u32> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 4 || !raw.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        return Err(io::Error::other(
            "権限は0000〜7777の8進数で指定してください",
        ));
    }
    u32::from_str_radix(raw, 8).map_err(io::Error::other)
}
pub fn identity(raw: &str) -> io::Result<String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > 128
        || raw.starts_with('-')
        || !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-@".contains(&b))
    {
        return Err(io::Error::other(
            "オーナー・グループはユーザー名または数値IDを指定してください",
        ));
    }
    Ok(raw.into())
}
pub const STAT_FORMAT: &str = "%u\\0%g\\0%a\\0%U\\0%G\\0%F\\0%d:%i\\0";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrips_paths_without_changing_linux_case_or_interpreting_shell_syntax() {
        let p = Location::parse(r"\\wsl$\Ubuntu\home\user\a'$(touch nope); x\..\Case.txt").unwrap();
        assert_eq!(p.linux, "/home/user/Case.txt");
        assert_eq!(Location::parse(&p.unc()), Some(p.clone()));
        assert_ne!(
            p,
            Location::parse(r"\\wsl.localhost\Ubuntu\home\user\case.txt").unwrap()
        );
        assert!(Location::parse(r"\\server\share\file").is_none());
        assert_eq!(
            Location::parse(r"\\?\UNC\wsl.localhost\Ubuntu\home\user\Case.txt"),
            Some(p.clone())
        );
        assert!(Location::parse(r"\\wsl$\-exec\file").is_none());
        assert_eq!(
            decode_distributions(
                &"Ubuntu\r\ndocker-desktop\r\n"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>()
            )
            .unwrap(),
            ["Ubuntu", "docker-desktop"]
        );
    }
    #[test]
    fn validates_stat_permissions_and_arguments() {
        let p = Permissions::parse(
            b"1000\0\x31\x30\x30\x31\0\x37\x35\x35\0user\0group\0directory\0\x31:123\0",
        )
        .unwrap();
        assert_eq!((p.uid, p.gid, p.mode), (1000, 1001, 0o755));
        for bad in ["888", "-1", "0755;id", "10000", ""] {
            assert!(mode(bad).is_err());
        }
        for bad in ["--root", "a:b", "a\nb", "$(id)", ""] {
            assert!(identity(bad).is_err());
        }
        assert!(Permissions::parse(b"0\0root\0").is_err());
    }
}

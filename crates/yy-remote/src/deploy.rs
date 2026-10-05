//! エージェントの配置と起動（11 章 6.2）。
//!
//! 接続先の OS とアーキテクチャを調べ、端末が持つエージェントのファイルを SSH 越しに送る
//! （`curl`・`wget`・`sftp-server`・インターネット接続は使わない）。配置先には版とハッシュの
//! 付いたフォルダを作り、配置したファイルの SHA-256 が手元のファイルと一致することを確かめる。

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use crate::{Process, Transport, run, shell_quote};

/// 端末が持つエージェントのファイル（exe と同じフォルダの `agents\`）。
#[derive(Clone, Debug)]
pub struct AgentFiles {
    dir: PathBuf,
}

impl AgentFiles {
    pub fn new(dir: impl Into<PathBuf>) -> AgentFiles {
        AgentFiles { dir: dir.into() }
    }

    /// 実行ファイルと同じフォルダの `agents`。環境変数 `YY_AGENT_DIR` があればそれ
    /// （開発中にビルドしたエージェントを使う場合）。
    pub fn beside_exe() -> AgentFiles {
        if let Some(d) = std::env::var_os("YY_AGENT_DIR") {
            return AgentFiles::new(d);
        }
        let dir = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.join("agents")))
            .unwrap_or_else(|| PathBuf::from("agents"));
        AgentFiles::new(dir)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// アーキテクチャ（`x86_64` / `aarch64`）のエージェントのファイル。
    pub fn file(&self, arch: &str) -> PathBuf {
        self.dir.join(format!("yy-agent-{arch}-linux"))
    }
}

/// 接続先の環境。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Platform {
    /// `uname -s`
    pub os: String,
    /// `uname -m` を正規化したもの（`x86_64` / `aarch64` など）
    pub arch: String,
    pub home: Vec<u8>,
}

/// 出力の区切りの印（ログインシェルが出す余計な文字と区別する）
const MARK: &str = "\x1eyyeditor\x1e";

/// 接続先の OS・アーキテクチャ・ホームを調べる。Linux 以外は対象外（11 章 6.1）。
pub fn probe(t: &dyn Transport) -> io::Result<Platform> {
    let cmd = format!(r#"printf '{MARK}\n'; uname -s; uname -m; printf '%s\n' "$HOME""#);
    let out = run(t, cmd.as_bytes(), b"")?;
    let text = &out.stdout;
    let mark = format!("{MARK}\n");
    let Some(pos) = find(text, mark.as_bytes()) else {
        return Err(io::Error::other(format!(
            "接続先でコマンドを実行できませんでした: {}",
            out.message()
        )));
    };
    let mut lines = text[pos + mark.len()..].split(|&b| b == b'\n');
    let mut next = || {
        lines
            .next()
            .map(|l| l.trim_ascii().to_vec())
            .unwrap_or_default()
    };
    let os = String::from_utf8_lossy(&next()).into_owned();
    let machine = String::from_utf8_lossy(&next()).into_owned();
    let home = next();
    if os != "Linux" {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("対応していない接続先です（{os} {machine}）。Linux にだけ接続できます"),
        ));
    }
    let arch = match machine.as_str() {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" | "armv8l" => "aarch64",
        m => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("対応していない接続先です（Linux {m}）"),
            ));
        }
    };
    if home.is_empty() || !home.starts_with(b"/") {
        return Err(io::Error::other("接続先のホームフォルダがわかりません"));
    }
    Ok(Platform {
        os,
        arch: arch.to_owned(),
        home,
    })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// 配置するエージェント。
#[derive(Clone, Debug)]
pub struct AgentImage {
    pub local: PathBuf,
    pub sha256: String,
}

impl AgentImage {
    /// `arch` のエージェントのファイルを読み、ハッシュを求める。
    pub fn load(files: &AgentFiles, arch: &str) -> io::Result<AgentImage> {
        let local = files.file(arch);
        let sha256 = File::open(&local)
            .and_then(|mut f| sha256_hex(&mut f))
            .map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!(
                        "{arch} 用のエージェントのファイルがありません（{}）。yyeditor を\
                         インストールし直してください: {e}",
                        local.display()
                    ),
                )
            })?;
        Ok(AgentImage { local, sha256 })
    }

    /// 配置先のフォルダの名前（`<版>-<ハッシュの先頭 16 文字>`）。
    pub fn version_dir(&self) -> String {
        format!("{}-{}", env!("CARGO_PKG_VERSION"), &self.sha256[..16])
    }
}

/// 読み出した内容の SHA-256（16 進数の小文字）。
pub fn sha256_hex(r: &mut dyn io::Read) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// 配置先のフォルダ。`agent_dir` が指定されていればそれ（先頭の `~/` はホーム）。
pub fn agent_root(platform: &Platform, agent_dir: Option<&str>) -> Vec<u8> {
    match agent_dir.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => match d.strip_prefix("~/") {
            Some(rest) => yy_proto::join_path(&platform.home, rest.as_bytes()),
            None if d == "~" => platform.home.clone(),
            None => d.as_bytes().to_vec(),
        },
        None => yy_proto::join_path(&platform.home, b".yyeditor/agent"),
    }
}

/// 配置済みのエージェントのハッシュを調べる（なければ `None`）。
fn installed_hash(t: &dyn Transport, exe: &[u8]) -> io::Result<Option<String>> {
    let mut cmd = b"P=".to_vec();
    cmd.extend_from_slice(&shell_quote(exe));
    cmd.extend_from_slice(br#"; test -x "$P" && "$P" --sha256"#);
    let out = run(t, &cmd, b"")?;
    if !out.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text
        .lines()
        .map(str::trim)
        .rfind(|l| l.len() == 64 && l.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_owned))
}

/// エージェントを配置する（同じものが配置済みなら何もしない）。配置したファイルのパスを返す。
pub fn install(
    t: &dyn Transport,
    image: &AgentImage,
    platform: &Platform,
    agent_dir: Option<&str>,
) -> io::Result<Vec<u8>> {
    let dir = yy_proto::join_path(
        &agent_root(platform, agent_dir),
        image.version_dir().as_bytes(),
    );
    let exe = yy_proto::join_path(&dir, b"yy-agent");
    if installed_hash(t, &exe)?.as_deref() == Some(image.sha256.as_str()) {
        return Ok(exe);
    }
    let data = std::fs::read(&image.local)?;
    let mut cmd = b"D=".to_vec();
    cmd.extend_from_slice(&shell_quote(&dir));
    cmd.extend_from_slice(
        br#"; umask 077 && mkdir -p "$D" && cat > "$D/.yy-agent.tmp" && chmod 700 "$D/.yy-agent.tmp" && mv -f "$D/.yy-agent.tmp" "$D/yy-agent""#,
    );
    let out = run(t, &cmd, &data)?;
    if !out.success() {
        return Err(io::Error::other(format!(
            "エージェントを配置できませんでした（{}）: {}",
            yy_proto::display_path(&dir),
            out.message()
        )));
    }
    match installed_hash(t, &exe)? {
        Some(h) if h == image.sha256 => Ok(exe),
        Some(_) => {
            let mut rm = b"rm -f ".to_vec();
            rm.extend_from_slice(&shell_quote(&exe));
            let _ = run(t, &rm, b"");
            Err(io::Error::other(
                "配置したエージェントが壊れています（ハッシュが一致しません）",
            ))
        }
        None => Err(io::Error::other(format!(
            "配置したエージェントを実行できません（{}）。ホームフォルダが noexec の場合は、\
             接続設定の agent_dir で実行できる場所を指定してください",
            yy_proto::display_path(&exe)
        ))),
    }
}

/// 配置したエージェントを起動し、起動の印まで読み進めた状態で返す。
pub fn launch(t: &dyn Transport, exe: &[u8]) -> io::Result<Process> {
    let mut cmd = shell_quote(exe);
    cmd.extend_from_slice(b" serve --stdio");
    let (stdin, mut stdout, finish) = t.exec(&cmd)?.into_parts();
    match yy_proto::skip_to_magic(&mut stdout) {
        Ok(_) => Ok(Process::new(stdin, stdout, finish)),
        Err(e) => {
            drop(stdin);
            let detail = finish()
                .map(|x| String::from_utf8_lossy(x.stderr.trim_ascii()).into_owned())
                .unwrap_or_default();
            Err(io::Error::new(
                e.kind(),
                if detail.is_empty() {
                    e.to_string()
                } else {
                    format!("{e} {detail}")
                },
            ))
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::local::LocalTransport;

    #[test]
    fn probes_the_local_machine() {
        let p = probe(&LocalTransport::new()).unwrap();
        assert_eq!(p.os, "Linux");
        assert!(p.home.starts_with(b"/"));
    }

    #[test]
    fn agent_root_expands_home() {
        let p = Platform {
            os: "Linux".into(),
            arch: "x86_64".into(),
            home: b"/home/u".to_vec(),
        };
        assert_eq!(agent_root(&p, None), b"/home/u/.yyeditor/agent");
        assert_eq!(agent_root(&p, Some("~/x")), b"/home/u/x");
        assert_eq!(agent_root(&p, Some("/work/y")), b"/work/y");
        assert_eq!(agent_root(&p, Some("  ")), b"/home/u/.yyeditor/agent");
    }

    #[test]
    fn missing_agent_file_is_explained() {
        let dir = tempfile::tempdir().unwrap();
        let e = AgentImage::load(&AgentFiles::new(dir.path()), "aarch64").unwrap_err();
        assert!(e.to_string().contains("aarch64 用のエージェント"), "{e}");
    }
}

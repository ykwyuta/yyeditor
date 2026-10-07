//! SCP（13 章 4）。
//!
//! 接続先の `scp -t`（受け取り）・`scp -f`（送り出し）と、SCP の手順（`C` 行・応答の 1 バイト）で
//! 話す。SCP には途中から送る仕組みがないので、レジュームは転送の側（[`crate::xfer`]）で
//! 区切りごとに送って接続先のシェルでつなげる（`cat >>`）。読み出しの途中からは `tail -c +N` を使う。

use std::io::{self, BufRead, BufReader, Read, Write};

use crate::{Transport, run, shell_quote};

/// 応答の 1 バイト（0 なら成功、1・2 なら続く 1 行が説明）を読む。
fn read_ack(r: &mut dyn BufRead, what: &str) -> io::Result<()> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)
        .map_err(|e| io::Error::new(e.kind(), format!("{what}: SCP の応答がありません: {e}")))?;
    match b[0] {
        0 => Ok(()),
        1 | 2 => {
            let mut line = Vec::new();
            r.read_until(b'\n', &mut line)?;
            let msg = String::from_utf8_lossy(line.trim_ascii()).into_owned();
            let kind = if msg.contains("ermission denied") {
                io::ErrorKind::PermissionDenied
            } else if msg.contains("o such file") {
                io::ErrorKind::NotFound
            } else {
                io::ErrorKind::Other
            };
            Err(io::Error::new(kind, format!("{what}: scp: {msg}")))
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{what}: SCP の応答が不正です（{other}）"),
        )),
    }
}

/// `input` から `size` バイトを、接続先の `path`（ファイルのパス）に送る。`progress(送った量)` が
/// `false` を返したら中止する。
pub fn upload(
    t: &dyn Transport,
    path: &[u8],
    mode: u32,
    size: u64,
    input: &mut dyn Read,
    progress: &mut dyn FnMut(u64) -> bool,
) -> io::Result<()> {
    let what = format!("SCP の送信 {}", crate::display(path));
    let mut cmd = b"scp -t ".to_vec();
    cmd.extend_from_slice(&shell_quote(path));
    let (mut stdin, stdout, finish) = t.exec(&cmd)?.into_parts();
    let mut out = BufReader::new(stdout);
    let result = (|| {
        read_ack(&mut out, &what)?;
        let name = path.rsplit(|&b| b == b'/').next().unwrap_or(path);
        let mut header = format!("C{:04o} {size} ", mode & 0o7777).into_bytes();
        header.extend_from_slice(name);
        header.push(b'\n');
        stdin.write_all(&header)?;
        stdin.flush()?;
        read_ack(&mut out, &what)?;
        let mut buf = vec![0u8; 256 << 10];
        let mut sent = 0u64;
        while sent < size {
            let want = buf.len().min((size - sent) as usize);
            let n = input.read(&mut buf[..want])?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("{what}: 手元のファイルが途中で終わりました"),
                ));
            }
            stdin.write_all(&buf[..n])?;
            sent += n as u64;
            if !progress(sent) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "中止しました"));
            }
        }
        stdin.write_all(&[0])?;
        stdin.flush()?;
        read_ack(&mut out, &what)
    })();
    drop(stdin);
    drop(out);
    let exit = finish();
    result?;
    match exit {
        Ok(e) if e.status.is_some_and(|s| s != 0) => Err(io::Error::other(format!(
            "{what}: scp が失敗しました（終了コード {}）{}",
            e.status.unwrap_or(0),
            String::from_utf8_lossy(e.stderr.trim_ascii())
        ))),
        _ => Ok(()),
    }
}

/// 接続先の `path` を `offset` バイト目から `out` に受け取る。最初からなら `scp -f`、途中からは
/// `tail -c +N`。受け取った量を返す。`expected` は残りの大きさ（途中からのときの確認に使う）。
pub fn download(
    t: &dyn Transport,
    path: &[u8],
    offset: u64,
    out: &mut dyn Write,
    progress: &mut dyn FnMut(u64) -> bool,
) -> io::Result<u64> {
    let what = format!("SCP の受信 {}", crate::display(path));
    let quoted = shell_quote(path);
    let mut buf = vec![0u8; 256 << 10];
    if offset > 0 {
        let mut cmd = format!("tail -c +{} ", offset + 1).into_bytes();
        cmd.extend_from_slice(&quoted);
        let (stdin, mut stdout, finish) = t.exec(&cmd)?.into_parts();
        drop(stdin);
        let mut got = 0u64;
        loop {
            let n = stdout.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            got += n as u64;
            if !progress(got) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "中止しました"));
            }
        }
        let e = finish()?;
        if e.status != Some(0) {
            return Err(io::Error::other(format!(
                "{what}: tail が失敗しました {}",
                String::from_utf8_lossy(e.stderr.trim_ascii())
            )));
        }
        return Ok(got);
    }
    let mut cmd = b"scp -f ".to_vec();
    cmd.extend_from_slice(&quoted);
    let (mut stdin, stdout, finish) = t.exec(&cmd)?.into_parts();
    let mut r = BufReader::new(stdout);
    let result = (|| {
        stdin.write_all(&[0])?;
        stdin.flush()?;
        // 時刻の行（T…）は使わない。C 行を待つ
        let size = loop {
            let mut kind = [0u8; 1];
            r.read_exact(&mut kind)?;
            let mut line = Vec::new();
            r.read_until(b'\n', &mut line)?;
            match kind[0] {
                b'T' => {
                    stdin.write_all(&[0])?;
                    stdin.flush()?;
                }
                b'C' => {
                    let text = String::from_utf8_lossy(&line);
                    let size: u64 = text
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| {
                            io::Error::other(format!("{what}: SCP の C 行を読めません"))
                        })?;
                    break size;
                }
                1 | 2 => {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "{what}: scp: {}",
                            String::from_utf8_lossy(line.trim_ascii())
                        ),
                    ));
                }
                _ => return Err(io::Error::other(format!("{what}: SCP の応答が不正です"))),
            }
        };
        stdin.write_all(&[0])?;
        stdin.flush()?;
        let mut got = 0u64;
        while got < size {
            let want = buf.len().min((size - got) as usize);
            let n = r.read(&mut buf[..want])?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("{what}: 途中で切れました（{got} / {size} バイト）"),
                ));
            }
            out.write_all(&buf[..n])?;
            got += n as u64;
            if !progress(got) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "中止しました"));
            }
        }
        read_ack(&mut r, &what)?;
        stdin.write_all(&[0])?;
        stdin.flush()?;
        Ok(got)
    })();
    drop(stdin);
    // 読み口を閉じてから終わりを待つ（scp が書き込みで止まっていても終わるように）
    drop(r);
    let _ = finish();
    result
}

/// 接続先のファイルの大きさ（なければ `None`）。シェルの `wc -c` を使う。
pub fn remote_size(t: &dyn Transport, path: &[u8]) -> io::Result<Option<u64>> {
    let mut cmd = b"if [ -f ".to_vec();
    cmd.extend_from_slice(&shell_quote(path));
    cmd.extend_from_slice(b" ]; then wc -c < ");
    cmd.extend_from_slice(&shell_quote(path));
    cmd.extend_from_slice(b"; else echo none; fi");
    let out = run(t, &cmd, b"")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let text = text.trim();
    if text == "none" {
        return Ok(None);
    }
    text.parse()
        .map(Some)
        .map_err(|_| io::Error::other(format!("大きさを読めません: {}", out.message())))
}

/// 接続先でシェルのコマンドを実行する（失敗したら説明つきのエラー）。
pub fn shell(t: &dyn Transport, cmd: &[u8], what: &str) -> io::Result<()> {
    let out = run(t, cmd, b"")?;
    if out.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{what}: {}", out.message())))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::local::LocalTransport;
    use std::os::unix::ffi::OsStrExt;

    fn has_scp() -> bool {
        std::process::Command::new("sh")
            .args(["-c", "command -v scp"])
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn uploads_and_downloads() {
        if !has_scp() {
            eprintln!("scp がないため飛ばします");
            return;
        }
        let t = LocalTransport::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x y.bin");
        let p = path.as_os_str().as_bytes();
        let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 253) as u8).collect();
        upload(&t, p, 0o644, data.len() as u64, &mut &data[..], &mut |_| {
            true
        })
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), data);
        assert_eq!(remote_size(&t, p).unwrap(), Some(1_000_000));
        assert_eq!(remote_size(&t, b"/nonexistent/zz").unwrap(), None);

        let mut got = Vec::new();
        assert_eq!(
            download(&t, p, 0, &mut got, &mut |_| true).unwrap(),
            1_000_000
        );
        assert_eq!(got, data);
        let mut tail = Vec::new();
        assert_eq!(
            download(&t, p, 999_000, &mut tail, &mut |_| true).unwrap(),
            1000
        );
        assert_eq!(tail, &data[999_000..]);

        let e = download(&t, b"/nonexistent/zz", 0, &mut Vec::new(), &mut |_| true).unwrap_err();
        assert!(e.to_string().contains("scp"), "{e}");
    }
}

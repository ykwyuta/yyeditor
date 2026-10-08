//! ファイルのハッシュ（BLAKE3。18 章 6）。

use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::fs::Fs;

/// ハッシュ（32 バイト）。
pub type Hash = [u8; 32];

/// 先頭・末尾として読む大きさ。
pub const EDGE: u64 = 64 << 10;

/// ファイル全体のハッシュ。`step(読んだバイト数)` が `false` を返したら中止する。
pub fn full(fs: &dyn Fs, path: &Path, step: &mut dyn FnMut(u64) -> bool) -> io::Result<Hash> {
    let mut f = fs.open_read(path)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        h.update(&buf[..n]);
        if !step(n as u64) {
            return Err(crate::cancelled());
        }
    }
    Ok(*h.finalize().as_bytes())
}

/// 大きさと、先頭・末尾の [`EDGE`] バイトのハッシュ（重複の候補を絞る。18 章 6）。
pub fn edges(fs: &dyn Fs, path: &Path, size: u64) -> io::Result<Hash> {
    let mut f = fs.open_read(path)?;
    let mut h = blake3::Hasher::new();
    h.update(&size.to_le_bytes());
    let mut buf = vec![0u8; EDGE.min(size) as usize];
    f.read_exact(&mut buf)?;
    h.update(&buf);
    if size > EDGE {
        let start = size.saturating_sub(EDGE).max(EDGE);
        f.seek(SeekFrom::Start(start))?;
        let mut tail = vec![0u8; (size - start) as usize];
        f.read_exact(&mut tail)?;
        h.update(&tail);
    }
    Ok(*h.finalize().as_bytes())
}

/// バイト列のハッシュ。
pub fn bytes(data: &[u8]) -> Hash {
    *blake3::hash(data).as_bytes()
}

/// 16 進数の表記。
pub fn hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::Local;

    #[test]
    fn hashes() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        let b = d.path().join("b");
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&a, &big).unwrap();
        let mut other = big.clone();
        other[150_000] ^= 1; // 真ん中だけ違う
        std::fs::write(&b, &other).unwrap();
        let fa = full(&Local, &a, &mut |_| true).unwrap();
        assert_eq!(fa, bytes(&big));
        assert_ne!(fa, full(&Local, &b, &mut |_| true).unwrap());
        // 先頭・末尾は同じ
        assert_eq!(
            edges(&Local, &a, big.len() as u64).unwrap(),
            edges(&Local, &b, big.len() as u64).unwrap()
        );
        // 小さなファイル・空
        std::fs::write(&a, b"xyz").unwrap();
        std::fs::write(&b, b"").unwrap();
        assert_ne!(edges(&Local, &a, 3).unwrap(), edges(&Local, &b, 0).unwrap());
        assert!(crate::is_cancelled(
            &full(&Local, &a, &mut |_| false).unwrap_err()
        ));
        assert_eq!(hex(&[0xab; 32]).len(), 64);
    }
}

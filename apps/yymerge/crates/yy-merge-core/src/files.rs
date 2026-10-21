//! Bounded snapshots, explicit encodings, lossless round trips and atomic output.
use crate::{Cancellation, MAX_BYTES, Result};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use yy_encoding::{Encoding, EscapeMode, decode_all, encode_all};

#[derive(Clone)]
pub struct Snapshot {
    pub path: PathBuf,
    pub text: Arc<str>,
    pub encoding: Encoding,
    pub bom: Vec<u8>,
    bytes: Vec<u8>,
}
pub fn read_bounded(path: &Path, cancel: &Cancellation) -> Result<Vec<u8>> {
    cancel.check()?;
    let meta = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err(format!(
            "{}: regular files up to 32 MiB only",
            path.display()
        ));
    }
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    let mut block = [0; 64 * 1024];
    loop {
        cancel.check()?;
        let n = file.read(&mut block).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > MAX_BYTES as usize {
            return Err("file grew beyond 32 MiB limit".into());
        }
        bytes.extend_from_slice(&block[..n]);
    }
    Ok(bytes)
}
impl Snapshot {
    /// No heuristic encoding guesses. BOM, otherwise UTF-8; legacy encodings explicit.
    pub fn open(path: &Path, charset: Option<&str>, cancel: &Cancellation) -> Result<Self> {
        let bytes = read_bounded(path, cancel)?;
        let (auto, bom_len) = if bytes.starts_with(&[0xff, 0xfe, 0, 0]) {
            (Encoding::Utf32Le, 4)
        } else if bytes.starts_with(&[0, 0, 0xfe, 0xff]) {
            (Encoding::Utf32Be, 4)
        } else if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
            (Encoding::Utf8, 3)
        } else if bytes.starts_with(&[0xff, 0xfe]) {
            (Encoding::Utf16Le, 2)
        } else if bytes.starts_with(&[0xfe, 0xff]) {
            (Encoding::Utf16Be, 2)
        } else {
            (Encoding::Utf8, 0)
        };
        let encoding = match charset {
            Some(name) => {
                Encoding::from_name(name).ok_or_else(|| format!("unknown encoding: {name}"))?
            }
            None => auto,
        };
        if bom_len > 0 && encoding != auto {
            return Err("encoding conflicts with BOM".into());
        }
        let (decoded, _) = decode_all(encoding, &bytes[bom_len..], false);
        let text = String::from_utf8(decoded).map_err(|_| "invalid text encoding")?;
        if text.contains('\0')
            || encode_all(encoding, text.as_bytes(), EscapeMode::Reject)
                .map_err(|_| "invalid text encoding")?
                != bytes[bom_len..]
        {
            return Err(format!(
                "{}: binary, invalid or non-round-trip encoding; specify --encoding for legacy text",
                path.display()
            ));
        }
        Ok(Self {
            path: fs::canonicalize(path).map_err(|e| e.to_string())?,
            text: text.into(),
            encoding,
            bom: bytes[..bom_len].to_vec(),
            bytes,
        })
    }
    pub fn encode(&self, text: &str) -> Result<Vec<u8>> {
        let mut out = self.bom.clone();
        out.extend(
            encode_all(self.encoding, text.as_bytes(), EscapeMode::Reject)
                .map_err(|_| format!("text cannot be represented in {}", self.encoding))?,
        );
        Ok(out)
    }
    pub fn verify(&self, cancel: &Cancellation) -> Result<()> {
        if read_bounded(&self.path, cancel)? != self.bytes {
            return Err(format!("input changed: {}", self.path.display()));
        }
        Ok(())
    }
}

/// Never follows an output symlink. Input snapshots are revalidated before commit.
pub fn save(
    path: &Path,
    bytes: &[u8],
    inputs: &[&Snapshot],
    overwrite: bool,
    cancel: &Cancellation,
) -> Result<()> {
    cancel.check()?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("output exceeds the initial 32 MiB limit".into());
    }
    let parent = fs::canonicalize(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )
    .map_err(|e| e.to_string())?;
    let target = parent.join(path.file_name().ok_or("output file name required")?);
    let previous = match fs::symlink_metadata(&target) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_file() {
                return Err("output must be a regular file, not a link".into());
            }
            if inputs
                .iter()
                .any(|s| s.path == fs::canonicalize(&target).unwrap_or_default())
            {
                return Err("output must differ from input paths".into());
            }
            if !overwrite {
                return Err("output already exists; use --overwrite".into());
            }
            Some(read_bounded(&target, cancel)?)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temp.write_all(bytes)
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    for input in inputs {
        input.verify(cancel)?;
    }
    cancel.check()?;
    if let Some(old) = previous {
        if fs::symlink_metadata(&target)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
            || read_bounded(&target, cancel)? != old
        {
            return Err("output changed during operation".into());
        }
        temp.persist(target).map_err(|e| e.to_string())?;
    } else {
        temp.persist_noclobber(target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encodings_preserve_bytes_and_save_is_guarded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("日本語.txt");
        for encoding in [Encoding::Utf8, Encoding::Utf16Le, Encoding::Cp932] {
            let data =
                encode_all(encoding, "日本語\r\n末尾".as_bytes(), EscapeMode::Reject).unwrap();
            fs::write(&path, &data).unwrap();
            let s = Snapshot::open(&path, Some(encoding.name()), &Cancellation::default()).unwrap();
            assert_eq!(s.encode(&s.text).unwrap(), data);
            assert!(save(&path, b"oops", &[&s], true, &Cancellation::default()).is_err());
            let out = dir.path().join("out");
            fs::write(&out, b"keep").unwrap();
            assert!(save(&out, b"oops", &[&s], false, &Cancellation::default()).is_err());
            fs::write(&path, b"changed").unwrap();
            assert!(save(&out, b"oops", &[&s], true, &Cancellation::default()).is_err());
            assert_eq!(fs::read(&out).unwrap(), b"keep");
        }
    }
    #[test]
    fn bom_and_binary() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        for bytes in [
            b"\xef\xbb\xbfhello\r\n".to_vec(),
            vec![255, 254, 65, 0, 10, 0],
        ] {
            fs::write(&p, &bytes).unwrap();
            let s = Snapshot::open(&p, None, &Cancellation::default()).unwrap();
            assert_eq!(s.encode(&s.text).unwrap(), bytes);
        }
        fs::write(&p, b"a\0b").unwrap();
        assert!(Snapshot::open(&p, None, &Cancellation::default()).is_err());
    }
}

//! Trust boundaries for preview resources and external links.
use std::io;
use std::path::{Path, PathBuf};

/// Decode exactly once and validate after decoding. Encoded separators are never filenames.
pub fn document_relative_path(raw: &str) -> Option<String> {
    let raw = raw.split(['?', '#']).next()?;
    let mut out = Vec::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let byte = if bytes[i] == b'%' {
            let h = (*bytes.get(i + 1)? as char).to_digit(16)?;
            let l = (*bytes.get(i + 2)? as char).to_digit(16)?;
            i += 3;
            let byte = (h * 16 + l) as u8;
            if matches!(byte, b'/' | b'\\') {
                return None;
            }
            byte
        } else {
            let byte = bytes[i];
            i += 1;
            byte
        };
        out.push(byte);
    }
    let path = String::from_utf8(out).ok()?;
    if !safe_relative_path(&path) {
        return None;
    }
    Some(path)
}

fn safe_relative_path(path: &str) -> bool {
    !path.split('/').any(|part| {
        part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part.chars().any(|c| {
                c.is_control() || matches!(c, '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
            })
    })
}

/// Resolve links before reading; a symlink outside the document folder is not a resource.
pub fn local_document_file(root: &Path, relative: &str) -> io::Result<PathBuf> {
    if !safe_relative_path(relative) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "不正なプレビュー資源のパスです",
        ));
    }
    let root = root.canonicalize()?;
    let path = root.join(relative).canonicalize()?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "文書フォルダ外の資源は開けません",
        ));
    }
    Ok(path)
}

pub fn remote_path_is_within(root: &[u8], path: &[u8]) -> bool {
    let root = if root == b"/" {
        root
    } else {
        root.strip_suffix(b"/").unwrap_or(root)
    };
    root.starts_with(b"/")
        && (path == root
            || path
                .strip_prefix(root)
                .is_some_and(|rest| root == b"/" || rest.starts_with(b"/")))
}

/// Only explicit user gestures may leave the preview. Executable types are never shell-opened.
pub fn may_open_external_file(path: &Path, user_initiated: bool) -> bool {
    user_initiated
        && matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some(
                "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "webp"
                    | "bmp"
                    | "ico"
                    | "avif"
                    | "pdf"
                    | "mp4"
                    | "webm"
                    | "mp3"
                    | "wav"
                    | "ogg"
                    | "docx"
                    | "xlsx"
                    | "pptx"
            )
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_traversal_after_decoding() {
        for p in [
            "../x",
            "%2e%2e/x",
            "%2e%2e%2fx",
            "..%2Fx",
            "%2fetc/passwd",
            "%5csecret",
            "a\\..\\x",
            "C:secret",
            "/root",
            "a//b",
            "a/%00x",
            "x%",
            "x%zz",
            "a/.",
            "a/..",
            "a/",
            "x.",
        ] {
            assert!(document_relative_path(p).is_none(), "{p}");
        }
        assert_eq!(
            document_relative_path("images/%E6%97%A5%E6%9C%AC.png?q=x"),
            Some("images/日本.png".into())
        );
    }
    #[test]
    fn external_launch_requires_a_gesture_and_a_document_type() {
        for ext in [
            "exe", "bat", "cmd", "lnk", "url", "ps1", "vbs", "hta", "msi", "scr",
        ] {
            assert!(!may_open_external_file(
                Path::new(&format!("payload.{ext}")),
                true
            ));
        }
        assert!(!may_open_external_file(Path::new("image.png"), false));
        assert!(may_open_external_file(Path::new("image.PNG"), true));
    }
    #[test]
    fn remote_roots_require_a_component_boundary() {
        assert!(remote_path_is_within(
            b"/home/u/docs",
            b"/home/u/docs/images/a.png"
        ));
        assert!(!remote_path_is_within(
            b"/home/u/docs",
            b"/home/u/docs-other/a.png"
        ));
        assert!(!remote_path_is_within(b"/home/u/docs", b"/home/u/secret"));
        assert!(remote_path_is_within(b"/", b"/images/a.png"));
    }

    #[test]
    fn local_resources_stay_inside_the_document_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("docs");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("100%.txt"), "inside").unwrap();
        std::fs::write(dir.path().join("secret.txt"), "outside").unwrap();
        assert!(local_document_file(&root, "100%.txt").is_ok());
        assert!(local_document_file(&root, "../secret.txt").is_err());
        assert!(local_document_file(&root, "..").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("secret.txt"), root.join("link.txt"))
                .unwrap();
            assert!(local_document_file(&root, "link.txt").is_err());
        }
    }
}

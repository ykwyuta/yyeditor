//! クリップボード履歴の保存と、Control 二連続の判定。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const MAX_BYTES: usize = 1024 * 1024;
pub(crate) const MENU_LIMIT: usize = 100;
const DOUBLE_CONTROL_MS: u32 = 500;
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn default_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("yyclip").join("history"))
}

pub(crate) struct Store {
    dir: PathBuf,
    last_text: Option<String>,
    suppress_text: Option<String>,
}

impl Store {
    pub(crate) fn new(dir: PathBuf) -> Self {
        let last_text = history_files(&dir)
            .into_iter()
            .find_map(|p| read_text(&p).ok());
        Self {
            dir,
            last_text,
            suppress_text: None,
        }
    }

    pub(crate) fn observe(&mut self, text: String) -> io::Result<bool> {
        if self.suppress_text.take().as_deref() == Some(&text)
            || self.last_text.as_deref() == Some(&text)
        {
            return Ok(false);
        }
        save_text(&self.dir, &text)?;
        self.last_text = Some(text);
        Ok(true)
    }

    pub(crate) fn suppress_next(&mut self, text: String) {
        self.suppress_text = Some(text);
    }

    pub(crate) fn cancel_suppression(&mut self) {
        self.suppress_text = None;
    }

    pub(crate) fn previews(&self) -> Vec<(PathBuf, String)> {
        history_files(&self.dir)
            .into_iter()
            .take(MENU_LIMIT)
            .filter_map(|path| preview(&path).ok().map(|s| (path, s)))
            .collect()
    }

    pub(crate) fn load(&self, path: &Path) -> io::Result<String> {
        read_text(path)
    }
}

fn history_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("txt")))
        .collect();
    files.sort_unstable_by(|a, b| b.file_name().cmp(&a.file_name()));
    files
}

fn read_text(path: &Path) -> io::Result<String> {
    let size = path.metadata()?.len();
    if size == 0 || size > MAX_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "履歴のサイズが対象外",
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "履歴のサイズが対象外",
        ));
    }
    String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn preview(path: &Path) -> io::Result<String> {
    let size = path.metadata()?.len();
    if size == 0 || size > MAX_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "履歴のサイズが対象外",
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(384).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn save_text(dir: &Path, text: &str) -> io::Result<()> {
    if text.is_empty() || text.len() > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "履歴のサイズが対象外",
        ));
    }
    fs::create_dir_all(dir)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let serial = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    let name = format!("{now:020}-{:010}-{serial:016}.txt", std::process::id());
    let final_path = dir.join(&name);
    let temp_path = dir.join(format!("{name}.tmp"));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(text.as_bytes())?;
        drop(file);
        fs::rename(&temp_path, final_path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp_path);
    }
    result
}

pub(crate) fn label(text: &str) -> String {
    let start = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(72)
        .collect::<String>();
    let mut label = start.trim().to_owned();
    if text.chars().count() > 72 {
        label.push('…');
    }
    if label.is_empty() {
        "(空白)".to_owned()
    } else {
        label
    }
}

#[derive(Default)]
pub(crate) struct ControlSequence {
    held: u8,
    tainted: bool,
    fired: bool,
    released_at: Option<u32>,
}

impl ControlSequence {
    /// 左・右・汎用 Control の仮想キーをそれぞれ 1・2・4 として渡す。
    pub(crate) fn event(&mut self, control_bit: u8, down: bool, time: u32) -> bool {
        if control_bit == 0 {
            if down {
                self.released_at = None;
                if self.held != 0 {
                    self.tainted = true;
                }
            }
            return false;
        }
        if down {
            let was_held = self.held != 0;
            self.held |= control_bit;
            if was_held {
                return false;
            }
            self.tainted = false;
            if self
                .released_at
                .take()
                .is_some_and(|last| time.wrapping_sub(last) <= DOUBLE_CONTROL_MS)
            {
                self.fired = true;
                return true;
            }
        } else {
            let was_held = self.held != 0;
            self.held &= !control_bit;
            if was_held && self.held == 0 {
                self.released_at = (!self.tainted && !self.fired).then_some(time);
                self.fired = false;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_control_needs_release_and_no_other_key() {
        let mut keys = ControlSequence::default();
        assert!(!keys.event(1, true, 100));
        assert!(!keys.event(1, true, 120));
        assert!(!keys.event(1, false, 150));
        assert!(keys.event(2, true, 300));
        assert!(!keys.event(2, false, 320));
        assert!(!keys.event(1, true, 350));
        assert!(!keys.event(1, false, 370));
        keys.event(0, true, 400);
        assert!(!keys.event(1, true, 420));
    }

    #[test]
    fn unmatched_release_and_expired_tap_do_not_trigger() {
        let mut keys = ControlSequence::default();
        keys.event(1, false, 10);
        assert!(!keys.event(1, true, 100));
        keys.event(1, false, 150);
        assert!(!keys.event(1, true, 651));
    }

    #[test]
    fn store_persists_utf8_and_enforces_limit() {
        let dir = std::env::temp_dir().join(format!(
            "yyclip-test-{}-{}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut store = Store::new(dir.clone());
        let text = "あ".repeat(100);
        assert!(store.observe(text.clone()).unwrap());
        assert!(!store.observe(text.clone()).unwrap());
        assert_eq!(store.load(&store.previews()[0].0).unwrap(), text);
        store.suppress_next("復元".to_owned());
        assert!(!store.observe("復元".to_owned()).unwrap());
        assert!(store.observe("x".repeat(MAX_BYTES)).unwrap());
        assert!(store.observe("x".repeat(MAX_BYTES + 1)).is_err());
        assert_eq!(Store::new(dir.clone()).previews().len(), 2);
        fs::remove_dir_all(dir).unwrap();
    }
}

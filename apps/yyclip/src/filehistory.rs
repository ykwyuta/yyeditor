//! 現在のユーザーの Recent ショートカットを監視する。通常のファイル I/O は監視しない。
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Sender},
    time::{Duration, SystemTime},
};
use windows::{
    Win32::{
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ},
        UI::Shell::{
            FOLDERID_Recent, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink,
        },
    },
    core::{HSTRING, Interface},
};

pub(crate) const DISPLAY_LIMIT: usize = 100;
const LOG_LIMIT: usize = 1000;

pub(crate) fn log_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("yyclip/file-history.log"))
}

fn key(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_lowercase()
}

pub(crate) fn load(log: &Path) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && seen.insert(key(p)))
        .take(LOG_LIMIT)
        .collect()
}

struct Store {
    log: PathBuf,
    entries: Vec<PathBuf>,
}

impl Store {
    fn record(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> io::Result<bool> {
        let mut entries = self.entries.clone();
        for path in paths {
            // 一行一パスの形式を保つ。Recent のディレクトリや URL は対象外。
            if !path.is_absolute() || path.to_string_lossy().contains(['\r', '\n']) {
                continue;
            }
            let k = key(&path);
            entries.retain(|p| key(p) != k);
            entries.insert(0, path);
            entries.truncate(LOG_LIMIT);
        }
        if entries == self.entries {
            return Ok(false);
        }
        let text = entries
            .iter()
            .map(|p| format!("{}\n", p.display()))
            .collect::<String>();
        if let Some(parent) = self.log.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.log.with_extension("tmp");
        fs::write(&tmp, text)?;
        fs::rename(tmp, &self.log)?;
        self.entries = entries;
        Ok(true)
    }
}

fn recent_dir() -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Recent, KF_FLAG_DEFAULT, None).ok()?;
        let path = p.to_string().ok().map(PathBuf::from);
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0.cast()));
        path
    }
}

fn target(shortcut: &Path) -> Option<PathBuf> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        file.Load(&HSTRING::from(shortcut.as_os_str()), STGM_READ)
            .ok()?;
        let mut path = vec![0u16; 32768];
        // Resolve は呼ばない。移動先検索やダイアログを発生させない。
        link.GetPath(&mut path, std::ptr::null_mut(), 0).ok()?;
        let end = path.iter().position(|c| *c == 0)?;
        use std::os::windows::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_wide(&path[..end]));
        path.is_file().then_some(path)
    }
}

struct Watcher {
    dir: PathBuf,
    seen: HashMap<PathBuf, SystemTime>,
    since: SystemTime,
}

impl Watcher {
    fn scan(&mut self, store: &mut Store) -> io::Result<bool> {
        let mut links = fs::read_dir(&self.dir)?
            .filter_map(Result::ok)
            .filter_map(|e| {
                let path = e.path();
                if !path
                    .extension()
                    .is_some_and(|s| s.eq_ignore_ascii_case("lnk"))
                {
                    return None;
                }
                Some((path, e.metadata().ok()?.modified().ok()?))
            })
            .collect::<Vec<_>>();
        links.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        let mut seen = HashMap::new();
        let mut paths = Vec::new();
        for (link, time) in links {
            if time > self.since && self.seen.get(&link) != Some(&time) {
                let Some(path) = target(&link) else {
                    // 書き込み途中や一時的に利用できないリンクは次回も試す。
                    continue;
                };
                paths.push(path);
            }
            seen.insert(link, time);
        }
        let changed = store.record(paths)?;
        self.seen = seen;
        Ok(changed)
    }
}

/// 監視とリンク解決は UI スレッドから分離。Sender の破棄で終了する。
pub(crate) fn start(log: PathBuf) -> Sender<PathBuf> {
    let (send, recv) = mpsc::channel();
    std::thread::spawn(move || {
        let Some(_apartment) = super::resident::ComApartment::new() else {
            return;
        };
        let since = fs::metadata(&log)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let mut store = Store {
            entries: load(&log),
            log,
        };
        let mut watcher = recent_dir().map(|dir| Watcher {
            dir,
            seen: HashMap::new(),
            since,
        });
        loop {
            if let Some(watcher) = watcher.as_mut()
                && let Err(e) = watcher.scan(&mut store)
            {
                eprintln!("ファイル履歴の監視: {e}");
            }
            match recv.recv_timeout(Duration::from_secs(2)) {
                Ok(path) => {
                    if let Err(e) = store.record([path]) {
                        eprintln!("ファイル履歴の保存: {e}");
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    send
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn watches_shortcuts_without_recording_background_file_reads() {
        let _apartment = super::super::resident::ComApartment::new().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "yyclip-recent-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let recent = dir.join("Recent");
        fs::create_dir(&recent).unwrap();
        let file = dir.join("日本語 file.txt");
        fs::write(&file, "test").unwrap();
        let link_path = recent.join("document.lnk");
        unsafe {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            link.SetPath(&HSTRING::from(file.as_os_str())).unwrap();
            let persist: IPersistFile = link.cast().unwrap();
            persist
                .Save(&HSTRING::from(link_path.as_os_str()), true)
                .unwrap();
        }
        // TEMP は CI では RUNNER~1 のような短いパスの場合がある。
        // ShellLink は長いパスを返すので、同じ実ファイルを指すことを確かめる。
        let resolved = target(&link_path).unwrap();
        assert_eq!(
            resolved.canonicalize().unwrap(),
            file.canonicalize().unwrap()
        );
        let file = resolved;
        let log = dir.join("history.log");
        let mut store = Store {
            log: log.clone(),
            entries: vec![],
        };
        let mut watcher = Watcher {
            dir: recent,
            seen: HashMap::new(),
            since: SystemTime::UNIX_EPOCH,
        };
        assert!(watcher.scan(&mut store).unwrap());
        assert_eq!(load(&log), vec![file.clone()]);
        fs::read(&file).unwrap();
        assert!(!watcher.scan(&mut store).unwrap());
        // 次のユーザー操作で同じリンクが更新されたら先頭に戻す。
        let second = dir.join("second.txt");
        fs::write(&second, "second").unwrap();
        store.record([second.clone()]).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&link_path)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(2))
            .unwrap();
        assert!(watcher.scan(&mut store).unwrap());
        assert_eq!(load(&log), vec![file, second]);
        // 再起動時、保存済みログより古い Recent で順番を上書きしない。
        watcher.since = SystemTime::now() + Duration::from_secs(4);
        watcher.seen.clear();
        assert!(!watcher.scan(&mut store).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unique_full_paths_limits_and_restart() {
        let dir = std::env::temp_dir().join(format!(
            "yyclip-files-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let log = dir.join("file-history.log");
        let mut store = Store {
            log: log.clone(),
            entries: vec![],
        };
        store
            .record((0..1005).map(|n| PathBuf::from(format!("C:\\日本語 folder\\{n}.txt"))))
            .unwrap();
        assert_eq!(store.entries.len(), 1000);
        assert_eq!(load(&log), store.entries);
        assert_eq!(load(&log).iter().take(DISPLAY_LIMIT).count(), 100);
        let duplicate = PathBuf::from("c:/日本語 folder/1000.txt");
        store
            .record([duplicate.clone(), PathBuf::from("relative.txt")])
            .unwrap();
        assert_eq!(store.entries[0], duplicate);
        assert_eq!(store.entries.len(), 1000);
        assert_eq!(
            store
                .entries
                .iter()
                .filter(|p| key(p) == key(&duplicate))
                .count(),
            1
        );
        assert!(!store.record([duplicate]).unwrap());
        let text = fs::read_to_string(log).unwrap();
        assert_eq!(text.lines().count(), 1000);
        assert!(text.lines().all(|s| Path::new(s).is_absolute()));
        fs::remove_dir_all(dir).unwrap();
    }
}

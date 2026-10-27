//! タブごとの表示テキストのログ。描画・スクロール位置は参照しない。

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::os::windows::fs::OpenOptionsExt;

use super::*;

pub(super) struct TextLog {
    pub path: PathBuf,
    writer: BufWriter<File>,
    /// 3270 はホストの画面更新ごとの変更行を記録する。VT のログは改行の確定が契機。
    screen: Vec<String>,
}

impl TextLog {
    pub fn create(path: &Path, screen: Vec<String>) -> io::Result<Self> {
        // 閲覧は許可するが、ほかのタブ・プロセスによる同時書き込みや切り詰めを許可しない。
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .share_mode(windows::Win32::Storage::FileSystem::FILE_SHARE_READ.0)
            .open(path)?;
        Ok(Self {
            path: path.to_owned(),
            writer: BufWriter::new(file),
            screen,
        })
    }

    fn write(&mut self, text: &str) -> io::Result<()> {
        if !text.is_empty() {
            self.writer.write_all(text.as_bytes())?;
            self.writer.flush()?;
        }
        Ok(())
    }

    fn snapshot(&mut self, term: &Terminal) -> io::Result<()> {
        let screen = snapshot(term);
        let mut text = String::new();
        for (row, line) in screen.iter().enumerate() {
            if self.screen.get(row) != Some(line) {
                text.push_str(line);
                text.push('\n');
            }
        }
        self.write(&text)?;
        self.screen = screen;
        Ok(())
    }
}

fn snapshot(term: &Terminal) -> Vec<String> {
    (0..term.rows())
        .map(|r| term.screen_row(r).text())
        .collect()
}

impl Tab {
    pub(super) fn start_log(&mut self, path: &Path) -> io::Result<()> {
        if self.exited.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "終了済みのタブではログ出力を開始できません",
            ));
        }
        if self.log.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "このタブはすでにログを出力中です",
            ));
        }
        let screen = if self.tn.is_some() {
            snapshot(&self.term)
        } else {
            Vec::new()
        };
        let log = TextLog::create(path, screen)?;
        if self.tn.is_none() {
            self.term.start_text_capture();
        }
        self.log = Some(log);
        Ok(())
    }

    pub(super) fn write_log(&mut self) -> io::Result<()> {
        let Some(log) = &mut self.log else {
            return Ok(());
        };
        if self.tn.is_some() {
            log.snapshot(&self.term)
        } else {
            log.write(&self.term.take_text_capture())
        }
    }

    pub(super) fn stop_log(&mut self) -> io::Result<Option<PathBuf>> {
        let Some(mut log) = self.log.take() else {
            return Ok(None);
        };
        if self.tn.is_some() {
            log.snapshot(&self.term)?;
        } else {
            log.write(&self.term.stop_text_capture())?;
        }
        log.writer.flush()?;
        Ok(Some(log.path))
    }
}

pub(super) fn toggle(owner: HWND, tab_id: u64) {
    let target = with(|a| {
        a.tabs
            .iter()
            .find(|t| t.id == tab_id)
            .map(|t| (t.title(), t.log.is_some()))
    })
    .flatten();
    let Some((title, logging)) = target else {
        return;
    };
    if logging {
        let result = with(|a| {
            a.tabs
                .iter_mut()
                .find(|t| t.id == tab_id)
                .map(Tab::stop_log)
        })
        .flatten();
        match result {
            Some(Ok(Some(path))) => {
                set_status(&format!("ログ出力を停止しました: {}", path.display()))
            }
            Some(Err(e)) => error_box(owner, &format!("ログを保存できませんでした。\n{e}")),
            _ => {}
        }
    } else {
        let Some(path) = pick_path(owner, &title) else {
            return;
        };
        let result = with(|a| {
            a.tabs
                .iter_mut()
                .find(|t| t.id == tab_id)
                .map(|t| t.start_log(&path))
        })
        .flatten();
        match result {
            Some(Ok(())) => {
                set_status(&format!("テキストログを出力中: {}", path.display()));
            }
            Some(Err(e)) => error_box(owner, &format!("ログファイルを開けませんでした。\n{e}")),
            None => {}
        }
    }
    with(|a| {
        if let Some(i) = a.tabs.iter().position(|t| t.id == tab_id) {
            a.update_tab_label(i);
        }
    });
}

fn pick_path(owner: HWND, title: &str) -> Option<PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{FileSaveDialog, IFileSaveDialog, SIGDN_FILESYSPATH};
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = [COMDLG_FILTERSPEC {
            pszName: w!("テキストログ (*.txt)"),
            pszSpec: w!("*.txt"),
        }];
        let _ = dialog.SetFileTypes(&filters);
        let _ = dialog.SetDefaultExtension(w!("txt"));
        let title: String = title
            .chars()
            .map(|c| {
                if c.is_control() || "<>:\"/\\|?*".contains(c) {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let _ = dialog.SetFileName(&windows::core::HSTRING::from(format!("yyterm-{title}.txt")));
        let _ = dialog.SetTitle(w!("このタブのテキストログの保存先"));
        dialog.Show(Some(owner)).ok()?;
        let result = dialog.GetResult().ok()?;
        let path = result.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const _));
        text.map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::time::{Duration, Instant};

    struct Reader(crossbeam_channel::Receiver<Vec<u8>>);
    impl Read for Reader {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let data = self.0.recv().unwrap_or_default();
            assert!(data.len() <= out.len());
            out[..data.len()].copy_from_slice(&data);
            Ok(data.len())
        }
    }

    fn backend() -> (pty::Backend, crossbeam_channel::Sender<Vec<u8>>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let close = tx.clone();
        (
            pty::Backend {
                input: Box::new(io::sink()),
                output: Box::new(Reader(rx)),
                resize: Box::new(|_, _| {}),
                wait: Box::new(|| Some(0)),
                kill: Box::new(move || {
                    let _ = close.send(Vec::new());
                }),
            },
            tx,
        )
    }

    fn pump_until(condition: impl Fn() -> bool) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if condition() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("terminal output was not logged within the deadline");
    }

    #[test]
    fn logs_are_off_by_default_isolated_per_tab_and_unaffected_by_scrolling() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );
        }
        QUIET.with(|q| q.set(true));
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one.txt");
        let two = dir.path().join("two.txt");
        let frame = create(
            Place::Local(None),
            None,
            Some(vec!["cmd.exe".into(), "/c".into(), "exit".into()]),
        )
        .unwrap();
        let (b1, tx1) = backend();
        let (b2, tx2) = backend();
        let id1 = with(|a| {
            a.add_tab(Place::Local(None), b1, None);
            a.add_tab(Place::Local(None), b2, None);
            assert!(a.tabs.iter().all(|t| t.log.is_none()));
            a.tabs[1].start_log(&one).unwrap();
            // 別タブで同じファイルを選んでも、書き込み中の内容を切り詰めない。
            assert!(a.tabs[2].start_log(&one).is_err());
            a.tabs[2].start_log(&two).unwrap();
            a.tabs[1].id
        })
        .unwrap();
        tx1.send(b"\x1b[31mone\x1b[0m\r\n".to_vec()).unwrap();
        tx2.send("二番目\r\n".as_bytes().to_vec()).unwrap();
        pump_until(|| {
            std::fs::read_to_string(&one).unwrap_or_default() == "one\n"
                && std::fs::read_to_string(&two).unwrap_or_default() == "二番目\n"
        });
        with(|a| {
            a.activate(1);
            for _ in 0..5 {
                a.scroll_view(10);
                a.scroll_view(-10);
                a.paint();
                a.write_text_logs();
            }
            a.activate(2);
        });
        assert_eq!(std::fs::read_to_string(&one).unwrap(), "one\n");
        tx1.send(b"10%\r\x1b[2K100%\r\npartial-one".to_vec())
            .unwrap();
        tx2.send(b"last-two".to_vec()).unwrap();
        pump_until(|| {
            std::fs::read_to_string(&one).unwrap_or_default() == "one\n100%\n"
                && with(|a| a.tabs[2].term.screen_text().contains("last-two")) == Some(true)
        });
        // 選択中のタブとは別のタブを停止する（右クリックで渡すタブ ID を使う）。
        toggle(frame, id1);
        assert_eq!(with(|a| a.active), Some(2));
        assert_eq!(with(|a| a.tabs[2].log.is_some()), Some(true));
        assert_eq!(
            std::fs::read_to_string(&one).unwrap(),
            "one\n100%\npartial-one\n"
        );
        with(|a| a.close_tabs(0, &[1, 2]));
        assert_eq!(std::fs::read_to_string(&two).unwrap(), "二番目\nlast-two\n");
        unsafe {
            let _ = DestroyWindow(frame);
        }
        shutdown();
    }
}

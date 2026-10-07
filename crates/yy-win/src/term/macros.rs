//! 3270 のマクロの実行（14 章 13 節）: スクリプトは別のスレッドで動き、画面を読む・キーを送る
//! などの操作は UI のスレッドに頼む（[`UiHost`]）。UI のスレッドはタブの画面が変わるたびに
//! 世代を進めて、待っているスクリプトを起こす。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use yy_3270_macro::{Answer, Host, MacroError, Op, Snapshot};

/// UI のスレッドへの頼みごと。
pub(crate) enum Request {
    Snapshot(Sender<Result<Snapshot, String>>),
    Act(Op, Sender<Result<Answer, String>>),
    /// スクリプトが終わった
    Finished(Result<(), MacroError>),
}

/// 画面の世代（変わるたびに増える）。
#[derive(Default)]
pub(crate) struct Generation {
    n: Mutex<u64>,
    cv: Condvar,
}

impl Generation {
    pub(crate) fn bump(&self) {
        *self.n.lock().unwrap() += 1;
        self.cv.notify_all();
    }
}

/// 実行中のマクロ（UI のスレッドが持つ）。
pub(crate) struct MacroRun {
    /// 動かしているタブ
    pub tab_id: u64,
    pub name: String,
    pub stop: Arc<AtomicBool>,
    pub generation: Arc<Generation>,
    pub rx: Receiver<Request>,
    /// 終わりを待っている IND$FILE の転送の返事
    pub transfer_reply: Option<Sender<Result<Answer, String>>>,
}

/// スクリプトのスレッドから見たホスト。
struct UiHost {
    tx: Sender<Request>,
    frame: isize,
    message: u32,
    stop: Arc<AtomicBool>,
    generation: Arc<Generation>,
}

impl UiHost {
    fn post(&self) {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.frame as *mut _)),
                self.message,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

const GONE: &str = "タブが閉じられました";

impl Host for UiHost {
    fn snapshot(&self) -> Result<Snapshot, String> {
        let (tx, rx) = channel();
        self.tx.send(Request::Snapshot(tx)).map_err(|_| GONE)?;
        self.post();
        rx.recv().map_err(|_| GONE.to_owned())?
    }

    fn generation(&self) -> u64 {
        *self.generation.n.lock().unwrap()
    }

    fn wait_change(&self, since: u64, timeout: Duration) {
        let g = self.generation.n.lock().unwrap();
        let _ = self.generation.cv.wait_timeout_while(g, timeout, |n| {
            *n == since && !self.stop.load(Ordering::Relaxed)
        });
    }

    fn act(&self, op: Op) -> Result<Answer, String> {
        let (tx, rx) = channel();
        self.tx.send(Request::Act(op, tx)).map_err(|_| GONE)?;
        self.post();
        // 転送は終わるまで返事が来ない。止められたら待つのをやめる
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(r) => return r,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if self.stop.load(Ordering::Relaxed) {
                        return Err("停止しました".into());
                    }
                }
                Err(_) => return Err(GONE.into()),
            }
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// マクロを別のスレッドで始める。
pub(crate) fn start(
    tab_id: u64,
    name: &str,
    script: String,
    out_dir: PathBuf,
    timeout: u64,
    frame: HWND,
    message: u32,
) -> MacroRun {
    let (tx, rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let generation = Arc::new(Generation::default());
    let frame_raw = frame.0 as isize;
    let host = UiHost {
        tx: tx.clone(),
        frame: frame.0 as isize,
        message,
        stop: stop.clone(),
        generation: generation.clone(),
    };
    std::thread::spawn(move || {
        let host: Arc<dyn Host> = Arc::new(host);
        let r = yy_3270_macro::run(
            &script,
            host.clone(),
            yy_3270_macro::Options { out_dir, timeout },
        );
        let _ = tx.send(Request::Finished(r));
        unsafe {
            let _ = PostMessageW(
                Some(HWND(frame_raw as *mut _)),
                message,
                WPARAM(0),
                LPARAM(0),
            );
        }
    });
    MacroRun {
        tab_id,
        name: name.to_owned(),
        stop,
        generation,
        rx,
        transfer_reply: None,
    }
}

/// マクロのフォルダ（既定は `%APPDATA%\yyeditor\macros`）。
pub(crate) fn macro_folder(configured: &str) -> Option<PathBuf> {
    if !configured.trim().is_empty() {
        return Some(PathBuf::from(configured.trim()));
    }
    yy_config::config_dir().map(|d| d.join("macros"))
}

/// マクロの出力のフォルダ（既定はドキュメントの `yyterm\macros-out`）。
pub(crate) fn output_folder(configured: &str) -> PathBuf {
    if !configured.trim().is_empty() {
        return PathBuf::from(configured.trim());
    }
    std::env::var_os("USERPROFILE")
        .map(|h| {
            PathBuf::from(h)
                .join("Documents")
                .join("yyterm")
                .join("macros-out")
        })
        .unwrap_or_else(|| PathBuf::from("macros-out"))
}

/// 接続時のマクロなど、名前で書いたマクロのファイル（相対ならマクロのフォルダから）。
pub(crate) fn resolve(folder: Option<&Path>, name: &str) -> PathBuf {
    let p = Path::new(name.trim());
    if p.is_absolute() {
        return p.to_path_buf();
    }
    let with_ext = if p.extension().is_none() {
        p.with_extension("rhai")
    } else {
        p.to_path_buf()
    };
    match folder {
        Some(f) => f.join(with_ext),
        None => with_ext,
    }
}

/// 資格情報の名前（`tn3270/名前`）。
pub(crate) fn password_key(name: &str) -> String {
    format!("tn3270/{}", name.trim())
}

/// マクロのファイルを選ぶ（開く・保存）。
pub(crate) fn pick_file(owner: HWND, folder: Option<&Path>, save: Option<&str>) -> Option<PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog, IShellItem,
        SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    };
    use windows::core::{HSTRING, Interface, w};
    unsafe {
        let d: IFileDialog = if save.is_some() {
            CoCreateInstance::<_, IFileSaveDialog>(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
                .ok()?
                .cast()
                .ok()?
        } else {
            CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
                .ok()?
                .cast()
                .ok()?
        };
        let filters = [
            COMDLG_FILTERSPEC {
                pszName: w!("マクロ (*.rhai)"),
                pszSpec: w!("*.rhai"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("すべてのファイル (*.*)"),
                pszSpec: w!("*.*"),
            },
        ];
        let _ = d.SetFileTypes(&filters);
        let _ = d.SetDefaultExtension(w!("rhai"));
        if let Some(f) = folder {
            let _ = std::fs::create_dir_all(f);
            if let Ok(item) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(f.as_os_str()), None)
            {
                let _ = d.SetFolder(&item);
            }
        }
        if let Some(name) = save {
            let _ = d.SetFileName(&HSTRING::from(name));
            let _ = d.SetTitle(w!("記録したマクロを保存"));
        } else {
            let _ = d.SetTitle(w!("実行するマクロ"));
        }
        d.Show(Some(owner)).ok()?;
        let item = d.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_macro_names() {
        let f = Path::new("C:\\m");
        assert_eq!(resolve(Some(f), "logon"), f.join("logon.rhai"));
        assert_eq!(resolve(Some(f), "a\\b.rhai"), f.join("a\\b.rhai"));
        assert_eq!(resolve(None, "x"), PathBuf::from("x.rhai"));
        assert_eq!(password_key(" prod "), "tn3270/prod");
        assert!(output_folder("D:\\out").ends_with("out"));
    }
}

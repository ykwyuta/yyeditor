//! 異常終了の記録（クラッシュログ）。
//!
//! - panic（Rust の異常）と、Windows の処理されない例外（アクセス違反など）を
//!   `%LOCALAPPDATA%\yyeditor\logs\crash-アプリ-日時-プロセス番号.log` に書き、書いた場所を知らせる。
//!   中身はアプリと版・日時・スレッド・panic の文言と場所（例外なら例外コードとアドレス）・
//!   バックトレース。
//! - 起動中は `running-アプリ-プロセス番号.txt` をほかから消せないように開いておき、正しく終われば消す。
//!   次の起動で消せるもの（開いているプロセスがない）が残っていれば「前回は異常終了した」と分かる（記録を
//!   書けずに落ちたときも）。同時に動いているほかのウィンドウの印は消せないので、数えない。
//! - 知らせ（メッセージボックス）は UI のスレッドの panic と例外のときだけ。作業スレッドの panic は
//!   受け止めて続けることがある（転送・計算）ので、記録だけ書く。
//! - 記録は新しいものから 30 個まで残す。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, HWND};
use windows::Win32::System::Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::HSTRING;

/// 記録を残すアプリの名前（yyeditor・yysheet など）。
static APP: OnceLock<String> = OnceLock::new();
/// UI のスレッド（[`install`] を呼んだスレッド）。
static MAIN: OnceLock<std::thread::ThreadId> = OnceLock::new();
/// 起動中の印（開いたままにして、ほかのプロセスから消せないようにする）
static MARKER: Mutex<Option<(PathBuf, std::fs::File)>> = Mutex::new(None);
/// 記録を書いている間（記録を書く途中の異常で、もう一度書かない）。
static WRITING: AtomicBool = AtomicBool::new(false);

/// 記録を置くフォルダ（`%LOCALAPPDATA%\yyeditor\logs`）。
pub(crate) fn log_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("yyeditor")
        .join("logs")
}

/// 前回の起動の印で、残っていたもの（開いているプロセスがないので消せたもの）があるか。
fn left_markers(dir: &Path, app: &str) -> bool {
    let prefix = format!("running-{app}-");
    let mut found = false;
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with(&prefix)
            && std::fs::remove_file(e.path()).is_ok()
        {
            found = true;
        }
    }
    found
}

/// 起動中の印を開く（閉じるまでほかのプロセスからは開けない・消せない）。
fn open_marker(dir: &Path, app: &str) -> Option<(PathBuf, std::fs::File)> {
    use std::os::windows::fs::OpenOptionsExt;
    let pid = std::process::id();
    let path = dir.join(format!("running-{app}-{pid}.txt"));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .share_mode(0)
        .open(&path)
        .ok()?;
    let _ = writeln!(
        f,
        "{} {app} を起動（プロセス {pid}）",
        crate::remote::local_clock()
    );
    Some((path, f))
}

/// 記録を始める（アプリの起動の初めに呼ぶ）。前回が異常終了だったなら、その知らせ（記録の場所）を返す。
pub(crate) fn install(app: &str) -> Option<String> {
    let _ = APP.set(app.to_string());
    let _ = MAIN.set(std::thread::current().id());
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let previous = left_markers(&dir, app).then(|| {
        let last = logs(&dir, app).pop();
        match last {
            Some(p) => format!("前回の {app} は異常終了しました。記録: {}", p.display()),
            None => format!(
                "前回の {app} は異常終了しました（記録はありません）。記録のフォルダ: {}",
                dir.display()
            ),
        }
    });
    if let Ok(mut m) = MARKER.lock() {
        *m = open_marker(&dir, app);
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let current = std::thread::current();
        let thread = current.name().unwrap_or("（名前なし）").to_string();
        let main = MAIN.get() == Some(&current.id());
        let body = format!(
            "panic（スレッド {thread}{}）: {info}\n\nバックトレース:\n{}",
            if main { "。UI" } else { "。作業" },
            std::backtrace::Backtrace::force_capture()
        );
        write_and_tell(&body, main);
        prev(info);
    }));
    unsafe {
        SetUnhandledExceptionFilter(Some(on_exception));
    }
    prune(&dir, app, 30);
    previous
}

/// 正しく終わったことを残す（アプリの終わりに呼ぶ）。
pub(crate) fn clean_exit() {
    let taken = MARKER.lock().ok().and_then(|mut m| m.take());
    if let Some((path, f)) = taken {
        drop(f);
        let _ = std::fs::remove_file(path);
    }
}

/// Windows の処理されない例外（アクセス違反など）。
unsafe extern "system" fn on_exception(p: *const EXCEPTION_POINTERS) -> i32 {
    let (code, addr) = unsafe {
        p.as_ref()
            .and_then(|e| e.ExceptionRecord.as_ref())
            .map(|r| (r.ExceptionCode.0 as u32, r.ExceptionAddress as usize))
            .unwrap_or((0, 0))
    };
    let what = if code == EXCEPTION_ACCESS_VIOLATION.0 as u32 {
        "アクセス違反"
    } else {
        "例外"
    };
    let body = format!(
        "処理されない{what}: コード 0x{code:08X}、アドレス 0x{addr:X}\n\nバックトレース:\n{}",
        std::backtrace::Backtrace::force_capture()
    );
    write_and_tell(&body, true);
    // EXCEPTION_CONTINUE_SEARCH（Windows の既定の処理〔エラー報告〕に任せる）
    0
}

/// 記録を書き、`tell` なら書いた場所を知らせる。
fn write_and_tell(body: &str, tell: bool) {
    if WRITING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = APP.get().map(String::as_str).unwrap_or("yyeditor");
    if let Some(path) = write_log(&log_dir(), app, body).filter(|_| tell) {
        let text = format!(
            "{app} で異常が起きました。記録を残しました:\n{}\n\n不具合の報告には、このファイルを添えてください。",
            path.display()
        );
        unsafe {
            MessageBoxW(
                Some(HWND::default()),
                &HSTRING::from(text),
                &HSTRING::from(app),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    WRITING.store(false, Ordering::SeqCst);
}

/// 記録のファイルを書く（書いたファイル）。
fn write_log(dir: &Path, app: &str, body: &str) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!(
        "crash-{app}-{}-{}.log",
        crate::remote::local_clock()
            .replace([' ', ':'], "-")
            .replace('.', "-"),
        std::process::id()
    ));
    let mut f = std::fs::File::create(&path).ok()?;
    let _ = writeln!(
        f,
        "{app} {}（{}）\n日時: {}（UNIX 時刻 {secs}）\nプロセス: {}\n実行ファイル: {}\n\n{body}",
        env!("CARGO_PKG_VERSION"),
        if cfg!(debug_assertions) {
            "デバッグ版"
        } else {
            "リリース版"
        },
        crate::remote::local_clock(),
        std::process::id(),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    );
    let _ = f.sync_all();
    Some(path)
}

/// アプリの記録（古い順）。
fn logs(dir: &Path, app: &str) -> Vec<PathBuf> {
    let prefix = format!("crash-{app}-");
    let mut v: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    v.sort();
    v.into_iter().map(|x| x.1).collect()
}

/// 古い記録を消す（新しいものを `keep` 個残す）。
fn prune(dir: &Path, app: &str, keep: usize) {
    let all = logs(dir, app);
    let n = all.len().saturating_sub(keep);
    for p in all.into_iter().take(n) {
        let _ = std::fs::remove_file(p);
    }
}

/// 記録のフォルダをエクスプローラーで開く（ヘルプ メニュー）。
pub(crate) fn open_log_dir() {
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);
    unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(
            None,
            &HSTRING::from("open"),
            &HSTRING::from(dir.as_os_str()),
            None,
            None,
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("yy-crash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn writes_logs_and_keeps_the_newest() {
        let dir = temp_dir("logs");
        let p = write_log(&dir, "yytest", "panic: テスト").unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("yytest "), "{text}");
        assert!(text.contains("panic: テスト"), "{text}");
        assert!(
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("crash-yytest-")
        );
        for i in 0..4 {
            std::fs::write(dir.join(format!("crash-yytest-old{i}.log")), "x").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::fs::write(dir.join("crash-other-1.log"), "x").unwrap();
        prune(&dir, "yytest", 2);
        let left = logs(&dir, "yytest");
        assert_eq!(left.len(), 2);
        assert!(left[1].ends_with("crash-yytest-old3.log"));
        // ほかのアプリの記録は消さない
        assert!(dir.join("crash-other-1.log").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn markers_tell_a_previous_crash() {
        let dir = temp_dir("markers");
        // 開いている印（動いているウィンドウ）は前回の異常終了に数えない
        let (path, f) = open_marker(&dir, "yytest").unwrap();
        assert!(!left_markers(&dir, "yytest"));
        assert!(path.exists());
        // 閉じたまま残った印（異常終了）は数えて消す
        drop(f);
        assert!(left_markers(&dir, "yytest"));
        assert!(!path.exists());
        assert!(!left_markers(&dir, "yytest"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

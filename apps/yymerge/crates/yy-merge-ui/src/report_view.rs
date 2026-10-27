//! Background export of a captured, completed two-way comparison.
use super::*;
pub(super) struct Job {
    receiver: mpsc::Receiver<Result<PathBuf>>,
    pub cancel: Arc<Cancellation>,
}
fn ready(a: &App) -> Result<&Loaded> {
    rules_ready(a)?;
    if a.report_job.is_some() {
        return Err("HTMLレポートを作成中です。[停止] で中止できます。".into());
    }
    let data = a
        .loaded
        .as_ref()
        .ok_or("先に2方向のファイル比較を開いてください。")?;
    if data.merged.is_some() || data.rules != a.options.rules.core() {
        return Err("比較が正常に完了してからHTMLレポートを保存してください。".into());
    }
    Ok(data)
}
pub(super) fn start(a: &mut App, path: PathBuf) -> Result<()> {
    let data = ready(a)?;
    for (i, input) in [&a.options.left, &a.options.right].into_iter().enumerate() {
        let input = input.as_ref().ok_or("比較入力がありません。")?;
        if std::fs::canonicalize(input).map_err(|e| e.to_string())? != data.inputs[i].path {
            return Err("比較入力が変わっています。再比較してください。".into());
        }
    }
    let left = data.inputs[0].clone();
    let right = data.inputs[1].clone();
    let rules = data.rules;
    let cancel = Arc::new(Cancellation::default());
    let worker_cancel = cancel.clone();
    let (tx, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let compared =
                yy_merge_core::rules::compare(&left.text, &right.text, rules, &worker_cancel)?;
            let html =
                yy_merge_core::report::html(&left, &right, rules, &compared, &worker_cancel)?;
            yy_merge_core::files::save(
                &path,
                html.as_bytes(),
                &[&left, &right],
                true,
                &worker_cancel,
            )?;
            Ok(path)
        })();
        let _ = tx.send(result);
    });
    a.report_job = Some(Job { receiver, cancel });
    text(a.status, "HTMLレポートを作成中… [停止] で中止できます");
    update_menu(a);
    Ok(())
}
pub(super) fn command() -> Result<()> {
    app(|a| ready(a).map(|_| ())).unwrap_or(Ok(()))?;
    if let Some(path) = choose_file_ext(true, "2方向比較のHTMLレポートを保存", Some("html"))
    {
        app(|a| start(a, path)).unwrap_or(Ok(()))?;
    }
    Ok(())
}
pub(super) fn update_menu(a: &App) {
    unsafe {
        let _ = EnableMenuItem(
            GetMenu(a.hwnd),
            REPORT_HTML as u32,
            MF_BYCOMMAND
                | if ready(a).is_ok() {
                    MF_ENABLED
                } else {
                    MF_GRAYED
                },
        );
    }
}
pub(super) fn tick(a: &mut App) -> Option<Result<PathBuf>> {
    let result = match a.report_job.as_ref()?.receiver.try_recv() {
        Ok(result) => result,
        Err(mpsc::TryRecvError::Empty) => return None,
        Err(mpsc::TryRecvError::Disconnected) => {
            Err("HTML report worker stopped unexpectedly".into())
        }
    };
    a.report_job = None;
    update_menu(a);
    Some(result)
}
pub(super) fn smoke(a: &mut App, dir: &std::path::Path) -> Result<()> {
    let path = dir.join("comparison.html");
    let expected_exit = a.exit;
    start(a, path.clone())?;
    if start(a, dir.join("duplicate.html")).is_ok() {
        return Err("parallel HTML export was accepted".into());
    }
    let wait = |a: &mut App| {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = tick(a) {
                return result;
            }
            if Instant::now() > until {
                return Err("HTML export smoke timeout".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    if wait(a)? != path || a.exit != expected_exit {
        return Err("HTML export changed comparison result".into());
    }
    let html = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    if !html.contains("EquivalentUnderRules")
        || !html.contains("<tr class=ignored>")
        || !html.contains("Alpha \t")
    {
        return Err("HTML export lost original text or display rules".into());
    }
    let input = a.options.left.clone().unwrap();
    start(a, input.clone())?;
    if wait(a).is_ok() || std::fs::read(&input).map_err(|e| e.to_string())? != b"Alpha \t\r\n" {
        return Err("HTML report overwrote a comparison input".into());
    }
    // Changed input after the displayed comparison must not be exported as current.
    let before = std::fs::read(&input).map_err(|e| e.to_string())?;
    std::fs::write(&input, b"changed externally\n").map_err(|e| e.to_string())?;
    start(a, path.clone())?;
    if wait(a).is_ok() || std::fs::read_to_string(&path).map_err(|e| e.to_string())? != html {
        return Err("stale HTML export replaced an existing report".into());
    }
    std::fs::write(&input, before).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("html-report-smoke.txt"), "PASS: asynchronous captured-rule HTML export, original text/endings, duplicate job and input overwrite rejected, stale inputs preserve existing report, comparison result retained\n").map_err(|e| e.to_string())
}

//! Work-session UI; saving work never marks RESULT as saved or resolves conflicts.
use super::*;
pub(super) struct Resume {
    pub manual: bool,
    pub selected: usize,
}
pub(super) struct Job {
    pub(super) receiver: mpsc::Receiver<Result<PathBuf>>,
    pub cancel: Arc<Cancellation>,
}
pub(super) fn load(path: &std::path::Path, cancel: &Cancellation) -> Result<Loaded> {
    let (state, _) = yy_merge_core::session::Session::open(path, cancel)?;
    let [base, left, right] = state.inputs;
    let diff = yy_merge_core::compare(&left.text, &right.text, cancel)?;
    let merge_comparison = Some(super::super::comparison::prepare_merge(
        &base.text,
        &left.text,
        &right.text,
        &state.result,
        cancel,
    )?);
    Ok(Loaded {
        comparison_resume: None,
        inputs: vec![left, right, base],
        diff,
        rules: Default::default(),
        ignored: vec![],
        merged: Some(state.model),
        comparison: None,
        merge_comparison,
        result_text: Some(state.result),
        resume: Some(Resume {
            manual: state.manual,
            selected: state.selected,
        }),
    })
}
fn can_open(a: &App) -> bool {
    !a.options.git_diff && !a.options.git_merge && a.receiver.is_none()
}
fn ready(a: &App) -> Result<&Loaded> {
    if !can_open(a) || a.folder_mode || a.session_job.is_some() {
        return Err("処理中でない通常の3方向マージで作業セッションを保存してください。".into());
    }
    let data = a
        .loaded
        .as_ref()
        .filter(|d| d.merged.is_some())
        .ok_or("先に3方向マージを開いてください。")?;
    Ok(data)
}
pub(super) fn save(a: &mut App, path: PathBuf) -> Result<()> {
    let data = ready(a)?;
    for (i, input) in [&a.options.left, &a.options.right, &a.options.base]
        .into_iter()
        .enumerate()
    {
        let input = input.as_ref().ok_or("マージ入力がありません。")?;
        if std::fs::canonicalize(input).map_err(|e| e.to_string())? != data.inputs[i].path {
            return Err("比較が正常に完了してから作業セッションを保存してください。".into());
        }
    }
    let inputs = data.inputs.clone();
    let model = data.merged.clone().unwrap();
    let result = a.editors[3].text();
    let manual = a.manual;
    let selected = a.selected;
    let cancel = Arc::new(Cancellation::default());
    let worker_cancel = cancel.clone();
    let (tx, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let session = yy_merge_core::session::Session::capture(
                [&inputs[2], &inputs[0], &inputs[1]],
                &model,
                result,
                manual,
                selected,
                &worker_cancel,
            )?;
            let bytes = session.json(&worker_cancel)?;
            let refs: Vec<_> = inputs.iter().collect();
            yy_merge_core::files::save(&path, &bytes, &refs, true, &worker_cancel)?;
            Ok(path)
        })();
        let _ = tx.send(result);
    });
    a.session_job = Some(Job { receiver, cancel });
    text(a.status, "作業セッションを保存中… [停止] で中止できます");
    update_menu(a);
    Ok(())
}
pub(super) fn command(id: u16) -> Result<()> {
    if id == SESSION_OPEN {
        if !app(|a| can_open(a)).unwrap_or(false) {
            return Err("比較中またはGit専用画面ではセッションを開けません。".into());
        }
        if app(|a| a.edited && !a.saved).unwrap_or(false)
            && !confirm("未保存のRESULTを破棄して作業セッションを開きますか？")
        {
            return Ok(());
        }
        if let Some(path) =
            choose_file_ext(false, "3方向マージの作業セッションを開く", Some("json"))
        {
            app(|a| {
                begin_options(
                    a,
                    Gui {
                        session: Some(path),
                        ..Default::default()
                    },
                )
            });
        }
    } else {
        app(|a| ready(a).map(|_| ())).unwrap_or(Ok(()))?;
        if let Some(path) = choose_file_ext(
            true,
            "作業セッションを保存（RESULTの保存とは別です）",
            Some("json"),
        ) {
            app(|a| save(a, path)).unwrap_or(Ok(()))?;
        }
    }
    Ok(())
}
pub(super) fn update_menu(a: &App) {
    unsafe {
        for (id, enabled) in [
            (SESSION_OPEN, can_open(a)),
            (SESSION_SAVE, ready(a).is_ok()),
        ] {
            let _ = EnableMenuItem(
                GetMenu(a.hwnd),
                id as u32,
                MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
            );
        }
    }
}
pub(super) fn tick(a: &mut App) -> Option<Result<PathBuf>> {
    let result = match a.session_job.as_ref()?.receiver.try_recv() {
        Ok(result) => result,
        Err(mpsc::TryRecvError::Empty) => return None,
        Err(mpsc::TryRecvError::Disconnected) => {
            Err("session save worker stopped unexpectedly".into())
        }
    };
    a.session_job = None;
    update_menu(a);
    comparison_session_view::update_menu(a);
    folders_view::update_menu(a);
    Some(result)
}
pub(super) fn smoke(a: &mut App, dir: &std::path::Path) -> Result<()> {
    let original_options = a.options.clone();
    let path = dir.join("work-session.json");
    let wait = |a: &mut App| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = tick(a) {
                return result;
            }
            if Instant::now() > deadline {
                return Err("session save smoke timeout".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    save(a, path.clone())?;
    wait(a)?;
    if a.saved {
        return Err("session save marked RESULT saved".into());
    }
    resolve(a, Some(Choice::Right))?;
    begin_options(
        a,
        Gui {
            session: Some(path.clone()),
            ..Default::default()
        },
    );
    await_rule_smoke(a)?;
    if a.loaded
        .as_ref()
        .unwrap()
        .merged
        .as_ref()
        .unwrap()
        .unresolved()
        != 1
        || a.saved
        || !a.edited
        || a.manual
        || !a.history.is_empty()
        || a.options.output.is_some()
    {
        return Err("session resume lost unresolved/unsaved state or inherited output".into());
    }
    let draft = "編集中のRESULT（未確定）\r\n";
    a.editors[3] = editor(draft, unsafe { GetDpiForWindow(a.hwnd) })?;
    a.manual = true;
    save(a, path.clone())?;
    wait(a)?;
    begin_options(
        a,
        Gui {
            session: Some(path.clone()),
            ..Default::default()
        },
    );
    await_rule_smoke(a)?;
    if a.editors[3].text() != draft
        || !a.manual
        || a.loaded
            .as_ref()
            .unwrap()
            .merged
            .as_ref()
            .unwrap()
            .unresolved()
            != 1
    {
        return Err("session resume lost unconfirmed manual edits".into());
    }
    let input = a.options.left.clone().unwrap();
    save(a, input.clone())?;
    if wait(a).is_ok() {
        return Err("session save overwrote input".into());
    }
    let before = std::fs::read(&input).map_err(|e| e.to_string())?;
    std::fs::write(&input, b"changed after session save\n").map_err(|e| e.to_string())?;
    begin_options(
        a,
        Gui {
            session: Some(path),
            ..Default::default()
        },
    );
    if await_rule_smoke(a).is_ok() || a.editors[3].text() != draft {
        return Err("changed-input session replaced current work".into());
    }
    std::fs::write(&input, before).map_err(|e| e.to_string())?;
    a.options = original_options;
    install(a, super::load(&a.options, &Cancellation::default())?)?;
    std::fs::write(dir.join("session-smoke.txt"), "PASS: background work save/restore, unresolved and manual RESULT retained, no output inherited, RESULT remains unsaved, undo reset, input overwrite and changed-input restore rejected\n").map_err(|e|e.to_string())
}

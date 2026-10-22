//! Stage the complete rescan before replacing the current folder view.
use super::*;
use yy_merge_core::folder_session::Session;
pub(crate) struct LoadJob {
    receiver: mpsc::Receiver<Result<(Session, Vec<FolderRow>)>>,
    cancel: Arc<Cancellation>,
}
impl Drop for LoadJob {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
pub(crate) fn cancel_load(a: &mut App) {
    if a.folder_session_job.take().is_some() {
        text(
            a.status,
            "フォルダー作業セッションの読込を中止しました（現在の画面は保持）",
        );
    }
}
fn can_open(a: &App) -> bool {
    !a.options.git_diff
        && !a.options.git_merge
        && a.receiver.is_none()
        && a.folders.as_ref().is_none_or(|f| !f.is_running())
}
fn ready(a: &App) -> bool {
    can_open(a)
        && a.folder_mode
        && a.folder_session_job.is_none()
        && a.session_job.is_none()
        && a.folders
            .as_ref()
            .is_some_and(|f| !f.view_plan && f.error.is_none())
}
pub(crate) fn update_menu(a: &App) {
    unsafe {
        for (id, enabled) in [
            (FOLDER_SESSION_OPEN, can_open(a)),
            (FOLDER_SESSION_SAVE, ready(a)),
        ] {
            let _ = EnableMenuItem(
                GetMenu(a.hwnd),
                id as u32,
                MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
            );
        }
    }
}
pub(crate) fn begin_load(a: &mut App, path: PathBuf) -> Result<()> {
    if !can_open(a) {
        return Err("処理中またはGit専用画面ではフォルダー作業セッションを開けません".into());
    }
    cancel_load(a);
    search_view::cancel(a);
    let cancel = Arc::new(Cancellation::default());
    let worker_cancel = cancel.clone();
    let (tx, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let session = Session::open(&path, &worker_cancel)?;
            let entries = if let Some(base) = &session.base {
                yy_merge_core::folders::three::compare(
                    base,
                    &session.left,
                    &session.right,
                    session.recursive,
                    &worker_cancel,
                )?
                .into_iter()
                .map(FolderRow::from)
                .collect()
            } else {
                yy_merge_core::folders::compare(
                    &session.left,
                    &session.right,
                    session.recursive,
                    &worker_cancel,
                )?
                .into_iter()
                .map(FolderRow::from)
                .collect()
            };
            worker_cancel.check()?;
            Ok((session, entries))
        })();
        let _ = tx.send(result);
    });
    a.folder_session_job = Some(LoadJob { receiver, cancel });
    text(
        a.status,
        "フォルダー作業セッションを読込・再走査中… [停止]で中止できます",
    );
    update_menu(a);
    Ok(())
}
fn install(a: &mut App, session: Session, entries: Vec<FolderRow>) -> Result<()> {
    let filter = session.filter()?;
    let selected = session.selected;
    let mut f = FolderView {
        recursive: session.recursive,
        filter,
        visible: vec![],
        filter_due: None,
        controls_dirty: true,
        view_plan: false,
        plan: None,
        plan_receiver: None,
        plan_error: None,
        export_receiver: None,
        export_result: None,
        run_directory: None,
        base: session.base,
        left: session.left,
        right: session.right,
        entries,
        page: 0,
        receiver: None,
        cancel: Arc::default(),
        error: None,
    };
    rebuild_filter(&mut f);
    let index = selected.and_then(|path| {
        f.visible
            .iter()
            .position(|i| f.entries[*i].entry.path == path)
    });
    if let Some(index) = index {
        f.page = index / PAGE;
    }
    a.folders = Some(f);
    a.folder_mode = true;
    a.options.folder_session = None;
    refresh(a);
    layout(a);
    if let Some(index) = index {
        unsafe {
            SendMessageW(a.list, LB_SETCURSEL, Some(WPARAM(index % PAGE)), None);
        }
        selection(a);
    }
    Ok(())
}
fn poll(a: &mut App) -> Option<Result<()>> {
    let result = match a.folder_session_job.as_ref()?.receiver.try_recv() {
        Ok(result) => result,
        Err(mpsc::TryRecvError::Empty) => return None,
        Err(_) => Err("folder session worker stopped unexpectedly".into()),
    };
    a.folder_session_job = None;
    Some(result.and_then(|(session, entries)| install(a, session, entries)))
}
pub(crate) fn tick(a: &mut App) {
    if let Some(Err(e)) = poll(a) {
        if a.loaded.is_none() && a.folders.is_none() {
            a.exit = 2;
        }
        text(
            a.status,
            &format!("作業セッションを開けません。現在の画面は保持: {e}"),
        );
    }
    update_menu(a);
}
fn selected_path(a: &App) -> Option<PathBuf> {
    let f = a.folders.as_ref()?;
    let index = unsafe { SendMessageW(a.list, LB_GETCURSEL, None, None).0 };
    if index < 0 {
        return None;
    }
    visible_row(f, f.page * PAGE + index as usize).map(|row| row.entry.path.clone())
}
pub(crate) fn save(a: &mut App, path: PathBuf) -> Result<()> {
    if !ready(a) {
        return Err("完了したフォルダー比較一覧で作業セッションを保存してください。同期計画の表示中は保存できません".into());
    }
    let selected = selected_path(a);
    if a.folders.as_ref().is_some_and(|f| f.filter_due.is_some()) {
        filter_view::apply(a);
    }
    let f = a.folders.as_ref().unwrap();
    let (left, right, base, recursive, filter) = (
        f.left.clone(),
        f.right.clone(),
        f.base.clone(),
        f.recursive,
        f.filter.clone(),
    );
    let cancel = Arc::new(Cancellation::default());
    let worker_cancel = cancel.clone();
    let (tx, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let session = Session::capture(
                &left,
                &right,
                base.as_deref(),
                recursive,
                &filter,
                selected,
                &worker_cancel,
            )?;
            session.save(&path, true, &worker_cancel)?;
            Ok(path)
        })();
        let _ = tx.send(result);
    });
    a.session_job = Some(super::super::session_view::Job { receiver, cancel });
    text(
        a.status,
        "フォルダー比較の作業セッションを保存中… [停止]で中止できます",
    );
    update_menu(a);
    Ok(())
}
pub(crate) fn command(id: u16) -> Result<()> {
    if id == FOLDER_SESSION_OPEN {
        if !app(|a| can_open(a)).unwrap_or(false) {
            return Err("処理中またはGit専用画面では開けません".into());
        }
        if let Some(path) = choose_file_ext(
            false,
            "フォルダー比較の作業セッションを開いて再走査",
            Some("json"),
        ) {
            app(|a| begin_load(a, path)).unwrap_or(Ok(()))?;
        }
    } else {
        if !app(|a| ready(a)).unwrap_or(false) {
            return Err("完了したフォルダー比較一覧を表示してください".into());
        }
        if let Some(path) = choose_file_ext(
            true,
            "フォルダー比較の作業セッションを保存（比較対象の外）",
            Some("json"),
        ) {
            app(|a| save(a, path)).unwrap_or(Ok(()))?;
        }
    }
    Ok(())
}
pub(crate) fn smoke(a: &mut App, dir: &std::path::Path) -> Result<()> {
    let original = a.folders.take();
    let mode = a.folder_mode;
    let flags = (a.manual, a.saved, a.edited, a.exit);
    let texts: Vec<_> = a.editors.iter().map(|e| e.text()).collect();
    let left = dir.join("session-folder-left");
    let right = dir.join("session-folder-right");
    let base = dir.join("session-folder-base");
    for root in [&left, &right, &base] {
        std::fs::create_dir_all(root.join("nested")).map_err(|e| e.to_string())?;
        std::fs::write(root.join("same.txt"), "same").map_err(|e| e.to_string())?;
    }
    std::fs::write(left.join("nested/selected.csv"), "left").map_err(|e| e.to_string())?;
    let path = dir.join("folder-session.json");
    let cancel = Cancellation::default();
    let session = Session::capture(
        &left,
        &right,
        None,
        true,
        &Filter::new(".csv".into(), Default::default())?,
        Some("nested/selected.csv".into()),
        &cancel,
    )?;
    session.save(&path, false, &cancel)?;
    let wait = |a: &mut App| -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = poll(a) {
                return result;
            }
            if Instant::now() > deadline {
                return Err("folder session smoke timeout".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    begin_load(a, path.clone())?;
    wait(a)?;
    if selected_path(a).as_deref() != Some(std::path::Path::new("nested/selected.csv"))
        || a.folders.as_ref().unwrap().filter.query() != ".csv"
        || a.folders.as_ref().unwrap().plan.is_some()
        || exit_code(a) != Some(1)
    {
        return Err("folder session lost filter/selection or inherited plan".into());
    }
    let saved = dir.join("folder-session-saved.json");
    save(a, saved.clone())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = super::super::session_view::tick(a) {
            result?;
            break;
        }
        if Instant::now() > deadline {
            return Err("folder session save timeout".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let saved_config = Session::open(&saved, &cancel)?;
    if saved_config.selected.as_deref() != Some(std::path::Path::new("nested/selected.csv")) {
        return Err("folder session save lost selected path".into());
    }
    std::fs::write(right.join("nested/selected.csv"), "left").map_err(|e| e.to_string())?;
    begin_load(a, path.clone())?;
    wait(a)?;
    if exit_code(a) != Some(0) {
        return Err("folder session restored stale results".into());
    }
    let three = Session::capture(
        &left,
        &right,
        Some(&base),
        false,
        &Filter::default(),
        None,
        &cancel,
    )?;
    let three_path = dir.join("folder-session-three.json");
    three.save(&three_path, false, &cancel)?;
    begin_load(a, three_path)?;
    wait(a)?;
    if a.folders.as_ref().unwrap().base.is_none()
        || a.folders.as_ref().unwrap().recursive
        || a.folders
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .any(|row| row.entry.path == std::path::Path::new("nested/selected.csv"))
    {
        return Err("folder session lost ancestor/recursion".into());
    }
    let before = a.folders.as_ref().unwrap().left.clone();
    let count = a.folders.as_ref().unwrap().entries.len();
    begin_load(a, path.clone())?;
    cancel_load(a);
    tick(a);
    if a.folders.as_ref().unwrap().left != before
        || a.folders.as_ref().unwrap().entries.len() != count
    {
        return Err("cancelled folder session replaced current view".into());
    }
    std::fs::write(dir.join("folder-session-invalid.json"), "{}").map_err(|e| e.to_string())?;
    begin_load(a, dir.join("folder-session-invalid.json"))?;
    if wait(a).is_ok() || a.folders.as_ref().unwrap().entries.len() != count {
        return Err("invalid folder session replaced current view".into());
    }
    if texts != a.editors.iter().map(|e| e.text()).collect::<Vec<_>>()
        || flags != (a.manual, a.saved, a.edited, a.exit)
    {
        return Err("folder session changed underlying file work".into());
    }
    a.folders = original;
    a.folder_mode = mode;
    super::super::refresh(a);
    layout(a);
    std::fs::write(dir.join("folder-session-smoke.txt"),"PASS: background folder setup save/open, current-content rescan, filter and selected path, ancestor/nonrecursive scan, no sync plan inherited, cancellation/invalid metadata preserve view, underlying file work retained\n").map_err(|e|e.to_string())
}

//! Two-way work sessions, separate from editable three-way merge sessions.
use super::*;
use yy_merge_core::comparison_session::{Position, Selection, Session};
pub(super) fn load(path: &std::path::Path, cancel: &Cancellation) -> Result<Loaded> {
    let (state, _) = Session::open(path, cancel)?;
    let [left, right] = state.inputs;
    let comparison = Some(super::super::comparison::prepare(
        &left.text,
        &right.text,
        &state.comparison.diff,
        cancel,
    )?);
    Ok(Loaded {
        comparison_resume: Some(state.position),
        resume: None,
        inputs: vec![left, right],
        diff: state.comparison.diff,
        rules: state.rules,
        ignored: state.comparison.ignored,
        merged: None,
        comparison,
        merge_comparison: None,
        result_text: None,
    })
}
pub(super) fn restore_position(a: &mut App, position: Position) {
    a.selected = position.difference;
    refresh(a);
    for (editor, selection) in a.editors.iter_mut().zip(position.selections) {
        editor
            .document
            .set_selections(yy_core::SelectionSet::single(yy_core::Selection::new(
                selection.anchor,
                selection.head,
            )));
        editor.viewport.top =
            yy_core::motion::line_start(editor.document.snapshot(), selection.head);
        if let Some(view) = &mut editor.aligned {
            view.top = view.row_for_offset(selection.head);
        }
        editor.follow_caret = true;
        editor.reveal_caret();
    }
    search_view::focus(a, position.active);
    sync_sources(a, position.active);
    for pane in &a.panes {
        invalidate(*pane);
    }
    unsafe {
        let _ = SetFocus(Some(a.panes[position.active]));
    }
}
fn can_open(a: &App) -> bool {
    !a.options.git_diff && !a.options.git_merge && a.receiver.is_none()
}
fn ready(a: &App) -> Result<&Loaded> {
    if !can_open(a) || a.folder_mode || a.session_job.is_some() {
        return Err("処理中でない通常の2方向比較で作業セッションを保存してください".into());
    }
    a.loaded
        .as_ref()
        .filter(|data| data.merged.is_none() && a.editors.len() == 2)
        .ok_or("先に2方向比較を開いてください".into())
}
pub(super) fn save(a: &mut App, path: PathBuf) -> Result<()> {
    let data = ready(a)?;
    for (path, input) in [&a.options.left, &a.options.right]
        .into_iter()
        .zip(&data.inputs)
    {
        if path
            .as_ref()
            .and_then(|p| std::fs::canonicalize(p).ok())
            .is_none_or(|p| p != input.path)
        {
            return Err("比較が正常に完了してから作業セッションを保存してください".into());
        }
    }
    let inputs = data.inputs.clone();
    let rules = data.rules;
    let selections = std::array::from_fn(|i| {
        let s = a.editors[i].document.selections().primary();
        Selection {
            anchor: s.anchor,
            head: s.head,
        }
    });
    let position = Position {
        selections,
        active: a.search.active,
        difference: a.selected,
    };
    let cancel = Arc::new(Cancellation::default());
    let worker_cancel = cancel.clone();
    let (tx, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let state =
                Session::capture([&inputs[0], &inputs[1]], rules, position, &worker_cancel)?;
            let bytes = state.json(&worker_cancel)?;
            yy_merge_core::files::save(
                &path,
                &bytes,
                &[&inputs[0], &inputs[1]],
                true,
                &worker_cancel,
            )?;
            Ok(path)
        })();
        let _ = tx.send(result);
    });
    a.session_job = Some(session_view::Job { receiver, cancel });
    text(
        a.status,
        "2方向比較の作業セッションを保存中… [停止] で中止できます",
    );
    update_menu(a);
    session_view::update_menu(a);
    Ok(())
}
pub(super) fn command(id: u16) -> Result<()> {
    if id == COMPARISON_SESSION_OPEN {
        if !app(|a| can_open(a)).unwrap_or(false) {
            return Err("比較中またはGit専用画面では開けません".into());
        }
        if app(|a| a.edited && !a.saved).unwrap_or(false)
            && !confirm("未保存のRESULTを破棄して2方向比較の作業セッションを開きますか？")
        {
            return Ok(());
        }
        if let Some(path) = choose_file_ext(false, "2方向比較の作業セッションを開く", Some("json"))
        {
            app(|a| {
                begin_options(
                    a,
                    Gui {
                        comparison_session: Some(path),
                        ..Default::default()
                    },
                )
            });
        }
    } else {
        app(|a| ready(a).map(|_| ())).unwrap_or(Ok(()))?;
        if let Some(path) = choose_file_ext(true, "2方向比較の作業セッションを保存", Some("json"))
        {
            app(|a| save(a, path)).unwrap_or(Ok(()))?;
        }
    }
    Ok(())
}
pub(super) fn update_menu(a: &App) {
    unsafe {
        for (id, enabled) in [
            (COMPARISON_SESSION_OPEN, can_open(a)),
            (COMPARISON_SESSION_SAVE, ready(a).is_ok()),
        ] {
            let _ = EnableMenuItem(
                GetMenu(a.hwnd),
                id as u32,
                MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
            );
        }
    }
}
pub(super) fn smoke(a: &mut App, dir: &std::path::Path) -> Result<()> {
    let original = a.options.clone();
    let left = dir.join("comparison-session-left.txt");
    let right = dir.join("comparison-session-right.txt");
    // Different BOM-less encodings also survive a rule recompare after resume.
    std::fs::write(&left, [0x94, 0x4c, b' ', b'A', b'\r', b'\n']).map_err(|e| e.to_string())?;
    std::fs::write(&right, "猫 a\n").map_err(|e| e.to_string())?;
    let cancel = Cancellation::default();
    let l = Snapshot::open(&left, Some("CP932"), &cancel)?;
    let r = Snapshot::open(&right, None, &cancel)?;
    let rules = yy_merge_core::rules::Rules {
        ignore_case: true,
        ignore_eol: true,
        ..Default::default()
    };
    let position = Position {
        selections: [
            Selection { anchor: 4, head: 0 },
            Selection { anchor: 0, head: 3 },
        ],
        active: 1,
        difference: 0,
    };
    let path = dir.join("comparison-session.json");
    let state = Session::capture([&l, &r], rules, position, &cancel)?;
    std::fs::write(&path, state.json(&cancel)?).map_err(|e| e.to_string())?;
    begin_options(
        a,
        Gui {
            comparison_session: Some(path.clone()),
            ..Default::default()
        },
    );
    await_rule_smoke(a)?;
    if a.editors.len() != 2
        || a.loaded.as_ref().unwrap().rules != rules
        || a.options.output.is_some()
        || a.options.base.is_some()
        || a.options.comparison_session.is_some()
        || a.edited
        || a.manual
        || a.exit != 1 // Text is equivalent under rules; the encodings still differ.
        || a.search.active != 1
    {
        return Err("comparison session resume lost settings/flags".into());
    }
    for (e, s) in a.editors.iter().zip(position.selections) {
        if *e.document.selections().primary() != yy_core::Selection::new(s.anchor, s.head) {
            return Err("comparison session selection was lost".into());
        }
    }
    let saved = dir.join("comparison-session-saved.json");
    save(a, saved.clone())?;
    let wait = |a: &mut App| -> Result<PathBuf> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = session_view::tick(a) {
                return result;
            }
            if Instant::now() > deadline {
                return Err("comparison session save timeout".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    wait(a)?;
    let (restored, _) = Session::open(&saved, &cancel)?;
    if restored.position != position || restored.rules != rules {
        return Err("GUI comparison session saved wrong position/rules".into());
    }
    set_rule(a, RULE_RESET)?;
    await_rule_smoke(a)?;
    if a.exit != 1 || a.loaded.as_ref().unwrap().inputs[0].text.as_ref() != "猫 A\r\n" {
        return Err("resumed comparison rule recompare lost encodings".into());
    }
    save(a, left.clone())?;
    if wait(a).is_ok() {
        return Err("comparison session overwrote input".into());
    }
    let current = a.editors[0].text();
    std::fs::write(&left, b"changed").map_err(|e| e.to_string())?;
    begin_options(
        a,
        Gui {
            comparison_session: Some(path),
            ..Default::default()
        },
    );
    if await_rule_smoke(a).is_ok() || a.editors[0].text() != current {
        return Err("changed comparison input replaced current view".into());
    }
    a.options = original.clone();
    begin_options(a, original);
    await_rule_smoke(a)?;
    std::fs::write(dir.join("comparison-session-smoke.txt"),"PASS: two-way background save/resume, rules and oriented Unicode selections/active pane, mixed encodings retained across rule recompare, input overwrite and changed-input restore rejected, no result destination or manual/edited state inherited\n").map_err(|e|e.to_string())
}

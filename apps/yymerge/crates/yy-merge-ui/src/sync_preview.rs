//! Synchronization proposal view; all work, including JSON export, runs off the UI thread.
use super::*;
use std::path::Path;
use yy_merge_core::sync_plan::{self, Mode, Side};

fn mode(id: u16) -> Option<Mode> {
    Some(match id {
        PLAN_UPDATE_RIGHT => Mode::UpdateRight,
        PLAN_UPDATE_LEFT => Mode::UpdateLeft,
        PLAN_MIRROR_RIGHT => Mode::MirrorRight,
        PLAN_MIRROR_LEFT => Mode::MirrorLeft,
        PLAN_MERGE => Mode::Merge,
        _ => return None,
    })
}
pub(super) fn update_menu(a: &App) {
    let f = a.folders.as_ref();
    let ready = f.is_some_and(|f| !f.is_running()) && a.receiver.is_none();
    unsafe {
        let menu = GetMenu(a.hwnd);
        for id in [
            PLAN_UPDATE_RIGHT,
            PLAN_UPDATE_LEFT,
            PLAN_MIRROR_RIGHT,
            PLAN_MIRROR_LEFT,
            PLAN_MERGE,
            PLAN_SAVE,
            PLAN_SHOW,
            PLAN_APPLY,
            PLAN_ROLLBACK,
            PLAN_RESUME,
        ] {
            let enabled = match id {
                PLAN_SHOW => f.is_some_and(|f| {
                    f.plan.is_some() || f.plan_receiver.is_some() || f.plan_error.is_some()
                }),
                PLAN_APPLY => {
                    ready
                        && f.is_some_and(|f| {
                            f.run_directory.is_none()
                                && f.plan.as_ref().is_some_and(|p| {
                                    p.fingerprints.is_some()
                                        && p.exit_code() == 0
                                        && p.summary.operations > 0
                                })
                        })
                }
                PLAN_ROLLBACK | PLAN_RESUME => ready,
                PLAN_SAVE => ready && f.is_some_and(|f| f.plan.is_some()),
                PLAN_MERGE => ready && f.is_some_and(|f| f.base.is_some()),
                _ => ready && f.is_some_and(|f| f.base.is_none()),
            };
            let _ = EnableMenuItem(
                menu,
                id as u32,
                MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
            );
        }
    }
}
fn start_plan(f: &mut FolderView, mode: Mode) -> Result<()> {
    if f.is_running() {
        return Err("処理中です。完了または停止後に計画を作成してください。".into());
    }
    if (mode == Mode::Merge) != f.base.is_some() {
        return Err("3方向フォルダーには祖先付き統合計画、2方向フォルダーには方向指定の計画を選択してください。".into());
    }
    f.cancel = Arc::default();
    f.plan = None;
    f.plan_error = None;
    f.view_plan = true;
    f.page = 0;
    f.export_result = None;
    f.run_directory = None;
    let (tx, rx) = mpsc::channel();
    f.plan_receiver = Some(rx);
    let (base, left, right, cancel) = (
        f.base.clone(),
        f.left.clone(),
        f.right.clone(),
        f.cancel.clone(),
    );
    std::thread::spawn(move || {
        let _ = tx.send(sync_plan::build(
            &left,
            &right,
            base.as_deref(),
            mode,
            &cancel,
        ));
    });
    Ok(())
}
fn export(f: &mut FolderView, path: PathBuf, overwrite: bool) -> Result<()> {
    if f.is_running() {
        return Err("処理中です。完了後に保存してください。".into());
    }
    let plan = f.plan.clone().ok_or("同期計画を作成してください")?;
    f.cancel = Arc::default();
    f.export_result = None;
    let cancel = f.cancel.clone();
    let (tx, rx) = mpsc::channel();
    f.export_receiver = Some(rx);
    std::thread::spawn(move || {
        let result = (|| {
            plan.validate_report_path(&path)?;
            let report = plan.report()?;
            yy_merge_core::files::save(&path, report.as_bytes(), &[], overwrite, &cancel)?;
            Ok(format!("JSON保存済み: {}", path.display()))
        })();
        let _ = tx.send(result);
    });
    Ok(())
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Apply,
    Resume,
    Rollback,
}
fn start_run(f: &mut FolderView, run: PathBuf, mode: RunMode) -> Result<()> {
    if f.is_running() {
        return Err("処理中です。完了後に実行してください。".into());
    }
    let plan = if mode != RunMode::Apply {
        None
    } else {
        if f.run_directory.is_some() {
            return Err("再実行するには新しい計画を作成してください。".into());
        }
        let plan = f.plan.as_ref().ok_or("同期計画を作成してください")?;
        if plan.fingerprints.is_none() || plan.exit_code() != 0 {
            return Err("要確認・未比較を解決してから計画を作成し直してください。".into());
        }
        Some((**plan).clone())
    };
    f.cancel = Arc::default();
    f.run_directory = Some(run.clone());
    f.export_result = None;
    let cancel = f.cancel.clone();
    let (tx, rx) = mpsc::channel();
    f.export_receiver = Some(rx);
    std::thread::spawn(move || {
        let result = if let Some(plan) = plan {
            yy_merge_core::sync_run::apply(plan, &run, &cancel)
        } else if mode == RunMode::Resume {
            yy_merge_core::sync_run::resume(&run, &cancel)
        } else {
            yy_merge_core::sync_run::rollback(&run, &cancel)
        };
        let result = result
            .and_then(|report| {
                let message = format!(
                    "{}: {}件 / 実行記録・退避先: {}",
                    match mode {
                        RunMode::Apply => "同期",
                        RunMode::Resume => "同期再開",
                        RunMode::Rollback => "復元",
                    },
                    report.completed,
                    report.run_directory.display()
                );
                if let Some(error) = report.error {
                    Err(format!("{message} / 停止理由: {error}"))
                } else {
                    Ok(message)
                }
            })
            .map_err(|e| format!("{e} / 実行記録を確認: {}", run.display()));
        let _ = tx.send(result);
    });
    Ok(())
}
pub(super) fn command(id: u16) -> Option<Result<()>> {
    if matches!(id, PLAN_APPLY | PLAN_ROLLBACK | PLAN_RESUME) {
        let mode = match id {
            PLAN_ROLLBACK => RunMode::Rollback,
            PLAN_RESUME => RunMode::Resume,
            _ => RunMode::Apply,
        };
        let restoring = mode == RunMode::Rollback;
        let existing = mode != RunMode::Apply;
        let review=app(|a| {
            let f=a.folders.as_ref().ok_or("先にフォルダー比較を開いてください")?;
            if f.is_running() || a.receiver.is_some() {return Err("処理中です。完了後に実行してください。".to_string());}
            if restoring {return Ok("実行記録の内容を逆順に復元します。実行後に変更されたファイルは上書きしません。".to_string());}
            if existing {return Ok("入力・退避データ・準備済みのコピーを検査して、中断した同期を再開します。変更があれば停止します。".to_string());}
            let p=f.plan.as_ref().ok_or("同期計画を作成してください")?;
            if f.run_directory.is_some() || p.fingerprints.is_none() || p.exit_code()!=0 {return Err("新しい未解決項目のない計画が必要です。".into());}
            let count=|action| p.items.iter().filter(|i|i.action==action).count();
            Ok(format!("{}\n左: {}\n右: {}\n新規ファイル {} / 置換 {} / 削除 {} / フォルダー作成 {} / 削除 {}\n元ファイルは退避します。",mode_label(p.mode),p.left.display(),p.right.display(),count(sync_plan::Action::CreateFile),count(sync_plan::Action::ReplaceFile),count(sync_plan::Action::DeleteFile),count(sync_plan::Action::CreateDirectory),count(sync_plan::Action::RemoveDirectory)))
        }).unwrap_or_else(||Err("画面がありません".into()));
        let review = match review {
            Ok(r) => r,
            Err(e) => return Some(Err(e)),
        };
        let Some(selected) = choose_folder(if existing {
            "再開／復元する実行記録フォルダー"
        } else {
            "退避先の親フォルダー（入力の外・同じドライブ）"
        }) else {
            return Some(Ok(()));
        };
        let run = if existing {
            selected
        } else {
            selected.join(format!(
                "yymerge-run-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ))
        };
        let saved = if existing {
            match yy_merge_core::sync_run::read_plan(
                &run.join("plan.json"),
                &Cancellation::default(),
            ) {
                Ok(p) => Some(p),
                Err(e) => return Some(Err(e)),
            }
        } else {
            None
        };
        let review = if let Some(p) = &saved {
            format!(
                "{review}\n\n記録の方式: {}\n左: {}\n右: {}\n全計画の操作数: {}（再開時に完了済み分を判別）",
                mode_label(p.mode),
                p.left.display(),
                p.right.display(),
                p.summary.operations
            )
        } else {
            review
        };
        if !confirm(&format!(
            "{review}\n\n実行記録: {}\n\n実行しますか？",
            run.display()
        )) {
            return Some(Ok(()));
        }
        return Some(
            app(|a| {
                let f = a.folders.as_mut().ok_or("フォルダー比較がありません")?;
                if let Some(plan) = saved {
                    f.left = plan.left.clone();
                    f.right = plan.right.clone();
                    f.base = plan.base.clone();
                    f.entries.clear();
                    f.visible.clear();
                    f.error = Some("実行記録を開きました。比較結果は再走査してください。".into());
                    f.plan_error = None;
                    f.plan = Some(Arc::new(plan));
                }
                start_run(f, run, mode)?;
                a.folder_mode = true;
                a.folders.as_mut().unwrap().view_plan = true;
                super::refresh(a);
                layout(a);
                Ok(())
            })
            .unwrap_or(Ok(())),
        );
    }
    if let Some(mode) = mode(id) {
        return Some(
            app(|a| {
                if a.receiver.is_some() {
                    return Err("ファイル比較中です。完了後に計画を作成してください。".into());
                }
                let f = a
                    .folders
                    .as_mut()
                    .ok_or("先にフォルダー比較を開いてください")?;
                start_plan(f, mode)?;
                a.folder_mode = true;
                super::refresh(a);
                layout(a);
                Ok(())
            })
            .unwrap_or(Ok(())),
        );
    }
    if id == PLAN_SHOW {
        return Some(
            app(|a| {
                let f = a.folders.as_mut().ok_or("先に同期計画を作成してください")?;
                if f.plan.is_none() && f.plan_receiver.is_none() && f.plan_error.is_none() {
                    return Err("先に同期計画を作成してください".into());
                }
                f.view_plan = true;
                f.page = 0;
                a.folder_mode = true;
                super::refresh(a);
                layout(a);
                Ok(())
            })
            .unwrap_or(Ok(())),
        );
    }
    if id == PLAN_SAVE {
        if !app(|a| {
            a.folders
                .as_ref()
                .is_some_and(|f| !f.is_running() && f.plan.is_some())
        })
        .unwrap_or(false)
        {
            return Some(Err("同期計画の作成が完了してから保存してください。".into()));
        }
        let Some(path) = choose_file(true, "同期計画JSONの保存先（すべての入力フォルダーの外）")
        else {
            return Some(Ok(()));
        };
        let overwrite = path.exists();
        if overwrite && !confirm(&format!("{} を置き換えますか？", path.display())) {
            return Some(Ok(()));
        }
        return Some(
            app(|a| {
                export(a.folders.as_mut().unwrap(), path, overwrite)?;
                a.folder_mode = true;
                a.folders.as_mut().unwrap().view_plan = true;
                super::refresh(a);
                layout(a);
                Ok(())
            })
            .unwrap_or(Ok(())),
        );
    }
    None
}
fn selected(f: &FolderView, list: HWND) -> Option<&sync_plan::Item> {
    let i = unsafe { SendMessageW(list, LB_GETCURSEL, None, None).0 };
    if i < 0 {
        return None;
    }
    f.plan.as_ref()?.items.get(f.page * PAGE + i as usize)
}
fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::UpdateRight => "左→右: 追加・更新",
        Mode::UpdateLeft => "右→左: 追加・更新",
        Mode::MirrorRight => "左→右: ミラー",
        Mode::MirrorLeft => "右→左: ミラー",
        Mode::Merge => "祖先付き統合",
    }
}
pub(super) fn refresh(a: &mut App) {
    let Some(f) = &a.folders else {
        return;
    };
    let mut extent = 0;
    unsafe {
        SendMessageW(a.list, WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(a.list, LB_RESETCONTENT, None, None);
        if let Some(plan) = &f.plan {
            for item in plan.items.iter().skip(f.page * PAGE).take(PAGE) {
                let side = match item.direction {
                    Some(Side::Left) => " / 対象: 左",
                    Some(Side::Right) => " / 対象: 右",
                    None => "",
                };
                let label = format!("[{}{side}] {}", item.action.label(), item.path.display());
                extent = extent.max(label.encode_utf16().count());
                SendMessageW(
                    a.list,
                    LB_ADDSTRING,
                    None,
                    Some(LPARAM(HSTRING::from(label).as_ptr() as isize)),
                );
            }
        }
        SendMessageW(
            a.list,
            LB_SETHORIZONTALEXTENT,
            Some(WPARAM(extent * 14 * GetDpiForWindow(a.hwnd) as usize / 96)),
            None,
        );
        SendMessageW(a.list, WM_SETREDRAW, Some(WPARAM(1)), None);
    }
    invalidate(a.list);
    let message = if f.plan_receiver.is_some() {
        "入力を再比較して同期計画を作成中… [停止]で中止できます".into()
    } else if let Some(error) = &f.plan_error {
        format!("計画作成未完了: {error}")
    } else if let Some(plan) = &f.plan {
        format!(
            "計画（全入力対象・比較フィルターなし） | 操作候補{} / 要確認{} / 未比較{} / 維持{} | ページ{}/{}{}",
            plan.summary.operations,
            plan.summary.review,
            plan.summary.uncompared,
            plan.summary.kept,
            f.page + 1,
            plan.items.len().div_ceil(PAGE).max(1),
            if f.export_receiver.is_some() {
                " | 処理中… [停止]で中止（実行済み分は記録・退避を保持）".into()
            } else {
                f.export_result
                    .as_ref()
                    .map(|r| match r {
                        Ok(s) => format!(" | {s}"),
                        Err(e) => format!(" | 処理停止: {e}"),
                    })
                    .unwrap_or_default()
            }
        )
    } else {
        "同期計画を作成してください".into()
    };
    text(a.status, &message);
    let mode = f
        .plan
        .as_ref()
        .map(|p| mode_label(p.mode))
        .unwrap_or("作成中／未完了");
    text(
        a.hwnd,
        &format!(
            "yymerge — 同期計画: {mode}（{}） 左: {} / 右: {}",
            if f.run_directory.is_some() {
                "実行記録あり・結果を確認"
            } else {
                "未実行"
            },
            f.left.display(),
            f.right.display()
        ),
    );
    update_menu(a);
}
pub(super) fn selection(a: &mut App) {
    if let Some(f) = &a.folders
        && let Some(item) = selected(f, a.list)
    {
        text(
            a.status,
            &format!(
                "{}: {} | {} | {} → {} | {}",
                if f.run_directory.is_some() {
                    "実行前の計画"
                } else {
                    "未実行"
                },
                item.action.label(),
                item.path.display(),
                item.source
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "（コピー元なし）".into()),
                item.destination
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "（対象なし）".into()),
                item.reason
            ),
        );
    }
}
pub(super) fn tick(a: &mut App) {
    let Some(f) = &mut a.folders else {
        return;
    };
    let event = f.plan_receiver.as_ref().and_then(|rx| match rx.try_recv() {
        Ok(r) => Some(r),
        Err(mpsc::TryRecvError::Empty) => None,
        Err(_) => Some(Err("plan worker stopped unexpectedly".into())),
    });
    let mut changed = false;
    if let Some(result) = event {
        f.plan_receiver = None;
        changed = true;
        match f.cancel.check().and(result) {
            Ok(plan) => f.plan = Some(Arc::new(plan)),
            Err(e) => {
                f.plan_error = Some(if e == "cancelled" {
                    "停止しました（部分的な計画は破棄しました）".into()
                } else {
                    e
                })
            }
        }
    }
    let event = f
        .export_receiver
        .as_ref()
        .and_then(|rx| match rx.try_recv() {
            Ok(r) => Some(r),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("export worker stopped unexpectedly".into())),
        });
    if let Some(result) = event {
        f.export_receiver = None;
        f.export_result = Some(result);
        changed = true;
    }
    if changed && a.folder_mode {
        super::refresh(a);
    }
}
pub(super) fn exit_code(f: &FolderView) -> i32 {
    if f.is_running() || f.cancel.check().is_err() {
        130
    } else if f.plan_error.is_some() || matches!(f.export_result, Some(Err(_))) {
        2
    } else {
        f.plan.as_ref().map(|p| p.exit_code()).unwrap_or(2)
    }
}
pub(super) fn selected_pair(a: &App) -> Result<Gui> {
    let f = a.folders.as_ref().ok_or("同期計画を選択してください")?;
    let item = selected(f, a.list).ok_or("計画から項目を選択してください")?;
    if item.kinds[1] != Some(NodeKind::File)
        || item.kinds[2] != Some(NodeKind::File)
        || item.action == sync_plan::Action::Uncompared
    {
        return Err(
            "左右に通常ファイルがある項目だけ内容比較を開けます。欠落を空ファイルで補いません。"
                .into(),
        );
    }
    Ok(Gui {
        left: Some(f.left.join(&item.path)),
        right: Some(f.right.join(&item.path)),
        base: if item.kinds[0] == Some(NodeKind::File) {
            f.base.as_ref().map(|p| p.join(&item.path))
        } else {
            None
        },
        encoding: a.options.encoding.clone(),
        ..Default::default()
    })
}
pub(super) fn smoke(a: &mut App, dir: &Path) -> Result<()> {
    start_plan(a.folders.as_mut().unwrap(), Mode::Merge)?;
    super::refresh(a);
    await_smoke_worker(a)?;
    let plan = a
        .folders
        .as_ref()
        .unwrap()
        .plan
        .as_ref()
        .ok_or("native plan was not generated")?;
    if plan.summary.review == 0 || !plan.preview_only {
        return Err("plan lost unresolved conflicts or its preview flag".into());
    }
    if !a.folders.as_ref().unwrap().filter.active()
        || !a.folders.as_ref().unwrap().visible.is_empty()
        || a.folders.as_ref().unwrap().row_count() != plan.items.len()
        || unsafe { SendMessageW(a.list, LB_GETCOUNT, None, None).0 }
            != plan.items.len().min(PAGE) as isize
    {
        return Err("display filter changed sync plan rows".into());
    }
    let index = plan
        .items
        .iter()
        .position(|i| i.action == sync_plan::Action::NeedsMerge)
        .ok_or("plan merge candidate missing")?;
    unsafe {
        SendMessageW(a.list, LB_SETCURSEL, Some(WPARAM(index)), None);
    }
    selection(a);
    if selected_pair(a)?.base.is_none() {
        return Err("plan did not retain ancestor for content merge".into());
    }
    let output = dir.join("sync-preview.json");
    export(a.folders.as_mut().unwrap(), output.clone(), true)?;
    await_smoke_worker(a)?;
    if !matches!(a.folders.as_ref().unwrap().export_result, Some(Ok(_))) {
        return Err("native plan export failed".into());
    }
    let report = std::fs::read_to_string(&output).map_err(|e| e.to_string())?;
    if !report.contains("\"preview_only\": true") {
        return Err("export lost preview-only marker".into());
    }
    let forbidden = a
        .folders
        .as_ref()
        .unwrap()
        .left
        .join("never-write-plan.json");
    export(a.folders.as_mut().unwrap(), forbidden.clone(), false)?;
    await_smoke_worker(a)?;
    if forbidden.exists() || !matches!(a.folders.as_ref().unwrap().export_result, Some(Err(_))) {
        return Err("plan export modified an input tree".into());
    }
    start_plan(a.folders.as_mut().unwrap(), Mode::Merge)?;
    a.folders.as_ref().unwrap().cancel.cancel();
    await_smoke_worker(a)?;
    if a.folders.as_ref().unwrap().plan.is_some() || exit_code(a.folders.as_ref().unwrap()) != 130 {
        return Err("cancelled plan published an executable-looking result".into());
    }
    a.folders.as_mut().unwrap().view_plan = false;
    super::refresh(a);
    if super::exit_code(a) != Some(1) {
        return Err("cancelled plan changed the completed comparison result".into());
    }
    std::fs::write(dir.join("sync-plan-smoke.txt"),"PASS: read-only plan worker, unresolved merge candidates, ancestor navigation, JSON export, input report rejected, cancellation discards proposal\n").map_err(|e| e.to_string())?;
    let left = dir.join("execution-left");
    let right = dir.join("execution-right");
    let run = dir.join("execution-run");
    std::fs::create_dir(&left).map_err(|e| e.to_string())?;
    std::fs::create_dir(&right).map_err(|e| e.to_string())?;
    std::fs::write(left.join("file"), b"updated").map_err(|e| e.to_string())?;
    std::fs::write(right.join("file"), b"original").map_err(|e| e.to_string())?;
    let f = a.folders.as_mut().unwrap();
    f.left = left;
    f.right = right.clone();
    f.base = None;
    start_plan(f, Mode::UpdateRight)?;
    await_smoke_worker(a)?;
    start_run(a.folders.as_mut().unwrap(), run.clone(), RunMode::Apply)?;
    await_smoke_worker(a)?;
    if !matches!(a.folders.as_ref().unwrap().export_result, Some(Ok(_)))
        || std::fs::read(right.join("file")).map_err(|e| e.to_string())? != b"updated"
    {
        return Err("native synchronization worker failed".into());
    }
    if start_run(
        a.folders.as_mut().unwrap(),
        dir.join("second-run"),
        RunMode::Apply,
    )
    .is_ok()
    {
        return Err("native UI allowed stale plan reuse".into());
    }
    // Simulate interruption after the original was backed up but before installation.
    let log_path = run.join("journal.jsonl");
    let log = std::fs::read_to_string(&log_path).map_err(|e| e.to_string())?;
    let log = log
        .strip_suffix("\"Finished\"\n")
        .ok_or("finished event missing")?;
    std::fs::write(&log_path, log).map_err(|e| e.to_string())?;
    std::fs::rename(right.join("file"), run.join("staging/0")).map_err(|e| e.to_string())?;
    start_run(a.folders.as_mut().unwrap(), run.clone(), RunMode::Resume)?;
    await_smoke_worker(a)?;
    if !matches!(a.folders.as_ref().unwrap().export_result, Some(Ok(_)))
        || std::fs::read(right.join("file")).map_err(|e| e.to_string())? != b"updated"
    {
        return Err("native resume worker failed".into());
    }
    start_run(a.folders.as_mut().unwrap(), run, RunMode::Rollback)?;
    await_smoke_worker(a)?;
    if !matches!(a.folders.as_ref().unwrap().export_result, Some(Ok(_)))
        || std::fs::read(right.join("file")).map_err(|e| e.to_string())? != b"original"
    {
        return Err("native restoration worker failed".into());
    }
    std::fs::write(dir.join("sync-resume-smoke.txt"),"PASS: asynchronous resume of backed-up pending replacement, original backup retained, rollback after resume\n").map_err(|e|e.to_string())?;
    std::fs::write(dir.join("sync-run-smoke.txt"),"PASS: asynchronous apply/rollback, originals restored, journal retained, stale plan reuse blocked\n").map_err(|e|e.to_string())
}

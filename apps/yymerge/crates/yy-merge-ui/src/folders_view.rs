//! Read-only folder results. Page the native list to keep large trees responsive.
use super::*;
use windows::Win32::UI::Shell::FOS_PICKFOLDERS;
use yy_merge_core::folders::three::{Entry as ThreeEntry, NodeKind, Status as ThreeStatus};
use yy_merge_core::folders::{Entry, Status};
use yy_merge_core::sync_plan::Plan;
#[path = "folder_filter.rs"]
mod filter_view;
#[path = "folder_session_view.rs"]
mod session_view;
pub(super) use session_view::{
    LoadJob as SessionJob, begin_load as begin_session_load, cancel_load as cancel_session_load,
};
#[path = "sync_preview.rs"]
mod sync_preview;
pub(super) use filter_view::{Controls, enter, layout_controls, notification};
use yy_merge_core::folders::filter::{Facts, Filter, path_key};

struct FolderRow {
    search_key: String,
    entry: Entry,
    three: Option<ThreeStatus>,
    kinds: Option<[Option<NodeKind>; 3]>,
    ancestor_file: bool,
}
impl From<Entry> for FolderRow {
    fn from(entry: Entry) -> Self {
        Self {
            search_key: path_key(&entry.path),
            entry,
            three: None,
            kinds: None,
            ancestor_file: false,
        }
    }
}
impl From<ThreeEntry> for FolderRow {
    fn from(e: ThreeEntry) -> Self {
        let file_pair = e.file_pair();
        let ancestor_file = e.three_files();
        let status = match e.status {
            ThreeStatus::Unchanged => Status::Equal,
            ThreeStatus::SkippedLink => Status::SkippedLink,
            ThreeStatus::Error => Status::Error,
            _ => Status::Different,
        };
        Self {
            search_key: path_key(&e.path),
            ancestor_file,
            three: Some(e.status),
            kinds: Some([e.base, e.left, e.right]),
            entry: Entry {
                path: e.path,
                status,
                file_pair,
                directory: [e.base, e.left, e.right].contains(&Some(NodeKind::Directory)),
                detail: e.detail,
            },
        }
    }
}

const PAGE: usize = 2_000;
pub(super) struct FolderView {
    recursive: bool,
    filter: Filter,
    visible: Vec<usize>,
    filter_due: Option<Instant>,
    controls_dirty: bool,
    view_plan: bool,
    plan: Option<Arc<Plan>>,
    plan_receiver: Option<mpsc::Receiver<Result<Plan>>>,
    plan_error: Option<String>,
    export_receiver: Option<mpsc::Receiver<Result<String>>>,
    export_result: Option<Result<String>>,
    run_directory: Option<PathBuf>,
    base: Option<PathBuf>,
    left: PathBuf,
    right: PathBuf,
    entries: Vec<FolderRow>,
    page: usize,
    receiver: Option<mpsc::Receiver<Result<Vec<FolderRow>>>>,
    pub(super) cancel: Arc<Cancellation>,
    error: Option<String>,
}
impl Drop for FolderView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl FolderView {
    pub(super) fn is_running(&self) -> bool {
        self.receiver.is_some() || self.plan_receiver.is_some() || self.export_receiver.is_some()
    }
    fn row_count(&self) -> usize {
        if self.view_plan {
            self.plan.as_ref().map(|p| p.items.len()).unwrap_or(0)
        } else {
            self.visible.len()
        }
    }
}
fn rebuild_filter(f: &mut FolderView) {
    f.visible = f
        .entries
        .iter()
        .enumerate()
        .filter_map(|(i, row)| {
            let facts = if let Some(status) = row.three {
                let kinds = row.kinds.unwrap();
                Facts::three(status, kinds[1].is_some(), kinds[2].is_some())
            } else {
                Facts::two(&row.entry.status)
            };
            f.filter.matches(&row.search_key, facts).then_some(i)
        })
        .collect();
    if !f.view_plan {
        f.page = 0;
    }
}
fn visible_row(f: &FolderView, index: usize) -> Option<&FolderRow> {
    f.visible.get(index).and_then(|i| f.entries.get(*i))
}
fn choose_folder(title: &str) -> Option<PathBuf> {
    unsafe {
        let d: IFileDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        d.SetOptions(d.GetOptions().ok()? | FOS_PICKFOLDERS).ok()?;
        d.SetTitle(&HSTRING::from(title)).ok()?;
        d.Show(app(|a| a.hwnd)).ok()?;
        let p = d.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let value = p.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(p.as_ptr().cast()));
        value
    }
}
fn start(f: &mut FolderView) {
    f.cancel.cancel();
    f.cancel = Arc::default();
    f.entries.clear();
    f.visible.clear();
    f.page = 0;
    f.error = None;
    f.plan = None;
    f.plan_receiver = None;
    f.plan_error = None;
    f.view_plan = false;
    f.export_receiver = None;
    f.export_result = None;
    let (tx, rx) = mpsc::channel();
    f.receiver = Some(rx);
    let (base, left, right, cancel, recursive) = (
        f.base.clone(),
        f.left.clone(),
        f.right.clone(),
        f.cancel.clone(),
        f.recursive,
    );
    std::thread::spawn(move || {
        let result = if let Some(base) = base {
            yy_merge_core::folders::three::compare(&base, &left, &right, recursive, &cancel)
                .map(|entries| entries.into_iter().map(FolderRow::from).collect())
        } else {
            yy_merge_core::folders::compare(&left, &right, recursive, &cancel)
                .map(|entries| entries.into_iter().map(FolderRow::from).collect())
        };
        let _ = tx.send(result);
    });
}
pub(super) fn command(id: u16) -> Option<Result<()>> {
    if matches!(id, FOLDER_SESSION_OPEN | FOLDER_SESSION_SAVE) {
        return Some(session_view::command(id));
    }
    if let Some(result) = sync_preview::command(id) {
        return Some(result);
    }
    if matches!(id, OPEN_FOLDERS | OPEN_THREE_FOLDERS) {
        if app(|a| a.receiver.is_some()).unwrap_or(false) {
            return Some(Err(
                "ファイル比較中です。完了してからフォルダーを開いてください。".into(),
            ));
        }
        let base = if id == OPEN_THREE_FOLDERS {
            let Some(path) = choose_folder("BASE: 共通祖先のフォルダーを選択") else {
                return Some(Ok(()));
            };
            Some(path)
        } else {
            None
        };
        let Some(left) = choose_folder("LEFT: 左フォルダーを選択（子フォルダーも比較）")
        else {
            return Some(Ok(()));
        };
        let Some(right) = choose_folder("RIGHT: 右フォルダーを選択（子フォルダーも比較）")
        else {
            return Some(Ok(()));
        };
        app(|a| {
            a.folders = Some(FolderView {
                recursive: true,
                filter: Default::default(),
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
                base,
                left,
                right,
                entries: vec![],
                page: 0,
                receiver: None,
                cancel: Arc::default(),
                error: None,
            });
            start(a.folders.as_mut().unwrap());
            a.folder_mode = true;
            refresh(a);
            layout(a);
        });
        return Some(Ok(()));
    }
    if id == OPEN_FOLDER_FILE {
        return Some(open_selected());
    }
    if matches!(id, SHOW_FOLDERS | SHOW_FILES | REFRESH_FOLDERS) {
        return Some(
            app(|a| {
                if id != SHOW_FILES && a.folders.is_none() {
                    return Err(
                        "先に [ファイル] → [フォルダー比較を開く] を選択してください。".into(),
                    );
                }
                a.folder_mode = id != SHOW_FILES;
                if id == SHOW_FOLDERS
                    && let Some(f) = &mut a.folders
                {
                    f.view_plan = false;
                    f.page = 0;
                }
                if id == REFRESH_FOLDERS {
                    start(a.folders.as_mut().unwrap());
                }
                super::refresh(a);
                layout(a);
                Ok(())
            })
            .unwrap_or(Ok(())),
        );
    }
    if app(|a| a.folder_mode).unwrap_or(false) {
        if matches!(id, NEXT | PREV) {
            app(|a| {
                if let Some(f) = &mut a.folders {
                    f.page = if id == PREV {
                        f.page.saturating_sub(1)
                    } else {
                        (f.page + 1).min(f.row_count().saturating_sub(1) / PAGE)
                    };
                }
                refresh(a);
            });
            return Some(Ok(()));
        }
        if matches!(
            id,
            SAVE | LEFT | RIGHT | BASE | BOTH | MANUAL | UNDO | REDO | REBUILD
        ) {
            return Some(Err("フォルダー比較は読み取り専用です。[比較] → [ファイル比較に戻る] でマージ操作に戻れます。".into()));
        }
    }
    None
}
fn label(row: &FolderRow) -> String {
    let entry = &row.entry;
    let status = if let Some(status) = row.three {
        match status {
            ThreeStatus::Unchanged => "祖先から変更なし",
            ThreeStatus::LeftChanged => "左の変更",
            ThreeStatus::RightChanged => "右の変更",
            ThreeStatus::SameChange => "両側で同じ変更",
            ThreeStatus::BothChanged => "両側変更: 内容のマージで判定",
            ThreeStatus::LeftAdded => "左で追加",
            ThreeStatus::RightAdded => "右で追加",
            ThreeStatus::SameAddition => "両側に同じ項目を追加",
            ThreeStatus::AdditionConflict => "衝突: 両側に異なる内容を追加",
            ThreeStatus::LeftDeleted => "左で削除",
            ThreeStatus::RightDeleted => "右で削除",
            ThreeStatus::BothDeleted => "両側で削除",
            ThreeStatus::DeleteModifyConflict => "衝突: 削除と変更",
            ThreeStatus::TypeConflict => "衝突: 項目の種類が異なる",
            ThreeStatus::SkippedLink => "未比較: リンクまたはその配下",
            ThreeStatus::Error => "未比較: 読み取り失敗",
        }
    } else {
        match entry.status {
            Status::Equal if entry.directory => "両側のフォルダー（内容は各行で確認）",
            Status::Equal => "一致",
            Status::Different => "内容が異なる",
            Status::LeftOnly => "左のみ",
            Status::RightOnly => "右のみ",
            Status::SkippedLink => "未比較: リンク",
            Status::TypeMismatch => "種類が異なる",
            Status::Error => "未比較: 読み取り失敗",
        }
    };
    format!(
        "[{status}]  {}{}",
        entry
            .path
            .to_string_lossy()
            .replace(['\r', '\n', '\t'], " "),
        if entry.directory {
            " [フォルダーを含む／配下は各行で確認]"
        } else {
            ""
        }
    )
}
pub(super) fn refresh(a: &mut App) {
    filter_view::sync_controls(a);
    sync_preview::update_menu(a);
    if a.folders.as_ref().is_some_and(|f| f.view_plan) {
        sync_preview::refresh(a);
        return;
    }
    let Some(f) = &a.folders else {
        return;
    };
    unsafe {
        SendMessageW(a.list, WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(a.list, LB_RESETCONTENT, None, None);
        for entry in f
            .visible
            .iter()
            .skip(f.page * PAGE)
            .take(PAGE)
            .filter_map(|i| f.entries.get(*i))
        {
            SendMessageW(
                a.list,
                LB_ADDSTRING,
                None,
                Some(LPARAM(HSTRING::from(label(entry)).as_ptr() as isize)),
            );
        }
        let extent = f
            .visible
            .iter()
            .skip(f.page * PAGE)
            .take(PAGE)
            .filter_map(|i| f.entries.get(*i))
            .map(|e| label(e).encode_utf16().count())
            .max()
            .unwrap_or(0);
        SendMessageW(
            a.list,
            LB_SETHORIZONTALEXTENT,
            Some(WPARAM(extent * 14 * GetDpiForWindow(a.hwnd) as usize / 96)),
            None,
        );
        SendMessageW(a.list, WM_SETREDRAW, Some(WPARAM(1)), None);
    }
    invalidate(a.list);
    let incomplete = f
        .entries
        .iter()
        .filter(|e| matches!(e.entry.status, Status::Error | Status::SkippedLink))
        .count();
    let differences = f
        .entries
        .iter()
        .filter(|e| {
            !matches!(
                e.entry.status,
                Status::Equal | Status::Error | Status::SkippedLink
            )
        })
        .count();
    let state = if f.receiver.is_some() {
        "比較中・停止できます".into()
    } else if let Some(e) = &f.error {
        format!("未完了: {e}（一覧は破棄しました）")
    } else {
        format!(
            "表示{}/全{}項目 / 全体の差異{} / 全体の未比較{} / ページ{}/{}・絞り込みは表示のみ・Enterでファイルを開く",
            f.visible.len(),
            f.entries.len(),
            differences,
            incomplete,
            f.page + 1,
            f.visible.len().div_ceil(PAGE).max(1)
        )
    };
    text(a.status, &state);
    text(
        a.hwnd,
        &format!(
            "yymerge — {}フォルダー比較: {} ⇔ {}{}",
            if f.base.is_some() {
                "3方向"
            } else {
                "2方向"
            },
            f.left.display(),
            f.right.display(),
            f.base
                .as_ref()
                .map(|p| format!(" / BASE: {}", p.display()))
                .unwrap_or_default()
        ),
    );
}

/// Exercise the actual native list, pagination, selection and resize without user dialogs.
pub(super) fn smoke(a: &mut App, dir: &std::path::Path) -> Result<()> {
    let left = dir.join("folders-left");
    let right = dir.join("folders-right");
    for root in [&left, &right] {
        std::fs::create_dir_all(root.join("nested")).map_err(|e| e.to_string())?;
        std::fs::write(root.join("same.txt"), "同じ\n").map_err(|e| e.to_string())?;
    }
    for (root, name, content) in [
        (&left, "nested/different.txt", "左\n"),
        (&right, "nested/different.txt", "右\n"),
        (&left, "left-only.txt", "left"),
        (&right, "right-only.txt", "right"),
    ] {
        std::fs::write(root.join(name), content).map_err(|e| e.to_string())?;
    }
    let entries = yy_merge_core::folders::compare(&left, &right, true, &Cancellation::default())?;
    let pair_index = entries
        .iter()
        .position(|e| e.status == Status::Different && e.file_pair)
        .ok_or("folder pair missing")?;
    if entries.len() != 5 {
        return Err("unexpected folder result count".into());
    }
    a.folders = Some(FolderView {
        recursive: true,
        filter: Default::default(),
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
        base: None,
        left,
        right,
        entries: entries.into_iter().map(FolderRow::from).collect(),
        page: 0,
        receiver: None,
        cancel: Arc::default(),
        error: None,
    });
    a.folder_mode = true;
    rebuild_filter(a.folders.as_mut().unwrap());
    refresh(a);
    layout(a);
    filter_view::smoke(a, dir)?;
    unsafe {
        if SendMessageW(a.list, LB_GETCOUNT, None, None).0 != 5 {
            return Err("native folder list did not show results".into());
        }
        SendMessageW(a.list, LB_SETCURSEL, Some(WPARAM(pair_index)), None);
    }
    selection(a);
    let selected = selected_pair(a)?;
    let f = a.folders.as_mut().unwrap();
    let l = Snapshot::open(
        selected.left.as_ref().unwrap(),
        None,
        &Cancellation::default(),
    )?;
    let r = Snapshot::open(
        selected.right.as_ref().unwrap(),
        None,
        &Cancellation::default(),
    )?;
    if yy_merge_core::compare(&l.text, &r.text, &Cancellation::default())?
        .edits
        .is_empty()
    {
        return Err("selected folder pair lost its difference".into());
    }
    for i in f.entries.len()..PAGE + 3 {
        f.entries.push(
            Entry {
                path: format!("page-{i}.txt").into(),
                status: Status::LeftOnly,
                file_pair: false,
                directory: false,
                detail: None,
            }
            .into(),
        );
    }
    rebuild_filter(f);
    refresh(a);
    unsafe {
        if SendMessageW(a.list, LB_GETCOUNT, None, None).0 != PAGE as isize {
            return Err("folder page limit not applied".into());
        }
    }
    a.folders.as_mut().unwrap().page = 1;
    refresh(a);
    unsafe {
        if SendMessageW(a.list, LB_GETCOUNT, None, None).0 != 3 {
            return Err("folder final page lost rows".into());
        }
    }
    // A real worker error must clear prior results, with an explicit incomplete state.
    a.folders.as_mut().unwrap().left = dir.join("missing-folder");
    start(a.folders.as_mut().unwrap());
    refresh(a);
    if !a.folders.as_ref().unwrap().entries.is_empty() {
        return Err("stale folder results survived refresh".into());
    }
    await_smoke_worker(a)?;
    if a.folders.as_ref().unwrap().error.is_none() || exit_code(a) != Some(2) {
        return Err("folder scan failure was not reported as incomplete".into());
    }
    let f = a.folders.as_mut().unwrap();
    f.left = f.right.clone();
    start(f);
    f.cancel.cancel();
    await_smoke_worker(a)?;
    if !a.folders.as_ref().unwrap().entries.is_empty() || exit_code(a) != Some(130) {
        return Err("cancelled folder scan published a successful result".into());
    }
    let base = dir.join("folders-base");
    std::fs::create_dir_all(base.join("nested")).map_err(|e| e.to_string())?;
    std::fs::write(base.join("nested/different.txt"), "祖先\n").map_err(|e| e.to_string())?;
    std::fs::write(base.join("same.txt"), "同じ\n").map_err(|e| e.to_string())?;
    std::fs::write(base.join("deleted.txt"), "祖先\n").map_err(|e| e.to_string())?;
    std::fs::write(dir.join("folders-right/deleted.txt"), "右の変更\n")
        .map_err(|e| e.to_string())?;
    for (side, data) in [("folders-left", "left"), ("folders-right", "right")] {
        std::fs::write(dir.join(side).join("ancestorless.txt"), data).map_err(|e| e.to_string())?;
    }
    let f = a.folders.as_mut().unwrap();
    f.base = Some(base.clone());
    f.left = dir.join("folders-left");
    start(f);
    await_smoke_worker(a)?;
    if a.folders.as_ref().unwrap().error.is_some() {
        return Err("three-way folder worker failed".into());
    }
    let select = |a: &mut App, status| -> Result<()> {
        let i = a
            .folders
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.three == Some(status))
            .ok_or("three-way folder status missing")?;
        unsafe {
            SendMessageW(a.list, LB_SETCURSEL, Some(WPARAM(i)), None);
        }
        selection(a);
        Ok(())
    };
    select(a, ThreeStatus::BothChanged)?;
    let options = selected_pair(a)?;
    if options.base != Some(base.join("nested/different.txt")) {
        return Err("folder selection did not retain its ancestor file".into());
    }
    let data = load(&options, &Cancellation::default())?;
    if data.merged.as_ref().map(Merge::unresolved) != Some(1) {
        return Err("folder selection did not open a three-way conflict".into());
    }
    select(a, ThreeStatus::AdditionConflict)?;
    if selected_pair(a)?.base.is_some() {
        return Err("missing ancestor was fabricated as an empty file".into());
    }
    select(a, ThreeStatus::DeleteModifyConflict)?;
    if selected_pair(a).is_ok() {
        return Err("deleted file was fabricated as an empty file".into());
    }
    filter_view::smoke_three(a)?;
    sync_preview::smoke(a, dir)?;
    session_view::smoke(a, dir)?;
    a.folders.take();
    a.folder_mode = false;
    super::refresh(a);
    layout(a);
    std::fs::write(dir.join("folders-smoke.txt"), "PASS: two/three-way folders, ancestor selection to file merge, ancestorless additions to two-way view, deletion conflicts blocked, native list, 2000-row pagination, scan failure/cancellation discard partial results, file view restored\n")
        .map_err(|e| e.to_string())
}
pub(super) fn update_menu(a: &App) {
    session_view::update_menu(a);
    sync_preview::update_menu(a);
}
fn await_smoke_worker(a: &mut App) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while a.folders.as_ref().is_some_and(FolderView::is_running) {
        tick(a);
        if Instant::now() > deadline {
            return Err("folder smoke worker timed out".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
pub(super) fn selection(a: &mut App) {
    if a.folders.as_ref().is_some_and(|f| f.view_plan) {
        sync_preview::selection(a);
        return;
    }
    if let Some(f) = &a.folders {
        let index = unsafe { SendMessageW(a.list, LB_GETCURSEL, None, None).0 };
        if index >= 0
            && let Some(e) = visible_row(f, f.page * PAGE + index as usize)
        {
            text(
                a.status,
                &format!(
                    "{}{} | {} | 表示{}/全{}・全体の未比較{}",
                    label(e),
                    e.kinds
                        .map(|k| {
                            let names = k.map(|kind| match kind {
                                Some(NodeKind::File) => "ファイル",
                                Some(NodeKind::Directory) => "フォルダー",
                                Some(NodeKind::Link) => "リンク",
                                None => "欠落",
                            });
                            format!(
                                " | BASE: {} / LEFT: {} / RIGHT: {}",
                                names[0], names[1], names[2]
                            )
                        })
                        .unwrap_or_default(),
                    e.entry.detail.as_deref().unwrap_or(if e.ancestor_file {
                        "ダブルクリックまたはEnter: BASE付き3方向ファイルマージ"
                    } else if e.entry.file_pair && f.base.is_some() {
                        "ダブルクリックまたはEnter: 2方向表示（祖先が通常ファイルではありません）"
                    } else if e.entry.file_pair {
                        "ダブルクリックまたはEnter: 別ウィンドウでファイル比較"
                    } else {
                        "両側に通常ファイルがある項目だけファイル比較を開けます"
                    }),
                    f.visible.len(),
                    f.entries.len(),
                    f.entries
                        .iter()
                        .filter(|e| matches!(e.entry.status, Status::SkippedLink | Status::Error))
                        .count()
                ),
            );
        }
    }
}
pub(super) fn tick(a: &mut App) {
    session_view::tick(a);
    filter_view::tick(a);
    sync_preview::tick(a);
    let Some(f) = &mut a.folders else {
        return;
    };
    let event = f.receiver.as_ref().and_then(|rx| match rx.try_recv() {
        Ok(r) => Some(r),
        Err(mpsc::TryRecvError::Empty) => None,
        Err(_) => Some(Err("folder worker stopped unexpectedly".into())),
    });
    if let Some(result) = event {
        f.receiver = None;
        let result = f.cancel.check().and(result);
        match result {
            Ok(entries) => {
                f.entries = entries;
                rebuild_filter(f);
            }
            Err(e) => {
                f.error = Some(if e == "cancelled" {
                    "停止しました".into()
                } else {
                    e
                })
            }
        }
        if a.folder_mode {
            refresh(a);
        }
    }
}
pub(super) fn exit_code(a: &App) -> Option<i32> {
    if !a.folder_mode {
        return None;
    }
    let f = a.folders.as_ref()?;
    if f.view_plan {
        return Some(sync_preview::exit_code(f));
    }
    Some(
        if f.receiver.is_some() || f.error.as_deref() == Some("停止しました") {
            130
        } else if f.error.is_some()
            || f.entries
                .iter()
                .any(|e| matches!(e.entry.status, Status::Error | Status::SkippedLink))
        {
            2
        } else {
            i32::from(f.entries.iter().any(|e| e.entry.status != Status::Equal))
        },
    )
}
pub(super) fn open_selected() -> Result<()> {
    let pair = app(|a| selected_pair(a)).unwrap_or_else(|| Err("window closed".into()))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new(exe);
    child
        .arg("--left")
        .arg(pair.left.unwrap())
        .arg("--right")
        .arg(pair.right.unwrap());
    if let Some(base) = pair.base {
        child.arg("--base").arg(base);
    }
    if let Some(encoding) = pair.encoding {
        child.arg("--encoding").arg(encoding);
    }
    // Reap the child without blocking this comparison window.
    let mut process = child.spawn().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _ = process.wait();
    });
    Ok(())
}
fn selected_pair(a: &App) -> Result<Gui> {
    if !a.folder_mode {
        return Err("フォルダー比較の一覧を表示してください。".into());
    }
    let f = a.folders.as_ref().ok_or("フォルダーを選択してください")?;
    if f.view_plan {
        return sync_preview::selected_pair(a);
    }
    let index = unsafe { SendMessageW(a.list, LB_GETCURSEL, None, None).0 };
    let e = (index >= 0)
        .then(|| visible_row(f, f.page * PAGE + index as usize))
        .flatten()
        .ok_or("一覧からファイルを選択してください")?;
    if !e.entry.file_pair {
        return Err("両側に通常ファイルがある項目を選択してください。リンク・フォルダー・片側のみ・読み取り失敗の項目は開きません。".into());
    }
    Ok(Gui {
        base: if e.ancestor_file
            && yy_merge_core::office::Kind::detect(&e.entry.path).is_none()
            && !yy_merge_core::images::is_image(&e.entry.path)
        {
            f.base.as_ref().map(|p| p.join(&e.entry.path))
        } else {
            None
        },
        left: Some(f.left.join(&e.entry.path)),
        right: Some(f.right.join(&e.entry.path)),
        encoding: a.options.encoding.clone(),
        ..Default::default()
    })
}

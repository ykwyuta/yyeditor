use std::{
    cell::RefCell,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{Com::*, LibraryLoader::*},
        UI::{
            HiDpi::*,
            Input::KeyboardAndMouse::*,
            Shell::{FileOpenDialog, FileSaveDialog, IFileDialog, SIGDN_FILESYSPATH},
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, PCWSTR, w},
};
use yy_merge_cli::Gui;
use yy_merge_core::{Cancellation, Choice, Diff, Merge, Part, Result, files::Snapshot};
use yy_ui_win::editor::Editor;

#[path = "folders_view.rs"]
mod folders_view;

const FRAME: PCWSTR = w!("YYMergeFrame");
const PANE: PCWSTR = w!("YYMergePane");
const OPEN: u16 = 100;
const OPEN_MERGE: u16 = 101;
const SAVE: u16 = 102;
const PREV: u16 = 103;
const NEXT: u16 = 104;
const LEFT: u16 = 105;
const RIGHT: u16 = 106;
const BASE: u16 = 107;
const BOTH: u16 = 108;
const MANUAL: u16 = 109;
const UNDO: u16 = 110;
const HELP: u16 = 111;
const STOP: u16 = 112;
const REDO: u16 = 113;
const EXIT: u16 = 114;
const REBUILD: u16 = 115;
const OPEN_FOLDERS: u16 = 116;
const SHOW_FOLDERS: u16 = 117;
const SHOW_FILES: u16 = 118;
const REFRESH_FOLDERS: u16 = 119;
const OPEN_FOLDER_FILE: u16 = 120;
const OPEN_THREE_FOLDERS: u16 = 121;
const PLAN_UPDATE_RIGHT: u16 = 122;
const PLAN_UPDATE_LEFT: u16 = 123;
const PLAN_MIRROR_RIGHT: u16 = 124;
const PLAN_MIRROR_LEFT: u16 = 125;
const PLAN_MERGE: u16 = 126;
const PLAN_SHOW: u16 = 127;
const PLAN_SAVE: u16 = 128;
const PLAN_APPLY: u16 = 129;
const PLAN_ROLLBACK: u16 = 130;
const PLAN_RESUME: u16 = 131;
const HELP_MD: &str = include_str!("../../../help/help.md");
const SS_LEFTNOWORDWRAP: u32 = 12;
thread_local! {static APP:RefCell<Option<App>>=const{RefCell::new(None)};}
struct Loaded {
    inputs: Vec<Snapshot>,
    diff: Diff,
    merged: Option<Merge>,
    comparison: Option<super::comparison::Prepared>,
    merge_comparison: Option<super::comparison::MergePrepared>,
    result_text: Option<String>,
}
struct Projection {
    generation: u64,
    version: u64,
    result: Result<super::comparison::MergePrepared>,
}
struct History {
    model: Merge,
    text: String,
    manual: bool,
}
fn history_bytes(h: &History) -> usize {
    h.text.len()
        + h.model
            .parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => t.len(),
                Part::Conflict(c) => {
                    c.base.len()
                        + c.left.len()
                        + c.right.len()
                        + match &c.resolution {
                            Some(Choice::Manual(t)) => t.len(),
                            _ => 0,
                        }
                }
            })
            .sum::<usize>()
}
fn trim_history(a: &mut App) {
    let mut bytes = a
        .history
        .iter()
        .chain(a.redo.iter())
        .map(history_bytes)
        .sum::<usize>();
    while bytes > 64 * 1024 * 1024 {
        let h = if !a.history.is_empty() {
            a.history.remove(0)
        } else if !a.redo.is_empty() {
            a.redo.remove(0)
        } else {
            break;
        };
        bytes = bytes.saturating_sub(history_bytes(&h));
    }
}
struct App {
    folders: Option<folders_view::FolderView>,
    folder_mode: bool,
    hwnd: HWND,
    panes: Vec<HWND>,
    labels: Vec<HWND>,
    editors: Vec<Editor>,
    status: HWND,
    list: HWND,
    buttons: Vec<HWND>,
    font: HFONT,
    options: Gui,
    loaded: Option<Loaded>,
    receiver: Option<mpsc::Receiver<Result<Loaded>>>,
    projection_receiver: Option<mpsc::Receiver<Projection>>,
    projection_cancel: Arc<Cancellation>,
    projection_due: Option<Instant>,
    generation: u64,
    alignment_warning: Option<String>,
    cancel: Arc<Cancellation>,
    selected: usize,
    history: Vec<History>,
    redo: Vec<History>,
    manual: bool,
    saved: bool,
    edited: bool,
    smoke: Option<PathBuf>,
    smoke_phase: u8,
    smoke_result: Option<String>,
    exit: i32,
}
fn app<T>(f: impl FnOnce(&mut App) -> T) -> Option<T> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}
fn win<T>(r: windows::core::Result<T>) -> Result<T> {
    r.map_err(|e| e.to_string())
}
fn text(hwnd: HWND, value: &str) {
    unsafe {
        let _ = SetWindowTextW(hwnd, &HSTRING::from(value));
    }
}
fn invalidate(hwnd: HWND) {
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}
fn error(message: &str) {
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(message),
            w!("yymerge"),
            MB_OK | MB_ICONERROR,
        );
    }
}
fn confirm(message: &str) -> bool {
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(message),
            w!("yymerge"),
            MB_YESNO | MB_ICONQUESTION,
        ) == IDYES
    }
}
fn editor(text: &str, dpi: u32) -> Result<Editor> {
    let mut e = win(Editor::new(text, dpi))?;
    e.set_language(None);
    Ok(e)
}
fn child(parent: HWND, class: PCWSTR, label: &str, id: u16, style: WINDOW_STYLE) -> Result<HWND> {
    unsafe {
        win(CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            &HSTRING::from(label),
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            1,
            1,
            Some(parent),
            Some(HMENU(id as usize as *mut _)),
            None,
            None,
        ))
    }
}
fn load(g: &Gui, c: &Cancellation) -> Result<Loaded> {
    let left = g.left.as_ref().ok_or("左ファイルを選択してください")?;
    let right = g.right.as_ref().ok_or("右ファイルを選択してください")?;
    let mut inputs = vec![
        Snapshot::open(left, g.encoding.as_deref(), c)?,
        Snapshot::open(right, g.encoding.as_deref(), c)?,
    ];
    let diff = yy_merge_core::compare(&inputs[0].text, &inputs[1].text, c)?;
    let merged = if let Some(base) = &g.base {
        inputs.push(Snapshot::open(base, g.encoding.as_deref(), c)?);
        if inputs
            .iter()
            .any(|s| s.encoding != inputs[2].encoding || s.bom != inputs[2].bom)
        {
            return Err("マージ入力の文字コードとBOMを揃えてください".into());
        }
        Some(yy_merge_core::merge(
            &inputs[2].text,
            &inputs[0].text,
            &inputs[1].text,
            c,
        )?)
    } else {
        None
    };
    for input in &inputs {
        input.verify(c)?;
    }
    let result_text = merged.as_ref().map(|m| m.render(true)).transpose()?;
    let (comparison, merge_comparison) = if let Some(result) = &result_text {
        (
            None,
            Some(super::comparison::prepare_merge(
                &inputs[2].text,
                &inputs[0].text,
                &inputs[1].text,
                result,
                c,
            )?),
        )
    } else {
        (
            Some(super::comparison::prepare(
                &inputs[0].text,
                &inputs[1].text,
                &diff,
                c,
            )?),
            None,
        )
    };
    Ok(Loaded {
        inputs,
        diff,
        merged,
        comparison,
        merge_comparison,
        result_text,
    })
}
fn begin(a: &mut App) {
    if a.receiver.is_some() {
        return;
    }
    a.generation = a.generation.wrapping_add(1);
    a.projection_cancel.cancel();
    a.projection_receiver = None;
    a.projection_due = None;
    a.alignment_warning = None;
    a.cancel = Arc::new(Cancellation::default());
    let c = a.cancel.clone();
    let g = a.options.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(load(&g, &c));
    });
    a.receiver = Some(rx);
    a.exit = 130;
    text(a.status, "比較中… [停止] で中止できます");
}
fn install(a: &mut App, mut data: Loaded) -> Result<()> {
    let dpi = unsafe { GetDpiForWindow(a.hwnd) };
    let mut editors = vec![
        editor(&data.inputs[0].text, dpi)?,
        editor(&data.inputs[1].text, dpi)?,
    ];
    if data.merged.is_some() {
        editors.push(editor(&data.inputs[2].text, dpi)?);
        editors.push(editor(
            &data.result_text.take().ok_or("merge result missing")?,
            dpi,
        )?);
        let prepared = data
            .merge_comparison
            .take()
            .ok_or("merge display map missing")?;
        for (editor, (view, marks)) in editors
            .iter_mut()
            .zip(prepared.panes.into_iter().zip(prepared.marks))
        {
            editor.aligned = view;
            editor.marks = marks;
        }
        a.alignment_warning = prepared.warning;
    } else {
        let comparison = data
            .comparison
            .take()
            .ok_or("comparison display map missing")?;
        editors[0].aligned = Some(comparison.left);
        editors[0].marks = comparison.left_marks;
        editors[1].aligned = Some(comparison.right);
        editors[1].marks = comparison.right_marks;
    }
    a.editors = editors;
    a.loaded = Some(data);
    a.history.clear();
    a.redo.clear();
    a.selected = 0;
    a.manual = false;
    a.saved = false;
    a.edited = false;
    refresh(a);
    layout(a);
    jump(a);
    Ok(())
}
fn refresh(a: &mut App) {
    if a.folder_mode {
        folders_view::refresh(a);
        return;
    }
    text(a.hwnd, "yymerge — 比較 / マージ");
    unsafe {
        SendMessageW(a.list, LB_RESETCONTENT, None, None);
        SendMessageW(a.list, LB_SETHORIZONTALEXTENT, Some(WPARAM(0)), None);
    }
    if let Some(data) = &a.loaded {
        if let Some(m) = &data.merged {
            for p in &m.parts {
                if let Part::Conflict(c) = p {
                    let label = format!(
                        "{} #{}  BASE {}–{}",
                        if c.resolution.is_some() { "✓" } else { "!" },
                        c.id,
                        c.base_lines.start + 1,
                        c.base_lines.end.max(c.base_lines.start + 1)
                    );
                    unsafe {
                        SendMessageW(
                            a.list,
                            LB_ADDSTRING,
                            None,
                            Some(LPARAM(HSTRING::from(label).as_ptr() as isize)),
                        );
                    }
                }
            }
            text(
                a.status,
                &format!(
                    "3方向マージ | 未解決 {} | {} | {} | 保存先: {}",
                    m.unresolved(),
                    if a.saved { "保存済み" } else { "未保存" },
                    if a.projection_due.is_some() || a.projection_receiver.is_some() {
                        "整列を更新中"
                    } else if let Some(w) = &a.alignment_warning {
                        w
                    } else if a.editors[3].aligned.is_none() {
                        "原文3ペイン同期 / RESULTは個別表示"
                    } else {
                        "4ペイン同期"
                    },
                    a.options
                        .output
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "[結果を保存] で指定".into())
                ),
            );
        } else {
            for (i, e) in data.diff.edits.iter().enumerate() {
                let label = format!(
                    "差分 {}  左 {} / 右 {}",
                    i + 1,
                    e.base.start + 1,
                    e.replacement.start + 1
                );
                unsafe {
                    SendMessageW(
                        a.list,
                        LB_ADDSTRING,
                        None,
                        Some(LPARAM(HSTRING::from(label).as_ptr() as isize)),
                    );
                }
            }
            let same = data.diff.edits.is_empty()
                && data.inputs[0].encoding == data.inputs[1].encoding
                && data.inputs[0].bom == data.inputs[1].bom;
            a.exit = i32::from(!same);
            text(
                a.status,
                &format!(
                    "{} | {} 差分 | 左右同期 | 左 {} / 右 {} | 改行・BOMも比較対象",
                    if same { "一致" } else { "相違あり" },
                    data.diff.edits.len(),
                    data.inputs[0].encoding,
                    data.inputs[1].encoding
                ),
            );
        }
        for (i, label) in a.labels.iter().enumerate() {
            let s = match i {
                0 => format!("LEFT: {} [読取専用]", data.inputs[0].path.display()),
                1 => format!("RIGHT: {} [読取専用]", data.inputs[1].path.display()),
                2 => data
                    .inputs
                    .get(2)
                    .map(|s| format!("BASE（共通祖先）: {}", s.path.display()))
                    .unwrap_or_default(),
                _ => "RESULT: マージ結果 [編集可能 / Ctrl+Z で編集取消]".into(),
            };
            text(*label, &s);
        }
    }
    unsafe {
        SendMessageW(a.list, LB_SETCURSEL, Some(WPARAM(a.selected)), None);
    }
}
fn count(a: &App) -> usize {
    a.loaded
        .as_ref()
        .map(|d| {
            d.merged
                .as_ref()
                .map(|m| {
                    m.parts
                        .iter()
                        .filter(|p| matches!(p, Part::Conflict(_)))
                        .count()
                })
                .unwrap_or(d.diff.edits.len())
        })
        .unwrap_or(0)
}
fn sync_sources(a: &mut App, source: usize) {
    if source >= a.editors.len() {
        return;
    }
    if let Some(view) = &a.editors[source].aligned {
        let top = view.top;
        let x = a.editors[source].scroll_x;
        for other in 0..a.editors.len() {
            if other == source || a.editors[other].aligned.is_none() {
                continue;
            }
            a.editors[other].aligned.as_mut().unwrap().top = top;
            a.editors[other].scroll_x = x;
            a.editors[other].follow_caret = false;
            invalidate(a.panes[other]);
        }
    }
}
fn request_projection(a: &mut App, immediate: bool) {
    if a.editors.len() != 4 || a.receiver.is_some() {
        return;
    }
    a.generation = a.generation.wrapping_add(1);
    a.projection_cancel.cancel();
    a.projection_receiver = None;
    a.projection_cancel = Arc::new(Cancellation::default());
    a.projection_due = Some(
        Instant::now()
            + if immediate {
                Duration::ZERO
            } else {
                Duration::from_millis(350)
            },
    );
    a.alignment_warning = None;
}
fn apply_projection(a: &mut App, prepared: super::comparison::MergePrepared) {
    let focus = unsafe { GetFocus() };
    let mut anchor = a
        .panes
        .iter()
        .position(|h| *h == focus)
        .unwrap_or(if a.manual { 3 } else { 2 })
        .min(3);
    if prepared.panes[anchor].is_none() {
        anchor = 2;
    }
    let at = a.editors[anchor]
        .aligned
        .as_ref()
        .and_then(|v| v.source_near_row(v.top))
        .unwrap_or(a.editors[anchor].viewport.top);
    for (editor, (view, marks)) in a
        .editors
        .iter_mut()
        .zip(prepared.panes.into_iter().zip(prepared.marks))
    {
        editor.clear_alignment();
        editor.aligned = view;
        editor.marks = marks;
        editor.follow_caret = false;
    }
    if let Some(view) = &mut a.editors[anchor].aligned {
        view.top = view.row_for_offset(at);
    }
    if anchor == 3 {
        a.editors[3].reveal_caret();
    }
    sync_sources(a, anchor);
    invalidate(a.panes[anchor]);
    a.alignment_warning = prepared.warning;
}
fn projection_tick(a: &mut App) {
    if a.projection_due.is_some_and(|due| Instant::now() >= due) {
        a.projection_due = None;
        if let Some(data) = &a.loaded {
            let [left, right, base] = [
                data.inputs[0].text.clone(),
                data.inputs[1].text.clone(),
                data.inputs[2].text.clone(),
            ];
            let snapshot = a.editors[3].document.snapshot().clone();
            let version = a.editors[3].document.version();
            let generation = a.generation;
            let cancel = a.projection_cancel.clone();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let result = cancel
                    .check()
                    .and_then(|_| {
                        String::from_utf8(snapshot.read(0..snapshot.len()))
                            .map_err(|e| e.to_string())
                    })
                    .and_then(|text| {
                        super::comparison::prepare_merge(&base, &left, &right, &text, &cancel)
                    });
                let _ = tx.send(Projection {
                    generation,
                    version,
                    result,
                });
            });
            a.projection_receiver = Some(rx);
        }
    }
    let event = a
        .projection_receiver
        .as_ref()
        .and_then(|rx| match rx.try_recv() {
            Ok(e) => Some(e),
            Err(mpsc::TryRecvError::Disconnected) => Some(Projection {
                generation: a.generation,
                version: a.editors[3].document.version(),
                result: Err("整列ワーカーが終了しました".into()),
            }),
            Err(_) => None,
        });
    if let Some(event) = event {
        a.projection_receiver = None;
        if event.generation == a.generation && a.editors[3].document.version() == event.version {
            match event.result {
                Ok(prepared) => apply_projection(a, prepared),
                Err(e) => {
                    if e != "cancelled" {
                        a.alignment_warning = Some(e);
                    }
                }
            }
            refresh(a);
        }
    }
}
fn jump(a: &mut App) {
    let positions = a
        .loaded
        .as_ref()
        .map(|d| {
            if let Some(m) = &d.merged {
                let conflicts: Vec<_> = m
                    .parts
                    .iter()
                    .filter_map(|p| {
                        if let Part::Conflict(c) = p {
                            Some(c)
                        } else {
                            None
                        }
                    })
                    .collect();
                conflicts
                    .get(a.selected)
                    .map(|c| {
                        vec![
                            (0, c.left_lines.start),
                            (1, c.right_lines.start),
                            (2, c.base_lines.start),
                        ]
                    })
                    .unwrap_or_default()
            } else {
                d.diff
                    .edits
                    .get(a.selected)
                    .map(|e| vec![(0, e.base.start), (1, e.replacement.start)])
                    .unwrap_or_default()
            }
        })
        .unwrap_or_default();
    for (i, line) in positions {
        let source = &a.loaded.as_ref().unwrap().inputs[i].text;
        let at = if line == 0 {
            0
        } else {
            source
                .match_indices('\n')
                .nth(line - 1)
                .map_or(source.len() as u64, |(i, _)| i as u64 + 1)
        };
        a.editors[i]
            .document
            .set_selections(yy_core::SelectionSet::single(yy_core::Selection::caret(at)));
        a.editors[i].viewport.top = at;
        if let Some(view) = &mut a.editors[i].aligned {
            view.top = view.row_for_offset(at);
        }
        a.editors[i].follow_caret = false;
        invalidate(a.panes[i]);
    }
    if !a.manual
        && let Some(m) = a.loaded.as_ref().and_then(|d| d.merged.as_ref())
        && let Ok((_, ranges)) = m.render_indexed(true)
        && let Some((_, range)) = ranges.get(a.selected)
    {
        let e = &mut a.editors[3];
        let at = range.start as u64;
        e.document
            .set_selections(yy_core::SelectionSet::single(yy_core::Selection::caret(at)));
        e.viewport.top = yy_core::motion::line_start(e.document.snapshot(), at);
        e.follow_caret = false;
        if let Some(view) = &mut e.aligned {
            view.top = view.row_for_offset(at);
        }
        invalidate(a.panes[3]);
    }
    let source_count = if a.editors.len() == 4 { 3 } else { 2 };
    if let Some(top) = a.editors[..source_count]
        .iter()
        .filter_map(|e| e.aligned.as_ref().map(|v| v.top))
        .min()
    {
        for (i, e) in a.editors.iter_mut().enumerate() {
            if let Some(view) = &mut e.aligned {
                view.top = top;
                invalidate(a.panes[i]);
            }
        }
    }
    unsafe {
        SendMessageW(a.list, LB_SETCURSEL, Some(WPARAM(a.selected)), None);
    }
}
fn capture(a: &App) -> Option<History> {
    Some(History {
        model: a.loaded.as_ref()?.merged.clone()?,
        text: a.editors.get(3)?.text(),
        manual: a.manual,
    })
}
fn restore(a: &mut App, h: History) -> Result<()> {
    a.loaded.as_mut().unwrap().merged = Some(h.model);
    a.editors[3] = editor(&h.text, unsafe { GetDpiForWindow(a.hwnd) })?;
    a.manual = h.manual;
    a.saved = false;
    a.edited = true;
    request_projection(a, true);
    refresh(a);
    jump(a);
    invalidate(a.panes[3]);
    Ok(())
}
fn resolve(a: &mut App, choice: Option<Choice>) -> Result<()> {
    if a.receiver.is_some() {
        return Ok(());
    }
    if a.manual {
        return Err("手動編集した結果を保護しています。競合選択に戻るには、マージ操作の取消で手動編集前に戻してください。手動編集の完了を確定すると保存できます。".into());
    }
    let Some(h) = capture(a) else {
        return Ok(());
    };
    let m = a.loaded.as_mut().unwrap().merged.as_mut().unwrap();
    let id = m
        .parts
        .iter()
        .filter_map(|p| {
            if let Part::Conflict(c) = p {
                Some(c.id)
            } else {
                None
            }
        })
        .nth(a.selected)
        .ok_or("競合を選択してください")?;
    m.resolve(id, choice)?;
    a.history.push(h);
    a.redo.clear();
    a.editors[3] = editor(&m.render(true)?, unsafe { GetDpiForWindow(a.hwnd) })?;
    a.saved = false;
    a.edited = true;
    request_projection(a, true);
    refresh(a);
    jump(a);
    invalidate(a.panes[3]);
    Ok(())
}
fn choose_file(save: bool, title: &str) -> Option<PathBuf> {
    unsafe {
        let d: IFileDialog = CoCreateInstance(
            if save {
                &FileSaveDialog
            } else {
                &FileOpenDialog
            },
            None,
            CLSCTX_INPROC_SERVER,
        )
        .ok()?;
        d.SetTitle(&HSTRING::from(title)).ok()?;
        d.Show(app(|a| a.hwnd)).ok()?;
        let p = d.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.as_ptr().cast()));
        s.map(PathBuf::from)
    }
}
fn command(id: u16) -> Result<()> {
    if let Some(result) = folders_view::command(id) {
        return result;
    }
    if id == HELP {
        return win(yy_ui_win::help::show(
            HELP_MD,
            "yymerge ヘルプ",
            None,
            "yymerge-help",
        ));
    }
    if id == STOP {
        app(|a| {
            if let Some(f) = &a.folders
                && f.is_running()
            {
                f.cancel.cancel();
            }
            a.cancel.cancel();
            a.projection_cancel.cancel();
            a.projection_due = None;
        });
        return Ok(());
    }
    if id == REBUILD {
        app(|a| {
            request_projection(a, true);
            refresh(a);
        });
        return Ok(());
    }
    if id == EXIT {
        if let Some(hwnd) = app(|a| a.hwnd) {
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        return Ok(());
    }
    if matches!(id, OPEN | OPEN_MERGE) {
        if app(|a| a.receiver.is_some()).unwrap_or(false) {
            return Err("比較中です。停止してから開いてください。".into());
        }
        if app(|a| a.edited && !a.saved).unwrap_or(false)
            && !confirm("未保存の結果を破棄して別の比較を開きますか？")
        {
            return Ok(());
        }
        let base = if id == OPEN_MERGE {
            let Some(p) = choose_file(false, "BASE: 共通祖先を選択") else {
                return Ok(());
            };
            Some(p)
        } else {
            None
        };
        let Some(left) = choose_file(false, "LEFT: 左ファイルを選択") else {
            return Ok(());
        };
        let Some(right) = choose_file(false, "RIGHT: 右ファイルを選択") else {
            return Ok(());
        };
        app(|a| {
            a.folder_mode = false;
            a.options = Gui {
                base,
                left: Some(left),
                right: Some(right),
                encoding: a.options.encoding.clone(),
                ..Default::default()
            };
            begin(a);
            layout(a);
        });
        return Ok(());
    }
    if id == SAVE {
        let ready = app(|a| {
            a.receiver.is_none()
                && a.loaded
                    .as_ref()
                    .and_then(|d| d.merged.as_ref())
                    .is_some_and(|m| m.unresolved() == 0)
        })
        .unwrap_or(false);
        if !ready {
            return Err("未解決の競合があります。左右の選択、または手動編集の完了を確定してから保存してください。".into());
        }
        let Some(path) = app(|a| a.options.output.clone())
            .flatten()
            .or_else(|| choose_file(true, "RESULT: 結果の保存先（入力とは別のファイル）"))
        else {
            return Ok(());
        };
        let overwrite = path.exists();
        if overwrite && !confirm(&format!("{} を置き換えますか？", path.display())) {
            return Ok(());
        }
        return app(|a| {
            let d = a.loaded.as_ref().unwrap();
            let bytes = d.inputs[2].encode(&a.editors[3].text())?;
            let refs = d.inputs.iter().collect::<Vec<_>>();
            yy_merge_core::files::save(&path, &bytes, &refs, overwrite, &Cancellation::default())?;
            a.options.output = Some(path);
            a.saved = true;
            a.exit = 0;
            refresh(a);
            Ok(())
        })
        .unwrap_or(Ok(()));
    }
    if id == MANUAL {
        if !app(|a| a.editors.len() == 4).unwrap_or(false) {
            return Ok(());
        }
        if !confirm(
            "RESULTの内容を、すべての競合を手動解決した最終結果として確定しますか？ マーカー文字列が残っていないかも確認してください。",
        ) {
            return Ok(());
        }
        return app(|a| {
            if let Some(h) = capture(a) {
                a.history.push(h);
                a.redo.clear();
            }
            for part in &mut a.loaded.as_mut().unwrap().merged.as_mut().unwrap().parts {
                if let Part::Conflict(c) = part
                    && c.resolution.is_none()
                {
                    c.resolution = Some(Choice::Manual(String::new()));
                }
            }
            a.manual = true;
            a.saved = false;
            a.edited = true;
            refresh(a);
            Ok(())
        })
        .unwrap_or(Ok(()));
    }
    app(|a| match id {
        LEFT => resolve(a, Some(Choice::Left)),
        RIGHT => resolve(a, Some(Choice::Right)),
        BASE => resolve(a, Some(Choice::Base)),
        BOTH => resolve(a, Some(Choice::Both)),
        NEXT | PREV => {
            let n = count(a);
            if n > 0 {
                a.selected = if id == NEXT {
                    (a.selected + 1) % n
                } else {
                    (a.selected + n - 1) % n
                };
                jump(a);
            }
            Ok(())
        }
        UNDO | REDO => {
            let h = if id == UNDO {
                a.history.pop()
            } else {
                a.redo.pop()
            };
            if let Some(h) = h {
                if let Some(now) = capture(a) {
                    if id == UNDO {
                        a.redo.push(now)
                    } else {
                        a.history.push(now)
                    }
                }
                restore(a, h)?;
            }
            Ok(())
        }
        _ => Ok(()),
    })
    .unwrap_or(Ok(()))
}
fn layout(a: &mut App) {
    folders_view::update_menu(a);
    unsafe {
        let mut r = RECT::default();
        let _ = GetClientRect(a.hwnd, &mut r);
        let dpi = GetDpiForWindow(a.hwnd) as i32;
        let unit = |x| x * dpi / 96;
        let w = r.right.max(1);
        let h = r.bottom.max(1);
        let bar = unit(34);
        let label = unit(25);
        let status = unit(26);
        for (i, b) in a.buttons.iter().enumerate() {
            let _ = MoveWindow(
                *b,
                unit(6 + i as i32 * 97),
                unit(3),
                unit(91),
                unit(28),
                true,
            );
        }
        let side = unit(230).min(w / 3);
        let area = (w - side - unit(12)).max(2);
        let top = bar + unit(5);
        let bottom = (h - status - unit(5)).max(top + 2);
        for (i, label) in if a.folder_mode {
            ["▱ 開く", "▣ 保存", "↑ 前ページ", "↓ 次ページ", "■ 停止"]
        } else {
            ["▱ 開く", "▣ 保存", "↑ 前へ", "↓ 次へ", "■ 停止"]
        }
        .iter()
        .enumerate()
        {
            text(a.buttons[i], label);
        }
        let _ = EnableWindow(a.buttons[1], !a.folder_mode);
        if a.folder_mode {
            for hwnd in a.panes.iter().chain(&a.labels) {
                let _ = ShowWindow(*hwnd, SW_HIDE);
            }
            let _ = MoveWindow(
                a.list,
                unit(4),
                top,
                (w - unit(8)).max(1),
                (bottom - top).max(1),
                true,
            );
            let _ = MoveWindow(
                a.status,
                unit(5),
                h - status,
                (w - unit(10)).max(1),
                status,
                true,
            );
            return;
        }
        let half = area / 2;
        let merge = a.editors.len() == 4;
        let _ = MoveWindow(
            a.list,
            area + unit(8),
            top,
            side,
            (bottom - top).max(1),
            true,
        );
        let _ = MoveWindow(
            a.status,
            unit(5),
            h - status,
            (w - unit(10)).max(1),
            status,
            true,
        );
        for i in 0..4 {
            let visible = i < 2 || merge;
            let _ = ShowWindow(a.panes[i], if visible { SW_SHOW } else { SW_HIDE });
            let _ = ShowWindow(a.labels[i], if visible { SW_SHOW } else { SW_HIDE });
            if !visible {
                continue;
            }
            let (x, y, pw, ph) = if merge {
                if i < 3 {
                    let third = area / 3;
                    (
                        unit(4) + i as i32 * third,
                        top,
                        third - unit(4),
                        ((bottom - top) * 45 / 100).max(label + 1),
                    )
                } else {
                    (
                        unit(4),
                        top + (bottom - top) * 45 / 100 + unit(4),
                        area - unit(4),
                        ((bottom - top) * 55 / 100 - unit(4)).max(label + 1),
                    )
                }
            } else {
                (
                    unit(4) + i as i32 * half,
                    top,
                    half - unit(4),
                    (bottom - top).max(label + 1),
                )
            };
            let _ = MoveWindow(a.labels[i], x, y, pw.max(1), label, true);
            let _ = MoveWindow(
                a.panes[i],
                x,
                y + label,
                pw.max(1),
                (ph - label).max(1),
                true,
            );
            invalidate(a.panes[i]);
        }
    }
}
fn menu() -> Result<HMENU> {
    unsafe {
        let root = win(CreateMenu())?;
        for (title, items) in [
            (
                "ファイル(&F)",
                vec![
                    (OPEN, "2方向比較を開く…"),
                    (OPEN_MERGE, "3方向マージを開く…"),
                    (OPEN_FOLDERS, "フォルダー比較を開く…"),
                    (OPEN_THREE_FOLDERS, "3方向フォルダー比較を開く…"),
                    (SAVE, "結果を保存…\tCtrl+S"),
                    (EXIT, "終了"),
                ],
            ),
            (
                "比較(&D)",
                vec![
                    (PREV, "前の差分・競合\tShift+F7"),
                    (NEXT, "次の差分・競合\tF7"),
                    (STOP, "処理を停止"),
                    (REBUILD, "3方向マージの整列を更新"),
                    (SHOW_FOLDERS, "フォルダー比較の結果に戻る"),
                    (SHOW_FILES, "ファイル比較に戻る"),
                    (REFRESH_FOLDERS, "フォルダー比較を再実行"),
                    (OPEN_FOLDER_FILE, "選択したファイルを比較…"),
                ],
            ),
            (
                "マージ(&M)",
                vec![
                    (LEFT, "選択した競合: LEFTを採用"),
                    (RIGHT, "選択した競合: RIGHTを採用"),
                    (BASE, "選択した競合: BASEを採用"),
                    (BOTH, "選択した競合: LEFT→RIGHTの順に両方"),
                    (MANUAL, "手動編集の完了を確定…"),
                    (UNDO, "マージ操作を取り消す"),
                    (REDO, "マージ操作をやり直す"),
                ],
            ),
            (
                "同期計画(&S)",
                vec![
                    (PLAN_UPDATE_RIGHT, "左 → 右: 追加・更新の計画"),
                    (PLAN_UPDATE_LEFT, "右 → 左: 追加・更新の計画"),
                    (PLAN_MIRROR_RIGHT, "左 → 右: ミラーの計画（削除候補を含む）"),
                    (PLAN_MIRROR_LEFT, "右 → 左: ミラーの計画（削除候補を含む）"),
                    (PLAN_MERGE, "祖先付き: 左右の統合計画"),
                    (PLAN_SHOW, "同期計画を表示"),
                    (PLAN_SAVE, "同期計画をJSONで保存…"),
                    (PLAN_APPLY, "計画を確認して実行…"),
                    (PLAN_ROLLBACK, "実行記録から復元…"),
                    (PLAN_RESUME, "実行記録から同期を再開…"),
                    (SHOW_FOLDERS, "フォルダー比較の結果に戻る"),
                ],
            ),
            ("ヘルプ(&H)", vec![(HELP, "yymergeの使い方\tF1")]),
        ] {
            let sub = win(CreatePopupMenu())?;
            for (id, label) in items {
                win(AppendMenuW(
                    sub,
                    MF_STRING,
                    id as usize,
                    &HSTRING::from(label),
                ))?;
            }
            win(AppendMenuW(
                root,
                MF_POPUP,
                sub.0 as usize,
                &HSTRING::from(title),
            ))?;
        }
        Ok(root)
    }
}

fn control_font(dpi: u32) -> HFONT {
    unsafe {
        CreateFontW(
            -(14 * dpi as i32 / 96),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            FF_MODERN.0 as u32,
            w!("UDEV Gothic"),
        )
    }
}
fn update_dpi(a: &mut App, dpi: u32) {
    for editor in &mut a.editors {
        editor.dpi = dpi;
        editor.renderer.set_dpi(dpi);
    }
    let font = control_font(dpi);
    if !font.0.is_null() {
        for hwnd in a
            .labels
            .iter()
            .chain(a.buttons.iter())
            .chain([&a.list, &a.status])
        {
            unsafe {
                SendMessageW(
                    *hwnd,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
        }
        let old = std::mem::replace(&mut a.font, font);
        unsafe {
            let _ = DeleteObject(old.into());
        }
    }
    layout(a);
}
pub fn run(options: Gui) -> Result<i32> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        win(CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok())?;
        yy_ui_win::font::register_gdi();
        let instance = win(GetModuleHandleW(None))?;
        let icon = LoadIconW(
            Some(instance.into()),
            PCWSTR(std::ptr::without_provenance(1)),
        )
        .unwrap_or_default();
        for (name, proc) in [
            (
                FRAME,
                frame_proc as unsafe extern "system" fn(_, _, _, _) -> _,
            ),
            (PANE, pane_proc),
        ] {
            let wc = WNDCLASSW {
                lpszClassName: name,
                lpfnWndProc: Some(proc),
                hInstance: instance.into(),
                hIcon: icon,
                hCursor: win(LoadCursorW(
                    None,
                    if name == PANE { IDC_IBEAM } else { IDC_ARROW },
                ))?,
                hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
                style: CS_HREDRAW | CS_VREDRAW,
                ..Default::default()
            };
            RegisterClassW(&wc);
        }
        let hwnd = win(CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            FRAME,
            w!("yymerge — 比較 / マージ"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1280,
            820,
            None,
            None,
            Some(instance.into()),
            None,
        ))?;
        win(SetMenu(hwnd, Some(menu()?)))?;
        let dpi = GetDpiForWindow(hwnd);
        let font = control_font(dpi);
        let mut panes = Vec::new();
        let mut labels = Vec::new();
        for i in 0..4 {
            panes.push(child(hwnd, PANE, "", 200 + i, WS_TABSTOP)?);
            labels.push(child(
                hwnd,
                w!("STATIC"),
                if i == 0 {
                    "LEFT: [開く] で比較するファイルを選択"
                } else if i == 1 {
                    "RIGHT: 読み取り専用"
                } else {
                    ""
                },
                220 + i,
                WINDOW_STYLE(SS_LEFTNOWORDWRAP),
            )?);
        }
        let list = child(
            hwnd,
            w!("LISTBOX"),
            "",
            240,
            WS_TABSTOP | WS_VSCROLL | WS_HSCROLL | WS_BORDER | WINDOW_STYLE(LBS_NOTIFY as u32),
        )?;
        let status = child(
            hwnd,
            w!("STATIC"),
            "[開く] または [ファイル] → [3方向マージを開く] から開始できます。F1: ヘルプ",
            241,
            WINDOW_STYLE(SS_LEFTNOWORDWRAP),
        )?;
        let mut buttons = Vec::new();
        for (id, label) in [
            (OPEN, "▱ 開く"),
            (SAVE, "▣ 保存"),
            (PREV, "↑ 前へ"),
            (NEXT, "↓ 次へ"),
            (STOP, "■ 停止"),
        ] {
            buttons.push(child(hwnd, w!("BUTTON"), label, id, WS_TABSTOP)?);
        }
        for h in labels.iter().chain(buttons.iter()).chain([&list, &status]) {
            SendMessageW(
                *h,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
        let smoke = options.ui_smoke_test.clone();
        let should_load = options.left.is_some() && options.right.is_some();
        let mut state = App {
            folders: None,
            folder_mode: false,
            hwnd,
            panes,
            labels,
            editors: vec![editor("", dpi)?, editor("", dpi)?],
            status,
            list,
            buttons,
            font,
            options,
            loaded: None,
            receiver: None,
            projection_receiver: None,
            projection_cancel: Arc::default(),
            projection_due: None,
            generation: 0,
            alignment_warning: None,
            cancel: Arc::default(),
            selected: 0,
            history: vec![],
            redo: vec![],
            manual: false,
            saved: false,
            edited: false,
            smoke,
            smoke_phase: 0,
            smoke_result: None,
            exit: 130,
        };
        if state.smoke.is_some() && !should_load {
            let dir = state.smoke.as_ref().unwrap();
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            for (name, data) in [
                ("base.txt", "共通\na\nb\n"),
                ("left.txt", "共通\nLEFT\nb\n"),
                ("right.txt", "共通\nRIGHT\nb\n"),
            ] {
                std::fs::write(dir.join(name), data).map_err(|e| e.to_string())?;
            }
            state.options.base = Some(dir.join("base.txt"));
            state.options.left = Some(dir.join("left.txt"));
            state.options.right = Some(dir.join("right.txt"));
            state.options.output = Some(dir.join("result.txt"));
            begin(&mut state);
        } else if should_load {
            begin(&mut state);
        }
        APP.with(|a| *a.borrow_mut() = Some(state));
        app(layout);
        let _ = ShowWindow(hwnd, SW_SHOW);
        SetTimer(Some(hwnd), 1, 50, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if yy_ui_win::help::translate(&msg) {
                continue;
            }
            if msg.message == WM_KEYDOWN
                && msg.wParam.0 == 13
                && app(|a| a.folder_mode && msg.hwnd == a.list).unwrap_or(false)
            {
                if let Err(e) = folders_view::open_selected() {
                    error(&e);
                }
                continue;
            }
            if msg.message == WM_KEYDOWN
                && let Some(id) = shortcut(msg.wParam.0)
            {
                if let Err(e) = command(id) {
                    error(&e);
                }
                app(trim_history);
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let code = app(|a| a.exit).unwrap_or(2);
        yy_ui_win::help::close();
        app(|a| {
            a.cancel.cancel();
            a.projection_cancel.cancel();
            let _ = DeleteObject(a.font.into());
        });
        APP.with(|a| a.borrow_mut().take());
        CoUninitialize();
        Ok(code)
    }
}

unsafe extern "system" fn frame_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            app(layout);
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            unsafe {
                let p = &mut *(l.0 as *mut MINMAXINFO);
                let dpi = GetDpiForWindow(hwnd) as i32;
                p.ptMinTrackSize.x = 680 * dpi / 96;
                p.ptMinTrackSize.y = 420 * dpi / 96;
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            unsafe {
                let r = &*(l.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER,
                );
            }
            app(|a| {
                update_dpi(a, unsafe { GetDpiForWindow(hwnd) });
            });
            LRESULT(0)
        }
        WM_COMMAND => {
            if w.0 as u16 == 240 && app(|a| a.folder_mode).unwrap_or(false) {
                if (w.0 >> 16) as u32 == LBN_DBLCLK {
                    if let Err(e) = folders_view::open_selected() {
                        error(&e);
                    }
                } else if (w.0 >> 16) as u32 == LBN_SELCHANGE {
                    app(folders_view::selection);
                }
            } else if w.0 as u16 == 240 && (w.0 >> 16) as u32 == LBN_SELCHANGE {
                app(|a| {
                    let i = unsafe { SendMessageW(a.list, LB_GETCURSEL, None, None).0 };
                    if i >= 0 {
                        a.selected = i as usize;
                        jump(a);
                    }
                });
            } else if let Err(e) = command(w.0 as u16) {
                error(&e);
            }
            app(trim_history);
            LRESULT(0)
        }
        WM_TIMER => {
            app(folders_view::tick);
            app(projection_tick);
            let event = app(|a| {
                a.receiver.as_ref().and_then(|r| match r.try_recv() {
                    Ok(e) => Some(e),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Some(Err("comparison worker stopped unexpectedly".into()))
                    }
                    Err(_) => None,
                })
            })
            .flatten();
            if let Some(result) = event {
                app(|a| a.receiver = None);
                let result = result.and_then(|data| app(|a| install(a, data)).unwrap_or(Ok(())));
                if let Err(e) = result {
                    text(app(|a| a.status).unwrap_or_default(), &e);
                    if app(|a| a.smoke.is_some()).unwrap_or(false) {
                        app(|a| {
                            let _ =
                                std::fs::write(a.smoke.as_ref().unwrap().join("ui-error.txt"), &e);
                            a.exit = 2;
                        });
                        unsafe {
                            let _ = DestroyWindow(hwnd);
                        }
                    } else {
                        error(&e);
                    }
                } else if app(|a| a.smoke.is_some()).unwrap_or(false) {
                    let result = app(smoke).unwrap_or(Ok(()));
                    if let Err(e) = result {
                        app(|a| {
                            let _ =
                                std::fs::write(a.smoke.as_ref().unwrap().join("ui-error.txt"), e);
                            a.exit = 2;
                        });
                        unsafe {
                            let _ = DestroyWindow(hwnd);
                        }
                    }
                }
            }
            let smoke_result = app(smoke_tick).unwrap_or(Ok(false));
            match smoke_result {
                Ok(true) => unsafe {
                    let _ = DestroyWindow(hwnd);
                },
                Err(e) => {
                    app(|a| {
                        if let Some(dir) = &a.smoke {
                            let _ = std::fs::write(dir.join("ui-error.txt"), e);
                        }
                        a.exit = 2;
                    });
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            if app(|a| a.edited && !a.saved).unwrap_or(false)
                && !confirm("未保存の結果を破棄して閉じますか？")
            {
                return LRESULT(0);
            }
            app(|a| {
                if let Some(code) = folders_view::exit_code(a) {
                    a.exit = code;
                }
                a.cancel.cancel();
                a.projection_cancel.cancel();
                if a.loaded.as_ref().is_some_and(|d| d.merged.is_some()) && !a.saved {
                    a.exit = 130;
                }
            });
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}
fn shortcut(key: usize) -> Option<u16> {
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    match key {
        112 => Some(HELP),
        118 => Some(if shift { PREV } else { NEXT }),
        83 if ctrl => Some(SAVE),
        _ => None,
    }
}
unsafe extern "system" fn pane_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_KEYDOWN
        && let Some(id) = shortcut(w.0)
    {
        if let Err(e) = command(id) {
            error(&e);
        }
        return LRESULT(0);
    }
    let result = app(|a| {
        let i = a.panes.iter().position(|h| *h == hwnd)?;
        if i >= a.editors.len() {
            return None;
        }
        let editable = i == 3 && a.receiver.is_none();
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let mutation = msg == WM_CHAR && (w.0 >= 32 || matches!(w.0, 9 | 13))
            || msg == WM_IME_COMPOSITION
            || msg == WM_KEYDOWN
                && (matches!(w.0, 8 | 46) || ctrl && matches!(w.0, 86 | 88 | 89 | 90));
        let history = if editable && mutation && !a.manual {
            capture(a)
        } else {
            None
        };
        let e = &mut a.editors[i];
        let before = e.document.version();
        let value = match msg {
            WM_PAINT => {
                unsafe {
                    let mut ps = PAINTSTRUCT::default();
                    BeginPaint(hwnd, &mut ps);
                    let mut r = RECT::default();
                    let _ = GetClientRect(hwnd, &mut r);
                    let _ = e.paint(hwnd, r.right.max(1) as u32, r.bottom.max(1) as u32);
                    let _ = EndPaint(hwnd, &ps);
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_GETDLGCODE => {
                LRESULT((DLGC_WANTARROWS | DLGC_WANTCHARS | DLGC_WANTALLKEYS) as isize)
            }
            WM_CHAR => {
                if editable {
                    e.follow_caret = true;
                    e.character(w.0 as u16);
                }
                invalidate(hwnd);
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
                let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
                if editable || matches!(w.0, 33..=40) || (ctrl && matches!(w.0, 65 | 67)) {
                    e.follow_caret = true;
                    e.key(hwnd, w.0 as u32, ctrl, shift);
                }
                invalidate(hwnd);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                unsafe {
                    let _ = SetFocus(Some(hwnd));
                    SetCapture(hwnd);
                }
                e.follow_caret = true;
                e.click(
                    l.0 as i16 as i32,
                    (l.0 >> 16) as i16 as i32,
                    unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0,
                );
                invalidate(hwnd);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                unsafe {
                    if GetCapture() == hwnd {
                        let _ = ReleaseCapture();
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE if unsafe { GetCapture() } == hwnd => {
                let (x, y) = (l.0 as i16 as i32, (l.0 >> 16) as i16 as i32);
                let mut r = RECT::default();
                unsafe {
                    let _ = GetClientRect(hwnd, &mut r);
                }
                if y < 0 {
                    e.scroll_rows(-1);
                } else if y >= r.bottom {
                    e.scroll_rows(1);
                }
                e.click(x, y.clamp(0, (r.bottom - 1).max(0)), true);
                e.follow_caret = true;
                invalidate(hwnd);
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = -((w.0 >> 16) as i16 as i64) / 120 * 3;
                let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
                if shift {
                    e.scroll_x = (e.scroll_x + delta as f32 * 30.).max(0.);
                } else {
                    e.scroll_rows(delta);
                }
                invalidate(hwnd);
                LRESULT(0)
            }
            WM_IME_COMPOSITION if editable => {
                let update = yy_ui_win::ime::read_composition(hwnd, l.0 as u32);
                if let Some(result) = update.result {
                    e.document.insert_text(&result, false);
                }
                e.composition =
                    update
                        .composing
                        .filter(|(s, _)| !s.is_empty())
                        .map(|(text, cursor)| yy_ui_win::render::Composition {
                            offsets: vec![e.document.selections().primary().head],
                            text,
                            cursor,
                        });
                invalidate(hwnd);
                LRESULT(0)
            }
            WM_IME_ENDCOMPOSITION => {
                e.composition = None;
                invalidate(hwnd);
                LRESULT(0)
            }
            _ => return None,
        };
        if editable && e.document.version() != before {
            e.clear_alignment();
            e.marks.clear();
            if let Some(h) = history {
                a.history.push(h);
                a.redo.clear();
            }
            a.manual = true;
            a.saved = false;
            a.edited = true;
            request_projection(a, false);
            refresh(a);
            trim_history(a);
        }
        if matches!(msg, WM_KEYDOWN | WM_LBUTTONDOWN | WM_MOUSEMOVE) && a.editors[i].follow_caret {
            a.editors[i].reveal_caret();
        }
        if matches!(
            msg,
            WM_KEYDOWN | WM_LBUTTONDOWN | WM_MOUSEWHEEL | WM_MOUSEMOVE
        ) {
            sync_sources(a, i);
        }
        Some(value)
    })
    .flatten();
    result.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, w, l) })
}

fn smoke(a: &mut App) -> Result<()> {
    let dir = a.smoke.clone().unwrap();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    folders_view::smoke(a, &dir)?;
    for (width, height) in [(680, 420), (1920, 1080), (1000, 700), (1280, 820)] {
        unsafe {
            win(SetWindowPos(
                a.hwnd,
                None,
                0,
                0,
                width,
                height,
                SWP_NOMOVE | SWP_NOZORDER,
            ))?;
        }
        layout(a);
        let mut client = RECT::default();
        unsafe {
            win(GetClientRect(a.hwnd, &mut client))?;
        }
        for (index, pane) in a.panes.iter().enumerate() {
            let mut rect = RECT::default();
            unsafe {
                win(GetWindowRect(*pane, &mut rect))?;
                let points = std::slice::from_raw_parts_mut((&raw mut rect).cast::<POINT>(), 2);
                MapWindowPoints(None, Some(a.hwnd), points);
            }
            if rect.left < 0
                || rect.top < 0
                || rect.right > client.right
                || rect.bottom > client.bottom
                || rect.right <= rect.left
                || rect.bottom <= rect.top
            {
                return Err(format!(
                    "pane outside client after resize {width}x{height}: {rect:?}"
                ));
            }
            let mut client = RECT::default();
            unsafe {
                win(GetClientRect(*pane, &mut client))?;
            }
            // Paint each real HWND at every size, not just an offscreen image.
            // This exercises back-buffer resizing as well as child placement.
            win(a.editors[index].paint(*pane, client.right as u32, client.bottom as u32))?;
        }
    }
    if a.loaded
        .as_ref()
        .and_then(|d| d.merged.as_ref())
        .map(|m| m.unresolved())
        != Some(1)
    {
        return Err("expected one native GUI conflict".into());
    }
    resolve(a, Some(Choice::Right))?;
    let chosen = a.editors[3].text();
    if !chosen.contains("RIGHT") || chosen.contains("<<<<<<<") {
        return Err("conflict resolution did not update editor".into());
    }
    let history = a.history.pop().ok_or("missing merge undo")?;
    restore(a, history)?;
    if a.loaded
        .as_ref()
        .unwrap()
        .merged
        .as_ref()
        .unwrap()
        .unresolved()
        != 1
    {
        return Err("merge undo lost conflict state".into());
    }
    resolve(a, Some(Choice::Right))?;
    let pixels = win(a.editors[3].offscreen(700, 300))?;
    std::fs::write(dir.join("editor.bgra"), pixels).map_err(|e| e.to_string())?;
    let data = a.loaded.as_ref().unwrap();
    let refs = data.inputs.iter().collect::<Vec<_>>();
    let bytes = data.inputs[2].encode(&a.editors[3].text())?;
    yy_merge_core::files::save(
        &dir.join("result.txt"),
        &bytes,
        &refs,
        true,
        &Cancellation::default(),
    )?;
    comparison_smoke(a, &dir)?;
    win(yy_ui_win::help::show(
        HELP_MD,
        "yymerge ヘルプ",
        Some("merge"),
        "yymerge-help",
    ))?;
    four_pane_smoke(a, &dir)?;
    a.smoke_result = Some(a.editors[3].text());
    a.smoke_phase = 1;
    let end = a.editors[3].document.snapshot().len();
    a.editors[3]
        .document
        .set_selections(yy_core::SelectionSet::single(yy_core::Selection::caret(
            end,
        )));
    unsafe {
        win(PostMessageW(
            Some(a.panes[3]),
            WM_CHAR,
            WPARAM('追' as usize),
            LPARAM(0),
        ))?;
    }
    Ok(())
}
fn four_pane_smoke(a: &mut App, dir: &std::path::Path) -> Result<()> {
    a.projection_cancel.cancel();
    a.projection_due = None;
    a.projection_receiver = None;
    let data = a.loaded.as_ref().unwrap();
    let p = super::comparison::prepare_merge(
        &data.inputs[2].text,
        &data.inputs[0].text,
        &data.inputs[1].text,
        &a.editors[3].text(),
        &Cancellation::default(),
    )?;
    apply_projection(a, p);
    let rows = a.editors[0].aligned.as_ref().unwrap().rows.len();
    for (i, e) in a.editors.iter_mut().enumerate() {
        let view = e.aligned.as_mut().ok_or("four pane map missing")?;
        if view.rows.len() != rows {
            return Err("four pane row counts differ".into());
        }
        view.top = 0;
        e.follow_caret = false;
        e.scroll_x = 0.;
        let image = win(e.offscreen(700, 300))?;
        std::fs::write(
            dir.join(format!("merge-pane-{i}.bmp")),
            yy_ui_win::util::encode_bmp(700, 300, &image),
        )
        .map_err(|e| e.to_string())?;
    }
    a.editors[3].scroll_rows(i64::MAX);
    sync_sources(a, 3);
    for e in &mut a.editors {
        win(e.offscreen(700, 180))?;
    }
    let top = a.editors[3].aligned.as_ref().unwrap().top;
    if top != rows - 1
        || a.editors
            .iter()
            .any(|e| e.aligned.as_ref().unwrap().top != top)
    {
        return Err("different-height four-pane scroll desynchronized".into());
    }
    Ok(())
}
fn smoke_tick(a: &mut App) -> Result<bool> {
    if a.smoke_phase == 0 || a.smoke.is_none() {
        return Ok(false);
    }
    if a.projection_due.is_some() || a.projection_receiver.is_some() {
        return Ok(false);
    }
    if a.alignment_warning.is_some() {
        return Err(format!(
            "smoke alignment warning: {:?}",
            a.alignment_warning
        ));
    }
    if a.editors.iter().any(|e| e.aligned.is_none()) {
        return Ok(false);
    }
    if a.smoke_phase == 1 {
        if !a.editors[3].text().ends_with('追') {
            return Ok(false);
        }
        let (tx, rx) = mpsc::channel();
        tx.send(Projection {
            generation: a.generation.wrapping_sub(1),
            version: a.editors[3].document.version(),
            result: Ok(super::comparison::MergePrepared {
                panes: std::array::from_fn(|_| None),
                marks: std::array::from_fn(|_| Vec::new()),
                warning: Some("stale projection".into()),
            }),
        })
        .map_err(|e| e.to_string())?;
        a.projection_receiver = Some(rx);
        projection_tick(a);
        if a.alignment_warning.is_some() || a.editors.iter().any(|e| e.aligned.is_none()) {
            return Err("stale generation changed the display".into());
        }
        a.editors[3].document.undo();
        if a.editors[3].text() != *a.smoke_result.as_ref().unwrap() {
            return Err("background alignment lost editor undo history".into());
        }
        a.editors[3].clear_alignment();
        request_projection(a, true);
        a.smoke_phase = 2;
        return Ok(false);
    }
    if a.editors[3].text() != *a.smoke_result.as_ref().unwrap() {
        return Err("stale alignment overwrote result text".into());
    }
    for (i, input) in a.loaded.as_ref().unwrap().inputs.iter().enumerate() {
        if a.editors[i].text() != input.text.as_ref() {
            return Err("alignment changed an input document".into());
        }
    }
    std::fs::write(a.smoke.as_ref().unwrap().join("ui-smoke.txt"),"PASS: four-pane ancestor alignment, different-height synchronized scroll, debounced manual edit projection, stale generation rejected, undo preserved, original bytes preserved, resize, conflict resolution, guarded save, help\n").map_err(|e|e.to_string())?;
    a.saved = true;
    a.exit = 0;
    a.smoke_phase = 0;
    Ok(true)
}
fn comparison_smoke(a: &mut App, dir: &std::path::Path) -> Result<()> {
    let tail = (0..100)
        .map(|i| format!("unchanged {i}\n"))
        .collect::<String>();
    let left = format!("header\nvalue=日本\nsame\n{tail}");
    let right = format!("header\nvalue=日本語\ninserted\nsame\n{tail}");
    let c = Cancellation::default();
    let diff = yy_merge_core::compare(&left, &right, &c)?;
    let p = super::comparison::prepare(&left, &right, &diff, &c)?;
    let mut l = editor(&left, 96)?;
    let mut r = editor(&right, 96)?;
    l.aligned = Some(p.left);
    r.aligned = Some(p.right);
    l.marks = p.left_marks;
    r.marks = p.right_marks;
    if l.aligned.as_ref().unwrap().rows[2].source.is_some() {
        return Err("insertion has no display placeholder".into());
    }
    let old_left = std::mem::replace(&mut a.editors[0], l);
    let old_right = std::mem::replace(&mut a.editors[1], r);
    for (i, name) in [(0, "aligned-left.bmp"), (1, "aligned-right.bmp")] {
        let image = win(a.editors[i].offscreen(700, 300))?;
        std::fs::write(
            dir.join(name),
            yy_ui_win::util::encode_bmp(700, 300, &image),
        )
        .map_err(|e| e.to_string())?;
    }
    a.editors[0].scroll_rows(8);
    a.editors[0].scroll_x = 37.;
    sync_sources(a, 0);
    if a.editors[0].aligned.as_ref().unwrap().top != 8
        || a.editors[1].aligned.as_ref().unwrap().top != 8
        || a.editors[1].scroll_x != 37.
    {
        return Err("source panes failed to synchronize".into());
    }
    a.editors[1].scroll_rows(-3);
    sync_sources(a, 1);
    for i in 0..2 {
        win(a.editors[i].offscreen(700, 300))?;
    }
    if a.editors[0].aligned.as_ref().unwrap().top != 5 {
        return Err("reverse synchronization failed".into());
    }
    for (i, text) in [(0, &left), (1, &right)] {
        a.editors[i].document.select_all();
        if a.editors[i]
            .document
            .selected_text(1024 * 1024)
            .map_err(|e| e.to_string())?
            .as_deref()
            != Some(text.as_str())
        {
            return Err("display placeholders leaked into selection".into());
        }
    }
    a.editors[0] = old_left;
    a.editors[1] = old_right;
    Ok(())
}

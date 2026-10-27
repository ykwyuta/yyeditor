use std::{
    cell::RefCell,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::Instant,
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{Com::*, LibraryLoader::GetModuleHandleW},
        UI::{
            Controls::*,
            HiDpi::*,
            Input::KeyboardAndMouse::*,
            Shell::{
                Common::COMDLG_FILTERSPEC, FileOpenDialog, FileSaveDialog, IFileDialog,
                SIGDN_FILESYSPATH,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, PCWSTR, Result, w},
};
use yy_db_core::{
    Database, DbKind, Options, ResultSet, plan::Plan, session::Cancellation, transfer,
};
use yy_ui_win::{
    editor::Editor,
    font,
    grid::{Align, Cell, GridPainter, Scene},
};
#[path = "document_tabs.rs"]
mod document_tabs;
#[path = "er_view.rs"]
mod er_view;
#[path = "performance.rs"]
mod performance;
#[path = "plan_view.rs"]
mod plan_view;

const FRAME: PCWSTR = w!("YYDbManager");
const EDITOR: PCWSTR = w!("YYDbSql");
const GRID: PCWSTR = w!("YYDbGrid");
const CONNECT: u16 = 101;
const RUN: u16 = 102;
const STOP: u16 = 103;
const ESTIMATE: u16 = 104;
const ACTUAL: u16 = 105;
const TABLES: u16 = 106;
const COMMIT: u16 = 107;
const ROLLBACK: u16 = 108;
const EXPORT: u16 = 109;
const IMPORT: u16 = 110;
const OPEN_SQL: u16 = 111;
const SAVE_SQL: u16 = 112;
const OPEN_PLAN: u16 = 113;
const SAVE_PLAN: u16 = 114;
const SAVE_SVG: u16 = 115;
const NEW_SQL: u16 = 116;
const SQL_TABS: u16 = 117;
const RESULT_TABS: u16 = 118;
const ER: u16 = 119;
const SAVE_DRAW: u16 = 120;
const PROFILE_NEW: u16 = 121;
const PROFILE_SAVE: u16 = 122;
const PROFILE_DELETE: u16 = 123;
const PROFILES: u16 = 124;
const CATALOG_REFRESH: u16 = 125;
const DB2_ACTUAL_SETUP: u16 = 126;
const HELP_USAGE: u16 = 127;
const HELP_ORACLE: u16 = 128;
const HELP_DB2: u16 = 129;
const HELP_PLANS: u16 = 130;
const HELP_TROUBLE: u16 = 131;
const HELP_YAML: u16 = 132;
const HELP_PERFORMANCE: u16 = 133;
const HELP_PLAN_FILES: u16 = 134;
#[path = "connection_dialog.rs"]
mod connection_dialog;
#[path = "help.rs"]
mod help;
#[path = "transfer_dialog.rs"]
mod transfer_dialog;
thread_local! {static APP:RefCell<Option<App>>=const{RefCell::new(None)};}
enum Command {
    Connect(Options),
    Run(String),
    Explain(String, bool),
    Tables,
    Catalog,
    Columns(usize, String, String),
    Diagram,
    Import(PathBuf, FileFormat, String),
    Export(Arc<ResultSet>, PathBuf, FileFormat),
}
enum Event {
    Connected(Cancellation, DbKind),
    Results(Vec<ResultSet>, String),
    Plan(Plan),
    Diagram(yy_db_core::schema::Model),
    Message(String),
    Failed(String),
    Catalog(Vec<yy_db_core::catalog::Object>),
    Columns(usize, Vec<yy_db_core::catalog::Column>),
}
enum FileFormat {
    Delimited(transfer::Delimited),
    Parquet,
    Fixed(PathBuf, String),
}
impl FileFormat {
    fn fixed(
        path: &std::path::Path,
        charset: &str,
    ) -> yy_db_core::Result<(yy_cobol::Layout, yy_cobol::Codec)> {
        let layout = yy_cobol::parse(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)?;
        if !layout.warnings.is_empty() {
            return Err(format!(
                "コピー句の解釈に注意が必要です: {:?}",
                layout.warnings
            ));
        }
        let charset = yy_cobol::Charset::from_name(charset)
            .ok_or("文字コードはMS932またはCCSID930等を指定してください")?;
        Ok((layout, yy_cobol::Codec::new(charset)))
    }
    fn read(&self, path: &std::path::Path) -> yy_db_core::Result<ResultSet> {
        match self {
            Self::Delimited(p) => transfer::read_delimited(path, p),
            Self::Parquet => transfer::read_parquet(path),
            Self::Fixed(copybook, charset) => {
                let (layout, codec) = Self::fixed(copybook, charset)?;
                transfer::read_fixed(path, &layout, codec, &[])
            }
        }
    }
    fn write(&self, set: &ResultSet, path: &std::path::Path) -> yy_db_core::Result<()> {
        match self {
            Self::Delimited(p) => transfer::export_delimited(set, path, p),
            Self::Parquet => transfer::export_parquet(set, path),
            Self::Fixed(copybook, charset) => {
                let (layout, codec) = Self::fixed(copybook, charset)?;
                transfer::export_fixed(set, path, &layout, codec, &[])
            }
        }
    }
}
fn file_format(
    settings: &transfer_dialog::Settings,
    copybook: Option<PathBuf>,
) -> yy_db_core::Result<FileFormat> {
    Ok(match settings.format {
        3 => FileFormat::Parquet,
        4 => FileFormat::Fixed(
            copybook.ok_or("コピー句が必要です")?,
            settings.charset.clone(),
        ),
        _ => {
            let delimiter = match settings.format {
                1 => '\t',
                2 => {
                    let s = &settings.delimiter;
                    let mut chars = s.chars();
                    let c = chars.next().ok_or("区切り文字を指定してください")?;
                    if chars.next().is_some() {
                        return Err("区切り文字は1文字です".into());
                    }
                    c
                }
                _ => ',',
            };
            FileFormat::Delimited(transfer::Delimited {
                delimiter,
                ..Default::default()
            })
        }
    })
}
struct App {
    editor_hwnd: HWND,
    grid_hwnd: HWND,
    kind: HWND,
    target: HWND,
    status: HWND,
    properties: HWND,
    sql_tabs: HWND,
    result_tabs: HWND,
    transfer_settings: transfer_dialog::Settings,
    controls: Vec<(HWND, i32, i32, i32, i32)>,
    editors: Vec<Editor>,
    sql_index: usize,
    documents: document_tabs::State,
    performance: performance::State,
    grid: GridPainter,
    results: Vec<Arc<ResultSet>>,
    result_index: usize,
    top: u64,
    left: usize,
    selection: (u64, u32),
    plan: Option<Plan>,
    drawing: Option<yy_draw::Document>,
    zoom: f64,
    origin: yy_draw::Point,
    renderer: yy_draw::render::Renderer,
    tx: mpsc::Sender<Command>,
    rx: mpsc::Receiver<Event>,
    cancellation: Option<Cancellation>,
    busy: bool,
    dpi: u32,
    font: HFONT,
    profile_combo: HWND,
    profiles: Vec<super::profiles::Profile>,
    profiles_path: PathBuf,
    tree: HWND,
    tree_items: Vec<(HTREEITEM, yy_db_core::catalog::Object, bool)>,
    profile_items: Vec<(HTREEITEM, usize)>,
    connection_label: String,
    pending_label: String,
    connection_kind: DbKind,
    editor_painted: (i32, i32),
    grid_painted: (i32, i32),
    plan_view: plan_view::View,
    er: Option<yy_db_core::schema::Model>,
    er_view: er_view::View,
    smoke: Option<Smoke>,
}
fn smoke_tick(a: &mut App) -> yy_db_core::Result<bool> {
    let Some(mut smoke) = a.smoke.take() else {
        return Ok(false);
    };
    if smoke.started.elapsed().as_secs() > 20 {
        return Err(format!("UI試験タイムアウト: {}", text(a.status)));
    }
    if text(a.status).starts_with("エラー:") {
        return Err(text(a.status));
    }
    match smoke.stage {
        0 if a.cancellation.is_some() => {
            let sql = "CREATE TABLE department(id INTEGER PRIMARY KEY, name VARCHAR);\nCREATE TABLE employee(id INTEGER PRIMARY KEY, department_id INTEGER REFERENCES department(id), name VARCHAR);\nINSERT INTO department VALUES (1, '開発部');\nINSERT INTO employee VALUES (1,1,'山田'), (2,1,'佐藤');\nSELECT e.name, d.name AS department FROM employee e JOIN department d ON e.department_id=d.id;";
            a.editors[0] = Editor::new(sql, a.dpi).map_err(|e| e.to_string())?;
            a.tx.send(Command::Run(sql.into()))
                .map_err(|e| e.to_string())?;
            a.busy = true;
            smoke.stage = 1;
        }
        1 if !a.busy && !a.results.is_empty() => {
            if a.results.last().unwrap().row_count != 2 {
                return Err("UI結果の行数が不正です".into());
            }
            a.tx.send(Command::Explain(
                "SELECT e.name, d.name FROM employee e JOIN department d ON e.department_id=d.id"
                    .into(),
                false,
            ))
            .map_err(|e| e.to_string())?;
            a.busy = true;
            smoke.stage = 2;
        }
        2 if !a.busy && a.plan.is_some() => {
            std::fs::create_dir_all(&smoke.directory).map_err(|e| e.to_string())?;
            let doc = a.drawing.as_ref().ok_or("計画図がありません")?;
            std::fs::write(
                smoke.directory.join("plan.png"),
                a.renderer
                    .png(doc, &yy_draw::render::Export::document(doc), 1.)?,
            )
            .map_err(|e| e.to_string())?;
            let pixels = a.editors[0]
                .offscreen(1000, 300)
                .map_err(|e| e.to_string())?;
            std::fs::write(
                smoke.directory.join("sql-editor.bmp"),
                yy_ui_win::util::encode_bmp(1000, 300, &pixels),
            )
            .map_err(|e| e.to_string())?;
            a.tx.send(Command::Diagram).map_err(|e| e.to_string())?;
            a.busy = true;
            smoke.stage = 3;
        }
        3 if !a.busy && a.plan.is_none() && a.drawing.is_some() => {
            let doc = a.drawing.as_ref().unwrap();
            if a.er.as_ref().map(|m| m.entities.len()) != Some(2) || doc.connectors.len() != 1 {
                return Err("UI経由のER図が不正です".into());
            }
            std::fs::write(
                smoke.directory.join("er.png"),
                a.renderer
                    .png(doc, &yy_draw::render::Export::document(doc), 1.)?,
            )
            .map_err(|e| e.to_string())?;
            a.tx.send(Command::Catalog).map_err(|e| e.to_string())?;
            a.busy = true;
            smoke.stage = 4;
        }
        4 if !a.busy && !a.tree_items.is_empty() => {
            let item = a
                .tree_items
                .iter()
                .find(|e| e.1.name == "employee")
                .ok_or("ツリーにテーブルがありません")?;
            a.tx.send(Command::Columns(
                item.0.0 as usize,
                item.1.schema.clone(),
                item.1.name.clone(),
            ))
            .map_err(|e| e.to_string())?;
            a.busy = true;
            smoke.stage = 5;
        }
        5 if !a.busy => {
            if !a.tree_items.iter().any(|e| e.1.name == "employee" && e.2) {
                return Err("ツリーの列取得に失敗しました".into());
            }
            performance::smoke(a, &smoke.directory)?;
            if let Some(path) = std::env::var_os("YYDBMANAGER_PLAN_FIXTURE") {
                document_tabs::select(a, false);
                plan_view::present(a, read_plan(&PathBuf::from(path))?)?;
                return Ok(true);
            }
            a.zoom = 1.25 * a.dpi as f64 / 96.;
            a.origin = yy_draw::Point::new(32., 24.);
            a.tx.send(Command::Run(
                "SELECT 11 AS first_result; SELECT 22 AS second_result;".into(),
            ))
            .map_err(|e| e.to_string())?;
            a.busy = true;
            smoke.stage = 6;
        }
        6 if !a.busy => {
            if !a.documents.er_active
                || a.er.as_ref().map(|m| m.entities.len()) != Some(2)
                || a.drawing.is_none()
                || a.zoom != 1.25 * a.dpi as f64 / 96.
                || a.origin != yy_draw::Point::new(32., 24.)
                || a.results.len() != 2
            {
                return Err("バックグラウンドのSQL完了がER図の作図状態を変更しました".into());
            }
            return Ok(true);
        }
        _ => {}
    }
    a.smoke = Some(smoke);
    Ok(false)
}
fn with_app<T>(f: impl FnOnce(&mut App) -> T) -> Option<T> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

fn resize_smoke(hwnd: HWND, directory: &std::path::Path) -> yy_db_core::Result<()> {
    let (controls, editor, grid, dpi) =
        with_app(|a| (a.controls.clone(), a.editor_hwnd, a.grid_hwnd, a.dpi))
            .ok_or("画面がありません")?;
    unsafe {
        for (width, height) in [(980, 650), (1600, 950), (1280, 800)] {
            let mut r = RECT {
                right: width * dpi as i32 / 96,
                bottom: height * dpi as i32 / 96,
                ..Default::default()
            };
            AdjustWindowRectExForDpi(
                &mut r,
                WS_OVERLAPPEDWINDOW,
                true,
                WINDOW_EX_STYLE::default(),
                dpi,
            )
            .map_err(|e| e.to_string())?;
            SetWindowPos(
                hwnd,
                None,
                0,
                0,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .map_err(|e| e.to_string())?;
            let mut client = RECT::default();
            GetClientRect(hwnd, &mut client).map_err(|e| e.to_string())?;
            for &(child, _, _, _, _) in &controls {
                let mut rect = RECT::default();
                GetWindowRect(child, &mut rect).map_err(|e| e.to_string())?;
                let mut point = POINT {
                    x: rect.left,
                    y: rect.top,
                };
                let _ = ScreenToClient(hwnd, &mut point);
                if point.x < 0
                    || point.y < 0
                    || point.x + rect.right - rect.left > client.right
                    || point.y + rect.bottom - rect.top > client.bottom
                {
                    return Err(format!(
                        "サイズ変更で部品がはみ出しました: {width}x{height}, id={}",
                        GetDlgCtrlID(child)
                    ));
                }
            }
            for child in [editor, grid] {
                invalidate(child);
                send_message(child, WM_PAINT, WPARAM(0), LPARAM(0));
            }
            let actual =
                with_app(|a| (a.editor_painted, a.grid_painted)).ok_or("描画状態がありません")?;
            for (child, size) in [(editor, actual.0), (grid, actual.1)] {
                let mut rect = RECT::default();
                GetClientRect(child, &mut rect).map_err(|e| e.to_string())?;
                if size != (rect.right, rect.bottom) {
                    return Err("サイズ変更後の描画領域が一致しません".into());
                }
            }
        }
    }
    plan_view::smoke(directory)?;
    er_view::smoke(directory)?;
    document_tabs::smoke(directory)?;
    let (tabs, count) = with_app(|a| (a.result_tabs, a.results.len())).ok_or("結果がありません")?;
    unsafe {
        if count < 2
            || send_message(tabs, TCM_GETITEMCOUNT, WPARAM(0), LPARAM(0)).0 as usize != count
        {
            return Err("複数の結果セットのタブ数が一致しません".into());
        }
        for index in [count - 1, 0] {
            with_app(|a| {
                a.top = 10;
                a.left = 2;
                a.selection = (10, 2);
            });
            send_message(tabs, TCM_SETCURSEL, WPARAM(index), LPARAM(0));
            let notification = NMHDR {
                hwndFrom: tabs,
                idFrom: RESULT_TABS as usize,
                code: TCN_SELCHANGE,
            };
            send_message(
                hwnd,
                WM_NOTIFY,
                WPARAM(RESULT_TABS as usize),
                LPARAM(&notification as *const _ as isize),
            );
            let valid = with_app(|a| {
                a.result_index == index
                    && a.top == 0
                    && a.left == 0
                    && a.selection == (0, 0)
                    && a.drawing.is_none()
                    && !a.documents.er_active
                    && a.results.get(a.result_index).is_some()
            });
            if valid != Some(true) {
                return Err("結果タブの切り替えが表示・出力対象に反映されません".into());
            }
        }
    }
    std::fs::write(directory.join("ui-smoke.txt"),"Native UI: connect, SQL tabs, multiple result tabs and selection, plan, ER, object tree, lazy columns and 3 resize/repaint sizes OK\n").map_err(|e|e.to_string())
}
unsafe fn send_message(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { SendMessageW(hwnd, msg, Some(w), Some(l)) }
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h).max(0) as usize;
        let mut b = vec![0; n + 1];
        let n = GetWindowTextW(h, &mut b) as usize;
        String::from_utf16_lossy(&b[..n])
    }
}
fn set_text(h: HWND, s: &str) {
    unsafe {
        let _ = SetWindowTextW(h, &HSTRING::from(s));
    }
}
fn invalidate(h: HWND) {
    unsafe {
        let _ = InvalidateRect(Some(h), None, false);
    }
}
fn message(s: &str) {
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(s),
            w!("yydbmanager"),
            MB_OK | MB_ICONERROR,
        );
    }
}
fn worker(rx: mpsc::Receiver<Command>, tx: mpsc::Sender<Event>) {
    let mut db: Option<Database> = None;
    let mut flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    for cmd in rx {
        let result = (|| -> yy_db_core::Result<Event> {
            match cmd {
                Command::Connect(o) => {
                    let c = Database::open(&o)?;
                    let cancel = c.cancellation();
                    flag = cancel.flag();
                    db = Some(c);
                    Ok(Event::Connected(cancel, o.kind))
                }
                Command::Export(set, path, format) => {
                    format.write(&set, &path)?;
                    Ok(Event::Message(format!(
                        "{}行を出力しました: {}",
                        set.row_count,
                        path.display()
                    )))
                }
                cmd => {
                    let db = db.as_mut().ok_or("先にデータベースへ接続してください")?;
                    flag.store(false, std::sync::atomic::Ordering::Relaxed);
                    let now = Instant::now();
                    match cmd {
                        Command::Run(sql) => {
                            let sets = db.execute(&sql, &flag)?;
                            let summary = format!(
                                "{}結果セット / {:.3}秒",
                                sets.len(),
                                now.elapsed().as_secs_f64()
                            );
                            Ok(Event::Results(sets, summary))
                        }
                        Command::Explain(sql, actual) => {
                            Ok(Event::Plan(db.explain(&sql, actual, &flag)?))
                        }
                        Command::Tables => {
                            Ok(Event::Results(db.tables()?, "テーブル・ビュー一覧".into()))
                        }
                        Command::Catalog => Ok(Event::Catalog(db.catalog()?)),
                        Command::Columns(item, schema, table) => {
                            Ok(Event::Columns(item, db.object_columns(&schema, &table)?))
                        }
                        Command::Diagram => Ok(Event::Diagram(db.er_model()?)),
                        Command::Import(path, format, table) => {
                            let set = format.read(&path)?;
                            Ok(Event::Message(format!(
                                "{}行を {} へ投入しました",
                                db.import_into(&set, &table, true)?,
                                table
                            )))
                        }
                        _ => unreachable!(),
                    }
                }
            }
        })();
        let _ = tx.send(result.unwrap_or_else(Event::Failed));
    }
}

pub fn run(initial: Option<PathBuf>) -> Result<()> {
    run_inner(initial, None, false)
}
pub fn run_plan(path: PathBuf) -> Result<()> {
    run_inner(Some(path), None, true)
}
pub fn smoke(directory: PathBuf) -> Result<()> {
    std::fs::create_dir_all(&directory).map_err(|e| {
        windows::core::Error::new(windows::Win32::Foundation::E_FAIL, e.to_string())
    })?;
    let _ = std::fs::remove_file(directory.join("ui-smoke.txt"));
    run_inner(None, Some(directory), false)
}
struct Smoke {
    directory: PathBuf,
    stage: u8,
    started: Instant,
}
fn run_inner(initial: Option<PathBuf>, smoke: Option<PathBuf>, force_plan: bool) -> Result<()> {
    let initial_er = initial
        .as_ref()
        .filter(|p| {
            !force_plan && {
                p.extension().is_some_and(|e| {
                    ["yyer", "yaml", "yml"]
                        .iter()
                        .any(|ext| e.eq_ignore_ascii_case(ext))
                })
            }
        })
        .map(|p| yy_db_core::schema::ErDocument::load(p))
        .transpose()
        .map_err(|e| windows::core::Error::new(E_FAIL, e))?;
    let plan_path = initial
        .as_ref()
        .filter(|p| force_plan || yy_db_core::plan_file::is_plan_path(p))
        .cloned();
    let initial_plan = plan_path
        .as_ref()
        .map(|path| read_plan(path))
        .transpose()
        .map_err(|e| windows::core::Error::new(E_FAIL, e))?;
    let initial = if initial_plan.is_some() || initial_er.is_some() {
        None
    } else {
        initial
    };
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        font::register_gdi();
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES
                | ICC_TREEVIEW_CLASSES
                | ICC_TAB_CLASSES
                | ICC_LISTVIEW_CLASSES,
        });
        let instance = GetModuleHandleW(None)?;
        let app_icon = LoadIconW(
            Some(instance.into()),
            PCWSTR(std::ptr::without_provenance(1)),
        )
        .unwrap_or_default();
        for (name, procedure) in [
            (
                FRAME,
                frame_proc as unsafe extern "system" fn(_, _, _, _) -> _,
            ),
            (EDITOR, editor_proc),
            (GRID, grid_proc),
        ] {
            let class = WNDCLASSW {
                style: CS_HREDRAW
                    | CS_VREDRAW
                    | if name == GRID {
                        CS_DBLCLKS
                    } else {
                        WNDCLASS_STYLES::default()
                    },
                hInstance: instance.into(),
                lpszClassName: name,
                hIcon: app_icon,
                lpfnWndProc: Some(procedure),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hbrBackground: GetSysColorBrush(if name == FRAME {
                    COLOR_BTNFACE
                } else {
                    COLOR_WINDOW
                }),
                ..Default::default()
            };
            RegisterClassW(&class);
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            FRAME,
            w!("yydbmanager"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1360,
            900,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        SetMenu(hwnd, Some(build_menu()?))?;
        let dpi = GetDpiForWindow(hwnd);
        let font_handle = control_font(dpi);
        let mut controls = vec![];
        let mut control = |class: PCWSTR,
                           label: &str,
                           id: u16,
                           style: u32,
                           x: i32,
                           y: i32,
                           width: i32,
                           height: i32|
         -> Result<HWND> {
            let h = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                &HSTRING::from(label),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
                0,
                0,
                1,
                1,
                Some(hwnd),
                Some(HMENU(id as usize as *mut _)),
                Some(instance.into()),
                None,
            )?;
            send_message(h, WM_SETFONT, WPARAM(font_handle.0 as usize), LPARAM(0));
            controls.push((h, x, y, width, height));
            Ok(h)
        };
        let profile_combo = control(
            w!("COMBOBOX"),
            "",
            PROFILES,
            CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0,
            8,
            8,
            220,
            300,
        )?;
        let tree = control(
            WC_TREEVIEWW,
            "",
            210,
            WS_BORDER.0
                | WS_TABSTOP.0
                | TVS_HASLINES
                | TVS_HASBUTTONS
                | TVS_LINESATROOT
                | TVS_SHOWSELALWAYS,
            8,
            86,
            240,
            500,
        )?;
        let kind = control(
            w!("COMBOBOX"),
            "",
            201,
            CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0,
            8,
            8,
            155,
            300,
        )?;
        for k in DbKind::ALL {
            send_message(
                kind,
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(wide(k.name()).as_ptr() as isize),
            );
        }
        send_message(kind, CB_SETCURSEL, WPARAM(0), LPARAM(0));
        let target = control(
            w!("EDIT"),
            initial
                .as_ref()
                .and_then(|p| p.to_str())
                .unwrap_or(":memory:"),
            202,
            WS_BORDER.0 | ES_AUTOHSCROLL as u32 | ES_PASSWORD as u32,
            172,
            8,
            700,
            28,
        )?;
        for (i, (id, label, width)) in [
            (CONNECT, "接続", 92),
            (RUN, "実行 F5", 110),
            (STOP, "停止", 92),
            (ESTIMATE, "推定計画", 120),
        ]
        .into_iter()
        .enumerate()
        {
            let x = [8, 108, 226, 326, 454][i];
            control(
                w!("BUTTON"),
                label,
                id,
                BS_OWNERDRAW as u32,
                x,
                44,
                width,
                30,
            )?;
        }
        let sql_tabs = control(
            WC_TABCONTROLW,
            "",
            SQL_TABS,
            TCS_FOCUSNEVER,
            260,
            82,
            700,
            28,
        )?;
        add_sql_tab(sql_tabs, 0, "SQL 1");
        let editor_hwnd = control(
            EDITOR,
            "",
            203,
            WS_BORDER.0 | WS_TABSTOP.0,
            0,
            112,
            100,
            100,
        )?;
        let result_tabs = control(
            WC_TABCONTROLW,
            "",
            RESULT_TABS,
            TCS_FOCUSNEVER,
            8,
            420,
            230,
            220,
        )?;

        let grid_hwnd = control(GRID, "", 204, WS_BORDER.0 | WS_TABSTOP.0, 0, 455, 100, 100)?;
        let properties = control(
            WC_LISTVIEWW,
            "",
            209,
            WS_BORDER.0 | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS,
            0,
            455,
            100,
            100,
        )?;
        let status = control(
            w!("STATIC"),
            "右側の「新規接続…」から接続先を登録します。保存した接続名をダブルクリックして接続してください。",
            205,
            0,
            8,
            800,
            1200,
            45,
        )?;
        let profiles_path = smoke
            .as_ref()
            .map(|dir| dir.join("connections.dat"))
            .or_else(|| std::env::var_os("YYDBMANAGER_CONNECTIONS_FILE").map(PathBuf::from))
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap_or_else(|| ".".into()))
                    .join("yydbmanager/connections.dat")
            });
        let profiles = match super::profiles::load(&profiles_path) {
            Ok(v) => v,
            Err(e) => {
                set_text(status, &e);
                vec![]
            }
        };
        control(
            w!("STATIC"),
            "オブジェクト エクスプローラー",
            211,
            0,
            8,
            86,
            240,
            22,
        )?;
        for h in [profile_combo, kind, target] {
            let _ = ShowWindow(h, SW_HIDE);
        }
        controls.retain(|(h, _, _, _, _)| ![profile_combo, kind, target].contains(h));
        for (id, label, x) in [
            (PROFILE_NEW, "新規接続…", 0),
            (PROFILE_SAVE, "接続先を編集…", 112),
        ] {
            let h = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                &HSTRING::from(label),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0,
                0,
                1,
                1,
                Some(hwnd),
                Some(HMENU(id as usize as *mut _)),
                Some(instance.into()),
                None,
            )?;
            send_message(h, WM_SETFONT, WPARAM(font_handle.0 as usize), LPARAM(0));
            controls.push((h, x, 0, 104, 28));
        }
        let (tx_cmd, rx_cmd) = mpsc::channel();
        let (tx_event, rx_event) = mpsc::channel();
        std::thread::spawn(move || worker(rx_cmd, tx_event));
        let app = App {
            editor_hwnd,
            grid_hwnd,
            kind,
            target,
            status,
            properties,
            sql_tabs,
            result_tabs,
            transfer_settings: transfer_dialog::Settings::default(),
            controls,
            editors: vec![Editor::new(
                "-- SQLを入力して F5 で実行します\r\nSELECT 'yydbmanager' AS application, 9223372036854775807 AS exact_integer;",
                dpi,
            )?],
            sql_index: 0,
            documents: document_tabs::State::default(),
            grid: GridPainter::new(font::BUNDLED_FAMILY, 11., dpi)?,
            results: vec![],
            result_index: 0,
            top: 0,
            left: 0,
            selection: (0, 0),
            plan: None,
            drawing: None,
            er: None,
            er_view: er_view::View::default(),
            zoom: 1.,
            origin: yy_draw::Point::new(0., 0.),
            renderer: yy_draw::render::Renderer::new(Some(font::BUNDLED_FONT)),
            tx: tx_cmd,
            rx: rx_event,
            cancellation: None,
            busy: false,
            dpi,
            font: font_handle,
            profile_combo,
            profiles,
            profiles_path,
            tree,
            tree_items: vec![],
            profile_items: vec![],
            connection_label: "未接続".into(),
            pending_label: "SQLite :memory:".into(),
            connection_kind: DbKind::Sqlite,
            performance: performance::State::default(),
            editor_painted: (0, 0),
            grid_painted: (0, 0),
            plan_view: plan_view::View::new(grid_hwnd, properties, dpi)?,
            smoke: smoke.map(|directory| Smoke {
                directory,
                stage: 0,
                started: Instant::now(),
            }),
        };
        APP.with(|a| *a.borrow_mut() = Some(app));
        with_app(refresh_profiles);
        if let Some(model) = initial_er {
            with_app(|a| er_view::present_document(a, model));
        }
        if let Some(plan) = initial_plan {
            with_app(|a| {
                if let Some(sql) = plan
                    .nodes
                    .first()
                    .and_then(|n| n.properties.get("StatementText"))
                    && let Ok(editor) = Editor::new(sql, a.dpi)
                {
                    a.editors[0] = editor;
                }
                if let Err(e) = plan_view::present(a, plan) {
                    set_text(a.status, &e);
                }
            });
        }
        with_app(|a| populate_tree(a, vec![]));
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut monitor_info).as_bool() {
            let area = monitor_info.rcWork;
            let width = (1360 * dpi as i32 / 96).min(area.right - area.left);
            let height = (900 * dpi as i32 / 96).min(area.bottom - area.top);
            let _ = SetWindowPos(
                hwnd,
                None,
                area.left + (area.right - area.left - width) / 2,
                area.top + (area.bottom - area.top - height) / 2,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        layout(hwnd);
        SetTimer(Some(hwnd), 1, 50, None);
        let visible = with_app(|a| a.smoke.is_none()).unwrap_or(true);
        let _ = ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
        if !visible {
            with_app(|a| {
                a.tx.send(Command::Connect(Options::memory(DbKind::Sqlite)))
                    .unwrap();
                a.busy = true;
            });
        }
        let _ = SetFocus(Some(editor_hwnd));
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if yy_ui_win::help::translate(&msg) {
                continue;
            }
            if msg.message == WM_KEYDOWN && msg.wParam.0 == VK_F1.0 as usize {
                command(HELP_USAGE);
                continue;
            }
            if msg.message == WM_KEYDOWN && msg.wParam.0 == VK_F5.0 as usize {
                command(RUN);
                continue;
            }
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        yy_ui_win::help::close();
        APP.with(|a| {
            if let Some(app) = a.borrow_mut().take() {
                let _ = DeleteObject(app.font.into());
            }
        });
        CoUninitialize();
        Ok(())
    }
}
fn control_font(dpi: u32) -> HFONT {
    unsafe {
        CreateFontW(
            -(16 * dpi as i32 / 96),
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

fn build_menu() -> Result<HMENU> {
    unsafe {
        let menu = CreateMenu()?;
        for (title, entries) in [
            (
                "ファイル(&F)",
                vec![
                    (NEW_SQL, "SQLを新規作成"),
                    (OPEN_SQL, "SQLを開く…"),
                    (SAVE_SQL, "SQLを保存…"),
                    (0, ""),
                    (er_view::NEW, "ERモデルを新規作成…"),
                    (er_view::OPEN, "ERモデルを開く…"),
                    (er_view::SAVE, "ERモデルを保存…"),
                    (er_view::MARKDOWN, "Markdown設計書をエクスポート…"),
                    (0, ""),
                    (IMPORT, "データをインポート…"),
                    (EXPORT, "結果をエクスポート…"),
                ],
            ),
            (
                "接続(&C)",
                vec![
                    (CONNECT, "接続"),
                    (PROFILE_NEW, "接続先を新規作成…"),
                    (PROFILE_SAVE, "選択した接続先を編集…"),
                    (PROFILE_DELETE, "選択した接続設定を削除…"),
                    (0, ""),
                    (TABLES, "テーブル一覧を結果に表示"),
                ],
            ),
            (
                "クエリ(&Q)",
                vec![
                    (RUN, "実行\tF5"),
                    (STOP, "実行を停止"),
                    (0, ""),
                    (COMMIT, "COMMIT"),
                    (ROLLBACK, "ROLLBACK"),
                ],
            ),
            (
                "実行計画(&P)",
                vec![
                    (ESTIMATE, "推定実行計画を表示"),
                    (ACTUAL, "実行して実測計画を取得"),
                    (DB2_ACTUAL_SETUP, "Db2実測収集の準備SQL…"),
                    (0, ""),
                    (OPEN_PLAN, "実行計画を開く…"),
                    (SAVE_PLAN, "実行計画を保存…"),
                ],
            ),
            (
                "表示(&V)",
                vec![
                    (ER, "ER図を表示"),
                    (SAVE_SVG, "図をSVGで保存…"),
                    (SAVE_DRAW, "図をyydrawで保存…"),
                ],
            ),
            (
                "ヘルプ(&H)",
                vec![
                    (HELP_USAGE, "使い方\tF1"),
                    (HELP_PLANS, "実行計画の取得と読み方…"),
                    (HELP_PLAN_FILES, "実行計画ファイルの形式と取得方法…"),
                    (HELP_PERFORMANCE, "DB別の性能分析ビュー…"),
                    (HELP_ORACLE, "Oracleの権限と実測取得の準備…"),
                    (HELP_DB2, "Db2の権限と実測収集の準備…"),
                    (HELP_TROUBLE, "実測取得のトラブルシューティング…"),
                    (HELP_YAML, "ERモデルYAMLの仕様と読み込み…"),
                ],
            ),
        ] {
            let popup = CreatePopupMenu()?;
            for (id, label) in entries {
                if id == ER {
                    AppendMenuW(popup, MF_POPUP, er_view::menu()?.0 as usize, w!("ER図"))?;
                    continue;
                }
                AppendMenuW(
                    popup,
                    if id == 0 { MF_SEPARATOR } else { MF_STRING },
                    id as usize,
                    &HSTRING::from(label),
                )?;
            }
            AppendMenuW(menu, MF_POPUP, popup.0 as usize, &HSTRING::from(title))?;
        }
        Ok(menu)
    }
}
fn refresh_profiles(a: &mut App) {
    unsafe {
        send_message(a.profile_combo, CB_RESETCONTENT, WPARAM(0), LPARAM(0));
        send_message(
            a.profile_combo,
            CB_SETCUEBANNER,
            WPARAM(0),
            LPARAM(w!("保存した接続設定を選択").as_ptr() as isize),
        );
        for p in &a.profiles {
            send_message(
                a.profile_combo,
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(wide(&format!("{} ({})", p.name, p.kind.name())).as_ptr() as isize),
            );
        }
    }
}
fn tree_context(hwnd: HWND, mut point: POINT) {
    let Some(tree) = with_app(|a| a.tree) else {
        return;
    };
    unsafe {
        if point.x == -1 && point.y == -1 {
            let mut r = RECT::default();
            let _ = GetClientRect(tree, &mut r);
            point = POINT {
                x: r.right / 2,
                y: 24,
            };
            let _ = ClientToScreen(tree, &mut point);
        } else {
            let mut local = point;
            let _ = ScreenToClient(tree, &mut local);
            let mut hit = TVHITTESTINFO {
                pt: local,
                ..Default::default()
            };
            send_message(
                tree,
                TVM_HITTEST,
                WPARAM(0),
                LPARAM(&mut hit as *mut _ as isize),
            );
            if hit.hItem.0 != 0 {
                send_message(
                    tree,
                    TVM_SELECTITEM,
                    WPARAM(TVGN_CARET as usize),
                    LPARAM(hit.hItem.0),
                );
            }
        }
        let Some((entries, diagnostic_kind)) = with_app(|a| {
            let item = HTREEITEM(
                send_message(
                    tree,
                    TVM_GETNEXTITEM,
                    WPARAM(TVGN_CARET as usize),
                    LPARAM(0),
                )
                .0,
            );
            let profile = a
                .profile_items
                .iter()
                .find(|(h, _)| *h == item)
                .and_then(|(_, i)| a.profiles.get(*i));
            let mut entries = vec![(PROFILE_NEW, "接続先を新規作成…")];
            if profile.is_some() {
                entries.extend([
                    (CONNECT, "接続"),
                    (PROFILE_SAVE, "接続先を編集…"),
                    (PROFILE_DELETE, "接続先を削除…"),
                ]);
            }
            if a.cancellation.is_some() && profile.is_none_or(|p| p.name == a.connection_label) {
                entries.push((CATALOG_REFRESH, "オブジェクト一覧を更新"));
            }
            (
                entries,
                tree_profile(a, item)
                    .and_then(|i| a.profiles.get(i))
                    .map(|p| p.kind),
            )
        }) else {
            return;
        };
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        for (id, label) in entries {
            let _ = AppendMenuW(menu, MF_STRING, id as usize, &HSTRING::from(label));
        }
        if let Some(kind) = diagnostic_kind
            && let Err(error) = performance::append_menu(menu, kind)
        {
            let _ = DestroyMenu(menu);
            message(&format!("性能分析メニューを作成できません: {error}"));
            return;
        }
        let id = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            Some(0),
            hwnd,
            None,
        )
        .0 as u16;
        let _ = DestroyMenu(menu);
        if id != 0 {
            command(id);
        }
    }
}
fn tree_profile(a: &App, mut item: HTREEITEM) -> Option<usize> {
    for _ in 0..32 {
        if let Some((_, index)) = a.profile_items.iter().find(|(h, _)| *h == item) {
            return (*index < a.profiles.len()).then_some(*index);
        }
        if item.0 == 0 {
            break;
        }
        item = HTREEITEM(unsafe {
            send_message(
                a.tree,
                TVM_GETNEXTITEM,
                WPARAM(TVGN_PARENT as usize),
                LPARAM(item.0),
            )
            .0
        });
    }
    None
}
fn select_profile(a: &mut App, index: usize) {
    if let Some(profile) = a.profiles.get(index) {
        unsafe {
            send_message(a.profile_combo, CB_SETCURSEL, WPARAM(index), LPARAM(0));
            send_message(
                a.kind,
                CB_SETCURSEL,
                WPARAM(
                    DbKind::ALL
                        .iter()
                        .position(|k| *k == profile.kind)
                        .unwrap_or(0),
                ),
                LPARAM(0),
            );
        }
        set_text(a.target, &profile.target);
        set_text(
            a.status,
            &format!(
                "接続先「{}」を選択しました。ダブルクリックまたは「接続」で接続します。",
                profile.name
            ),
        );
    }
}
fn add_sql_tab(tabs: HWND, index: usize, label: &str) {
    let mut label = wide(label);
    let item = TCITEMW {
        mask: TCIF_TEXT,
        pszText: windows::core::PWSTR(label.as_mut_ptr()),
        ..Default::default()
    };
    unsafe {
        send_message(
            tabs,
            TCM_INSERTITEMW,
            WPARAM(index),
            LPARAM(&item as *const _ as isize),
        );
        send_message(tabs, TCM_SETCURSEL, WPARAM(index), LPARAM(0));
    }
}
fn tree_insert(tree: HWND, parent: HTREEITEM, label: &str, children: bool) -> HTREEITEM {
    unsafe {
        let mut text = wide(label);
        let item = TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 {
                item: TVITEMW {
                    mask: TVIF_TEXT | TVIF_CHILDREN,
                    pszText: windows::core::PWSTR(text.as_mut_ptr()),
                    cChildren: TVITEMEXW_CHILDREN(i32::from(children)),
                    ..Default::default()
                },
            },
        };
        HTREEITEM(
            send_message(
                tree,
                TVM_INSERTITEMW,
                WPARAM(0),
                LPARAM(&item as *const _ as isize),
            )
            .0,
        )
    }
}
fn populate_tree(a: &mut App, objects: Vec<yy_db_core::catalog::Object>) {
    unsafe {
        send_message(a.tree, TVM_DELETEITEM, WPARAM(0), LPARAM(TVI_ROOT.0));
    }
    a.tree_items.clear();
    a.profile_items.clear();
    let connections = tree_insert(a.tree, TVI_ROOT, "接続先", true);
    let create = tree_insert(a.tree, connections, "新しい接続先…", false);
    a.profile_items.push((create, usize::MAX));
    let mut active = None;
    let selected =
        unsafe { send_message(a.profile_combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as usize };
    for (index, profile) in a.profiles.iter().enumerate() {
        let connected = a.cancellation.is_some() && a.connection_label == profile.name;
        let item = tree_insert(
            a.tree,
            connections,
            &format!(
                "{} ({}){}",
                profile.name,
                profile.kind.name(),
                if connected { " [接続中]" } else { "" }
            ),
            connected && !objects.is_empty(),
        );
        a.profile_items.push((item, index));
        if connected {
            active = Some(item);
        }
        if index == selected {
            unsafe {
                send_message(
                    a.tree,
                    TVM_SELECTITEM,
                    WPARAM(TVGN_CARET as usize),
                    LPARAM(item.0),
                );
            }
        }
    }
    unsafe {
        send_message(
            a.tree,
            TVM_EXPAND,
            WPARAM(TVE_EXPAND.0 as usize),
            LPARAM(connections.0),
        );
    }
    let root = active.unwrap_or_else(|| {
        if a.cancellation.is_some() {
            tree_insert(
                a.tree,
                connections,
                &format!("接続中: {}", a.connection_label),
                !objects.is_empty(),
            )
        } else {
            connections
        }
    });
    let mut schemas = std::collections::BTreeMap::new();
    let mut groups = std::collections::BTreeMap::new();
    for object in objects {
        let schema = *schemas
            .entry(object.schema.clone())
            .or_insert_with(|| tree_insert(a.tree, root, &object.schema, true));
        let category = match object.kind.as_str() {
            "TABLE" => "テーブル",
            "VIEW" => "ビュー",
            "INDEX" => "インデックス",
            "PROCEDURE" => "ストアド プロシージャ",
            "FUNCTION" => "関数",
            "SEQUENCE" => "シーケンス",
            "TRIGGER" => "トリガー",
            "PACKAGE" => "パッケージ",
            _ => &object.kind,
        };
        let group = *groups
            .entry((object.schema.clone(), object.kind.clone()))
            .or_insert_with(|| tree_insert(a.tree, schema, category, true));
        let expandable = matches!(object.kind.as_str(), "TABLE" | "VIEW");
        let item = tree_insert(a.tree, group, &object.name, expandable);
        a.tree_items.push((item, object, !expandable));
    }
    unsafe {
        send_message(
            a.tree,
            TVM_EXPAND,
            WPARAM(TVE_EXPAND.0 as usize),
            LPARAM(root.0),
        );
    }
    if a.cancellation.is_none() {
        set_text(
            a.status,
            "右側の「新規接続…」から接続先を登録します。保存した接続名をダブルクリックして接続してください。",
        );
        return;
    }
    set_text(
        a.status,
        &format!(
            "オブジェクト {}件。テーブル／ビューを展開すると列を取得します。ダブルクリックでSELECTを編集します。",
            a.tree_items.len()
        ),
    );
}

#[derive(Default)]
struct NameDialog {
    done: std::cell::Cell<bool>,
    value: RefCell<Option<String>>,
    edit: std::cell::Cell<HWND>,
}
unsafe extern "system" fn name_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut NameDialog;
        if !state.is_null() {
            if msg == WM_COMMAND && matches!((w.0 & 0xffff) as i32, 1 | 2) {
                if (w.0 & 0xffff) == 1 {
                    let edit = (*state).edit.get();
                    let value = text(edit);
                    (*state).value.replace(Some(value));
                }
                (*state).done.set(true);
                let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
            if msg == WM_CLOSE {
                (*state).done.set(true);
                let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
        }
        DefWindowProcW(hwnd, msg, w, l)
    }
}

fn input_text(parent: HWND, title: &str, initial: &str, font: HFONT, dpi: u32) -> Option<String> {
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: w!("YYDbName"),
            lpfnWndProc: Some(name_proc),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
            ..Default::default()
        };
        RegisterClassW(&class);
        let px = |v: i32| v * dpi as i32 / 96;
        let mut rect = RECT::default();
        let _ = GetWindowRect(parent, &mut rect);
        let dialog = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            w!("YYDbName"),
            &HSTRING::from(title),
            WS_CAPTION | WS_SYSMENU | WS_POPUP,
            rect.left + px(80),
            rect.top + px(80),
            px(470),
            px(175),
            Some(parent),
            None,
            Some(instance.into()),
            None,
        )
        .ok()?;
        let mut state = NameDialog::default();
        SetWindowLongPtrW(dialog, GWLP_USERDATA, &mut state as *mut _ as isize);
        let edit = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("EDIT"),
            &HSTRING::from(initial),
            WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            px(16),
            px(18),
            px(420),
            px(28),
            Some(dialog),
            None,
            Some(instance.into()),
            None,
        )
        .ok()?;
        state.edit.set(edit);
        send_message(edit, EM_SETLIMITTEXT, WPARAM(128), LPARAM(0));
        send_message(edit, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
        for (id, label, x) in [
            (
                1,
                if title == "接続設定を保存" {
                    "保存"
                } else if title.contains("検索") {
                    "検索"
                } else {
                    "適用"
                },
                230,
            ),
            (2, "キャンセル", 335),
        ] {
            let b = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                &HSTRING::from(label),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | WINDOW_STYLE(if id == 1 { BS_DEFPUSHBUTTON as u32 } else { 0 }),
                px(x),
                px(70),
                px(100),
                px(30),
                Some(dialog),
                Some(HMENU(id as *mut _)),
                Some(instance.into()),
                None,
            )
            .ok()?;
            send_message(b, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
        }
        let _ = EnableWindow(parent, false);
        let _ = ShowWindow(dialog, SW_SHOW);
        let _ = SetFocus(Some(edit));
        let mut msg = MSG::default();
        while !state.done.get() {
            let code = GetMessageW(&mut msg, None, 0, 0).0;
            if code <= 0 {
                if code == 0 {
                    PostQuitMessage(msg.wParam.0 as i32);
                }
                break;
            }
            if !IsDialogMessageW(dialog, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let result = state.value.take();
        let _ = EnableWindow(parent, true);
        let _ = DestroyWindow(dialog);
        let _ = SetForegroundWindow(parent);
        result
    }
}

fn draw_tool(item: &DRAWITEMSTRUCT) {
    unsafe {
        let dc = item.hDC;
        let r = item.rcItem;
        FillRect(
            dc,
            &r,
            GetSysColorBrush(if item.itemState.0 & ODS_SELECTED.0 != 0 {
                COLOR_3DSHADOW
            } else {
                COLOR_BTNFACE
            }),
        );
        let mut edge = r;
        let _ = DrawEdge(
            dc,
            &mut edge,
            if item.itemState.0 & ODS_SELECTED.0 != 0 {
                EDGE_SUNKEN
            } else {
                EDGE_RAISED
            },
            BF_RECT,
        );
        let dpi = GetDpiForWindow(item.hwndItem);
        let px = |v: i32| v * dpi as i32 / 96;
        let x = r.left + px(8);
        let y = r.top + (r.bottom - r.top - px(16)) / 2;
        let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
        let color = if disabled {
            COLORREF(GetSysColor(COLOR_GRAYTEXT))
        } else if item.CtlID == RUN as u32 {
            COLORREF(0x34872b)
        } else if item.CtlID == STOP as u32 {
            COLORREF(0x3434c0)
        } else {
            COLORREF(0x9c6428)
        };
        let pen = CreatePen(PS_SOLID, px(2).max(1), color);
        let brush = CreateSolidBrush(color);
        let old_pen = SelectObject(dc, pen.into());
        let old_brush = SelectObject(dc, brush.into());
        match item.CtlID as u16 {
            RUN => {
                let points = [
                    POINT { x, y },
                    POINT {
                        x: x + px(14),
                        y: y + px(8),
                    },
                    POINT { x, y: y + px(16) },
                ];
                let _ = Polygon(dc, &points);
            }
            STOP => {
                let _ = Rectangle(dc, x, y, x + px(15), y + px(15));
            }
            CONNECT => {
                let _ = Rectangle(dc, x + px(3), y + px(5), x + px(13), y + px(12));
                let _ = MoveToEx(dc, x + px(5), y, None);
                let _ = LineTo(dc, x + px(5), y + px(5));
                let _ = MoveToEx(dc, x + px(11), y, None);
                let _ = LineTo(dc, x + px(11), y + px(5));
                let _ = MoveToEx(dc, x + px(8), y + px(12), None);
                let _ = LineTo(dc, x + px(8), y + px(18));
            }
            ESTIMATE => {
                for (dx, dy) in [(5, 0), (0, 11), (11, 11)] {
                    let _ = Rectangle(dc, x + px(dx), y + px(dy), x + px(dx + 5), y + px(dy + 5));
                }
                let _ = MoveToEx(dc, x + px(7), y + px(5), None);
                let _ = LineTo(dc, x + px(2), y + px(11));
                let _ = MoveToEx(dc, x + px(7), y + px(5), None);
                let _ = LineTo(dc, x + px(13), y + px(11));
            }
            _ => {
                let _ = Arc(dc, x, y, x + px(16), y + px(16), x, y + px(8), x + px(8), y);
                let _ = MoveToEx(dc, x, y + px(8), None);
                let _ = LineTo(dc, x + px(5), y + px(8));
            }
        }
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        let _ = DeleteObject(pen.into());
        let _ = DeleteObject(brush.into());
        let _ = SetBkMode(dc, TRANSPARENT);
        let mut label = wide(&text(item.hwndItem));
        let mut label_rect = RECT {
            left: x + px(24),
            ..r
        };
        label.pop();
        let tool_font = send_message(item.hwndItem, WM_GETFONT, WPARAM(0), LPARAM(0));
        let old_font = SelectObject(dc, HGDIOBJ(tool_font.0 as *mut _));
        let _ = SetTextColor(
            dc,
            COLORREF(GetSysColor(if disabled {
                COLOR_GRAYTEXT
            } else {
                COLOR_BTNTEXT
            })),
        );
        DrawTextW(
            dc,
            &mut label,
            &mut label_rect,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT,
        );
        SelectObject(dc, old_font);
        if item.itemState.0 & ODS_FOCUS.0 != 0 {
            let _ = DrawFocusRect(dc, &r);
        }
    }
}

fn detail_dialog(parent: HWND, title: &str, value: &str, font: HFONT, dpi: u32) {
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return;
        };
        let class = WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: w!("YYDbName"),
            lpfnWndProc: Some(name_proc),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            ..Default::default()
        };
        RegisterClassW(&class);
        let px = |n: i32| n * dpi as i32 / 96;
        let mut rect = RECT::default();
        let _ = GetWindowRect(parent, &mut rect);
        let Ok(dialog) = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            w!("YYDbName"),
            &HSTRING::from(title),
            WS_CAPTION | WS_SYSMENU | WS_POPUP,
            rect.left + px(40),
            rect.top + px(40),
            px(900),
            px(580),
            Some(parent),
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };
        let mut state = NameDialog::default();
        SetWindowLongPtrW(dialog, GWLP_USERDATA, &mut state as *mut _ as isize);
        let edit = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            w!("EDIT"),
            None,
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WS_VSCROLL
                | WS_HSCROLL
                | WINDOW_STYLE(
                    (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_AUTOHSCROLL) as u32,
                ),
            px(12),
            px(12),
            px(860),
            px(470),
            Some(dialog),
            None,
            Some(instance.into()),
            None,
        );
        if let Ok(edit) = edit {
            state.edit.set(edit);
            send_message(edit, EM_SETLIMITTEXT, WPARAM(i32::MAX as usize), LPARAM(0));
            set_text(edit, value);
            send_message(edit, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
        }
        if let Ok(button) = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("BUTTON"),
            w!("閉じる"),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32),
            px(770),
            px(500),
            px(100),
            px(28),
            Some(dialog),
            Some(HMENU(2usize as *mut _)),
            Some(instance.into()),
            None,
        ) {
            send_message(button, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
        }
        let _ = EnableWindow(parent, false);
        let _ = ShowWindow(dialog, SW_SHOW);
        let mut msg = MSG::default();
        while !state.done.get() {
            let code = GetMessageW(&mut msg, None, 0, 0).0;
            if code <= 0 {
                if code == 0 {
                    PostQuitMessage(msg.wParam.0 as i32);
                }
                break;
            }
            if !IsDialogMessageW(dialog, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let _ = EnableWindow(parent, true);
        let _ = DestroyWindow(dialog);
        let _ = SetForegroundWindow(parent);
    }
}

fn layout(hwnd: HWND) {
    unsafe {
        let mut r = RECT::default();
        let _ = GetClientRect(hwnd, &mut r);
        if r.right <= 0 || r.bottom <= 0 {
            return;
        }
        // Moving a child can synchronously send paint messages. Release APP before
        // calling Win32 so those messages can access the editor and result state.
        let placements = with_app(|a| {
            let show_properties = document_tabs::properties_visible(a);
            let px = |dip: i32| ((dip as i64 * a.dpi as i64 + 48) / 96) as i32;
            let top = px(76);
            let split =
                (r.bottom * 46 / 100).clamp(top + px(120), (r.bottom - px(220)).max(top + px(120)));
            let placements = a
                .controls
                .iter()
                .map(|&(h, x, y, width, height)| {
                    let (x, y, width, height) = if h == a.tree {
                        (r.right - px(280), px(108), px(272), r.bottom - px(162))
                    } else if h == a.editor_hwnd {
                        (px(8), top, r.right - px(296), split - top - px(8))
                    } else if h == a.result_tabs {
                        (px(8), split, r.right - px(296), px(28))
                    } else if h == a.grid_hwnd {
                        (
                            px(8),
                            if a.documents.er_active {
                                top
                            } else {
                                split + px(34)
                            },
                            r.right - px(if show_properties { 536 } else { 296 }),
                            if a.documents.er_active {
                                r.bottom - top - px(62)
                            } else {
                                r.bottom - split - px(96)
                            },
                        )
                    } else if h == a.properties {
                        (
                            r.right - px(520),
                            if a.documents.er_active {
                                top
                            } else {
                                split + px(34)
                            },
                            px(224),
                            if a.documents.er_active {
                                r.bottom - top - px(62)
                            } else {
                                r.bottom - split - px(96)
                            },
                        )
                    } else if h == a.status {
                        (px(8), r.bottom - px(48), r.right - px(16), px(40))
                    } else if h == a.kind {
                        (px(236), px(8), px(145), px(height))
                    } else if h == a.target {
                        (px(389), px(8), r.right - px(397), px(28))
                    } else if h == a.sql_tabs {
                        (px(8), px(44), r.right - px(296), px(28))
                    } else if GetDlgCtrlID(h) == 211 {
                        (r.right - px(280), px(44), px(272), px(24))
                    } else if GetDlgCtrlID(h) == PROFILE_NEW as i32 {
                        (r.right - px(280), px(74), px(134), px(28))
                    } else if GetDlgCtrlID(h) == PROFILE_SAVE as i32 {
                        (r.right - px(138), px(74), px(130), px(28))
                    } else {
                        (
                            px(x),
                            px(if y == 44 { 8 } else { y }),
                            px(width),
                            px(height),
                        )
                    };
                    (h, x, y, width.max(1), height.max(1))
                })
                .collect::<Vec<_>>();
            (
                placements,
                a.properties,
                show_properties,
                a.editor_hwnd,
                a.documents.er_active,
            )
        });
        if let Some((placements, properties, show_properties, editor, er_active)) = placements {
            let grid = with_app(|a| a.grid_hwnd);
            if let Some((tabs, has_results)) = with_app(|a| {
                (
                    a.result_tabs,
                    !a.documents.er_active && !a.results.is_empty(),
                )
            }) {
                let _ = ShowWindow(
                    tabs,
                    if has_results {
                        SW_SHOWNOACTIVATE
                    } else {
                        SW_HIDE
                    },
                );
            }
            let _ = ShowWindow(
                editor,
                if er_active {
                    SW_HIDE
                } else {
                    SW_SHOWNOACTIVATE
                },
            );
            for id in [RUN, ESTIMATE] {
                if let Ok(button) = GetDlgItem(Some(hwnd), id as i32) {
                    let _ = EnableWindow(button, !er_active);
                }
            }
            {
                let menu = GetMenu(hwnd);
                for id in [RUN, ESTIMATE, ACTUAL, SAVE_SQL] {
                    let _ = EnableMenuItem(
                        menu,
                        id as u32,
                        MF_BYCOMMAND | if er_active { MF_GRAYED } else { MF_ENABLED },
                    );
                }
            }
            for (h, x, y, width, height) in placements {
                let _ = MoveWindow(h, x, y, width, height, false);
            }
            let _ = ShowWindow(
                properties,
                if show_properties {
                    SW_SHOWNOACTIVATE
                } else {
                    SW_HIDE
                },
            );
            let _ = RedrawWindow(
                Some(hwnd),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
            );
            if let Some(grid) = grid {
                plan_view::sync(grid);
            }
        }
    }
}
fn file_dialog(save: bool, extension: &str) -> Option<PathBuf> {
    file_dialog_title(save, extension, None)
}
fn file_dialog_title(save: bool, extension: &str, title: Option<&str>) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileDialog = CoCreateInstance(
            if save {
                &FileSaveDialog
            } else {
                &FileOpenDialog
            },
            None,
            CLSCTX_INPROC_SERVER,
        )
        .ok()?;
        let name = HSTRING::from(if extension == "plan" && !save {
            "実行計画（全DB）".to_owned()
        } else {
            format!("*.{extension}")
        });
        let pattern = HSTRING::from(if extension == "plan" && !save {
            "*.yyplan;*.sqlplan;*.xml;*.json;*.csv;*.db2plan;*.plan;*.txt".to_owned()
        } else if extension == "yyer.yaml" {
            "*.yyer.yaml;*.yaml;*.yml;*.yyer".to_owned()
        } else {
            format!("*.{extension}")
        });
        let filters = [
            COMDLG_FILTERSPEC {
                pszName: PCWSTR(name.as_ptr()),
                pszSpec: PCWSTR(pattern.as_ptr()),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("すべてのファイル"),
                pszSpec: w!("*.*"),
            },
        ];
        dialog.SetFileTypes(&filters).ok()?;
        dialog.SetDefaultExtension(&HSTRING::from(extension)).ok()?;
        if let Some(title) = title {
            dialog.SetTitle(&HSTRING::from(title)).ok()?;
        }
        dialog.Show(None).ok()?;
        let item = dialog.GetResult().ok()?;
        let path = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = path.to_string().ok();
        CoTaskMemFree(Some(path.as_ptr().cast()));
        s.map(PathBuf::from)
    }
}
fn read_plan(path: &std::path::Path) -> yy_db_core::Result<Plan> {
    yy_db_core::plan_file::load(path)
}
fn command(id: u16) {
    if performance::command(id) {
        return;
    }
    if let Some(section) = help::section(id) {
        if let Err(error) = help::show(section) {
            message(&format!("ヘルプを表示できません: {error}"));
        }
        return;
    }
    if matches!(id, RUN | ESTIMATE | ACTUAL | SAVE_SQL)
        && with_app(|a| a.documents.er_active).unwrap_or(false)
    {
        with_app(|a| {
            set_text(
                a.status,
                "SQLを実行・保存するにはSQLタブを選択してください。",
            )
        });
        return;
    }
    if er_view::command(id) {
        return;
    }
    let selected = with_app(|a| {
        let n = unsafe { send_message(a.profile_combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
        (
            unsafe { GetParent(a.editor_hwnd).unwrap_or_default() },
            a.profiles
                .get(n as usize)
                .map(|p| p.name.clone())
                .unwrap_or_default(),
            a.font,
            a.dpi,
        )
    });
    if matches!(id, PROFILE_NEW | PROFILE_SAVE) {
        let Some((parent, _, font, dpi)) = selected else {
            return;
        };
        let context = with_app(|a| {
            let index = unsafe {
                send_message(a.profile_combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as usize
            };
            let old = if id == PROFILE_SAVE {
                a.profiles.get(index).cloned()
            } else {
                None
            };
            let names = a
                .profiles
                .iter()
                .enumerate()
                .filter(|(i, _)| id == PROFILE_NEW || *i != index)
                .map(|(_, p)| p.name.clone())
                .collect();
            (index, old, names)
        });
        let Some((index, old, names)) = context else {
            return;
        };
        if let Some(profile) = connection_dialog::edit(parent, old, names, font, dpi) {
            let result = with_app(|a| -> yy_db_core::Result<()> {
                let mut profiles = a.profiles.clone();
                let index = if id == PROFILE_SAVE && index < profiles.len() {
                    profiles[index] = profile;
                    index
                } else {
                    profiles.push(profile);
                    profiles.len() - 1
                };
                super::profiles::save(&a.profiles_path, &profiles)?;
                a.profiles = profiles;
                refresh_profiles(a);
                select_profile(a, index);
                let objects = a.tree_items.iter().map(|e| e.1.clone()).collect();
                populate_tree(a, objects);
                set_text(
                    a.status,
                    "接続先を保存しました。右側の接続先をダブルクリックすると接続します。",
                );
                Ok(())
            });
            if let Some(Err(error)) = result {
                message(&error);
            }
        }
        return;
    }
    if id == CONNECT
        && with_app(|a| unsafe {
            send_message(a.profile_combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 < 0
        })
        .unwrap_or(true)
    {
        with_app(|a| {
            set_text(
                a.status,
                "右側のオブジェクトエクスプローラーで接続先を選択してください。「新規接続…」から登録できます。",
            )
        });
        return;
    }
    if id == PROFILE_DELETE {
        let Some((parent, name, _, _)) = selected.as_ref() else {
            return;
        };
        if name.is_empty() {
            message("削除する接続設定を選択してください");
            return;
        }
        if unsafe {
            MessageBoxW(
                Some(*parent),
                &HSTRING::from(format!("接続設定「{name}」を削除しますか？")),
                w!("接続設定"),
                MB_YESNO | MB_ICONQUESTION,
            )
        } != IDYES
        {
            return;
        }
    }
    // Dialogs run outside the RefCell borrow because their modal loop dispatches messages.
    let transfer_settings = if matches!(id, IMPORT | EXPORT) {
        let Some((parent, _, font, dpi)) = selected else {
            return;
        };
        let Some(settings) = with_app(|a| a.transfer_settings.clone()) else {
            return;
        };
        let Some(settings) = transfer_dialog::edit(parent, id == EXPORT, settings, font, dpi)
        else {
            return;
        };
        with_app(|a| a.transfer_settings = settings.clone());
        settings
    } else {
        transfer_dialog::Settings::default()
    };
    let file_mode = transfer_settings.format;
    let extension = match file_mode {
        1 => "tsv",
        3 => "parquet",
        4 => "dat",
        _ => "csv",
    };
    let path = match id {
        OPEN_SQL => file_dialog(false, "sql"),
        SAVE_SQL => file_dialog(true, "sql"),
        IMPORT => file_dialog(false, extension),
        EXPORT => file_dialog(true, extension),
        OPEN_PLAN => file_dialog_title(false, "plan", Some("実行計画を開く（接続不要・全DB）")),
        SAVE_PLAN => file_dialog(true, "yyplan"),
        SAVE_SVG => file_dialog(true, "svg"),
        SAVE_DRAW => file_dialog(true, "yydraw"),
        _ => None,
    };
    if matches!(
        id,
        OPEN_SQL | SAVE_SQL | IMPORT | EXPORT | OPEN_PLAN | SAVE_PLAN | SAVE_SVG | SAVE_DRAW
    ) && path.is_none()
    {
        return;
    }
    let copybook = if matches!(id, IMPORT | EXPORT) && file_mode == 4 {
        let Some(p) = file_dialog(false, "cbl") else {
            return;
        };
        Some(p)
    } else {
        None
    };
    let outcome = with_app(|a| -> yy_db_core::Result<()> {
        let mut send = None;
        match id {
            PROFILE_DELETE => {
                let index = unsafe {
                    send_message(a.profile_combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as usize
                };
                if index < a.profiles.len() {
                    let mut profiles = a.profiles.clone();
                    profiles.remove(index);
                    super::profiles::save(&a.profiles_path, &profiles)?;
                    a.profiles = profiles;
                    refresh_profiles(a);
                    let objects = a.tree_items.iter().map(|e| e.1.clone()).collect();
                    populate_tree(a, objects);
                    set_text(
                        a.status,
                        "接続設定を削除しました。現在のDB接続は継続しています。",
                    );
                }
            }
            STOP => {
                if let Some(c) = &a.cancellation {
                    c.cancel();
                    set_text(
                        a.status,
                        "停止を要求しました。DBからの完了通知を待っています。",
                    );
                }
                return Ok(());
            }
            NEW_SQL => {
                document_tabs::select(a, false);
                a.editors
                    .push(Editor::new("", a.dpi).map_err(|e| e.to_string())?);
                a.sql_index = a.editors.len() - 1;
                add_sql_tab(a.sql_tabs, a.sql_index, &format!("SQL {}", a.sql_index + 1));
                invalidate(a.editor_hwnd);
            }
            DB2_ACTUAL_SETUP => {
                document_tabs::select(a, false);
                a.editors.push(
                    Editor::new(yy_db_core::plan::DB2_ACTUAL_SETUP_SQL, a.dpi)
                        .map_err(|e| e.to_string())?,
                );
                a.sql_index = a.editors.len() - 1;
                add_sql_tab(a.sql_tabs, a.sql_index, "Db2実測収集の準備");
                invalidate(a.editor_hwnd);
                set_text(
                    a.status,
                    "準備SQLを表示しました。接続先では未実行です。必要な文をDB管理者が確認して実行してください。",
                );
            }
            OPEN_SQL => {
                document_tabs::select(a, false);
                let text = std::fs::read_to_string(path.unwrap()).map_err(|e| e.to_string())?;
                a.editors[a.sql_index] = Editor::new(&text, a.dpi).map_err(|e| e.to_string())?;
                a.performance.clear_tab(a.sql_index);
                invalidate(a.editor_hwnd);
            }
            SAVE_SQL => std::fs::write(path.unwrap(), a.editors[a.sql_index].text())
                .map_err(|e| e.to_string())?,
            OPEN_PLAN => {
                let path = path.unwrap();
                let plan = read_plan(&path)?;
                document_tabs::select(a, false);
                plan_view::present(a, plan)?;
                invalidate(a.grid_hwnd);
            }
            SAVE_PLAN => {
                let p = a.plan.as_ref().ok_or("計画がありません")?;
                std::fs::write(
                    path.unwrap(),
                    serde_json::to_vec_pretty(p).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
            }
            SAVE_SVG => {
                let doc = a.drawing.as_ref().ok_or("図がありません")?;
                let svg = a
                    .renderer
                    .svg(doc, &yy_draw::render::Export::document(doc))?;
                std::fs::write(path.unwrap(), svg).map_err(|e| e.to_string())?;
            }
            SAVE_DRAW => {
                let doc = a.drawing.as_ref().ok_or("図がありません")?;
                yy_draw::format::save(&path.unwrap(), doc)?;
            }
            CONNECT => {
                if a.busy {
                    return Err("実行完了後に接続を変更してください".into());
                }
                let index = unsafe {
                    send_message(a.kind, CB_GETCURSEL, WPARAM(0), LPARAM(0))
                        .0
                        .max(0) as usize
                };
                let kind = DbKind::ALL[index.min(6)];
                let target = text(a.target);
                a.performance.pending = Some((kind, target.clone()));
                let profile_index = unsafe {
                    send_message(a.profile_combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as usize
                };
                a.pending_label = a
                    .profiles
                    .get(profile_index)
                    .filter(|p| p.kind == kind && p.target == target)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| kind.name().into());
                send = Some(Command::Connect(Options {
                    kind,
                    path: if matches!(kind, DbKind::Sqlite | DbKind::DuckDb)
                        && !target.trim().is_empty()
                        && target != ":memory:"
                    {
                        Some(PathBuf::from(&target))
                    } else {
                        None
                    },
                    connection_string: target,
                    read_only: false,
                }));
            }
            RUN | ESTIMATE | ACTUAL => {
                a.performance.check(a.sql_index)?;
                let sql = a.editors[a.sql_index].execution_text();
                send = Some(if id == RUN {
                    Command::Run(sql)
                } else {
                    Command::Explain(sql, id == ACTUAL)
                });
            }
            TABLES => send = Some(Command::Tables),
            CATALOG_REFRESH => send = Some(Command::Catalog),
            ER => send = Some(Command::Diagram),
            COMMIT => send = Some(Command::Run("COMMIT".into())),
            ROLLBACK => send = Some(Command::Run("ROLLBACK".into())),
            EXPORT => {
                let format = file_format(&transfer_settings, copybook)?;
                send = Some(Command::Export(
                    a.results
                        .get(a.result_index)
                        .ok_or("出力する結果がありません")?
                        .clone(),
                    path.unwrap(),
                    format,
                ));
            }
            IMPORT => {
                let p = path.unwrap();
                let table = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("imported")
                    .to_owned();
                send = Some(Command::Import(
                    p,
                    file_format(&transfer_settings, copybook)?,
                    table,
                ));
            }
            _ => {}
        }
        if let Some(cmd) = send {
            if a.busy {
                return Err("別の操作が実行中です".into());
            }
            a.tx.send(cmd).map_err(|e| e.to_string())?;
            a.busy = true;
            set_text(
                a.status,
                "処理中…（SQL実行は接続を変更せず同じセッションで行います）",
            );
        }
        Ok(())
    });
    if let Some(Err(e)) = outcome {
        message(&e);
    }
}
unsafe extern "system" fn frame_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_APP + 43 {
        if unsafe { IsWindowEnabled(hwnd) }.as_bool() {
            layout(hwnd);
            if let Some(target) = with_app(|a| {
                if a.documents.er_active {
                    a.grid_hwnd
                } else {
                    a.editor_hwnd
                }
            }) {
                unsafe {
                    let _ = SetFocus(Some(target));
                }
            }
        }
        return LRESULT(0);
    }
    if msg == WM_APP + 42 {
        return LRESULT(isize::from(with_app(|_| true).unwrap_or(false)));
    }
    match msg {
        WM_DRAWITEM => {
            if l.0 != 0 {
                let item = unsafe { &*(l.0 as *const DRAWITEMSTRUCT) };
                if item.CtlType == ODT_BUTTON {
                    draw_tool(item);
                    return LRESULT(1);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, w, l) }
        }
        WM_NOTIFY => {
            if l.0 == 0 {
                return LRESULT(0);
            }
            let notification = unsafe { &*(l.0 as *const NMHDR) };
            let mut prevent = false;
            let mut detail = None;
            let mut tree_command = None;
            let mut result_changed = false;
            with_app(|a| {
                if notification.hwndFrom == a.properties && notification.code == NM_DBLCLK {
                    let row = unsafe {
                        send_message(
                            a.properties,
                            LVM_GETNEXTITEM,
                            WPARAM(usize::MAX),
                            LPARAM(LVNI_SELECTED as isize),
                        )
                        .0
                    } as usize;
                    if let Some(plan) = a.plan.as_ref()
                        && let Some(hit) = a.plan_view.selected
                    {
                        let (index, edge) = match hit {
                            yy_db_core::plan_visual::Hit::Node(i) => (i, false),
                            yy_db_core::plan_visual::Hit::Edge(i) => (i, true),
                        };
                        if let Some((name, value)) =
                            yy_db_core::plan_visual::details(plan, index, edge)
                                .into_iter()
                                .nth(row)
                        {
                            detail = Some((name, value, a.font, a.dpi));
                        }
                    } else if let Some(model) = a.er.as_ref()
                        && let Some(index) = a.er_view.selected
                        && let Some((name, value)) =
                            model.entity_details(index).into_iter().nth(row)
                    {
                        detail = Some((name, value, a.font, a.dpi));
                    }
                    return;
                }
                if notification.hwndFrom == a.sql_tabs && notification.code == TCN_SELCHANGE {
                    let index = unsafe {
                        send_message(a.sql_tabs, TCM_GETCURSEL, WPARAM(0), LPARAM(0))
                            .0
                            .max(0) as usize
                    };
                    if a.documents.er_open && index == a.editors.len() {
                        document_tabs::select(a, true);
                    } else {
                        a.sql_index = index.min(a.editors.len() - 1);
                        document_tabs::select(a, false);
                    }
                    result_changed = true;
                    invalidate(a.editor_hwnd);
                    return;
                }
                if notification.hwndFrom == a.result_tabs && notification.code == TCN_SELCHANGE {
                    let index = unsafe {
                        send_message(a.result_tabs, TCM_GETCURSEL, WPARAM(0), LPARAM(0)).0
                    };
                    if index >= 0 && (index as usize) < a.results.len() {
                        document_tabs::select(a, false);
                        a.result_index = index as usize;
                        a.drawing = None;
                        a.plan = None;
                        a.top = 0;
                        a.left = 0;
                        a.selection = (0, 0);
                        result_changed = true;
                        invalidate(a.grid_hwnd);
                    }
                    return;
                }
                if notification.hwndFrom != a.tree {
                    return;
                }
                if notification.code == TVN_SELCHANGEDW {
                    let event = unsafe { &*(l.0 as *const NMTREEVIEWW) };
                    if let Some(index) = tree_profile(a, event.itemNew.hItem) {
                        select_profile(a, index);
                    } else {
                        unsafe {
                            send_message(
                                a.profile_combo,
                                CB_SETCURSEL,
                                WPARAM(usize::MAX),
                                LPARAM(0),
                            );
                        }
                    }
                }
                if notification.code == NM_DBLCLK {
                    let item = HTREEITEM(unsafe {
                        send_message(
                            a.tree,
                            TVM_GETNEXTITEM,
                            WPARAM(TVGN_CARET as usize),
                            LPARAM(0),
                        )
                        .0
                    });
                    if let Some((_, index)) = a.profile_items.iter().find(|(h, _)| *h == item) {
                        let index = *index;
                        tree_command = Some(if index == usize::MAX {
                            PROFILE_NEW
                        } else {
                            select_profile(a, index);
                            CONNECT
                        });
                        return;
                    }
                }
                if notification.code == TVN_ITEMEXPANDINGW {
                    let event = unsafe { &*(l.0 as *const NMTREEVIEWW) };
                    if event.action == TVE_EXPAND
                        && let Some((item, object, false)) =
                            a.tree_items.iter().find(|e| e.0 == event.itemNew.hItem)
                    {
                        if a.busy {
                            prevent = true;
                            return;
                        }
                        if a.tx
                            .send(Command::Columns(
                                item.0 as usize,
                                object.schema.clone(),
                                object.name.clone(),
                            ))
                            .is_ok()
                        {
                            a.busy = true;
                        }
                        prevent = true;
                    }
                } else if notification.code == NM_DBLCLK {
                    let selected = HTREEITEM(unsafe {
                        send_message(
                            a.tree,
                            TVM_GETNEXTITEM,
                            WPARAM(TVGN_CARET as usize),
                            LPARAM(0),
                        )
                        .0
                    });
                    if let Some((_, object, _)) = a
                        .tree_items
                        .iter()
                        .find(|e| e.0 == selected && matches!(e.1.kind.as_str(), "TABLE" | "VIEW"))
                        && let (Ok(schema), Ok(name)) = (
                            yy_db_core::sql::identifier_for(a.connection_kind, &object.schema),
                            yy_db_core::sql::identifier_for(a.connection_kind, &object.name),
                        )
                        && let Ok(editor) =
                            Editor::new(&format!("SELECT * FROM {schema}.{name};"), a.dpi)
                    {
                        let label = object.name.clone();
                        document_tabs::select(a, false);
                        a.editors.push(editor);
                        a.sql_index = a.editors.len() - 1;
                        add_sql_tab(a.sql_tabs, a.sql_index, &label);
                        invalidate(a.editor_hwnd);
                        result_changed = true;
                    }
                }
            });
            if result_changed {
                layout(hwnd);
            }
            if let Some(id) = tree_command {
                command(id);
            }
            if let Some((name, value, font, dpi)) = detail {
                detail_dialog(hwnd, &name, &value, font, dpi);
            }
            LRESULT(isize::from(prevent))
        }
        WM_GETMINMAXINFO => {
            unsafe {
                let dpi = GetDpiForWindow(hwnd).max(96);
                let mut bounds = RECT {
                    right: (980 * dpi / 96) as i32,
                    bottom: (600 * dpi / 96) as i32,
                    ..Default::default()
                };
                let _ = AdjustWindowRectExForDpi(
                    &mut bounds,
                    WS_OVERLAPPEDWINDOW,
                    false,
                    WINDOW_EX_STYLE::default(),
                    dpi,
                );
                let info = &mut *(l.0 as *mut MINMAXINFO);
                info.ptMinTrackSize = POINT {
                    x: bounds.right - bounds.left,
                    y: bounds.bottom - bounds.top,
                };
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = (w.0 & 0xffff) as u32;
            let update = with_app(|a| {
                a.dpi = dpi;
                a.grid.set_dpi(dpi);
                for editor in &mut a.editors {
                    editor.dpi = dpi;
                    editor.renderer.set_dpi(dpi);
                }
                let old = a.font;
                a.font = control_font(dpi);
                (
                    a.controls.iter().map(|c| c.0).collect::<Vec<_>>(),
                    a.font,
                    old,
                )
            });
            unsafe {
                if let Some((controls, font, old)) = update {
                    for h in controls {
                        send_message(h, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
                    }
                    let _ = DeleteObject(old.into());
                }
                let bounds = &*(l.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    bounds.left,
                    bounds.top,
                    bounds.right - bounds.left,
                    bounds.bottom - bounds.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            layout(hwnd);
            LRESULT(0)
        }
        WM_SIZE => {
            if w.0 != SIZE_MINIMIZED as usize {
                layout(hwnd);
            }
            LRESULT(0)
        }
        WM_CONTEXTMENU => {
            if with_app(|a| a.tree.0 as usize == w.0).unwrap_or(false) {
                tree_context(
                    hwnd,
                    POINT {
                        x: (l.0 as u16 as i16) as i32,
                        y: ((l.0 >> 16) as u16 as i16) as i32,
                    },
                );
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, w, l) }
        }
        WM_COMMAND => {
            let id = (w.0 & 0xffff) as u16;
            let notification = (w.0 >> 16) as u16;
            if notification == CBN_SELCHANGE as u16 && id == PROFILES {
                with_app(|a| {
                    let n = unsafe {
                        send_message(a.profile_combo, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as usize
                    };
                    if let Some(p) = a.profiles.get(n) {
                        let index = DbKind::ALL.iter().position(|k| *k == p.kind).unwrap_or(0);
                        unsafe {
                            send_message(a.kind, CB_SETCURSEL, WPARAM(index), LPARAM(0));
                        }
                        set_text(a.target, &p.target);
                        set_text(
                            a.status,
                            "接続設定を選択しました。「接続」で接続先を切り替えます。",
                        );
                    }
                });
            } else if notification == 0 {
                command(id);
            }
            layout(hwnd);
            LRESULT(0)
        }
        WM_TIMER => {
            let relayout = with_app(|a| {
                let properties_visible = (
                    document_tabs::properties_visible(a),
                    !a.results.is_empty(),
                    a.documents.er_active,
                );
                while let Ok(event) = a.rx.try_recv() {
                    a.busy = false;
                    let restore_er = a.documents.er_active
                        && matches!(
                            &event,
                            Event::Connected(..) | Event::Results(..) | Event::Plan(..)
                        );
                    if restore_er {
                        document_tabs::swap_canvas(a);
                    }
                    match event {
                        Event::Connected(c, kind) => {
                            a.performance.connected = a.performance.pending.take();
                            a.cancellation = Some(c);
                            a.connection_kind = kind;
                            a.connection_label = a.pending_label.clone();
                            a.results.clear();
                            a.plan = None;
                            a.drawing = None;
                            a.tree_items.clear();
                            unsafe {
                                send_message(
                                    a.result_tabs,
                                    TCM_DELETEALLITEMS,
                                    WPARAM(0),
                                    LPARAM(0),
                                );
                                send_message(a.tree, TVM_DELETEITEM, WPARAM(0), LPARAM(TVI_ROOT.0));
                            }
                            tree_insert(a.tree, TVI_ROOT, "オブジェクトを取得しています…", false);
                            if a.tx.send(Command::Catalog).is_ok() {
                                a.busy = true;
                            }
                            invalidate(a.grid_hwnd);
                            set_text(a.status, "接続しました。F5でSQLを実行できます。");
                        }
                        Event::Results(sets, summary) => {
                            a.results = sets.into_iter().map(Arc::new).collect();
                            a.result_index = 0;
                            a.top = 0;
                            a.left = 0;
                            a.drawing = None;
                            a.plan = None;
                            unsafe {
                                send_message(
                                    a.result_tabs,
                                    TCM_DELETEALLITEMS,
                                    WPARAM(0),
                                    LPARAM(0),
                                );
                                for (i, r) in a.results.iter().enumerate() {
                                    add_sql_tab(
                                        a.result_tabs,
                                        i,
                                        &format!("結果 {}（{}行）", i + 1, r.row_count),
                                    );
                                }
                                send_message(a.result_tabs, TCM_SETCURSEL, WPARAM(0), LPARAM(0));
                            }
                            set_text(a.status, &summary);
                            invalidate(a.grid_hwnd);
                        }
                        Event::Plan(plan) => {
                            if let Err(e) = plan_view::present(a, plan) {
                                set_text(a.status, &e);
                            }
                        }
                        Event::Diagram(model) => er_view::present(a, model),
                        Event::Message(s) => set_text(a.status, &s),
                        Event::Failed(s) => set_text(a.status, &format!("エラー: {s}")),
                        Event::Catalog(objects) => populate_tree(a, objects),
                        Event::Columns(item, columns) => {
                            let parent = HTREEITEM(item as isize);
                            for column in columns {
                                tree_insert(
                                    a.tree,
                                    parent,
                                    &format!(
                                        "{} : {} {}",
                                        column.name,
                                        column.data_type,
                                        if matches!(column.nullable.as_str(), "NO" | "N") {
                                            "NOT NULL"
                                        } else {
                                            "NULL"
                                        }
                                    ),
                                    false,
                                );
                            }
                            if let Some(entry) =
                                a.tree_items.iter_mut().find(|entry| entry.0 == parent)
                            {
                                entry.2 = true;
                            }
                            unsafe {
                                send_message(
                                    a.tree,
                                    TVM_EXPAND,
                                    WPARAM(TVE_EXPAND.0 as usize),
                                    LPARAM(parent.0),
                                );
                            }
                        }
                    }
                    if restore_er {
                        document_tabs::swap_canvas(a);
                        er_view::properties(a);
                    }
                }
                properties_visible
                    != (
                        document_tabs::properties_visible(a),
                        !a.results.is_empty(),
                        a.documents.er_active,
                    )
            });
            if relayout == Some(true) {
                layout(hwnd);
            }
            plan_view::tick();
            let smoke_dir = with_app(|a| a.smoke.as_ref().map(|s| s.directory.clone())).flatten();
            match with_app(smoke_tick) {
                Some(Ok(true)) => unsafe {
                    if let Some(dir) = smoke_dir
                        && let Err(e) = resize_smoke(hwnd, &dir)
                    {
                        let _ = std::fs::write(dir.join("ui-error.txt"), e);
                        PostQuitMessage(1);
                        return LRESULT(0);
                    }
                    let _ = DestroyWindow(hwnd);
                },
                Some(Err(e)) => {
                    if let Some(dir) = smoke_dir {
                        let _ = std::fs::write(dir.join("ui-error.txt"), e);
                    }
                    unsafe {
                        PostQuitMessage(1);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let busy = with_app(|a| a.busy).unwrap_or(false);
            if busy {
                message("処理中です。停止して完了を待ってから閉じてください。");
            } else {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
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
unsafe extern "system" fn editor_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let handled = with_app(|a| {
        let editor = &mut a.editors[a.sql_index];
        match msg {
            WM_PAINT => {
                unsafe {
                    let mut ps = PAINTSTRUCT::default();
                    BeginPaint(hwnd, &mut ps);
                    let mut r = RECT::default();
                    let _ = GetClientRect(hwnd, &mut r);
                    if editor
                        .paint(hwnd, r.right.max(1) as u32, r.bottom.max(1) as u32)
                        .is_ok()
                    {
                        a.editor_painted = (r.right, r.bottom);
                    }
                    let _ = EndPaint(hwnd, &ps);
                }
                true
            }
            WM_ERASEBKGND => true,
            WM_GETDLGCODE => {
                return Some(LRESULT(
                    (DLGC_WANTALLKEYS | DLGC_WANTCHARS | DLGC_WANTARROWS) as isize,
                ));
            }
            WM_CHAR => {
                editor.character(w.0 as u16);
                invalidate(hwnd);
                true
            }
            WM_KEYDOWN => {
                let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
                let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
                editor.key(hwnd, w.0 as u32, ctrl, shift);
                invalidate(hwnd);
                true
            }
            WM_LBUTTONDOWN => {
                unsafe {
                    let _ = SetFocus(Some(hwnd));
                }
                editor.click(
                    l.0 as i16 as i32,
                    (l.0 >> 16) as i16 as i32,
                    unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0,
                );
                invalidate(hwnd);
                true
            }
            WM_IME_COMPOSITION => {
                let update = yy_ui_win::ime::read_composition(hwnd, l.0 as u32);
                if let Some(result) = update.result {
                    editor.document.insert_text(&result, false);
                }
                editor.composition =
                    update
                        .composing
                        .filter(|(s, _)| !s.is_empty())
                        .map(|(text, cursor)| yy_ui_win::render::Composition {
                            offsets: vec![editor.document.selections().primary().head],
                            text,
                            cursor,
                        });
                invalidate(hwnd);
                true
            }
            WM_IME_ENDCOMPOSITION => {
                editor.composition = None;
                invalidate(hwnd);
                true
            }
            _ => false,
        }
        .then_some(LRESULT(0))
    })
    .flatten();
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, w, l) })
}
unsafe extern "system" fn grid_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if let Some(result) = plan_view::handle(hwnd, msg, w, l) {
        return result;
    }
    if let Some(result) = er_view::handle(hwnd, msg, w, l) {
        return result;
    }
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut r = RECT::default();
                let _ = GetClientRect(hwnd, &mut r);
                FillRect(dc, &r, GetSysColorBrush(COLOR_WINDOW));
                with_app(|a| {
                    if let Some(doc) = &a.drawing {
                        let bounds = yy_draw::Rect {
                            x: a.origin.x,
                            y: a.origin.y,
                            w: r.right.max(1) as f64 / a.zoom,
                            h: r.bottom.max(1) as f64 / a.zoom,
                        };
                        let options = yy_draw::render::Export {
                            bounds,
                            background: Some(0xffffff),
                            ..yy_draw::render::Export::document(doc)
                        };
                        let image = if a.er.is_some() && a.plan.is_none() {
                            a.renderer.raster_flat_icons(doc, &options, a.zoom)
                        } else {
                            a.renderer.raster(doc, &options, a.zoom)
                        };
                        if let Ok(image) = image {
                            let mut bgra = image.data().to_vec();
                            for p in bgra.as_chunks_mut::<4>().0 {
                                p.swap(0, 2);
                            }
                            let info = BITMAPINFO {
                                bmiHeader: BITMAPINFOHEADER {
                                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                                    biWidth: image.width() as i32,
                                    biHeight: -(image.height() as i32),
                                    biPlanes: 1,
                                    biBitCount: 32,
                                    biCompression: BI_RGB.0,
                                    ..Default::default()
                                },
                                ..Default::default()
                            };
                            StretchDIBits(
                                dc,
                                0,
                                0,
                                r.right,
                                r.bottom,
                                0,
                                0,
                                r.right,
                                r.bottom,
                                Some(bgra.as_ptr().cast()),
                                &info,
                                DIB_RGB_COLORS,
                                SRCCOPY,
                            );
                            if a.plan.is_some() {
                                plan_view::overlay(a, dc, &r);
                            } else if a.er.is_some() {
                                er_view::overlay(a, dc);
                            }
                        }
                    } else if let Some(set) = a.results.get(a.result_index) {
                        let scale = a.dpi as f32 / 96.;
                        let width = r.right as f32 / scale;
                        let height = r.bottom as f32 / scale;
                        let h = a.grid.row_h;
                        let n = ((height / h) as usize).saturating_sub(1).min(200);
                        let rows = set.page(a.top, n).unwrap_or_default();
                        let columns = (a.left..set.columns.len())
                            .take_while(|i| ((*i - a.left) as f32) * 180. < width - 55.)
                            .map(|i| (i as u32, 55. + (i - a.left) as f32 * 180., 180.))
                            .collect::<Vec<_>>();
                        let mut cells = vec![
                            columns
                                .iter()
                                .map(|(i, _, _)| Cell {
                                    text: set.columns[*i as usize].name.clone(),
                                    table_head: true,
                                    ..Default::default()
                                })
                                .collect::<Vec<_>>(),
                        ];
                        cells.extend(rows.iter().map(|row| {
                            columns
                                .iter()
                                .map(|(i, _, _)| {
                                    let v = &row[*i as usize];
                                    Cell {
                                        text: v.display(),
                                        align: if matches!(
                                            v,
                                            yy_db_core::Value::Integer(_)
                                                | yy_db_core::Value::Decimal(_)
                                                | yy_db_core::Value::Float(_)
                                        ) {
                                            Align::Right
                                        } else {
                                            Align::Left
                                        },
                                        color: if *v == yy_db_core::Value::Null {
                                            Some((130, 130, 130))
                                        } else {
                                            None
                                        },
                                        ..Default::default()
                                    }
                                })
                                .collect()
                        }));
                        let positions = (0..cells.len())
                            .map(|i| {
                                (
                                    if i == 0 { 0 } else { a.top + i as u64 },
                                    h * (i as f32 + 1.),
                                    h,
                                )
                            })
                            .collect::<Vec<_>>();
                        let labels = positions
                            .iter()
                            .map(|(r, _, _)| (*r, false))
                            .collect::<Vec<_>>();
                        let scene = Scene {
                            cols: &columns,
                            rows: &positions,
                            header_w: 55.,
                            cells: &cells,
                            sel: (a.selection.0, a.selection.1, a.selection.0, a.selection.1),
                            active: a.selection,
                            editing: false,
                            row_labels: &labels,
                            buttons: &[],
                            col_types: &[],
                            marks: &[],
                            point: None,
                            fill: None,
                            handle: false,
                            fill_button: None,
                        };
                        let _ = a.grid.paint(hwnd, r.right as u32, r.bottom as u32, &scene);
                    } else {
                        let old_font = SelectObject(dc, a.font.into());
                        let _ = SetBkMode(dc, TRANSPARENT);
                        let s = wide("SQLを実行すると、ここに結果と実行計画を表示します。");
                        let _ = TextOutW(dc, 16, 16, &s[..s.len() - 1]);
                        SelectObject(dc, old_font);
                    }
                    a.grid_painted = (r.right, r.bottom);
                });
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            with_app(|a| {
                let delta = (w.0 >> 16) as i16 as f64 / 120.;
                if a.drawing.is_some() {
                    if unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0 {
                        a.origin.x -= delta * 100. / a.zoom;
                    } else if unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0 {
                        a.origin.y -= delta * 100. / a.zoom;
                    } else {
                        let mut pt = POINT {
                            x: l.0 as i16 as i32,
                            y: (l.0 >> 16) as i16 as i32,
                        };
                        unsafe {
                            let _ = ScreenToClient(hwnd, &mut pt);
                        }
                        let previous = a.zoom;
                        a.zoom = (a.zoom * 1.12_f64.powf(delta)).clamp(0.1, 4.);
                        a.origin.x += pt.x as f64 / previous - pt.x as f64 / a.zoom;
                        a.origin.y += pt.y as f64 / previous - pt.y as f64 / a.zoom;
                    }
                } else if unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0 {
                    a.left = a.left.saturating_add_signed(-(delta as isize)).min(
                        a.results
                            .get(a.result_index)
                            .map(|s| s.columns.len().saturating_sub(1))
                            .unwrap_or(0),
                    );
                } else {
                    a.top = a.top.saturating_add_signed(-(delta as i64) * 3).min(
                        a.results
                            .get(a.result_index)
                            .map(|s| s.row_count.saturating_sub(1))
                            .unwrap_or(0),
                    );
                }
                invalidate(hwnd);
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

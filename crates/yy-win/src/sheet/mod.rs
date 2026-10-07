//! スプレッドシート（yysheet。15 章 12）。
//!
//! フレーム（メニュー・数式バー・シートのタブ・ステータスバー）と格子（独自描画）。格子は見える範囲の
//! セルだけを読んで描く。セルの編集は格子の上に重ねた EDIT コントロールで行う（IME はそのまま使える）。
//! ファイルを開く・CSV の取り込み・保存・書き出しはバックグラウンドで行い、進みをステータスバーに出す
//! （Esc で中止）。
//!
//! 状態 [`App`] は UI スレッドのスレッドローカルに置く。ダイアログ・待ち（`remote::wait`）の間は
//! 状態を借りたままにしない。

mod bulkui;
mod entry;
mod fillhandle;
mod filter;
mod fixedui;
mod format;
mod multiui;
mod paint;
mod view;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, DeleteObject, EndPaint, HFONT, HGDIOBJ, InvalidateRect, PAINTSTRUCT,
    ScreenToClient, UpdateWindow,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    DragAcceptFiles, DragFinish, DragQueryFileW, FileOpenDialog, FileSaveDialog, HDROP,
    IFileOpenDialog, IFileSaveDialog, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, Result, w};
use yy_config::Config;
use yy_numfmt::{DateSystem, FmtValue, Parsed};
use yy_sheet::csv::{CsvOptions, ExportOptions};
use yy_sheet::{Context as SheetCtx, Document, Value, Workbook, yys};

use crate::util::{Context, error_box, info_box, wide};
use crate::{default_proc, hiword, loword};
use paint::{Align, ButtonState, Cell, GridPainter, Scene};
use yy_sheet::style::HAlign;

/// 0xRRGGBB → (r, g, b)。
fn rgb(c: u32) -> (u8, u8, u8) {
    ((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

const FRAME_CLASS: PCWSTR = w!("YYSheetFrame");
const GRID_CLASS: PCWSTR = w!("YYSheetGrid");

const ID_NEW: u16 = 1;
const ID_OPEN: u16 = 2;
const ID_SAVE: u16 = 3;
const ID_SAVE_AS: u16 = 4;
const ID_EXPORT_CSV: u16 = 5;
const ID_EXIT: u16 = 6;
const ID_OPEN_FIXED: u16 = 7;
const ID_EXPORT_FIXED: u16 = 8;
const ID_OPEN_MULTI: u16 = 9;
const ID_MULTI_LAYOUT: u16 = 27;
const ID_ROW_LAYOUT: u16 = 28;
const ID_FIXED_LAYOUT: u16 = 26;
const ID_UNDO: u16 = 10;
const ID_REDO: u16 = 11;
const ID_CUT: u16 = 12;
const ID_COPY: u16 = 13;
const ID_PASTE: u16 = 14;
const ID_DELETE: u16 = 15;
const ID_SELECT_ALL: u16 = 16;
const ID_FILL_DOWN: u16 = 17;
const ID_FIND: u16 = 18;
const ID_FIND_NEXT: u16 = 19;
const ID_REPLACE: u16 = 25;
const ID_DEDUP: u16 = 58;
const ID_TO_NUMBER: u16 = 59;
const ID_TO_TEXT: u16 = 66;
const ID_INSERT_ROWS: u16 = 20;
const ID_DELETE_ROWS: u16 = 21;
const ID_INSERT_COLS: u16 = 22;
const ID_DELETE_COLS: u16 = 23;
const ID_ADD_SHEET: u16 = 24;
const ID_ZOOM_IN: u16 = 30;
const ID_ZOOM_OUT: u16 = 31;
const ID_ZOOM_RESET: u16 = 32;
const ID_SORT_ASC: u16 = 50;
const ID_SORT_DESC: u16 = 51;
const ID_SORT: u16 = 52;
const ID_COMMIT_SORT: u16 = 53;
const ID_FILTER: u16 = 54;
const ID_STAGES: u16 = 55;
const ID_REAPPLY: u16 = 56;
const ID_CLEAR_VIEW: u16 = 57;
const ID_FORMAT_CELLS: u16 = 60;
const ID_BOLD: u16 = 61;
const ID_ITALIC: u16 = 62;
const ID_FILL: u16 = 63;
const ID_FONT_COLOR: u16 = 64;
const ID_CLEAR_FORMAT: u16 = 65;
/// 罫線（なし・格子・外枠・上・下・左・右の順）
const ID_BORDER_BASE: u16 = 70;
/// 表示形式（`format::PRESETS` の順）
const ID_NUMFMT_BASE: u16 = 80;
const ID_MEMORY: u16 = 40;
const ID_ABOUT: u16 = 41;
const ID_HELP: u16 = 42;
const ID_HELP_COBOL: u16 = 43;
const ID_HELP_FUNCS: u16 = 44;

/// STATIC の文字を上下の中央に置く
const SS_CENTERIMAGE: u32 = 0x200;
/// マウスのメッセージの wParam（Shift・Ctrl）
const MK_SHIFT: usize = 0x4;
const MK_CONTROL: usize = 0x8;

/// 式の再計算をバックグラウンドで始める（編集のあとにフレームへ送る）
const WM_APP_RECALC: u32 = WM_APP + 40;
/// 式の入力の補助（候補・参照の枠）を出し直す
const WM_APP_ENTRY: u32 = WM_APP + 41;

const IDC_FORMULA: u16 = 100;
const IDC_TABS: u16 = 101;

/// 選択範囲の集計を同期で計算するセルの数の上限。
const STATS_LIMIT: u64 = 200_000;
/// コピー・貼り付け・消去を一度に行うセルの数の上限。
const CLIP_LIMIT: u64 = 2_000_000;

/// 開いている文書の元。
#[derive(Clone, Debug)]
enum Origin {
    New,
    Yys,
    Csv(CsvOptions),
    /// 固定長ファイル（設定はシートが持つ）
    Fixed,
}

/// セルの編集。
struct Editor {
    hwnd: HWND,
    font: HFONT,
    cell: (u64, u32),
    /// 文字の入力で始めた（矢印で確定して移動する）
    enter_mode: bool,
}

/// マウスでの操作。
#[derive(Clone, Copy, Debug)]
enum Drag {
    Select,
    /// 式に入れる参照を選んでいる
    Point,
    /// フィルハンドル（広げる先・右ボタン）
    Fill(paint::Range4, bool),
    /// 列の幅（列・始めの x〔DIP〕・始めの幅〔DIP〕）
    ColWidth(u32, f32, f32),
}

struct App {
    frame: HWND,
    grid: HWND,
    formula: HWND,
    name_box: HWND,
    tabs: HWND,
    status: HWND,
    ui_font: HFONT,
    config: Config,
    ctx: Arc<SheetCtx>,
    doc: Document,
    origin: Origin,
    sheet: usize,
    top: u64,
    left: u32,
    cur: (u64, u32),
    anchor: (u64, u32),
    painter: GridPainter,
    editor: Option<Editor>,
    drag: Option<Drag>,
    /// 最後に描いた配置（列・行・行番号の幅）
    cols: Vec<(u32, f32, f32)>,
    rows: Vec<(u64, f32)>,
    header_w: f32,
    size_px: (i32, i32),
    /// 列全体・行全体を選んでいる（列見出し・行番号のクリック、すべて選択）
    whole: (bool, bool),
    /// 前の検索・置換
    last_find: Option<yy_sheet::bulk::Replace>,
    /// 式に入れている参照
    point: Option<entry::Point>,
    /// 関数の候補・引数の書き方の小窓
    assist: entry::Assist,
    /// 編集中の式の参照の枠（範囲・色）
    marks: Vec<(paint::Range4, (u8, u8, u8))>,
    /// 別のシートの参照を選んでいる間の、元のシートの表示
    home: Option<entry::Home>,
    /// 直前のフィル（オートフィル オプションのボタン）
    last_fill: Option<fillhandle::LastFill>,
    /// 最後に使った固定長ファイルの設定（ダイアログの初期値）
    fixed_last: Option<yy_sheet::fixed::FixedSpec>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

thread_local! {
    /// ステータスバー（状態を借りている間にも書けるよう、状態とは別に持つ）
    static STATUS: std::cell::Cell<HWND> = const { std::cell::Cell::new(HWND(std::ptr::null_mut())) };
}

fn set_status(text: &str) {
    let h = STATUS.with(|s| s.get());
    if !h.is_invalid() {
        let w = HSTRING::from(text);
        unsafe {
            let _ = SendMessageW(
                h,
                SB_SETTEXTW,
                Some(WPARAM(0)),
                Some(LPARAM(w.as_ptr() as isize)),
            );
        }
    }
}

/// yysheet を起動する。
pub fn run_sheet(initial: Option<PathBuf>) -> Result<()> {
    crate::util::set_app_name("yysheet");
    let r = run_inner(initial);
    if let Err(e) = &r {
        error_box(
            HWND::default(),
            &format!("起動できませんでした。\n{}", crate::util::describe_error(e)),
        );
    }
    r
}

fn run_inner(initial: Option<PathBuf>) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .context("CoInitializeEx")?;
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES | ICC_TAB_CLASSES | ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
    }
    std::thread::spawn(yy_io::remove_stale_temps);
    if let Ok(m) = unsafe { GetModuleHandleW(None) } {
        crate::help::use_sheet_help(m.into());
    }
    let frame = create()?;
    if let Some(p) = initial {
        open_path(&p);
    }
    let _ = frame;
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if (msg.message == WM_KEYDOWN || msg.message == WM_SYSKEYDOWN) && key_hook(&msg) {
                entry::after_dispatch(&msg);
                continue;
            }
            entry::before_dispatch(&msg);
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            entry::after_dispatch(&msg);
        }
    }
    APP.with(|a| a.borrow_mut().take());
    Ok(())
}

fn create_menu() -> Result<HMENU> {
    unsafe {
        let bar = CreateMenu()?;
        let add = |m: HMENU, id: u16, text: &str| {
            let _ = AppendMenuW(m, MF_STRING, id as usize, &HSTRING::from(text));
        };
        let sep = |m: HMENU| {
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        };
        let file = CreatePopupMenu()?;
        add(file, ID_NEW, "新規(&N)\tCtrl+N");
        add(file, ID_OPEN, "開く(&O)...\tCtrl+O");
        add(file, ID_OPEN_FIXED, "固定長ファイルを開く(&F)...");
        add(
            file,
            ID_OPEN_MULTI,
            "固定長ファイルを開く（マルチレイアウト）(&U)...",
        );
        sep(file);
        add(file, ID_SAVE, "上書き保存(&S)\tCtrl+S");
        add(file, ID_SAVE_AS, "名前を付けて保存(&A)...\tCtrl+Shift+S");
        add(file, ID_EXPORT_CSV, "CSV に書き出し(&E)...");
        add(file, ID_EXPORT_FIXED, "固定長ファイルに書き出し(&L)...");
        sep(file);
        add(file, ID_EXIT, "終了(&X)");
        let edit = CreatePopupMenu()?;
        add(edit, ID_UNDO, "元に戻す(&U)\tCtrl+Z");
        add(edit, ID_REDO, "やり直し(&R)\tCtrl+Y");
        sep(edit);
        add(edit, ID_CUT, "切り取り(&T)\tCtrl+X");
        add(edit, ID_COPY, "コピー(&C)\tCtrl+C");
        add(edit, ID_PASTE, "貼り付け(&P)\tCtrl+V");
        add(edit, ID_DELETE, "内容を消す(&D)\tDelete");
        add(edit, ID_FILL_DOWN, "下へコピー(&W)\tCtrl+D");
        sep(edit);
        add(edit, ID_FIND, "検索(&F)...\tCtrl+F");
        add(edit, ID_FIND_NEXT, "次を検索(&N)\tF3");
        add(edit, ID_REPLACE, "置換(&H)...\tCtrl+H");
        sep(edit);
        add(edit, ID_SELECT_ALL, "すべて選択(&A)\tCtrl+A");
        let insert = CreatePopupMenu()?;
        add(insert, ID_INSERT_ROWS, "行を挿入(&R)\tCtrl++");
        add(insert, ID_INSERT_COLS, "列を挿入(&C)");
        add(insert, ID_DELETE_ROWS, "行を削除(&D)\tCtrl+-");
        add(insert, ID_DELETE_COLS, "列を削除(&L)");
        sep(insert);
        add(insert, ID_ADD_SHEET, "シートを追加(&S)");
        let view = CreatePopupMenu()?;
        add(view, ID_ZOOM_IN, "拡大(&I)\tCtrl++（ホイール）");
        add(view, ID_ZOOM_OUT, "縮小(&O)\tCtrl+-（ホイール）");
        add(view, ID_ZOOM_RESET, "100%(&R)\tCtrl+0");
        let fmt = CreatePopupMenu()?;
        add(fmt, ID_FORMAT_CELLS, "セルの書式設定(&E)...\tCtrl+1");
        sep(fmt);
        let numfmt = CreatePopupMenu()?;
        for (i, (code, name)) in format::PRESETS.iter().enumerate() {
            add(
                numfmt,
                ID_NUMFMT_BASE + i as u16,
                &format!("{name}\t{code}"),
            );
        }
        AppendMenuW(
            fmt,
            MF_POPUP,
            numfmt.0 as usize,
            &HSTRING::from("表示形式(&N)"),
        )?;
        add(fmt, ID_BOLD, "太字(&B)\tCtrl+B");
        add(fmt, ID_ITALIC, "斜体(&I)\tCtrl+I");
        add(fmt, ID_FILL, "塗りつぶしの色(&F)...");
        add(fmt, ID_FONT_COLOR, "文字の色(&C)...");
        let border = CreatePopupMenu()?;
        for (i, name) in [
            "罫線なし(&N)",
            "格子(&A)",
            "外枠(&O)",
            "上罫線(&T)",
            "下罫線(&B)",
            "左罫線(&L)",
            "右罫線(&R)",
        ]
        .iter()
        .enumerate()
        {
            add(border, ID_BORDER_BASE + i as u16, name);
        }
        AppendMenuW(fmt, MF_POPUP, border.0 as usize, &HSTRING::from("罫線(&R)"))?;
        sep(fmt);
        add(fmt, ID_CLEAR_FORMAT, "書式のクリア(&L)");
        let data = CreatePopupMenu()?;
        add(data, ID_SORT_ASC, "昇順に並べ替え(&A)");
        add(data, ID_SORT_DESC, "降順に並べ替え(&D)");
        add(data, ID_SORT, "並べ替え(&S)...");
        add(data, ID_COMMIT_SORT, "並べ替えを確定(&C)");
        sep(data);
        add(data, ID_FILTER, "列の絞り込み(&F)...\tCtrl+Shift+L");
        add(data, ID_STAGES, "絞り込みの段階(&G)...");
        add(data, ID_REAPPLY, "再適用(&R)\tCtrl+Alt+L");
        add(data, ID_CLEAR_VIEW, "絞り込み・並べ替えを解除(&X)");
        sep(data);
        add(data, ID_DEDUP, "重複の削除(&U)...");
        add(data, ID_FIXED_LAYOUT, "固定長のレイアウト(&Y)...");
        add(data, ID_MULTI_LAYOUT, "マルチレイアウトの設定(&M)...");
        add(data, ID_ROW_LAYOUT, "行のレイアウトを指定(&W)...");
        add(data, ID_TO_NUMBER, "列を数値に変換(&V)");
        add(data, ID_TO_TEXT, "列を文字列に変換(&T)");
        let help = CreatePopupMenu()?;
        add(help, ID_HELP, "yysheet ヘルプ(&H)\tF1");
        add(help, ID_HELP_FUNCS, "関数の一覧(&F)");
        add(help, ID_HELP_COBOL, "COBOL の型の一覧(&C)");
        sep(help);
        add(help, ID_MEMORY, "メモリの使用状況(&M)");
        add(help, ID_ABOUT, "yysheet について(&A)");
        for (m, t) in [
            (file, "ファイル(&F)"),
            (edit, "編集(&E)"),
            (insert, "挿入・削除(&I)"),
            (view, "表示(&V)"),
            (fmt, "書式(&O)"),
            (data, "データ(&D)"),
            (help, "ヘルプ(&H)"),
        ] {
            AppendMenuW(bar, MF_POPUP, m.0 as usize, &HSTRING::from(t))?;
        }
        Ok(bar)
    }
}

fn create() -> Result<HWND> {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let (icon, icon_small) = crate::app_icons(instance);
        let arrow = LoadCursorW(None, IDC_ARROW)?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(frame_proc),
            hInstance: instance,
            hCursor: arrow,
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(
                (windows::Win32::Graphics::Gdi::COLOR_BTNFACE.0 + 1) as usize as *mut _,
            ),
            hIcon: icon,
            hIconSm: icon_small,
            lpszClassName: FRAME_CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let grid_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(grid_proc),
            hInstance: instance,
            hCursor: arrow,
            lpszClassName: GRID_CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&grid_class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let assist_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(entry::assist_proc),
            hInstance: instance,
            hCursor: arrow,
            lpszClassName: entry::ASSIST_CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&assist_class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let (config, config_error) = Config::load();
        crate::font::register_gdi();
        let frame = CreateWindowExW(
            WS_EX_ACCEPTFILES,
            FRAME_CLASS,
            w!("yysheet"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1200,
            800,
            None,
            Some(create_menu()?),
            Some(instance),
            None,
        )
        .context("CreateWindowExW(frame)")?;
        let dpi = GetDpiForWindow(frame).max(96);
        let ui_font = crate::util::ui_font(dpi);
        let child =
            |class: PCWSTR, text: PCWSTR, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, id: u16| {
                let h = CreateWindowExW(
                    ex,
                    class,
                    text,
                    WS_CHILD | WS_VISIBLE | style,
                    0,
                    0,
                    10,
                    10,
                    Some(frame),
                    Some(HMENU(id as usize as *mut _)),
                    Some(instance),
                    None,
                )
                .unwrap_or_default();
                SendMessageW(
                    h,
                    WM_SETFONT,
                    Some(WPARAM(ui_font.0 as usize)),
                    Some(LPARAM(1)),
                );
                h
            };
        let name_box = child(
            w!("STATIC"),
            w!("A1"),
            WINDOW_STYLE(SS_CENTERIMAGE),
            WS_EX_CLIENTEDGE,
            0,
        );
        let formula = child(
            w!("EDIT"),
            w!(""),
            WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            WS_EX_CLIENTEDGE,
            IDC_FORMULA,
        );
        let grid = child(
            GRID_CLASS,
            w!(""),
            WS_VSCROLL | WS_HSCROLL | WS_TABSTOP,
            WINDOW_EX_STYLE(0),
            0,
        );
        let tabs = child(
            WC_TABCONTROLW,
            w!(""),
            WINDOW_STYLE(TCS_BOTTOM | TCS_FOCUSNEVER),
            WINDOW_EX_STYLE(0),
            IDC_TABS,
        );
        let status = child(
            STATUSCLASSNAMEW,
            w!(""),
            WINDOW_STYLE(SBARS_SIZEGRIP),
            WINDOW_EX_STYLE(0),
            0,
        );
        let limit = (config.sheet.memory_limit_gb.max(0.5) * (1u64 << 30) as f64) as u64;
        let work_dir = if config.sheet.work_dir.trim().is_empty() {
            std::env::temp_dir()
        } else {
            PathBuf::from(config.sheet.work_dir.trim())
        };
        let ctx = SheetCtx::new(limit, work_dir);
        let painter = GridPainter::new(&config.sheet.font_family, config.sheet.font_size, dpi)?;
        crate::remote::install(
            crate::remote::RemoteState::new(None, config.remote.clone()),
            crate::remote::Host {
                frame,
                status: set_status,
            },
        );
        DragAcceptFiles(frame, true);
        STATUS.with(|s| s.set(status));
        let app = App {
            frame,
            grid,
            formula,
            name_box,
            tabs,
            status,
            ui_font,
            config,
            doc: {
                let mut d = Document::new(ctx.clone());
                d.defer_recalc = true;
                d
            },
            ctx,
            origin: Origin::New,
            sheet: 0,
            top: 0,
            left: 0,
            cur: (0, 0),
            anchor: (0, 0),
            painter,
            editor: None,
            drag: None,
            cols: Vec::new(),
            rows: Vec::new(),
            header_w: 40.0,
            size_px: (0, 0),
            whole: (false, false),
            last_find: None,
            point: None,
            assist: entry::Assist::create(frame, instance, ui_font),
            marks: Vec::new(),
            home: None,
            last_fill: None,
            fixed_last: None,
        };
        APP.with(|a| *a.borrow_mut() = Some(app));
        with(|a| {
            a.refresh_tabs();
            a.layout();
            a.update_title();
            a.sync_formula();
        });
        let _ = ShowWindow(frame, SW_SHOWDEFAULT);
        let _ = UpdateWindow(frame);
        let _ = SetFocus(Some(grid));
        set_status("準備完了");
        if let Some(e) = config_error {
            error_box(
                frame,
                &format!("設定を読めませんでした（既定の設定で起動します）。\n{e}"),
            );
        }
        Ok(frame)
    }
}

// ---- 表示 ----------------------------------------------------------------------------

/// セルの表示（表示形式・寄せ方・色）。
fn display(
    v: &Value,
    format: Option<&str>,
    width_chars: f32,
    sys: DateSystem,
) -> (String, Align, Option<(u8, u8, u8)>) {
    match v {
        Value::Empty => (String::new(), Align::Left, None),
        // CBL.LOW-VALUE()・CBL.HIGH-VALUE()（固定長の項目のすべてのバイト）
        Value::Text(s) if yy_sheet::value::figurative_label(s).is_some() => (
            yy_sheet::value::figurative_label(s)
                .unwrap_or_default()
                .into(),
            Align::Center,
            Some((31, 111, 208)),
        ),
        Value::Text(s) => match format {
            Some(f) => {
                let r = yy_numfmt::format::parsed(f).format(FmtValue::Text(s), sys);
                (r.text, Align::Left, r.color.map(|c| c.rgb()))
            }
            None => (s.to_string(), Align::Left, None),
        },
        Value::Bool(b) => (
            (if *b { "TRUE" } else { "FALSE" }).into(),
            Align::Center,
            None,
        ),
        Value::Error(e) => (e.text().into(), Align::Center, None),
        Value::Number(n) => {
            let width = width_chars.floor().max(1.0) as usize;
            match format {
                Some(f) if !yy_numfmt::format::parsed(f).is_general() => {
                    let r = yy_numfmt::format::parsed(f).format(FmtValue::Number(*n), sys);
                    let text = if r.overflow || r.text.chars().count() > width + 2 {
                        "#".repeat(width)
                    } else {
                        r.text
                    };
                    (text, Align::Right, r.color.map(|c| c.rgb()))
                }
                _ => (
                    yy_numfmt::general_fit(*n, width).unwrap_or_else(|| "#".repeat(width)),
                    Align::Right,
                    None,
                ),
            }
        }
    }
}

/// 式の入力か（`=` で始まり、続きがある）。
fn is_formula(text: &str) -> bool {
    text.len() > 1 && text.starts_with('=')
}

/// 入力された文字列をセルの値にする。
fn parse_entry(text: &str, sys: DateSystem) -> Value {
    if text.is_empty() {
        return Value::Empty;
    }
    if let Some(rest) = text.strip_prefix('\'') {
        return Value::text(rest);
    }
    if let Some(e) = yy_sheet::CellError::parse(text) {
        return Value::Error(e);
    }
    match yy_numfmt::parse_input(text, sys) {
        Parsed::Number(n, _) => Value::Number(n),
        Parsed::Bool(b) => Value::Bool(b),
        Parsed::Text => Value::text(text),
    }
}

/// レイアウト未確定の行の背景と、エラーの文字の色。
const UNDETERMINED_FILL: (u8, u8, u8) = (255, 199, 206);
const UNDETERMINED_FG: (u8, u8, u8) = (156, 0, 6);

/// マルチレイアウトのレイアウトの列のセル（見出し行は除く）か。
fn is_layout_cell(sheet: &yy_sheet::Sheet, r: u64, c: u32) -> bool {
    yy_sheet::fixed::layout_column(sheet) == Some(c)
        && !matches!(sheet.place(r, c), yy_sheet::Place::Header(_))
}

/// 固定長の項目の型に合わない入力の知らせ。
fn type_mismatch(f: &yy_cobol::Field, why: &str) -> String {
    format!(
        "入力が項目の型に合いません。\n項目 {}（{}・{} バイト）: {why}",
        f.name, f.describe, f.len
    )
}

/// 編集のときに見せる値（数式バー・編集の初期値）。
fn edit_text(v: &Value, format: Option<&str>, sys: DateSystem) -> String {
    match v {
        Value::Number(n) => match format {
            Some(f) if yy_numfmt::format::parsed(f).is_date() => {
                let l = f.to_ascii_lowercase();
                let fmt = if l.contains('y') || l.contains('d') {
                    if l.contains('h') {
                        "yyyy/m/d h:mm:ss"
                    } else {
                        "yyyy/m/d"
                    }
                } else {
                    "h:mm:ss"
                };
                yy_numfmt::format_number(fmt, *n, sys)
            }
            _ => yy_numfmt::general(*n),
        },
        Value::Text(s) if yy_numfmt::parse_input(s, sys) != Parsed::Text => format!("'{s}"),
        _ => v.general_text(),
    }
}

impl App {
    fn sheet(&self) -> &yy_sheet::Sheet {
        &self.doc.book.sheets[self.sheet]
    }

    fn sys(&self) -> DateSystem {
        self.doc.book.date_system
    }

    fn col_chars(&self, col: u32) -> f32 {
        self.sheet()
            .col_widths
            .get(&col)
            .copied()
            .unwrap_or(self.config.sheet.column_width)
    }

    fn col_dip(&self, col: u32) -> f32 {
        self.painter.col_px(self.col_chars(col))
    }

    /// セルの表示形式（セルの書式、なければ表の列の既定の形式）。
    fn format_of(&self, row: u64, col: u32) -> Option<Arc<str>> {
        self.sheet().format_at(row, col)
    }

    /// 見える列と行を求める。
    fn compute_layout(&mut self) {
        let (wpx, hpx) = self.size_px;
        let w = self.painter.to_dip(wpx);
        let h = self.painter.to_dip(hpx);
        let rh = self.painter.row_h;
        let nrows = ((h - rh) / rh).ceil().max(1.0) as u64 + 1;
        let last_row = self.top + nrows;
        let digits = (last_row + 1).to_string().len().max(3) as f32;
        self.header_w = (digits * self.painter.char_w + 14.0).round();
        self.rows = (0..nrows).map(|i| (self.top + i, i as f32 * rh)).collect();
        let mut cols = Vec::new();
        let mut x = 0.0;
        let mut c = self.left;
        while x < w - self.header_w && c < 16_384 {
            let cw = self.col_dip(c);
            cols.push((c, x, cw));
            x += cw;
            c += 1;
        }
        self.cols = cols;
    }

    fn visible_rows(&self) -> u64 {
        let h = self.painter.to_dip(self.size_px.1);
        ((h - self.painter.row_h) / self.painter.row_h)
            .floor()
            .max(1.0) as u64
    }

    fn visible_cols(&self) -> u32 {
        let w = self.painter.to_dip(self.size_px.0) - self.header_w;
        let mut x = 0.0;
        let mut n = 0;
        let mut c = self.left;
        while x + self.col_dip(c) <= w && c < 16_384 {
            x += self.col_dip(c);
            n += 1;
            c += 1;
        }
        n.max(1)
    }

    fn paint(&mut self) {
        self.sync_grid_size();
        self.compute_layout();
        let sys = self.sys();
        // マルチレイアウト: レイアウト未確定の行（背景を赤く、レイアウトの列にエラーを出す）
        let multi_lc = yy_sheet::fixed::layout_column(self.sheet());
        let head = self.sheet().table.header as u64;
        let last_row = multi_lc.map_or(0, |_| self.sheet().extent().0);
        let mut cells = Vec::with_capacity(self.rows.len());
        for &(row, _) in &self.rows {
            let undetermined = match multi_lc {
                Some(_) if row >= head && row < last_row => {
                    match yy_sheet::fixed::row_layout(
                        &self.ctx,
                        self.sheet(),
                        self.sheet().source_row(row),
                    ) {
                        yy_sheet::fixed::RowLayout::Undetermined(n) => Some(n),
                        _ => None,
                    }
                }
                _ => None,
            };
            let mut line = Vec::with_capacity(self.cols.len());
            for &(col, _, _) in &self.cols {
                let v = self
                    .sheet()
                    .get(&self.ctx, row, col)
                    .unwrap_or(Value::Error(yy_sheet::CellError::Value));
                let st = self.sheet().style_at(row, col);
                let fmt = st.num_fmt.clone().or_else(|| self.format_of(row, col));
                let chars = self.col_chars(col);
                let (text, align, color) = display(&v, fmt.as_deref(), chars, sys);
                let table_head = matches!(self.sheet().place(row, col), yy_sheet::Place::Header(_));
                let align = match st.align {
                    Some(HAlign::Left) => Align::Left,
                    Some(HAlign::Center) => Align::Center,
                    Some(HAlign::Right) => Align::Right,
                    _ => align,
                };
                let mut borders = [None; 4];
                for (side, b) in borders.iter_mut().enumerate() {
                    *b = st.line(side).map(|l| (l.style, rgb(l.color)));
                }
                let mut cell = Cell {
                    text,
                    align,
                    // 表示形式の色（[赤] など）が書式の文字の色より優先（Excel と同じ）
                    color: color.or(st.color_rgb().map(rgb)),
                    table_head,
                    fill: st.fill_rgb().map(rgb),
                    bold: st.bold.unwrap_or(false),
                    italic: st.italic.unwrap_or(false),
                    borders,
                };
                if let Some(name) = &undetermined {
                    cell.fill = Some(UNDETERMINED_FILL);
                    if Some(col) == multi_lc {
                        cell.text = match name {
                            Some(n) => format!("#レイアウト未確定（{n}）"),
                            None => "#レイアウト未確定".into(),
                        };
                        cell.color = Some(UNDETERMINED_FG);
                        cell.align = Align::Left;
                        cell.bold = true;
                    }
                }
                line.push(cell);
            }
            cells.push(line);
        }
        let (t, l, b, r) = self.selection();
        let sh = self.sheet();
        let in_view = sh.view.rows.is_some();
        let row_labels: Vec<(u64, bool)> = self
            .rows
            .iter()
            .map(|&(row, _)| {
                let data = matches!(sh.place(row, 0), yy_sheet::Place::Data(..));
                (sh.source_row(row) + 1, in_view && data)
            })
            .collect();
        let buttons = self.header_buttons();
        // 固定長: 列見出しに項目の型（マルチレイアウトはアクティブなセルの行のレイアウトの項目名と型）
        let active_src = self.sheet().source_row(self.cur.0);
        let col_types: Vec<(u32, String)> = self
            .cols
            .iter()
            .filter_map(|&(c, _, _)| {
                if multi_lc.is_some() {
                    yy_sheet::fixed::field_for(&self.ctx, self.sheet(), active_src, c)
                        .map(|(_, f)| (c, format!("{} {}", f.name, f.describe)))
                } else {
                    yy_sheet::fixed::column_field(self.sheet(), c)
                        .map(|(_, f)| (c, f.describe.clone()))
                }
            })
            .collect();
        let scene = Scene {
            col_types: &col_types,
            row_labels: &row_labels,
            buttons: &buttons,
            cols: &self.cols,
            rows: &self.rows,
            header_w: self.header_w,
            cells: &cells,
            sel: (t, l, b, r),
            active: self.cur,
            editing: self.editor.is_some() && self.home.is_none(),
            marks: &self.marks,
            point: self
                .point
                .as_ref()
                .filter(|p| p.sheet == self.sheet)
                .and_then(|p| p.range()),
            fill: match self.drag {
                Some(Drag::Fill(t, _)) => Some(t),
                _ => None,
            },
            handle: self.handle_shown(),
            fill_button: self.fill_button_dip(),
        };
        let (w, h) = self.size_px;
        if let Err(e) = self.painter.paint(self.grid, w as u32, h as u32, &scene) {
            set_status(&format!(
                "描画できません: {}",
                crate::util::describe_error(&e)
            ));
        }
    }

    fn selection(&self) -> (u64, u32, u64, u32) {
        (
            self.cur.0.min(self.anchor.0),
            self.cur.1.min(self.anchor.1),
            self.cur.0.max(self.anchor.0),
            self.cur.1.max(self.anchor.1),
        )
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.grid), None, false);
        }
    }

    fn update_title(&self) {
        let name = self
            .doc
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "無題".into());
        let star = if self.doc.dirty { "*" } else { "" };
        let title = format!("{star}{name} - yysheet");
        unsafe {
            let _ = SetWindowTextW(self.frame, &HSTRING::from(title));
        }
    }

    fn update_scrollbars(&self) {
        let (rows, cols) = self.sheet().extent();
        let vmax = (rows.max(self.top + self.visible_rows()) + 1000).min(i32::MAX as u64) as i32;
        let hmax = (cols.max(self.left + self.visible_cols()) + 30).min(16_384) as i32;
        unsafe {
            let si = SCROLLINFO {
                cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
                nMin: 0,
                nMax: vmax,
                nPage: self.visible_rows() as u32,
                nPos: self.top.min(i32::MAX as u64) as i32,
                nTrackPos: 0,
            };
            SetScrollInfo(self.grid, SB_VERT, &si, true);
            let si = SCROLLINFO {
                cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
                nMin: 0,
                nMax: hmax,
                nPage: self.visible_cols(),
                nPos: self.left as i32,
                nTrackPos: 0,
            };
            SetScrollInfo(self.grid, SB_HORZ, &si, true);
        }
    }

    /// アクティブなセルの名前と値を数式バーに出し、ステータスに選択範囲の集計を出す。
    fn sync_formula(&self) {
        let (r, c) = self.cur;
        let (t, l, b, rr) = self.selection();
        let name = if (t, l) == (b, rr) {
            format!(
                "{}{}",
                yy_sheet::col_name(c),
                self.sheet().source_row(r) + 1
            )
        } else {
            format!("{}R × {}C", b - t + 1, rr - l + 1)
        };
        let text = self.cell_edit_text(r, c);
        unsafe {
            let _ = SetWindowTextW(self.name_box, &HSTRING::from(name));
            if GetFocus() != self.formula {
                let _ = SetWindowTextW(self.formula, &HSTRING::from(text));
            }
        }
        // 選択範囲の集計（Excel と同じ: 個数・数値の合計・平均）
        let cells = (b - t + 1) * (rr - l + 1) as u64;
        if cells > 1 {
            let msg = if cells <= STATS_LIMIT {
                let (mut count, mut n, mut sum) = (0u64, 0u64, 0.0f64);
                for row in t..=b {
                    for col in l..=rr {
                        match self.sheet().get(&self.ctx, row, col).unwrap_or_default() {
                            Value::Empty => {}
                            Value::Number(x) => {
                                count += 1;
                                n += 1;
                                sum += x;
                            }
                            _ => count += 1,
                        }
                    }
                }
                if n > 0 {
                    format!(
                        "データの個数: {count}　合計: {}　平均: {}",
                        yy_numfmt::general(sum),
                        yy_numfmt::general(sum / n as f64)
                    )
                } else {
                    format!("データの個数: {count}")
                }
            } else {
                // 大きな範囲は表の列をチャンクごとに並列に集計する
                match self.sheet().totals(&self.ctx, t, l, b, rr) {
                    Ok(Some(x)) if x.numbers > 0 => format!(
                        "データの個数: {}　合計: {}　平均: {}",
                        crate::util::group_digits(x.count),
                        yy_numfmt::general(x.sum),
                        yy_numfmt::general(x.sum / x.numbers as f64)
                    ),
                    Ok(Some(x)) => {
                        format!("データの個数: {}", crate::util::group_digits(x.count))
                    }
                    _ => format!("{} セルを選択", crate::util::group_digits(cells)),
                }
            };
            set_status(&msg);
        }
        self.update_fixed_status();
    }

    /// ステータスバーの右の欄: 固定長の設定（文字コード・レコード長）と、アクティブなセルの列の
    /// 項目（名前・型・位置）。固定長でないシートでは欄を出さない。
    fn update_fixed_status(&self) {
        let sh = self.sheet();
        let text = sh.fixed.as_ref().map(|spec| {
            let src = sh.source_row(self.cur.0);
            let mut t = if spec.is_multi() {
                let row = match yy_sheet::fixed::row_layout(&self.ctx, sh, src) {
                    yy_sheet::fixed::RowLayout::Known(l) => format!(
                        "　行のレイアウト {}（レコード長 {} バイト）",
                        l.name, l.layout.record_len
                    ),
                    yy_sheet::fixed::RowLayout::Undetermined(_) => "　レイアウト未確定の行".into(),
                    yy_sheet::fixed::RowLayout::NotMulti => String::new(),
                };
                format!(
                    "固定長 {}・マルチレイアウト {} 種・1 行 {} バイト{row}",
                    spec.codec.charset.name(),
                    spec.multi.len(),
                    spec.data_len
                )
            } else {
                format!(
                    "固定長 {}・レコード長 {} バイト",
                    spec.codec.charset.name(),
                    spec.layout.record_len
                )
            };
            if let Some((_, f)) = yy_sheet::fixed::field_for(&self.ctx, sh, src, self.cur.1) {
                t.push_str(&format!(
                    "　{}: {}（{}〜{} バイト目）",
                    f.name,
                    f.describe,
                    f.offset + 1,
                    f.offset + f.len
                ));
            }
            t
        });
        unsafe {
            let dpi = GetDpiForWindow(self.frame).max(96) as i32;
            let mut rc = RECT::default();
            let _ = GetClientRect(self.status, &mut rc);
            let parts: Vec<i32> = match text {
                Some(_) => vec![(rc.right - 560 * dpi / 96).max(rc.right / 3), -1],
                None => vec![-1],
            };
            SendMessageW(
                self.status,
                SB_SETPARTS,
                Some(WPARAM(parts.len())),
                Some(LPARAM(parts.as_ptr() as isize)),
            );
            if let Some(t) = text {
                let w = HSTRING::from(t);
                SendMessageW(
                    self.status,
                    SB_SETTEXTW,
                    Some(WPARAM(1)),
                    Some(LPARAM(w.as_ptr() as isize)),
                );
            }
        }
    }

    fn refresh_tabs(&self) {
        unsafe {
            SendMessageW(self.tabs, TCM_DELETEALLITEMS, None, None);
            for (i, s) in self.doc.book.sheets.iter().enumerate() {
                let mut text = wide(&s.name);
                let item = TCITEMW {
                    mask: TCIF_TEXT,
                    pszText: windows::core::PWSTR(text.as_mut_ptr()),
                    ..Default::default()
                };
                SendMessageW(
                    self.tabs,
                    TCM_INSERTITEMW,
                    Some(WPARAM(i)),
                    Some(LPARAM(&item as *const _ as isize)),
                );
            }
            SendMessageW(self.tabs, TCM_SETCURSEL, Some(WPARAM(self.sheet)), None);
        }
    }

    /// 子ウィンドウを並べる。
    fn layout(&mut self) {
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(self.frame, &mut rc);
            SendMessageW(self.status, WM_SIZE, None, None);
            let mut src = RECT::default();
            let _ = GetWindowRect(self.status, &mut src);
            let status_h = src.bottom - src.top;
            let dpi = GetDpiForWindow(self.frame).max(96) as i32;
            let bar_h = 26 * dpi / 96;
            let tabs_h = 26 * dpi / 96;
            let name_w = 90 * dpi / 96;
            let w = rc.right;
            let h = rc.bottom - status_h;
            let _ = MoveWindow(self.name_box, 2, 2, name_w, bar_h - 4, true);
            let _ = MoveWindow(self.formula, name_w + 6, 2, w - name_w - 8, bar_h - 4, true);
            let grid_h = (h - bar_h - tabs_h).max(1);
            let _ = MoveWindow(self.grid, 0, bar_h, w, grid_h, true);
            let _ = MoveWindow(self.tabs, 0, bar_h + grid_h, w, tabs_h, true);
        }
        self.update_fixed_status();
        // MoveWindow は格子の WM_SIZE をその場で送るが、そのとき状態は借りられていて受け取れないので、
        // ここで大きさを読み直す
        self.sync_grid_size();
    }

    /// 格子の大きさ（クライアント領域）を読み直す。変わっていれば描画先と配置を合わせる。
    fn sync_grid_size(&mut self) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.grid, &mut rc);
        }
        let size = (rc.right - rc.left, rc.bottom - rc.top);
        if size != self.size_px {
            self.size_px = size;
            self.painter
                .resize_target(size.0.max(1) as u32, size.1.max(1) as u32);
            self.compute_layout();
            self.update_scrollbars();
            self.invalidate();
        }
    }

    // ---- 移動・選択 --------------------------------------------------------------------

    fn ensure_visible(&mut self) {
        let (r, c) = self.cur;
        let vr = self.visible_rows();
        if r < self.top {
            self.top = r;
        } else if r >= self.top + vr {
            self.top = r + 1 - vr;
        }
        if c < self.left {
            self.left = c;
        } else {
            while c >= self.left + self.visible_cols() && self.left < c {
                self.left += 1;
            }
        }
    }

    fn move_to(&mut self, r: u64, c: u32, extend: bool) {
        self.cur = (r, c.min(16_383));
        if !extend {
            self.anchor = self.cur;
            self.whole = (false, false);
        }
        self.ensure_visible();
        self.update_scrollbars();
        self.sync_formula();
        self.invalidate();
    }

    /// Ctrl+矢印: データの端へ（値のあるセルの並びの端、なければ次の値のあるセル・端）。
    fn edge(&self, dr: i64, dc: i64) -> (u64, u32) {
        let (rows, cols) = self.sheet().extent();
        let (mut r, mut c) = (self.cur.0 as i64, self.cur.1 as i64);
        let max_r = rows.max(1) as i64 - 1;
        let max_c = cols.max(1) as i64 - 1;
        let filled = |r: i64, c: i64| {
            r >= 0
                && c >= 0
                && !self
                    .sheet()
                    .get(&self.ctx, r as u64, c as u32)
                    .unwrap_or_default()
                    .is_empty()
        };
        // 上限（値を順に調べる数）。大きな表では端へ飛ぶ
        let mut budget = 100_000;
        let here = filled(r, c);
        let next = filled(r + dr, c + dc);
        if here && next {
            while filled(r + dr, c + dc) && budget > 0 {
                r += dr;
                c += dc;
                budget -= 1;
            }
        } else {
            loop {
                r += dr;
                c += dc;
                budget -= 1;
                if r < 0 || c < 0 || r > max_r || c > max_c || budget == 0 {
                    break;
                }
                if filled(r, c) {
                    break;
                }
            }
        }
        if budget == 0 && dr > 0 {
            r = max_r;
        }
        if budget == 0 && dc > 0 {
            c = max_c;
        }
        (
            r.clamp(0, max_r.max(self.cur.0 as i64)) as u64,
            c.clamp(0, 16_383) as u32,
        )
    }

    // ---- 編集 ----------------------------------------------------------------------------

    fn cell_rect_px(&self, row: u64, col: u32) -> Option<RECT> {
        let &(_, x, w) = self.cols.iter().find(|c| c.0 == col)?;
        let &(_, y) = self.rows.iter().find(|r| r.0 == row)?;
        let p = &self.painter;
        Some(RECT {
            left: p.to_px(self.header_w + x),
            top: p.to_px(p.row_h + y),
            right: p.to_px(self.header_w + x + w),
            bottom: p.to_px(p.row_h + y + p.row_h),
        })
    }

    fn begin_edit(&mut self, initial: Option<&str>) {
        if self.editor.is_some() {
            return;
        }
        self.ensure_visible();
        self.compute_layout();
        let (r, c) = self.cur;
        let Some(rc) = self.cell_rect_px(r, c) else {
            return;
        };
        let enter_mode = initial.is_some();
        let text = match initial {
            Some(t) => t.to_owned(),
            None => self.cell_edit_text(r, c),
        };
        unsafe {
            let font_h = -((self.painter.size_pt() * self.painter.dpi() / 72.0).round() as i32);
            let face = if self.config.sheet.font_family.is_empty() {
                "Yu Gothic UI".to_owned()
            } else {
                self.config.sheet.font_family.clone()
            };
            let font = CreateFontW(
                font_h,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                windows::Win32::Graphics::Gdi::DEFAULT_CHARSET,
                windows::Win32::Graphics::Gdi::OUT_DEFAULT_PRECIS,
                windows::Win32::Graphics::Gdi::CLIP_DEFAULT_PRECIS,
                windows::Win32::Graphics::Gdi::CLEARTYPE_QUALITY,
                0,
                &HSTRING::from(face),
            );
            let instance: HINSTANCE = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
            let Ok(h) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("EDIT"),
                &HSTRING::from(text.as_str()),
                WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                rc.left,
                rc.top,
                (rc.right - rc.left).max(60),
                rc.bottom - rc.top,
                Some(self.grid),
                None,
                Some(instance),
                None,
            ) else {
                let _ = DeleteObject(HGDIOBJ(font.0));
                return;
            };
            SendMessageW(
                h,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
            let n = text.encode_utf16().count();
            SendMessageW(h, EM_SETSEL, Some(WPARAM(n)), Some(LPARAM(n as isize)));
            let _ = SetFocus(Some(h));
            self.editor = Some(Editor {
                hwnd: h,
                font,
                cell: (r, c),
                enter_mode,
            });
        }
        self.invalidate();
        self.refresh_assist();
    }

    fn close_editor(&mut self) -> Option<String> {
        let ed = self.editor.take()?;
        let text = window_text(ed.hwnd);
        self.entry_reset();
        unsafe {
            let _ = DestroyWindow(ed.hwnd);
            let _ = DeleteObject(HGDIOBJ(ed.font.0));
            let _ = SetFocus(Some(self.grid));
        }
        self.invalidate();
        Some(text)
    }

    /// 数式バーをクリックした: セルの編集を数式バーへ移す（内容は映してある。確定しない）。
    fn move_edit_to_formula_bar(&mut self) {
        let Some(ed) = self.editor.take() else {
            return;
        };
        let text = window_text(ed.hwnd);
        self.point = None;
        unsafe {
            let _ = DestroyWindow(ed.hwnd);
            let _ = DeleteObject(HGDIOBJ(ed.font.0));
            if window_text(self.formula) != text {
                let _ = SetWindowTextW(self.formula, &HSTRING::from(text.as_str()));
            }
        }
        self.invalidate();
    }

    /// 数式バーで入力していれば確定する（格子をクリックしたとき。Excel と同じ）。
    fn commit_formula_bar(&mut self) {
        if unsafe { GetFocus() } != self.formula {
            return;
        }
        let text = window_text(self.formula);
        self.entry_reset();
        let (r, c) = self.cur;
        if text != self.cell_edit_text(r, c) {
            self.set_cell(r, c, &text);
        }
    }

    /// 編集を確定する（`false` なら取り消す）。
    /// 入力が受け付けられなかったら `false`。
    fn end_edit(&mut self, commit: bool) -> bool {
        let cell = self.editor.as_ref().map(|e| e.cell);
        let Some(text) = self.close_editor() else {
            return true;
        };
        if commit && let Some((r, c)) = cell {
            self.set_cell(r, c, &text)
        } else {
            // 数式バーに映していた入力を戻す
            self.sync_formula();
            true
        }
    }

    /// 確定して移動するキー（Enter・Tab・矢印）: 受け付けられなければ、入力した文字列で編集を
    /// 続ける（式の誤り・型に合わない値を直せるように）。受け付けられたら `true`。
    fn commit_or_reopen(&mut self) -> bool {
        let Some(ed) = self.editor.as_ref() else {
            return true;
        };
        let (text, cell) = (window_text(ed.hwnd), ed.cell);
        if self.end_edit(true) {
            return true;
        }
        self.cur = cell;
        self.anchor = cell;
        self.begin_edit(Some(&text));
        if let Some(e) = self.editor.as_mut() {
            e.enter_mode = false;
        }
        false
    }

    /// 編集のときに見せる文字列（式なら式）。
    fn cell_edit_text(&self, r: u64, c: u32) -> String {
        if let Some(f) = self.sheet().formula_at(r, c) {
            return f.text.to_string();
        }
        let v = self.sheet().get(&self.ctx, r, c).unwrap_or_default();
        let text = edit_text(&v, self.format_of(r, c).as_deref(), self.sys());
        // 英数字の埋め草（COBOL の空白）は編集では見せない（確定すると埋め直す）
        match yy_sheet::fixed::field_at(&self.ctx, self.sheet(), r, c) {
            Some((spec, f)) if !f.kind.is_numeric() => spec.codec.unpad(f, &text).to_string(),
            _ => text,
        }
    }

    /// セルに入力する（式・値）。式の誤り・固定長の項目の型に合わない値なら知らせて、入れずに
    /// `false` を返す。
    fn set_cell(&mut self, r: u64, c: u32, text: &str) -> bool {
        let sheet = self.sheet;
        let res = if is_formula(text) {
            let mut bad = None;
            let res = self.doc.edit(|b, ctx| {
                b.sheets[sheet].set_formula(ctx, r, c, text).map_err(|e| {
                    bad = Some(e.clone());
                    std::io::Error::other(e)
                })
            });
            if let Some(e) = bad {
                error_box(self.frame, &format!("式に誤りがあります。\n{e}"));
                self.after_edit();
                return false;
            }
            res
        } else if is_layout_cell(self.sheet(), r, c) {
            // マルチレイアウトの行のレイアウト（行のバイト列をそのレイアウトで読み直す）
            let mut bad = None;
            let res = self.doc.edit(|b, ctx| {
                yy_sheet::fixed::set_row_layout(ctx, &mut b.sheets[sheet], r, text).map_err(|e| {
                    bad = Some(e.clone());
                    std::io::Error::other(e)
                })
            });
            if let Some(e) = bad {
                error_box(
                    self.frame,
                    &format!("行のレイアウトを指定できませんでした。\n{e}"),
                );
                self.after_edit();
                return false;
            }
            res
        } else {
            let v = match yy_sheet::fixed::field_at(&self.ctx, self.sheet(), r, c) {
                Some((spec, f)) => match yy_sheet::fixed::entry_value(spec, f, text) {
                    Ok(v) => v,
                    Err(e) => {
                        error_box(self.frame, &type_mismatch(f, &e));
                        self.after_edit();
                        return false;
                    }
                },
                None => parse_entry(text, self.sys()),
            };
            self.doc.edit(|b, ctx| b.sheets[sheet].set(ctx, r, c, v))
        };
        let ok = res.is_ok();
        if let Err(e) = res {
            error_box(self.frame, &format!("入力できませんでした: {e}"));
        }
        self.after_edit();
        ok
    }

    fn after_edit(&mut self) {
        if self.doc.recalc_pending {
            unsafe {
                let _ = PostMessageW(Some(self.frame), WM_APP_RECALC, WPARAM(0), LPARAM(0));
            }
        }
        self.update_title();
        self.update_scrollbars();
        self.sync_formula();
        self.invalidate();
    }

    fn clear_selection(&mut self) {
        let (t, l, b, r) = self.selection();
        let cells = (b - t + 1) * (r - l + 1) as u64;
        if cells > CLIP_LIMIT {
            error_box(
                self.frame,
                &format!(
                    "一度に消せるのは {} セルまでです。",
                    crate::util::group_digits(CLIP_LIMIT)
                ),
            );
            return;
        }
        let sheet = self.sheet;
        let res = self.doc.edit(|bk, ctx| {
            let s = &mut bk.sheets[sheet];
            let (rows, cols) = s.extent();
            for row in t..=b.min(rows.saturating_sub(1)) {
                for col in l..=r.min(cols.saturating_sub(1)) {
                    s.set(ctx, row, col, Value::Empty)?;
                }
            }
            Ok(())
        });
        if let Err(e) = res {
            error_box(self.frame, &format!("消せませんでした: {e}"));
        }
        self.after_edit();
    }

    fn copy(&self) -> bool {
        let (t, l, b, r) = self.selection();
        let cells = (b - t + 1) * (r - l + 1) as u64;
        if cells > CLIP_LIMIT {
            error_box(
                self.frame,
                &format!(
                    "コピーできるのは {} セルまでです。大きな範囲は CSV に書き出してください。",
                    crate::util::group_digits(CLIP_LIMIT)
                ),
            );
            return false;
        }
        let sys = self.sys();
        let mut out = String::new();
        for row in t..=b {
            for col in l..=r {
                if col > l {
                    out.push('\t');
                }
                let v = self.sheet().get(&self.ctx, row, col).unwrap_or_default();
                let s = match (&v, self.format_of(row, col)) {
                    (Value::Number(n), Some(f)) => yy_numfmt::format_number(&f, *n, sys),
                    _ => v.general_text(),
                };
                if s.contains(['\t', '\n', '"']) {
                    out.push('"');
                    out.push_str(&s.replace('"', "\"\""));
                    out.push('"');
                } else {
                    out.push_str(&s);
                }
            }
            out.push_str("\r\n");
        }
        crate::clipboard::set_text(self.frame, &out, false)
    }

    fn paste(&mut self, text: &str) {
        let rows = parse_tsv(text);
        let cells: u64 = rows.iter().map(|r| r.len() as u64).sum();
        if cells > CLIP_LIMIT {
            error_box(
                self.frame,
                &format!(
                    "貼り付けられるのは {} セルまでです。",
                    crate::util::group_digits(CLIP_LIMIT)
                ),
            );
            return;
        }
        let (r0, c0) = (self.selection().0, self.selection().1);
        let sheet = self.sheet;
        let sys = self.sys();
        // 固定長の項目の型に合わない値があれば、貼り付けない
        let mut bad = None;
        let res = self.doc.edit(|b, ctx| {
            let s = &mut b.sheets[sheet];
            for (i, row) in rows.iter().enumerate() {
                for (j, v) in row.iter().enumerate() {
                    let (r, c) = (r0 + i as u64, c0 + j as u32);
                    // 式は式として（読めなければ文字列として）
                    if !(is_formula(v) && s.set_formula(ctx, r, c, v).is_ok()) {
                        if !is_formula(v) && is_layout_cell(s, r, c) {
                            yy_sheet::fixed::set_row_layout(ctx, s, r, v)
                                .map_err(std::io::Error::other)?;
                            continue;
                        }
                        let value = if is_formula(v) {
                            Value::text(v)
                        } else if let Some((spec, f)) = yy_sheet::fixed::field_at(ctx, s, r, c) {
                            match yy_sheet::fixed::entry_value(spec, f, v) {
                                Ok(x) => x,
                                Err(e) => {
                                    let at =
                                        format!("{}{}", yy_sheet::col_name(c), s.source_row(r) + 1);
                                    bad = Some(format!("{at}: {}", type_mismatch(f, &e)));
                                    return Err(std::io::Error::other("type"));
                                }
                            }
                        } else {
                            parse_entry(v, sys)
                        };
                        s.set(ctx, r, c, value)?;
                    }
                }
            }
            Ok(())
        });
        if let Some(m) = bad {
            error_box(self.frame, &format!("貼り付けませんでした。\n{m}"));
            return;
        }
        if let Err(e) = res {
            error_box(self.frame, &format!("貼り付けられませんでした: {e}"));
        }
        let h = rows.len().max(1) as u64;
        let w = rows.iter().map(Vec::len).max().unwrap_or(1).max(1) as u32;
        self.anchor = (r0, c0);
        self.cur = (r0 + h - 1, c0 + w - 1);
        self.after_edit();
    }

    /// 選択範囲の 1 行目を下へコピーする（式は相対参照をずらし、行が多ければ共有式にする）。
    fn fill_down(&mut self) {
        self.end_edit(true);
        let (t, l, b, r) = self.selection();
        if b <= t {
            return;
        }
        if self.sheet().view.rows.is_some() {
            info_box(
                self.frame,
                "絞り込み・並べ替えの表示中は下へコピーできません。解除してから行ってください。",
            );
            return;
        }
        // 値のコピーはセルごとに書くので、数を抑える
        let values = (l..=r)
            .filter(|&c| self.sheet().formula_at(t, c).is_none())
            .count() as u64;
        if values * (b - t) > CLIP_LIMIT {
            error_box(
                self.frame,
                &format!(
                    "値を一度に下へコピーできるのは {} セルまでです（式は何行でも共有式にできます）。",
                    crate::util::group_digits(CLIP_LIMIT)
                ),
            );
            return;
        }
        let sheet = self.sheet;
        let res = self.doc.edit(|bk, ctx| {
            bk.sheets[sheet]
                .fill_down(ctx, t, b, l, r)
                .map_err(std::io::Error::other)
        });
        if let Err(e) = res {
            error_box(self.frame, &format!("下へコピーできませんでした: {e}"));
        }
        self.after_edit();
    }

    fn insert_or_delete(&mut self, id: u16) {
        if matches!(id, ID_INSERT_ROWS | ID_DELETE_ROWS) && self.sheet().view.rows.is_some() {
            info_box(
                self.frame,
                "絞り込み・並べ替えの表示中は行を挿入・削除できません。\n\
                 解除するか、並べ替えを確定してから行ってください。",
            );
            return;
        }
        let (t, l, b, r) = self.selection();
        let sheet = self.sheet;
        let edit = match id {
            ID_INSERT_ROWS => yy_formula::Edit::InsertRows(t, b - t + 1),
            ID_DELETE_ROWS => yy_formula::Edit::DeleteRows(t, b - t + 1),
            ID_INSERT_COLS => yy_formula::Edit::InsertCols(l, r - l + 1),
            _ => yy_formula::Edit::DeleteCols(l, r - l + 1),
        };
        let res = self.doc.edit(|bk, ctx| bk.edit_rows_cols(ctx, sheet, edit));
        if let Err(e) = res {
            error_box(self.frame, &format!("できませんでした: {e}"));
        }
        self.after_edit();
    }

    fn zoom(&mut self, pt: Option<f32>) {
        let size = match pt {
            Some(d) => self.painter.size_pt() + d,
            None => self.config.sheet.font_size,
        };
        let _ = self.painter.set_size(size);
        self.compute_layout();
        self.update_scrollbars();
        self.invalidate();
    }

    fn switch_sheet(&mut self, i: usize) {
        if i >= self.doc.book.sheets.len() {
            return;
        }
        self.commit_formula_bar();
        self.end_edit(true);
        self.sheet = i;
        self.top = 0;
        self.left = 0;
        self.cur = (0, 0);
        self.anchor = (0, 0);
        self.update_scrollbars();
        self.sync_formula();
        self.invalidate();
    }

    fn set_document(&mut self, doc: Document, origin: Origin) {
        self.doc = doc;
        self.doc.defer_recalc = true;
        self.origin = origin;
        self.sheet = 0;
        self.top = 0;
        self.left = 0;
        self.cur = (0, 0);
        self.anchor = (0, 0);
        self.refresh_tabs();
        self.update_title();
        self.update_scrollbars();
        self.sync_formula();
        self.invalidate();
    }

    // ---- マウス ----------------------------------------------------------------------------

    /// 位置（ピクセル）のセル。見出しの上なら行・列は `None`。
    fn hit(&self, x: i32, y: i32) -> (Option<u64>, Option<u32>) {
        let xd = self.painter.to_dip(x) - self.header_w;
        let yd = self.painter.to_dip(y) - self.painter.row_h;
        let col = if xd < 0.0 {
            None
        } else {
            self.cols
                .iter()
                .find(|c| xd >= c.1 && xd < c.1 + c.2)
                .map(|c| c.0)
        };
        let row = if yd < 0.0 {
            None
        } else {
            Some(self.top + (yd / self.painter.row_h) as u64)
        };
        (row, col)
    }

    /// 見えている列のうち、見出しに絞り込みのボタンを出す列（表の列）。
    fn header_buttons(&self) -> Vec<(u32, ButtonState)> {
        let s = self.sheet();
        let n = s.table.cols();
        self.cols
            .iter()
            .filter(|c| c.0 < n)
            .map(|c| {
                (
                    c.0,
                    ButtonState {
                        filtered: s.view.filter_of(c.0).is_some(),
                        sorted: s.view.sort.iter().find(|k| k.col == c.0).map(|k| k.desc),
                    },
                )
            })
            .collect()
    }

    /// 列見出しの絞り込みのボタンの上か（列を返す）。
    fn header_button_at(&self, x: i32, y: i32) -> Option<u32> {
        let yd = self.painter.to_dip(y);
        if yd > self.painter.row_h {
            return None;
        }
        let xd = self.painter.to_dip(x) - self.header_w;
        let bw = self.painter.button_w();
        let n = self.sheet().table.cols();
        self.cols
            .iter()
            .find(|c| {
                c.0 < n && c.2 > bw * 2.0 && xd >= c.1 + c.2 - bw - 2.0 && xd < c.1 + c.2 - 2.0
            })
            .map(|c| c.0)
    }

    /// 列見出しの境目（列の幅を変える）にあるか。
    fn col_border(&self, x: i32, y: i32) -> Option<u32> {
        if self.painter.to_dip(y) > self.painter.row_h {
            return None;
        }
        let xd = self.painter.to_dip(x) - self.header_w;
        self.cols
            .iter()
            .find(|c| (xd - (c.1 + c.2)).abs() <= 3.0)
            .map(|c| c.0)
    }
}

fn window_text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(h, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..got])
    }
}

/// タブ区切り（Excel のクリップボードの形。引用符で囲んだ値の中の改行・タブ・`""`）を読む。
fn parse_tsv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    let mut at_start = true;
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '"' if at_start => {
                quoted = true;
                at_start = false;
            }
            '\t' => {
                row.push(std::mem::take(&mut cur));
                at_start = true;
            }
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cur));
                rows.push(std::mem::take(&mut row));
                at_start = true;
            }
            _ => {
                cur.push(c);
                at_start = false;
            }
        }
    }
    if !cur.is_empty() || !row.is_empty() {
        row.push(cur);
        rows.push(row);
    }
    rows
}

// ---- ファイル --------------------------------------------------------------------------

fn file_dialog_result(item: windows::Win32::UI::Shell::IShellItem) -> Option<PathBuf> {
    unsafe {
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        path.map(PathBuf::from)
    }
}

fn show_open(owner: HWND) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = [
            COMDLG_FILTERSPEC {
                pszName: w!("yysheet・CSV (*.yys;*.csv;*.tsv;*.txt)"),
                pszSpec: w!("*.yys;*.csv;*.tsv;*.txt"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("固定長ファイル (*.dat;*.bin)"),
                pszSpec: w!("*.dat;*.bin"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("すべてのファイル (*.*)"),
                pszSpec: w!("*.*"),
            },
        ];
        let _ = dialog.SetFileTypes(&filters);
        dialog.Show(Some(owner)).ok()?;
        file_dialog_result(dialog.GetResult().ok()?)
    }
}

/// 保存先（`csv_only` なら CSV だけ）。
fn show_save(owner: HWND, current: Option<&Path>, csv_only: bool) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let yys = COMDLG_FILTERSPEC {
            pszName: w!("yysheet (*.yys)"),
            pszSpec: w!("*.yys"),
        };
        let csv = COMDLG_FILTERSPEC {
            pszName: w!("CSV（UTF-8・カンマ区切り）(*.csv)"),
            pszSpec: w!("*.csv"),
        };
        let filters: Vec<COMDLG_FILTERSPEC> = if csv_only { vec![csv] } else { vec![yys, csv] };
        let _ = dialog.SetFileTypes(&filters);
        let _ = dialog.SetDefaultExtension(if csv_only { w!("csv") } else { w!("yys") });
        let stem = current
            .and_then(|p| p.file_stem())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Book1".into());
        let _ = dialog.SetFileName(&HSTRING::from(stem));
        dialog.Show(Some(owner)).ok()?;
        file_dialog_result(dialog.GetResult().ok()?)
    }
}

/// 先頭が `.yys` の印か（拡張子が違っても `.yys` なら開く）。
fn looks_like_yys(p: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 8];
    std::fs::File::open(p)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && &head == yys::MAGIC
}

fn is_csv_path(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "csv" | "tsv" | "txt"))
}

/// 変更を捨ててよいか（保存するかを尋ねる）。
fn confirm_discard() -> bool {
    let Some((frame, dirty, name)) = with(|a| {
        (
            a.frame,
            a.doc.dirty,
            a.doc
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "無題".into()),
        )
    }) else {
        return true;
    };
    if !dirty {
        return true;
    }
    let r = unsafe {
        MessageBoxW(
            Some(frame),
            &HSTRING::from(format!("{name} への変更を保存しますか？")),
            w!("yysheet"),
            MB_YESNOCANCEL | MB_ICONQUESTION,
        )
    };
    match r {
        IDYES => save(false),
        IDNO => true,
        _ => false,
    }
}

fn new_document() {
    if !confirm_discard() {
        return;
    }
    with(|a| {
        let doc = Document::with_book(a.ctx.clone(), Workbook::default());
        a.set_document(doc, Origin::New);
    });
}

fn open_dialog() {
    if !confirm_discard() {
        return;
    }
    let Some(frame) = with(|a| a.frame) else {
        return;
    };
    if let Some(p) = show_open(frame) {
        open_path(&p);
    }
}

/// ファイルを開く（`.yys` か CSV）。
fn open_path(path: &Path) {
    let Some((frame, ctx)) = with(|a| (a.frame, a.ctx.clone())) else {
        return;
    };
    if is_csv_path(path) {
        let pv = match yy_sheet::csv::preview(path) {
            Ok(p) => p,
            Err(e) => {
                error_box(
                    frame,
                    &format!("{} を開けませんでした。\n{e}", path.display()),
                );
                return;
            }
        };
        let opts = pv.options.clone();
        let p = path.to_owned();
        let label = format!("{} を取り込んでいます…（Esc で中止）", path.display());
        set_status(&label);
        let ctx2 = ctx.clone();
        let o2 = opts.clone();
        let r = crate::remote::wait(&set_status, move |w| {
            yy_sheet::csv::import(&ctx2, &p, &o2, &|done, total| {
                w.report(format!(
                    "取り込み中… {}%（Esc で中止）",
                    done * 100 / total.max(1)
                ));
                !w.cancelled()
            })
        });
        match r {
            Ok(sheet) => {
                let mut doc = Document::with_book(
                    ctx,
                    Workbook {
                        sheets: vec![sheet],
                        date_system: DateSystem::D1900,
                    },
                );
                doc.path = Some(path.to_owned());
                let rows = doc.book.sheets[0].table.rows;
                let cols = doc.book.sheets[0].table.cols();
                with(|a| a.set_document(doc, Origin::Csv(opts.clone())));
                set_status(&format!(
                    "{} 行 × {} 列を取り込みました（{}・{}）",
                    crate::util::group_digits(rows),
                    cols,
                    opts.encoding.label(),
                    opts.dialect.name()
                ));
            }
            Err(e) => {
                set_status("");
                if e.kind() != std::io::ErrorKind::Interrupted {
                    error_box(
                        frame,
                        &format!("{} を取り込めませんでした。\n{e}", path.display()),
                    );
                }
            }
        }
        return;
    }
    // .yys・CSV でなければ固定長ファイルとして開く（レイアウトを尋ねる）
    let is_yys = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("yys"));
    if !is_yys && !looks_like_yys(path) {
        fixedui::open_fixed_path(frame, ctx, path);
        return;
    }
    let p = path.to_owned();
    let ctx2 = ctx.clone();
    let r = crate::remote::wait(&set_status, move |_| yys::open(ctx2, &p));
    match r {
        Ok(doc) => {
            with(|a| a.set_document(doc, Origin::Yys));
            set_status("開きました");
            view::apply_saved();
        }
        Err(e) => error_box(
            frame,
            &format!("{} を開けませんでした。\n{e}", path.display()),
        ),
    }
}

/// 保存する（`ask` か、まだ保存先がなければ尋ねる）。CSV から開いたものは、CSV のまま保存するかを
/// 尋ねる。
fn save(ask: bool) -> bool {
    let Some((frame, path, origin)) = with(|a| {
        a.end_edit(true);
        (a.frame, a.doc.path.clone(), a.origin.clone())
    }) else {
        return false;
    };
    let target = match (&path, ask, &origin) {
        (Some(p), false, Origin::Yys) => p.clone(),
        (Some(p), false, Origin::Fixed) => {
            let r = unsafe {
                MessageBoxW(
                    Some(frame),
                    &HSTRING::from(format!(
                        "{} は固定長ファイルです。固定長のまま保存しますか？\n\n\
                         はい: 固定長で上書きする（文字コードを選べます。色・罫線・式・複数のシートは保存されません）\n\
                         いいえ: yysheet の形式（.yys）で保存する（レイアウトも保存します）",
                        p.display()
                    )),
                    w!("yysheet"),
                    MB_YESNOCANCEL | MB_ICONQUESTION,
                )
            };
            match r {
                IDYES => return fixedui::export_fixed(Some(p.clone())),
                IDNO => match show_save(frame, Some(p), false) {
                    Some(t) => t,
                    None => return false,
                },
                _ => return false,
            }
        }
        (Some(p), false, Origin::Csv(_)) => {
            let r = unsafe {
                MessageBoxW(
                    Some(frame),
                    &HSTRING::from(format!(
                        "{} は CSV です。CSV のまま保存しますか？\n\n\
                         はい: CSV で上書きする（色・罫線・複数のシートは保存されません）\n\
                         いいえ: yysheet の形式（.yys）で保存する",
                        p.display()
                    )),
                    w!("yysheet"),
                    MB_YESNOCANCEL | MB_ICONQUESTION,
                )
            };
            match r {
                IDYES => p.clone(),
                IDNO => match show_save(frame, Some(p), false) {
                    Some(t) => t,
                    None => return false,
                },
                _ => return false,
            }
        }
        _ => match show_save(frame, path.as_deref(), false) {
            Some(t) => t,
            None => return false,
        },
    };
    if is_csv_path(&target) {
        return export_csv(Some(target));
    }
    // 文書は借りたままにしない: ブックを複製（O(1)）して保存し、結果だけ戻す
    let Some((book, state)) = with(|a| (a.doc.book.clone(), a.doc.save_state())) else {
        return false;
    };
    let t = target.clone();
    let res = crate::remote::wait(&set_status, move |w| {
        yys::save_book(&book, state, &t, &mut |done, total| {
            w.report(format!("保存中… {}%", done * 100 / total.max(1)));
            !w.cancelled()
        })
    });
    let res = res.map(|saved| {
        let kind = saved.kind;
        with(|a| {
            a.doc.saved(saved);
            a.origin = Origin::Yys;
        });
        kind
    });
    with(|a| a.update_title());
    match res {
        Ok(kind) => {
            set_status(match kind {
                yys::SaveKind::Appended => "保存しました（変わった部分を書き足しました）",
                yys::SaveKind::Rewritten => "保存しました",
            });
            true
        }
        Err(e) => {
            set_status("");
            error_box(
                frame,
                &format!("{} に保存できませんでした。\n{e}", target.display()),
            );
            false
        }
    }
}

/// CSV に書き出す（`target` がなければ尋ねる）。
fn export_csv(target: Option<PathBuf>) -> bool {
    let Some((frame, path, ctx, sheet, sys, origin)) = with(|a| {
        a.end_edit(true);
        (
            a.frame,
            a.doc.path.clone(),
            a.ctx.clone(),
            a.sheet().clone(),
            a.sys(),
            a.origin.clone(),
        )
    }) else {
        return false;
    };
    let target = match target {
        Some(t) => t,
        None => match show_save(frame, path.as_deref(), true) {
            Some(t) => t,
            None => return false,
        },
    };
    let mut sheet = sheet;
    let mut order = None;
    if let Some(rows) = sheet.view.rows.clone() {
        let r = unsafe {
            MessageBoxW(
                Some(frame),
                &HSTRING::from(
                    "絞り込み・並べ替えの表示中です。\n\n\
                     はい: 見えている行を表示の順に書き出す\n\
                     いいえ: すべての行を元の順に書き出す",
                ),
                &HSTRING::from("yysheet"),
                MB_YESNOCANCEL | MB_ICONQUESTION,
            )
        };
        match r {
            IDYES => order = Some(rows),
            IDNO => sheet.view = yy_sheet::View::default(),
            _ => return false,
        }
    }
    let mut opts = ExportOptions::default();
    if let Origin::Csv(o) = &origin {
        // 開いたときの区切り文字・文字コードで書く
        opts.dialect = o.dialect;
        opts.encoding = o.encoding;
    }
    let t = target.clone();
    let r = crate::remote::wait(&set_status, move |w| {
        let order = order.as_deref().map(Vec::as_slice);
        yy_sheet::csv::export(&ctx, &sheet, sys, &t, &opts, order, &|done, total| {
            w.report(format!("書き出し中… {}%", done * 100 / total.max(1)));
            !w.cancelled()
        })
    });
    match r {
        Ok(rep) => {
            let mut msg = format!("{} 行を書き出しました", crate::util::group_digits(rep.rows));
            if rep.unencodable > 0 {
                msg.push_str(&format!(
                    "（文字コードにない文字 {} 個を ? にしました）",
                    rep.unencodable
                ));
                error_box(frame, &msg);
            }
            set_status(&msg);
            with(|a| {
                if matches!(a.origin, Origin::Csv(_))
                    && a.doc.path.as_deref() == Some(target.as_path())
                {
                    a.doc.dirty = false;
                    a.update_title();
                }
            });
            true
        }
        Err(e) => {
            set_status("");
            if e.kind() != std::io::ErrorKind::Interrupted {
                error_box(
                    frame,
                    &format!("{} に書き出せませんでした。\n{e}", target.display()),
                );
            }
            false
        }
    }
}

/// 任された式の再計算を、バックグラウンドのスレッドで行う（その間の入力は捨てる）。
fn recalc_in_background() {
    let Some((book, ctx)) = with(|a| a.doc.take_recalc().map(|b| (b, a.ctx.clone()))).flatten()
    else {
        return;
    };
    let mut book = book;
    let started = std::time::Instant::now();
    let book = crate::remote::wait(&set_status, move |w| {
        w.report("再計算中…".into());
        yy_sheet::formula::recalc(&mut book, &ctx);
        book
    });
    with(|a| {
        a.doc.put_recalc(book);
        a.update_scrollbars();
        a.sync_formula();
        a.invalidate();
    });
    let secs = started.elapsed().as_secs_f64();
    if secs >= 0.5 {
        set_status(&format!("再計算しました（{secs:.1} 秒）"));
    }
}

fn show_memory() {
    let Some((frame, text)) = with(|a| {
        let b = &a.ctx.budget;
        let mut t = format!(
            "上限: {}（設定 [sheet] memory_limit_gb）\n\n",
            crate::util::human_size(b.limit())
        );
        for p in yy_sheet::budget::Part::ALL {
            t.push_str(&format!(
                "{}: {} ／ {}\n",
                p.label(),
                crate::util::human_size(b.used(p)),
                crate::util::human_size(b.of(p))
            ));
        }
        t.push_str(&format!("\nキャッシュのチャンク: {}", a.ctx.cache.len()));
        (a.frame, t)
    }) else {
        return;
    };
    info_box(frame, &text);
}

fn command(id: u16) {
    match id {
        ID_NEW => new_document(),
        ID_OPEN => open_dialog(),
        ID_SAVE => {
            save(false);
        }
        ID_SAVE_AS => {
            save(true);
        }
        ID_EXPORT_CSV => {
            export_csv(None);
        }
        ID_OPEN_FIXED => fixedui::open_fixed(),
        ID_EXPORT_FIXED => {
            fixedui::export_fixed(None);
        }
        ID_FIXED_LAYOUT => fixedui::layout_dialog(),
        ID_OPEN_MULTI => multiui::open_multi(),
        ID_MULTI_LAYOUT => multiui::multi_layout_dialog(),
        ID_ROW_LAYOUT => multiui::row_layout_dialog(),
        ID_EXIT => {
            if let Some(f) = with(|a| a.frame) {
                unsafe {
                    let _ = PostMessageW(Some(f), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
        }
        ID_UNDO | ID_REDO => {
            with(|a| {
                a.end_edit(false);
                let ok = if id == ID_UNDO {
                    a.doc.undo()
                } else {
                    a.doc.redo()
                };
                if ok {
                    if a.sheet >= a.doc.book.sheets.len() {
                        a.sheet = 0;
                    }
                    a.refresh_tabs();
                    a.after_edit();
                }
            });
        }
        ID_COPY => {
            with(|a| a.copy());
        }
        ID_CUT => {
            with(|a| {
                if a.copy() {
                    a.clear_selection();
                }
            });
        }
        ID_PASTE => {
            let Some(frame) = with(|a| a.frame) else {
                return;
            };
            if let Some((text, _)) = crate::clipboard::get_text(frame) {
                with(|a| a.paste(&text));
            }
        }
        ID_DELETE => {
            with(|a| a.clear_selection());
        }
        ID_FILL_DOWN => {
            with(|a| a.fill_down());
        }
        ID_FIND => bulkui::find_dialog(),
        ID_FIND_NEXT => bulkui::find_next(),
        ID_REPLACE => bulkui::replace_dialog(),
        ID_DEDUP => bulkui::remove_duplicates(),
        ID_TO_NUMBER => bulkui::convert_columns(yy_sheet::bulk::Convert::Number),
        ID_TO_TEXT => bulkui::convert_columns(yy_sheet::bulk::Convert::Text),
        ID_SELECT_ALL => {
            with(|a| {
                let (rows, cols) = a.sheet().extent();
                a.anchor = (0, 0);
                a.cur = (rows.saturating_sub(1), cols.saturating_sub(1));
                a.whole = (true, true);
                a.sync_formula();
                a.invalidate();
            });
        }
        ID_INSERT_ROWS | ID_DELETE_ROWS | ID_INSERT_COLS | ID_DELETE_COLS => {
            with(|a| a.insert_or_delete(id));
        }
        ID_ADD_SHEET => {
            with(|a| {
                let n = a.doc.book.sheets.len();
                let name = (1..)
                    .map(|i| format!("Sheet{}", n + i))
                    .find(|s| !a.doc.book.sheets.iter().any(|x| &*x.name == s.as_str()))
                    .unwrap_or_else(|| "Sheet".into());
                let _ = a.doc.edit(|b, _| {
                    b.sheets.push(yy_sheet::Sheet::new(&name));
                    Ok(())
                });
                a.refresh_tabs();
                let last = a.doc.book.sheets.len() - 1;
                a.switch_sheet(last);
                a.refresh_tabs();
                a.update_title();
            });
        }
        ID_ZOOM_IN => {
            with(|a| a.zoom(Some(1.0)));
        }
        ID_ZOOM_OUT => {
            with(|a| a.zoom(Some(-1.0)));
        }
        ID_ZOOM_RESET => {
            with(|a| a.zoom(None));
        }
        ID_SORT_ASC => view::sort_active(false),
        ID_SORT_DESC => view::sort_active(true),
        ID_SORT => view::sort_dialog(),
        ID_COMMIT_SORT => view::commit_sort(),
        ID_FILTER => view::filter_column(None),
        ID_STAGES => view::stages_dialog(),
        ID_REAPPLY => view::reapply(),
        ID_CLEAR_VIEW => view::clear(),
        ID_FORMAT_CELLS => format::format_dialog(),
        ID_BOLD => format::toggle(true),
        ID_ITALIC => format::toggle(false),
        ID_FILL => format::choose_color(true),
        ID_FONT_COLOR => format::choose_color(false),
        ID_CLEAR_FORMAT => format::clear_format(),
        id if (ID_BORDER_BASE..ID_BORDER_BASE + 7).contains(&id) => {
            use yy_sheet::style::{BorderPreset as B, Line, LineStyle};
            let preset = [
                B::None,
                B::All,
                B::Outline,
                B::Top,
                B::Bottom,
                B::Left,
                B::Right,
            ][(id - ID_BORDER_BASE) as usize];
            format::apply_border(
                preset,
                Line {
                    style: LineStyle::Thin,
                    color: 0,
                },
            );
        }
        id if (ID_NUMFMT_BASE..ID_NUMFMT_BASE + format::PRESETS.len() as u16).contains(&id) => {
            format::set_number_format(format::PRESETS[(id - ID_NUMFMT_BASE) as usize].0);
        }
        ID_HELP | ID_HELP_COBOL | ID_HELP_FUNCS => {
            let section = match id {
                ID_HELP_COBOL => Some("cobol-types"),
                ID_HELP_FUNCS => Some("functions"),
                _ => None,
            };
            if let Err(e) = crate::help::show(section)
                && let Some(f) = with(|a| a.frame)
            {
                error_box(
                    f,
                    &format!(
                        "ヘルプを表示できません: {}",
                        crate::util::describe_error(&e)
                    ),
                );
            }
        }
        ID_MEMORY => show_memory(),
        ID_ABOUT => {
            if let Some(f) = with(|a| a.frame) {
                info_box(
                    f,
                    &format!(
                        "yysheet {}\n\n大量のデータ（50 億セル）を扱うスプレッドシート。\n\
                         独自形式（.yys）と RFC 4180 の CSV に対応します。",
                        env!("CARGO_PKG_VERSION")
                    ),
                );
            }
        }
        _ => {}
    }
}

/// メッセージループでのキーの横取り（ショートカット、編集中の Enter・Esc・Tab・矢印）。
fn key_hook(msg: &MSG) -> bool {
    let vk = VIRTUAL_KEY(msg.wParam.0 as u16);
    // F1（ヘルプのウィンドウの中では、そのウィンドウのキーのまま）
    if vk == VK_F1 && msg.message == WM_KEYDOWN && !crate::help::contains(msg.hwnd) {
        command(ID_HELP);
        return true;
    }
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    // 式の入力（候補の一覧・F4・矢印での参照）
    if msg.message == WM_KEYDOWN
        && with(|a| {
            a.entry_target()
                .filter(|t| *t == msg.hwnd)
                .is_some_and(|t| a.entry_key(t, vk, shift, ctrl))
        }) == Some(true)
    {
        return true;
    }
    // 編集中のセル
    let editing = with(|a| a.editor.as_ref().map(|e| (e.hwnd, e.enter_mode))).flatten();
    if let Some((h, enter_mode)) = editing
        && msg.hwnd == h
    {
        let mv = |dr: i64, dc: i64| {
            with(|a| {
                if !a.commit_or_reopen() {
                    return;
                }
                let (r, c) = a.cur;
                let nr = (r as i64 + dr).max(0) as u64;
                let nc = (c as i64 + dc).max(0) as u32;
                a.move_to(nr, nc, false);
            });
        };
        match vk {
            VK_RETURN => mv(if shift { -1 } else { 1 }, 0),
            VK_TAB => mv(0, if shift { -1 } else { 1 }),
            VK_ESCAPE => {
                with(|a| a.end_edit(false));
            }
            VK_UP if enter_mode => mv(-1, 0),
            VK_DOWN if enter_mode => mv(1, 0),
            VK_LEFT if enter_mode => mv(0, -1),
            VK_RIGHT if enter_mode => mv(0, 1),
            _ => return false,
        }
        return true;
    }
    // 数式バー
    if let Some(f) = with(|a| a.formula)
        && msg.hwnd == f
    {
        match vk {
            VK_RETURN => {
                with(|a| {
                    let text = window_text(a.formula);
                    // 別のシートを表示していれば元のシートに戻してから
                    a.entry_reset();
                    let (r, c) = a.cur;
                    if !a.set_cell(r, c, &text) {
                        // 直せるよう数式バーに入力を残す
                        unsafe {
                            let _ = SetWindowTextW(a.formula, &HSTRING::from(text.as_str()));
                            let _ = SetFocus(Some(a.formula));
                        }
                        return;
                    }
                    unsafe {
                        let _ = SetFocus(Some(a.grid));
                    }
                    a.move_to(r + 1, c, false);
                });
                return true;
            }
            VK_ESCAPE => {
                with(|a| {
                    a.entry_reset();
                    unsafe {
                        let _ = SetFocus(Some(a.grid));
                    }
                    a.sync_formula();
                });
                return true;
            }
            _ => return false,
        }
    }
    if vk == VK_F3 && !ctrl && msg.message == WM_KEYDOWN {
        command(ID_FIND_NEXT);
        return true;
    }
    if !ctrl {
        return false;
    }
    let alt = unsafe { GetKeyState(VK_MENU.0 as i32) } < 0;
    if vk == VK_L && (shift || alt) {
        command(if alt { ID_REAPPLY } else { ID_FILTER });
        return true;
    }
    let id = match (vk, shift) {
        (VK_N, false) => ID_NEW,
        (VK_O, false) => ID_OPEN,
        (VK_S, false) => ID_SAVE,
        (VK_S, true) => ID_SAVE_AS,
        (VK_Z, false) => ID_UNDO,
        (VK_Y, false) => ID_REDO,
        (VK_A, false) => ID_SELECT_ALL,
        (VK_OEM_PLUS | VK_ADD, _) => ID_INSERT_ROWS,
        (VK_OEM_MINUS | VK_SUBTRACT, _) => ID_DELETE_ROWS,
        (VK_0 | VK_NUMPAD0, false) => ID_ZOOM_RESET,
        (VK_1, false) => ID_FORMAT_CELLS,
        (VK_B, false) => ID_BOLD,
        (VK_D, false) => ID_FILL_DOWN,
        (VK_F, false) => ID_FIND,
        (VK_H, false) => ID_REPLACE,
        (VK_I, false) => ID_ITALIC,
        // コピー・貼り付けは格子にフォーカスがあるときだけ（数式バーの EDIT では EDIT に任せる）
        (VK_C | VK_X | VK_V, false) => {
            let Some(grid) = with(|a| a.grid) else {
                return false;
            };
            if msg.hwnd != grid {
                return false;
            }
            match vk {
                VK_C => ID_COPY,
                VK_X => ID_CUT,
                _ => ID_PASTE,
            }
        }
        _ => return false,
    };
    command(id);
    true
}

// ---- ウィンドウプロシージャ ------------------------------------------------------------

extern "system" fn frame_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            with(|a| {
                a.layout();
                a.place_editor();
            });
            LRESULT(0)
        }
        WM_SETFOCUS => {
            if let Some(g) = with(|a| a.grid) {
                unsafe {
                    let _ = SetFocus(Some(g));
                }
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = loword(wparam.0) as u16;
            let code = hiword(wparam.0);
            if id == IDC_FORMULA {
                if code == EN_SETFOCUS {
                    with(|a| a.move_edit_to_formula_bar());
                }
                if code == EN_SETFOCUS || code == EN_KILLFOCUS {
                    unsafe {
                        let _ = PostMessageW(Some(hwnd), WM_APP_ENTRY, WPARAM(0), LPARAM(0));
                    }
                }
                return LRESULT(0);
            }
            command(id);
            LRESULT(0)
        }
        WM_NOTIFY => {
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if hdr.idFrom == IDC_TABS as usize && hdr.code == TCN_SELCHANGE {
                with(|a| {
                    let i = unsafe { SendMessageW(a.tabs, TCM_GETCURSEL, None, None) }.0;
                    // 式の入力中なら、そのシートのセルを参照として選べるようにする
                    if i >= 0 && a.point_sheet(i as usize) {
                        return;
                    }
                    if i >= 0 {
                        a.switch_sheet(i as usize);
                    }
                    unsafe {
                        let _ = SetFocus(Some(a.grid));
                    }
                });
            }
            LRESULT(0)
        }
        WM_DROPFILES => {
            let drop = HDROP(wparam.0 as *mut _);
            let mut buf = [0u16; 1024];
            let n = unsafe { DragQueryFileW(drop, 0, Some(&mut buf)) } as usize;
            unsafe { DragFinish(drop) };
            if n > 0 && confirm_discard() {
                let p = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
                open_path(&p);
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = hiword(wparam.0);
            with(|a| {
                a.painter.set_dpi(dpi);
                let f = crate::util::ui_font(dpi);
                for h in [a.formula, a.name_box, a.tabs, a.status] {
                    unsafe {
                        SendMessageW(h, WM_SETFONT, Some(WPARAM(f.0 as usize)), Some(LPARAM(1)));
                    }
                }
                let old = std::mem::replace(&mut a.ui_font, f);
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(old.0));
                }
            });
            let r = unsafe { &*(lparam.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            if confirm_discard() {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        crate::remote::WM_APP_REMOTE_WAKE => LRESULT(0),
        WM_APP_RECALC => {
            recalc_in_background();
            LRESULT(0)
        }
        WM_APP_ENTRY => {
            with(|a| a.refresh_assist());
            LRESULT(0)
        }
        WM_ACTIVATE | WM_MOVE => {
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_APP_ENTRY, WPARAM(0), LPARAM(0));
            }
            default_proc(hwnd, msg, wparam, lparam)
        }
        _ => default_proc(hwnd, msg, wparam, lparam),
    }
}

fn mouse_pos(lparam: LPARAM) -> (i32, i32) {
    (
        (lparam.0 & 0xFFFF) as i16 as i32,
        ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    )
}

extern "system" fn grid_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe {
                BeginPaint(hwnd, &mut ps);
            }
            with(|a| a.paint());
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_SIZE => {
            let (w, h) = (
                loword(lparam.0 as usize) as i32,
                hiword(lparam.0 as usize) as i32,
            );
            with(|a| {
                a.size_px = (w, h);
                a.painter.resize_target(w as u32, h as u32);
                a.compute_layout();
                a.update_scrollbars();
                a.invalidate();
            });
            LRESULT(0)
        }
        WM_GETDLGCODE => LRESULT((DLGC_WANTARROWS | DLGC_WANTCHARS | DLGC_WANTTAB) as isize),
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (x, y) = mouse_pos(lparam);
            let shift = wparam.0 & MK_SHIFT != 0;
            let ctrl = wparam.0 & MK_CONTROL != 0;
            // 式の入力中なら、クリックしたセル・列・行を参照として入れる
            if with(|a| a.point_click(x, y, shift)) == Some(true) {
                unsafe {
                    SetCapture(hwnd);
                }
                return LRESULT(0);
            }
            // 別のシートの参照を選んでいる途中で参照を入れられない位置なら、確定して元のシートへ
            if with(|a| a.home.is_some()) == Some(true) {
                with(|a| {
                    a.commit_formula_bar();
                    a.end_edit(true);
                });
                return LRESULT(0);
            }
            with(|a| a.commit_formula_bar());
            // オートフィル オプションのボタン
            if let Some((pt, dates, checked)) = with(|a| a.fill_button_down(x, y)).flatten() {
                if let Some(id) = fillhandle::options_menu(hwnd, pt.x, pt.y, dates, checked) {
                    with(|a| a.refill(id));
                }
                return LRESULT(0);
            }
            with(|a| a.last_fill = None);
            unsafe {
                let _ = SetFocus(Some(hwnd));
                SetCapture(hwnd);
            }
            // フィルハンドル
            if with(|a| a.handle_down(x, y, msg == WM_LBUTTONDBLCLK, ctrl, false)) == Some(true) {
                if msg == WM_LBUTTONDBLCLK {
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                }
                return LRESULT(0);
            }
            let button = with(|a| a.header_button_at(x, y)).flatten();
            if let Some(c) = button {
                unsafe {
                    let _ = ReleaseCapture();
                }
                with(|a| {
                    a.end_edit(true);
                    a.anchor = (a.cur.0, c);
                    a.cur = a.anchor;
                    a.sync_formula();
                    a.invalidate();
                });
                view::filter_column(Some(c));
                return LRESULT(0);
            }
            with(|a| {
                a.end_edit(true);
                if let Some(c) = a.col_border(x, y) {
                    let start = a.painter.to_dip(x);
                    a.drag = Some(Drag::ColWidth(c, start, a.col_dip(c)));
                    return;
                }
                let (row, col) = a.hit(x, y);
                match (row, col) {
                    (Some(r), Some(c)) => {
                        a.move_to(r, c, shift);
                        if msg == WM_LBUTTONDBLCLK {
                            a.begin_edit(None);
                            return;
                        }
                    }
                    (None, Some(c)) => {
                        // 列全体
                        let rows = a.sheet().extent().0.max(1);
                        a.anchor = (0, if shift { a.anchor.1 } else { c });
                        a.cur = (rows - 1, c);
                        a.whole = (true, false);
                        a.sync_formula();
                        a.invalidate();
                    }
                    (Some(r), None) => {
                        let cols = a.sheet().extent().1.max(1);
                        a.anchor = (if shift { a.anchor.0 } else { r }, 0);
                        a.cur = (r, cols - 1);
                        a.whole = (false, true);
                        a.sync_formula();
                        a.invalidate();
                    }
                    (None, None) => {}
                }
                a.drag = Some(Drag::Select);
            });
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = mouse_pos(lparam);
            with(|a| match a.drag {
                Some(Drag::Point) => a.point_drag(x, y),
                Some(Drag::Fill(..)) => {
                    unsafe {
                        if let Ok(c) = LoadCursorW(None, IDC_CROSS) {
                            SetCursor(Some(c));
                        }
                    }
                    a.handle_drag(x, y);
                }
                Some(Drag::Select) => {
                    let (row, col) = a.hit(x, y);
                    let r = row.unwrap_or(a.top);
                    let c = col.unwrap_or(a.cols.last().map(|c| c.0).unwrap_or(0));
                    if (r, c) != a.cur {
                        a.cur = (r, c);
                        a.ensure_visible();
                        a.update_scrollbars();
                        a.sync_formula();
                        a.invalidate();
                    }
                }
                Some(Drag::ColWidth(c, start, w0)) => {
                    let w = (w0 + a.painter.to_dip(x) - start).max(8.0);
                    let chars = a.painter.col_chars(w);
                    let sheet = a.sheet;
                    std::sync::Arc::make_mut(&mut a.doc.book.sheets[sheet].col_widths)
                        .insert(c, chars);
                    a.doc.dirty = true;
                    a.invalidate();
                }
                None => {
                    let cursor = if a.col_border(x, y).is_some() {
                        Some(IDC_SIZEWE)
                    } else if a.on_handle(x, y) {
                        Some(IDC_CROSS)
                    } else {
                        None
                    };
                    if let Some(id) = cursor {
                        unsafe {
                            if let Ok(c) = LoadCursorW(None, id) {
                                SetCursor(Some(c));
                            }
                        }
                    }
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            let ctrl = wparam.0 & MK_CONTROL != 0;
            with(|a| {
                if matches!(a.drag, Some(Drag::ColWidth(..))) {
                    a.update_title();
                    a.update_scrollbars();
                }
                if matches!(a.drag, Some(Drag::Fill(_, false))) {
                    a.handle_up(ctrl);
                } else if !matches!(a.drag, Some(Drag::Fill(_, true))) {
                    a.drag = None;
                }
                a.invalidate();
            });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let ctrl = wparam.0 & MK_CONTROL != 0;
            with(|a| {
                if ctrl {
                    a.zoom(Some(if delta > 0 { 1.0 } else { -1.0 }));
                    return;
                }
                let lines = (delta / 120 * 3) as i64;
                a.top = (a.top as i64 - lines).max(0) as u64;
                a.update_scrollbars();
                a.invalidate();
                // 編集中はそのまま（式に入れる参照を選べるように）。欄をセルに合わせる
                a.place_editor();
            });
            LRESULT(0)
        }
        WM_VSCROLL | WM_HSCROLL => {
            let code = SCROLLBAR_COMMAND(loword(wparam.0) as i32);
            let vert = msg == WM_VSCROLL;
            with(|a| {
                let page = if vert {
                    a.visible_rows() as i64
                } else {
                    a.visible_cols() as i64
                };
                let pos = if vert { a.top as i64 } else { a.left as i64 };
                let track = || unsafe {
                    let mut si = SCROLLINFO {
                        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                        fMask: SIF_TRACKPOS,
                        ..Default::default()
                    };
                    let _ = GetScrollInfo(hwnd, if vert { SB_VERT } else { SB_HORZ }, &mut si);
                    si.nTrackPos as i64
                };
                let new = match code {
                    SB_LINEUP => pos - 1,
                    SB_LINEDOWN => pos + 1,
                    SB_PAGEUP => pos - page,
                    SB_PAGEDOWN => pos + page,
                    SB_THUMBTRACK | SB_THUMBPOSITION => track(),
                    SB_TOP => 0,
                    _ => pos,
                }
                .max(0);
                if vert {
                    a.top = new as u64;
                } else {
                    a.left = (new as u32).min(16_383);
                }
                a.update_scrollbars();
                a.place_editor();
                a.invalidate();
            });
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wparam.0 as u16);
            let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
            let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
            with(|a| {
                let (r, c) = a.cur;
                let step = |a: &mut App, dr: i64, dc: i64| {
                    let (nr, nc) = if ctrl {
                        a.edge(dr, dc)
                    } else {
                        ((r as i64 + dr).max(0) as u64, (c as i64 + dc).max(0) as u32)
                    };
                    a.move_to(nr, nc, shift);
                };
                match vk {
                    VK_UP => step(a, -1, 0),
                    VK_DOWN => step(a, 1, 0),
                    VK_LEFT => step(a, 0, -1),
                    VK_RIGHT => step(a, 0, 1),
                    VK_PRIOR => {
                        let p = a.visible_rows();
                        a.top = a.top.saturating_sub(p);
                        a.move_to(r.saturating_sub(p), c, shift);
                    }
                    VK_NEXT => {
                        let p = a.visible_rows();
                        a.top += p;
                        a.move_to(r + p, c, shift);
                    }
                    VK_HOME if ctrl => a.move_to(0, 0, shift),
                    VK_HOME => a.move_to(r, 0, shift),
                    VK_END if ctrl => {
                        let (rows, cols) = a.sheet().extent();
                        a.move_to(rows.saturating_sub(1), cols.saturating_sub(1), shift);
                    }
                    VK_TAB => a.move_to(r, if shift { c.saturating_sub(1) } else { c + 1 }, false),
                    VK_RETURN => {
                        a.move_to(if shift { r.saturating_sub(1) } else { r + 1 }, c, false)
                    }
                    VK_F2 => a.begin_edit(None),
                    VK_DELETE => a.clear_selection(),
                    VK_ESCAPE => {
                        a.anchor = a.cur;
                        a.invalidate();
                    }
                    _ => {}
                }
            });
            LRESULT(0)
        }
        WM_CHAR => {
            let ch = wparam.0 as u32;
            if ch >= 0x20
                && ch != 0x7F
                && let Some(c) = char::from_u32(ch)
            {
                let mut b = [0u8; 4];
                let s = c.encode_utf8(&mut b).to_owned();
                with(|a| a.begin_edit(Some(&s)));
            }
            LRESULT(0)
        }
        WM_IME_STARTCOMPOSITION => {
            // 日本語の入力を始めたら、セルの編集を始めて IME の入力を編集の欄に移す
            with(|a| a.begin_edit(Some("")));
            default_proc(hwnd, msg, wparam, lparam)
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            with(|a| a.invalidate());
            LRESULT(0)
        }
        WM_CONTEXTMENU => {
            let (x, y) = mouse_pos(lparam);
            unsafe {
                if let Ok(menu) = CreatePopupMenu() {
                    for (id, t) in [
                        (ID_CUT, "切り取り"),
                        (ID_COPY, "コピー"),
                        (ID_PASTE, "貼り付け"),
                        (0, ""),
                        (ID_INSERT_ROWS, "行を挿入"),
                        (ID_DELETE_ROWS, "行を削除"),
                        (ID_INSERT_COLS, "列を挿入"),
                        (ID_DELETE_COLS, "列を削除"),
                        (0, ""),
                        (ID_DELETE, "内容を消す"),
                    ] {
                        if id == 0 {
                            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
                        } else {
                            let _ = AppendMenuW(menu, MF_STRING, id as usize, &HSTRING::from(t));
                        }
                    }
                    let mut pt = POINT { x, y };
                    if x == -1 && y == -1 {
                        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt);
                    }
                    let cmd = TrackPopupMenu(
                        menu,
                        TPM_RETURNCMD | TPM_RIGHTBUTTON,
                        pt.x,
                        pt.y,
                        Some(0),
                        hwnd,
                        None,
                    );
                    let _ = DestroyMenu(menu);
                    if cmd.0 > 0 {
                        command(cmd.0 as u16);
                    }
                }
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            let Some((target, dates)) = with(|a| a.take_right_fill()).flatten() else {
                return default_proc(hwnd, msg, wparam, lparam);
            };
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
            }
            match fillhandle::options_menu(hwnd, pt.x, pt.y, dates, 0) {
                Some(id) => {
                    with(|a| a.right_fill(target, id));
                }
                None => set_status("準備完了"),
            }
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            let (x, y) = mouse_pos(lparam);
            // 右ボタンでフィルハンドルをドラッグ（離すと仕方のメニュー）
            if with(|a| a.handle_down(x, y, false, false, true)) == Some(true) {
                unsafe {
                    let _ = SetFocus(Some(hwnd));
                    SetCapture(hwnd);
                }
                return LRESULT(0);
            }
            with(|a| {
                let (row, col) = a.hit(x, y);
                if let (Some(r), Some(c)) = (row, col) {
                    let (t, l, b, rr) = a.selection();
                    if !((t..=b).contains(&r) && (l..=rr).contains(&c)) {
                        a.move_to(r, c, false);
                    }
                }
            });
            let _ = ScreenToClient;
            LRESULT(0)
        }
        _ => default_proc(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsv_parsing() {
        assert_eq!(
            parse_tsv("a\tb\r\n\"x\ty\"\t\"q\"\"q\"\r\n"),
            vec![vec!["a", "b"], vec!["x\ty", "q\"q"]]
        );
        assert_eq!(parse_tsv("1\t2"), vec![vec!["1", "2"]]);
        assert_eq!(parse_tsv("\"改\n行\"\tz\n"), vec![vec!["改\n行", "z"]]);
    }

    #[test]
    fn entries_and_display() {
        let sys = DateSystem::D1900;
        assert_eq!(parse_entry("123", sys), Value::Number(123.0));
        assert_eq!(parse_entry("'123", sys), Value::text("123"));
        assert_eq!(
            parse_entry("#N/A", sys),
            Value::Error(yy_sheet::CellError::NA)
        );
        assert_eq!(parse_entry("2026/10/7", sys), Value::Number(46302.0));
        let (t, a, _) = display(&Value::Number(46302.0), Some("yyyy/m/d"), 10.0, sys);
        assert_eq!((t.as_str(), a), ("2026/10/7", Align::Right));
        let (t, _, _) = display(&Value::Number(1234567890123.0), None, 8.0, sys);
        assert_eq!(t, "1.23E+12");
        assert_eq!(
            edit_text(&Value::Number(46302.0), Some("yyyy/m/d"), sys),
            "2026/10/7"
        );
        assert_eq!(edit_text(&Value::text("00123"), None, sys), "00123");
        assert_eq!(edit_text(&Value::text("123"), None, sys), "'123");
    }
}

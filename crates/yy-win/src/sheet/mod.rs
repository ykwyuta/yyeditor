//! スプレッドシート（yysheet。15 章 12）。
//!
//! フレーム（メニュー・数式バー・シートのタブ・ステータスバー）と格子（独自描画）。格子は見える範囲の
//! セルだけを読んで描く。セルの編集は格子の上に重ねた EDIT コントロールで行う（IME はそのまま使える）。
//! ファイルを開く・CSV の取り込み・保存・書き出しはバックグラウンドで行い、進みをステータスバーに出す
//! （Esc で中止）。
//!
//! 状態 [`App`] は UI スレッドのスレッドローカルに置く。ダイアログ・待ち（`remote::wait`）の間は
//! 状態を借りたままにしない。

mod paint;

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
use paint::{Align, Cell, GridPainter, Scene};

const FRAME_CLASS: PCWSTR = w!("YYSheetFrame");
const GRID_CLASS: PCWSTR = w!("YYSheetGrid");

const ID_NEW: u16 = 1;
const ID_OPEN: u16 = 2;
const ID_SAVE: u16 = 3;
const ID_SAVE_AS: u16 = 4;
const ID_EXPORT_CSV: u16 = 5;
const ID_EXIT: u16 = 6;
const ID_UNDO: u16 = 10;
const ID_REDO: u16 = 11;
const ID_CUT: u16 = 12;
const ID_COPY: u16 = 13;
const ID_PASTE: u16 = 14;
const ID_DELETE: u16 = 15;
const ID_SELECT_ALL: u16 = 16;
const ID_INSERT_ROWS: u16 = 20;
const ID_DELETE_ROWS: u16 = 21;
const ID_INSERT_COLS: u16 = 22;
const ID_DELETE_COLS: u16 = 23;
const ID_ADD_SHEET: u16 = 24;
const ID_ZOOM_IN: u16 = 30;
const ID_ZOOM_OUT: u16 = 31;
const ID_ZOOM_RESET: u16 = 32;
const ID_MEMORY: u16 = 40;
const ID_ABOUT: u16 = 41;

/// STATIC の文字を上下の中央に置く
const SS_CENTERIMAGE: u32 = 0x200;
/// マウスのメッセージの wParam（Shift・Ctrl）
const MK_SHIFT: usize = 0x4;
const MK_CONTROL: usize = 0x8;

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
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

fn set_status(text: &str) {
    if let Some(h) = with(|a| a.status) {
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
    let frame = create()?;
    if let Some(p) = initial {
        open_path(&p);
    }
    let _ = frame;
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if (msg.message == WM_KEYDOWN || msg.message == WM_SYSKEYDOWN) && key_hook(&msg) {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
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
        sep(file);
        add(file, ID_SAVE, "上書き保存(&S)\tCtrl+S");
        add(file, ID_SAVE_AS, "名前を付けて保存(&A)...\tCtrl+Shift+S");
        add(file, ID_EXPORT_CSV, "CSV に書き出し(&E)...");
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
        let help = CreatePopupMenu()?;
        add(help, ID_MEMORY, "メモリの使用状況(&M)");
        add(help, ID_ABOUT, "yysheet について(&A)");
        for (m, t) in [
            (file, "ファイル(&F)"),
            (edit, "編集(&E)"),
            (insert, "挿入・削除(&I)"),
            (view, "表示(&V)"),
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
        let app = App {
            frame,
            grid,
            formula,
            name_box,
            tabs,
            status,
            ui_font,
            config,
            doc: Document::new(ctx.clone()),
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

    /// セルの表示形式（表の列の既定の形式）。
    fn format_of(&self, row: u64, col: u32) -> Option<Arc<str>> {
        match self.sheet().place(row, col) {
            yy_sheet::Place::Data(_, c) => self.sheet().table.columns[c as usize].format.clone(),
            _ => None,
        }
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
        self.compute_layout();
        let sys = self.sys();
        let mut cells = Vec::with_capacity(self.rows.len());
        for &(row, _) in &self.rows {
            let mut line = Vec::with_capacity(self.cols.len());
            for &(col, _, _) in &self.cols {
                let v = self
                    .sheet()
                    .get(&self.ctx, row, col)
                    .unwrap_or(Value::Error(yy_sheet::CellError::Value));
                let fmt = self.format_of(row, col);
                let chars = self.col_chars(col);
                let (text, align, color) = display(&v, fmt.as_deref(), chars, sys);
                let table_head = matches!(self.sheet().place(row, col), yy_sheet::Place::Header(_));
                line.push(Cell {
                    text,
                    align,
                    color,
                    table_head,
                });
            }
            cells.push(line);
        }
        let (t, l, b, r) = self.selection();
        let scene = Scene {
            cols: &self.cols,
            rows: &self.rows,
            header_w: self.header_w,
            cells: &cells,
            sel: (t, l, b, r),
            active: self.cur,
            editing: self.editor.is_some(),
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
            format!("{}{}", yy_sheet::col_name(c), r + 1)
        } else {
            format!("{}R × {}C", b - t + 1, rr - l + 1)
        };
        let v = self.sheet().get(&self.ctx, r, c).unwrap_or_default();
        let text = edit_text(&v, self.format_of(r, c).as_deref(), self.sys());
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
                format!("{} セルを選択", crate::util::group_digits(cells))
            };
            set_status(&msg);
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
            None => {
                let v = self.sheet().get(&self.ctx, r, c).unwrap_or_default();
                edit_text(&v, self.format_of(r, c).as_deref(), self.sys())
            }
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
    }

    fn close_editor(&mut self) -> Option<String> {
        let ed = self.editor.take()?;
        let text = window_text(ed.hwnd);
        unsafe {
            let _ = DestroyWindow(ed.hwnd);
            let _ = DeleteObject(HGDIOBJ(ed.font.0));
            let _ = SetFocus(Some(self.grid));
        }
        self.invalidate();
        Some(text)
    }

    /// 編集を確定する（`false` なら取り消す）。
    fn end_edit(&mut self, commit: bool) {
        let cell = self.editor.as_ref().map(|e| e.cell);
        let Some(text) = self.close_editor() else {
            return;
        };
        if commit && let Some((r, c)) = cell {
            self.set_cell(r, c, &text);
        }
    }

    fn set_cell(&mut self, r: u64, c: u32, text: &str) {
        let v = parse_entry(text, self.sys());
        let sheet = self.sheet;
        let res = self.doc.edit(|b, ctx| b.sheets[sheet].set(ctx, r, c, v));
        if let Err(e) = res {
            error_box(self.frame, &format!("入力できませんでした: {e}"));
        }
        self.after_edit();
    }

    fn after_edit(&mut self) {
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
        let res = self.doc.edit(|b, ctx| {
            let s = &mut b.sheets[sheet];
            for (i, row) in rows.iter().enumerate() {
                for (j, v) in row.iter().enumerate() {
                    s.set(ctx, r0 + i as u64, c0 + j as u32, parse_entry(v, sys))?;
                }
            }
            Ok(())
        });
        if let Err(e) = res {
            error_box(self.frame, &format!("貼り付けられませんでした: {e}"));
        }
        let h = rows.len().max(1) as u64;
        let w = rows.iter().map(Vec::len).max().unwrap_or(1).max(1) as u32;
        self.anchor = (r0, c0);
        self.cur = (r0 + h - 1, c0 + w - 1);
        self.after_edit();
    }

    fn insert_or_delete(&mut self, id: u16) {
        let (t, l, b, r) = self.selection();
        let sheet = self.sheet;
        let res = self.doc.edit(|bk, ctx| {
            let s = &mut bk.sheets[sheet];
            match id {
                ID_INSERT_ROWS => s.insert_rows(ctx, t, b - t + 1),
                ID_DELETE_ROWS => s.delete_rows(ctx, t, b - t + 1),
                ID_INSERT_COLS => s.insert_cols(ctx, l, r - l + 1),
                _ => {
                    s.delete_cols(l, r - l + 1);
                    Ok(())
                }
            }
        });
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
    let p = path.to_owned();
    let ctx2 = ctx.clone();
    let r = crate::remote::wait(&set_status, move |_| yys::open(ctx2, &p));
    match r {
        Ok(doc) => {
            with(|a| a.set_document(doc, Origin::Yys));
            set_status("開きました");
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
    let mut opts = ExportOptions::default();
    if let Origin::Csv(o) = &origin {
        // 開いたときの区切り文字・文字コードで書く
        opts.dialect = o.dialect;
        opts.encoding = o.encoding;
    }
    let t = target.clone();
    let r = crate::remote::wait(&set_status, move |w| {
        yy_sheet::csv::export(&ctx, &sheet, sys, &t, &opts, None, &|done, total| {
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
        ID_SELECT_ALL => {
            with(|a| {
                let (rows, cols) = a.sheet().extent();
                a.anchor = (0, 0);
                a.cur = (rows.saturating_sub(1), cols.saturating_sub(1));
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
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    // 編集中のセル
    let editing = with(|a| a.editor.as_ref().map(|e| (e.hwnd, e.enter_mode))).flatten();
    if let Some((h, enter_mode)) = editing
        && msg.hwnd == h
    {
        let mv = |dr: i64, dc: i64| {
            with(|a| {
                a.end_edit(true);
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
                    let (r, c) = a.cur;
                    a.set_cell(r, c, &text);
                    unsafe {
                        let _ = SetFocus(Some(a.grid));
                    }
                    a.move_to(r + 1, c, false);
                });
                return true;
            }
            VK_ESCAPE => {
                with(|a| {
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
    if !ctrl {
        return false;
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
            with(|a| a.layout());
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
                    with(|a| a.end_edit(true));
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
            unsafe {
                let _ = SetFocus(Some(hwnd));
                SetCapture(hwnd);
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
                        a.sync_formula();
                        a.invalidate();
                    }
                    (Some(r), None) => {
                        let cols = a.sheet().extent().1.max(1);
                        a.anchor = (if shift { a.anchor.0 } else { r }, 0);
                        a.cur = (r, cols - 1);
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
                    if a.col_border(x, y).is_some() {
                        unsafe {
                            if let Ok(c) = LoadCursorW(None, IDC_SIZEWE) {
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
            with(|a| {
                if matches!(a.drag, Some(Drag::ColWidth(..))) {
                    a.update_title();
                    a.update_scrollbars();
                }
                a.drag = None;
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
                if let Some(ed) = &a.editor {
                    let _ = ed;
                    a.end_edit(true);
                }
            });
            LRESULT(0)
        }
        WM_VSCROLL | WM_HSCROLL => {
            let code = SCROLLBAR_COMMAND(loword(wparam.0) as i32);
            let vert = msg == WM_VSCROLL;
            with(|a| {
                a.end_edit(true);
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
        WM_RBUTTONDOWN => {
            let (x, y) = mouse_pos(lparam);
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

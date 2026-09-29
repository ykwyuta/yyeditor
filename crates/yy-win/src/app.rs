//! アプリケーション状態とウィンドウプロシージャ。

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Controls::{
    SB_SETPARTS, SB_SETTEXTW, SBARS_SIZEGRIP, STATUSCLASSNAMEW, SetScrollInfo,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::Ime::ISC_SHOWUICOMPOSITIONWINDOW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    DragAcceptFiles, DragFinish, DragQueryFileW, FileOpenDialog, FileSaveDialog, HDROP,
    IFileOpenDialog, IFileSaveDialog, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Result, w};
use yy_buffer::{LineLookup, Snapshot};
use yy_config::Config;
use yy_core::{Document, EditKind, Selection, SelectionSet, motion};
use yy_jobs::{JobPool, Notifier};
use yy_layout::{
    Row, RowConfig, Viewport, next_row_start, prev_row_start, row_at, row_containing, rows_from,
};

use crate::render::{Composition, Frame, Renderer};
use crate::util::{Context, error_box, group_digits, human_size, info_box, wide};
use crate::{FRAME_CLASS, VIEW_CLASS, clipboard, default_proc, hiword, ime, loword};

// メニュー・アクセラレータのコマンド ID
const ID_OPEN: u16 = 101;
const ID_CLOSE: u16 = 102;
const ID_EXIT: u16 = 103;
const ID_NEW: u16 = 104;
const ID_SAVE: u16 = 105;
const ID_SAVE_AS: u16 = 106;
const ID_GOTO: u16 = 201;
const ID_ZOOM_IN: u16 = 202;
const ID_ZOOM_OUT: u16 = 203;
const ID_ZOOM_RESET: u16 = 204;
const ID_LINE_NUMBERS: u16 = 205;
const ID_ABOUT: u16 = 301;
const ID_UNDO: u16 = 401;
const ID_REDO: u16 = 402;
const ID_CUT: u16 = 403;
const ID_COPY: u16 = 404;
const ID_PASTE: u16 = 405;
const ID_SELECT_ALL: u16 = 406;
const ID_DELETE: u16 = 407;

const ID_STATUS: i32 = 1000;
const TIMER_BLINK: usize = 1;

/// ワーカースレッドから行数カウントの進捗を知らせるメッセージ
const WM_APP_INDEX: u32 = WM_APP + 1;
const WM_APP_RELAYOUT: u32 = WM_APP + 2;
const WM_APP_SCROLLBARS: u32 = WM_APP + 3;
const WM_APP_REPAINT: u32 = WM_APP + 4;
const WM_APP_RESIZE: u32 = WM_APP + 5;

/// バイト位置比例スクロールバーの範囲
const SCROLL_RANGE: i32 = 1 << 16;
/// クリップボードにコピーできる最大サイズ
const MAX_CLIPBOARD_BYTES: u64 = 256 << 20;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// アプリ状態を借用して `f` を実行する。再入中（すでに借用中）なら `None`。
///
/// メッセージボックスやファイルダイアログはモーダルループ中にウィンドウプロシージャを
/// 再入させるため、`f` の中では呼ばないこと。
pub(crate) fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

pub(crate) fn shutdown() {
    // ドキュメント（mmap）とジョブプールを UI スレッド上で解放する
    let app = APP.with(|cell| cell.borrow_mut().take());
    drop(app);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScrollMode {
    /// 行数比例（行数が確定していて少ない場合）
    Lines,
    /// バイト位置比例（巨大ファイル・行数未確定）
    Bytes,
}

/// マウスでドラッグ選択中の状態。
struct Drag {
    anchor: u64,
    /// Ctrl+クリックで追加中なら、追加前の選択
    base: Option<SelectionSet>,
}

pub(crate) struct App {
    frame: HWND,
    view: HWND,
    status: HWND,
    menu_edit: HMENU,
    menu_view: HMENU,
    config: Config,
    rows_cfg: RowConfig,
    pool: JobPool,
    doc: Document,
    vp: Viewport,
    scroll_x: f32,
    scroll_mode: ScrollMode,
    renderer: Renderer,
    /// ビューのクライアント領域（ピクセル）
    view_px: (u32, u32),
    show_line_numbers: bool,
    index_posted: Arc<AtomicBool>,
    caret_visible: bool,
    focused: bool,
    overwrite: bool,
    composition: Option<Composition>,
    drag: Option<Drag>,
    /// WM_CHAR で届いたサロゲートペアの前半
    high_surrogate: Option<u16>,
}

pub(crate) fn create_accelerators() -> Result<HACCEL> {
    let ctrl = FVIRTKEY | FCONTROL;
    let ctrl_shift = FVIRTKEY | FCONTROL | FSHIFT;
    let accels = [
        (ctrl, b'N' as u16, ID_NEW),
        (ctrl, b'O' as u16, ID_OPEN),
        (ctrl, b'S' as u16, ID_SAVE),
        (ctrl_shift, b'S' as u16, ID_SAVE_AS),
        (ctrl, b'W' as u16, ID_CLOSE),
        (ctrl, b'Z' as u16, ID_UNDO),
        (ctrl, b'Y' as u16, ID_REDO),
        (ctrl_shift, b'Z' as u16, ID_REDO),
        (ctrl, b'X' as u16, ID_CUT),
        (ctrl, b'C' as u16, ID_COPY),
        (ctrl, b'V' as u16, ID_PASTE),
        (ctrl, b'A' as u16, ID_SELECT_ALL),
        (ctrl, b'G' as u16, ID_GOTO),
        (ctrl, VK_ADD.0, ID_ZOOM_IN),
        (ctrl, VK_OEM_PLUS.0, ID_ZOOM_IN),
        (ctrl, VK_SUBTRACT.0, ID_ZOOM_OUT),
        (ctrl, VK_OEM_MINUS.0, ID_ZOOM_OUT),
        (ctrl, b'0' as u16, ID_ZOOM_RESET),
        (ctrl, VK_NUMPAD0.0, ID_ZOOM_RESET),
    ]
    .map(|(fvirt, key, cmd)| ACCEL {
        fVirt: fvirt,
        key,
        cmd,
    });
    unsafe { CreateAcceleratorTableW(&accels) }
}

fn create_menu() -> Result<(HMENU, HMENU, HMENU)> {
    unsafe {
        let item = |menu: HMENU, id: u16, text: windows::core::PCWSTR| {
            AppendMenuW(menu, MF_STRING, id as usize, text)
        };
        let sep = |menu: HMENU| AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let bar = CreateMenu()?;

        let file = CreatePopupMenu()?;
        item(file, ID_NEW, w!("新規作成(&N)\tCtrl+N"))?;
        item(file, ID_OPEN, w!("開く(&O)...\tCtrl+O"))?;
        item(file, ID_SAVE, w!("上書き保存(&S)\tCtrl+S"))?;
        item(
            file,
            ID_SAVE_AS,
            w!("名前を付けて保存(&A)...\tCtrl+Shift+S"),
        )?;
        item(file, ID_CLOSE, w!("閉じる(&C)\tCtrl+W"))?;
        sep(file)?;
        item(file, ID_EXIT, w!("終了(&X)\tAlt+F4"))?;

        let edit = CreatePopupMenu()?;
        item(edit, ID_UNDO, w!("元に戻す(&U)\tCtrl+Z"))?;
        item(edit, ID_REDO, w!("やり直し(&R)\tCtrl+Y"))?;
        sep(edit)?;
        item(edit, ID_CUT, w!("切り取り(&T)\tCtrl+X"))?;
        item(edit, ID_COPY, w!("コピー(&C)\tCtrl+C"))?;
        item(edit, ID_PASTE, w!("貼り付け(&P)\tCtrl+V"))?;
        item(edit, ID_DELETE, w!("削除(&D)\tDel"))?;
        sep(edit)?;
        item(edit, ID_SELECT_ALL, w!("すべて選択(&A)\tCtrl+A"))?;

        let view = CreatePopupMenu()?;
        item(view, ID_GOTO, w!("行へ移動(&G)...\tCtrl+G"))?;
        sep(view)?;
        item(view, ID_ZOOM_IN, w!("拡大(&I)\tCtrl++"))?;
        item(view, ID_ZOOM_OUT, w!("縮小(&O)\tCtrl+-"))?;
        item(view, ID_ZOOM_RESET, w!("標準のサイズ(&R)\tCtrl+0"))?;
        sep(view)?;
        AppendMenuW(
            view,
            MF_STRING | MF_CHECKED,
            ID_LINE_NUMBERS as usize,
            w!("行番号(&L)"),
        )?;

        let help = CreatePopupMenu()?;
        item(help, ID_ABOUT, w!("バージョン情報(&A)"))?;
        AppendMenuW(bar, MF_POPUP, file.0 as usize, w!("ファイル(&F)"))?;
        AppendMenuW(bar, MF_POPUP, edit.0 as usize, w!("編集(&E)"))?;
        AppendMenuW(bar, MF_POPUP, view.0 as usize, w!("表示(&V)"))?;
        AppendMenuW(bar, MF_POPUP, help.0 as usize, w!("ヘルプ(&H)"))?;
        Ok((bar, edit, view))
    }
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

impl App {
    /// ウィンドウを作成してアプリ状態を初期化する。フレームウィンドウを返す。
    pub(crate) fn create(hinstance: HINSTANCE, initial_file: Option<PathBuf>) -> Result<HWND> {
        let (config, config_error) = Config::load();
        unsafe {
            let (menu, menu_edit, menu_view) = create_menu().context("create_menu")?;
            let frame = CreateWindowExW(
                WS_EX_ACCEPTFILES,
                FRAME_CLASS,
                w!("yyeditor"),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                1100,
                760,
                None,
                Some(menu),
                Some(hinstance),
                None,
            )
            .context("CreateWindowExW(frame)")?;
            let view = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                VIEW_CLASS,
                None,
                WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_HSCROLL,
                0,
                0,
                0,
                0,
                Some(frame),
                None,
                Some(hinstance),
                None,
            )
            .context("CreateWindowExW(view)")?;
            let status = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                STATUSCLASSNAMEW,
                None,
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SBARS_SIZEGRIP),
                0,
                0,
                0,
                0,
                Some(frame),
                Some(HMENU(ID_STATUS as isize as *mut _)),
                Some(hinstance),
                None,
            )
            .context("CreateWindowExW(status)")?;
            DragAcceptFiles(frame, true);

            let dpi = GetDpiForWindow(view).max(96);
            let renderer = Renderer::new(
                &config.editor.font_family,
                config.editor.font_size,
                config.editor.tab_width,
                config.colors.clone(),
                dpi,
            )?;
            let app = App {
                frame,
                view,
                status,
                menu_edit,
                menu_view,
                rows_cfg: RowConfig::new(config.view.max_row_bytes.max(256) as u64),
                show_line_numbers: config.view.line_numbers,
                config,
                pool: JobPool::new(0),
                doc: Document::new_empty(),
                vp: Viewport::default(),
                scroll_x: 0.0,
                scroll_mode: ScrollMode::Lines,
                renderer,
                view_px: (0, 0),
                index_posted: Arc::new(AtomicBool::new(false)),
                caret_visible: true,
                focused: false,
                overwrite: false,
                composition: None,
                drag: None,
                high_surrogate: None,
            };
            APP.with(|cell| *cell.borrow_mut() = Some(app));
            with_app(|a| {
                a.update_line_number_menu();
                a.layout_children();
                a.update_title();
                a.update_status();
                a.update_scrollbars();
            });

            let _ = ShowWindow(frame, SW_SHOWDEFAULT);
            let _ = SetFocus(Some(view));

            if let Some(e) = config_error {
                error_box(frame, &e.to_string());
            }
            if let Some(path) = initial_file {
                open_path(frame, path);
            }
            Ok(frame)
        }
    }

    // ---- 寸法 ------------------------------------------------------------

    fn page_rows(&self) -> usize {
        let h = self.renderer.px_to_dip(self.view_px.1 as f32);
        let lh = self.renderer.metrics().line_height.max(1.0);
        ((h / lh).floor() as usize).max(1)
    }

    fn text_origin_x(&self) -> f32 {
        self.renderer
            .text_origin_x(self.line_digits(), self.show_line_numbers)
    }

    fn text_area_width(&self) -> f32 {
        let w = self.renderer.px_to_dip(self.view_px.0 as f32);
        (w - self.text_origin_x()).max(0.0)
    }

    fn line_digits(&self) -> usize {
        let n = self.doc.snapshot().estimated_line_count();
        n.to_string().len()
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.view), None, false);
        }
    }

    fn notifier(&self) -> Notifier {
        let frame = self.frame.0 as isize;
        let posted = self.index_posted.clone();
        Arc::new(move || {
            if !posted.swap(true, Ordering::AcqRel) {
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(frame as *mut _)),
                        WM_APP_INDEX,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        })
    }

    // ---- ステータスバー・タイトル ----------------------------------------

    fn layout_children(&mut self) {
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(self.frame, &mut rc);
            SendMessageW(self.status, WM_SIZE, None, None);
            let mut src = RECT::default();
            let _ = GetWindowRect(self.status, &mut src);
            let sh = src.bottom - src.top;
            let w = rc.right - rc.left;
            let h = (rc.bottom - rc.top - sh).max(0);
            let _ = MoveWindow(self.view, 0, 0, w, h, true);
            // 位置 | サイズ | 文字コード | 改行コード | 挿入/上書き | 進捗
            let parts = [w - 640, w - 520, w - 380, w - 310, w - 250, -1].map(|x| x.max(0));
            SendMessageW(
                self.status,
                SB_SETPARTS,
                Some(WPARAM(parts.len())),
                Some(LPARAM(parts.as_ptr() as isize)),
            );
        }
    }

    fn set_status(&self, part: usize, text: &str) {
        let s = wide(text);
        unsafe {
            SendMessageW(
                self.status,
                SB_SETTEXTW,
                Some(WPARAM(part)),
                Some(LPARAM(s.as_ptr() as isize)),
            );
        }
    }

    fn update_title(&self) {
        let mark = if self.doc.is_modified() { "*" } else { "" };
        let title = format!("{mark}{} - yyeditor", self.doc.display_name());
        unsafe {
            let _ = SetWindowTextW(self.frame, &HSTRING::from(title));
        }
    }

    fn update_status(&self) {
        let snap = self.doc.snapshot();
        let sels = self.doc.selections();
        let head = sels.primary().head;
        let pos = snap.line_of_offset(head);
        let approx = if pos.exact { "" } else { "約 " };
        let col = motion::column_of(snap, head, 1 << 20)
            .map(|c| group_digits(c + 1))
            .unwrap_or_else(|| "-".into());
        let mut text = format!("  {approx}{} 行, {col} 列", group_digits(pos.line + 1));
        let selected: u64 = sels.iter().map(|s| s.end() - s.start()).sum();
        if sels.len() > 1 {
            text += &format!("  (カーソル {} 個)", sels.len());
        }
        if selected > 0 {
            text += &format!("  ({} バイト選択)", group_digits(selected));
        }
        self.set_status(0, &text);
        self.set_status(1, &format!("  {}", human_size(snap.len())));
        let enc = match self.doc.bom() {
            yy_io::Bom::Utf8 => "UTF-8 (BOM 付き)",
            yy_io::Bom::None => "UTF-8",
        };
        self.set_status(2, &format!("  {enc}"));
        self.set_status(3, &format!("  {}", self.doc.eol().label()));
        self.set_status(
            4,
            if self.overwrite {
                "  上書き"
            } else {
                "  挿入"
            },
        );
        let progress = match self.doc.indexing_progress() {
            Some(p) => format!("  行数を数えています… {:.0}%", p * 100.0),
            None => String::new(),
        };
        self.set_status(5, &progress);
    }

    fn update_scrollbars(&mut self) {
        let snap = self.doc.snapshot();
        let page = self.page_rows();
        let mut si = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL | SIF_DISABLENOSCROLL,
            ..Default::default()
        };
        match snap.line_count() {
            Some(lines) if lines <= self.config.view.line_scroll_limit => {
                self.scroll_mode = ScrollMode::Lines;
                si.nMin = 0;
                si.nMax = (lines as i32 - 1).max(0);
                si.nPage = page as u32;
                si.nPos = snap.line_of_offset(self.vp.top).line as i32;
            }
            _ => {
                self.scroll_mode = ScrollMode::Bytes;
                let len = snap.len().max(1);
                let rows = rows_from(snap, self.rows_cfg, self.vp.top, page);
                let visible = rows.last().map(|r| r.next - self.vp.top).unwrap_or(0);
                si.nMin = 0;
                si.nMax = SCROLL_RANGE - 1;
                si.nPage = ((visible as f64 / len as f64) * SCROLL_RANGE as f64)
                    .clamp(1.0, SCROLL_RANGE as f64) as u32;
                si.nPos = (self.vp.fraction(snap) * SCROLL_RANGE as f64) as i32;
            }
        }
        unsafe {
            SetScrollInfo(self.view, SB_VERT, &si, true);
        }
        let area = self.text_area_width();
        let hsi = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL | SIF_DISABLENOSCROLL,
            nMin: 0,
            nMax: (self.renderer.max_text_width + self.renderer.metrics().char_width * 4.0) as i32,
            nPage: area as u32,
            nPos: self.scroll_x as i32,
            nTrackPos: 0,
        };
        unsafe {
            SetScrollInfo(self.view, SB_HORZ, &hsi, true);
        }
    }

    fn update_menu_state(&self) {
        let set = |id: u16, on: bool| unsafe {
            let flag = if on { MF_ENABLED } else { MF_GRAYED };
            let _ = EnableMenuItem(self.menu_edit, id as u32, MF_BYCOMMAND | flag);
        };
        let has_sel = !self.doc.selections().all_empty();
        set(ID_UNDO, self.doc.can_undo());
        set(ID_REDO, self.doc.can_redo());
        set(ID_CUT, has_sel);
        set(ID_COPY, has_sel);
        set(ID_DELETE, has_sel);
    }

    // ---- スクロール ------------------------------------------------------

    fn after_scroll(&mut self) {
        self.update_scrollbars();
        self.update_status();
        self.invalidate();
    }

    fn scroll_rows(&mut self, delta: i64) {
        let page = self.page_rows();
        if self
            .vp
            .scroll_rows(self.doc.snapshot(), self.rows_cfg, delta, page)
        {
            self.after_scroll();
        }
    }

    fn scroll_to_offset(&mut self, offset: u64) {
        let page = self.page_rows();
        self.vp
            .scroll_to_offset(self.doc.snapshot(), self.rows_cfg, offset, page);
        self.after_scroll();
    }

    fn max_scroll_x(&self) -> f32 {
        (self.renderer.max_text_width + self.renderer.metrics().char_width * 4.0
            - self.text_area_width())
        .max(0.0)
    }

    fn scroll_horizontal(&mut self, x: f32) {
        let x = x.clamp(0.0, self.max_scroll_x().max(self.scroll_x));
        if x != self.scroll_x {
            self.scroll_x = x.max(0.0);
            self.update_scrollbars();
            self.invalidate();
        }
    }

    fn on_vscroll(&mut self, code: u32) {
        let page = self.page_rows() as i64;
        match SCROLLBAR_COMMAND(code as i32) {
            SB_LINEUP => self.scroll_rows(-1),
            SB_LINEDOWN => self.scroll_rows(1),
            SB_PAGEUP => self.scroll_rows(-(page - 1).max(1)),
            SB_PAGEDOWN => self.scroll_rows((page - 1).max(1)),
            SB_TOP => self.scroll_to_offset(0),
            SB_BOTTOM => self.scroll_to_offset(u64::MAX),
            SB_THUMBTRACK | SB_THUMBPOSITION => {
                let mut si = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_TRACKPOS,
                    ..Default::default()
                };
                unsafe {
                    let _ = GetScrollInfo(self.view, SB_VERT, &mut si);
                }
                let snap = self.doc.snapshot().clone();
                match self.scroll_mode {
                    ScrollMode::Lines => {
                        if let LineLookup::Found(off) = snap.line_start(si.nTrackPos as u64, false)
                        {
                            self.scroll_to_offset(off);
                        }
                    }
                    ScrollMode::Bytes => {
                        let f = si.nTrackPos as f64 / SCROLL_RANGE as f64;
                        let rows = self.page_rows();
                        self.vp.scroll_to_fraction(&snap, self.rows_cfg, f, rows);
                        // ドラッグ中はつまみ位置を動かさない（位置の再計算で揺れるため）
                        self.update_status();
                        self.invalidate();
                    }
                }
            }
            _ => {}
        }
    }

    fn on_hscroll(&mut self, code: u32) {
        let cw = self.renderer.metrics().char_width;
        let area = self.text_area_width();
        match SCROLLBAR_COMMAND(code as i32) {
            SB_LINELEFT => self.scroll_horizontal(self.scroll_x - cw * 4.0),
            SB_LINERIGHT => self.scroll_horizontal(self.scroll_x + cw * 4.0),
            SB_PAGELEFT => self.scroll_horizontal(self.scroll_x - area * 0.8),
            SB_PAGERIGHT => self.scroll_horizontal(self.scroll_x + area * 0.8),
            SB_LEFT => self.scroll_horizontal(0.0),
            SB_RIGHT => self.scroll_horizontal(f32::MAX),
            SB_THUMBTRACK | SB_THUMBPOSITION => {
                let mut si = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_TRACKPOS,
                    ..Default::default()
                };
                unsafe {
                    let _ = GetScrollInfo(self.view, SB_HORZ, &mut si);
                }
                self.scroll_horizontal(si.nTrackPos as f32);
            }
            _ => {}
        }
    }

    fn on_wheel(&mut self, delta: i16, horizontal: bool) {
        if key_down(VK_CONTROL) && !horizontal {
            let step = if delta > 0 { 1.0 } else { -1.0 };
            self.zoom(self.renderer.font_size_pt() + step);
            return;
        }
        let notches = delta as f32 / WHEEL_DELTA as f32;
        if horizontal || key_down(VK_SHIFT) {
            let sign = if horizontal { 1.0 } else { -1.0 };
            let dx = sign * notches * self.renderer.metrics().char_width * 8.0;
            self.scroll_horizontal(self.scroll_x + dx);
        } else {
            let rows = -(notches * self.config.view.wheel_lines as f32).round() as i64;
            self.scroll_rows(rows);
        }
    }

    fn zoom(&mut self, pt: f32) {
        if let Err(e) = self.renderer.set_font_size(pt) {
            error_box(self.frame, &format!("フォントを変更できません: {e}"));
            return;
        }
        let page = self.page_rows();
        self.vp.clamp(self.doc.snapshot(), self.rows_cfg, page);
        self.after_scroll();
    }

    fn update_line_number_menu(&self) {
        let flag = if self.show_line_numbers {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        unsafe {
            CheckMenuItem(
                self.menu_view,
                ID_LINE_NUMBERS as u32,
                (MF_BYCOMMAND | flag).0,
            );
        }
    }

    // ---- キャレット ------------------------------------------------------

    /// 描画キャッシュを文書の版に合わせる（ヒットテストの前に呼ぶ）。
    fn sync_renderer(&mut self) {
        self.renderer.set_version(self.doc.version());
    }

    fn row_of(&self, offset: u64) -> Row {
        let snap = self.doc.snapshot();
        row_at(
            snap,
            self.rows_cfg,
            row_containing(snap, self.rows_cfg, offset),
        )
    }

    /// キャレットの点滅をやり直す（操作直後は表示する）。
    fn reset_blink(&mut self) {
        self.caret_visible = true;
        if self.focused {
            unsafe {
                let ms = GetCaretBlinkTime();
                if ms != u32::MAX && ms > 0 {
                    SetTimer(Some(self.view), TIMER_BLINK, ms, None);
                }
            }
        }
    }

    /// 主キャレットが画面内に入るように縦横にスクロールする。
    fn ensure_caret_visible(&mut self) {
        self.sync_renderer();
        let head = self.doc.selections().primary().head;
        let page = self.page_rows();
        self.vp
            .ensure_visible(self.doc.snapshot(), self.rows_cfg, head, page);
        let row = self.row_of(head);
        let x = self.renderer.caret_x(&row, head);
        let area = self.text_area_width();
        let margin = (self.renderer.metrics().char_width * 4.0).min(area / 3.0);
        if x < self.scroll_x + margin {
            self.scroll_x = (x - margin).max(0.0);
        } else if x > self.scroll_x + area - margin {
            self.scroll_x = x - area + margin;
        }
    }

    /// キャレットを動かした後の共通処理。
    fn after_move(&mut self) {
        self.reset_blink();
        self.ensure_caret_visible();
        self.update_scrollbars();
        self.update_status();
        self.update_ime_position();
        self.invalidate();
    }

    /// 内容を変更した後の共通処理。
    fn after_edit(&mut self) {
        if !self.doc.snapshot().is_fully_indexed() {
            let n = self.notifier();
            self.doc.maintain_indexing(&self.pool, n);
        }
        self.after_move();
        self.update_title();
    }

    /// 行 `rows` 行分上下に移動した位置と、その水平位置。
    fn vertical_target(
        &mut self,
        snap: &Snapshot,
        offset: u64,
        goal_x: Option<f32>,
        rows: i64,
    ) -> (u64, f32) {
        let cfg = self.rows_cfg;
        let start = row_containing(snap, cfg, offset);
        let x = match goal_x {
            Some(x) => x,
            None => {
                let row = row_at(snap, cfg, start);
                self.renderer.caret_x(&row, offset)
            }
        };
        let mut r = start;
        for _ in 0..rows.unsigned_abs() {
            let n = if rows > 0 {
                next_row_start(snap, cfg, r)
            } else {
                prev_row_start(snap, cfg, r)
            };
            match n {
                Some(n) => r = n,
                None => return (if rows > 0 { snap.len() } else { 0 }, x),
            }
        }
        let target = row_at(snap, cfg, r);
        (self.renderer.hit_test(&target, x), x)
    }

    fn move_vertical(&mut self, rows: i64, extend: bool) {
        self.sync_renderer();
        let snap = self.doc.snapshot().clone();
        let sels = self.doc.selections().clone();
        let moved = sels.map(|s| {
            let (head, x) = self.vertical_target(&snap, s.head, s.goal_x, rows);
            let mut n = if extend {
                Selection::new(s.anchor, head)
            } else {
                Selection::caret(head)
            };
            n.goal_x = Some(x);
            n
        });
        self.doc.set_selections(moved);
        self.after_move();
    }

    /// 主カーソルの上下の行にカーソルを追加する（Ctrl+Alt+↑↓）。
    fn add_caret_vertical(&mut self, rows: i64) {
        self.sync_renderer();
        let snap = self.doc.snapshot().clone();
        let mut sels = self.doc.selections().clone();
        let p = *sels.primary();
        let (head, x) = self.vertical_target(&snap, p.head, p.goal_x, rows);
        let mut c = Selection::caret(head);
        c.goal_x = Some(x);
        sels.add(c);
        self.doc.set_selections(sels);
        self.after_move();
    }

    fn move_horizontal(&mut self, dir: i32, extend: bool, word: bool) {
        self.doc.move_carets(extend, |snap, s| {
            if !extend && !s.is_empty() {
                if dir < 0 { s.start() } else { s.end() }
            } else {
                match (dir < 0, word) {
                    (true, false) => motion::prev_grapheme(snap, s.head),
                    (false, false) => motion::next_grapheme(snap, s.head),
                    (true, true) => motion::prev_word(snap, s.head),
                    (false, true) => motion::next_word(snap, s.head),
                }
            }
        });
        self.after_move();
    }

    // ---- キーボード -------------------------------------------------------

    fn on_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        let ctrl = key_down(VK_CONTROL);
        let shift = key_down(VK_SHIFT);
        let alt = key_down(VK_MENU);
        let page = self.page_rows() as i64;
        match vk {
            VK_LEFT => self.move_horizontal(-1, shift, ctrl),
            VK_RIGHT => self.move_horizontal(1, shift, ctrl),
            VK_UP if ctrl && alt => self.add_caret_vertical(-1),
            VK_DOWN if ctrl && alt => self.add_caret_vertical(1),
            VK_UP if ctrl => self.scroll_rows(-1),
            VK_DOWN if ctrl => self.scroll_rows(1),
            VK_UP => self.move_vertical(-1, shift),
            VK_DOWN => self.move_vertical(1, shift),
            VK_PRIOR | VK_NEXT => {
                let d = if vk == VK_PRIOR {
                    -(page - 1).max(1)
                } else {
                    (page - 1).max(1)
                };
                let size = self.page_rows();
                self.vp
                    .scroll_rows(self.doc.snapshot(), self.rows_cfg, d, size);
                self.move_vertical(d, shift);
            }
            VK_HOME => {
                self.doc.move_carets(shift, |snap, s| {
                    if ctrl {
                        0
                    } else {
                        motion::smart_home(snap, s.head)
                    }
                });
                self.after_move();
            }
            VK_END => {
                self.doc.move_carets(shift, |snap, s| {
                    if ctrl {
                        snap.len()
                    } else {
                        motion::line_end(snap, s.head)
                    }
                });
                self.after_move();
            }
            VK_BACK => {
                let changed = if ctrl {
                    self.doc.delete_word_backward()
                } else {
                    self.doc.delete_backward()
                };
                if changed {
                    self.after_edit();
                }
            }
            VK_DELETE if shift => return false,
            VK_DELETE => {
                let changed = if ctrl {
                    self.doc.delete_word_forward()
                } else {
                    self.doc.delete_forward()
                };
                if changed {
                    self.after_edit();
                }
            }
            VK_INSERT if !ctrl && !shift => {
                self.overwrite = !self.overwrite;
                self.update_status();
                self.invalidate();
            }
            VK_ESCAPE => {
                let mut sels = self.doc.selections().clone();
                if sels.len() > 1 {
                    sels.collapse_to_primary();
                } else {
                    let h = sels.primary().head;
                    sels = SelectionSet::single(Selection::caret(h));
                }
                self.doc.set_selections(sels);
                self.after_move();
            }
            _ => return false,
        }
        true
    }

    fn on_char(&mut self, code: u16) {
        // Ctrl+英字などは制御文字として届くので無視する（AltGr = Ctrl+Alt は通す）
        if key_down(VK_CONTROL) && !key_down(VK_MENU) {
            return;
        }
        let text = match code {
            0x0D => {
                if self.doc.insert_newline(true) {
                    self.after_edit();
                }
                return;
            }
            0x09 => "\t".to_owned(),
            0xD800..=0xDBFF => {
                self.high_surrogate = Some(code);
                return;
            }
            0xDC00..=0xDFFF => match self.high_surrogate.take() {
                Some(hi) => String::from_utf16_lossy(&[hi, code]),
                None => return,
            },
            c if c < 0x20 || c == 0x7F => return,
            c => String::from_utf16_lossy(&[c]),
        };
        if self.doc.insert_text(&text, self.overwrite) {
            self.after_edit();
        }
    }

    // ---- マウス ----------------------------------------------------------

    /// ビューのクライアント座標（ピクセル）に最も近い文書の位置。
    fn offset_at_point(&mut self, x_px: i32, y_px: i32) -> u64 {
        self.sync_renderer();
        let x = self.renderer.px_to_dip(x_px as f32);
        let y = self.renderer.px_to_dip(y_px as f32);
        let lh = self.renderer.metrics().line_height.max(1.0);
        let snap = self.doc.snapshot().clone();
        let cfg = self.rows_cfg;
        let row_start = if y < 0.0 {
            prev_row_start(&snap, cfg, self.vp.top).unwrap_or(0)
        } else {
            let idx = (y / lh).floor() as usize;
            let rows = rows_from(&snap, cfg, self.vp.top, idx + 1);
            match rows.last() {
                Some(r) => r.start,
                None => return snap.len(),
            }
        };
        let row = row_at(&snap, cfg, row_start);
        let tx = x - self.text_origin_x() + self.scroll_x;
        self.renderer.hit_test(&row, tx)
    }

    fn on_lbutton_down(&mut self, x: i32, y: i32, double: bool) {
        let pos = self.offset_at_point(x, y);
        let shift = key_down(VK_SHIFT);
        let ctrl = key_down(VK_CONTROL);
        let mut sels = self.doc.selections().clone();
        if double {
            let r = motion::word_range(self.doc.snapshot(), pos);
            self.doc
                .set_selections(SelectionSet::single(Selection::new(r.start, r.end)));
            self.drag = None;
        } else if shift {
            let anchor = sels.primary().anchor;
            self.doc
                .set_selections(SelectionSet::single(Selection::new(anchor, pos)));
            self.drag = Some(Drag { anchor, base: None });
        } else if ctrl {
            let base = sels.clone();
            sels.add(Selection::caret(pos));
            self.doc.set_selections(sels);
            self.drag = Some(Drag {
                anchor: pos,
                base: Some(base),
            });
        } else {
            self.doc
                .set_selections(SelectionSet::single(Selection::caret(pos)));
            self.drag = Some(Drag {
                anchor: pos,
                base: None,
            });
        }
        self.after_move();
    }

    fn on_mouse_move(&mut self, x: i32, y: i32) {
        let Some(drag) = &self.drag else {
            return;
        };
        let (anchor, base) = (drag.anchor, drag.base.clone());
        // ビューの外に出たら 1 行ずつスクロールする
        let h = self.view_px.1 as i32;
        if y < 0 {
            self.scroll_rows(-1);
        } else if y > h {
            self.scroll_rows(1);
        }
        let pos = self.offset_at_point(x, y);
        let sel = Selection::new(anchor, pos);
        let sels = match base {
            Some(mut b) => {
                b.add(sel);
                b
            }
            None => SelectionSet::single(sel),
        };
        if &sels != self.doc.selections() {
            self.doc.set_selections(sels);
            self.after_move();
        }
    }

    // ---- IME -------------------------------------------------------------

    /// 変換ウィンドウ・候補ウィンドウを主キャレットの位置に合わせる。
    fn update_ime_position(&mut self) {
        self.sync_renderer();
        let head = self.doc.selections().primary().head;
        let page = self.page_rows();
        let rows = rows_from(self.doc.snapshot(), self.rows_cfg, self.vp.top, page + 1);
        let Some(i) = rows.iter().position(|r| r.shows_caret(head)) else {
            return;
        };
        let lh = self.renderer.metrics().line_height;
        let x = self.text_origin_x() + self.renderer.caret_x(&rows[i], head) - self.scroll_x;
        let y = i as f32 * lh;
        let scale = |v: f32| (v * self.renderer.px_to_dip(1.0).recip()).round() as i32;
        ime::set_position(self.view, scale(x), scale(y), scale(lh));
    }

    fn on_composition(&mut self, update: ime::CompositionUpdate) {
        let mut edited = false;
        if let Some(result) = update.result {
            self.composition = None;
            edited |= self.doc.insert_text(&result, self.overwrite);
        }
        if let Some((text, cursor)) = update.composing {
            if text.is_empty() {
                self.composition = None;
            } else {
                // 変換を始めたときに選択範囲があれば削除する
                if self.composition.is_none() && !self.doc.selections().all_empty() {
                    edited |= self.doc.delete_selection(EditKind::Other);
                }
                self.composition = Some(Composition {
                    offset: self.doc.selections().primary().head,
                    text,
                    cursor,
                });
            }
        }
        if edited {
            self.after_edit();
        } else {
            self.after_move();
        }
    }

    // ---- 描画 ------------------------------------------------------------

    fn paint(&mut self) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(self.view, &mut ps);
            let snap = self.doc.snapshot();
            let page = self.page_rows();
            let rows = rows_from(snap, self.rows_cfg, self.vp.top, page + 1);
            let first = snap.line_of_offset(self.vp.top);
            // 表示範囲に関係する選択・キャレットだけを渡す
            let (lo, hi) = (
                self.vp.top,
                rows.last().map(|r| r.next).unwrap_or(self.vp.top),
            );
            let sels = self.doc.selections();
            let selections: Vec<_> = sels
                .iter()
                .filter(|s| !s.is_empty() && s.end() >= lo && s.start() <= hi)
                .map(|s| s.range())
                .collect();
            let carets: Vec<u64> = sels
                .iter()
                .map(|s| s.head)
                .filter(|h| (lo..=hi).contains(h))
                .collect();
            let frame = Frame {
                version: self.doc.version(),
                rows: &rows,
                first_line: first.line,
                line_exact: first.exact,
                line_digits: self.line_digits(),
                show_line_numbers: self.show_line_numbers,
                scroll_x: self.scroll_x,
                selections: &selections,
                carets: &carets,
                caret_visible: self.focused && self.caret_visible,
                overwrite: self.overwrite,
                composition: self.composition.as_ref(),
            };
            let before = self.renderer.max_text_width;
            let result = self
                .renderer
                .draw(self.view, self.view_px.0, self.view_px.1, &frame);
            let _ = EndPaint(self.view, &ps);
            match result {
                Ok(true) => {}
                // 描画ターゲットを作り直したので描き直す
                Ok(false) => self.invalidate(),
                Err(e) => eprintln!("draw failed: {}", crate::util::describe_error(&e)),
            }
            if self.renderer.max_text_width != before {
                // 描画後にスクロールバーを更新すると WM_SIZE が再入し得るため後回しにする
                let _ = PostMessageW(Some(self.view), WM_APP_SCROLLBARS, WPARAM(0), LPARAM(0));
            }
        }
    }

    fn resize_view(&mut self, w: u32, h: u32) {
        self.view_px = (w, h);
        self.renderer.resize(w, h);
        let page = self.page_rows();
        self.vp.clamp(self.doc.snapshot(), self.rows_cfg, page);
        self.update_scrollbars();
        self.invalidate();
    }

    // ---- 文書の切り替え・保存 -----------------------------------------------

    fn set_document(&mut self, doc: Document) {
        ime::cancel(self.view);
        self.composition = None;
        self.drag = None;
        self.doc = doc;
        let n = self.notifier();
        self.doc.start_indexing(&self.pool, n);
        self.vp = Viewport::default();
        self.scroll_x = 0.0;
        self.renderer.clear_cache();
        self.update_title();
        self.after_move();
    }

    fn open(&mut self, path: PathBuf) -> std::result::Result<(), String> {
        let doc = Document::open(&path).map_err(|e| format!("{}\n\n{e}", path.display()))?;
        self.set_document(doc);
        Ok(())
    }

    fn save_to(&mut self, path: Option<PathBuf>) -> std::result::Result<(), String> {
        let result = unsafe {
            let old = SetCursor(LoadCursorW(None, IDC_WAIT).ok());
            let r = match &path {
                Some(p) => self.doc.save_as(p),
                None => self.doc.save(),
            };
            SetCursor(Some(old));
            r
        };
        let shown = path
            .as_deref()
            .or(self.doc.path())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        result.map_err(|e| format!("{shown}\n\n{e}"))?;
        let n = self.notifier();
        self.doc.maintain_indexing(&self.pool, n);
        self.renderer.clear_cache();
        self.update_title();
        self.after_move();
        Ok(())
    }

    fn on_index_progress(&mut self) {
        self.index_posted.store(false, Ordering::Release);
        if self.doc.poll_indexing() {
            if self.doc.indexing_progress().is_none() && !self.doc.snapshot().is_fully_indexed() {
                // 編集で分割されたピースなど、数え残しがあれば続けて数える
                let n = self.notifier();
                self.doc.maintain_indexing(&self.pool, n);
            }
            self.update_scrollbars();
            self.update_status();
            self.invalidate();
        }
    }

    /// 行へ移動。行数が未確定の範囲はその場で数える。
    fn goto_line(&mut self, line: u64) {
        let snap = self.doc.snapshot().clone();
        let lookup = match snap.line_start(line - 1, false) {
            LineLookup::NotIndexed => unsafe {
                let old = SetCursor(LoadCursorW(None, IDC_WAIT).ok());
                let r = snap.line_start(line - 1, true);
                SetCursor(Some(old));
                r
            },
            other => other,
        };
        match lookup {
            LineLookup::Found(off) => {
                self.doc
                    .set_selections(SelectionSet::single(Selection::caret(off)));
                self.scroll_to_offset(off);
                self.after_move();
            }
            _ => info_box(
                self.frame,
                &format!("{} 行目はありません。", group_digits(line)),
            ),
        }
    }

    fn copy_selection(&self) -> std::result::Result<Option<String>, u64> {
        self.doc.selected_text(MAX_CLIPBOARD_BYTES)
    }
}

// ---- コマンド（モーダル UI を伴うため状態の借用の外で実行する） ---------------

/// 変更を保存するか確認する。続行してよければ `true`。
fn confirm_discard(hwnd: HWND) -> bool {
    let Some((modified, name)) = with_app(|a| (a.doc.is_modified(), a.doc.display_name())) else {
        return false;
    };
    if !modified {
        return true;
    }
    let r = unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(format!("「{name}」への変更を保存しますか？")),
            &HSTRING::from("yyeditor"),
            MB_YESNOCANCEL | MB_ICONWARNING,
        )
    };
    match r {
        IDYES => cmd_save(hwnd, false),
        IDNO => true,
        _ => false,
    }
}

fn open_path(hwnd: HWND, path: PathBuf) {
    if let Some(Err(msg)) = with_app(|a| a.open(path)) {
        error_box(hwnd, &format!("ファイルを開けません。\n{msg}"));
    }
}

/// 保存する。`as_new` または名前がなければ保存先を尋ねる。保存できたら `true`。
fn cmd_save(hwnd: HWND, as_new: bool) -> bool {
    let Some(current) = with_app(|a| a.doc.path().map(|p| p.to_owned())) else {
        return false;
    };
    let target = if as_new || current.is_none() {
        match show_save_dialog(hwnd, current.as_deref()) {
            Some(p) => Some(p),
            None => return false,
        }
    } else {
        None
    };
    match with_app(|a| a.save_to(target)) {
        Some(Ok(())) => true,
        Some(Err(msg)) => {
            error_box(hwnd, &format!("保存できませんでした。\n{msg}"));
            false
        }
        None => false,
    }
}

fn show_open_dialog(owner: HWND) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dialog.Show(Some(owner)).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        path.map(PathBuf::from)
    }
}

fn show_save_dialog(owner: HWND, current: Option<&std::path::Path>) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = [
            COMDLG_FILTERSPEC {
                pszName: w!("すべてのファイル (*.*)"),
                pszSpec: w!("*.*"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("テキスト ファイル (*.txt)"),
                pszSpec: w!("*.txt"),
            },
        ];
        let _ = dialog.SetFileTypes(&filters);
        let name = current
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "無題.txt".to_owned());
        let _ = dialog.SetFileName(&HSTRING::from(name));
        dialog.Show(Some(owner)).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        path.map(PathBuf::from)
    }
}

fn cmd_copy(hwnd: HWND, cut: bool) {
    let Some(sel) = with_app(|a| a.copy_selection()) else {
        return;
    };
    match sel {
        Ok(Some(text)) => {
            if !clipboard::set_text(hwnd, &text) {
                error_box(hwnd, "クリップボードにコピーできませんでした。");
                return;
            }
            if cut {
                with_app(|a| {
                    if a.doc.delete_selection(EditKind::Cut) {
                        a.after_edit();
                    }
                });
            }
        }
        Ok(None) => {}
        Err(n) => info_box(
            hwnd,
            &format!(
                "選択範囲が大きすぎるためコピーできません（{}）。",
                human_size(n)
            ),
        ),
    }
}

fn on_command(hwnd: HWND, id: u16) {
    match id {
        ID_NEW => {
            if confirm_discard(hwnd) {
                with_app(|a| a.set_document(Document::new_empty()));
            }
        }
        ID_OPEN => {
            if confirm_discard(hwnd)
                && let Some(p) = show_open_dialog(hwnd)
            {
                open_path(hwnd, p);
            }
        }
        ID_SAVE => {
            cmd_save(hwnd, false);
        }
        ID_SAVE_AS => {
            cmd_save(hwnd, true);
        }
        ID_CLOSE => {
            if confirm_discard(hwnd) {
                with_app(|a| a.set_document(Document::new_empty()));
            }
        }
        ID_EXIT => unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        ID_UNDO => {
            with_app(|a| {
                if a.doc.undo() {
                    a.composition = None;
                    a.after_edit();
                }
            });
        }
        ID_REDO => {
            with_app(|a| {
                if a.doc.redo() {
                    a.composition = None;
                    a.after_edit();
                }
            });
        }
        ID_CUT => cmd_copy(hwnd, true),
        ID_COPY => cmd_copy(hwnd, false),
        ID_PASTE => {
            if let Some(text) = clipboard::get_text(hwnd) {
                with_app(|a| {
                    if a.doc.paste(&text) {
                        a.after_edit();
                    }
                });
            }
        }
        ID_DELETE => {
            with_app(|a| {
                if a.doc.delete_selection(EditKind::Other) || a.doc.delete_forward() {
                    a.after_edit();
                }
            });
        }
        ID_SELECT_ALL => {
            with_app(|a| {
                a.doc.select_all();
                a.after_move();
            });
        }
        ID_GOTO => {
            let Some((current, total)) = with_app(|a| {
                let snap = a.doc.snapshot();
                let cur = snap.line_of_offset(a.doc.selections().primary().head).line + 1;
                let total = match snap.line_count() {
                    Some(n) => format!("1 〜 {}", group_digits(n)),
                    None => format!("1 〜 約 {}", group_digits(snap.estimated_line_count())),
                };
                (cur, total)
            }) else {
                return;
            };
            let prompt = format!("行番号 ({total}):");
            if let Some(line) = crate::goto::prompt_line(hwnd, &prompt, current) {
                with_app(|a| a.goto_line(line));
            }
        }
        ID_ZOOM_IN => {
            with_app(|a| a.zoom(a.renderer.font_size_pt() + 1.0));
        }
        ID_ZOOM_OUT => {
            with_app(|a| a.zoom(a.renderer.font_size_pt() - 1.0));
        }
        ID_ZOOM_RESET => {
            with_app(|a| a.zoom(a.config.editor.font_size));
        }
        ID_LINE_NUMBERS => {
            with_app(|a| {
                a.show_line_numbers = !a.show_line_numbers;
                a.update_line_number_menu();
                a.after_scroll();
            });
        }
        ID_ABOUT => info_box(
            hwnd,
            &format!(
                "yyeditor {}\n\n巨大ファイル対応の軽量テキストエディタ",
                env!("CARGO_PKG_VERSION")
            ),
        ),
        _ => {}
    }
}

fn dropped_file(hdrop: HDROP) -> Option<PathBuf> {
    unsafe {
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        let result = if count > 0 {
            let len = DragQueryFileW(hdrop, 0, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(hdrop, 0, Some(&mut buf));
            Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
        } else {
            None
        };
        DragFinish(hdrop);
        result
    }
}

pub(crate) extern "system" fn frame_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_SIZE => {
            if with_app(|a| a.layout_children()).is_none() {
                // 再入中なら後でやり直す
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_APP_RELAYOUT, WPARAM(0), LPARAM(0));
                }
            }
            LRESULT(0)
        }
        WM_APP_RELAYOUT => {
            with_app(|a| a.layout_children());
            LRESULT(0)
        }
        WM_SETFOCUS => {
            if let Some(view) = with_app(|a| a.view) {
                unsafe {
                    let _ = SetFocus(Some(view));
                }
            }
            LRESULT(0)
        }
        WM_INITMENUPOPUP => {
            with_app(|a| a.update_menu_state());
            LRESULT(0)
        }
        WM_COMMAND => {
            on_command(hwnd, loword(wparam.0) as u16);
            LRESULT(0)
        }
        WM_DROPFILES => {
            if let Some(p) = dropped_file(HDROP(wparam.0 as *mut _))
                && confirm_discard(hwnd)
            {
                open_path(hwnd, p);
            }
            LRESULT(0)
        }
        WM_APP_INDEX => {
            with_app(|a| a.on_index_progress());
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = hiword(wparam.0);
            unsafe {
                let rc = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    rc.left,
                    rc.top,
                    rc.right - rc.left,
                    rc.bottom - rc.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            with_app(|a| {
                a.renderer.set_dpi(dpi);
                a.renderer.clear_cache();
                a.after_scroll();
            });
            LRESULT(0)
        }
        WM_CLOSE => {
            if confirm_discard(hwnd) {
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
        _ => default_proc(hwnd, msg, wparam, lparam),
    }
}

fn point_of(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

pub(crate) extern "system" fn view_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            if with_app(|a| a.paint()).is_none() {
                // 状態を借用中（モーダルループ中など）は描画せず、後で再描画する
                unsafe {
                    let mut ps = PAINTSTRUCT::default();
                    BeginPaint(hwnd, &mut ps);
                    let _ = EndPaint(hwnd, &ps);
                    let _ = PostMessageW(Some(hwnd), WM_APP_REPAINT, WPARAM(0), LPARAM(0));
                }
            }
            LRESULT(0)
        }
        WM_APP_REPAINT => {
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_APP_SCROLLBARS => {
            with_app(|a| a.update_scrollbars());
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_SIZE => {
            let (w, h) = (loword(lparam.0 as usize), hiword(lparam.0 as usize));
            if with_app(|a| a.resize_view(w, h)).is_none() {
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_APP_RESIZE, wparam, lparam);
                }
            }
            LRESULT(0)
        }
        WM_APP_RESIZE => {
            let (w, h) = (loword(lparam.0 as usize), hiword(lparam.0 as usize));
            with_app(|a| a.resize_view(w, h));
            LRESULT(0)
        }
        WM_SETFOCUS => {
            with_app(|a| {
                a.focused = true;
                a.reset_blink();
                a.update_ime_position();
                a.invalidate();
            });
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_BLINK);
            }
            with_app(|a| {
                a.focused = false;
                a.drag = None;
                a.invalidate();
            });
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_BLINK => {
            with_app(|a| {
                a.caret_visible = !a.caret_visible;
                a.invalidate();
            });
            LRESULT(0)
        }
        WM_VSCROLL => {
            with_app(|a| a.on_vscroll(loword(wparam.0)));
            LRESULT(0)
        }
        WM_HSCROLL => {
            with_app(|a| a.on_hscroll(loword(wparam.0)));
            LRESULT(0)
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = hiword(wparam.0) as u16 as i16;
            with_app(|a| a.on_wheel(delta, msg == WM_MOUSEHWHEEL));
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let handled = with_app(|a| a.on_key(VIRTUAL_KEY(wparam.0 as u16))).unwrap_or(false);
            if handled {
                LRESULT(0)
            } else {
                // Shift+Delete / Ctrl+Insert / Shift+Insert（切り取り・コピー・貼り付け）
                let vk = VIRTUAL_KEY(wparam.0 as u16);
                let parent = unsafe { GetParent(hwnd).unwrap_or_default() };
                let cmd = match vk {
                    VK_DELETE if key_down(VK_SHIFT) => Some(ID_CUT),
                    VK_INSERT if key_down(VK_CONTROL) => Some(ID_COPY),
                    VK_INSERT if key_down(VK_SHIFT) => Some(ID_PASTE),
                    _ => None,
                };
                match cmd {
                    Some(c) => {
                        on_command(parent, c);
                        LRESULT(0)
                    }
                    None => default_proc(hwnd, msg, wparam, lparam),
                }
            }
        }
        WM_CHAR => {
            with_app(|a| a.on_char(wparam.0 as u16));
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            unsafe {
                let _ = SetFocus(Some(hwnd));
                SetCapture(hwnd);
            }
            let (x, y) = point_of(lparam);
            with_app(|a| a.on_lbutton_down(x, y, msg == WM_LBUTTONDBLCLK));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = point_of(lparam);
            with_app(|a| a.on_mouse_move(x, y));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            with_app(|a| a.drag = None);
            LRESULT(0)
        }
        WM_IME_SETCONTEXT => {
            // システムの変換ウィンドウを出さず、変換中の文字列は自分で描く
            let lp = LPARAM(lparam.0 & !(ISC_SHOWUICOMPOSITIONWINDOW as isize));
            default_proc(hwnd, msg, wparam, lp)
        }
        WM_IME_STARTCOMPOSITION => {
            with_app(|a| a.update_ime_position());
            LRESULT(0)
        }
        WM_IME_COMPOSITION => {
            let update = ime::read_composition(hwnd, lparam.0 as u32);
            with_app(|a| a.on_composition(update));
            LRESULT(0)
        }
        WM_IME_ENDCOMPOSITION => {
            with_app(|a| {
                a.composition = None;
                a.invalidate();
            });
            LRESULT(0)
        }
        _ => default_proc(hwnd, msg, wparam, lparam),
    }
}

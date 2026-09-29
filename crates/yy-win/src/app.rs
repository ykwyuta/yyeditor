//! アプリケーション状態とウィンドウプロシージャ。

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Controls::{SB_SETPARTS, SB_SETTEXTW, STATUSCLASSNAMEW, SetScrollInfo};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{
    DragAcceptFiles, DragFinish, DragQueryFileW, FileOpenDialog, HDROP, IFileOpenDialog,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Result, w};
use yy_buffer::LineLookup;
use yy_config::Config;
use yy_core::Document;
use yy_jobs::JobPool;
use yy_layout::{RowConfig, Viewport, rows_from};

use crate::render::{Frame, Renderer};
use crate::util::{Context, error_box, group_digits, human_size, info_box, wide};
use crate::{FRAME_CLASS, VIEW_CLASS, default_proc, hiword, loword};

// メニュー・アクセラレータのコマンド ID
const ID_OPEN: u16 = 101;
const ID_CLOSE: u16 = 102;
const ID_EXIT: u16 = 103;
const ID_GOTO: u16 = 201;
const ID_ZOOM_IN: u16 = 202;
const ID_ZOOM_OUT: u16 = 203;
const ID_ZOOM_RESET: u16 = 204;
const ID_LINE_NUMBERS: u16 = 205;
const ID_ABOUT: u16 = 301;

const ID_STATUS: i32 = 1000;

/// ワーカースレッドから行数カウントの進捗を知らせるメッセージ
const WM_APP_INDEX: u32 = WM_APP + 1;

/// バイト位置比例スクロールバーの範囲
const SCROLL_RANGE: i32 = 1 << 16;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// アプリ状態を借用して `f` を実行する。再入中（すでに借用中）なら `None`。
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

pub(crate) struct App {
    frame: HWND,
    view: HWND,
    status: HWND,
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
}

pub(crate) fn create_accelerators() -> Result<HACCEL> {
    let ctrl = FVIRTKEY | FCONTROL;
    let accels = [
        (ctrl, b'O' as u16, ID_OPEN),
        (ctrl, b'W' as u16, ID_CLOSE),
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

fn create_menu() -> Result<(HMENU, HMENU)> {
    unsafe {
        let bar = CreateMenu()?;
        let file = CreatePopupMenu()?;
        AppendMenuW(file, MF_STRING, ID_OPEN as usize, w!("開く(&O)...\tCtrl+O"))?;
        AppendMenuW(file, MF_STRING, ID_CLOSE as usize, w!("閉じる(&C)\tCtrl+W"))?;
        AppendMenuW(file, MF_SEPARATOR, 0, None)?;
        AppendMenuW(file, MF_STRING, ID_EXIT as usize, w!("終了(&X)\tAlt+F4"))?;
        let view = CreatePopupMenu()?;
        AppendMenuW(
            view,
            MF_STRING,
            ID_GOTO as usize,
            w!("行へ移動(&G)...\tCtrl+G"),
        )?;
        AppendMenuW(view, MF_SEPARATOR, 0, None)?;
        AppendMenuW(view, MF_STRING, ID_ZOOM_IN as usize, w!("拡大(&I)\tCtrl++"))?;
        AppendMenuW(
            view,
            MF_STRING,
            ID_ZOOM_OUT as usize,
            w!("縮小(&O)\tCtrl+-"),
        )?;
        AppendMenuW(
            view,
            MF_STRING,
            ID_ZOOM_RESET as usize,
            w!("標準のサイズ(&R)\tCtrl+0"),
        )?;
        AppendMenuW(view, MF_SEPARATOR, 0, None)?;
        AppendMenuW(
            view,
            MF_STRING | MF_CHECKED,
            ID_LINE_NUMBERS as usize,
            w!("行番号(&L)"),
        )?;
        let help = CreatePopupMenu()?;
        AppendMenuW(help, MF_STRING, ID_ABOUT as usize, w!("バージョン情報(&A)"))?;
        AppendMenuW(bar, MF_POPUP, file.0 as usize, w!("ファイル(&F)"))?;
        AppendMenuW(bar, MF_POPUP, view.0 as usize, w!("表示(&V)"))?;
        AppendMenuW(bar, MF_POPUP, help.0 as usize, w!("ヘルプ(&H)"))?;
        Ok((bar, view))
    }
}

impl App {
    /// ウィンドウを作成してアプリ状態を初期化する。フレームウィンドウを返す。
    pub(crate) fn create(hinstance: HINSTANCE, initial_file: Option<PathBuf>) -> Result<HWND> {
        let (config, config_error) = Config::load();
        unsafe {
            let (menu, menu_view) = create_menu().context("create_menu")?;
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
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(windows::Win32::UI::Controls::SBARS_SIZEGRIP),
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
                open_path(path);
            }
            Ok(frame)
        }
    }

    fn page_rows(&self) -> usize {
        let h = self.renderer.px_to_dip(self.view_px.1 as f32);
        let lh = self.renderer.metrics().line_height.max(1.0);
        ((h / lh).floor() as usize).max(1)
    }

    fn text_area_width(&self) -> f32 {
        let w = self.renderer.px_to_dip(self.view_px.0 as f32);
        (w - self
            .renderer
            .text_origin_x(self.line_digits(), self.show_line_numbers))
        .max(0.0)
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
            // ステータスバーの区切り: 位置 | サイズ | 文字コード | 進捗
            let parts = [w - 520, w - 380, w - 260, -1].map(|x| x.max(0));
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
        let title = match self.doc.path() {
            Some(_) => format!("{} - yyeditor", self.doc.display_name()),
            None => "yyeditor".to_owned(),
        };
        unsafe {
            let _ = SetWindowTextW(self.frame, &HSTRING::from(title));
        }
    }

    fn update_status(&self) {
        let snap = self.doc.snapshot();
        let pos = snap.line_of_offset(self.vp.top);
        let total = match snap.line_count() {
            Some(n) => group_digits(n),
            None => format!("約 {}", group_digits(snap.estimated_line_count())),
        };
        let approx = if pos.exact { "" } else { "約 " };
        self.set_status(
            0,
            &format!(
                "  {approx}{} 行目 / 全 {total} 行",
                group_digits(pos.line + 1)
            ),
        );
        self.set_status(1, &format!("  {}", human_size(self.doc.file_len())));
        let enc = match self.doc.bom() {
            yy_io::Bom::Utf8 => "UTF-8 (BOM 付き)",
            yy_io::Bom::None => "UTF-8",
        };
        self.set_status(2, &format!("  {enc}"));
        let progress = match self.doc.indexing_progress() {
            Some(p) => format!("  行数を数えています… {:.0}%", p * 100.0),
            None => String::new(),
        };
        self.set_status(3, &progress);
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

    fn scroll_horizontal(&mut self, x: f32) {
        let max = (self.renderer.max_text_width + self.renderer.metrics().char_width * 4.0
            - self.text_area_width())
        .max(0.0);
        let x = x.clamp(0.0, max);
        if x != self.scroll_x {
            self.scroll_x = x;
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

    fn on_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let page = self.page_rows() as i64;
        let cw = self.renderer.metrics().char_width;
        match vk {
            VK_UP => self.scroll_rows(-1),
            VK_DOWN => self.scroll_rows(1),
            VK_PRIOR => self.scroll_rows(-(page - 1).max(1)),
            VK_NEXT => self.scroll_rows((page - 1).max(1)),
            VK_HOME if ctrl => self.scroll_to_offset(0),
            VK_END if ctrl => self.scroll_to_offset(u64::MAX),
            VK_HOME => self.scroll_horizontal(0.0),
            VK_END => self.scroll_horizontal(f32::MAX),
            VK_LEFT => self.scroll_horizontal(self.scroll_x - cw * 4.0),
            VK_RIGHT => self.scroll_horizontal(self.scroll_x + cw * 4.0),
            _ => return false,
        }
        true
    }

    fn on_wheel(&mut self, delta: i16, horizontal: bool) {
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        if ctrl && !horizontal {
            let step = if delta > 0 { 1.0 } else { -1.0 };
            self.zoom(self.renderer.font_size_pt() + step);
            return;
        }
        let notches = delta as f32 / WHEEL_DELTA as f32;
        if horizontal || shift {
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

    fn paint(&mut self) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(self.view, &mut ps);
            let snap = self.doc.snapshot();
            let page = self.page_rows();
            let rows = rows_from(snap, self.rows_cfg, self.vp.top, page + 1);
            let first = snap.line_of_offset(self.vp.top);
            let frame = Frame {
                rows: &rows,
                first_line: first.line,
                line_exact: first.exact,
                line_digits: self.line_digits(),
                show_line_numbers: self.show_line_numbers,
                scroll_x: self.scroll_x,
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

    fn open(&mut self, path: PathBuf) -> std::result::Result<(), String> {
        let mut doc = Document::open(&path).map_err(|e| format!("{}\n\n{e}", path.display()))?;
        let frame = self.frame.0 as isize;
        let posted = self.index_posted.clone();
        doc.start_indexing(
            &self.pool,
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
            }),
        );
        self.doc = doc;
        self.vp = Viewport::default();
        self.scroll_x = 0.0;
        self.renderer.clear_cache();
        self.update_title();
        self.after_scroll();
        Ok(())
    }

    fn close(&mut self) {
        self.doc = Document::new_empty();
        self.vp = Viewport::default();
        self.scroll_x = 0.0;
        self.renderer.clear_cache();
        self.update_title();
        self.after_scroll();
    }

    fn on_index_progress(&mut self) {
        self.index_posted.store(false, Ordering::Release);
        if self.doc.poll_indexing() {
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
            LineLookup::Found(off) => self.scroll_to_offset(off),
            _ => info_box(
                self.frame,
                &format!("{} 行目はありません。", group_digits(line)),
            ),
        }
    }
}

/// 状態を借用せずにファイルを開く（エラー表示のモーダルを借用の外で出すため）。
fn open_path(path: PathBuf) {
    let frame = with_app(|a| a.frame);
    if let Some(Err(msg)) = with_app(|a| a.open(path)) {
        if let Some(f) = frame {
            error_box(f, &format!("ファイルを開けません。\n{msg}"));
        }
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

fn on_command(hwnd: HWND, id: u16) {
    match id {
        ID_OPEN => {
            if let Some(p) = show_open_dialog(hwnd) {
                open_path(p);
            }
        }
        ID_CLOSE => {
            with_app(|a| a.close());
        }
        ID_EXIT => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        ID_GOTO => {
            let Some((current, total)) = with_app(|a| {
                let snap = a.doc.snapshot();
                let cur = snap.line_of_offset(a.vp.top).line + 1;
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
        WM_COMMAND => {
            on_command(hwnd, loword(wparam.0) as u16);
            LRESULT(0)
        }
        WM_DROPFILES => {
            if let Some(p) = dropped_file(HDROP(wparam.0 as *mut _)) {
                open_path(p);
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
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => default_proc(hwnd, msg, wparam, lparam),
    }
}

const WM_APP_RELAYOUT: u32 = WM_APP + 2;
const WM_APP_SCROLLBARS: u32 = WM_APP + 3;

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
                default_proc(hwnd, msg, wparam, lparam)
            }
        }
        WM_LBUTTONDOWN => {
            unsafe {
                let _ = SetFocus(Some(hwnd));
            }
            LRESULT(0)
        }
        _ => default_proc(hwnd, msg, wparam, lparam),
    }
}

const WM_APP_REPAINT: u32 = WM_APP + 4;
const WM_APP_RESIZE: u32 = WM_APP + 5;

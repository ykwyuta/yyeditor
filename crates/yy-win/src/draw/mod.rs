//! Native yydraw shell; the same portable scene is used on screen and during export.
mod arrange;
mod association;
mod colors;
mod connection_points;
mod features;
mod interaction;
mod paint;
mod palette;
mod wire;
mod work;
use crate::util::{error_box, info_box};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET,
            DeleteObject, EndPaint, FF_DONTCARE, HFONT, HGDIOBJ, InvalidateRect,
            OUT_DEFAULT_PRECIS, PAINTSTRUCT,
        },
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree,
            },
            LibraryLoader::GetModuleHandleW,
        },
        UI::{
            Controls::*,
            HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
                SetProcessDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::*,
            Shell::{
                Common::COMDLG_FILTERSPEC, DragAcceptFiles, DragFinish, DragQueryFileW,
                FileOpenDialog, FileSaveDialog, HDROP, IFileOpenDialog, IFileSaveDialog,
                SIGDN_FILESYSPATH,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, PCWSTR, Result, w},
};
use yy_draw::{
    render::{Export, Renderer},
    *,
};

const CLASS: PCWSTR = w!("YYDrawFrame");
const NEW: u16 = 1;
const OPEN: u16 = 2;
const SAVE: u16 = 3;
const SAVE_AS: u16 = 4;
const SVG: u16 = 5;
const PNG: u16 = 6;
const IMPORT: u16 = 7;
const CLOSE: u16 = 8;
const UNDO: u16 = 10;
const REDO: u16 = 11;
const DELETE: u16 = 12;
const DUPLICATE: u16 = 13;
const SELECT_ALL: u16 = 14;
const SELECT: u16 = 15;
const STRAIGHT: u16 = 16;
const ORTHO: u16 = 17;
const GRID: u16 = 18;
const SNAP: u16 = 19;
const JUMPS: u16 = 20;
const FIT: u16 = 21;
const APPLY: u16 = 22;
const GROUP: u16 = 23;
const UNGROUP: u16 = 24;
const FRONT: u16 = 25;
const BACK: u16 = 26;
const ALIGN_LEFT: u16 = 27;
const ALIGN_TOP: u16 = 28;
const CENTER_X: u16 = 29;
const DISTRIBUTE: u16 = 30;
const ALIGN_RIGHT: u16 = 43;
const ALIGN_BOTTOM: u16 = 44;
const CENTER_Y: u16 = 45;
const DISTRIBUTE_Y: u16 = 46;
const ALIGN_SELECTION: u16 = 47;
const ALIGN_PAGE: u16 = 48;
const HELP: u16 = 31;
const COPY: u16 = 32;
const PASTE: u16 = 33;
const CUT: u16 = 34;
const EXPORT_OPTIONS: u16 = 35;
const CANCEL: u16 = 64;
const PAGES: u16 = 103;
const WORK_TIMER: usize = 701;
const PALETTE: u16 = 100;
const TABS: u16 = 101;
const FILTER: u16 = 102;
const LABEL: u16 = 110;
const PROP: u16 = 120;
const FIELDS: &[&str] = &[
    "X",
    "Y",
    "幅",
    "高さ",
    "文字サイズ (DIP)",
    "塗り #RRGGBB",
    "線 #RRGGBB",
    "線幅",
    "ジャンプ (0自動/1優先/2無)",
    "角の形",
    "角の半径 (DIP)",
];

#[derive(Clone, Copy)]
enum Tool {
    Select,
    Shape(Shape),
    Icon(Id),
    LibraryIcon(Id),
    CatalogIcon(usize),
    Connect(bool),
}
struct Tab {
    uid: u64,
    doc: Document,
    saved: Document,
    path: Option<PathBuf>,
    history: History,
    selected: Vec<Id>,
    zoom: f64,
    origin: Point,
}
impl Tab {
    fn new(doc: Document, path: Option<PathBuf>) -> Self {
        Self {
            uid: NEXT_TAB.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            saved: doc.clone(),
            doc,
            path,
            history: History::default(),
            selected: vec![],
            zoom: 1.,
            origin: Point::new(-24., -24.),
        }
    }
    fn dirty(&self) -> bool {
        self.doc != self.saved
    }
}
enum Drag {
    Waypoint {
        before: Document,
        id: Id,
        index: usize,
    },
    Endpoint {
        before: Document,
        id: Id,
        start: bool,
    },
    Move {
        before: Document,
        start: Point,
        selection: editing::MoveSelection,
    },
    Resize {
        before: Document,
        id: Id,
    },
    Pan {
        start: Point,
        origin: Point,
    },
    Box {
        start: Point,
        current: Point,
    },
    Connect {
        start: Endpoint,
        draft: drawing::Draft,
        orthogonal: bool,
    },
}
struct App {
    hwnd: HWND,
    tabs: Vec<Tab>,
    tab: usize,
    tool: Tool,
    drag: Option<Drag>,
    renderer: Renderer,
    routing: router::Cache,
    painter: paint::Painter,
    palette: HWND,
    palette_tools: Vec<Tool>,
    palette_state: palette::State,
    filter: HWND,
    tabs_hwnd: HWND,
    label: HWND,
    fields: Vec<HWND>,
    controls: Vec<(HWND, i32, i32, i32, i32)>,
    font: HFONT,
    width: i32,
    height: i32,
    dpi: f64,
    dirty: bool,
    pixels: Vec<u8>,
    pixsize: (u32, u32),
    pixel_revision: u64,
    image_camera: Option<interaction::Camera>,
    image_uid: u64,
    view_deadline: Option<std::time::Instant>,
    zoom_motion: Option<interaction::ZoomMotion>,
    fill_color: u32,
    line_color: u32,
    rounded_lines: bool,
    corner_radius: f64,
    layout_reference: layout::Reference,
    pending_connector: Option<(u64, Id)>,
    hover_node: Option<Id>,
    hover_port: Option<Endpoint>,
    custom_colors: [windows::Win32::Foundation::COLORREF; 16],
    sprite: Option<work::Sprite>,
    preview_visible: bool,
    preview_ids: Vec<Id>,
    drag_offset: Point,
    property_dirty: bool,
    property_values: Vec<String>,
    property_label: String,
    updating: bool,
    clipboard: Option<(Document, Vec<Id>)>,
    transparent: bool,
    export_grid: bool,
    export_scale: f64,
    export_scope: u16,
    export_text: bool,
    export_margin: f64,
    export_color: u32,
    pages_combo: HWND,
    library: Document,
    work: Option<yy_draw::jobs::Task<work::Outcome>>,
    frame: Option<(u64, yy_draw::jobs::Task<work::Frame>)>,
    revision: u64,
    close_after: Option<u64>,
    closing_all: bool,
    guides: Vec<yy_draw::editing::Guide>,
    pending_error: Option<String>,
}
static NEXT_TAB: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
thread_local! {static APP:RefCell<Option<App>>=const{RefCell::new(None)};}
fn with<T>(f: impl FnOnce(&mut App) -> T) -> Option<T> {
    APP.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}
fn point(lp: LPARAM) -> Point {
    Point::new(
        (lp.0 as u16 as i16) as f64,
        ((lp.0 >> 16) as u16 as i16) as f64,
    )
}
fn key(v: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(v.0 as i32) < 0 }
}

pub fn run_draw(initial: Option<PathBuf>) -> Result<()> {
    crate::crash::install("yydraw");
    crate::util::set_app_name("yydraw");
    let result = run(initial);
    crate::crash::clean_exit();
    result
}
fn run(initial: Option<PathBuf>) -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let ic = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_TAB_CLASSES,
        };
        let _ = InitCommonControlsEx(&ic);
        crate::font::register_gdi();
        let instance = GetModuleHandleW(None)?;
        let (large, small) = crate::app_icons(instance.into());
        let class = WNDCLASSEXW {
            style: CS_DBLCLKS,
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            hInstance: instance.into(),
            lpfnWndProc: Some(proc),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: large,
            hIconSm: small,
            ..Default::default()
        };
        RegisterClassExW(&class);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("yydraw"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1200,
            820,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let (config, _) = yy_config::Config::load();
        let doc = Document {
            font_family: config.editor.font_family,
            font_size: f64::from(config.editor.font_size) * 96. / 72.,
            ..Default::default()
        };
        let mut app = App::new(hwnd, doc, None)?;
        app.create_controls()?;
        SetMenu(hwnd, Some(menu()?))?;
        app.refresh_tabs();
        app.properties();
        app.layout();
        APP.with(|a| *a.borrow_mut() = Some(app));
        SetTimer(Some(hwnd), WORK_TIMER, 16, None);
        DragAcceptFiles(hwnd, true);
        let _ = ShowWindow(hwnd, SW_SHOW);
        if let Some(path) = initial {
            open_path(hwnd, path);
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let is_edit = with(|a| {
                a.label == msg.hwnd
                    || a.fields.contains(&msg.hwnd)
                    || a.filter == msg.hwnd
                    || a.palette == msg.hwnd
                    || a.palette_state.category == msg.hwnd
            })
            .unwrap_or(false);
            let cancel_drag = msg.wParam.0 == VK_ESCAPE.0 as usize
                && with(|a| a.drag.is_some() || a.palette_state.drag.is_some()).unwrap_or(false);
            if msg.message == WM_KEYDOWN
                && (!is_edit || cancel_drag)
                && shortcut(hwnd, msg.wParam.0 as u16)
            {
                continue;
            }
            if msg.message == WM_KEYUP && msg.wParam.0 == VK_SHIFT.0 as usize {
                with(|a| a.refresh_wire_constraint());
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|a| {
            if let Some(a) = a.borrow_mut().take() {
                let _ = DeleteObject(HGDIOBJ(a.font.0));
            }
        });
    }
    Ok(())
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MEASUREITEM => {
            let item = unsafe { &mut *(lp.0 as *mut MEASUREITEMSTRUCT) };
            if item.CtlID == u32::from(PALETTE) {
                item.itemHeight = unsafe { GetDpiForWindow(hwnd) } * 64 / 96;
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_DRAWITEM => {
            let item = unsafe { &*(lp.0 as *const DRAWITEMSTRUCT) };
            if item.CtlID == u32::from(PALETTE) {
                with(|a| a.draw_palette_item(item));
                return LRESULT(1);
            }
            if item.CtlID == u32::from(colors::FILL) || item.CtlID == u32::from(colors::LINE) {
                with(|a| a.draw_color_button(item));
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_INITMENUPOPUP => {
            with(|a| a.update_menu(HMENU(wp.0 as *mut _)));
            LRESULT(0)
        }
        WM_CONTEXTMENU => {
            arrange::context_menu(hwnd, point(lp));
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = wp.0 as u16;
            let notification = (wp.0 >> 16) as u16;
            if id == PAGES && notification == CBN_SELCHANGE as u16 {
                with(|a| {
                    if !a.apply_properties() {
                        return;
                    }
                    let index = unsafe { SendMessageW(a.pages_combo, CB_GETCURSEL, None, None).0 };
                    if index >= 0 {
                        let tab = a.current();
                        let _ = tab.doc.switch_page(index as usize);
                        let _ = tab.saved.switch_page(index as usize);
                        tab.selected.clear();
                        a.properties();
                        a.refresh_pages();
                        a.invalidate();
                    }
                });
            } else if (id == FILTER && notification == EN_CHANGE as u16)
                || (id == palette::CATEGORY && notification == CBN_SELCHANGE as u16)
            {
                with(|a| a.rebuild_palette());
            } else if id == PALETTE && notification == LBN_SELCHANGE as u16 {
                with(|a| a.choose_palette());
            } else if (id == LABEL || (PROP..PROP + FIELDS.len() as u16).contains(&id))
                && (notification == EN_CHANGE as u16 || notification == CBN_SELCHANGE as u16)
            {
                with(|a| {
                    if !a.updating {
                        a.property_dirty = true;
                    }
                });
            } else {
                command(hwnd, id);
            }
            LRESULT(0)
        }
        WM_NOTIFY => {
            let header = unsafe { &*(lp.0 as *const NMHDR) };
            if header.idFrom == TABS as usize && header.code == TCN_SELCHANGE {
                with(|a| {
                    if !a.apply_properties() {
                        unsafe {
                            SendMessageW(a.tabs_hwnd, TCM_SETCURSEL, Some(WPARAM(a.tab)), None);
                        }
                        return;
                    }
                    let i = unsafe { SendMessageW(a.tabs_hwnd, TCM_GETCURSEL, None, None).0 };
                    if i >= 0 && (i as usize) < a.tabs.len() {
                        a.tab = i as usize;
                        a.tool = Tool::Select;
                        a.rebuild_palette();
                        a.properties();
                        a.invalidate();
                    }
                });
            }
            LRESULT(0)
        }
        WM_SIZE => {
            with(|a| {
                a.width = (lp.0 as u16) as i32;
                a.height = ((lp.0 >> 16) as u16) as i32;
                a.layout();
            });
            LRESULT(0)
        }
        WM_TIMER if wp.0 == WORK_TIMER => {
            with(|a| {
                a.tick_zoom(std::time::Instant::now());
            });
            with(|a| a.poll_thumbnails());
            let next = with(|a| a.poll_work()).flatten();
            if let Some(next) = next {
                unsafe {
                    let _ = PostMessageW(Some(hwnd), next, WPARAM(0), LPARAM(0));
                }
            }
            LRESULT(0)
        }
        work::CLOSE_SAVED => {
            close_tab(hwnd);
            LRESULT(0)
        }
        work::ERROR_READY => {
            if let Some(e) = with(|a| a.pending_error.take()).flatten() {
                error_box(hwnd, &e);
            }
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let info = unsafe { &mut *(lp.0 as *const MINMAXINFO as *mut MINMAXINFO) };
            let dpi = with(|a| a.dpi).unwrap_or(1.);
            info.ptMinTrackSize.x = (980. * dpi) as i32;
            info.ptMinTrackSize.y = (800. * dpi) as i32;
            LRESULT(0)
        }
        WM_DPICHANGED => {
            with(|a| {
                a.dpi = (wp.0 as u16) as f64 / 96.;
                a.palette_state.dpi_changed();
                a.scale_font();
                a.rebuild_palette();
                a.layout();
            });
            let r = unsafe { &*(lp.0 as *const RECT) };
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
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe {
                BeginPaint(hwnd, &mut ps);
            }
            let error = with(|a| a.paint()).and_then(|r| r.err());
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            if let Some(e) = error {
                with(|a| a.status(&e));
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_LBUTTONDOWN => {
            with(|a| a.down(point(lp), false));
            LRESULT(0)
        }
        WM_MBUTTONDOWN => {
            with(|a| a.down(point(lp), true));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            with(|a| a.motion(point(lp)));
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_MBUTTONUP => {
            with(|a| {
                if !a.finish_palette_drag(point(lp)) {
                    a.up(point(lp));
                }
            });
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            with(|a| {
                let had_palette = a.palette_state.drag.take().is_some();
                let drag = a.drag.take();
                let had_drag = drag.is_some();
                if let Some(
                    Drag::Waypoint { before, .. }
                    | Drag::Move { before, .. }
                    | Drag::Resize { before, .. }
                    | Drag::Endpoint { before, .. },
                ) = drag
                {
                    a.tabs[a.tab].doc = before;
                    a.drag_offset = Point::default();
                }
                if had_drag || had_palette {
                    a.guides.clear();
                    a.invalidate();
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            with(|a| a.double_click(point(lp)));
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            with(|a| {
                let delta = ((wp.0 >> 16) as u16 as i16) as f64;
                let raw = point(lp);
                let mut cursor = windows::Win32::Foundation::POINT {
                    x: raw.x as i32,
                    y: raw.y as i32,
                };
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut cursor);
                }
                a.wheel(
                    Point::new(cursor.x as f64, cursor.y as f64),
                    delta,
                    key(VK_CONTROL),
                    key(VK_SHIFT),
                );
            });
            LRESULT(0)
        }
        WM_DROPFILES => {
            let drop = HDROP(wp.0 as *mut _);
            let len = unsafe { DragQueryFileW(drop, 0, None) };
            let mut buf = vec![0u16; len as usize + 1];
            unsafe {
                DragQueryFileW(drop, 0, Some(&mut buf));
                DragFinish(drop);
            }
            if let Ok(p) = String::from_utf16(&buf[..len as usize]) {
                open_path(hwnd, PathBuf::from(p));
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            with(|a| a.closing_all = true);
            if close_all(hwnd) {
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
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
impl App {
    fn scale_font(&mut self) {
        unsafe {
            let old = self.font;
            self.font = CreateFontW(
                (-(14. * self.dpi)) as i32,
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
                FF_DONTCARE.0 as u32,
                &HSTRING::from(&self.tabs[self.tab].doc.font_family),
            );
            for (h, ..) in &self.controls {
                SendMessageW(
                    *h,
                    WM_SETFONT,
                    Some(WPARAM(self.font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
            let _ = DeleteObject(HGDIOBJ(old.0));
        }
    }
    fn new(hwnd: HWND, doc: Document, path: Option<PathBuf>) -> Result<Self> {
        unsafe {
            let mut client = RECT::default();
            GetClientRect(hwnd, &mut client)?;
            Ok(Self {
                hwnd,
                tabs: vec![Tab::new(doc, path)],
                tab: 0,
                tool: Tool::Select,
                drag: None,
                renderer: Renderer::new(Some(crate::font::BUNDLED_FONT)),
                routing: router::Cache::default(),
                painter: paint::Painter::new()?,
                palette: HWND::default(),
                palette_tools: vec![],
                palette_state: palette::State::default(),
                filter: HWND::default(),
                tabs_hwnd: HWND::default(),
                label: HWND::default(),
                fields: vec![],
                controls: vec![],
                font: HFONT::default(),
                width: client.right.max(1),
                height: client.bottom.max(1),
                dpi: f64::from(GetDpiForWindow(hwnd)) / 96.,
                dirty: true,
                pixels: vec![],
                pixsize: (0, 0),
                pixel_revision: 0,
                image_camera: None,
                image_uid: 0,
                view_deadline: None,
                zoom_motion: None,
                fill_color: 0xf0f6ff,
                line_color: 0x345782,
                rounded_lines: false,
                corner_radius: 12.,
                layout_reference: layout::Reference::Selection,
                pending_connector: None,
                hover_node: None,
                hover_port: None,
                custom_colors: [windows::Win32::Foundation::COLORREF(0xffffff); 16],
                sprite: None,
                preview_visible: false,
                preview_ids: vec![],
                drag_offset: Point::default(),
                property_dirty: false,
                property_values: vec![],
                property_label: String::new(),
                updating: false,
                clipboard: None,
                transparent: true,
                export_grid: false,
                export_scale: 2.,
                export_scope: 61,
                export_text: false,
                export_margin: 16.,
                export_color: 0xffffff,
                pages_combo: HWND::default(),
                library: features::load_library(),
                work: None,
                frame: None,
                revision: 0,
                close_after: None,
                closing_all: false,
                guides: vec![],
                pending_error: None,
            })
        }
    }

    fn current(&mut self) -> &mut Tab {
        &mut self.tabs[self.tab]
    }
    fn control(
        &mut self,
        class: PCWSTR,
        text: &str,
        id: u16,
        style: WINDOW_STYLE,
        rect: (i32, i32, i32, i32),
    ) -> Result<HWND> {
        unsafe {
            let h = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                &HSTRING::from(text),
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                1,
                1,
                Some(self.hwnd),
                Some(HMENU(id as usize as *mut _)),
                None,
                None,
            )?;
            SendMessageW(
                h,
                WM_SETFONT,
                Some(WPARAM(self.font.0 as usize)),
                Some(LPARAM(1)),
            );
            self.controls.push((h, rect.0, rect.1, rect.2, rect.3));
            Ok(h)
        }
    }
    fn create_controls(&mut self) -> Result<()> {
        unsafe {
            self.font = CreateFontW(
                (-(14. * self.dpi)) as i32,
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
                FF_DONTCARE.0 as u32,
                &HSTRING::from(&self.tabs[0].doc.font_family),
            );
            for (i, (id, text)) in [
                (NEW, "新規"),
                (OPEN, "開く"),
                (SAVE, "保存"),
                (SELECT, "選択"),
                (STRAIGHT, "直線→"),
                (ORTHO, "直交→"),
                (FIT, "全体表示"),
                (IMPORT, "素材追加"),
                (SVG, "SVG"),
                (PNG, "PNG"),
            ]
            .iter()
            .enumerate()
            {
                self.control(
                    w!("BUTTON"),
                    text,
                    *id,
                    WS_TABSTOP,
                    (6 + i as i32 * 82, 4, 78, 28),
                )?;
            }
            self.pages_combo = self.control(
                w!("COMBOBOX"),
                "",
                PAGES,
                WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
                (-224, 36, 212, 300),
            )?;
            self.tabs_hwnd =
                self.control(WC_TABCONTROLW, "", TABS, WS_TABSTOP, (0, 36, 1000, 28))?;
            self.filter = self.control(
                w!("EDIT"),
                "",
                FILTER,
                WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                (6, 72, 248, 24),
            )?;
            SendMessageW(
                self.filter,
                EM_SETCUEBANNER,
                Some(WPARAM(1)),
                Some(LPARAM(w!("選択カテゴリ内を検索").0 as isize)),
            );
            self.palette = self.control(
                w!("LISTBOX"),
                "",
                PALETTE,
                WS_BORDER
                    | WS_VSCROLL
                    | WINDOW_STYLE((LBS_NOTIFY | LBS_OWNERDRAWFIXED | LBS_HASSTRINGS) as u32),
                (6, 130, 248, 600),
            )?;
            self.palette_state.category = self.control(
                w!("COMBOBOX"),
                "",
                palette::CATEGORY,
                WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
                (6, 100, 248, 500),
            )?;
            palette::install(self.palette, self.hwnd);
            self.control(
                w!("STATIC"),
                "選択した図形・接続線",
                0,
                WINDOW_STYLE(0),
                (-224, 76, 216, 24),
            )?;
            self.label = self.control(
                w!("EDIT"),
                "",
                LABEL,
                WS_BORDER
                    | WS_TABSTOP
                    | WS_VSCROLL
                    | WINDOW_STYLE(
                        ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | ES_WANTRETURN as u32,
                    ),
                (-224, 104, 212, 90),
            )?;
            SendMessageW(self.label, EM_SETLIMITTEXT, Some(WPARAM(65536)), None);
            for (i, label) in FIELDS.iter().enumerate() {
                let (x, y, width) = if i < 4 {
                    (-224 + (i % 2) as i32 * 110, 204 + (i / 2) as i32 * 45, 102)
                } else {
                    (-224, 204 + (i as i32 - 2) * 45, 212)
                };
                self.control(w!("STATIC"), label, 0, WINDOW_STYLE(0), (x, y, width, 19))?;
                let h = self.control(
                    if i == 9 { w!("COMBOBOX") } else { w!("EDIT") },
                    "",
                    PROP + i as u16,
                    WS_BORDER
                        | WS_TABSTOP
                        | WINDOW_STYLE(if i == 9 {
                            CBS_DROPDOWNLIST as u32
                        } else {
                            ES_AUTOHSCROLL as u32
                        }),
                    (
                        x,
                        y + 19,
                        if i == 5 || i == 6 { 154 } else { width },
                        if i == 9 { 150 } else { 23 },
                    ),
                )?;
                if i == 9 {
                    for label in [w!("尖角"), w!("丸角")] {
                        SendMessageW(h, CB_ADDSTRING, None, Some(LPARAM(label.0 as isize)));
                    }
                }
                self.fields.push(h);
                if i == 5 || i == 6 {
                    self.control(
                        w!("BUTTON"),
                        "色…",
                        if i == 5 { colors::FILL } else { colors::LINE },
                        WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
                        (-66, y + 19, 54, 23),
                    )?;
                }
            }
            self.control(
                w!("BUTTON"),
                "適用",
                APPLY,
                WS_TABSTOP,
                (-224, 618, 100, 28),
            )?;
            self.control(w!("STATIC"),"Shift: 上下左右に固定\nShift+Tab: 折れ点を追加\nBackspace: 折れ点を戻す\nCtrl+ホイール: ズーム",0,WINDOW_STYLE(0),(-224,656,218,88))?;
            self.rebuild_palette();
            Ok(())
        }
    }
    fn choose_palette(&mut self) {
        let i = unsafe { SendMessageW(self.palette, LB_GETCURSEL, None, None).0 };
        if i >= 0 {
            if let Some(tool) = self.palette_tools.get(i as usize) {
                self.tool = *tool;
            }
        }
        self.status("キャンバスをクリックして配置");
    }
    fn layout(&mut self) {
        unsafe {
            for &(h, x, y, w, height) in &self.controls {
                let x = if x < 0 {
                    self.width + (x as f64 * self.dpi) as i32
                } else {
                    (x as f64 * self.dpi) as i32
                };
                let w = if h == self.tabs_hwnd {
                    self.width - (236. * self.dpi) as i32
                } else {
                    (w as f64 * self.dpi) as i32
                };
                let height = if h == self.palette {
                    (self.height - (138. * self.dpi) as i32).max(20)
                } else {
                    (height as f64 * self.dpi) as i32
                };
                let _ = MoveWindow(h, x, (y as f64 * self.dpi) as i32, w, height, true);
            }
        }
        self.invalidate();
    }
    fn viewport(&self) -> (i32, i32, u32, u32) {
        let l = (palette::WIDTH as f64 * self.dpi) as i32;
        let top = (70. * self.dpi) as i32;
        let w = (self.width - l - (236. * self.dpi) as i32).max(1) as u32;
        let h = (self.height - top).max(1) as u32;
        (l, top, w, h)
    }
    fn document_point(&self, p: Point) -> Point {
        let (l, t, _, _) = self.viewport();
        let tab = &self.tabs[self.tab];
        let scale = self.dpi * tab.zoom;
        Point::new(
            (p.x - l as f64) / scale + tab.origin.x,
            (p.y - t as f64) / scale + tab.origin.y,
        )
    }
    fn invalidate(&mut self) {
        self.zoom_motion = None;
        self.view_deadline = None;
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
        if let Some((_, task)) = &self.frame {
            task.control.cancel();
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }
    fn invalidate_overlay(&self) {
        let (l, t, w, h) = self.viewport();
        let r = RECT {
            left: l,
            top: t,
            right: l + w as i32,
            bottom: t + h as i32,
        };
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), Some(&r), false);
        }
    }
    fn status(&self, s: &str) {
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &HSTRING::from(format!("yydraw — {s}")));
        }
    }
    fn refresh_tabs(&mut self) {
        unsafe {
            SendMessageW(self.tabs_hwnd, TCM_DELETEALLITEMS, None, None);
            for (i, t) in self.tabs.iter().enumerate() {
                let name = t
                    .path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("図{}", i + 1));
                let name = HSTRING::from(format!("{}{}", if t.dirty() { "* " } else { "" }, name));
                let item = TCITEMW {
                    mask: TCIF_TEXT,
                    pszText: windows::core::PWSTR(name.as_ptr() as *mut _),
                    ..Default::default()
                };
                SendMessageW(
                    self.tabs_hwnd,
                    TCM_INSERTITEMW,
                    Some(WPARAM(i)),
                    Some(LPARAM((&item as *const TCITEMW) as isize)),
                );
            }
            SendMessageW(self.tabs_hwnd, TCM_SETCURSEL, Some(WPARAM(self.tab)), None);
        }
        self.refresh_pages();
        self.status("方眼紙 / Ctrl+S 保存 / F1 操作ガイド");
    }
    fn paint(&mut self) -> std::result::Result<(), String> {
        let (l, t, w, h) = self.viewport();
        if self.frame_ready() && self.frame.is_none() {
            let tab = &self.tabs[self.tab];
            let scale = self.dpi * tab.zoom;
            let options = Export {
                bounds: Rect {
                    x: tab.origin.x - interaction::OVERSCAN / scale,
                    y: tab.origin.y - interaction::OVERSCAN / scale,
                    w: (w as f64 + 2. * interaction::OVERSCAN) / scale,
                    h: (h as f64 + 2. * interaction::OVERSCAN) / scale,
                },
                background: Some(0xffffff),
                grid: tab.doc.show_grid,
                text: false,
                selected: tab.selected.clone(),
            };
            let (doc, moving) = if let Some(Drag::Move { before, .. }) = &self.drag {
                (before.clone(), Some(tab.selected.clone()))
            } else {
                (tab.doc.clone(), None)
            };
            let renderer = self.renderer.clone();
            let revision = self.revision;
            self.frame = Some((
                revision,
                yy_draw::jobs::Task::spawn(move |control| {
                    work::draw_frame(&renderer, doc, options, scale, moving, control)
                }),
            ));
            self.dirty = false;
        }
        let arrow_preview = self.arrow_preview();
        let tab = &self.tabs[self.tab];
        let scale = self.dpi * tab.zoom;
        let screen = |p: Point| {
            Point::new(
                (p.x - tab.origin.x) * scale + l as f64,
                (p.y - tab.origin.y) * scale + t as f64,
            )
        };
        let page = tab.doc.page_bounds();
        let corners = [
            Point::new(page.x, page.y),
            Point::new(page.x + page.w, page.y),
            Point::new(page.x + page.w, page.y + page.h),
            Point::new(page.x, page.y + page.h),
        ];
        let mut overlay: Vec<_> = (0..4)
            .map(|i| (screen(corners[i]), screen(corners[(i + 1) % 4]), 0xb7bfcc))
            .collect();
        overlay.extend(
            self.guides
                .iter()
                .map(|(a, b)| (screen(*a), screen(*b), 0xd243ac)),
        );
        if let Some(Drag::Box { start, current }) = self.drag {
            let corners = [
                start,
                Point::new(current.x, start.y),
                current,
                Point::new(start.x, current.y),
            ];
            overlay.extend(
                (0..4).map(|i| (screen(corners[i]), screen(corners[(i + 1) % 4]), 0x3974d7)),
            );
        }
        let origin = screen(Point::default());
        let show_image = self.image_uid == tab.uid;
        overlay.push((
            Point::new(origin.x, t as f64),
            Point::new(origin.x, (t + h as i32) as f64),
            0x85a8bc,
        ));
        overlay.push((
            Point::new(l as f64, origin.y),
            Point::new((l + w as i32) as f64, origin.y),
            0x85a8bc,
        ));
        if self.preview_visible && show_image {
            self.routing.prepare(&tab.doc);
            for c in &tab.doc.connectors {
                let ids = &self.preview_ids;
                if ids.contains(&c.id)
                    || matches!(c.start,Endpoint::Attached{node,..} if ids.contains(&node))
                    || matches!(c.end,Endpoint::Attached{node,..} if ids.contains(&node))
                {
                    let route = self.routing.route(c);
                    overlay.extend(
                        route
                            .points
                            .windows(2)
                            .map(|p| (screen(p[0]), screen(p[1]), c.color)),
                    );
                }
            }
        }
        let sprite = if let Some(drag) = &self.palette_state.drag
            && drag.active
        {
            self.palette_state
                .thumbs
                .get(&drag.key)
                .map(|thumb| paint::Layer {
                    pixels: &thumb.pixels,
                    size: (thumb.size, thumb.size),
                    position: Point::new(
                        drag.current.x - 36. * scale,
                        drag.current.y - 36. * scale,
                    ),
                    destination: (72. * scale, 72. * scale),
                    key: format!("palette:{}:{}", drag.key, thumb.size),
                    opacity: 0.8,
                })
        } else if self.preview_visible && show_image {
            self.sprite.as_ref().map(|sprite| paint::Layer {
                pixels: &sprite.pixels,
                size: sprite.size,
                position: screen(Point::new(
                    sprite.bounds.x + self.drag_offset.x,
                    sprite.bounds.y + self.drag_offset.y,
                )),
                destination: (sprite.bounds.w * scale, sprite.bounds.h * scale),
                key: format!("move:{}", self.pixel_revision),
                opacity: 1.,
            })
        } else {
            None
        };
        let visible = origin.x >= l as f64
            && origin.x < (l + w as i32) as f64
            && origin.y >= t as f64
            && origin.y < (t + h as i32) as f64;
        let origin_label = if visible {
            "原点 (0, 0)  X →  Y ↓".to_owned()
        } else {
            format!(
                "原点 (0, 0) は左上から X={:.0}, Y={:.0} px",
                origin.x - l as f64,
                origin.y - t as f64
            )
        };
        let camera = interaction::Camera {
            origin: tab.origin,
            scale,
        };
        let cached = self.image_camera.unwrap_or(camera);
        let ratio = scale / cached.scale;
        let view = paint::View {
            viewport: Rect {
                x: l as f64,
                y: t as f64,
                w: w as f64,
                h: h as f64,
            },
            strokes: arrow_preview.into_iter().collect(),
            ports: self.connection_markers(),
            grid: if tab.doc.show_grid {
                Some(tab.doc.grid * scale)
            } else {
                None
            },
            base: paint::Layer {
                pixels: &self.pixels,
                size: if show_image { self.pixsize } else { (0, 0) },
                position: camera.position(cached, Point::new(l as f64, t as f64)),
                destination: (self.pixsize.0 as f64 * ratio, self.pixsize.1 as f64 * ratio),
                key: format!("base:{}", self.pixel_revision),
                opacity: 1.,
            },
            sprite,
            origin,
            origin_label,
        };
        self.painter
            .paint(
                self.hwnd,
                (self.width.max(1) as u32, self.height.max(1) as u32),
                &view,
                &overlay,
            )
            .map_err(|e| e.to_string())
    }
    fn properties(&mut self) {
        self.updating = true;
        let tab = &self.tabs[self.tab];
        let id = tab.selected.first().copied();
        let mut values = vec![String::new(); FIELDS.len()];
        values[5] = format!("{:06X}", self.fill_color);
        values[6] = format!("{:06X}", self.line_color);
        values[9] = if self.rounded_lines {
            "丸角"
        } else {
            "尖角"
        }
        .into();
        values[10] = self.corner_radius.to_string();
        let mut text = String::new();
        if let Some(n) = id.and_then(|id| tab.doc.node(id)) {
            let r = n.rect;
            values = vec![
                r.x.to_string(),
                r.y.to_string(),
                r.w.to_string(),
                r.h.to_string(),
                n.font_size.to_string(),
                format!("{:06X}", n.fill),
                format!("{:06X}", n.stroke),
                n.line_width.to_string(),
                "0".into(),
                values[9].clone(),
                values[10].clone(),
            ];
            text = n.label.clone();
        } else if let Some(c) = id.and_then(|id| tab.doc.connectors.iter().find(|c| c.id == id)) {
            values[6] = format!("{:06X}", c.color);
            values[7] = c.width.to_string();
            values[8] = match c.jump {
                Jump::Auto => "0",
                Jump::Always => "1",
                Jump::Never => "2",
            }
            .into();
            values[9] = if c.corner_radius > 0. {
                "丸角"
            } else {
                "尖角"
            }
            .into();
            values[10] = if c.corner_radius > 0. {
                c.corner_radius
            } else {
                self.corner_radius
            }
            .to_string();
            text = c.label.clone();
        }
        unsafe {
            let _ = SetWindowTextW(self.label, &HSTRING::from(&text));
            for (h, value) in self.fields.iter().zip(&values) {
                let _ = SetWindowTextW(*h, &HSTRING::from(value));
            }
            SendMessageW(
                self.fields[9],
                CB_SETCURSEL,
                Some(WPARAM(usize::from(values[9] == "丸角"))),
                None,
            );
        }
        self.property_values = values;
        self.property_label = text;
        self.property_dirty = false;
        self.updating = false;
        unsafe {
            for id in [colors::FILL, colors::LINE] {
                if let Ok(button) = GetDlgItem(Some(self.hwnd), i32::from(id)) {
                    let _ = InvalidateRect(Some(button), None, false);
                }
            }
        }
    }
    fn apply_properties(&mut self) -> bool {
        let values: Vec<_> = self.fields.iter().map(|h| text(*h)).collect();
        let label = text(self.label);
        if !self.property_dirty && values == self.property_values && label == self.property_label {
            return true;
        }
        let mut next = self.tabs[self.tab].doc.clone();
        let ids = self.tabs[self.tab].selected.clone();
        let changed: Vec<_> = values
            .iter()
            .enumerate()
            .map(|(i, v)| self.property_values.get(i) != Some(v))
            .collect();
        for (i, value) in values.iter().enumerate() {
            if changed[i] && i != 8 && i != 9 {
                let valid = if i == 5 || i == 6 {
                    u32::from_str_radix(value.trim_start_matches('#'), 16)
                        .is_ok_and(|v| v <= 0xffffff)
                } else {
                    value.parse::<f64>().is_ok_and(|v| v.is_finite())
                };
                if !valid {
                    self.status(&format!("{} の値を確認してください", FIELDS[i]));
                    return false;
                }
                if i == 10
                    && !value
                        .parse::<f64>()
                        .is_ok_and(|v| (0.1..=10000.).contains(&v))
                {
                    self.status("角の半径は 0.1～10000 DIP で指定してください");
                    return false;
                }
            }
        }
        for n in &mut next.nodes {
            if ids.contains(&n.id) {
                let fields = [
                    &mut n.rect.x,
                    &mut n.rect.y,
                    &mut n.rect.w,
                    &mut n.rect.h,
                    &mut n.font_size,
                ];
                for (i, (field, value)) in fields.into_iter().zip(&values).enumerate() {
                    if !changed[i] {
                        continue;
                    }
                    if let Ok(v) = value.parse::<f64>() {
                        *field = v;
                    }
                }
                if let Ok(v) = u32::from_str_radix(values[5].trim_start_matches('#'), 16)
                    && changed[5]
                {
                    n.fill = v;
                }
                if let Ok(v) = u32::from_str_radix(values[6].trim_start_matches('#'), 16)
                    && changed[6]
                {
                    n.stroke = v;
                }
                if let Ok(v) = values[7].parse()
                    && changed[7]
                {
                    n.line_width = v;
                }
                if label != self.property_label {
                    n.label = label.clone();
                }
            }
        }
        for c in &mut next.connectors {
            if ids.contains(&c.id) {
                if label != self.property_label {
                    c.label = label.clone();
                }
                if let Ok(v) = u32::from_str_radix(values[6].trim_start_matches('#'), 16)
                    && changed[6]
                {
                    c.color = v;
                }
                if let Ok(v) = values[7].parse()
                    && changed[7]
                {
                    c.width = v;
                }
                if changed[8] {
                    c.jump = match values[8].as_str() {
                        "1" => Jump::Always,
                        "2" => Jump::Never,
                        _ => Jump::Auto,
                    };
                }
                if changed[9] || changed[10] {
                    c.corner_radius = if values[9] == "丸角" {
                        values[10].parse().unwrap_or(self.corner_radius)
                    } else {
                        0.
                    };
                }
            }
        }
        match next.validate() {
            Ok(()) => {
                if changed[9] {
                    self.rounded_lines = values[9] == "丸角";
                }
                if changed[10] {
                    self.corner_radius = values[10].parse().unwrap_or(self.corner_radius);
                }
                let tab = self.current();
                let before = std::mem::replace(&mut tab.doc, next);
                tab.history.commit(before, &tab.doc);
                self.property_dirty = false;
                self.property_values = values;
                self.property_label = label;
                self.refresh_tabs();
                self.invalidate();
                true
            }
            Err(e) => {
                self.status(&e);
                false
            }
        }
    }
    fn down(&mut self, p: Point, pan: bool) {
        self.stop_zoom();
        let (l, t, w, h) = self.viewport();
        if p.x < l as f64
            || p.y < t as f64
            || p.x > (l as u32 + w) as f64
            || p.y > (t as u32 + h) as f64
        {
            return;
        }
        if !self.apply_properties() {
            return;
        }
        let q = self.document_point(p);
        self.update_port_hover(p);
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
            SetCapture(self.hwnd);
        }
        if pan || key(VK_SPACE) {
            self.drag = Some(Drag::Pan {
                start: p,
                origin: self.tabs[self.tab].origin,
            });
            return;
        }
        match self.tool {
            Tool::Shape(_) | Tool::Icon(_) | Tool::LibraryIcon(_) | Tool::CatalogIcon(_) => {
                let service_label = if let Tool::CatalogIcon(index) = self.tool {
                    yy_draw::catalog::all().get(index).map(|e| e.name.clone())
                } else {
                    None
                };
                let asset = if let Tool::CatalogIcon(index) = self.tool {
                    yy_draw::catalog::place(&mut self.tabs[self.tab].doc, index).ok()
                } else if let Tool::LibraryIcon(id) = self.tool {
                    let asset = self.library.assets.iter().find(|a| a.id == id).cloned();
                    if let Some(asset) = asset {
                        let mut bundle = Document::default();
                        bundle.assets.push(asset.clone());
                        bundle.next_id = self.library.next_id;
                        bundle.packs = self
                            .library
                            .packs
                            .iter()
                            .filter(|p| p.assets.contains(&id))
                            .cloned()
                            .map(|mut p| {
                                p.assets = vec![id];
                                p
                            })
                            .collect();
                        self.tabs[self.tab].doc.paste(&bundle, Point::default());
                        self.tabs[self.tab]
                            .doc
                            .assets
                            .iter()
                            .find(|a| a.svg == asset.svg)
                            .map(|a| a.id)
                    } else {
                        None
                    }
                } else if let Tool::Icon(id) = self.tool {
                    Some(id)
                } else {
                    None
                };
                let shape = if asset.is_some() {
                    Shape::Icon
                } else {
                    shape_from_tool(self.tool)
                };
                let fill = self.fill_color;
                let line = self.line_color;
                let tab = self.current();
                let before = tab.doc.clone();
                let id = if shape == Shape::Junction {
                    yy_draw::editing::junction(&mut tab.doc, q, 10. / tab.zoom)
                } else {
                    tab.doc.add_node(shape, q, asset)
                };
                if let Some(n) = tab.doc.nodes.iter_mut().find(|n| n.id == id) {
                    n.fill = fill;
                    n.stroke = line;
                }
                if let Some(label) = service_label
                    && let Some(n) = tab.doc.nodes.iter_mut().find(|n| n.id == id)
                {
                    n.label = label;
                }
                tab.selected = vec![id];
                tab.history.commit(before, &tab.doc);
                self.tool = Tool::Select;
                self.properties();
                self.refresh_tabs();
                self.invalidate();
            }
            Tool::Connect(orthogonal) => {
                let start = self.tabs[self.tab]
                    .doc
                    .attach_near(q, 10. / self.tabs[self.tab].zoom);
                self.drag = Some(Drag::Connect {
                    start,
                    draft: drawing::Draft::new(self.tabs[self.tab].doc.endpoint(start)),
                    orthogonal,
                });
            }
            Tool::Select => {
                let doc = &self.tabs[self.tab].doc;
                // A visible resize handle takes precedence over ports and objects
                // behind it, including the selected object's corner ports.
                if let [id] = self.tabs[self.tab].selected.as_slice()
                    && doc.node(*id).is_some_and(|n| {
                        !n.locked
                            && Point::new(n.rect.x + n.rect.w, n.rect.y + n.rect.h).distance(q)
                                < 6. / self.tabs[self.tab].zoom
                    })
                {
                    self.drag = Some(Drag::Resize {
                        before: doc.clone(),
                        id: *id,
                    });
                    return;
                }
                if let Some((id, index)) = doc
                    .connectors
                    .iter()
                    .filter(|c| self.tabs[self.tab].selected.contains(&c.id))
                    .find_map(|c| {
                        c.waypoints
                            .iter()
                            .position(|p| p.distance(q) < 10. / self.tabs[self.tab].zoom)
                            .map(|i| (c.id, i))
                    })
                {
                    self.drag = Some(Drag::Waypoint {
                        before: doc.clone(),
                        id,
                        index,
                    });
                    return;
                }
                if let Some((id, start)) = doc
                    .connectors
                    .iter()
                    .filter(|c| self.tabs[self.tab].selected.contains(&c.id))
                    .find_map(|c| {
                        if doc.endpoint(c.start).distance(q) < 10. / self.tabs[self.tab].zoom {
                            Some((c.id, true))
                        } else if doc.endpoint(c.end).distance(q) < 10. / self.tabs[self.tab].zoom {
                            Some((c.id, false))
                        } else {
                            None
                        }
                    })
                {
                    self.drag = Some(Drag::Endpoint {
                        before: doc.clone(),
                        id,
                        start,
                    });
                    return;
                }
                if !key(VK_MENU)
                    && let Some(start) = doc.nearest_port(q, 6. / self.tabs[self.tab].zoom)
                {
                    self.drag = Some(Drag::Connect {
                        start,
                        draft: drawing::Draft::new(doc.endpoint(start)),
                        orthogonal: true,
                    });
                    self.invalidate_overlay();
                    return;
                }
                let hit = geometry::hit(&self.tabs[self.tab].doc, q, 6. / self.tabs[self.tab].zoom);
                let tab = self.current();
                if let Some(id) = hit {
                    let unit = if key(VK_CONTROL) {
                        vec![id]
                    } else {
                        tab.doc.expanded_selection(&[id])
                    };
                    if key(VK_SHIFT) {
                        if unit.iter().all(|id| tab.selected.contains(id)) {
                            let ids: std::collections::HashSet<_> = unit.into_iter().collect();
                            tab.selected.retain(|id| !ids.contains(id));
                        } else {
                            let mut ids: std::collections::HashSet<_> =
                                tab.selected.iter().copied().collect();
                            tab.selected
                                .extend(unit.into_iter().filter(|id| ids.insert(*id)));
                        }
                    } else if !tab.selected.contains(&id) {
                        tab.selected = unit;
                    }
                    if !key(VK_CONTROL) {
                        tab.selected = tab.doc.expanded_selection(&tab.selected);
                    }
                    let resize = tab.selected.len() == 1
                        && tab.doc.node(id).is_some_and(|n| {
                            Point::new(n.rect.x + n.rect.w, n.rect.y + n.rect.h).distance(q)
                                < 12. / tab.zoom
                        });
                    self.drag = Some(if resize {
                        Drag::Resize {
                            before: self.tabs[self.tab].doc.clone(),
                            id,
                        }
                    } else {
                        Drag::Move {
                            before: self.tabs[self.tab].doc.clone(),
                            start: q,
                            selection: editing::MoveSelection::new(
                                &self.tabs[self.tab].doc,
                                &self.tabs[self.tab].selected,
                            ),
                        }
                    });
                    self.drag_offset = Point::default();
                    self.preview_visible = false;
                } else {
                    if !key(VK_SHIFT) {
                        tab.selected.clear();
                    }
                    self.drag = Some(Drag::Box {
                        start: q,
                        current: q,
                    });
                }
                self.properties();
                self.invalidate();
            }
        }
    }
    fn motion(&mut self, p: Point) {
        self.follow_zoom_pointer(p, std::time::Instant::now());
        self.update_port_hover(p);
        if let Some(drag) = &mut self.palette_state.drag {
            drag.current = p;
            drag.active |= p.distance(drag.start) > 4. * self.dpi;
            self.invalidate_overlay();
            return;
        }
        let q = self.document_point(p);
        let tab = &mut self.tabs[self.tab];
        match &mut self.drag {
            Some(Drag::Waypoint { id, index, .. }) => {
                let q = if key(VK_MENU) {
                    q
                } else {
                    tab.doc.snap_point(q)
                };
                if let Some(c) = tab.doc.connectors.iter_mut().find(|c| c.id == *id)
                    && let Some(p) = c.waypoints.get_mut(*index)
                {
                    *p = q;
                }
            }
            Some(Drag::Endpoint { id, start, .. }) => {
                let endpoint = tab.doc.attach_near(q, 10. / tab.zoom);
                if let Some(c) = tab.doc.connectors.iter_mut().find(|c| c.id == *id) {
                    if *start {
                        c.start = endpoint;
                    } else {
                        c.end = endpoint;
                    }
                }
            }
            Some(Drag::Move {
                before,
                start,
                selection,
            }) => {
                let mut d = Point::new(q.x - start.x, q.y - start.y);
                if !key(VK_MENU) {
                    d = tab.doc.snap_point(d);
                    let (aligned, guides) =
                        yy_draw::editing::align(before, &tab.selected, d, 5. / tab.zoom);
                    d = aligned;
                    self.guides = guides;
                } else {
                    self.guides.clear();
                }
                selection.apply(&mut tab.doc, d);
                self.drag_offset = d;
            }
            Some(Drag::Resize { before, id }) => {
                let q = if key(VK_MENU) {
                    q
                } else {
                    tab.doc.snap_point(q)
                };
                if let Some(n) = tab.doc.nodes.iter_mut().find(|n| n.id == *id) {
                    let original = before.node(*id).unwrap().rect;
                    let ratio = original.w / original.h;
                    n.rect = original;
                    n.rect.w = (q.x - n.rect.x).max(16.);
                    n.rect.h = (q.y - n.rect.y).max(16.);
                    if n.shape == Shape::Icon || key(VK_SHIFT) {
                        n.rect.h = n.rect.w / ratio;
                    }
                }
            }
            Some(Drag::Pan { start, origin }) => {
                tab.origin = Point::new(
                    origin.x - (p.x - start.x) / (tab.zoom * self.dpi),
                    origin.y - (p.y - start.y) / (tab.zoom * self.dpi),
                );
            }
            Some(Drag::Box { current, .. }) => *current = q,
            Some(Drag::Connect { draft, .. }) => {
                let pointer = if key(VK_MENU) {
                    q
                } else {
                    tab.doc.snap_point(q)
                };
                draft.update(pointer, key(VK_SHIFT));
            }
            None => return,
        }
        if matches!(
            self.drag,
            Some(Drag::Move { .. } | Drag::Connect { .. } | Drag::Box { .. })
        ) {
            self.invalidate_overlay();
        } else if matches!(self.drag, Some(Drag::Pan { .. })) {
            self.view_changed();
        } else {
            self.invalidate();
        }
        self.update_port_hover(p);
    }
    fn up(&mut self, p: Point) {
        let line_color = self.line_color;
        let radius = if self.rounded_lines {
            self.corner_radius
        } else {
            0.
        };
        self.motion(p);
        if let Some(drag) = self.drag.take() {
            let tab = self.current();
            match drag {
                Drag::Waypoint { before, .. }
                | Drag::Move { before, .. }
                | Drag::Resize { before, .. }
                | Drag::Endpoint { before, .. } => tab.history.commit(before, &tab.doc),
                Drag::Box { start, current } => {
                    let r = Rect {
                        x: start.x.min(current.x),
                        y: start.y.min(current.y),
                        w: (start.x - current.x).abs(),
                        h: (start.y - current.y).abs(),
                    };
                    for n in &tab.doc.nodes {
                        if r.contains(n.rect.center()) && !tab.selected.contains(&n.id) {
                            tab.selected.push(n.id);
                        }
                    }
                    tab.selected = tab.doc.expanded_selection(&tab.selected);
                }
                Drag::Connect {
                    start,
                    draft,
                    orthogonal,
                } => {
                    if tab.doc.endpoint(start).distance(draft.current) > 4.
                        || !draft.bends.is_empty()
                    {
                        let before = tab.doc.clone();
                        let end = draft.endpoint(&tab.doc, 10. / tab.zoom);
                        let id = tab.doc.connect(start, end, orthogonal && !draft.manual);
                        let c = tab.doc.connectors.last_mut().unwrap();
                        c.color = line_color;
                        c.corner_radius = radius;
                        c.waypoints = draft.bends;
                        tab.selected = vec![id];
                        tab.history.commit(before, &tab.doc);
                        self.pending_connector = Some((tab.uid, id));
                    }
                }
                Drag::Pan { .. } => {}
            }
        }
        unsafe {
            let _ = ReleaseCapture();
        }
        self.refresh_tabs();
        self.properties();
        self.guides.clear();
        self.invalidate();
    }
}
fn shape_from_tool(t: Tool) -> Shape {
    if let Tool::Shape(s) = t {
        s
    } else {
        Shape::Icon
    }
}
fn text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd).max(0) as usize;
        let mut data = vec![0; len + 1];
        let n = GetWindowTextW(hwnd, &mut data);
        String::from_utf16_lossy(&data[..n.max(0) as usize])
    }
}

fn menu() -> Result<HMENU> {
    unsafe {
        let root = CreateMenu()?;
        for (name, items) in [
            (
                "ファイル",
                vec![
                    (NEW, "新規\tCtrl+N"),
                    (OPEN, "開く…\tCtrl+O"),
                    (SAVE, "保存\tCtrl+S"),
                    (SAVE_AS, "名前を付けて保存…"),
                    (SVG, "SVG にエクスポート…"),
                    (PNG, "PNG にエクスポート…"),
                    (EXPORT_OPTIONS, "出力の設定…"),
                    (IMPORT, "SVG・PNG 素材を追加…"),
                    (36, "素材フォルダを追加…"),
                    (CLOSE, "タブを閉じる\tCtrl+W"),
                ],
            ),
            (
                "編集",
                vec![
                    (UNDO, "元に戻す\tCtrl+Z"),
                    (REDO, "やり直す\tCtrl+Y"),
                    (CUT, "切り取り\tCtrl+X"),
                    (COPY, "コピー\tCtrl+C"),
                    (PASTE, "貼り付け\tCtrl+V"),
                    (DUPLICATE, "複製\tCtrl+D"),
                    (DELETE, "削除\tDelete"),
                    (SELECT_ALL, "すべて選択\tCtrl+A"),
                ],
            ),
            ("配置", arrange::ITEMS.to_vec()),
            (
                "表示・接続",
                vec![
                    (SELECT, "選択ツール\tEsc"),
                    (STRAIGHT, "直線・矢印"),
                    (ORTHO, "直交線・矢印"),
                    (GRID, "方眼紙の表示"),
                    (SNAP, "格子に吸着"),
                    (JUMPS, "交差でアーチを描く"),
                    (37, "格子間隔…"),
                    (38, "アーチの半径…"),
                    (39, "フォント…"),
                    (40, "選択線の終点矢印を切り替え"),
                    (41, "選択線の始点矢印を切り替え"),
                    (42, "選択線の破線を切り替え"),
                    (FIT, "図全体を表示"),
                ],
            ),
            ("ヘルプ", vec![(HELP, "操作ガイド\tF1")]),
        ] {
            let popup = CreatePopupMenu()?;
            for (id, label) in items {
                AppendMenuW(popup, MF_STRING, id as usize, &HSTRING::from(label))?;
            }
            AppendMenuW(root, MF_POPUP, popup.0 as usize, &HSTRING::from(name))?;
        }
        features::add_menus(root)?;
        Ok(root)
    }
}
fn shortcut(hwnd: HWND, k: u16) -> bool {
    if with(|a| a.wire_key(k)).unwrap_or(false) {
        return true;
    }
    let ctrl = key(VK_CONTROL);
    let shift = key(VK_SHIFT);
    let id = if ctrl {
        match k {
            0x4e => NEW,
            0x4f => OPEN,
            0x53 => {
                if shift {
                    SAVE_AS
                } else {
                    SAVE
                }
            }
            0x57 => CLOSE,
            0x5a => {
                if shift {
                    REDO
                } else {
                    UNDO
                }
            }
            0x59 => REDO,
            0x44 => DUPLICATE,
            0x41 => SELECT_ALL,
            0x43 => COPY,
            0x56 => PASTE,
            0x58 => CUT,
            0x47 => {
                if shift {
                    UNGROUP
                } else {
                    GROUP
                }
            }
            _ => 0,
        }
    } else {
        match k {
            0x2e => DELETE,
            0x1b => SELECT,
            0x71 => {
                with(|a| unsafe {
                    let _ = SetFocus(Some(a.label));
                });
                return true;
            }
            0x70 => HELP,
            0x25..=0x28 => {
                with(|a| {
                    let step = if shift { 1. } else { a.tabs[a.tab].doc.grid };
                    let (dx, dy) = match k {
                        0x25 => (-step, 0.),
                        0x26 => (0., -step),
                        0x27 => (step, 0.),
                        _ => (0., step),
                    };
                    let tab = a.current();
                    let before = tab.doc.clone();
                    tab.doc.translate(&tab.selected, dx, dy);
                    tab.history.commit(before, &tab.doc);
                    a.properties();
                    a.refresh_tabs();
                    a.invalidate();
                });
                return true;
            }
            _ => 0,
        }
    };
    if id == 0 {
        false
    } else {
        command(hwnd, id);
        true
    }
}
fn command(hwnd: HWND, id: u16) {
    if id >= 100 {
        return;
    }
    if id == CANCEL {
        features::handle(hwnd, id);
        return;
    }
    if id == colors::FILL || id == colors::LINE {
        colors::choose(hwnd, id);
        return;
    }
    if !with(|a| a.apply_properties()).unwrap_or(true) {
        return;
    }
    if arrange::handle(hwnd, id) {
        return;
    }
    if features::handle(hwnd, id) {
        return;
    }
    if matches!(id, OPEN | IMPORT | 36) {
        let folder = id == 36;
        if let Some(path) = dialog(
            hwnd,
            false,
            if id == OPEN { "yyd" } else { "svg;*.png" },
            folder,
        ) {
            if id == OPEN {
                open_path(hwnd, path);
            } else {
                import_assets(hwnd, path);
            }
        }
        return;
    }
    if matches!(id, SAVE | SAVE_AS) {
        save_current(hwnd, id == SAVE_AS);
        return;
    }
    if matches!(id, SVG | PNG) {
        if with(|a| a.busy()).unwrap_or(true) {
            return;
        }
        if let Some(path) = dialog(hwnd, true, if id == SVG { "svg" } else { "png" }, false) {
            with(|a| match a.export_document() {
                Ok((doc, options)) => {
                    let renderer = a.renderer.clone();
                    let scale = a.export_scale;
                    a.work = Some(yy_draw::jobs::Task::spawn(move |c| {
                        c.checkpoint(5, "図を出力中")?;
                        let bytes = if id == SVG {
                            renderer.svg(&doc, &options)?.into_bytes()
                        } else {
                            renderer.png(&doc, &options, scale)?
                        };
                        c.checkpoint(70, "出力を保存中")?;
                        format::atomic_write_control(&path, &bytes, &c)?;
                        Ok(work::Outcome::Exported)
                    }));
                }
                Err(e) => a.status(&e),
            });
        }
        return;
    }
    if id == CLOSE {
        close_tab(hwnd);
        return;
    }
    if id == HELP {
        info_box(hwnd, include_str!("../../help/draw.md"));
        return;
    }
    if id == EXPORT_OPTIONS {
        let initial = with(|a| a.export_scale.to_string()).unwrap_or_default();
        if let Some(value) = crate::goto::prompt_text(hwnd, "PNG 出力", "倍率 (0.1～8)", &initial)
        {
            match value.parse::<f64>() {
                Ok(v) if (0.1..=8.).contains(&v) => {
                    with(|a| a.export_scale = v);
                }
                _ => error_box(hwnd, "倍率を確認してください"),
            }
        }
        return;
    }
    if matches!(id, 37..=39) {
        let initial = with(|a| match id {
            37 => a.tabs[a.tab].doc.grid.to_string(),
            38 => a.tabs[a.tab].doc.jump_radius.to_string(),
            _ => a.tabs[a.tab].doc.font_family.clone(),
        })
        .unwrap_or_default();
        if let Some(value) = crate::goto::prompt_text(
            hwnd,
            "図の設定",
            match id {
                37 => "格子間隔 (1～1000 DIP)",
                38 => "アーチの半径 (0.5～100 DIP)",
                _ => "フォント名",
            },
            &initial,
        ) {
            with(|a| {
                let tab = a.current();
                let before = tab.doc.clone();
                match id {
                    37 => {
                        if let Ok(v) = value.parse() {
                            tab.doc.grid = v
                        }
                    }
                    38 => {
                        if let Ok(v) = value.parse() {
                            tab.doc.jump_radius = v
                        }
                    }
                    _ => tab.doc.font_family = value,
                }
                if tab.doc.validate().is_err() {
                    tab.doc = before;
                } else {
                    tab.history.commit(before, &tab.doc);
                }
                a.refresh_tabs();
                a.invalidate();
            });
        }
        return;
    }
    with(|a| {
        a.apply_properties();
        if id == NEW {
            let source = &a.tabs[a.tab].doc;
            let doc = Document {
                font_family: source.font_family.clone(),
                font_size: source.font_size,
                ..Default::default()
            };
            a.tabs.push(Tab::new(doc, None));
            a.tab = a.tabs.len() - 1;
            a.rebuild_palette();
        } else if id == APPLY {
            a.properties();
        } else if id == SELECT {
            a.palette_state.drag = None;
            a.tool = Tool::Select;
            if let Some(
                Drag::Waypoint { before, .. }
                | Drag::Move { before, .. }
                | Drag::Resize { before, .. }
                | Drag::Endpoint { before, .. },
            ) = a.drag.take()
            {
                a.tabs[a.tab].doc = before;
                a.drag_offset = Point::default();
            }
            unsafe {
                let _ = ReleaseCapture();
            }
        } else if id == STRAIGHT || id == ORTHO {
            a.tool = Tool::Connect(id == ORTHO);
            a.status(
                "ドラッグで接続 / Shift: 上下左右 / Shift+Tab: 折れ点 / Backspace: 折れ点を戻す",
            );
        } else if id == FIT {
            let (_, _, w, h) = a.viewport();
            let dpi = a.dpi;
            let tab = a.current();
            let b = tab.doc.bounds();
            tab.zoom = ((w as f64 / b.w).min(h as f64 / b.h) / dpi).clamp(0.1, 8.);
            tab.origin = Point::new(b.x, b.y);
        } else {
            let tab = &mut a.tabs[a.tab];
            let before = tab.doc.clone();
            match id {
                UNDO => {
                    tab.history.undo(&mut tab.doc);
                    tab.selected.clear();
                }
                REDO => {
                    tab.history.redo(&mut tab.doc);
                    tab.selected.clear();
                }
                DELETE => {
                    tab.doc.delete(&tab.selected);
                    tab.selected.clear();
                }
                DUPLICATE => {
                    tab.selected = tab.doc.duplicate(&tab.selected);
                }
                COPY | CUT => {
                    let fragment = tab.doc.fragment(&tab.selected);
                    if !features::copy_shapes(a.hwnd, &fragment) {
                        return;
                    }
                    a.clipboard = Some((
                        fragment.clone(),
                        fragment
                            .nodes
                            .iter()
                            .map(|n| n.id)
                            .chain(fragment.connectors.iter().map(|c| c.id))
                            .collect(),
                    ));
                    if id == CUT {
                        tab.doc.delete(&tab.selected);
                        tab.selected.clear();
                    }
                }
                PASTE => {
                    if let Some(source) = features::pasted_shapes(a.hwnd, &a.clipboard) {
                        tab.selected = tab.doc.paste(&source, Point::new(16., 16.));
                    }
                }
                SELECT_ALL => {
                    tab.selected = tab
                        .doc
                        .nodes
                        .iter()
                        .map(|n| n.id)
                        .chain(tab.doc.connectors.iter().map(|c| c.id))
                        .collect();
                }
                GRID => tab.doc.show_grid = !tab.doc.show_grid,
                SNAP => tab.doc.snap = !tab.doc.snap,
                JUMPS => tab.doc.jumps = !tab.doc.jumps,
                FRONT | BACK => {
                    let selected = tab.selected.clone();
                    tab.doc.nodes.sort_by_key(|n| {
                        if id == FRONT {
                            selected.contains(&n.id)
                        } else {
                            !selected.contains(&n.id)
                        }
                    });
                    tab.doc.connectors.sort_by_key(|c| {
                        if id == FRONT {
                            selected.contains(&c.id)
                        } else {
                            !selected.contains(&c.id)
                        }
                    });
                }
                40..=42 => {
                    for c in &mut tab.doc.connectors {
                        if tab.selected.contains(&c.id) {
                            match id {
                                40 => c.arrow_end = !c.arrow_end,
                                41 => c.arrow_start = !c.arrow_start,
                                _ => c.dashed = !c.dashed,
                            }
                        }
                    }
                }
                _ => {}
            }
            if !matches!(id, UNDO | REDO) {
                tab.history.commit(before, &tab.doc);
            }
        }
        a.properties();
        a.rebuild_palette();
        a.refresh_tabs();
        a.invalidate();
    });
}
fn open_path(_hwnd: HWND, path: PathBuf) {
    with(|a| {
        if a.busy() {
            return;
        }
        a.work = Some(yy_draw::jobs::Task::spawn(move |c| {
            c.checkpoint(5, "文書を読み込み中")?;
            let doc = format::load(&path)?;
            c.checkpoint(95, "文書を表示")?;
            Ok(work::Outcome::Opened { path, doc })
        }));
    });
}
fn save_current(hwnd: HWND, save_as: bool) -> bool {
    if !with(|a| a.apply_properties()).unwrap_or(false) || with(|a| a.busy()).unwrap_or(true) {
        return false;
    }
    let current = with(|a| a.tabs[a.tab].path.clone()).flatten();
    let path = if save_as || current.is_none() {
        dialog(hwnd, true, "yyd", false)
    } else {
        current
    };
    let Some(path) = path else {
        return false;
    };
    with(|a| a.begin_save(path)).unwrap_or(false)
}
fn close_tab(hwnd: HWND) -> bool {
    if with(|a| a.busy()).unwrap_or(false) {
        return false;
    }
    if !with(|a| a.apply_properties()).unwrap_or(false) {
        return false;
    }
    let dirty = with(|a| a.tabs[a.tab].dirty()).unwrap_or(false);
    if dirty {
        match unsafe {
            MessageBoxW(
                Some(hwnd),
                w!("変更を保存しますか？"),
                w!("yydraw"),
                MB_YESNOCANCEL | MB_ICONQUESTION,
            )
        } {
            IDCANCEL => {
                with(|a| a.closing_all = false);
                return false;
            }
            IDYES => {
                if save_current(hwnd, false) {
                    with(|a| a.close_after = Some(a.tabs[a.tab].uid));
                }
                return false;
            }
            _ => {}
        }
    }
    with(|a| {
        a.tabs.remove(a.tab);
        if a.tabs.is_empty() {
            a.tabs.push(Tab::new(Document::default(), None));
        }
        a.tab = a.tab.min(a.tabs.len() - 1);
        a.rebuild_palette();
        a.refresh_tabs();
        a.properties();
        a.invalidate();
    });
    true
}
fn close_all(hwnd: HWND) -> bool {
    let count = with(|a| a.tabs.len()).unwrap_or(0);
    for _ in 0..count {
        if !close_tab(hwnd) {
            return false;
        }
    }
    true
}
fn dialog(hwnd: HWND, save: bool, ext: &str, folder: bool) -> Option<PathBuf> {
    unsafe {
        let spec = HSTRING::from(format!("*.{ext}"));
        let title = HSTRING::from(if folder { "素材フォルダ" } else { ext });
        let filter = [COMDLG_FILTERSPEC {
            pszName: PCWSTR(title.as_ptr()),
            pszSpec: PCWSTR(spec.as_ptr()),
        }];
        let item = if save {
            let d: IFileSaveDialog =
                CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            d.SetFileTypes(&filter).ok()?;
            d.SetDefaultExtension(&HSTRING::from(ext)).ok()?;
            d.Show(Some(hwnd)).ok()?;
            d.GetResult().ok()?
        } else {
            let d: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            if folder {
                d.SetOptions(d.GetOptions().ok()? | windows::Win32::UI::Shell::FOS_PICKFOLDERS)
                    .ok()?;
            } else {
                d.SetFileTypes(&filter).ok()?;
            }
            d.Show(Some(hwnd)).ok()?;
            d.GetResult().ok()?
        };
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(name.0.cast()));
        path
    }
}
fn import_assets(_hwnd: HWND, path: PathBuf) {
    with(|a| {
        a.begin_import(move |renderer, c| {
            fn visit(
                path: &Path,
                out: &mut Vec<PathBuf>,
                depth: usize,
                c: &yy_draw::jobs::Control,
            ) -> std::result::Result<(), String> {
                c.checkpoint(10, "素材を検索中")?;
                if depth > 12 || out.len() >= 5000 {
                    return Err("素材フォルダは 12 階層・5000 個までです".into());
                }
                if path.is_file() {
                    if path.extension().is_some_and(|e| {
                        e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("png")
                    }) {
                        out.push(path.to_owned());
                    }
                } else {
                    for item in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                        let item = item.map_err(|e| e.to_string())?;
                        if !item.file_type().map_err(|e| e.to_string())?.is_symlink() {
                            visit(&item.path(), out, depth + 1, c)?;
                        }
                    }
                }
                Ok(())
            }
            use std::io::Read;
            let mut files = vec![];
            visit(&path, &mut files, 0, c)?;
            files.sort();
            if files.is_empty() {
                return Err("SVG・PNG 素材が見つかりません".into());
            }
            let mut doc = Document::default();
            for (i, file) in files.iter().enumerate() {
                c.checkpoint(10 + (i * 40 / files.len()) as u32, "素材を変換中")?;
                let mut bytes = vec![];
                std::fs::File::open(file)
                    .map_err(|e| e.to_string())?
                    .take(8 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                let id = doc.id();
                let name = file
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .replace('_', " ");
                doc.assets.push(
                    renderer.normalize_asset(
                        &bytes,
                        name,
                        file.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        id,
                    )?,
                );
            }
            let ids = doc.assets.iter().map(|a| a.id).collect::<Vec<_>>();
            let digest = yy_draw::library::digest(&doc, &ids);
            doc.packs.push(yy_draw::library::Pack {
                id: format!(
                    "local-{}",
                    path.file_stem().unwrap_or_default().to_string_lossy()
                ),
                version: digest[..12].into(),
                source: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                terms: "利用者が追加した素材。提供元の条件に従って利用してください。".into(),
                digest,
                assets: ids,
                enabled: true,
            });
            doc.validate()?;
            Ok(doc)
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    struct ShiftGuard([u8; 256]);
    impl ShiftGuard {
        fn press() -> Self {
            unsafe {
                let mut state = [0; 256];
                GetKeyboardState(&mut state).unwrap();
                let original = state;
                state[VK_SHIFT.0 as usize] |= 0x80;
                SetKeyboardState(&state).unwrap();
                Self(original)
            }
        }
    }
    impl Drop for ShiftGuard {
        fn drop(&mut self) {
            unsafe {
                SetKeyboardState(&self.0).unwrap();
            }
        }
    }
    struct TestWindow(HWND);
    impl Drop for TestWindow {
        fn drop(&mut self) {
            unsafe {
                let font = APP.with(|a| a.borrow_mut().take().map(|a| a.font));
                let _ = DestroyWindow(self.0);
                if let Some(font) = font {
                    let _ = DeleteObject(HGDIOBJ(font.0));
                }
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool() {}
            }
        }
    }
    fn mouse(hwnd: HWND, msg: u32, p: Point) {
        unsafe {
            SendMessageW(
                hwnd,
                msg,
                None,
                Some(LPARAM(
                    ((p.y as i32 as u32) << 16 | (p.x as i32 as u32 & 0xffff)) as isize,
                )),
            );
        }
    }
    fn screen(p: Point) -> Point {
        with(|a| {
            let (l, t, _, _) = a.viewport();
            let tab = &a.tabs[a.tab];
            Point::new(
                l as f64 + (p.x - tab.origin.x) * a.dpi * tab.zoom,
                t as f64 + (p.y - tab.origin.y) * a.dpi * tab.zoom,
            )
        })
        .unwrap()
    }
    fn wait_canvas() {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if with(|a| {
                a.tick_zoom(std::time::Instant::now());
                a.poll_work();
                a.paint().unwrap();
                a.frame.is_none() && !a.dirty
            })
            .unwrap()
            {
                return;
            }
            assert!(std::time::Instant::now() < end);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    fn wait_work() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let done = with(|a| {
                a.poll_work();
                assert!(a.pending_error.is_none(), "{:?}", a.pending_error);
                a.work.is_none()
            })
            .unwrap();
            if done {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "job timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    #[test]
    fn yydraw_native_edit_connect_save_and_export() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            crate::font::register_gdi();
            let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_TAB_CLASSES,
            });
            RegisterClassExW(&WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                hInstance: GetModuleHandleW(None).unwrap().into(),
                lpszClassName: CLASS,
                lpfnWndProc: Some(proc),
                ..Default::default()
            });
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS,
                w!("yydraw native test"),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                0,
                0,
                1200,
                850,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let window = TestWindow(hwnd);
            let mut app = App::new(hwnd, Document::default(), None).unwrap();
            // Exercise the canvas on a 1024 x 768 logical desktop as well as
            // on high-DPI desktops, without requiring a larger CI monitor.
            SetWindowPos(
                hwnd,
                None,
                0,
                0,
                (1024. * app.dpi) as i32,
                (768. * app.dpi) as i32,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .unwrap();
            let mut client = RECT::default();
            GetClientRect(hwnd, &mut client).unwrap();
            app.width = client.right;
            app.height = client.bottom;
            app.create_controls().unwrap();
            app.layout();
            app.refresh_tabs();
            app.properties();
            APP.with(|a| *a.borrow_mut() = Some(app));
            assert_eq!(with(|a| a.palette_state.items.len()).unwrap(), SHAPES.len());
            for (shape, p) in [
                (Shape::Rounded, Point::new(40., 40.)),
                (Shape::Cylinder, Point::new(360., 160.)),
            ] {
                with(|a| a.tool = Tool::Shape(shape));
                mouse(hwnd, WM_LBUTTONDOWN, screen(p));
                mouse(hwnd, WM_LBUTTONUP, screen(p));
            }
            let ids =
                with(|a| a.tabs[0].doc.nodes.iter().map(|n| n.id).collect::<Vec<_>>()).unwrap();
            assert_eq!(ids.len(), 2);
            let label = with(|a| {
                a.tabs[0].selected = vec![ids[0]];
                a.properties();
                a.label
            })
            .unwrap();
            SetWindowTextW(label, w!("日本語ラベル\r\nArchitecture")).unwrap();
            command(hwnd, APPLY);
            assert!(with(|a| a.tabs[0].doc.nodes[0].label.starts_with("日本語ラベル")).unwrap());
            with(|a| a.tool = Tool::Connect(true));
            wait_canvas();
            mouse(hwnd, WM_LBUTTONDOWN, screen(Point::new(184., 80.)));
            let revision = with(|a| a.revision).unwrap();
            mouse(hwnd, WM_MOUSEMOVE, screen(Point::new(360., 200.)));
            with(|a| {
                a.paint().unwrap();
                assert_eq!(a.revision, revision);
                assert!(a.frame.is_none());
                assert_eq!(a.tabs[0].doc.connectors.len(), 0);
                let preview = a.arrow_preview().unwrap();
                assert!(preview.points.len() >= 2);
                assert!(preview.arrow_size > 0.);
            });
            mouse(hwnd, WM_LBUTTONUP, screen(Point::new(360., 200.)));
            assert!(with(|a|matches!(a.tabs[0].doc.connectors[0].start,Endpoint::Attached{node,..}if node==ids[0])).unwrap());
            let endpoints = with(|a| {
                let d = &a.tabs[0].doc;
                (
                    d.endpoint(d.connectors[0].start),
                    d.endpoint(d.connectors[0].end),
                )
            })
            .unwrap();
            with(|a| {
                a.tool = Tool::Select;
                a.tabs[0].selected.clear();
            });
            mouse(hwnd, WM_LBUTTONDOWN, screen(Point::new(90., 70.)));
            let revision = with(|a| a.revision).unwrap();
            mouse(hwnd, WM_MOUSEMOVE, screen(Point::new(122., 94.)));
            assert_eq!(
                with(|a| a.revision).unwrap(),
                revision,
                "moving a sprite must not restart full-scene rendering"
            );
            mouse(hwnd, WM_LBUTTONUP, screen(Point::new(122., 94.)));
            let moved = with(|a| {
                let d = &a.tabs[0].doc;
                d.endpoint(d.connectors[0].start)
            })
            .unwrap();
            assert_eq!(moved, Point::new(endpoints.0.x + 32., endpoints.0.y + 24.));
            command(hwnd, UNDO);
            assert_eq!(
                with(|a| {
                    let d = &a.tabs[0].doc;
                    d.endpoint(d.connectors[0].start)
                })
                .unwrap(),
                endpoints.0
            );
            command(hwnd, REDO);
            // Changing a shared property must not collapse the positions of a multiple selection.
            let field = with(|a| {
                a.tabs[0].selected = ids.clone();
                a.properties();
                a.fields[5]
            })
            .unwrap();
            SetWindowTextW(field, w!("FFDD88")).unwrap();
            command(hwnd, APPLY);
            assert!(
                with(
                    |a| a.tabs[0].doc.nodes[0].rect.x != a.tabs[0].doc.nodes[1].rect.x
                        && a.tabs[0].doc.nodes.iter().all(|n| n.fill == 0xffdd88)
                )
                .unwrap()
            );
            command(hwnd, COPY);
            with(|a| assert!(a.apply_chosen_color(true, 0x1090d0)));
            assert!(with(|a| a.tabs[0].doc.nodes.iter().all(|n| n.fill == 0x1090d0)).unwrap());
            command(hwnd, UNDO);
            assert!(with(|a| a.tabs[0].doc.nodes.iter().all(|n| n.fill == 0xffdd88)).unwrap());
            with(|a| {
                a.tabs[0].selected = ids.clone();
                a.properties();
            });
            command(hwnd, PASTE);
            assert_eq!(with(|a| a.tabs[0].doc.nodes.len()).unwrap(), 4);
            with(|a| a.tabs[0].doc.validate().unwrap());
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("test.yyd");
            with(|a| a.tabs[0].path = Some(path.clone()));
            assert!(save_current(hwnd, false));
            wait_work();
            let saved = format::load(&path).unwrap();
            assert_eq!(saved, with(|a| a.tabs[0].doc.clone()).unwrap());
            assert!(!with(|a| a.tabs[0].dirty()).unwrap());
            // Invalid property edits must leave both the document and saved file unchanged.
            let field = with(|a| a.fields[2]).unwrap();
            SetWindowTextW(field, w!("-10")).unwrap();
            assert!(!save_current(hwnd, false));
            assert_eq!(format::load(&path).unwrap(), saved);
            with(|a| a.properties());
            with(|a| {
                a.paint().unwrap();
                let o = Export::document(&a.tabs[0].doc);
                let png = a.renderer.png(&a.tabs[0].doc, &o, 1.).unwrap();
                assert!(png.len() > 500);
                let svg = a.renderer.svg(&a.tabs[0].doc, &o).unwrap();
                assert!(svg.contains("<path"));
                assert!(!svg.contains("<text"));
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let done = with(|a| {
                    a.poll_work();
                    a.paint().unwrap();
                    !a.pixels.is_empty() && a.frame.is_none() && !a.dirty
                })
                .unwrap();
                if done {
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            // Search and place a built-in cloud service icon.
            with(|a| {
                let camera = a.image_camera.unwrap();
                let displayed = interaction::Camera {
                    origin: a.tabs[0].origin,
                    scale: a.tabs[0].zoom * a.dpi,
                };
                let revision = a.pixel_revision;
                let cursor = Point::new(600., 300.);
                let anchor = a.document_point(cursor);
                for _ in 0..4 {
                    a.wheel(cursor, 120., true, false);
                    a.paint().unwrap();
                    assert!(a.frame.is_none());
                    assert_eq!(a.pixel_revision, revision);
                    assert_eq!(a.image_camera, Some(camera));
                    assert!(a.document_point(cursor).distance(anchor) < 1e-8);
                }
                let start = std::time::Instant::now();
                let mut previous = a.tabs[0].zoom;
                let target = displayed.scale / a.dpi * 1.15f64.powi(4);
                for ms in [16, 32, 48, 64] {
                    assert!(a.tick_zoom(start + std::time::Duration::from_millis(ms)));
                    assert!(a.tabs[0].zoom > previous && a.tabs[0].zoom < target);
                    assert!(a.document_point(cursor).distance(anchor) < 1e-8);
                    previous = a.tabs[0].zoom;
                }
                let moved = Point::new(650., 340.);
                let now = start + std::time::Duration::from_millis(64);
                let before = a.tabs[0].origin;
                a.follow_zoom_pointer(moved, now);
                assert!(a.tabs[0].origin.distance(before) < 1e-8);
                let moved_anchor = a.document_point(moved);
                a.tick_zoom(start + std::time::Duration::from_millis(96));
                assert!(a.document_point(moved).distance(moved_anchor) < 1e-8);
                a.tick_zoom(start + std::time::Duration::from_millis(250));
                assert!(a.zoom_motion.is_none());
                assert!((a.tabs[0].zoom - target).abs() < 1e-8);
                assert_eq!(a.pixel_revision, revision);
                let dpi = a.dpi;
                a.dpi = 1.25;
                let anchor = a.document_point(moved);
                a.wheel(moved, -120., true, false);
                a.tick_zoom(std::time::Instant::now() + std::time::Duration::from_millis(250));
                assert!((a.tabs[0].zoom - target / 1.15).abs() < 1e-8);
                assert!(a.document_point(moved).distance(anchor) < 1e-8);
                a.dpi = dpi;
                a.tabs[0].origin = displayed.origin;
                a.tabs[0].zoom = displayed.scale / a.dpi;
                a.view_changed();
                a.view_deadline = Some(std::time::Instant::now());
            });
            wait_canvas();
            let filter = with(|a| a.filter).unwrap();
            SetWindowTextW(filter, w!("Azure")).unwrap();
            assert!(
                with(|a| a.palette_tools.is_empty()).unwrap(),
                "search must stay inside the selected category"
            );
            let combo = with(|a| {
                let i = a
                    .palette_state
                    .categories
                    .iter()
                    .position(|c| c.starts_with("Azure / "))
                    .unwrap();
                SendMessageW(
                    a.palette_state.category,
                    CB_SETCURSEL,
                    Some(WPARAM(i)),
                    None,
                );
                a.palette_state.category
            })
            .unwrap();
            SendMessageW(
                hwnd,
                WM_COMMAND,
                Some(WPARAM(
                    palette::CATEGORY as usize | ((CBN_SELCHANGE as usize) << 16),
                )),
                Some(LPARAM(combo.0 as isize)),
            );
            with(|a| {
                let category = text(a.palette_state.category);
                let expected = yy_draw::catalog::all()
                    .iter()
                    .filter(|e| e.category_path() == category)
                    .count();
                assert_eq!(a.palette_tools.len(), expected);
                assert!(expected >= 3);
                assert!(expected < 714);
            });
            // Original business/IT icons appear before cloud categories and accept English search.
            let (azure_index, business_index) = with(|a| {
                let categories = &a.palette_state.categories;
                assert!(categories[2].starts_with("業務・IT / 01 "));
                let azure =
                    SendMessageW(a.palette_state.category, CB_GETCURSEL, None, None).0 as usize;
                let business = categories
                    .iter()
                    .position(|c| c.starts_with("業務・IT / 03 "))
                    .unwrap();
                (azure, business)
            })
            .unwrap();
            SetWindowTextW(filter, w!("")).unwrap();
            SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(business_index)), None);
            SendMessageW(
                hwnd,
                WM_COMMAND,
                Some(WPARAM(
                    palette::CATEGORY as usize | ((CBN_SELCHANGE as usize) << 16),
                )),
                None,
            );
            assert_eq!(with(|a| a.palette_tools.len()).unwrap(), 10);
            SetWindowTextW(filter, w!("eTl")).unwrap();
            with(|a| {
                assert_eq!(a.palette_state.items.len(), 1);
                assert_eq!(a.palette_state.items[0].label, "ETL・データ変換");
                a.tool = a.palette_tools[0];
            });
            mouse(hwnd, WM_LBUTTONDOWN, screen(Point::new(280., 280.)));
            mouse(hwnd, WM_LBUTTONUP, screen(Point::new(280., 280.)));
            with(|a| {
                let d = &a.tabs[0].doc;
                let node = d.nodes.last().unwrap();
                assert_eq!(node.label, "ETL・データ変換");
                assert_eq!(
                    d.assets
                        .iter()
                        .find(|asset| Some(asset.id) == node.asset)
                        .unwrap()
                        .source,
                    "yydraw:business-icons/v1"
                );
            });
            command(hwnd, UNDO);
            SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(azure_index)), None);
            SetWindowTextW(filter, w!("Azure")).unwrap();
            SendMessageW(
                hwnd,
                WM_COMMAND,
                Some(WPARAM(
                    palette::CATEGORY as usize | ((CBN_SELCHANGE as usize) << 16),
                )),
                None,
            );
            with(|a| {
                let bmp = a.test_palette_image();
                if let Some(path) = std::env::var_os("YYDRAW_QA_PALETTE") {
                    std::fs::write(path, bmp).unwrap();
                }
            });
            let palette = with(|a| a.palette).unwrap();
            let count = with(|a| a.tabs[0].doc.nodes.len()).unwrap();
            mouse(palette, WM_LBUTTONDOWN, Point::new(16., 20.));
            assert!(with(|a| a.palette_state.drag.is_some()).unwrap());
            mouse(hwnd, WM_MOUSEMOVE, screen(Point::new(320., 40.)));
            assert!(with(|a| a.palette_state.drag.as_ref().unwrap().active).unwrap());
            with(|a| a.paint().unwrap());
            mouse(hwnd, WM_LBUTTONUP, screen(Point::new(320., 40.)));
            assert_eq!(with(|a| a.tabs[0].doc.nodes.len()).unwrap(), count + 1);
            command(hwnd, UNDO);
            with(|a| {
                SendMessageW(a.palette, LB_SETCURSEL, Some(WPARAM(0)), None);
                a.choose_palette();
            });
            mouse(hwnd, WM_LBUTTONDOWN, screen(Point::new(320., 40.)));
            mouse(hwnd, WM_LBUTTONUP, screen(Point::new(320., 40.)));
            assert_eq!(
                with(|a| a.tabs[0].doc.nodes.last().unwrap().shape).unwrap(),
                Shape::Icon
            );
            // The moving layer is prepared once, then the same GPU texture follows
            // pointer motion without a new full-scene worker for every mouse event.
            mouse(hwnd, WM_LBUTTONDOWN, screen(Point::new(360., 60.)));
            with(|a| a.paint().unwrap());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if with(|a| {
                    a.poll_work();
                    a.paint().unwrap();
                    a.preview_visible && a.sprite.is_some() && a.frame.is_none()
                })
                .unwrap()
                {
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let revision = with(|a| a.revision).unwrap();
            for i in 0..12 {
                mouse(
                    hwnd,
                    WM_MOUSEMOVE,
                    screen(Point::new(360. + i as f64 * 2., 60. + i as f64)),
                );
                with(|a| a.paint().unwrap());
            }
            assert_eq!(with(|a| a.revision).unwrap(), revision);
            assert!(with(|a| a.frame.is_none()).unwrap());
            command(hwnd, SELECT);
            // Explicit points and editable waypoints are actual native canvas tools.
            // Resizing and reconnecting edit only the target, keeping embedded SVG allocations.
            let (original, corner, svg_allocation) = with(|a| {
                let resize_id = a.tabs[0].doc.nodes.last().unwrap().id;
                a.tabs[0].selected = vec![resize_id];
                a.properties();
                let d = &a.tabs[0].doc;
                let r = d.node(resize_id).unwrap().rect;
                (
                    d.clone(),
                    Point::new(r.x + r.w, r.y + r.h),
                    d.assets[0].svg.as_ptr() as usize,
                )
            })
            .unwrap();
            let click = screen(corner);
            with(|a| {
                let (l, t, w, h) = a.viewport();
                assert!(
                    click.x >= l as f64
                        && click.x < (l as u32 + w) as f64
                        && click.y >= t as f64
                        && click.y < (t as u32 + h) as f64,
                    "resize handle {click:?} outside viewport {:?}, dpi {}, corner {corner:?}",
                    a.viewport(),
                    a.dpi,
                );
            });
            mouse(hwnd, WM_LBUTTONDOWN, click);
            assert!(with(|a| matches!(a.drag, Some(Drag::Resize { .. }))).unwrap());
            for offset in [Point::new(32., 16.), Point::new(16., 32.)] {
                mouse(
                    hwnd,
                    WM_MOUSEMOVE,
                    screen(Point::new(corner.x + offset.x, corner.y + offset.y)),
                );
                assert_eq!(
                    with(|a| a.tabs[0].doc.assets[0].svg.as_ptr() as usize).unwrap(),
                    svg_allocation
                );
            }
            command(hwnd, SELECT);
            assert_eq!(with(|a| a.tabs[0].doc.clone()).unwrap(), original);
            let (end, svg_allocation) = with(|a| {
                let id = a.tabs[0].doc.connectors[0].id;
                a.tabs[0].selected = vec![id];
                a.properties();
                let d = &a.tabs[0].doc;
                (
                    d.endpoint(d.connectors[0].end),
                    d.assets[0].svg.as_ptr() as usize,
                )
            })
            .unwrap();
            mouse(hwnd, WM_LBUTTONDOWN, screen(end));
            assert!(with(|a| matches!(a.drag, Some(Drag::Endpoint { .. }))).unwrap());
            mouse(hwnd, WM_MOUSEMOVE, screen(Point::new(300., 320.)));
            assert_eq!(
                with(|a| a.tabs[0].doc.assets[0].svg.as_ptr() as usize).unwrap(),
                svg_allocation
            );
            command(hwnd, SELECT);
            assert_eq!(with(|a| a.tabs[0].doc.clone()).unwrap(), original);
            with(|a| {
                a.tabs[0].doc.connect(
                    Endpoint::Free(Point::new(80., 480.)),
                    Endpoint::Free(Point::new(480., 480.)),
                    false,
                );
                a.tabs[0].doc.connect(
                    Endpoint::Free(Point::new(280., 400.)),
                    Endpoint::Free(Point::new(280., 600.)),
                    false,
                );
                a.tool = Tool::Shape(Shape::Junction);
            });
            mouse(hwnd, WM_LBUTTONDOWN, screen(Point::new(280., 480.)));
            mouse(hwnd, WM_LBUTTONUP, screen(Point::new(280., 480.)));
            assert!(
                with(|a| a.tabs[0]
                    .doc
                    .nodes
                    .iter()
                    .any(|n| n.shape == Shape::Junction))
                .unwrap()
            );
            with(|a| {
                let id = a.tabs[0].doc.connect(
                    Endpoint::Free(Point::new(80., 360.)),
                    Endpoint::Free(Point::new(480., 360.)),
                    false,
                );
                a.tabs[0].selected = vec![id];
                a.tool = Tool::Select;
            });
            mouse(hwnd, WM_LBUTTONDBLCLK, screen(Point::new(200., 360.)));
            assert_eq!(
                with(|a| a.tabs[0].doc.connectors.last().unwrap().waypoints.len()).unwrap(),
                1
            );
            mouse(hwnd, WM_LBUTTONDOWN, screen(Point::new(200., 360.)));
            let svg_allocation = with(|a| a.tabs[0].doc.assets[0].svg.as_ptr() as usize).unwrap();
            mouse(hwnd, WM_MOUSEMOVE, screen(Point::new(216., 376.)));
            assert_eq!(
                with(|a| a.tabs[0].doc.assets[0].svg.as_ptr() as usize).unwrap(),
                svg_allocation
            );
            mouse(hwnd, WM_LBUTTONUP, screen(Point::new(216., 376.)));
            assert_eq!(
                with(|a| a.tabs[0].doc.connectors.last().unwrap().waypoints[0]).unwrap(),
                Point::new(216., 376.)
            );
            with(|a| {
                a.tabs[0].doc.add_page("Second".into()).unwrap();
                a.tabs[0]
                    .doc
                    .add_node(Shape::Triangle, Point::new(80., 80.), None);
                a.refresh_pages();
            });
            let combo = with(|a| a.pages_combo).unwrap();
            SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(0)), None);
            SendMessageW(
                hwnd,
                WM_COMMAND,
                Some(WPARAM(PAGES as usize | ((CBN_SELCHANGE as usize) << 16))),
                Some(LPARAM(combo.0 as isize)),
            );
            assert_eq!(with(|a| a.tabs[0].doc.active_page).unwrap(), 0);
            command(hwnd, 52);
            assert_eq!(with(|a| a.tabs[0].doc.pages.len()).unwrap(), 3);
            command(hwnd, 62);
            command(hwnd, 63);
            with(|a| {
                a.tabs[0].selected = vec![a.tabs[0].doc.nodes[0].id];
                let (d, o) = a.export_document().unwrap();
                assert_eq!(d.nodes.len(), 1);
                assert!(o.text);
            });
            // Visible extra handles create exact, persistent connections in selection mode.
            let ports = with(|a| {
                a.tool = Tool::Select;
                a.tabs[0].selected.clear();
                let d = &a.tabs[0].doc;
                let first = d.nodes[0].id;
                let second = d.nodes[1].id;
                let start = Endpoint::Attached {
                    node: first,
                    side: 8,
                };
                let end = Endpoint::Attached {
                    node: second,
                    side: 16,
                };
                (
                    start,
                    end,
                    d.endpoint(start),
                    d.endpoint(end),
                    d.connectors.len(),
                )
            })
            .unwrap();
            mouse(hwnd, WM_MOUSEMOVE, screen(ports.2));
            with(|a| {
                assert_eq!(a.hover_port, Some(ports.0));
                assert_eq!(a.connection_markers().len(), 20);
                assert!(a.connection_markers().iter().any(|p| p.active));
            });
            mouse(hwnd, WM_LBUTTONDOWN, screen(ports.2));
            assert!(with(|a| matches!(a.drag, Some(Drag::Connect { .. }))).unwrap());
            mouse(hwnd, WM_MOUSEMOVE, screen(ports.3));
            with(|a| {
                assert_eq!(a.hover_port, Some(ports.1));
                assert_eq!(a.connection_markers().len(), 40);
            });
            mouse(hwnd, WM_LBUTTONUP, screen(ports.3));
            with(|a| {
                let d = &a.tabs[0].doc;
                assert_eq!(d.connectors.len(), ports.4 + 1);
                let c = d.connectors.last().unwrap();
                assert_eq!(c.start, ports.0);
                assert_eq!(c.end, ports.1);
                assert_eq!(
                    yy_draw::format::decode(&yy_draw::format::encode(d).unwrap()).unwrap(),
                    *d
                );
            });
            // Shift and Tab build a manual cardinal route; settings affect preview and exports.
            with(|a| {
                a.tabs[0].selected.clear();
                a.tool = Tool::Connect(false);
                a.properties();
            });
            let (corners, radius) = with(|a| (a.fields[9], a.fields[10])).unwrap();
            SendMessageW(corners, CB_SETCURSEL, Some(WPARAM(1)), None);
            SendMessageW(
                hwnd,
                WM_COMMAND,
                Some(WPARAM(
                    (PROP + 9) as usize | ((CBN_SELCHANGE as usize) << 16),
                )),
                None,
            );
            SetWindowTextW(radius, w!("16")).unwrap();
            command(hwnd, APPLY);
            assert!(with(|a| a.rounded_lines && a.corner_radius == 16.).unwrap());
            let shift = ShiftGuard::press();
            mouse(hwnd, WM_LBUTTONDOWN, screen(Point::new(40., 440.)));
            mouse(hwnd, WM_MOUSEMOVE, screen(Point::new(160., 456.)));
            assert!(shortcut(hwnd, VK_TAB.0));
            mouse(hwnd, WM_MOUSEMOVE, screen(Point::new(120., 504.)));
            assert!(shortcut(hwnd, VK_TAB.0));
            assert!(shortcut(hwnd, VK_BACK.0));
            assert!(shortcut(hwnd, VK_TAB.0));
            mouse(hwnd, WM_MOUSEMOVE, screen(Point::new(280., 480.)));
            with(|a| {
                let revision = a.revision;
                let preview = a.arrow_preview().unwrap();
                assert_eq!(preview.corner_radius, 16. * a.dpi * a.tabs[0].zoom);
                assert_eq!(preview.points.len(), 4);
                a.paint().unwrap();
                assert_eq!(a.revision, revision);
            });
            mouse(hwnd, WM_LBUTTONUP, screen(Point::new(280., 480.)));
            drop(shift);
            with(|a| {
                let d = &a.tabs[0].doc;
                let c = d.connectors.last().unwrap();
                assert_eq!(
                    c.waypoints,
                    vec![Point::new(160., 440.), Point::new(160., 504.)]
                );
                assert_eq!(d.endpoint(c.end), Point::new(280., 504.));
                assert!(!c.orthogonal);
                assert_eq!(c.corner_radius, 16.);
                assert!(geometry::connector_path(d, d.connectors.len() - 1).contains(" Q "));
                assert_eq!(format::decode(&format::encode(d).unwrap()).unwrap(), *d);
            });
            SendMessageW(corners, CB_SETCURSEL, Some(WPARAM(0)), None);
            command(hwnd, APPLY);
            assert_eq!(
                with(|a| a.tabs[0].doc.connectors.last().unwrap().corner_radius).unwrap(),
                0.
            );
            SendMessageW(corners, CB_SETCURSEL, Some(WPARAM(1)), None);
            SetWindowTextW(radius, w!("-1")).unwrap();
            assert!(!with(|a| a.apply_properties()).unwrap());
            SetWindowTextW(radius, w!("8")).unwrap();
            command(hwnd, APPLY);
            assert_eq!(
                with(|a| a.tabs[0].doc.connectors.last().unwrap().corner_radius).unwrap(),
                8.
            );
            command(hwnd, UNDO);
            assert_eq!(
                with(|a| a.tabs[0].doc.connectors.last().unwrap().corner_radius).unwrap(),
                0.
            );
            command(hwnd, REDO);
            assert_eq!(
                with(|a| a.tabs[0].doc.connectors.last().unwrap().corner_radius).unwrap(),
                8.
            );
            let count = with(|a| a.tabs[0].doc.connectors.len()).unwrap();
            with(|a| a.tool = Tool::Connect(true));
            let shift = ShiftGuard::press();
            mouse(hwnd, WM_LBUTTONDOWN, screen(Point::new(40., 440.)));
            mouse(hwnd, WM_MOUSEMOVE, screen(Point::new(80., 440.)));
            assert!(shortcut(hwnd, VK_TAB.0));
            assert!(shortcut(hwnd, VK_ESCAPE.0));
            drop(shift);
            assert_eq!(with(|a| a.tabs[0].doc.connectors.len()).unwrap(), count);
            // Group-aware selection, menu state, both alignment references and Undo.
            let original_tab = with(|a| {
                let old = a.tab;
                let mut d = Document::default();
                for p in [
                    Point::new(40., 40.),
                    Point::new(240., 120.),
                    Point::new(440., 220.),
                ] {
                    d.add_node(Shape::Rectangle, p, None);
                    let n = d.nodes.last_mut().unwrap();
                    n.rect.w = 100.;
                    n.rect.h = 60.;
                }
                d.connect(
                    Endpoint::Attached {
                        node: d.nodes[0].id,
                        side: 1,
                    },
                    Endpoint::Attached {
                        node: d.nodes[1].id,
                        side: 3,
                    },
                    true,
                );
                a.tabs.push(Tab::new(d, None));
                a.tab = a.tabs.len() - 1;
                a.tabs[a.tab].selected =
                    vec![a.tabs[a.tab].doc.nodes[0].id, a.tabs[a.tab].doc.nodes[1].id];
                a.properties();
                a.refresh_tabs();
                a.invalidate();
                old
            })
            .unwrap();
            let mut keys = [0; 256];
            GetKeyboardState(&mut keys).unwrap();
            let restore = ShiftGuard(keys);
            keys[VK_CONTROL.0 as usize] |= 0x80;
            SetKeyboardState(&keys).unwrap();
            assert!(shortcut(hwnd, 0x47));
            drop(restore);
            with(|a| {
                let t = &a.tabs[a.tab];
                assert_eq!(t.selected.len(), 3);
                assert_eq!(t.doc.nodes[0].group, t.doc.connectors[0].group);
            });
            with(|a| a.tabs[a.tab].selected.clear());
            mouse(hwnd, WM_LBUTTONDOWN, screen(Point::new(80., 70.)));
            mouse(hwnd, WM_LBUTTONUP, screen(Point::new(80., 70.)));
            assert_eq!(with(|a| a.tabs[a.tab].selected.len()).unwrap(), 3);
            let shift = ShiftGuard::press();
            mouse(hwnd, WM_LBUTTONDOWN, screen(Point::new(480., 250.)));
            mouse(hwnd, WM_LBUTTONUP, screen(Point::new(480., 250.)));
            drop(shift);
            assert_eq!(with(|a| a.tabs[a.tab].selected.len()).unwrap(), 4);
            command(hwnd, ALIGN_BOTTOM);
            with(|a| {
                let d = &a.tabs[a.tab].doc;
                assert_eq!(d.nodes[1].rect.y - d.nodes[0].rect.y, 80.);
                assert_eq!(
                    d.nodes[1].rect.y + d.nodes[1].rect.h,
                    d.nodes[2].rect.y + d.nodes[2].rect.h
                );
                let bytes = format::encode(d).unwrap();
                assert_eq!(format::decode(&bytes).unwrap(), *d);
                let id = d.nodes[0].id;
                a.tabs[a.tab].selected = d.expanded_selection(&[id]);
            });
            command(hwnd, ALIGN_PAGE);
            command(hwnd, CENTER_X);
            with(|a| {
                let t = &a.tabs[a.tab];
                let units = t.doc.layout_units(&t.selected);
                assert_eq!(units[0].bounds.center().x, t.doc.page_bounds().w / 2.);
                let menu = CreatePopupMenu().unwrap();
                for &(id, label) in arrange::ITEMS {
                    AppendMenuW(menu, MF_STRING, id as usize, &HSTRING::from(label)).unwrap();
                }
                a.update_menu(menu);
                assert_ne!(
                    GetMenuState(menu, DISTRIBUTE as u32, MF_BYCOMMAND) & MF_GRAYED.0,
                    0
                );
                assert_eq!(
                    GetMenuState(menu, CENTER_X as u32, MF_BYCOMMAND) & MF_GRAYED.0,
                    0
                );
                DestroyMenu(menu).unwrap();
            });
            command(hwnd, UNGROUP);
            with(|a| {
                let t = &mut a.tabs[a.tab];
                assert!(t.doc.nodes.iter().all(|n| n.group.is_none()));
                assert!(t.doc.connectors[0].group.is_none());
                t.selected = t.doc.nodes.iter().map(|n| n.id).collect();
            });
            command(hwnd, DISTRIBUTE);
            let arranged = with(|a| a.tabs[a.tab].doc.clone()).unwrap();
            let r: Vec<_> = arranged.nodes.iter().map(|n| n.rect).collect();
            let mut xs = r.clone();
            xs.sort_by(|a, b| a.x.total_cmp(&b.x));
            assert!((xs[1].x - xs[0].x - xs[0].w - (xs[2].x - xs[1].x - xs[1].w)).abs() < 1e-8);
            command(hwnd, UNDO);
            command(hwnd, REDO);
            assert_eq!(with(|a| a.tabs[a.tab].doc.clone()).unwrap(), arranged);
            with(|a| {
                a.tabs.pop();
                a.tab = original_tab;
                a.layout_reference = layout::Reference::Selection;
                a.properties();
                a.refresh_tabs();
                a.invalidate();
            });
            // A save snapshot must not clear edits made while its worker is running.
            assert!(save_current(hwnd, false));
            with(|a| {
                a.close_after = Some(a.tabs[0].uid);
                a.tabs[0].doc.nodes[0]
                    .label
                    .push_str(" changed during save")
            });
            wait_work();
            assert!(with(|a| a.tabs[0].dirty()).unwrap());
            assert_eq!(with(|a| a.tabs.len()).unwrap(), 1);
            assert!(with(|a| a.close_after.is_none()).unwrap());
            // A cancelled read must not create a tab even when its result is already queued.
            with(|a| {
                let task = yy_draw::jobs::Task::spawn(|_| {
                    Ok(work::Outcome::Opened {
                        path: PathBuf::from("cancelled.yyd"),
                        doc: Document::default(),
                    })
                });
                task.control.cancel();
                a.work = Some(task);
            });
            wait_work();
            assert_eq!(with(|a| a.tabs.len()).unwrap(), 1);
            // Close is authorized only after the unchanged snapshot has reached disk.
            assert!(save_current(hwnd, false));
            with(|a| a.close_after = Some(a.tabs[0].uid));
            let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let message = with(|a| a.poll_work()).flatten();
                if let Some(message) = message {
                    assert_eq!(message, work::CLOSE_SAVED);
                    break;
                }
                assert!(std::time::Instant::now() < end);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert!(!with(|a| a.tabs[0].dirty()).unwrap());
            drop(window);
        }
    }
}

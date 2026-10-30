//! Native Windows host. User input/results stay in memory until an explicit save/copy.
#![allow(unsafe_code)]
use std::{
    cell::{Cell, RefCell},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};
use windows::Win32::System::SystemServices::{MK_CONTROL, SS_NOPREFIX, SS_OWNERDRAW};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{Com::*, DataExchange::*, LibraryLoader::GetModuleHandleW, Memory::*},
        UI::{
            Controls::*,
            HiDpi::*,
            Input::KeyboardAndMouse::*,
            Shell::{Common::COMDLG_FILTERSPEC, *},
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, Interface, PCWSTR, w},
};
use yytoolbox::{Field, MAX_INPUT, Output, Params, TOOLS, Tool};

const CLASS: PCWSTR = w!("YYToolboxFrame");
const HELP_CLASS: PCWSTR = w!("YYToolboxHelp");
const DONE: u32 = WM_APP + 1;
const SELECT: u16 = 101;
const RUN: u16 = 102;
const CANCEL: u16 = 103;
const EXAMPLE: u16 = 104;
const OPEN: u16 = 105;
const SAVE_INPUT: u16 = 106;
const COPY: u16 = 107;
const SAVE: u16 = 108;
const CLEAR: u16 = 109;
const HELP: u16 = 110;
const FIELD_BASE: u16 = 200;
const INPUT: u16 = 300;
const OUTPUT: u16 = 301;
const QR_VIEW: u16 = 302;
const ZOOM_OUT: u16 = 111;
const ZOOM_RESET: u16 = 112;
const ZOOM_IN: u16 = 113;
#[path = "ui_appearance.rs"]
mod appearance;

struct InputField {
    definition: Field,
    label: HWND,
    control: HWND,
}
struct Running {
    cancel: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}
enum Job {
    Text(Tool, String, Params),
    Images(Vec<PathBuf>, PathBuf, Params),
    Recording(crate::recording::Plan),
    VideoEdit(PathBuf, PathBuf, yytoolbox::video::Edit),
}
impl Job {
    fn run(self, cancel: &AtomicBool) -> yytoolbox::Result<Output> {
        match self {
            Self::Text(tool, input, params) => {
                yytoolbox::execute(tool, &input, &params, &|| cancel.load(Ordering::Relaxed))
            }
            Self::Images(files, folder, params) => {
                yytoolbox::images::resize_batch(&files, &folder, &params, &|| {
                    cancel.load(Ordering::Relaxed)
                })
            }
            Self::VideoEdit(source, destination, edit) => {
                crate::recording::video_edit::edit(&source, &destination, &edit, cancel)
            }
            Self::Recording(plan) => crate::recording::record(plan, cancel),
        }
    }
}
struct State {
    hwnd: HWND,
    heading: HWND,
    zoom_buttons: [HWND; 3],
    zoom: u32,
    #[cfg(test)]
    test_dpi: Option<u32>,
    scroll: Cell<i32>,
    content_height: Cell<i32>,
    background: HBRUSH,
    paper: HBRUSH,
    title_font: HFONT,
    select_label: HWND,
    select: HWND,
    buttons: Vec<(u16, HWND, i32)>,
    description: HWND,
    fields: Vec<InputField>,
    input_label: HWND,
    input: HWND,
    input_source: String,
    output_label: HWND,
    output: HWND,
    qr_view: HWND,
    status: HWND,
    tool: Tool,
    result: Option<Output>,
    fonts: Vec<HFONT>,
    font: HFONT,
    mono: HFONT,
    running: Option<Running>,
    tx: mpsc::Sender<yytoolbox::Result<Output>>,
    rx: mpsc::Receiver<yytoolbox::Result<Output>>,
    closing: bool,
    recording_ui: Option<crate::recording::UiGuard>,
}
thread_local! {static STATE:RefCell<Option<State>>=const {RefCell::new(None)};}
fn state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}
// The main thread receives key messages for the recording bar too.
fn escape() {
    if !state(|s| s.recording_ui.is_some()).unwrap_or(false) {
        command(CANCEL, 0);
    }
}
fn set_text(hwnd: HWND, text: &str) {
    let display = text.replace("\r\n", "\n").replace('\n', "\r\n");
    unsafe {
        let _ = SetWindowTextW(hwnd, &HSTRING::from(display));
    }
}
fn text(hwnd: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(hwnd).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let count = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..count.max(0) as usize])
    }
}
fn show(hwnd: HWND, visible: bool) {
    unsafe {
        let _ = ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
    }
}
fn uses_input(tool: Tool) -> bool {
    !matches!(
        tool,
        Tool::Password
            | Tool::Compound
            | Tool::Loan
            | Tool::AgeGrade
            | Tool::Recording
            | Tool::ResizeImages
            | Tool::VideoEdit
    )
}
fn navigation_label(tool: Tool) -> &'static str {
    match tool {
        Tool::Convert => "データ形式の相互変換",
        Tool::Password => "パスワード生成",
        Tool::DummyCsv => "ダミーCSV生成",
        Tool::Url => "URL変換",
        Tool::ResizeImages => "画像リサイズ",
        Tool::VideoEdit => "動画カット・クリップ",
        _ => tool.label(),
    }
}
fn ui_font(dpi: u32, mono: bool) -> HFONT {
    unsafe {
        let mut font = LOGFONTW {
            lfHeight: -(15 * dpi as i32 / 96),
            ..Default::default()
        };
        let name: Vec<_> = if mono { "Cascadia Mono" } else { "Segoe UI" }
            .encode_utf16()
            .collect();
        font.lfFaceName[..name.len()].copy_from_slice(&name);
        CreateFontIndirectW(&font)
    }
}

fn child(
    parent: HWND,
    class: PCWSTR,
    label: &str,
    style: WINDOW_STYLE,
    id: u16,
    font: HFONT,
) -> windows::core::Result<HWND> {
    unsafe {
        let hwnd = CreateWindowExW(
            if class == w!("EDIT") {
                WS_EX_CLIENTEDGE
            } else {
                WINDOW_EX_STYLE(0)
            },
            class,
            &HSTRING::from(label),
            WS_CHILD
                | WS_VISIBLE
                | style
                | if class == w!("STATIC") {
                    WINDOW_STYLE(SS_NOPREFIX.0)
                } else {
                    WINDOW_STYLE(0)
                },
            0,
            0,
            1,
            1,
            Some(parent),
            Some(HMENU(id as usize as *mut _)),
            Some(GetModuleHandleW(None)?.into()),
            None,
        )?;
        SendMessageW(
            hwnd,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
        Ok(hwnd)
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(worker) = self.running.take() {
            worker.cancel.store(true, Ordering::Relaxed);
            let _ = worker.thread.join();
        }
        for font in &self.fonts {
            unsafe {
                let _ = DeleteObject((*font).into());
            }
        }
        unsafe {
            let _ = DeleteObject(self.background.into());
            let _ = DeleteObject(self.paper.into());
        }
    }
}

impl State {
    fn params(&self) -> Params {
        self.fields
            .iter()
            .map(|f| (f.definition.key.into(), text(f.control)))
            .collect()
    }
    fn apply_tool(&mut self, tool: Tool) -> windows::core::Result<()> {
        for field in self.fields.drain(..) {
            unsafe {
                let _ = DestroyWindow(field.label);
                let _ = DestroyWindow(field.control);
            }
        }
        self.tool = tool;
        self.scroll.set(0);
        self.result = None;
        set_text(self.output, "");
        set_text(self.description, tool.description());
        set_text(self.heading, tool.label());
        if let Some((_, button, _)) = self.buttons.iter().find(|b| b.0 == RUN) {
            set_text(
                *button,
                match tool {
                    Tool::Recording => "録画を開始",
                    Tool::ResizeImages => "画像を選ぶ",
                    Tool::VideoEdit => "動画を選んで保存",
                    _ => "実行",
                },
            );
        }
        for (i, definition) in tool.fields().into_iter().enumerate() {
            let label = child(
                self.hwnd,
                w!("STATIC"),
                definition.label,
                WINDOW_STYLE(0),
                0,
                self.font,
            )?;
            let combo = !definition.choices.is_empty();
            let control = child(
                self.hwnd,
                if combo { w!("COMBOBOX") } else { w!("EDIT") },
                &definition.default,
                WS_TABSTOP
                    | if combo {
                        WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_HASSTRINGS) as u32) | WS_VSCROLL
                    } else {
                        WINDOW_STYLE(ES_AUTOHSCROLL as u32)
                    },
                FIELD_BASE + i as u16,
                self.font,
            )?;
            unsafe {
                if combo {
                    for choice in definition.choices {
                        SendMessageW(
                            control,
                            CB_ADDSTRING,
                            None,
                            Some(LPARAM(HSTRING::from(*choice).as_ptr() as isize)),
                        );
                    }
                    SendMessageW(control, CB_SETCURSEL, Some(WPARAM(0)), None);
                } else {
                    SendMessageW(control, EM_SETLIMITTEXT, Some(WPARAM(65536)), None);
                }
            }
            self.fields.push(InputField {
                definition,
                label,
                control,
            });
        }
        let params = self.params();
        self.input_source = tool.example(&params);
        set_text(self.input, &self.input_source);
        set_text(
            self.status,
            "Ctrl+Enterで実行 · Ctrl＋/−で拡大・縮小 · Ctrl+0で100% · F1でヘルプ",
        );
        self.enabled();
        self.layout();
        Ok(())
    }
    fn enabled(&self) {
        let busy = self.running.is_some();
        let audio_only =
            self.tool == Tool::Recording && crate::recording::audio_only(&self.params());
        if self.tool == Tool::Recording
            && let Some((_, button, _)) = self.buttons.iter().find(|b| b.0 == RUN)
        {
            set_text(
                *button,
                if audio_only {
                    "録音を開始"
                } else {
                    "録画を開始"
                },
            );
        }
        unsafe {
            let _ = EnableWindow(self.select, !busy);
            let _ = EnableWindow(self.input, !busy && uses_input(self.tool));
            for field in &self.fields {
                let video_only =
                    audio_only && matches!(field.definition.key, "area" | "fps" | "max_width");
                let _ = EnableWindow(field.control, !busy && !video_only);
            }
            for (id, hwnd, _) in &self.buttons {
                let on = match *id {
                    CANCEL => busy,
                    HELP => true,
                    SAVE => {
                        !busy
                            && self.result.is_some()
                            && !matches!(self.tool, Tool::Recording | Tool::VideoEdit)
                    }
                    COPY => !busy && self.result.is_some(),
                    OPEN | SAVE_INPUT | EXAMPLE => !busy && uses_input(self.tool),
                    _ => !busy,
                };
                let _ = EnableWindow(*hwnd, on);
            }
        }
        self.layout();
        unsafe {
            let _ = InvalidateRect(Some(self.qr_view), None, true);
        }
    }
    fn layout(&self) {
        self.arrange();
    }
    fn start(&mut self) {
        self.start_job(Job::Text(
            self.tool,
            self.input_source.clone(),
            self.params(),
        ));
    }
    fn start_job(&mut self, job: Job) {
        if self.running.is_some() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let tx = self.tx.clone();
        let hwnd = self.hwnd.0 as isize;
        self.result = None;
        set_text(self.output, "");
        if matches!(&job, Job::Recording(_)) {
            set_text(
                self.status,
                "収録中…小型バーの「停止して保存」で終了できます。",
            );
        } else {
            set_text(self.status, "処理中…「中止」またはEscで中止できます。");
        }
        let spawned = std::thread::Builder::new()
            .name("yytoolbox-work".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(|| job.run(&flag)).unwrap_or_else(|_| {
                    Err("処理を完了できませんでした。入力の大きさ・条件を確認してください".into())
                });
                let _ = tx.send(result);
                unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd as *mut _)), DONE, WPARAM(0), LPARAM(0));
                }
            });
        match spawned {
            Ok(thread) => self.running = Some(Running { cancel, thread }),
            Err(e) => {
                self.recording_ui.take();
                set_text(self.status, &format!("処理を開始できません: {e}"));
            }
        }
        self.enabled();
    }
}

fn create() -> windows::core::Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let icon = LoadIconW(
            Some(instance.into()),
            PCWSTR(std::ptr::without_provenance(1)),
        )
        .or_else(|_| LoadIconW(None, IDI_APPLICATION))?;
        let class = WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: CLASS,
            lpfnWndProc: Some(proc),
            hIcon: icon,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: GetSysColorBrush(COLOR_WINDOW),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return Err(windows::core::Error::from_thread());
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("yytoolbox — ローカル開発ツール"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN | WS_VSCROLL,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100,
            800,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let dpi = GetDpiForWindow(hwnd).max(96);
        let font = ui_font(dpi, false);
        let mono = ui_font(dpi, true);
        let select_label = child(hwnd, w!("STATIC"), "yytoolbox", WINDOW_STYLE(0), 0, font)?;
        let heading = child(hwnd, w!("STATIC"), "", WINDOW_STYLE(0), 0, font)?;
        let zoom_buttons = [
            child(
                hwnd,
                w!("BUTTON"),
                "−",
                WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32 | BS_NOTIFY as u32),
                ZOOM_OUT,
                font,
            )?,
            child(
                hwnd,
                w!("BUTTON"),
                "100%",
                WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32 | BS_NOTIFY as u32),
                ZOOM_RESET,
                font,
            )?,
            child(
                hwnd,
                w!("BUTTON"),
                "＋",
                WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32 | BS_NOTIFY as u32),
                ZOOM_IN,
                font,
            )?,
        ];
        let select = child(
            hwnd,
            w!("COMBOBOX"),
            "",
            WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_HASSTRINGS) as u32),
            SELECT,
            font,
        )?;
        for tool in TOOLS {
            SendMessageW(
                select,
                CB_ADDSTRING,
                None,
                Some(LPARAM(
                    HSTRING::from(navigation_label(tool)).as_ptr() as isize
                )),
            );
        }
        SendMessageW(select, CB_SETCURSEL, Some(WPARAM(0)), None);
        let mut buttons = Vec::new();
        for (id, label, width) in [
            (RUN, "実行", 72),
            (CANCEL, "中止", 64),
            (EXAMPLE, "入力例", 72),
            (OPEN, "入力を開く", 100),
            (SAVE_INPUT, "入力を保存", 100),
            (COPY, "結果をコピー", 110),
            (SAVE, "結果を保存", 110),
            (CLEAR, "クリア", 72),
            (HELP, "ヘルプ", 72),
        ] {
            buttons.push((
                id,
                child(
                    hwnd,
                    w!("BUTTON"),
                    label,
                    WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32 | BS_NOTIFY as u32),
                    id,
                    font,
                )?,
                width,
            ));
        }
        let description = child(hwnd, w!("STATIC"), "", WINDOW_STYLE(0), 0, font)?;
        let input_label = child(hwnd, w!("STATIC"), "入力", WINDOW_STYLE(0), 0, font)?;
        let style = WS_TABSTOP
            | WS_VSCROLL
            | WS_HSCROLL
            | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_WANTRETURN) as u32);
        let input = child(hwnd, w!("EDIT"), "", style, INPUT, mono)?;
        let output_label = child(hwnd, w!("STATIC"), "結果", WINDOW_STYLE(0), 0, font)?;
        let output = child(
            hwnd,
            w!("EDIT"),
            "",
            style | WINDOW_STYLE(ES_READONLY as u32),
            OUTPUT,
            mono,
        )?;
        let qr_view = child(
            hwnd,
            w!("STATIC"),
            "",
            WINDOW_STYLE(SS_OWNERDRAW.0),
            QR_VIEW,
            font,
        )?;
        SendMessageW(input, EM_SETLIMITTEXT, Some(WPARAM(MAX_INPUT)), None);
        SendMessageW(output, EM_SETLIMITTEXT, Some(WPARAM(0)), None);
        let status = child(hwnd, w!("STATIC"), "", WINDOW_STYLE(0), 0, font)?;
        let (tx, rx) = mpsc::channel();
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                hwnd,
                heading,
                zoom_buttons,
                zoom: 100,
                #[cfg(test)]
                test_dpi: None,
                scroll: Cell::new(0),
                content_height: Cell::new(0),
                background: CreateSolidBrush(COLORREF(0x00fcf9f6)),
                paper: CreateSolidBrush(COLORREF(0x00ffffff)),
                title_font: HFONT::default(),
                select_label,
                select,
                buttons,
                description,
                fields: Vec::new(),
                input_label,
                input,
                input_source: String::new(),
                output_label,
                output,
                qr_view,
                status,
                tool: Tool::Regex,
                result: None,
                fonts: vec![font, mono],
                font,
                mono,
                running: None,
                tx,
                rx,
                closing: false,
                recording_ui: None,
            })
        });
        state(|s| s.apply_tool(Tool::Regex)).transpose()?;
        state(State::refresh_fonts);
        // Initial bounds follow the monitor work area, including high-DPI laptops.
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut monitor_info).as_bool() {
            let work = monitor_info.rcWork;
            let dpi = GetDpiForWindow(hwnd).max(96) as i32;
            let width = (1100 * dpi / 96).min(work.right - work.left);
            let height = (800 * dpi / 96).min(work.bottom - work.top);
            let _ = SetWindowPos(
                hwnd,
                None,
                work.left + (work.right - work.left - width) / 2,
                work.top + (work.bottom - work.top - height) / 2,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        Ok(hwnd)
    }
}

fn done() {
    let closing = state(|s| {
        let Ok(result) = s.rx.try_recv() else {
            return false;
        };
        if let Some(worker) = s.running.take() {
            let _ = worker.thread.join();
        }
        s.recording_ui.take();
        match result {
            Ok(output) => {
                set_text(s.output, &output.text);
                set_text(s.status, &output.notice);
                s.result = Some(output);
            }
            Err(error) => {
                set_text(s.output, &format!("エラー: {error}"));
                set_text(s.status, &error);
                s.result = None;
            }
        }
        s.enabled();
        s.layout();
        s.closing
    })
    .unwrap_or(false);
    if closing {
        if let Some(hwnd) = state(|s| s.hwnd) {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
    }
}

fn command(id: u16, code: u16) {
    if code == EN_SETFOCUS as u16 || code == CBN_SETFOCUS as u16 || code == BN_SETFOCUS as u16 {
        state(|s| unsafe {
            if let Ok(control) = GetDlgItem(Some(s.hwnd), id as i32) {
                s.reveal(control);
            }
        });
        return;
    }
    if (id == INPUT || (FIELD_BASE..FIELD_BASE + 32).contains(&id))
        && (u32::from(code) == EN_CHANGE || u32::from(code) == CBN_SELCHANGE)
    {
        state(|s| {
            if s.running.is_none() {
                if id == INPUT {
                    s.input_source = text(s.input);
                }
                s.result = None;
                set_text(s.output, "");
                set_text(
                    s.status,
                    "入力・条件を変更しました。「実行」で結果を更新します。",
                );
                s.enabled();
            }
        });
        return;
    }
    match id {
        ZOOM_OUT => {
            state(|s| s.zoom_to(s.zoom.saturating_sub(25)));
        }
        ZOOM_RESET => {
            state(|s| s.zoom_to(100));
        }
        ZOOM_IN => {
            state(|s| s.zoom_to(s.zoom + 25));
        }
        SELECT if u32::from(code) == CBN_SELCHANGE => {
            state(|s| {
                let i = unsafe { SendMessageW(s.select, CB_GETCURSEL, None, None).0 };
                if let Some(tool) = TOOLS.get(i as usize) {
                    let _ = s.apply_tool(*tool);
                }
            });
        }
        RUN => match state(|s| (s.tool, s.running.is_some())) {
            Some((_, true)) => {}
            Some((Tool::Recording, false)) => start_recording(),
            Some((Tool::ResizeImages, false)) => start_images(),
            Some((Tool::VideoEdit, false)) => start_video(),
            _ => {
                state(State::start);
            }
        },
        CANCEL => {
            state(|s| {
                if let Some(r) = &s.running {
                    if let Some(guard) = &s.recording_ui {
                        guard.stopping();
                    }
                    r.cancel.store(true, Ordering::Relaxed);
                    set_text(
                        s.status,
                        if s.recording_ui.is_some() {
                            "収録を終了して保存しています…"
                        } else {
                            "中止を要求しました…"
                        },
                    );
                }
            });
        }
        EXAMPLE => {
            state(|s| {
                s.input_source = s.tool.example(&s.params());
                set_text(s.input, &s.input_source);
                s.result = None;
                set_text(s.output, "");
                s.enabled();
            });
        }
        CLEAR => {
            state(|s| {
                set_text(s.input, "");
                s.input_source.clear();
                set_text(s.output, "");
                s.result = None;
                set_text(s.status, "入力と結果をクリアしました");
                s.enabled();
            });
        }
        OPEN => open_input(),
        SAVE_INPUT => save(true),
        SAVE => save(false),
        COPY => copy(),
        HELP => {
            if let Err(e) = help() {
                error(&e.to_string());
            }
        }
        _ => {}
    }
}

extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_ERASEBKGND => {
            state(|s| unsafe {
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                FillRect(HDC(wparam.0 as *mut _), &rect, s.background);
            });
            LRESULT(1)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            appearance::color(hwnd, message, wparam, lparam)
        }
        WM_VSCROLL => {
            state(|s| unsafe {
                let mut info = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_TRACKPOS,
                    ..Default::default()
                };
                let _ = GetScrollInfo(hwnd, SB_VERT, &mut info);
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let target = match SCROLLBAR_COMMAND((wparam.0 & 0xffff) as i32) {
                    SB_LINEUP => s.scroll.get() - s.scale(40),
                    SB_LINEDOWN => s.scroll.get() + s.scale(40),
                    SB_PAGEUP => s.scroll.get() - rect.bottom * 4 / 5,
                    SB_PAGEDOWN => s.scroll.get() + rect.bottom * 4 / 5,
                    SB_TOP => 0,
                    SB_BOTTOM => s.content_height.get(),
                    SB_THUMBTRACK | SB_THUMBPOSITION => info.nTrackPos,
                    _ => s.scroll.get(),
                };
                s.scroll.set(target);
                s.layout();
            });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam.0 >> 16) as u16 as i16 as i32;
            state(|s| {
                if wparam.0 & MK_CONTROL.0 as usize != 0 {
                    s.zoom_to(if delta > 0 {
                        s.zoom + 25
                    } else {
                        s.zoom.saturating_sub(25)
                    });
                } else {
                    s.scroll_by(-delta * s.scale(48) / 120);
                }
            });
            LRESULT(0)
        }
        WM_HOTKEY if wparam.0 == crate::recording::STOP_HOTKEY as usize => {
            command(CANCEL, 0);
            LRESULT(0)
        }
        WM_DRAWITEM => {
            if lparam.0 != 0 {
                let item = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
                if item.CtlID == u32::from(QR_VIEW) {
                    draw_qr(item);
                    return LRESULT(1);
                }
                if (RUN..=ZOOM_IN).contains(&(item.CtlID as u16)) {
                    appearance::draw_button(item);
                    return LRESULT(1);
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_COMMAND => {
            command((wparam.0 & 0xffff) as u16, (wparam.0 >> 16) as u16);
            LRESULT(0)
        }
        WM_SIZE => {
            state(|s| s.layout());
            LRESULT(0)
        }
        WM_DPICHANGED => {
            unsafe {
                let rect = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            state(|s| s.refresh_fonts());
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            unsafe {
                let dpi = GetDpiForWindow(hwnd).max(96) as i32;
                let info = &mut *(lparam.0 as *mut MINMAXINFO);
                let mut monitor = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let (work_width, work_height) = if GetMonitorInfoW(
                    MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                    &mut monitor,
                )
                .as_bool()
                {
                    (
                        monitor.rcWork.right - monitor.rcWork.left,
                        monitor.rcWork.bottom - monitor.rcWork.top,
                    )
                } else {
                    (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
                };
                info.ptMinTrackSize = POINT {
                    x: (480 * dpi / 96).min(work_width),
                    y: (400 * dpi / 96).min(work_height),
                };
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let close = state(|s| {
                if let Some(r) = &s.running {
                    s.closing = true;
                    r.cancel.store(true, Ordering::Relaxed);
                    set_text(s.status, "処理を中止して終了します…");
                    false
                } else {
                    true
                }
            })
            .unwrap_or(true);
            if close {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        DONE => {
            done();
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn draw_qr(item: &DRAWITEMSTRUCT) {
    unsafe {
        FillRect(
            item.hDC,
            &item.rcItem,
            HBRUSH(GetStockObject(WHITE_BRUSH).0),
        );
    }
    state(|s| {
        let Some(qr) = s.result.as_ref().and_then(|r| r.qr.as_ref()) else {
            return;
        };
        let side = qr.width as i32 + 8;
        let scale = ((item.rcItem.right - item.rcItem.left)
            .min(item.rcItem.bottom - item.rcItem.top)
            / side)
            .max(0);
        if scale == 0 {
            return;
        }
        let left = item.rcItem.left + (item.rcItem.right - item.rcItem.left - scale * side) / 2;
        let top = item.rcItem.top + (item.rcItem.bottom - item.rcItem.top - scale * side) / 2;
        for y in 0..qr.width {
            for x in 0..qr.width {
                if qr.dark[y * qr.width + x] {
                    let rect = RECT {
                        left: left + (x as i32 + 4) * scale,
                        top: top + (y as i32 + 4) * scale,
                        right: left + (x as i32 + 5) * scale,
                        bottom: top + (y as i32 + 5) * scale,
                    };
                    unsafe {
                        FillRect(item.hDC, &rect, HBRUSH(GetStockObject(BLACK_BRUSH).0));
                    }
                }
            }
        }
    });
}

fn error(message: &str) {
    let owner = state(|s| s.hwnd);
    unsafe {
        MessageBoxW(
            owner,
            &HSTRING::from(message),
            w!("yytoolbox"),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn dialog(save: Option<&str>) -> Option<PathBuf> {
    unsafe {
        let owner = state(|s| s.hwnd)?;
        let dialog: IFileDialog = if save.is_some() {
            CoCreateInstance::<_, IFileSaveDialog>(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
                .ok()?
                .cast()
                .ok()?
        } else {
            CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
                .ok()?
                .cast()
                .ok()?
        };
        let filters = [COMDLG_FILTERSPEC {
            pszName: w!("すべてのファイル"),
            pszSpec: w!("*.*"),
        }];
        let _ = dialog.SetFileTypes(&filters);
        if let Some(name) = save {
            let _ = dialog.SetFileName(&HSTRING::from(name));
            if let Some((_, ext)) = name.rsplit_once('.') {
                let _ = dialog.SetDefaultExtension(&HSTRING::from(ext));
            }
        }
        dialog.Show(Some(owner)).ok()?;
        let item = dialog.GetResult().ok()?;
        let path = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let result = path.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(path.0.cast()));
        result
    }
}

fn open_input() {
    let Some(path) = dialog(None) else {
        return;
    };
    let loaded = (|| -> std::result::Result<String, String> {
        let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(MAX_INPUT as u64 + 4)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.starts_with(b"\xef\xbb\xbf") {
            bytes.drain(..3);
        }
        if bytes.len() > MAX_INPUT {
            return Err("入力ファイルは1 MiBまでです".into());
        }
        String::from_utf8(bytes).map_err(|_| "入力ファイルはUTF-8で保存してください".into())
    })();
    match loaded {
        Ok(input) => {
            state(|s| {
                set_text(s.input, &input);
                s.input_source = input;
                set_text(s.output, "");
                s.result = None;
                s.enabled();
            });
        }
        Err(e) => error(&e),
    }
}

fn start_recording() {
    let Some((hwnd, params)) = state(|s| (s.hwnd, s.params())) else {
        return;
    };
    let audio_only = crate::recording::audio_only(&params);
    let filename = if audio_only {
        "recording.mp3"
    } else {
        "recording.mp4"
    };
    // Validate before showing a save dialog or hiding the application.
    if let Err(message) = crate::recording::Plan::new(
        PathBuf::from(filename),
        RECT {
            right: 2,
            bottom: 2,
            ..Default::default()
        },
        &params,
    ) {
        error(&message);
        return;
    }
    let Some(destination) = dialog(Some(filename)) else {
        return;
    };
    let hotkey = match crate::recording::StopHotkey::register(
        hwnd,
        yytoolbox::value(&params, "stop_key", "なし（ボタンのみ）"),
    ) {
        Ok(hotkey) => hotkey,
        Err(message) => {
            error(&message);
            state(|s| {
                if let Some(field) = s.fields.iter().find(|f| f.definition.key == "stop_key") {
                    s.reveal(field.control);
                    unsafe {
                        let _ = SetFocus(Some(field.control));
                    }
                }
            });
            return;
        }
    };
    let started = (|| -> yytoolbox::Result<_> {
        let mut guard = crate::recording::UiGuard::hide(hwnd, hotkey)?;
        let rect = if audio_only {
            RECT::default()
        } else {
            let Some(rect) =
                crate::recording::area(yytoolbox::value(&params, "area", "全画面（メイン）"))?
            else {
                return Ok(None);
            };
            rect
        };
        let plan = crate::recording::Plan::new(destination, rect, &params)?;
        guard.show_controls(
            yytoolbox::integer(&params, "seconds", 1, 3600)?,
            yytoolbox::value(&params, "stop_key", "なし（ボタンのみ）"),
            CANCEL,
            audio_only,
        )?;
        Ok(Some((guard, plan)))
    })();
    match started {
        Ok(Some((guard, plan))) => {
            state(|s| {
                s.recording_ui = Some(guard);
                s.start_job(Job::Recording(plan));
            });
        }
        Ok(None) => {}
        Err(message) => error(&message),
    }
}
fn start_video() {
    let Some(params) = state(|s| s.params()) else {
        return;
    };
    let edit = match yytoolbox::video::Edit::parse(&params) {
        Ok(edit) => edit,
        Err(e) => {
            error(&e);
            return;
        }
    };
    let selected = (|| -> windows::core::Result<Option<PathBuf>> {
        unsafe {
            let Some(hwnd) = state(|s| s.hwnd) else {
                return Ok(None);
            };
            let picker: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            picker.SetTitle(w!("編集する動画を選択"))?;
            picker.SetOptions(picker.GetOptions()? | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM)?;
            picker.SetFileTypes(&[COMDLG_FILTERSPEC {
                pszName: w!("動画（Windows対応コーデック）"),
                pszSpec: w!("*.mp4;*.m4v;*.mov;*.wmv;*.avi;*.mkv;*.webm"),
            }])?;
            if picker.Show(Some(hwnd)).is_err() {
                return Ok(None);
            }
            let raw = picker.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = raw.to_string();
            CoTaskMemFree(Some(raw.0.cast()));
            Ok(Some(PathBuf::from(path?)))
        }
    })();
    match selected {
        Ok(Some(source)) => {
            if let Some(destination) = dialog(Some("edited.mp4")) {
                state(|s| s.start_job(Job::VideoEdit(source, destination, edit)));
            }
        }
        Ok(None) => {}
        Err(e) => error(&e.to_string()),
    }
}

fn start_images() {
    let Some(params) = state(|s| s.params()) else {
        return;
    };
    let selected = (|| -> windows::core::Result<Option<_>> {
        unsafe {
            let Some(hwnd) = state(|s| s.hwnd) else {
                return Ok(None);
            };
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            dialog.SetTitle(w!("リサイズする画像を複数選択"))?;
            dialog.SetOptions(
                dialog.GetOptions()?
                    | FOS_ALLOWMULTISELECT
                    | FOS_FILEMUSTEXIST
                    | FOS_FORCEFILESYSTEM,
            )?;
            dialog.SetFileTypes(&[COMDLG_FILTERSPEC {
                pszName: w!("画像 PNG/JPEG/WebP/BMP"),
                pszSpec: w!("*.png;*.jpg;*.jpeg;*.webp;*.bmp"),
            }])?;
            if dialog.Show(Some(hwnd)).is_err() {
                return Ok(None);
            }
            let items = dialog.GetResults()?;
            let count = items.GetCount()?;
            if count > 1000 {
                return Err(windows::core::Error::new(
                    E_INVALIDARG,
                    "画像は1000個までです",
                ));
            }
            let mut files = Vec::new();
            for index in 0..count {
                let item = items.GetItemAt(index)?;
                let raw = item.GetDisplayName(SIGDN_FILESYSPATH)?;
                let path = raw.to_string();
                CoTaskMemFree(Some(raw.0.cast()));
                files.push(PathBuf::from(path?));
            }
            let folder: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            folder.SetTitle(w!("リサイズ画像の出力フォルダ"))?;
            folder.SetOptions(folder.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
            if folder.Show(Some(hwnd)).is_err() {
                return Ok(None);
            }
            let item = folder.GetResult()?;
            let raw = item.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = raw.to_string();
            CoTaskMemFree(Some(raw.0.cast()));
            Ok(Some((files, PathBuf::from(path?))))
        }
    })();
    match selected {
        Ok(Some((files, folder))) => {
            state(|s| s.start_job(Job::Images(files, folder, params)));
        }
        Ok(None) => {}
        Err(e) => error(&e.to_string()),
    }
}

fn save(input: bool) {
    // MP4 was already finalized at the destination selected before recording.
    // Do not offer a second save dialog for its textual status report.
    if !input && state(|s| matches!(s.tool, Tool::Recording | Tool::VideoEdit)).unwrap_or(false) {
        return;
    }
    let data = state(|s| {
        if input {
            Some((
                if s.tool == Tool::DummyCsv {
                    "dummy-definition.yaml"
                } else {
                    "input.txt"
                },
                s.input_source.as_bytes().to_vec(),
            ))
        } else {
            s.result.as_ref().map(|r| (r.filename, r.bytes.clone()))
        }
    })
    .flatten();
    let Some((filename, bytes)) = data else {
        return;
    };
    let Some(path) = dialog(Some(filename)) else {
        return;
    };
    if let Err(e) = save_bytes(&path, &bytes) {
        error(&e.to_string());
    } else {
        state(|s| set_text(s.status, &format!("保存しました: {}", path.display())));
    }
}
fn save_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn copy() {
    let Some((owner, text)) =
        state(|s| s.result.as_ref().map(|r| (s.hwnd, r.text.clone()))).flatten()
    else {
        return;
    };
    if let Err(e) = copy_text(owner, &text) {
        error(&e.to_string());
    }
}
fn copy_text(owner: HWND, text: &str) -> windows::core::Result<()> {
    unsafe {
        let bytes: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let allocation = GlobalAlloc(GMEM_MOVEABLE, bytes.len() * 2)?;
        let ptr = GlobalLock(allocation);
        if ptr.is_null() {
            let _ = GlobalFree(Some(allocation));
            return Err(windows::core::Error::from_thread());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast(), bytes.len());
        let _ = GlobalUnlock(allocation);
        if let Err(e) = OpenClipboard(Some(owner)) {
            let _ = GlobalFree(Some(allocation));
            return Err(e);
        }
        let result =
            EmptyClipboard().and_then(|_| SetClipboardData(13, Some(HANDLE(allocation.0))));
        let _ = CloseClipboard();
        if let Err(e) = result {
            let _ = GlobalFree(Some(allocation));
            return Err(e);
        }
        Ok(())
    }
}

extern "system" fn help_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_NCDESTROY {
        unsafe {
            let font = HFONT(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut _);
            if !font.is_invalid() {
                let _ = DeleteObject(font.into());
            }
        }
    }
    if message == WM_SIZE {
        unsafe {
            if let Ok(edit) = GetDlgItem(Some(hwnd), 1) {
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let _ = MoveWindow(edit, 0, 0, rect.right, rect.bottom, true);
            }
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
fn help() -> windows::core::Result<()> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let _ = RegisterClassW(&WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: HELP_CLASS,
            lpfnWndProc: Some(help_proc),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            HELP_CLASS,
            w!("yytoolbox ヘルプ"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            850,
            650,
            state(|s| s.hwnd),
            None,
            Some(instance.into()),
            None,
        )?;
        let font = ui_font(GetDpiForWindow(hwnd).max(96), false);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, font.0 as isize);
        let edit = child(
            hwnd,
            w!("EDIT"),
            &include_str!("../help/help.md").replace('\n', "\r\n"),
            WS_VSCROLL
                | WS_HSCROLL
                | WINDOW_STYLE(
                    (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_AUTOHSCROLL) as u32,
                ),
            1,
            font,
        )?;
        SendMessageW(edit, EM_SETLIMITTEXT, Some(WPARAM(0)), None);
        let _ = ShowWindow(hwnd, SW_SHOW);
        SendMessageW(hwnd, WM_SIZE, None, None);
        Ok(())
    }
}

pub fn run() -> std::result::Result<(), String> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        });
    }
    let hwnd = create().map_err(|e| e.to_string())?;
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let main_target = msg.hwnd == hwnd || IsChild(hwnd, msg.hwnd).as_bool();
            if main_target && msg.message == WM_MOUSEWHEEL && GetKeyState(VK_CONTROL.0 as i32) < 0 {
                let delta = (msg.wParam.0 >> 16) as u16 as i16;
                command(if delta > 0 { ZOOM_IN } else { ZOOM_OUT }, 0);
                continue;
            }
            if msg.message == WM_KEYDOWN {
                let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                if ctrl && main_target {
                    let id = match msg.wParam.0 as u16 {
                        0xbb | 0x6b => Some(ZOOM_IN),
                        0xbd | 0x6d => Some(ZOOM_OUT),
                        0x30 | 0x60 => Some(ZOOM_RESET),
                        _ => None,
                    };
                    if let Some(id) = id {
                        command(id, 0);
                        continue;
                    }
                }
                if msg.wParam.0 == VK_F1.0 as usize {
                    command(HELP, 0);
                    continue;
                }
                if ctrl && msg.wParam.0 == VK_RETURN.0 as usize {
                    command(RUN, 0);
                    continue;
                }
                if msg.wParam.0 == VK_ESCAPE.0 as usize {
                    escape();
                    continue;
                }
            }
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    STATE.with(|s| s.borrow_mut().take());
    unsafe {
        CoUninitialize();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn caption_fits(hwnd: HWND, wrap: bool, inset: i32) {
        unsafe {
            let dc = GetDC(Some(hwnd));
            let font = HFONT(SendMessageW(hwnd, WM_GETFONT, None, None).0 as *mut _);
            let old = SelectObject(dc, font.into());
            let mut bounds = RECT::default();
            GetClientRect(hwnd, &mut bounds).unwrap();
            let mut measured = RECT {
                right: (bounds.right - inset).max(1),
                ..Default::default()
            };
            let mut chars: Vec<_> = text(hwnd).encode_utf16().collect();
            DrawTextW(
                dc,
                &mut chars,
                &mut measured,
                DT_CALCRECT | DT_NOPREFIX | if wrap { DT_WORDBREAK } else { DT_SINGLELINE },
            );
            SelectObject(dc, old);
            ReleaseDC(Some(hwnd), dc);
            assert!(
                measured.right <= bounds.right - inset && measured.bottom <= bounds.bottom,
                "clipped caption {} needs {measured:?}, has {bounds:?}",
                text(hwnd)
            );
        }
    }

    fn pump_until(predicate: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            unsafe {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    if message.message != WM_QUIT {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
            }
            if predicate() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "UI timeout: {:?}",
                state(|s| (s.tool, s.running.is_some(), text(s.status), text(s.output)))
            );
            unsafe {
                MsgWaitForMultipleObjectsEx(None, 20, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            }
        }
    }

    fn choose(tool: Tool) {
        let i = TOOLS.iter().position(|t| *t == tool).unwrap();
        state(|s| unsafe {
            SendMessageW(s.select, CB_SETCURSEL, Some(WPARAM(i)), None);
        });
        command(SELECT, CBN_SELCHANGE as u16);
        assert_eq!(state(|s| s.tool), Some(tool));
    }

    #[test]
    fn every_tool_layout_and_async_csv_save_and_clear() {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
        }
        let hwnd = create().unwrap();
        struct Frame(HWND);
        impl Drop for Frame {
            fn drop(&mut self) {
                unsafe {
                    let _ = DestroyWindow(self.0);
                }
                STATE.with(|s| s.borrow_mut().take());
            }
        }
        let _frame = Frame(hwnd);
        // Geometry/font checks do not need compositor repainting for each case.
        // Keep real controls, then show the frame for asynchronous interaction tests.
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        // Actual GDI controls/fonts at each modeled monitor DPI; changing the
        // user's Windows display settings is not necessary for this regression.
        for dpi in [192, 144, 120, 96] {
            state(|s| s.test_dpi = Some(dpi));
            for zoom in [200, 150, 125, 100, 75] {
                state(|s| s.zoom_to(zoom));
                for tool in TOOLS.into_iter().rev() {
                    choose(tool);
                    for logical_width in [480, 640, 1100] {
                        let width = logical_width * dpi as i32 / 96;
                        unsafe {
                            MoveWindow(hwnd, 0, 0, width, 520, false).unwrap();
                        }
                        state(|s| {
                            s.scroll.set(0);
                            s.layout();
                            let mut client = RECT::default();
                            let mut origin = POINT::default();
                            unsafe {
                                GetClientRect(hwnd, &mut client).unwrap();
                                ClientToScreen(hwnd, &mut origin).unwrap();
                            }
                            let mut controls = vec![
                                s.heading,
                                s.select_label,
                                s.select,
                                s.description,
                                s.output_label,
                                s.output,
                                s.status,
                            ];
                            controls.extend(s.zoom_buttons);
                            controls.extend(
                                s.buttons
                                    .iter()
                                    .filter(|b| s.button_visible(b.0))
                                    .map(|b| b.1),
                            );
                            if uses_input(tool) {
                                controls.extend([s.input_label, s.input]);
                            }
                            for field in &s.fields {
                                controls.extend([field.label, field.control]);
                            }
                            for h in controls {
                                let mut rect = RECT::default();
                                unsafe {
                                    GetWindowRect(h, &mut rect).unwrap();
                                }
                                assert!(
                                    rect.left >= origin.x
                                        && rect.right <= origin.x + client.right
                                        && rect.top >= origin.y
                                        && rect.bottom <= origin.y + s.content_height.get(),
                                    "{tool:?} zoom={zoom} width={width} unreachable control {} {rect:?} client={client:?} extent={}",
                                    text(h),
                                    s.content_height.get()
                                );
                            }
                            for h in [s.heading, s.description, s.status] {
                                caption_fits(h, true, 0);
                            }
                            for h in s.zoom_buttons.iter().copied().chain(
                                s.buttons
                                    .iter()
                                    .filter(|b| s.button_visible(b.0))
                                    .map(|b| b.1),
                            ) {
                                caption_fits(h, false, s.scale(12));
                            }
                            caption_fits(s.select, false, s.scale(30));
                            for field in &s.fields {
                                unsafe {
                                    let dc = GetDC(Some(field.label));
                                    let font = HFONT(
                                        SendMessageW(field.label, WM_GETFONT, None, None).0
                                            as *mut _,
                                    );
                                    let old = SelectObject(dc, font.into());
                                    let mut label_rect = RECT::default();
                                    GetClientRect(field.label, &mut label_rect).unwrap();
                                    let mut extent = SIZE::default();
                                    GetTextExtentPoint32W(
                                        dc,
                                        &field.definition.label.encode_utf16().collect::<Vec<_>>(),
                                        &mut extent,
                                    )
                                    .unwrap();
                                    assert!(
                                        extent.cx <= label_rect.right
                                            && extent.cy <= label_rect.bottom,
                                        "{tool:?} clipped label {} at {zoom}%",
                                        field.definition.label
                                    );
                                    let mut value_rect = RECT::default();
                                    GetClientRect(field.control, &mut value_rect).unwrap();
                                    for value in std::iter::once(field.definition.default.as_str())
                                        .chain(field.definition.choices.iter().copied())
                                    {
                                        GetTextExtentPoint32W(
                                            dc,
                                            &value.encode_utf16().collect::<Vec<_>>(),
                                            &mut extent,
                                        )
                                        .unwrap();
                                        let inset =
                                            s.scale(if field.definition.choices.is_empty() {
                                                8
                                            } else {
                                                30
                                            });
                                        assert!(
                                            extent.cx <= value_rect.right - inset,
                                            "{tool:?} clipped value {value} at {zoom}% width={width}: {} > {}",
                                            extent.cx,
                                            value_rect.right - inset
                                        );
                                    }
                                    SelectObject(dc, old);
                                    ReleaseDC(Some(field.label), dc);
                                }
                            }
                            // Keyboard focus must reveal fields below the viewport.
                            if let Some(field) = s.fields.last() {
                                s.reveal(field.control);
                                let mut rect = RECT::default();
                                unsafe {
                                    GetWindowRect(field.control, &mut rect).unwrap();
                                }
                                assert!(
                                    rect.top >= origin.y && rect.bottom <= origin.y + client.bottom,
                                    "focused control is unreachable"
                                );
                            }
                            for h in [s.input, s.output] {
                                if h == s.input && !uses_input(tool) {
                                    continue;
                                }
                                s.reveal(h);
                                let mut rect = RECT::default();
                                unsafe {
                                    GetWindowRect(h, &mut rect).unwrap();
                                }
                                assert!(
                                    rect.top >= origin.y && rect.bottom <= origin.y + client.bottom,
                                    "editor cannot fit in the viewport at {dpi} DPI / {zoom}%: {rect:?} {client:?}"
                                );
                            }
                            s.scroll_by(i32::MAX / 2);
                            let mut status = RECT::default();
                            unsafe {
                                GetWindowRect(s.status, &mut status).unwrap();
                            }
                            assert!(
                                status.bottom <= origin.y + client.bottom,
                                "status is unreachable"
                            );
                            s.scroll.set(0);
                            s.layout();
                        });
                    }
                }
            }
        }
        state(|s| s.test_dpi = None);
        state(|s| s.zoom_to(100));
        unsafe {
            MoveWindow(hwnd, 0, 0, 1100, 800, true).unwrap();
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        choose(Tool::DummyCsv);
        state(|s| {
            let preset = s
                .fields
                .iter()
                .find(|f| f.definition.key == "preset")
                .unwrap();
            unsafe {
                SendMessageW(preset.control, CB_SETCURSEL, Some(WPARAM(4)), None);
            }
            let rows = s
                .fields
                .iter()
                .find(|f| f.definition.key == "rows")
                .unwrap();
            set_text(rows.control, "17");
        });
        command(EXAMPLE, 0);
        assert!(state(|s| text(s.input).contains("TEST-ACCOUNT-")).unwrap());
        let definition = state(|s| s.input_source.clone()).unwrap();
        assert!(definition.starts_with("columns:\n"));
        let definition_dir = tempfile::tempdir().unwrap();
        let definition_path = definition_dir.path().join("definition.yaml");
        save_bytes(&definition_path, definition.as_bytes()).unwrap();
        assert_eq!(
            std::fs::read_to_string(definition_path).unwrap(),
            definition
        );
        command(RUN, 0);
        pump_until(|| state(|s| s.running.is_none() && s.result.is_some()).unwrap_or(false));
        let (bytes, body) = state(|s| {
            let result = s.result.as_ref().unwrap();
            (result.bytes.clone(), result.text.clone())
        })
        .unwrap();
        assert_eq!(
            csv::Reader::from_reader(body.as_bytes()).records().count(),
            17
        );
        state(|s| {
            let source = s.input_source.clone();
            s.zoom_to(200);
            assert_eq!(s.input_source, source);
            assert_eq!(s.result.as_ref().unwrap().bytes, bytes);
            s.zoom_to(100);
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.csv");
        save_bytes(&path, &bytes).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert!(
            unsafe { SendMessageW(state(|s| s.output).unwrap(), EM_GETLINECOUNT, None, None).0 }
                > 1,
            "multiline output is not displayed as lines"
        );
        command(CLEAR, 0);
        assert!(
            state(|s| s.result.is_none() && text(s.input).is_empty() && text(s.output).is_empty())
                .unwrap()
        );
        choose(Tool::Hash);
        let source = "first\nsecond\n";
        state(|s| {
            s.input_source = source.into();
            set_text(s.input, source);
        });
        command(RUN, 0);
        pump_until(|| state(|s| s.running.is_none() && s.result.is_some()).unwrap_or(false));
        let expected = yytoolbox::execute(Tool::Hash, source, &Params::new(), &|| false).unwrap();
        assert_eq!(
            state(|s| s.result.as_ref().unwrap().bytes.clone()).unwrap(),
            expected.bytes
        );
        assert_eq!(state(|s| text(s.input)).unwrap(), "first\r\nsecond\r\n");
        choose(Tool::Json);
        command(RUN, 0);
        pump_until(|| state(|s| s.running.is_none() && s.result.is_some()).unwrap_or(false));
        assert!(
            unsafe { SendMessageW(state(|s| s.output).unwrap(), EM_GETLINECOUNT, None, None).0 }
                > 1,
            "JSON line breaks are not displayed"
        );
        let input = state(|s| s.input).unwrap();
        set_text(input, "{bad json");
        // Multiline EDIT does not send EN_CHANGE for WM_SETTEXT; typing does.
        command(INPUT, EN_CHANGE as u16);
        command(RUN, 0);
        pump_until(|| {
            state(|s| {
                s.running.is_none() && s.result.is_none() && text(s.output).contains("エラー")
            })
            .unwrap_or(false)
        });
        choose(Tool::QrCode);
        command(RUN, 0);
        pump_until(|| {
            state(|s| s.running.is_none() && s.result.as_ref().is_some_and(|r| r.qr.is_some()))
                .unwrap_or(false)
        });
        assert!(
            state(|s| unsafe {
                IsWindowVisible(s.qr_view).as_bool() && !IsWindowVisible(s.output).as_bool()
            })
            .unwrap()
        );
        command(CLEAR, 0);
        assert!(
            state(|s| unsafe {
                !IsWindowVisible(s.qr_view).as_bool() && IsWindowVisible(s.output).as_bool()
            })
            .unwrap()
        );
        choose(Tool::ResizeImages);
        let files = tempfile::tempdir().unwrap();
        let input = files.path().join("ui-test.png");
        image::RgbaImage::from_pixel(32, 16, image::Rgba([0, 255, 0, 255]))
            .save(&input)
            .unwrap();
        state(|s| {
            s.start_job(Job::Images(
                vec![input],
                files.path().to_owned(),
                s.params(),
            ))
        });
        pump_until(|| state(|s| s.running.is_none() && s.result.is_some()).unwrap_or(false));
        assert!(files.path().join("ui-test-resized.png").is_file());
        assert!(state(|s| s.result.as_ref().unwrap().notice.contains("1件保存")).unwrap());
        let report = state(|s| s.result.take()).flatten().unwrap();
        choose(Tool::Recording);
        for audio_only in [true, false] {
            let id = state(|s| {
                let mode = s
                    .fields
                    .iter()
                    .find(|f| f.definition.key == "capture_mode")
                    .unwrap();
                unsafe {
                    SendMessageW(
                        mode.control,
                        CB_SETCURSEL,
                        Some(WPARAM(usize::from(audio_only))),
                        None,
                    );
                }
                FIELD_BASE
                    + s.fields
                        .iter()
                        .position(|f| f.definition.key == "capture_mode")
                        .unwrap() as u16
            })
            .unwrap();
            command(id, CBN_SELCHANGE as u16);
            state(|s| {
                for field in &s.fields {
                    let expected = !audio_only
                        || !matches!(field.definition.key, "area" | "fps" | "max_width");
                    assert_eq!(
                        unsafe { IsWindowEnabled(field.control) }.as_bool(),
                        expected,
                        "{}",
                        field.definition.key
                    );
                }
                let run = s.buttons.iter().find(|b| b.0 == RUN).unwrap().1;
                assert_eq!(
                    text(run),
                    if audio_only {
                        "録音を開始"
                    } else {
                        "録画を開始"
                    }
                );
            });
        }
        state(|s| {
            s.result = Some(report);
            s.enabled();
            let save = s.buttons.iter().find(|b| b.0 == SAVE).unwrap().1;
            assert!(
                !unsafe { IsWindowVisible(save) }.as_bool(),
                "recording must not ask for a second result save"
            );
            assert!(!unsafe { IsWindowEnabled(save) }.as_bool());
        });
        let key = "なし（ボタンのみ）";
        let hotkey = crate::recording::StopHotkey::register(hwnd, key).unwrap();
        let mut guard = crate::recording::UiGuard::hide(hwnd, hotkey).unwrap();
        guard.show_controls(60, key, CANCEL, false).unwrap();
        assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
        let flag = Arc::new(AtomicBool::new(false));
        state(|s| {
            s.recording_ui = Some(guard);
            s.running = Some(Running {
                cancel: flag.clone(),
                thread: std::thread::spawn(|| {}),
            });
        });
        // Exercise the same Escape route used by the main message loop, with
        // a live recording guard and cancellation flag. Explicit stop still works.
        escape();
        assert!(!flag.load(Ordering::Relaxed));
        command(CANCEL, 0);
        assert!(flag.load(Ordering::Relaxed));
        let running = state(|s| {
            s.recording_ui.take();
            s.running.take().unwrap()
        })
        .unwrap();
        running.thread.join().unwrap();
        assert!(unsafe { IsWindowVisible(hwnd) }.as_bool());
    }
}

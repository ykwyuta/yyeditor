//! Markdown / HTML のプレビュー（エディタの右側）。WebView2 で表示する。
//!
//! * ページ（`https://yy-preview.local/index.html`）と mermaid・KaTeX などの埋め込みファイルは、
//!   リソース要求を横取りしてメモリから返す（一時ファイルを作らない）
//! * `https://yy-doc.local/` の資源要求は、デコード後のパスと文書フォルダ内への解決を検証する。
//!   手元・SSH ともバックグラウンドで読み込み、取り寄せ終わってから答える（[`DocBase`]）。
//! * Markdown の編集は、ページを読み直さずに本文だけを差し替える（スクロール位置を保つ）
//! * 外部のリンクは既定のブラウザで、文書のフォルダの Markdown・HTML はエディタで開く
//!
//! WebView2 ランタイムがない環境（Wine など）では、その旨を表示する。
//!
//! WebView2 の関数の呼び出し中にイベントの処理が呼ばれることがあるため、状態（[`Shared`]）を
//! 借用したまま WebView2 の関数を呼ばない。

use std::cell::RefCell;
use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    AcceleratorKeyPressedEventHandler, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, NavigationCompletedEventHandler,
    NavigationStartingEventHandler, NewWindowRequestedEventHandler,
    WebResourceRequestedEventHandler,
};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_WINDOW, DT_CENTER, DT_NOPREFIX, DT_WORDBREAK, DrawTextW, EndPaint, FillRect,
    GetSysColorBrush, InvalidateRect, PAINTSTRUCT, SelectObject, SetBkMode, TRANSPARENT,
};
use windows::Win32::System::Com::{CoTaskMemFree, IStream};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_SHIFT};
use windows::Win32::UI::Shell::SHCreateMemStream;
#[cfg(not(test))]
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HRESULT, HSTRING, Interface, PCWSTR, PWSTR, Result, w};
use yy_preview::{DOC_HOST, Kind, PAGE_URL, PREVIEW_HOST, PageOptions};

use crate::PREVIEW_CLASS;
use crate::util::Context;

/// プレビューのリンクから文書を開く要求（`lparam` は `Box<PathBuf>` のポインタ）。フレームに送る。
pub(crate) const WM_APP_PREVIEW_OPEN: u32 = WM_APP + 21;
/// WebView2 を使えなかった（プレビューの欄には案内を表示している）。フレームに送る。
pub(crate) const WM_APP_PREVIEW_FAILED: u32 = WM_APP + 22;
// Poll owned result channels on the UI thread; never post heap pointers to a dying HWND.
const FETCH_TIMER: usize = 0x5959;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const FETCH_LIMIT: u64 = 32 << 20;
const MAX_FETCHES: usize = 4;
static ACTIVE_FETCHES: AtomicUsize = AtomicUsize::new(0);

struct FetchPermit;
impl FetchPermit {
    fn acquire() -> Option<Self> {
        crate::util::try_increment(&ACTIVE_FETCHES, MAX_FETCHES).then_some(Self)
    }
}
impl Drop for FetchPermit {
    fn drop(&mut self) {
        ACTIVE_FETCHES.fetch_sub(1, Ordering::AcqRel);
    }
}

/// 文書の相対パスの起点（`https://yy-doc.local/` に当たるフォルダ）。
#[derive(Clone)]
pub(crate) enum DocBase {
    /// 手元のフォルダ（資源の要求を検証してから読む）
    Local(PathBuf),
    /// SSH 接続先のフォルダ（要求ごとにエージェントから取り寄せる）
    Remote {
        session: std::sync::Arc<yy_remote::Session>,
        dir: yy_remote::RemoteUri,
    },
}

impl PartialEq for DocBase {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (DocBase::Local(a), DocBase::Local(b)) => a == b,
            (DocBase::Remote { dir: a, .. }, DocBase::Remote { dir: b, .. }) => a == b,
            _ => false,
        }
    }
}

/// 取り寄せている要求（答えるまで WebView2 を待たせる）。
// Bound queued requests separately from workers and completed result buffers.
const MAX_REQUESTS: usize = 32;
static ACTIVE_REQUESTS: AtomicUsize = AtomicUsize::new(0);
struct RequestPermit;
impl RequestPermit {
    fn acquire() -> Option<Self> {
        crate::util::try_increment(&ACTIVE_REQUESTS, MAX_REQUESTS).then_some(Self)
    }
}
impl Drop for RequestPermit {
    fn drop(&mut self) {
        ACTIVE_REQUESTS.fetch_sub(1, Ordering::AcqRel);
    }
}

struct PendingFetch {
    owner: isize,
    env: ICoreWebView2Environment,
    args: ICoreWebView2WebResourceRequestedEventArgs,
    deferral: ICoreWebView2Deferral,
    receiver: Option<mpsc::Receiver<(io::Result<Vec<u8>>, FetchPermit)>>,
    work: Option<(Option<DocBase>, String)>,
    _request: RequestPermit,
    cancelled: Arc<AtomicBool>,
    started: Instant,
    mime: &'static str,
}

thread_local! {
    /// 取り寄せている要求（番号 → 要求）
    static PENDING: RefCell<std::collections::HashMap<u64, PendingFetch>> =
        RefCell::new(std::collections::HashMap::new());
    static NEXT_FETCH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// WebView2 を使えないときなどにプレビューの欄に表示する文字列。
    static NOTICE: RefCell<std::collections::HashMap<isize, String>> =
        RefCell::new(std::collections::HashMap::new());
}

// ---- WebView2 の読み込み --------------------------------------------------

pub(crate) type CreateEnvironmentFn = unsafe extern "system" fn(
    browser_folder: PCWSTR,
    user_data_folder: PCWSTR,
    options: *mut c_void,
    handler: *mut c_void,
) -> HRESULT;

// MSVC では WebView2 の読み込み処理（WebView2LoaderStatic.lib。webview2-com-sys に同梱）を
// 静的にリンクする（WebView2Loader.dll を配布しなくてよい）
#[cfg(target_env = "msvc")]
#[link(name = "WebView2LoaderStatic", kind = "static")]
unsafe extern "system" {
    fn CreateCoreWebView2EnvironmentWithOptions(
        browser_folder: PCWSTR,
        user_data_folder: PCWSTR,
        options: *mut c_void,
        handler: *mut c_void,
    ) -> HRESULT;
}

#[cfg(target_env = "msvc")]
pub(crate) fn create_environment_fn() -> std::result::Result<CreateEnvironmentFn, String> {
    Ok(CreateCoreWebView2EnvironmentWithOptions)
}

/// MSVC 以外（開発・テスト用の MinGW ビルド）では、実行ファイルと同じフォルダの
/// WebView2Loader.dll があれば使う。
#[cfg(not(target_env = "msvc"))]
pub(crate) fn create_environment_fn() -> std::result::Result<CreateEnvironmentFn, String> {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    unsafe {
        let module = LoadLibraryW(w!("WebView2Loader.dll"))
            .map_err(|_| "WebView2Loader.dll が見つかりません".to_owned())?;
        let f = GetProcAddress(
            module,
            windows::core::s!("CreateCoreWebView2EnvironmentWithOptions"),
        )
        .ok_or_else(|| "WebView2Loader.dll が壊れています".to_owned())?;
        Ok(std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            CreateEnvironmentFn,
        >(f))
    }
}

/// WebView2 のデータ（キャッシュなど）を置くフォルダ（%LOCALAPPDATA%\yyeditor\WebView2）。
fn user_data_folder() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("yyeditor")
        .join("WebView2")
}

// ---- 状態 ------------------------------------------------------------------

/// 使える状態の WebView2。
#[derive(Clone)]
struct Web {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
}

/// 表示するページ。
#[derive(Default)]
struct Page {
    /// `PAGE_URL` で返す内容
    html: String,
    /// `html` の版（変えるたびに増やす）と、最後に返した版
    version: u64,
    served: u64,
    /// `html` の種類（案内の表示なら `None`）
    kind: Option<Kind>,
    /// 読み込み終わったページの種類（読み込み中は `None`）
    loaded: Option<Kind>,
    /// 読み込み中の移動の ID
    navigation: Option<u64>,
    /// 文書の相対パスの起点
    folder: Option<DocBase>,
    /// エディタの表示位置（先頭の行）
    line: Option<usize>,
}

struct Shared {
    frame: HWND,
    container: HWND,
    web: RefCell<Option<Web>>,
    page: RefCell<Page>,
    visible: std::cell::Cell<bool>,
    /// WebView2 を使えなかった
    failed: std::cell::Cell<bool>,
}

/// プレビューの欄（子ウィンドウ）と、その中の WebView2。
pub(crate) struct Preview {
    pub hwnd: HWND,
    shared: Rc<Shared>,
    started: bool,
}

impl Preview {
    /// プレビューの欄を作る（非表示）。WebView2 は最初に表示するときに作る。
    pub(crate) fn new(parent: HWND, frame: HWND) -> Result<Preview> {
        let hwnd = unsafe {
            let hinstance = GetWindowLongPtrW(parent, GWLP_HINSTANCE);
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PREVIEW_CLASS,
                None,
                WS_CHILD | WS_CLIPCHILDREN,
                0,
                0,
                0,
                0,
                Some(parent),
                None,
                Some(windows::Win32::Foundation::HINSTANCE(hinstance as *mut _)),
                None,
            )
            .context("CreateWindowExW(preview)")?
        };
        Ok(Preview {
            hwnd,
            shared: Rc::new(Shared {
                frame,
                container: hwnd,
                web: RefCell::new(None),
                page: RefCell::new(Page::default()),
                visible: std::cell::Cell::new(false),
                failed: std::cell::Cell::new(false),
            }),
            started: false,
        })
    }

    /// WebView2 を使えなかったか。
    pub(crate) fn is_unavailable(&self) -> bool {
        self.shared.failed.get()
    }

    /// 表示・非表示を切り替える。
    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.shared.visible.set(visible);
        unsafe {
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOW } else { SW_HIDE });
        }
        if visible && !self.started {
            self.started = true;
            set_notice(self.hwnd, "プレビューを準備しています…");
            if let Err(e) = start(self.shared.clone()) {
                fail(&self.shared, &e);
            }
        }
        if let Some(web) = self.web() {
            unsafe {
                let _ = web.controller.SetIsVisible(visible);
            }
        }
    }

    /// 欄の位置と大きさ（フレームのクライアント座標）。
    pub(crate) fn set_bounds(&self, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            let _ = MoveWindow(self.hwnd, x, y, w, h, true);
        }
        if let Some(web) = self.web() {
            unsafe {
                let _ = web.controller.SetBounds(RECT {
                    left: 0,
                    top: 0,
                    right: w,
                    bottom: h,
                });
            }
        }
    }

    fn web(&self) -> Option<Web> {
        self.shared.web.borrow().clone()
    }

    /// Markdown を表示する。同じ種類・同じフォルダのページを表示中なら本文だけを差し替える。
    pub(crate) fn show_markdown(
        &self,
        body: &str,
        opts: &PageOptions,
        folder: Option<DocBase>,
        line: Option<usize>,
    ) {
        let message = {
            let mut p = self.shared.page.borrow_mut();
            p.line = line;
            p.html = yy_preview::markdown_page(body, opts);
            let same = p.loaded == Some(Kind::Markdown) && p.folder == folder;
            p.kind = Some(Kind::Markdown);
            p.folder = folder;
            if same {
                // 読み直したときも最新の内容になるように、返した版も進める
                p.version += 1;
                p.served = p.version;
                Some(yy_preview::update_message(Some(body), line))
            } else {
                p.version += 1;
                None
            }
        };
        match message {
            Some(m) => self.post(&m),
            None => navigate(&self.shared),
        }
    }

    /// HTML 文書を表示する（ページを読み直す。スクロール位置はページ側で保つ）。
    pub(crate) fn show_html(&self, src: &str, opts: &PageOptions, folder: Option<DocBase>) {
        {
            let mut p = self.shared.page.borrow_mut();
            p.html = yy_preview::html_page(src, opts);
            p.kind = Some(Kind::Html);
            p.folder = folder;
            p.version += 1;
        }
        navigate(&self.shared);
    }

    /// 案内の文を表示する（プレビューできない文書など）。
    pub(crate) fn show_notice(&self, text: &str) {
        {
            let mut p = self.shared.page.borrow_mut();
            let html = format!(
                "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>body{{font-family:\
                 'Segoe UI','Yu Gothic UI',sans-serif;color:#59636e;display:flex;\
                 align-items:center;justify-content:center;height:90vh;margin:0 24px;\
                 text-align:center}}</style></head><body><p>{}</p></body></html>",
                yy_preview::escape(text)
            );
            if p.kind.is_none() && p.html == html {
                return;
            }
            p.html = html;
            p.kind = None;
            p.version += 1;
        }
        navigate(&self.shared);
    }

    /// エディタの先頭の行に合わせてスクロールする（Markdown のみ）。
    pub(crate) fn scroll_to_line(&self, line: usize) {
        let post = {
            let mut p = self.shared.page.borrow_mut();
            let changed = p.line != Some(line);
            p.line = Some(line);
            changed && p.loaded == Some(Kind::Markdown)
        };
        if post {
            self.post(&yy_preview::update_message(None, Some(line)));
        }
    }

    fn post(&self, json: &str) {
        if let Some(web) = self.web() {
            unsafe {
                let _ = web.webview.PostWebMessageAsJson(&HSTRING::from(json));
            }
        }
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        cancel_fetches(self.hwnd);
        if let Some(web) = self.shared.web.borrow_mut().take() {
            unsafe {
                let _ = web.controller.Close();
            }
        }
    }
}

/// 欄 `hwnd` に表示している案内。
fn notice_of(hwnd: HWND) -> String {
    NOTICE.with(|n| {
        n.borrow()
            .get(&(hwnd.0 as isize))
            .cloned()
            .unwrap_or_default()
    })
}

/// WebView2 を使えない: 案内を表示し、フレームに知らせる。
fn fail(shared: &Shared, reason: &str) {
    shared.failed.set(true);
    set_notice(shared.container, &unavailable_message(reason));
    unsafe {
        let _ = PostMessageW(
            Some(shared.frame),
            WM_APP_PREVIEW_FAILED,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

fn set_notice(hwnd: HWND, text: &str) {
    NOTICE.with(|n| {
        let mut n = n.borrow_mut();
        if text.is_empty() {
            n.remove(&(hwnd.0 as isize));
        } else {
            n.insert(hwnd.0 as isize, text.to_owned());
        }
    });
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

fn unavailable_message(e: &str) -> String {
    format!(
        "プレビューを表示できません。\n\n{e}\n\n\
         プレビューには Microsoft Edge WebView2 ランタイムが必要です\n\
         （Windows 10 / 11 には通常インストールされています）。"
    )
}

// ---- WebView2 の作成 ---------------------------------------------------------

/// WebView2 の環境とコントローラーを非同期に作る（できたら [`setup`] する）。
fn start(shared: Rc<Shared>) -> std::result::Result<(), String> {
    let create = create_environment_fn()?;
    let folder = user_data_folder();
    let _ = std::fs::create_dir_all(&folder);
    let s = shared.clone();
    let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |result: Result<()>, env: Option<ICoreWebView2Environment>| {
            let env = match (result, env) {
                (Ok(()), Some(env)) => env,
                (Err(e), _) => {
                    fail(&s, &e.message());
                    return Ok(());
                }
                (Ok(()), None) => return Ok(()),
            };
            let s2 = s.clone();
            let env2 = env.clone();
            let on_controller = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                move |result: Result<()>, controller: Option<ICoreWebView2Controller>| {
                    match (result, controller) {
                        (Ok(()), Some(controller)) => {
                            if let Err(e) = setup(&s2, env2, controller) {
                                fail(&s2, &e.message());
                            }
                        }
                        (Err(e), _) => fail(&s2, &e.message()),
                        (Ok(()), None) => {}
                    }
                    Ok(())
                },
            ));
            unsafe {
                if let Err(e) = env.CreateCoreWebView2Controller(s.container, &on_controller) {
                    fail(&s, &e.message());
                }
            }
            Ok(())
        },
    ));
    let folder = HSTRING::from(folder.as_os_str());
    let hr = unsafe {
        create(
            PCWSTR::null(),
            PCWSTR(folder.as_ptr()),
            std::ptr::null_mut(),
            handler.as_raw(),
        )
    };
    hr.ok().map_err(|e| e.message())
}

/// できた WebView2 を設定して、表示するページを読み込む。
fn setup(
    shared: &Rc<Shared>,
    env: ICoreWebView2Environment,
    controller: ICoreWebView2Controller,
) -> Result<()> {
    unsafe {
        let webview = controller.CoreWebView2()?;
        let settings = webview.Settings()?;
        let _ = settings.SetIsStatusBarEnabled(false);
        // F12 などはエディタのショートカットにする
        let _ = settings.SetAreDevToolsEnabled(false);
        let _ = settings.SetAreDefaultScriptDialogsEnabled(true);

        let filter = HSTRING::from(format!("https://{PREVIEW_HOST}/*"));
        webview.AddWebResourceRequestedFilter(&filter, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
        webview.AddWebResourceRequestedFilter(
            &HSTRING::from(format!("https://{DOC_HOST}/*")),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        )?;
        let mut token = 0i64;

        let s = shared.clone();
        let env2 = env;
        webview.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let _ = serve(&s, &env2, &args);
                }
                Ok(())
            })),
            &mut token,
        )?;

        let s = shared.clone();
        webview.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let uri = take_string(|p| args.Uri(p));
                if uri.starts_with(&format!("https://{PREVIEW_HOST}/")) {
                    cancel_fetches(s.container);
                    let mut id = 0u64;
                    let _ = args.NavigationId(&mut id);
                    s.page.borrow_mut().navigation = Some(id);
                } else {
                    args.SetCancel(true)?;
                    let mut user = windows::core::BOOL(0);
                    args.IsUserInitiated(&mut user)?;
                    open_link(&s, &uri, user.as_bool());
                }
                Ok(())
            })),
            &mut token,
        )?;

        let s = shared.clone();
        webview.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut id = 0u64;
                let _ = args.NavigationId(&mut id);
                let again = {
                    let mut p = s.page.borrow_mut();
                    if p.navigation != Some(id) {
                        return Ok(());
                    }
                    p.navigation = None;
                    let mut ok = windows::core::BOOL(0);
                    let _ = args.IsSuccess(&mut ok);
                    p.loaded = if ok.as_bool() { p.kind } else { None };
                    p.served != p.version
                };
                if again {
                    // 読み込み中に内容が変わった
                    navigate(&s);
                } else {
                    let line = s.page.borrow().line;
                    let loaded = s.page.borrow().loaded;
                    if let (Some(line), Some(Kind::Markdown)) = (line, loaded)
                        && let Some(web) = s.web.borrow().clone()
                    {
                        let _ = web.webview.PostWebMessageAsJson(&HSTRING::from(
                            yy_preview::update_message(None, Some(line)),
                        ));
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;

        let s = shared.clone();
        webview.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                args.SetHandled(true)?;
                let uri = take_string(|p| args.Uri(p));
                let mut user = windows::core::BOOL(0);
                args.IsUserInitiated(&mut user)?;
                open_link(&s, &uri, user.as_bool());
                Ok(())
            })),
            &mut token,
        )?;

        // プレビューにフォーカスがあってもエディタのショートカット（保存など）を使えるようにする
        let frame = shared.frame;
        controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                args.KeyEventKind(&mut kind)?;
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                    && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
                {
                    return Ok(());
                }
                let mut vk = 0u32;
                args.VirtualKey(&mut vk)?;
                if crate::app::translate_preview_shortcut(frame, vk) {
                    args.SetHandled(true)?;
                }
                Ok(())
            })),
            &mut token,
        )?;

        let _ = controller.SetIsVisible(shared.visible.get());
        let mut rc = RECT::default();
        let _ = GetClientRect(shared.container, &mut rc);
        let _ = controller.SetBounds(rc);

        *shared.web.borrow_mut() = Some(Web {
            controller,
            webview,
        });
    }
    set_notice(shared.container, "");
    navigate(shared);
    Ok(())
}

/// 表示するページへ移動する（文書のフォルダの割り当ても合わせる）。
fn navigate(shared: &Rc<Shared>) {
    cancel_fetches(shared.container);
    let Some(web) = shared.web.borrow().clone() else {
        return;
    };
    shared.page.borrow_mut().loaded = None;
    unsafe {
        let _ = web.webview.Navigate(&HSTRING::from(PAGE_URL));
    }
}

/// `https://yy-preview.local/` への要求に答える（ページと埋め込みファイル）。
fn serve(
    shared: &Rc<Shared>,
    env: &ICoreWebView2Environment,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
) -> Result<()> {
    let uri = unsafe { take_string(|p| args.Request()?.Uri(p)) };
    if let Some(rel) = uri.strip_prefix(&format!("https://{DOC_HOST}/")) {
        return serve_document_file(shared, env, args, rel);
    }
    let path = uri
        .strip_prefix(&format!("https://{PREVIEW_HOST}"))
        .unwrap_or("/")
        .split(['?', '#'])
        .next()
        .unwrap_or("/")
        .to_owned();
    let (status, body, mime): (i32, Vec<u8>, &str) = if path == "/index.html" || path == "/" {
        let mut p = shared.page.borrow_mut();
        p.served = p.version;
        (200, p.html.clone().into_bytes(), "text/html; charset=utf-8")
    } else if let Some((data, mime)) = yy_preview::asset(&path) {
        (200, data.into_owned(), mime)
    } else {
        (404, b"not found".to_vec(), "text/plain")
    };
    unsafe {
        let stream: Option<IStream> = SHCreateMemStream(Some(&body));
        let headers = HSTRING::from(format!(
            "Content-Type: {mime}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: https://yy-preview.local"
        ));
        let reason = if status == 200 {
            w!("OK")
        } else {
            w!("Not Found")
        };
        let response = env.CreateWebResourceResponse(stream.as_ref(), status, reason, &headers)?;
        args.SetResponse(&response)?;
    }
    Ok(())
}

/// 文書のフォルダの資源への要求。手元・リモートとも検証後にバックグラウンドで読み込む。
fn serve_document_file(
    shared: &Rc<Shared>,
    env: &ICoreWebView2Environment,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
    rel: &str,
) -> Result<()> {
    let permit = RequestPermit::acquire();
    #[cfg(test)]
    DOC_REQUESTS.with(|r| r.borrow_mut().push((rel.to_owned(), permit.is_none())));
    let Some(request) = permit else {
        return resource_response(env, args, 503, b"Too many preview requests", "text/plain");
    };
    let base = shared.page.borrow().folder.clone();
    let mime = mime_of(rel);
    let rel = rel.to_owned();
    let cancelled = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let deferral = unsafe { args.GetDeferral()? };
    let id = NEXT_FETCH.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    PENDING.with(|p| {
        p.borrow_mut().insert(
            id,
            PendingFetch {
                owner: shared.container.0 as isize,
                env: env.clone(),
                args: args.clone(),
                deferral,
                receiver: None,
                work: Some((base, rel)),
                _request: request,
                cancelled,
                started,
                mime,
            },
        )
    });
    if unsafe { SetTimer(Some(shared.container), FETCH_TIMER, 50, None) } == 0 {
        cancel_fetches(shared.container);
    }
    Ok(())
}

/// Resolve the resource under its document root before reading it.
fn remote_document_file(
    session: &yy_remote::Session,
    dir: &[u8],
    rel: &str,
) -> io::Result<Vec<u8>> {
    let root = session.real_path(dir)?;
    let path = session.real_path(&yy_proto::join_path(&root, rel.as_bytes()))?;
    if !yy_preview::security::remote_path_is_within(&root, &path) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "文書フォルダ外の資源は開けません",
        ));
    }
    Ok(path)
}

struct ResourceBuffer<'a> {
    data: &'a mut Vec<u8>,
    cancelled: &'a dyn Fn() -> bool,
}

impl Write for ResourceBuffer<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if (self.cancelled)() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "Preview cancelled"));
        }
        let len = self.data.len().saturating_add(bytes.len());
        if len as u64 > FETCH_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Preview resource too large",
            ));
        }
        if len > self.data.capacity() {
            let capacity = len.next_power_of_two().min(FETCH_LIMIT as usize);
            self.data
                .try_reserve_exact(capacity - self.data.len())
                .map_err(io::Error::other)?;
        }
        self.data.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn fetch_document_file(
    base: Option<DocBase>,
    raw: &str,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<Vec<u8>> {
    let check = || {
        if cancelled() {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Preview request cancelled",
            ))
        } else {
            Ok(())
        }
    };
    check()?;
    let rel = yy_preview::security::document_relative_path(raw).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "不正なプレビュー資源のパスです",
        )
    })?;
    let mut data = Vec::new();
    match base {
        Some(DocBase::Local(root)) => {
            let path = yy_preview::security::local_document_file(&root, &rel)?;
            let mut file = std::fs::File::open(path)?.take(FETCH_LIMIT + 1);
            let mut chunk = [0u8; 64 * 1024];
            loop {
                check()?;
                let n = file.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                ResourceBuffer {
                    data: &mut data,
                    cancelled,
                }
                .write_all(&chunk[..n])?;
            }
        }
        Some(DocBase::Remote { session, dir }) => {
            let path = remote_document_file(&session, &dir.path, &rel)?;
            session.download(
                &path,
                &mut ResourceBuffer {
                    data: &mut data,
                    cancelled,
                },
                &mut |done, total| !cancelled() && done <= FETCH_LIMIT && total <= FETCH_LIMIT,
            )?;
        }
        None => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "文書フォルダがありません",
            ));
        }
    }
    check()?;
    Ok(data)
}

fn resource_response(
    env: &ICoreWebView2Environment,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
    status: i32,
    body: &[u8],
    mime: &str,
) -> Result<()> {
    unsafe {
        let stream: Option<IStream> = SHCreateMemStream(Some(body));
        let headers = HSTRING::from(format!(
            "Content-Type: {mime}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: https://yy-preview.local"
        ));
        let reason = if status == 200 {
            w!("OK")
        } else {
            w!("Unavailable")
        };
        let response = env.CreateWebResourceResponse(stream.as_ref(), status, reason, &headers)?;
        args.SetResponse(&response)
    }
}

fn finish_fetch(p: PendingFetch, result: io::Result<Vec<u8>>) {
    p.cancelled.store(true, Ordering::Release);
    match result {
        Ok(body) => {
            let _ = resource_response(&p.env, &p.args, 200, &body, p.mime);
        }
        Err(_) => {
            let _ = resource_response(&p.env, &p.args, 404, b"Resource unavailable", "text/plain");
        }
    }
    unsafe {
        let _ = p.deferral.Complete();
    }
}

fn cancel_fetches(hwnd: HWND) {
    // Remove before calling COM, whose callbacks may reenter this map.
    let pending = PENDING.with(|p| {
        let mut map = p.borrow_mut();
        let ids: Vec<_> = map
            .iter()
            .filter(|(_, v)| v.owner == hwnd.0 as isize)
            .map(|(&id, _)| id)
            .collect();
        ids.into_iter()
            .filter_map(|id| map.remove(&id))
            .collect::<Vec<_>>()
    });
    unsafe {
        let _ = KillTimer(Some(hwnd), FETCH_TIMER);
    }
    for p in pending {
        finish_fetch(p, Err(io::Error::other("Preview closed or navigated")));
    }
}

fn start_fetch(p: &mut PendingFetch) {
    if p.work.is_none() {
        return;
    }
    let Some(permit) = FetchPermit::acquire() else {
        return;
    };
    let (base, rel) = p.work.take().unwrap();
    let cancelled = p.cancelled.clone();
    let started = p.started;
    let (sender, receiver) = mpsc::sync_channel(1);
    p.receiver = Some(receiver);
    // Keep the slot until the result is consumed. Timed-out I/O retains its slot
    // until it really exits, so stalled servers cannot cause replacement threads.
    let _ = std::thread::Builder::new()
        .name("preview-fetch".into())
        .spawn(move || {
            let stop = || cancelled.load(Ordering::Acquire) || started.elapsed() >= FETCH_TIMEOUT;
            let result = fetch_document_file(base, &rel, &stop);
            let _ = sender.send((result, permit));
        });
}

fn poll_fetches(hwnd: HWND) {
    let ready = PENDING.with(|p| {
        let mut map = p.borrow_mut();
        let mut ready = Vec::new();
        let ids: Vec<_> = map
            .iter()
            .filter(|(_, v)| v.owner == hwnd.0 as isize)
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            let p = map.get_mut(&id).unwrap();
            if p.started.elapsed() < FETCH_TIMEOUT {
                start_fetch(p);
            }
            let received = p
                .receiver
                .as_ref()
                .map_or(Err(mpsc::TryRecvError::Empty), |r| r.try_recv());
            let result = match received {
                Ok((body, permit)) => Some((body, Some(permit))),
                Err(mpsc::TryRecvError::Empty) if p.started.elapsed() < FETCH_TIMEOUT => None,
                Err(_) => Some((
                    Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Preview request expired",
                    )),
                    None,
                )),
            };
            if let Some((body, permit)) = result {
                ready.push((map.remove(&id).unwrap(), body, permit));
            }
        }
        ready
    });
    for (p, body, _permit) in ready {
        finish_fetch(p, body);
    }
    if !PENDING.with(|p| p.borrow().values().any(|p| p.owner == hwnd.0 as isize)) {
        unsafe {
            let _ = KillTimer(Some(hwnd), FETCH_TIMER);
        }
    }
}

/// 拡張子から Content-Type を決める。
fn mime_of(name: &str) -> &'static str {
    let ext = name
        .rsplit('/')
        .next()
        .and_then(|n| n.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "html" | "htm" => "text/html; charset=utf-8",
        "txt" | "md" | "csv" => "text/plain; charset=utf-8",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// ページの外へのリンク: 文書のフォルダの Markdown・HTML はエディタで、それ以外は既定の
/// アプリ（ブラウザなど）で開く。
fn open_link(shared: &Rc<Shared>, uri: &str, user_initiated: bool) {
    if !user_initiated {
        #[cfg(test)]
        BLOCKED_SCRIPT_LINKS.with(|count| count.set(count.get() + 1));
        return;
    }
    let doc_prefix = format!("https://{DOC_HOST}/");
    if let Some(rel) = uri.strip_prefix(&doc_prefix) {
        let Some(rel) = yy_preview::security::document_relative_path(rel) else {
            return;
        };
        let Some(base) = shared.page.borrow().folder.clone() else {
            return;
        };
        if rel.is_empty() {
            return;
        }
        let folder = match base {
            DocBase::Local(f) => f,
            DocBase::Remote { session, dir } => {
                // リモートの文書のフォルダの Markdown・HTML・テキストはエディタで開く
                if yy_preview::Kind::detect(None, Some(Path::new(&rel))).is_some()
                    || is_text_file(Path::new(&rel))
                {
                    let frame = shared.frame.0 as isize;
                    std::thread::spawn(move || {
                        if let Ok(path) = remote_document_file(&session, &dir.path, &rel) {
                            let uri = yy_remote::RemoteUri { path, ..dir };
                            post_open_frame(HWND(frame as *mut _), PathBuf::from(uri.to_string()));
                        }
                    });
                }
                return;
            }
        };
        let Ok(path) = yy_preview::security::local_document_file(&folder, &rel) else {
            return;
        };
        if yy_preview::Kind::detect(None, Some(&path)).is_some() || is_text_file(&path) {
            post_open(shared, path);
        } else if yy_preview::security::may_open_external_file(&path, user_initiated) {
            shell_open(&HSTRING::from(path.as_os_str()));
        }
        return;
    }
    let lower = uri.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")
    {
        shell_open(&HSTRING::from(uri));
    }
}

/// エディタで開くようフレームに頼む。
fn post_open(shared: &Rc<Shared>, path: PathBuf) {
    post_open_frame(shared.frame, path);
}

fn post_open_frame(frame: HWND, path: PathBuf) {
    let boxed = Box::into_raw(Box::new(path));
    unsafe {
        if PostMessageW(
            Some(frame),
            WM_APP_PREVIEW_OPEN,
            WPARAM(0),
            LPARAM(boxed as isize),
        )
        .is_err()
        {
            drop(Box::from_raw(boxed));
        }
    }
}

fn is_text_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("txt" | "csv" | "tsv" | "json" | "toml" | "yaml" | "yml" | "css" | "js")
    )
}

fn shell_open(target: &HSTRING) {
    #[cfg(test)]
    SHELL_OPEN_REQUESTS.with(|requests| requests.borrow_mut().push(target.to_string()));
    #[cfg(not(test))]
    unsafe {
        ShellExecuteW(None, w!("open"), target, None, None, SW_SHOWNORMAL);
    }
}

#[cfg(test)]
thread_local! {
    static SHELL_OPEN_REQUESTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static BLOCKED_SCRIPT_LINKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// 文書のフォルダへの要求（試験で、どの要求が上限で断られたかを見る）: (URL, 断ったか)
    static DOC_REQUESTS: RefCell<Vec<(String, bool)>> = const { RefCell::new(Vec::new()) };
}

/// WebView2 が返す文字列（CoTaskMemAlloc で確保）を受け取って解放する。
pub(crate) fn take_string(f: impl FnOnce(*mut PWSTR) -> Result<()>) -> String {
    let mut p = PWSTR::null();
    if f(&mut p).is_err() || p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string().unwrap_or_default() };
    unsafe { CoTaskMemFree(Some(p.0 as *const c_void)) };
    s
}

/// プレビューの欄のウィンドウプロシージャ（WebView2 を使えないときの案内を描く）。
pub(crate) extern "system" fn preview_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                FillRect(dc, &rc, GetSysColorBrush(COLOR_WINDOW));
                let text = notice_of(hwnd);
                if !text.is_empty() {
                    let font =
                        crate::util::ui_font(windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd));
                    let old = SelectObject(dc, font.into());
                    SetBkMode(dc, TRANSPARENT);
                    let mut wide: Vec<u16> = text.encode_utf16().collect();
                    let mut r = RECT {
                        left: rc.left + 24,
                        top: rc.top + rc.bottom / 3,
                        right: rc.right - 24,
                        bottom: rc.bottom,
                    };
                    DrawTextW(
                        dc,
                        &mut wide,
                        &mut r,
                        DT_CENTER | DT_WORDBREAK | DT_NOPREFIX,
                    );
                    SelectObject(dc, old);
                    let _ = windows::Win32::Graphics::Gdi::DeleteObject(font.into());
                }
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == FETCH_TIMER => {
                poll_fetches(hwnd);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                cancel_fetches(hwnd);
                set_notice(hwnd, "");
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_SIZE => {
                let _ = InvalidateRect(Some(hwnd), None, true);
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Ctrl・Shift が押されているか（WebView2 にフォーカスがあると GetKeyState は使えない）。
pub(crate) fn modifiers() -> (bool, bool) {
    unsafe {
        (
            GetAsyncKeyState(VK_CONTROL.0 as i32) < 0,
            GetAsyncKeyState(VK_SHIFT.0 as i32) < 0,
        )
    }
}

/// テスト用: メッセージの処理とページでのスクリプトの実行。
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::time::{Duration, Instant};
    use webview2_com::ExecuteScriptCompletedHandler;

    /// WebView2 を使う試験を 1 つずつ走らせる（並べて走らせると、ブラウザのプロセスが同時にいくつも立ち上がり、
    /// CI の小さなマシンでは時間切れになる）。
    pub(crate) fn webview2_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    impl Preview {
        /// 読み込み終わったページの種類。
        pub(crate) fn loaded_kind(&self) -> Option<Kind> {
            self.shared.page.borrow().loaded
        }
    }

    /// `done` が `true` を返すか時間切れになるまでメッセージを処理する。
    pub(crate) fn pump_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// ページでスクリプトを実行して結果（JSON）を返す。
    pub(crate) fn eval(preview: &Preview, script: &str) -> Option<String> {
        let web = preview.web()?;
        let result: Rc<RefCell<Option<String>>> = Rc::default();
        let r = result.clone();
        let handler = ExecuteScriptCompletedHandler::create(Box::new(move |hr, json| {
            *r.borrow_mut() = Some(if hr.is_ok() { json } else { String::new() });
            Ok(())
        }));
        unsafe {
            web.webview
                .ExecuteScript(&HSTRING::from(script), &handler)
                .ok()?;
        }
        pump_until(Duration::from_secs(10), || result.borrow().is_some());
        result.borrow_mut().take()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{eval, pump_until};
    use super::*;

    #[test]
    fn resource_reads_enforce_limit_and_cancellation() {
        let files = tempfile::tempdir().unwrap();
        let path = files.path().join("large.bin");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(FETCH_LIMIT + 1)
            .unwrap();
        let base = Some(DocBase::Local(files.path().to_owned()));
        assert_eq!(
            fetch_document_file(base.clone(), "large.bin", &|| false)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        let calls = Cell::new(0);
        assert_eq!(
            fetch_document_file(base, "large.bin", &|| {
                calls.set(calls.get() + 1);
                calls.get() >= 4
            })
            .unwrap_err()
            .kind(),
            io::ErrorKind::TimedOut
        );
        let mut data = vec![0; FETCH_LIMIT as usize];
        assert!(
            ResourceBuffer {
                data: &mut data,
                cancelled: &|| false
            }
            .write_all(b"x")
            .is_err()
        );
        assert_eq!(data.len(), FETCH_LIMIT as usize);
    }

    #[test]
    fn content_types_of_document_files() {
        assert_eq!(mime_of("img/a.PNG"), "image/png");
        assert_eq!(mime_of("x/y.svg"), "image/svg+xml");
        assert_eq!(mime_of("style.css"), "text/css; charset=utf-8");
        assert_eq!(mime_of("dir.v2/noext"), "application/octet-stream");
    }
    use std::cell::Cell;
    use std::time::Duration;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;

    /// WebView2 でページを表示し、mermaid・d3 の図と KaTeX の数式が描かれること、本文の差し替えが
    /// 反映されることを確かめる。WebView2 ランタイムがなければ飛ばす
    /// （`YY_REQUIRE_WEBVIEW2=1` なら失敗にする）。
    #[test]
    fn renders_markdown_with_mermaid_and_math_in_webview2() {
        let _serial = testing::webview2_lock();
        let required = std::env::var_os("YY_REQUIRE_WEBVIEW2").is_some();
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let instance = GetModuleHandleW(None).unwrap();
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(preview_proc),
                hInstance: instance.into(),
                lpszClassName: PREVIEW_CLASS,
                ..Default::default()
            };
            RegisterClassExW(&class);
        }
        let parent = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PREVIEW_CLASS,
                w!("preview test"),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                0,
                0,
                900,
                700,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        };
        let mut preview = Preview::new(parent, parent).unwrap();
        preview.set_visible(true);
        preview.set_bounds(0, 0, 880, 660);
        let md = "# Title\n\n```mermaid\ngraph TD\n  A-->B\n```\n\nInline $a^2$ and\n\n$$\\frac{1}{2}$$\n\n\
                  ```d3\nreturn d3.create('svg').attr('width', 100).attr('height', 50);\n```\n";
        let body = yy_preview::markdown_to_html(md, None);
        preview.show_markdown(&body, &PageOptions::default(), None, Some(0));

        let unavailable = || preview.is_unavailable();
        pump_until(Duration::from_secs(60), || {
            preview.shared.page.borrow().loaded == Some(Kind::Markdown) || unavailable()
        });
        let loaded = preview.shared.page.borrow().loaded == Some(Kind::Markdown);
        if !loaded {
            let notice = notice_of(preview.hwnd);
            if required {
                panic!("WebView2 のページを読み込めませんでした: {notice}");
            }
            eprintln!("WebView2 を使えないため飛ばします: {notice}");
            return;
        }

        // 図と数式が描かれるまで待つ
        let rendered = Cell::new(String::new());
        let ok = pump_until(Duration::from_secs(30), || {
            let r = eval(
                &preview,
                "(document.body.hasAttribute('data-d3-done') ? 'done' : 'wait') + \
                 ',' + document.querySelectorAll('pre.mermaid svg').length + \
                 ',' + document.querySelectorAll('.katex').length + \
                 ',' + document.querySelectorAll('.yy-d3-out svg').length",
            )
            .unwrap_or_default();
            rendered.set(r.clone());
            r == "\"done,1,2,1\""
        });
        assert!(ok, "rendered: {}", rendered.take());

        // 本文の差し替え（読み直さずに反映される）
        let body = yy_preview::markdown_to_html("# Updated\n\n$x$\n", None);
        preview.show_markdown(&body, &PageOptions::default(), None, Some(0));
        let ok = pump_until(Duration::from_secs(10), || {
            eval(&preview, "document.querySelector('h1').textContent").as_deref()
                == Some("\"Updated\"")
        });
        assert!(ok, "update was not applied");
        assert_eq!(preview.shared.page.borrow().loaded, Some(Kind::Markdown));

        // Verify real WebView2 routing with an inert OS-launch spy.
        let files = tempfile::tempdir().unwrap();
        let root = files.path().join("docs");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("inside.txt"), "inside").unwrap();
        std::fs::write(root.join("payload.cmd"), "inert test fixture").unwrap();
        std::fs::write(root.join("image.png"), "inert test fixture").unwrap();
        std::fs::write(files.path().join("secret.txt"), "outside-secret").unwrap();
        SHELL_OPEN_REQUESTS.with(|requests| requests.borrow_mut().clear());
        BLOCKED_SCRIPT_LINKS.with(|count| count.set(0));
        preview.show_html(
            r#"<html><head></head><body><script>
            (async () => { try {
                const good = await fetch('https://yy-doc.local/inside.txt');
                const text = await good.text();
                const bad = await fetch('https://yy-doc.local/%2e%2e%2fsecret.txt');
                document.body.dataset.security = text + ',' + bad.status;
                window.open('https://yy-doc.local/payload.cmd');
                location.href = 'https://yy-doc.local/payload.cmd';
            } catch (error) { document.body.dataset.security = String(error); } })();
            </script></body></html>"#,
            &PageOptions {
                has_folder: true,
                ..Default::default()
            },
            Some(DocBase::Local(root)),
        );
        let observed = RefCell::new(None);
        let completed = pump_until(Duration::from_secs(15), || {
            let result = eval(&preview, "document.body.dataset.security");
            let done = result.as_deref() == Some("\"inside,404\"")
                && BLOCKED_SCRIPT_LINKS.with(|count| count.get() >= 1);
            *observed.borrow_mut() = result;
            done
        });
        assert!(
            completed,
            "resource result: {:?}, blocked script links: {}",
            observed.borrow(),
            BLOCKED_SCRIPT_LINKS.with(|count| count.get())
        );
        open_link(&preview.shared, "https://yy-doc.local/payload.cmd", true);
        assert!(SHELL_OPEN_REQUESTS.with(|requests| requests.borrow().is_empty()));
        open_link(&preview.shared, "https://yy-doc.local/image.png", true);
        assert_eq!(
            SHELL_OPEN_REQUESTS.with(|requests| requests.borrow().len()),
            1
        );
        // Admission rejects excess requests without creating workers or deferrals.
        assert!(pump_until(Duration::from_secs(5), || ACTIVE_REQUESTS
            .load(Ordering::Acquire)
            == 0
            && ACTIVE_FETCHES.load(Ordering::Acquire) == 0));
        // 前の手順のスクリプトはページを移動させようとするので、新しいページで測る
        let folder = preview.shared.page.borrow().folder.clone();
        preview.show_html(
            "<html><body>overload</body></html>",
            &PageOptions {
                has_folder: true,
                ..Default::default()
            },
            folder,
        );
        assert!(pump_until(Duration::from_secs(10), || preview
            .loaded_kind()
            == Some(Kind::Html)
            && eval(&preview, "document.readyState").as_deref()
                == Some("\"complete\"")));
        let requests: Vec<_> = (0..MAX_REQUESTS)
            .map(|_| RequestPermit::acquire().unwrap())
            .collect();
        assert!(RequestPermit::acquire().is_none());
        eval(
            &preview,
            "fetch('https://yy-doc.local/inside.txt?overload').then(r => document.body.dataset.overload = String(r.status), e => document.body.dataset.overload = 'error: ' + e); 'started'",
        );
        let observed = RefCell::new(None);
        let overloaded = pump_until(Duration::from_secs(10), || {
            let r = eval(&preview, "document.body.dataset.overload");
            let done = r.as_deref() == Some("\"503\"");
            *observed.borrow_mut() = r;
            done
        });
        assert!(
            overloaded,
            "overload result: {:?}, active requests: {}, document requests: {:?}",
            observed.borrow(),
            ACTIVE_REQUESTS.load(Ordering::Acquire),
            DOC_REQUESTS.with(|r| r.borrow().clone())
        );
        drop(requests);

        eval(
            &preview,
            "Promise.all(Array.from({length: 8}, (_, i) => fetch('https://yy-doc.local/inside.txt?batch=' + i).then(r => r.text()))).then(xs => document.body.dataset.batch = xs.join(',')); 'started'",
        );
        assert!(pump_until(Duration::from_secs(5), || eval(
            &preview,
            "document.body.dataset.batch"
        )
        .as_deref()
            == Some(
                "\"inside,inside,inside,inside,inside,inside,inside,inside\""
            )));
        assert!(pump_until(Duration::from_secs(5), || ACTIVE_FETCHES
            .load(Ordering::Acquire)
            == 0));

        // Hold all worker slots to exercise real queued WebView2 requests deterministically.
        let permits: Vec<_> = (0..MAX_FETCHES)
            .map(|_| FetchPermit::acquire().unwrap())
            .collect();
        assert!(FetchPermit::acquire().is_none());
        let queue = |preview: &Preview| {
            eval(
                preview,
                "fetch('https://yy-doc.local/inside.txt?queued').catch(() => {}); 'started'",
            );
            assert!(pump_until(Duration::from_secs(5), || PENDING
                .with(|p| !p.borrow().is_empty())));
        };
        queue(&preview);
        PENDING.with(|p| {
            for p in p.borrow_mut().values_mut() {
                p.started = Instant::now() - FETCH_TIMEOUT;
            }
        });
        poll_fetches(preview.hwnd);
        assert!(PENDING.with(|p| p.borrow().is_empty()));
        queue(&preview);
        let folder = preview.shared.page.borrow().folder.clone();
        preview.show_html(
            "<html><body>next</body></html>",
            &PageOptions::default(),
            folder,
        );
        assert!(PENDING.with(|p| p.borrow().is_empty()));
        assert!(pump_until(Duration::from_secs(5), || preview.loaded_kind()
            == Some(Kind::Html)));
        queue(&preview);
        drop(preview);
        assert!(PENDING.with(|p| p.borrow().is_empty()));
        assert_eq!(ACTIVE_REQUESTS.load(Ordering::Acquire), 0);
        drop(permits);
        assert_eq!(ACTIVE_FETCHES.load(Ordering::Acquire), 0);
        unsafe {
            let _ = DestroyWindow(parent);
        }
    }
}

//! 広告ブロック（20 章）: フィルタの更新（別のスレッド。WinINet でプロファイルの経路から）と、
//! WebView2 の要求の照合・広告の枠を隠すスクリプトの差し込みに使う部品。
//!
//! 状態（エンジン・タブごとの件数）は `super::App` が持つ。ここの関数は、アプリの状態を借りたまま
//! WebView2 を呼ばないよう、要るものを引数で受け取る。

use std::path::PathBuf;
use std::sync::Arc;

use webview2_com::ExecuteScriptCompletedHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use windows::Win32::Foundation::{GetLastError, HWND, LPARAM, WPARAM};
use windows::Win32::Networking::WinInet::*;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{HSTRING, Interface, PCWSTR, w};
use yy_adblock::AdBlocker;
use yy_adblock::lists::{self, Cache, CacheMeta};
use yy_browser::rules::{Route, download_route};
use yy_browser::{FilterList, ProxyProfile};

/// 別のスレッドからの知らせ（`lparam` は `Box<AdMsg>`）。
pub(super) const WM_APP_ADBLOCK: u32 = WM_APP + 124;

/// 別のスレッドからの知らせ。
pub(super) enum AdMsg {
    /// エンジンができた
    Engine(Arc<AdBlocker>),
    /// 状態表示に出す
    Status(String),
    /// 更新が終わった（結果の説明）
    Done(String),
}

/// フィルタのキャッシュのフォルダ（`%LOCALAPPDATA%\yyeditor\yybrowser\filters`）。
pub(super) fn cache_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("yyeditor")
        .join("yybrowser")
        .join("filters")
}

fn post(frame: isize, m: AdMsg) {
    let p = Box::into_raw(Box::new(m));
    let ok = unsafe {
        PostMessageW(
            Some(HWND(frame as *mut _)),
            WM_APP_ADBLOCK,
            WPARAM(0),
            LPARAM(p as isize),
        )
    };
    if ok.is_err() {
        // ウィンドウがもうない
        drop(unsafe { Box::from_raw(p) });
    }
}

/// 有効なリストの本文（キャッシュ・ローカルのファイル）を集める: (名前, 本文) と、読めなかったもの。
fn load_sources(lists: &[FilterList], cache: &Cache) -> (Vec<(String, String)>, Vec<String>) {
    let mut v = Vec::new();
    let mut missing = Vec::new();
    for l in lists.iter().filter(|l| l.enabled) {
        let text = if l.is_local() {
            std::fs::read(lists::local_path(&l.url))
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned())
        } else {
            cache.text(&l.url)
        };
        match text {
            Some(t) => v.push((l.name.clone(), t)),
            None => missing.push(l.name.clone()),
        }
    }
    (v, missing)
}

/// フィルタを更新する（別のスレッド）。`initial` ならまずキャッシュからエンジンを作る。`force` なら期限に
/// 関係なく取り直す。終わったら [`AdMsg::Done`] を送る。
pub(super) fn spawn_update(
    frame: HWND,
    lists: Vec<FilterList>,
    profile: ProxyProfile,
    initial: bool,
    force: bool,
) {
    let frame = frame.0 as isize;
    std::thread::spawn(move || {
        let cache = Cache::new(cache_dir());
        let mut built = false;
        if initial {
            let (sources, _) = load_sources(&lists, &cache);
            post(frame, AdMsg::Engine(Arc::new(AdBlocker::build(sources))));
            built = true;
        }
        let now = lists::now();
        let mut changed = false;
        let mut errors = Vec::new();
        let mut notes = Vec::new();
        for l in lists.iter().filter(|l| l.enabled && !l.is_local()) {
            let due = force
                || cache.text(&l.url).is_none()
                || CacheMeta::is_due(cache.meta(&l.url).as_ref(), now);
            if !due {
                continue;
            }
            post(
                frame,
                AdMsg::Status(format!("広告ブロック: 「{}」を更新しています...", l.name)),
            );
            let route = download_route(&profile, &l.url);
            if let Route::Unsupported(why) = &route {
                notes.push(why.clone());
            }
            match download(&l.url, &route).and_then(|b| lists::validate_download(&b)) {
                Ok(text) => match cache.store(&l.url, &text, lists::now()) {
                    Ok(_) => changed = true,
                    Err(e) => errors.push(format!("{}: 保存できません: {e}", l.name)),
                },
                Err(e) => {
                    let _ = cache.fail(&l.url, &e, lists::now());
                    errors.push(format!("{}: {e}", l.name));
                }
            }
        }
        let (sources, missing) = load_sources(&lists, &cache);
        let count = sources.len();
        if changed || force || !built {
            post(frame, AdMsg::Engine(Arc::new(AdBlocker::build(sources))));
        }
        let mut msg = if changed {
            format!("広告ブロック: フィルタを更新しました（{count} 個のリスト）")
        } else {
            format!("広告ブロック: {count} 個のリストを使っています")
        };
        if !missing.is_empty() {
            msg.push_str(&format!("。まだないもの: {}", missing.join("・")));
        }
        if !errors.is_empty() {
            msg.push_str(&format!("。失敗: {}", errors.join("／")));
        }
        if !notes.is_empty() {
            msg.push_str(&format!("（{}）", notes.join("／")));
        }
        post(frame, AdMsg::Done(msg));
    });
}

/// WinINet のハンドル（閉じ忘れないように）。
struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = InternetCloseHandle(self.0);
            }
        }
    }
}

/// URL をダウンロードする（WinINet。キャッシュを使わない。30 秒で時間切れ。上限を超えたら打ち切る）。
fn download(url: &str, route: &Route) -> Result<Vec<u8>, String> {
    let last = || unsafe { GetLastError().0 };
    unsafe {
        let session = match route {
            Route::Direct => InternetOpenW(
                w!("yybrowser"),
                INTERNET_OPEN_TYPE_DIRECT.0,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ),
            Route::Proxy(p) => InternetOpenW(
                w!("yybrowser"),
                INTERNET_OPEN_TYPE_PROXY.0,
                &HSTRING::from(p.as_str()),
                w!("<local>"),
                0,
            ),
            Route::System | Route::Unsupported(_) => InternetOpenW(
                w!("yybrowser"),
                INTERNET_OPEN_TYPE_PRECONFIG.0,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ),
        };
        let session = Handle(session);
        if session.0.is_null() {
            return Err(format!("WinINet を使えません（{}）", last()));
        }
        let timeout: u32 = 30_000;
        for opt in [
            INTERNET_OPTION_CONNECT_TIMEOUT,
            INTERNET_OPTION_RECEIVE_TIMEOUT,
        ] {
            let _ = InternetSetOptionW(
                Some(session.0),
                opt,
                Some(&timeout as *const u32 as *const _),
                4,
            );
        }
        let req = Handle(InternetOpenUrlW(
            session.0,
            &HSTRING::from(url),
            None,
            INTERNET_FLAG_RELOAD | INTERNET_FLAG_NO_CACHE_WRITE | INTERNET_FLAG_NO_UI,
            None,
        ));
        if req.0.is_null() {
            return Err(format!("つなげません（WinINet {}）", last()));
        }
        if url.to_ascii_lowercase().starts_with("http") {
            let mut code = 0u32;
            let mut len = 4u32;
            if HttpQueryInfoW(
                req.0,
                HTTP_QUERY_STATUS_CODE | HTTP_QUERY_FLAG_NUMBER,
                Some(&mut code as *mut u32 as *mut _),
                &mut len,
                None,
            )
            .is_ok()
                && code != 200
            {
                return Err(format!("HTTP {code}"));
            }
        }
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut got = 0u32;
            InternetReadFile(
                req.0,
                buf.as_mut_ptr() as *mut _,
                buf.len() as u32,
                &mut got,
            )
            .map_err(|_| format!("読み込みが途中で切れました（WinINet {}）", last()))?;
            if got == 0 {
                break;
            }
            out.extend_from_slice(&buf[..got as usize]);
            if out.len() > lists::MAX_LIST_BYTES {
                return Err("大きすぎます".into());
            }
        }
        Ok(out)
    }
}

/// WebView2 の要求の種類 → adblock の種類の名前（document はトップでなければ sub_frame）。
pub(super) fn kind_name(ctx: COREWEBVIEW2_WEB_RESOURCE_CONTEXT) -> &'static str {
    match ctx {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => "image",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => "script",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => "stylesheet",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => "font",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => "media",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST
        | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH
        | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_EVENT_SOURCE => "xmlhttprequest",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET => "websocket",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING
        | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_CSP_VIOLATION_REPORT => "ping",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => "sub_frame",
        _ => "other",
    }
}

/// 要求を止めるか（トップのページの読み込みは止めない）。
pub(super) fn decide(
    blocker: &AdBlocker,
    page_url: &str,
    uri: &str,
    ctx: COREWEBVIEW2_WEB_RESOURCE_CONTEXT,
) -> bool {
    if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT && (uri == page_url || page_url.is_empty())
    {
        return false;
    }
    blocker.should_block(uri, page_url, kind_name(ctx))
}

/// 要求を止める（本文なしの 403 の応答を返す）。
pub(super) fn block(
    env: &ICoreWebView2Environment,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
) {
    unsafe {
        if let Ok(resp) =
            env.CreateWebResourceResponse(None, 403, w!("Blocked by yybrowser"), w!(""))
        {
            let _ = args.SetResponse(&resp);
        }
    }
}

/// 要求の照合を受ける・やめる（`ICoreWebView2_22` があれば iframe・Service Worker の要求も）。
pub(super) fn set_request_filter(webview: &ICoreWebView2, on: bool) {
    unsafe {
        if let Ok(w22) = webview.cast::<ICoreWebView2_22>() {
            if on {
                let _ = w22.AddWebResourceRequestedFilterWithRequestSourceKinds(
                    w!("*"),
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
                );
            } else {
                let _ = w22.RemoveWebResourceRequestedFilterWithRequestSourceKinds(
                    w!("*"),
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
                );
            }
        } else if on {
            let _ = webview
                .AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL);
        } else {
            let _ = webview
                .RemoveWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL);
        }
    }
}

/// スクリプトを実行する（結果は使わない）。
pub(super) fn run_script(webview: &ICoreWebView2, script: &str) {
    let handler = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
    unsafe {
        let _ = webview.ExecuteScript(&HSTRING::from(script), &handler);
    }
}

/// ページの読み込み（DOMContentLoaded）で: ホスト向けの CSS と、class・id を集めるスクリプトを差し込む。
/// タブに覚えておく非表示の情報を返す。
pub(super) fn on_dom_loaded(
    blocker: &AdBlocker,
    webview: &ICoreWebView2,
    page_url: &str,
) -> yy_adblock::PageCosmetic {
    let page = blocker.page_cosmetic(page_url);
    if let Some(s) = yy_adblock::engine::inject_css_script(&page.css) {
        run_script(webview, &s);
    }
    if !page.generichide {
        run_script(webview, yy_adblock::engine::COLLECTOR_SCRIPT);
    }
    page
}

/// ページからの class・id のメッセージで、汎用の規則の CSS を差し込む。
pub(super) fn on_message(
    blocker: &AdBlocker,
    page: &yy_adblock::PageCosmetic,
    webview: &ICoreWebView2,
    msg: &str,
) {
    let Some((classes, ids)) = yy_adblock::engine::parse_message(msg) else {
        return;
    };
    let css = blocker.generic_css(&classes, &ids, page);
    if let Some(s) = yy_adblock::engine::inject_css_script(&css) {
        run_script(webview, &s);
    }
}

/// フィルタリストの状態（一覧の画面に出す）。
pub(super) fn list_state(l: &FilterList) -> String {
    if l.is_local() {
        return match std::fs::read(lists::local_path(&l.url)) {
            Ok(b) => format!(
                "ローカル・規則 {}",
                crate::util::group_digits(lists::count_rules(&String::from_utf8_lossy(&b)) as u64)
            ),
            Err(_) => "ファイルを読めません".into(),
        };
    }
    let cache = Cache::new(cache_dir());
    match cache.meta(&l.url) {
        None => "まだ取得していません".into(),
        Some(m) => {
            let mut s = if m.fetched == 0 {
                "まだ取得していません".to_owned()
            } else {
                format!(
                    "規則 {}・{}に更新",
                    crate::util::group_digits(m.rules as u64),
                    ago(lists::now().saturating_sub(m.fetched))
                )
            };
            if !m.error.is_empty() {
                s.push_str(&format!("・失敗: {}", m.error));
            }
            s
        }
    }
}

/// 経った時間の説明（`3 時間前`）。
fn ago(secs: u64) -> String {
    match secs {
        0..60 => "今".into(),
        60..3600 => format!("{} 分前", secs / 60),
        3600..86400 => format!("{} 時間前", secs / 3600),
        _ => format!("{} 日前", secs / 86400),
    }
}

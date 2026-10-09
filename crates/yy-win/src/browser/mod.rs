//! タブブラウザ yybrowser の画面（19 章）。
//!
//! 1 つのウィンドウのタブは 1 つの WebView2 の環境（ブラウザのプロセス）を共有し、タブごとに 1 つの
//! コントローラーを持つ。環境はプロキシのプロファイル（`yy_browser::ProxyProfile`）の起動引数で作り、
//! データのフォルダもプロファイルごとに分ける（同じフォルダの環境は起動引数も同じでなければならないため）。
//!
//! WebView2 のイベントは `Navigate` などを呼んだその場で（同期的に）来ることがあるので、アプリの状態
//! （`APP`）を借りたまま WebView2 を呼ばない。必要なものを取り出してから呼ぶ。

mod adblock;
mod bookmarkui;
mod filterdlg;
mod proxydlg;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Interface, PCWSTR, PWSTR, Result, w};
use yy_browser::{ProfileList, ProxyProfile};
use yy_config::Config;

use crate::preview::take_string;
use crate::util::{Context, error_box, info_box};

const FRAME_CLASS: PCWSTR = w!("YYBrowserFrame");

// 部品の ID
const ID_TABS: u16 = 6000;
const ID_STATUS: u16 = 6001;
const ID_BACK: u16 = 6002;
const ID_FORWARD: u16 = 6003;
const ID_RELOAD: u16 = 6004;
const ID_HOME: u16 = 6005;
const ID_ADDRESS: u16 = 6006;
const ID_PROXY: u16 = 6007;
const ID_BADGE: u16 = 6008;
const ID_SHIELD: u16 = 6009;
const ID_STAR: u16 = 6015;
const ID_FIND_EDIT: u16 = 6010;
const ID_FIND_PREV: u16 = 6011;
const ID_FIND_NEXT: u16 = 6012;
const ID_FIND_CLOSE: u16 = 6013;
const ID_FIND_INFO: u16 = 6014;
// メニュー
const ID_NEW_TAB: u16 = 6100;
const ID_CLOSE_TAB: u16 = 6101;
const ID_REOPEN_TAB: u16 = 6102;
const ID_NEW_WINDOW: u16 = 6103;
const ID_PRINT: u16 = 6104;
const ID_EXIT: u16 = 6105;
const ID_FIND: u16 = 6106;
const ID_ZOOM_IN: u16 = 6110;
const ID_ZOOM_OUT: u16 = 6111;
const ID_ZOOM_RESET: u16 = 6112;
const ID_FULLSCREEN: u16 = 6113;
const ID_DEVTOOLS: u16 = 6114;
const ID_PROXY_SETTINGS: u16 = 6120;
const ID_HELP: u16 = 6130;
const ID_ABOUT: u16 = 6131;
const ID_SETTINGS: u16 = 6132;
const ID_AB_TOGGLE: u16 = 6140;
const ID_AB_SITE: u16 = 6141;
const ID_AB_UPDATE: u16 = 6142;
const ID_AB_LISTS: u16 = 6143;
const ID_BM_ADD: u16 = 6150;
const ID_BM_MANAGE: u16 = 6151;
/// メニューのブックマーク（`ID_BM_BASE + 番号`）
const ID_BM_BASE: u16 = 7000;
/// プロキシのプロファイルの切り替え（`ID_PROFILE_BASE + 番号`）
const ID_PROFILE_BASE: u16 = 6200;
/// 別のプロキシで新しいウィンドウ（`ID_WINDOW_BASE + 番号`）
const ID_WINDOW_BASE: u16 = 6300;

/// 環境ができた・タブの表示ができたなどの知らせ（`wparam` は世代。古い環境の知らせは捨てる）
const WM_APP_ENV_READY: u32 = WM_APP + 120;

/// 拡大の段階。
const ZOOMS: [f64; 13] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 3.0,
];

/// 1 つのタブ。
struct Tab {
    id: u64,
    controller: Option<ICoreWebView2Controller>,
    webview: Option<ICoreWebView2>,
    title: String,
    url: String,
    loading: bool,
    /// 表示ができる前に開くように頼まれた URL
    pending: Option<String>,
    /// 今のページで広告ブロックが止めた要求の数
    blocked: u32,
    /// 読み込み中・読み込んだトップのページの URL（NavigationStarting の URL。第三者の判断に使う）
    nav_url: String,
    /// 今のページの非表示の情報（汎用の規則を調べるときに使う）
    cosmetic: Option<yy_adblock::PageCosmetic>,
}

struct App {
    frame: HWND,
    tabs_hwnd: HWND,
    status: HWND,
    address: HWND,
    back: HWND,
    forward: HWND,
    reload: HWND,
    home: HWND,
    proxy_btn: HWND,
    /// 広告ブロックのボタン（🛡 件数）
    shield: HWND,
    /// ブックマークのボタン（☆・★）
    star: HWND,
    /// ブックマーク（★ の表示とメニュー。メニューを開くときに読み直す）
    bookmarks: yy_browser::bookmarks::Bookmarks,
    /// 広告ブロックのエンジン（できるまでは `None`）
    adblock: Option<Arc<yy_adblock::AdBlocker>>,
    /// フィルタを更新中
    adblock_busy: bool,
    /// アドレスバーの左の表示（開発者用証明書を利用中・転送中）。当てはまらなければ隠す
    badge: HWND,
    badge_visible: bool,
    find_bar: [HWND; 5],
    find_visible: bool,
    dpi: u32,
    config: Config,
    profiles: ProfileList,
    profile: ProxyProfile,
    env: Option<ICoreWebView2Environment>,
    /// 環境の世代（作り直すたびに増やす）
    generation: u64,
    tabs: Vec<Tab>,
    current: usize,
    next_id: u64,
    /// 閉じたタブの URL（Ctrl+Shift+T）
    closed: Vec<String>,
    /// 全画面の前の位置と形
    fullscreen: Option<(WINDOWPLACEMENT, i32)>,
    find: Option<ICoreWebView2Find>,
    /// 開発者用証明書（指紋が一致した）で証明書のエラーを許したホスト（環境を作り直すと空）
    dev_hosts: HashSet<String>,
}

/// 開発者用証明書を作る関数（rcgen を入れたビルドだけ。19 章 3.6）。
pub type DevCertFn = fn(&str) -> std::result::Result<yy_browser::DevCert, String>;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static DEV_CERT: Cell<Option<DevCertFn>> = const { Cell::new(None) };
    /// メニューバーの「広告ブロック」（開くときに中身を作る）
    static AB_MENU: Cell<isize> = const { Cell::new(0) };
    /// メニューバーの「ブックマーク」（開くときに中身を作る）
    static BM_MENU: Cell<isize> = const { Cell::new(0) };
}

/// 開発者用証明書を置くフォルダ（設定のフォルダの `browser-devcerts`）。
fn dev_cert_dir() -> PathBuf {
    yy_config::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("browser-devcerts")
}

fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

fn text_of(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..got])
    }
}

fn set_text(h: HWND, s: &str) {
    unsafe {
        let _ = SetWindowTextW(h, &HSTRING::from(s));
    }
}

/// プロファイルの一覧のファイル（設定のフォルダの `browser.toml`）。
fn profiles_path() -> PathBuf {
    yy_config::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("browser.toml")
}

/// プロファイルのデータのフォルダ（`%LOCALAPPDATA%\yyeditor\yybrowser\<名前>`）。
fn data_folder(p: &ProxyProfile) -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("yyeditor")
        .join("yybrowser")
        .join(p.data_folder_name())
}

/// yybrowser を起動する。`args`: `[--profile 名前] [--proxy URL|direct|system] [URL...]`。
/// `dev_cert` は開発者用証明書を作る関数（なければプロキシの設定で作れない）。
pub fn run_browser(args: Vec<String>, dev_cert: Option<DevCertFn>) -> Result<()> {
    DEV_CERT.with(|d| d.set(dev_cert));
    crate::util::set_app_name("yybrowser");
    crate::crash::install("yybrowser");
    let r = run_inner(args);
    crate::crash::clean_exit();
    if let Err(e) = &r {
        error_box(
            HWND::default(),
            &format!("起動できませんでした。\n{}", crate::util::describe_error(e)),
        );
    }
    r
}

/// コマンドラインを読む: (プロファイル, 開く URL)。
fn parse_args(
    args: &[String],
    profiles: &ProfileList,
) -> std::result::Result<(ProxyProfile, Vec<String>), String> {
    let mut profile = profiles.startup();
    let mut urls = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--profile" => {
                let name = it
                    .next()
                    .ok_or("--profile の後にプロファイルの名前がありません")?;
                profile = profiles
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("プロファイル「{name}」はありません"))?;
            }
            "--proxy" => {
                let v = it.next().ok_or("--proxy の後にプロキシがありません")?;
                profile = yy_browser::proxy::adhoc(v)?;
            }
            u => urls.push(u.to_owned()),
        }
    }
    Ok((profile, urls))
}

fn run_inner(args: Vec<String>) -> Result<()> {
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
    if let Ok(m) = unsafe { GetModuleHandleW(None) } {
        crate::help::use_app_help(m.into(), crate::help::BROWSER_MD, "yybrowser ヘルプ");
    }
    let (config, config_error) = Config::load();
    let (profiles, profiles_error) = match ProfileList::load(&profiles_path()) {
        Ok(p) => (p, None),
        Err(e) => (ProfileList::default(), Some(e.to_string())),
    };
    let (profile, urls) = match parse_args(&args, &profiles) {
        Ok(v) => v,
        Err(e) => {
            error_box(HWND::default(), &e);
            (profiles.startup(), Vec::new())
        }
    };
    let frame = create(config, profiles, profile)?;
    if let Some(e) = config_error {
        error_box(
            frame,
            &format!("設定を読めませんでした（既定の設定で起動します）。\n{e}"),
        );
    }
    if let Some(e) = profiles_error {
        error_box(
            frame,
            &format!(
                "プロキシのプロファイル（browser.toml）を読めませんでした（既定の一覧を使います）。\n{e}"
            ),
        );
    }
    let start: Vec<String> = if urls.is_empty() {
        vec![with(|a| a.config.browser.home.clone()).unwrap_or_default()]
    } else {
        urls
    };
    for u in start {
        new_tab(Some(&u));
    }
    start_environment();
    update_filters(true, false);
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
    // WebView2 を先に閉じる
    let tabs = with(|a| std::mem::take(&mut a.tabs)).unwrap_or_default();
    for t in tabs {
        if let Some(c) = t.controller {
            unsafe {
                let _ = c.Close();
            }
        }
    }
    APP.with(|a| a.borrow_mut().take());
    Ok(())
}

fn create_menu(profiles: &ProfileList, current: &str) -> Result<HMENU> {
    unsafe {
        let bar = CreateMenu()?;
        let add = |m: HMENU, id: u16, t: &str| {
            let _ = AppendMenuW(m, MF_STRING, id as usize, &HSTRING::from(t));
        };
        let file = CreatePopupMenu()?;
        add(file, ID_NEW_TAB, "新しいタブ(&T)\tCtrl+T");
        add(file, ID_NEW_WINDOW, "新しいウィンドウ(&N)\tCtrl+N");
        add(
            file,
            ID_REOPEN_TAB,
            "閉じたタブを開き直す(&R)\tCtrl+Shift+T",
        );
        add(file, ID_CLOSE_TAB, "タブを閉じる(&C)\tCtrl+W");
        let _ = AppendMenuW(file, MF_SEPARATOR, 0, None);
        add(file, ID_FIND, "ページ内を検索(&F)...\tCtrl+F");
        add(file, ID_PRINT, "印刷(&P)...\tCtrl+P");
        let _ = AppendMenuW(file, MF_SEPARATOR, 0, None);
        add(file, ID_SETTINGS, "設定ファイルを開く(&S)");
        add(file, ID_EXIT, "終了(&X)");
        let view = CreatePopupMenu()?;
        add(view, ID_ZOOM_IN, "拡大(&I)\tCtrl++");
        add(view, ID_ZOOM_OUT, "縮小(&O)\tCtrl+-");
        add(view, ID_ZOOM_RESET, "100%(&R)\tCtrl+0");
        let _ = AppendMenuW(view, MF_SEPARATOR, 0, None);
        add(view, ID_FULLSCREEN, "全画面(&F)\tF11");
        add(view, ID_DEVTOOLS, "開発者ツール(&D)\tF12");
        let proxy = CreatePopupMenu()?;
        fill_proxy_menu(proxy, profiles, current);
        let ab = CreatePopupMenu()?;
        AB_MENU.with(|c| c.set(ab.0 as isize));
        let bm = CreatePopupMenu()?;
        BM_MENU.with(|c| c.set(bm.0 as isize));
        let help = CreatePopupMenu()?;
        add(help, ID_HELP, "yybrowser ヘルプ(&H)\tF1");
        add(help, ID_ABOUT, "yybrowser について(&A)");
        for (m, t) in [
            (file, "ファイル(&F)"),
            (view, "表示(&V)"),
            (bm, "ブックマーク(&B)"),
            (proxy, "プロキシ(&P)"),
            (ab, "広告ブロック(&A)"),
            (help, "ヘルプ(&H)"),
        ] {
            AppendMenuW(bar, MF_POPUP, m.0 as usize, &HSTRING::from(t))?;
        }
        Ok(bar)
    }
}

/// プロキシのメニュー（プロファイルの切り替え・設定・別のプロキシで新しいウィンドウ）を作る。
fn fill_proxy_menu(m: HMENU, profiles: &ProfileList, current: &str) {
    unsafe {
        while GetMenuItemCount(Some(m)) > 0 {
            let _ = DeleteMenu(m, 0, MF_BYPOSITION);
        }
        for (i, p) in profiles.profiles.iter().enumerate().take(90) {
            let flags = if p.name == current {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            let label = format!("{}\t{}", p.name.replace('&', "&&"), p.mode.label());
            let _ = AppendMenuW(
                m,
                flags,
                (ID_PROFILE_BASE as usize) + i,
                &HSTRING::from(label),
            );
        }
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        if let Ok(sub) = CreatePopupMenu() {
            for (i, p) in profiles.profiles.iter().enumerate().take(90) {
                let _ = AppendMenuW(
                    sub,
                    MF_STRING,
                    (ID_WINDOW_BASE as usize) + i,
                    &HSTRING::from(p.name.replace('&', "&&")),
                );
            }
            let _ = AppendMenuW(
                m,
                MF_POPUP,
                sub.0 as usize,
                w!("別のプロキシで新しいウィンドウ(&W)"),
            );
        }
        let _ = AppendMenuW(
            m,
            MF_STRING,
            ID_PROXY_SETTINGS as usize,
            w!("プロキシの設定(&S)..."),
        );
    }
}

fn create(config: Config, profiles: ProfileList, profile: ProxyProfile) -> Result<HWND> {
    unsafe {
        let instance: windows::Win32::Foundation::HINSTANCE = GetModuleHandleW(None)?.into();
        let (icon, icon_small) = crate::app_icons(instance);
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(frame_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
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
        let frame = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            FRAME_CLASS,
            w!("yybrowser"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1200,
            850,
            None,
            Some(create_menu(&profiles, &profile.name)?),
            Some(instance),
            None,
        )
        .context("CreateWindowExW(frame)")?;
        let dpi = GetDpiForWindow(frame).max(96);
        let font = crate::util::ui_font(dpi);
        let child =
            |class: PCWSTR, text: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, id: u16| {
                let h = CreateWindowExW(
                    ex,
                    class,
                    &HSTRING::from(text),
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
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
                h
            };
        let tabs_hwnd = child(
            WC_TABCONTROLW,
            "",
            WS_CLIPSIBLINGS | WINDOW_STYLE(TCS_FOCUSNEVER),
            WINDOW_EX_STYLE(0),
            ID_TABS,
        );
        crate::tabclose::install(tabs_hwnd, frame);
        let button = |t: &str, id: u16| {
            child(
                w!("BUTTON"),
                t,
                WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
                WINDOW_EX_STYLE(0),
                id,
            )
        };
        let back = button("←", ID_BACK);
        let forward = button("→", ID_FORWARD);
        let reload = button("⟳", ID_RELOAD);
        let home = button("⌂", ID_HOME);
        let address = child(
            w!("EDIT"),
            "",
            WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            WS_EX_CLIENTEDGE,
            ID_ADDRESS,
        );
        let proxy_btn = button("", ID_PROXY);
        let badge = button("", ID_BADGE);
        let shield = button("🛡", ID_SHIELD);
        let star = button("☆", ID_STAR);
        let _ = ShowWindow(badge, SW_HIDE);
        let status = child(
            STATUSCLASSNAMEW,
            "",
            WINDOW_STYLE(SBARS_SIZEGRIP),
            WINDOW_EX_STYLE(0),
            ID_STATUS,
        );
        // ページ内検索の欄（初めは隠す）
        let find_edit = child(
            w!("EDIT"),
            "",
            WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            WS_EX_CLIENTEDGE,
            ID_FIND_EDIT,
        );
        let find_prev = button("↑", ID_FIND_PREV);
        let find_next = button("↓", ID_FIND_NEXT);
        let find_info = child(
            w!("STATIC"),
            "",
            WINDOW_STYLE(0),
            WINDOW_EX_STYLE(0),
            ID_FIND_INFO,
        );
        let find_close = button("×", ID_FIND_CLOSE);
        let find_bar = [find_edit, find_prev, find_next, find_info, find_close];
        for h in find_bar {
            let _ = ShowWindow(h, SW_HIDE);
        }
        let cue = HSTRING::from("ページ内を検索（Enter で次・Shift+Enter で前・Esc で閉じる）");
        SendMessageW(
            find_edit,
            EM_SETCUEBANNER,
            Some(WPARAM(1)),
            Some(LPARAM(cue.as_ptr() as isize)),
        );
        let cue = HSTRING::from("URL を入力するか検索");
        SendMessageW(
            address,
            EM_SETCUEBANNER,
            Some(WPARAM(1)),
            Some(LPARAM(cue.as_ptr() as isize)),
        );
        let app = App {
            frame,
            tabs_hwnd,
            status,
            address,
            back,
            forward,
            reload,
            home,
            proxy_btn,
            shield,
            star,
            bookmarks: bookmarkui::load(),
            adblock: None,
            adblock_busy: false,
            badge,
            badge_visible: false,
            find_bar,
            find_visible: false,
            dpi,
            config,
            profiles,
            profile,
            env: None,
            generation: 0,
            tabs: Vec::new(),
            current: 0,
            next_id: 1,
            closed: Vec::new(),
            fullscreen: None,
            find: None,
            dev_hosts: HashSet::new(),
        };
        APP.with(|a| *a.borrow_mut() = Some(app));
        with(|a| {
            a.update_proxy_label();
            a.layout();
        });
        let _ = ShowWindow(frame, SW_SHOWDEFAULT);
        let _ = windows::Win32::Graphics::Gdi::UpdateWindow(frame);
        Ok(frame)
    }
}

impl App {
    fn scaled(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }

    /// 部品を並べ、表示中のタブの WebView2 の大きさを合わせる。
    fn layout(&self) {
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(self.frame, &mut rc);
            let (w, h) = (rc.right, rc.bottom);
            if self.fullscreen.is_some() {
                for hw in [
                    self.tabs_hwnd,
                    self.back,
                    self.forward,
                    self.reload,
                    self.home,
                    self.address,
                    self.proxy_btn,
                    self.shield,
                    self.star,
                    self.badge,
                    self.status,
                ] {
                    let _ = ShowWindow(hw, SW_HIDE);
                }
                for hw in self.find_bar {
                    let _ = ShowWindow(hw, SW_HIDE);
                }
                self.set_web_bounds(RECT {
                    left: 0,
                    top: 0,
                    right: w,
                    bottom: h,
                });
                return;
            }
            for hw in [
                self.tabs_hwnd,
                self.back,
                self.forward,
                self.reload,
                self.home,
                self.address,
                self.proxy_btn,
                self.shield,
                self.star,
                self.status,
            ] {
                let _ = ShowWindow(hw, SW_SHOW);
            }
            SendMessageW(self.status, WM_SIZE, None, None);
            let mut sr = RECT::default();
            let _ = GetWindowRect(self.status, &mut sr);
            let status_h = sr.bottom - sr.top;
            let tab_h = self.scaled(26);
            let bar_h = self.scaled(32);
            let pad = self.scaled(4);
            let _ = MoveWindow(self.tabs_hwnd, 0, 0, w, tab_h, true);
            let btn = self.scaled(32);
            let mut x = pad;
            let y = tab_h + pad;
            let ch = bar_h - pad;
            for b in [self.back, self.forward, self.reload, self.home] {
                let _ = MoveWindow(b, x, y, btn, ch, true);
                x += btn + pad;
            }
            if self.badge_visible {
                let text = text_of(self.badge);
                let bw = self
                    .scaled(24 + 12 * text.chars().count() as i32)
                    .min(self.scaled(320));
                let _ = MoveWindow(self.badge, x, y, bw, ch, true);
                let _ = ShowWindow(self.badge, SW_SHOW);
                x += bw + pad;
            } else {
                let _ = ShowWindow(self.badge, SW_HIDE);
            }
            let proxy_w = self.scaled(240);
            let shield_w = self.scaled(72);
            let addr_w = (w - x - proxy_w - shield_w - btn - 4 * pad).max(self.scaled(80));
            let _ = MoveWindow(self.address, x, y, addr_w, ch, true);
            let _ = MoveWindow(self.star, x + addr_w + pad, y, btn, ch, true);
            let _ = MoveWindow(
                self.shield,
                x + addr_w + btn + 2 * pad,
                y,
                shield_w,
                ch,
                true,
            );
            let _ = MoveWindow(
                self.proxy_btn,
                x + addr_w + btn + shield_w + 3 * pad,
                y,
                proxy_w,
                ch,
                true,
            );
            let top = tab_h + bar_h + pad;
            let mut bottom = h - status_h;
            if self.find_visible {
                let fh = self.scaled(30);
                bottom -= fh;
                let fy = bottom + self.scaled(3);
                let fch = fh - self.scaled(6);
                let widths = [self.scaled(360), btn, btn, self.scaled(140), btn];
                let mut fx = pad;
                for (hw, wd) in self.find_bar.iter().zip(widths) {
                    let _ = MoveWindow(*hw, fx, fy, wd, fch, true);
                    let _ = ShowWindow(*hw, SW_SHOW);
                    fx += wd + pad;
                }
            } else {
                for hw in self.find_bar {
                    let _ = ShowWindow(hw, SW_HIDE);
                }
            }
            self.set_web_bounds(RECT {
                left: 0,
                top,
                right: w,
                bottom: bottom.max(top),
            });
        }
    }

    fn set_web_bounds(&self, rc: RECT) {
        for (i, t) in self.tabs.iter().enumerate() {
            if let Some(c) = &t.controller {
                unsafe {
                    if i == self.current {
                        let _ = c.SetBounds(rc);
                    }
                    let _ = c.SetIsVisible(i == self.current);
                }
            }
        }
    }

    fn update_proxy_label(&self) {
        set_text(
            self.proxy_btn,
            &format!("プロキシ: {} ▾", self.profile.name),
        );
        let args = self.profile.browser_args().unwrap_or_default();
        let msg = format!(
            "プロキシ: {}{}",
            self.profile.describe(),
            if args.is_empty() {
                String::new()
            } else {
                format!("（{args}）")
            }
        );
        self.set_status(&msg);
    }

    fn set_status(&self, s: &str) {
        unsafe {
            SendMessageW(
                self.status,
                SB_SETTEXTW,
                Some(WPARAM(0)),
                Some(LPARAM(HSTRING::from(s).as_ptr() as isize)),
            );
        }
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// タブの見出し（閉じるボタンの場所を空けるため後ろに空白を足す）。
    fn tab_label(t: &Tab) -> String {
        let mut title = if t.title.is_empty() {
            if t.url.is_empty() {
                "新しいタブ".to_owned()
            } else {
                t.url.clone()
            }
        } else {
            t.title.clone()
        };
        if title.chars().count() > 28 {
            title = title.chars().take(27).collect::<String>() + "…";
        }
        format!(
            "{}{}{}",
            if t.loading { "⟳ " } else { "" },
            title,
            crate::tabclose::LABEL_PAD
        )
    }

    fn refresh_tab_label(&self, i: usize) {
        let Some(t) = self.tabs.get(i) else { return };
        let s = crate::util::wide(&Self::tab_label(t));
        let item = TCITEMW {
            mask: TCIF_TEXT,
            pszText: PWSTR(s.as_ptr() as *mut _),
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                self.tabs_hwnd,
                TCM_SETITEMW,
                Some(WPARAM(i)),
                Some(LPARAM(&item as *const _ as isize)),
            );
        }
        if i == self.current {
            self.refresh_chrome();
        }
    }

    /// 表示中のタブに合わせて、アドレスバー・ボタン・タイトルを直す。
    fn refresh_chrome(&self) {
        let Some(t) = self.tabs.get(self.current) else {
            return;
        };
        // アドレスバーで入力中なら上書きしない
        let focused = unsafe { GetFocus() } == self.address;
        if !focused {
            set_text(self.address, &t.url);
        }
        let badge = self.badge_text(&t.url);
        self.refresh_shield();
        let star = if self.bookmarks.find(&t.url).is_some() {
            "★"
        } else {
            "☆"
        };
        if text_of(self.star) != star {
            set_text(self.star, star);
        }
        let title = if t.title.is_empty() {
            "yybrowser".to_owned()
        } else {
            format!("{} - yybrowser", t.title)
        };
        set_text(self.frame, &format!("{title} [{}]", self.profile.name));
        set_text(self.reload, if t.loading { "×" } else { "⟳" });
        let (mut back, mut fwd) = (windows::core::BOOL(0), windows::core::BOOL(0));
        if let Some(w) = &t.webview {
            unsafe {
                let _ = w.CanGoBack(&mut back);
                let _ = w.CanGoForward(&mut fwd);
            }
        }
        unsafe {
            let _ = EnableWindow(self.back, back.as_bool());
            let _ = EnableWindow(self.forward, fwd.as_bool());
        }
        let visible = badge.is_some();
        let text = badge.unwrap_or_default();
        if visible != self.badge_visible || text != text_of(self.badge) {
            set_text(self.badge, &text);
            // layout は &self なので、表示の有無は後で書き戻す（frame_proc で）
            unsafe {
                let _ = PostMessageW(
                    Some(self.frame),
                    WM_APP_BADGE,
                    WPARAM(visible as usize),
                    LPARAM(0),
                );
            }
        }
    }

    /// 広告ブロックのボタンの表示（切・止めないサイト・準備中・止めた数）。
    fn refresh_shield(&self) {
        let Some(t) = self.tabs.get(self.current) else {
            return;
        };
        let host = yy_browser::rules::url_host_port(&t.url)
            .map(|(_, h, _)| h)
            .unwrap_or_default();
        let text = if self.profile.adblock_off {
            "🛡 切".to_owned()
        } else if self.profile.adblock_allowed_site(&host) {
            "🛡 除外".to_owned()
        } else if self.adblock.is_none() {
            "🛡 …".to_owned()
        } else {
            format!("🛡 {}", t.blocked)
        };
        if text_of(self.shield) != text {
            set_text(self.shield, &text);
        }
    }

    /// アドレスバーの左に出す表示（`None` なら出さない）。
    ///
    /// * 開発者用証明書（指紋が一致）で証明書のエラーを許したホスト → 「🔒 開発者用証明書を利用中」
    /// * 転送するホスト → 「転送 → 127.0.0.1:8443」
    fn badge_text(&self, url: &str) -> Option<String> {
        let (scheme, host, port) = yy_browser::rules::url_host_port(url)?;
        if scheme == "https" && self.dev_hosts.contains(&host) {
            return Some("🔒 開発者用証明書を利用中".into());
        }
        let m = self.profile.host_map(&host, port)?;
        Some(format!("転送 → {}", m.address.trim()))
    }

    fn current_web(&self) -> Option<(ICoreWebView2, ICoreWebView2Controller)> {
        let t = self.tabs.get(self.current)?;
        Some((t.webview.clone()?, t.controller.clone()?))
    }
}

/// 表示中のタブの WebView2（借用を外してから使う）。
fn current_web() -> Option<(ICoreWebView2, ICoreWebView2Controller)> {
    with(|a| a.current_web()).flatten()
}

// ---- 環境 ------------------------------------------------------------------------------

/// 今のプロファイルで WebView2 の環境を作る（作り直すときも）。できたらタブの表示を作る。
fn start_environment() {
    let Some((profile, generation, frame)) = with(|a| {
        a.generation += 1;
        a.env = None;
        a.dev_hosts.clear();
        (a.profile.clone(), a.generation, a.frame)
    }) else {
        return;
    };
    let args = match profile.browser_args() {
        Ok(a) => a,
        Err(e) => {
            error_box(frame, &format!("プロキシの設定が正しくありません: {e}"));
            String::new()
        }
    };
    let folder = data_folder(&profile);
    let _ = std::fs::create_dir_all(&folder);
    let create = match crate::preview::create_environment_fn() {
        Ok(c) => c,
        Err(e) => {
            env_failed(&e);
            return;
        }
    };
    let options = CoreWebView2EnvironmentOptions::default();
    unsafe {
        options.set_additional_browser_arguments(args);
    }
    let options: ICoreWebView2EnvironmentOptions = options.into();
    let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |result: Result<()>, env: Option<ICoreWebView2Environment>| {
            match (result, env) {
                (Ok(()), Some(env)) => {
                    let ok = with(|a| {
                        if a.generation != generation {
                            return false;
                        }
                        a.env = Some(env);
                        true
                    })
                    .unwrap_or(false);
                    if ok {
                        unsafe {
                            let _ = PostMessageW(
                                Some(frame),
                                WM_APP_ENV_READY,
                                WPARAM(generation as usize),
                                LPARAM(0),
                            );
                        }
                    }
                }
                (Err(e), _) => env_failed(&e.message()),
                (Ok(()), None) => {}
            }
            Ok(())
        },
    ));
    let folder_w = HSTRING::from(folder.as_os_str());
    let hr = unsafe {
        create(
            PCWSTR::null(),
            PCWSTR(folder_w.as_ptr()),
            options.as_raw(),
            handler.as_raw(),
        )
    };
    if let Err(e) = hr.ok() {
        env_failed(&e.message());
    }
}

fn env_failed(e: &str) {
    let Some(frame) = with(|a| a.frame) else {
        return;
    };
    error_box(
        frame,
        &format!(
            "ブラウザ（WebView2）を起動できませんでした。\n{e}\n\n\
             ・WebView2 ランタイムが入っているか確かめてください。\n\
             ・同じプロキシのプロファイルの yybrowser をほかに開いていて、その後でプロファイルを書き換えた\
             ときは、ほかのウィンドウを閉じてからやり直してください。"
        ),
    );
}

/// 環境ができたので、表示のないタブに表示を作る。
fn on_env_ready(generation: u64) {
    let Some(work) = with(|a| {
        if a.generation != generation {
            return None;
        }
        let env = a.env.clone()?;
        let ids: Vec<u64> = a
            .tabs
            .iter()
            .filter(|t| t.controller.is_none())
            .map(|t| t.id)
            .collect();
        Some((env, ids, a.frame))
    })
    .flatten() else {
        return;
    };
    let (env, ids, frame) = work;
    for id in ids {
        create_controller(&env, frame, id, generation);
    }
}

fn create_controller(env: &ICoreWebView2Environment, frame: HWND, id: u64, generation: u64) {
    let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
        move |result: Result<()>, controller: Option<ICoreWebView2Controller>| {
            match (result, controller) {
                (Ok(()), Some(c)) => attach(id, generation, c),
                (Err(e), _) => {
                    with(|a| a.set_status(&format!("タブを開けませんでした: {}", e.message())));
                }
                (Ok(()), None) => {}
            }
            Ok(())
        },
    ));
    unsafe {
        if let Err(e) = env.CreateCoreWebView2Controller(frame, &handler) {
            with(|a| a.set_status(&format!("タブを開けませんでした: {}", e.message())));
        }
    }
}

/// できた表示をタブに付け、イベントを受け、頼まれていた URL を開く。
fn attach(id: u64, generation: u64, controller: ICoreWebView2Controller) {
    let webview = match unsafe { controller.CoreWebView2() } {
        Ok(w) => w,
        Err(_) => return,
    };
    let devtools = with(|a| a.config.browser.devtools).unwrap_or(true);
    // 環境が作り直された・タブが閉じられた
    let pending = with(|a| {
        if a.generation != generation {
            return None;
        }
        let i = a.index_of(id)?;
        a.tabs[i].controller = Some(controller.clone());
        a.tabs[i].webview = Some(webview.clone());
        Some(a.tabs[i].pending.take())
    })
    .flatten();
    let Some(pending) = pending else {
        unsafe {
            let _ = controller.Close();
        }
        return;
    };
    unsafe {
        if let Ok(s) = webview.Settings() {
            let _ = s.SetAreDevToolsEnabled(devtools);
            let _ = s.SetIsStatusBarEnabled(true);
            let _ = s.SetAreDefaultContextMenusEnabled(true);
        }
    }
    if let Err(e) = add_events(id, &webview, &controller) {
        with(|a| a.set_status(&format!("イベントを受けられません: {}", e.message())));
    }
    with(|a| a.layout());
    let url =
        pending.unwrap_or_else(|| with(|a| a.config.browser.home.clone()).unwrap_or_default());
    navigate_web(&webview, &url);
    let is_current = with(|a| a.index_of(id) == Some(a.current)).unwrap_or(false);
    if is_current {
        focus_page_or_address(&url);
    }
}

fn navigate_web(webview: &ICoreWebView2, url: &str) {
    let url = if url.trim().is_empty() {
        "about:blank"
    } else {
        url
    };
    unsafe {
        if let Err(e) = webview.Navigate(&HSTRING::from(url)) {
            with(|a| a.set_status(&format!("開けません: {url}: {}", e.message())));
        }
    }
}

/// 空のページならアドレスバーへ、そうでなければページへフォーカスを移す。
fn focus_page_or_address(url: &str) {
    if url.is_empty() || url == "about:blank" {
        if let Some(h) = with(|a| a.address) {
            unsafe {
                let _ = SetFocus(Some(h));
            }
        }
    } else if let Some((_, c)) = current_web() {
        unsafe {
            let _ = c.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
        }
    }
}

fn add_events(
    id: u64,
    webview: &ICoreWebView2,
    controller: &ICoreWebView2Controller,
) -> Result<()> {
    let mut token = 0i64;
    unsafe {
        webview.add_DocumentTitleChanged(
            &DocumentTitleChangedEventHandler::create(Box::new(move |sender, _| {
                if let Some(w) = sender {
                    let title = take_string(|p| w.DocumentTitle(p));
                    with(|a| {
                        if let Some(i) = a.index_of(id) {
                            a.tabs[i].title = title;
                            a.refresh_tab_label(i);
                        }
                    });
                }
                Ok(())
            })),
            &mut token,
        )?;
        webview.add_SourceChanged(
            &SourceChangedEventHandler::create(Box::new(move |sender, _| {
                if let Some(w) = sender {
                    let url = take_string(|p| w.Source(p));
                    with(|a| {
                        if let Some(i) = a.index_of(id) {
                            a.tabs[i].url = url;
                            a.refresh_tab_label(i);
                        }
                    });
                }
                Ok(())
            })),
            &mut token,
        )?;
        webview.add_HistoryChanged(
            &HistoryChangedEventHandler::create(Box::new(move |_, _| {
                with(|a| {
                    if a.index_of(id) == Some(a.current) {
                        a.refresh_chrome();
                    }
                });
                Ok(())
            })),
            &mut token,
        )?;
        webview.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let uri = args.map(|a| take_string(|p| a.Uri(p))).unwrap_or_default();
                with(|a| {
                    if let Some(i) = a.index_of(id) {
                        a.tabs[i].loading = true;
                        a.tabs[i].nav_url = uri;
                        a.tabs[i].blocked = 0;
                        a.tabs[i].cosmetic = None;
                        a.refresh_tab_label(i);
                    }
                });
                Ok(())
            })),
            &mut token,
        )?;
        webview.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                let mut ok = windows::core::BOOL(1);
                let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                if let Some(args) = &args {
                    let _ = args.IsSuccess(&mut ok);
                    let _ = args.WebErrorStatus(&mut status);
                }
                with(|a| {
                    if let Some(i) = a.index_of(id) {
                        a.tabs[i].loading = false;
                        a.refresh_tab_label(i);
                        if i == a.current {
                            if ok.as_bool() {
                                a.update_proxy_label();
                            } else {
                                a.set_status(&format!(
                                    "読み込めませんでした（{}）。プロキシ: {}",
                                    web_error(status),
                                    a.profile.describe()
                                ));
                            }
                        }
                    }
                });
                Ok(())
            })),
            &mut token,
        )?;
        // 新しいウィンドウ（target=_blank・window.open）は新しいタブで開く
        webview.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                args.SetHandled(true)?;
                let uri = take_string(|p| args.Uri(p));
                // 呼ばれている途中なので、開くのは後で
                if let Some(frame) = with(|a| a.frame) {
                    let s = Box::into_raw(Box::new(uri));
                    let _ =
                        PostMessageW(Some(frame), WM_APP_OPEN_TAB, WPARAM(0), LPARAM(s as isize));
                }
                Ok(())
            })),
            &mut token,
        )?;
        webview.add_ContainsFullScreenElementChanged(
            &ContainsFullScreenElementChangedEventHandler::create(Box::new(move |sender, _| {
                if let Some(w) = sender {
                    let mut full = windows::core::BOOL(0);
                    let _ = w.ContainsFullScreenElement(&mut full);
                    set_fullscreen(full.as_bool());
                }
                Ok(())
            })),
            &mut token,
        )?;
        webview.add_ProcessFailed(
            &ProcessFailedEventHandler::create(Box::new(move |_, _| {
                with(|a| {
                    a.set_status("ページのプロセスが止まりました。再読み込み（F5）してください");
                });
                Ok(())
            })),
            &mut token,
        )?;
        // Basic 認証（サーバー・プロキシ）
        if let Ok(w10) = webview.cast::<ICoreWebView2_10>() {
            w10.add_BasicAuthenticationRequested(
                &BasicAuthenticationRequestedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        basic_auth(&args);
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
        // 証明書のエラー: 転送するホストで、開発者用証明書の指紋が一致したときだけ許す
        if let Ok(w14) = webview.cast::<ICoreWebView2_14>() {
            w14.add_ServerCertificateErrorDetected(
                &ServerCertificateErrorDetectedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        server_certificate_error(&args);
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
        // 広告ブロック: 要求の照合（フィルタはプロファイルで入のときだけ登録する）
        webview.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    on_resource_requested(id, &args);
                }
                Ok(())
            })),
            &mut token,
        )?;
        if with(|a| !a.profile.adblock_off).unwrap_or(false) {
            adblock::set_request_filter(webview, true);
        }
        // 広告ブロック: 広告の枠を隠す（ホスト向けの CSS と、class・id を集めるスクリプト）
        if let Ok(w2) = webview.cast::<ICoreWebView2_2>() {
            w2.add_DOMContentLoaded(
                &DOMContentLoadedEventHandler::create(Box::new(move |sender, _| {
                    if let Some(w) = sender {
                        on_dom_loaded(id, &w);
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
        webview.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |sender, args| {
                if let (Some(w), Some(args)) = (sender, args) {
                    let msg = take_string(|p| args.TryGetWebMessageAsString(p));
                    on_web_message(id, &w, &msg);
                }
                Ok(())
            })),
            &mut token,
        )?;
        // ページにフォーカスがあってもブラウザのショートカットを使う
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
                if is_shortcut(vk as u16) {
                    args.SetHandled(true)?;
                    // 呼ばれている途中なので、行うのは後で
                    if let Some(frame) = with(|a| a.frame) {
                        let mods = modifiers();
                        let _ = PostMessageW(
                            Some(frame),
                            WM_APP_SHORTCUT,
                            WPARAM(vk as usize),
                            LPARAM(mods as isize),
                        );
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

/// 新しいタブで開く（`lparam` は `Box<String>`）。
const WM_APP_OPEN_TAB: u32 = WM_APP + 121;
/// ページの中で押したショートカット（`wparam` は仮想キー、`lparam` は修飾キーの組）。
const WM_APP_SHORTCUT: u32 = WM_APP + 122;
/// アドレスバーの左の表示を出す・隠す（`wparam` が 1 なら出す）。
const WM_APP_BADGE: u32 = WM_APP + 123;

fn web_error(s: COREWEBVIEW2_WEB_ERROR_STATUS) -> &'static str {
    match s {
        COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT => "つなげません",
        COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED => "ホスト名を解決できません",
        COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT => "時間切れ",
        COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED => "接続が切れました",
        COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_RESET => "接続がリセットされました",
        COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED => "ネットワークにつながっていません",
        COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT
        | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED
        | COREWEBVIEW2_WEB_ERROR_STATUS_CLIENT_CERTIFICATE_CONTAINS_ERRORS
        | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED
        | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID => "証明書のエラー",
        COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED => "中止しました",
        COREWEBVIEW2_WEB_ERROR_STATUS_VALID_AUTHENTICATION_CREDENTIALS_REQUIRED
        | COREWEBVIEW2_WEB_ERROR_STATUS_VALID_PROXY_AUTHENTICATION_REQUIRED => "認証が必要です",
        _ => "エラー",
    }
}

/// 証明書のエラーの見立て。
#[derive(Debug, PartialEq, Eq)]
enum DevCertCheck {
    /// 転送するホストでない・指紋を登録していない → ふつうのエラーの画面
    NotPinned,
    /// 登録した指紋と一致した（ホスト, 指紋）
    Match(String, String),
    /// 一致しなかった（ホスト, サーバーの証明書の指紋）
    Mismatch(String, Option<String>),
}

/// 証明書のエラーが起きた URL とサーバーの証明書（PEM）を、プロファイルの開発者用証明書と照らす。
fn check_dev_cert(profile: &ProxyProfile, uri: &str, pem: &str) -> DevCertCheck {
    let Some((_, host, port)) = yy_browser::rules::url_host_port(uri) else {
        return DevCertCheck::NotPinned;
    };
    let Some(pinned) = profile.host_map(&host, port).and_then(|m| m.pinned()) else {
        return DevCertCheck::NotPinned;
    };
    let actual = yy_browser::rules::cert_fingerprint(pem.as_bytes());
    if actual.as_deref() == Some(pinned.as_str()) {
        DevCertCheck::Match(host, pinned)
    } else {
        DevCertCheck::Mismatch(host, actual)
    }
}

/// 証明書のエラーの引数から（URL, サーバーの証明書の PEM）。
fn cert_error_parts(
    args: &ICoreWebView2ServerCertificateErrorDetectedEventArgs,
) -> (String, String) {
    let uri = take_string(|p| unsafe { args.RequestUri(p) });
    let pem = match unsafe { args.ServerCertificate() } {
        Ok(c) => take_string(|p| unsafe { c.ToPemEncoding(p) }),
        Err(_) => String::new(),
    };
    (uri, pem)
}

/// 証明書のエラー。転送するホストに開発者用証明書の指紋があり、サーバーの証明書の指紋と一致すれば
/// 許す（このセッションの間。OS の証明書ストアは変えない）。それ以外はふつうのエラーの画面。
fn server_certificate_error(args: &ICoreWebView2ServerCertificateErrorDetectedEventArgs) {
    let (uri, pem) = cert_error_parts(args);
    let Some(check) = with(|a| check_dev_cert(&a.profile, &uri, &pem)) else {
        return;
    };
    match check {
        DevCertCheck::NotPinned => {}
        DevCertCheck::Match(host, pinned) => {
            unsafe {
                let _ = args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW);
            }
            with(|a| {
                a.set_status(&format!(
                    "{host}: 開発者用証明書（SHA-256 {pinned}）を使っています"
                ));
                a.dev_hosts.insert(host);
                a.refresh_chrome();
            });
        }
        DevCertCheck::Mismatch(host, actual) => {
            with(|a| {
                a.set_status(&format!(
                    "{host}: サーバーの証明書が開発者用証明書と一致しません（サーバー: {}）",
                    actual.as_deref().unwrap_or("読めません")
                ))
            });
        }
    }
}

// ---- 広告ブロック（20 章） ------------------------------------------------------------

/// このタブで広告ブロックを使うか（エンジン・トップのページの URL）。
fn adblock_for(a: &App, id: u64) -> Option<(Arc<yy_adblock::AdBlocker>, String)> {
    let blocker = a.adblock.clone()?;
    let t = a.tabs.get(a.index_of(id)?)?;
    let page = if t.nav_url.is_empty() {
        t.url.clone()
    } else {
        t.nav_url.clone()
    };
    let host = yy_browser::rules::url_host_port(&page)
        .map(|(_, h, _)| h)
        .unwrap_or_default();
    a.profile.adblock_on(&host).then_some((blocker, page))
}

/// 要求を照らし、広告・追跡なら止める。
fn on_resource_requested(id: u64, args: &ICoreWebView2WebResourceRequestedEventArgs) {
    let Some(Some((blocker, page, env))) =
        with(|a| adblock_for(a, id).and_then(|(b, p)| Some((b, p, a.env.clone()?))))
    else {
        return;
    };
    let uri = match unsafe { args.Request() } {
        Ok(r) => take_string(|p| unsafe { r.Uri(p) }),
        Err(_) => return,
    };
    let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
    unsafe {
        let _ = args.ResourceContext(&mut ctx);
    }
    if !adblock::decide(&blocker, &page, &uri, ctx) {
        return;
    }
    adblock::block(&env, args);
    with(|a| {
        if let Some(i) = a.index_of(id) {
            a.tabs[i].blocked += 1;
            if i == a.current {
                a.refresh_shield();
            }
        }
    });
}

/// ページの読み込み（DOMContentLoaded）: 広告の枠を隠す。
fn on_dom_loaded(id: u64, webview: &ICoreWebView2) {
    let url = take_string(|p| unsafe { webview.Source(p) });
    let Some(Some((blocker, _))) = with(|a| adblock_for(a, id)) else {
        return;
    };
    let page = adblock::on_dom_loaded(&blocker, webview, &url);
    with(|a| {
        if let Some(i) = a.index_of(id) {
            a.tabs[i].cosmetic = Some(page);
        }
    });
}

/// ページからのメッセージ（class・id）: 当てはまる汎用の規則で隠す。
fn on_web_message(id: u64, webview: &ICoreWebView2, msg: &str) {
    if !msg.starts_with(yy_adblock::engine::MESSAGE_PREFIX) {
        return;
    }
    let Some(Some((blocker, page))) = with(|a| {
        let (b, _) = adblock_for(a, id)?;
        let page = a.tabs.get(a.index_of(id)?)?.cosmetic.clone()?;
        Some((b, page))
    }) else {
        return;
    };
    adblock::on_message(&blocker, &page, webview, msg);
}

/// フィルタを更新する（別のスレッド）。更新中なら何もしない。
fn update_filters(initial: bool, force: bool) {
    let Some(Some((frame, lists, profile))) = with(|a| {
        if a.adblock_busy {
            return None;
        }
        a.adblock_busy = true;
        Some((a.frame, a.profiles.adblock.lists.clone(), a.profile.clone()))
    }) else {
        return;
    };
    adblock::spawn_update(frame, lists, profile, initial, force);
}

/// 別のスレッドからの知らせ。
fn on_adblock_msg(m: adblock::AdMsg) {
    match m {
        adblock::AdMsg::Engine(b) => {
            with(|a| {
                a.adblock = Some(b);
                a.refresh_shield();
            });
        }
        adblock::AdMsg::Status(s) => {
            with(|a| a.set_status(&s));
        }
        adblock::AdMsg::Done(s) => {
            with(|a| {
                a.adblock_busy = false;
                a.set_status(&s);
                a.refresh_shield();
            });
        }
    }
}

/// プロファイル（今のものと一覧の中の同じもの）を書き換えて保存する。
fn edit_profile(f: impl FnOnce(&mut ProxyProfile)) {
    let Some((frame, list)) = with(|a| {
        f(&mut a.profile);
        let name = a.profile.name.clone();
        if let Some(p) = a.profiles.profiles.iter_mut().find(|p| p.name == name) {
            *p = a.profile.clone();
        }
        (a.frame, a.profiles.clone())
    }) else {
        return;
    };
    if let Err(e) = list.save(&profiles_path()) {
        error_box(frame, &format!("保存できません: {e}"));
    }
}

/// 広告ブロックの入・切（このプロファイル）。タブの要求の照合を付け外しする。
fn toggle_adblock() {
    edit_profile(|p| p.adblock_off = !p.adblock_off);
    let Some((on, webviews)) = with(|a| {
        let ws: Vec<ICoreWebView2> = a.tabs.iter().filter_map(|t| t.webview.clone()).collect();
        (!a.profile.adblock_off, ws)
    }) else {
        return;
    };
    for w in &webviews {
        adblock::set_request_filter(w, on);
    }
    with(|a| {
        a.set_status(if on {
            "広告ブロックを入にしました（再読み込みで反映します）"
        } else {
            "広告ブロックを切にしました（再読み込みで反映します）"
        });
        a.refresh_shield();
    });
}

/// 今のタブのサイトを「止めない」に足す・外し、再読み込みする。
fn toggle_adblock_site() {
    let Some(Some(host)) = with(|a| {
        let t = a.tabs.get(a.current)?;
        yy_browser::rules::url_host_port(&t.url).map(|(_, h, _)| h)
    }) else {
        return;
    };
    let allowed = with(|a| a.profile.adblock_allowed_site(&host)).unwrap_or(false);
    edit_profile(|p| {
        if allowed {
            // 足したもの（このホストか、その上のドメイン）を外す
            let h = host.clone();
            p.adblock_allow.retain(|d| {
                let d = d.trim().to_ascii_lowercase();
                !(h == d || h.ends_with(&format!(".{d}")))
            });
        } else {
            p.set_adblock_allowed(&host, true);
        }
    });
    if let Some((w, _)) = current_web() {
        unsafe {
            let _ = w.Reload();
        }
    }
    with(|a| a.refresh_shield());
}

/// フィルタリストの一覧を編集する。保存したら作り直す（足したものはダウンロードする）。
fn edit_filter_lists() {
    let Some((frame, lists)) = with(|a| (a.frame, a.profiles.adblock.lists.clone())) else {
        return;
    };
    let Some(edited) = filterdlg::edit(frame, lists) else {
        return;
    };
    let Some(list) = with(|a| {
        a.profiles.adblock.lists = edited;
        a.profiles.clone()
    }) else {
        return;
    };
    if let Err(e) = list.save(&profiles_path()) {
        error_box(frame, &format!("保存できません: {e}"));
        return;
    }
    update_filters(true, false);
}

/// 広告ブロックのボタン・メニューの項目を足す。
fn fill_adblock_menu(m: HMENU, a: &App) {
    let host = a
        .tabs
        .get(a.current)
        .and_then(|t| yy_browser::rules::url_host_port(&t.url))
        .map(|(_, h, _)| h)
        .unwrap_or_default();
    unsafe {
        let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(
            m,
            MF_STRING | check(!a.profile.adblock_off),
            ID_AB_TOGGLE as usize,
            &HSTRING::from(format!(
                "広告ブロック（プロファイル「{}」）(&B)",
                a.profile.name.replace('&', "&&")
            )),
        );
        let site_flags = if host.is_empty() || a.profile.adblock_off {
            MF_STRING | MF_GRAYED
        } else {
            MF_STRING | check(a.profile.adblock_allowed_site(&host))
        };
        let _ = AppendMenuW(
            m,
            site_flags,
            ID_AB_SITE as usize,
            &HSTRING::from(if host.is_empty() {
                "このサイトでは止めない(&S)".to_owned()
            } else {
                format!("このサイトでは止めない（{host}）(&S)")
            }),
        );
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(
            m,
            if a.adblock_busy {
                MF_STRING | MF_GRAYED
            } else {
                MF_STRING
            },
            ID_AB_UPDATE as usize,
            w!("フィルタを今すぐ更新(&U)"),
        );
        let _ = AppendMenuW(
            m,
            MF_STRING,
            ID_AB_LISTS as usize,
            w!("フィルタリスト(&L)..."),
        );
    }
}

/// 広告ブロックのボタンを押したとき: メニューを出す。
fn shield_menu() {
    let Some((frame, btn, m)) = with(|a| {
        let m = unsafe { CreatePopupMenu() }.ok()?;
        fill_adblock_menu(m, a);
        Some((a.frame, a.shield, m))
    })
    .flatten() else {
        return;
    };
    unsafe {
        let mut rc = RECT::default();
        let _ = GetWindowRect(btn, &mut rc);
        let _ = TrackPopupMenu(
            m,
            TPM_LEFTALIGN | TPM_TOPALIGN,
            rc.left,
            rc.bottom,
            None,
            frame,
            None,
        );
        let _ = DestroyMenu(m);
    }
}

// ---- ブックマーク（19 章 4.1） ---------------------------------------------------------

/// 今のページをブックマークに足す（登録済みなら編集・削除）。
fn bookmark_page() {
    let Some(Some((frame, url, title))) = with(|a| {
        let t = a.tabs.get(a.current)?;
        Some((a.frame, t.url.clone(), t.title.clone()))
    }) else {
        return;
    };
    if url.is_empty() || url == "about:blank" {
        info_box(frame, "ブックマークにするページを開いてください。");
        return;
    }
    let mut list = bookmarkui::load();
    let existing = list.find(&url);
    let b = match existing {
        Some(i) => list.items[i].clone(),
        None => yy_browser::bookmarks::Bookmark::new(&title, &url, ""),
    };
    let folders = list.folders();
    // ダイアログの間にほかのウィンドウが書き換えることがあるので、結果は読み直した一覧に当てる
    let result = bookmarkui::edit(frame, b, existing.is_some(), folders);
    list = bookmarkui::load();
    match result {
        bookmarkui::EditResult::Save(nb) => {
            if let Some(i) = list.find(&url).filter(|_| nb.url != url) {
                list.items.remove(i);
            }
            list.put(nb);
        }
        bookmarkui::EditResult::Delete => {
            if let Some(i) = list.find(&url) {
                list.items.remove(i);
            }
        }
        bookmarkui::EditResult::Cancel => return,
    }
    if bookmarkui::save(frame, &list) {
        with(|a| {
            a.bookmarks = list;
            a.refresh_chrome();
        });
    }
}

/// ブックマークの管理の画面。開くものを選んだら開く。
fn manage_bookmarks() {
    let Some(frame) = with(|a| a.frame) else {
        return;
    };
    let open = bookmarkui::manage(frame);
    with(|a| {
        a.bookmarks = bookmarkui::load();
        a.refresh_chrome();
    });
    match open {
        Some((url, true)) => new_tab(Some(&url)),
        Some((url, false)) => open_in_current(&url),
        None => {}
    }
}

/// アドレスバーの左の表示を押したとき: 詳しく出す。
fn show_badge_details() {
    let Some((frame, text)) = with(|a| {
        let t = a.tabs.get(a.current)?;
        let (scheme, host, port) = yy_browser::rules::url_host_port(&t.url)?;
        let m = a.profile.host_map(&host, port);
        let mut s = format!("{scheme}://{host}:{port}\n");
        match m {
            Some(m) => s.push_str(&format!(
                "転送先: {}（プロファイル「{}」のホストの転送）\n",
                m.address.trim(),
                a.profile.name
            )),
            None => s.push_str("転送はしていません\n"),
        }
        if scheme == "https" && a.dev_hosts.contains(&host) {
            let fp = m.and_then(|m| m.pinned()).unwrap_or_default();
            s.push_str(&format!(
                "\n開発者用証明書を利用中です。\nサーバーの証明書は公に信頼された認証局のものではありませんが、                 プロファイルに登録した開発者用証明書（SHA-256 の指紋）と一致したので、このウィンドウの間だけ                 受け入れています。OS の証明書ストアは変えていません。\n\nSHA-256: {fp}"
            ));
        }
        Some((a.frame, s))
    })
    .flatten() else {
        return;
    };
    info_box(frame, &text);
}

/// Basic 認証のユーザー名とパスワードを尋ねる。
fn basic_auth(args: &ICoreWebView2BasicAuthenticationRequestedEventArgs) {
    let Some(frame) = with(|a| a.frame) else {
        return;
    };
    let uri = take_string(|p| unsafe { args.Uri(p) });
    let challenge = take_string(|p| unsafe { args.Challenge(p) });
    let title = "認証が必要です";
    let prompt = format!("{uri}\n{challenge}\nユーザー名:");
    let Some(user) = crate::goto::prompt_text(frame, title, &prompt, "") else {
        unsafe {
            let _ = args.SetCancel(true);
        }
        return;
    };
    let Some(pass) = crate::goto::prompt_secret(frame, title, &format!("{user} のパスワード:"))
    else {
        unsafe {
            let _ = args.SetCancel(true);
        }
        return;
    };
    unsafe {
        if let Ok(r) = args.Response() {
            let _ = r.SetUserName(&HSTRING::from(user));
            let _ = r.SetPassword(&HSTRING::from(pass));
        }
    }
}

// ---- タブ ------------------------------------------------------------------------------

/// 新しいタブを開く（`url` が `None` ならホーム）。環境がまだなら、できてから表示を作る。
fn new_tab(url: Option<&str>) {
    let Some((env, frame, id, generation)) = with(|a| {
        let id = a.next_id;
        a.next_id += 1;
        let home = a.config.browser.home.clone();
        let url = url.map(str::to_owned).unwrap_or(home);
        let t = Tab {
            id,
            controller: None,
            webview: None,
            title: String::new(),
            url: url.clone(),
            loading: false,
            pending: Some(url),
            blocked: 0,
            nav_url: String::new(),
            cosmetic: None,
        };
        let i = a.tabs.len();
        a.tabs.push(t);
        let s = crate::util::wide(&App::tab_label(&a.tabs[i]));
        let item = TCITEMW {
            mask: TCIF_TEXT,
            pszText: PWSTR(s.as_ptr() as *mut _),
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                a.tabs_hwnd,
                TCM_INSERTITEMW,
                Some(WPARAM(i)),
                Some(LPARAM(&item as *const _ as isize)),
            );
        }
        (a.env.clone(), a.frame, id, a.generation)
    }) else {
        return;
    };
    let i = with(|a| a.tabs.len() - 1).unwrap_or(0);
    select_tab(i);
    if let Some(env) = env {
        create_controller(&env, frame, id, generation);
    }
}

fn select_tab(i: usize) {
    with(|a| {
        if i >= a.tabs.len() {
            return;
        }
        a.current = i;
        unsafe {
            SendMessageW(a.tabs_hwnd, TCM_SETCURSEL, Some(WPARAM(i)), None);
        }
        a.find = None;
        a.layout();
        a.refresh_chrome();
    });
    if let Some((_, c)) = current_web() {
        unsafe {
            let _ = c.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
        }
    }
}

fn close_tab(i: usize) {
    let Some((controller, last)) = with(|a| {
        if i >= a.tabs.len() {
            return None;
        }
        let t = a.tabs.remove(i);
        if !t.url.is_empty() && t.url != "about:blank" {
            a.closed.push(t.url.clone());
            if a.closed.len() > 30 {
                a.closed.remove(0);
            }
        }
        unsafe {
            SendMessageW(a.tabs_hwnd, TCM_DELETEITEM, Some(WPARAM(i)), None);
        }
        if a.current >= a.tabs.len() {
            a.current = a.tabs.len().saturating_sub(1);
        } else if a.current > i {
            a.current -= 1;
        }
        Some((t.controller, a.tabs.is_empty()))
    })
    .flatten() else {
        return;
    };
    if let Some(c) = controller {
        unsafe {
            let _ = c.Close();
        }
    }
    if last {
        // 最後のタブを閉じたらウィンドウも閉じる
        if let Some(f) = with(|a| a.frame) {
            unsafe {
                let _ = PostMessageW(Some(f), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        return;
    }
    let cur = with(|a| a.current).unwrap_or(0);
    select_tab(cur);
}

// ---- 操作 ------------------------------------------------------------------------------

/// アドレスバーの内容を開く。
fn go_address() {
    let Some((text, search)) = with(|a| (text_of(a.address), a.config.browser.search_url.clone()))
    else {
        return;
    };
    let Some(url) = yy_browser::input::to_url(&text, &search) else {
        return;
    };
    open_in_current(&url);
}

fn open_in_current(url: &str) {
    if let Some((w, c)) = current_web() {
        navigate_web(&w, url);
        unsafe {
            let _ = c.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
        }
    } else {
        // 表示ができる前: できたら開く
        with(|a| {
            let cur = a.current;
            if let Some(t) = a.tabs.get_mut(cur) {
                t.pending = Some(url.to_owned());
                t.url = url.to_owned();
            }
        });
    }
}

fn zoom(step: i32) {
    let Some((_, c)) = current_web() else { return };
    unsafe {
        let mut z = 1.0f64;
        let _ = c.ZoomFactor(&mut z);
        let next = if step == 0 {
            1.0
        } else {
            let i = ZOOMS
                .iter()
                .position(|v| (*v - z).abs() < 0.01)
                .unwrap_or_else(|| ZOOMS.iter().position(|v| *v > z).unwrap_or(ZOOMS.len() - 1));
            ZOOMS[(i as i32 + step).clamp(0, ZOOMS.len() as i32 - 1) as usize]
        };
        let _ = c.SetZoomFactor(next);
        with(|a| a.set_status(&format!("拡大: {}%", (next * 100.0).round())));
    }
}

fn set_fullscreen(on: bool) {
    with(|a| unsafe {
        if on == a.fullscreen.is_some() {
            return;
        }
        if on {
            let mut wp = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            let _ = GetWindowPlacement(a.frame, &mut wp);
            let style = GetWindowLongW(a.frame, GWL_STYLE);
            a.fullscreen = Some((wp, style));
            let mon = windows::Win32::Graphics::Gdi::MonitorFromWindow(
                a.frame,
                windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST,
            );
            let mut mi = windows::Win32::Graphics::Gdi::MONITORINFO {
                cbSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = windows::Win32::Graphics::Gdi::GetMonitorInfoW(mon, &mut mi);
            let _ = SetMenu(a.frame, None);
            SetWindowLongW(
                a.frame,
                GWL_STYLE,
                style & !(WS_OVERLAPPEDWINDOW.0 as i32) | WS_POPUP.0 as i32,
            );
            let r = mi.rcMonitor;
            let _ = SetWindowPos(
                a.frame,
                Some(HWND_TOP),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_FRAMECHANGED | SWP_SHOWWINDOW,
            );
        } else if let Some((wp, style)) = a.fullscreen.take() {
            SetWindowLongW(a.frame, GWL_STYLE, style);
            if let Ok(m) = create_menu(&a.profiles, &a.profile.name) {
                let _ = SetMenu(a.frame, Some(m));
            }
            let _ = SetWindowPlacement(a.frame, &wp);
            let _ = SetWindowPos(
                a.frame,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
        }
        a.layout();
    });
}

fn show_find(on: bool) {
    let edit = with(|a| {
        a.find_visible = on;
        a.layout();
        a.find_bar[0]
    });
    if on {
        if let Some(e) = edit {
            unsafe {
                let _ = SetFocus(Some(e));
                SendMessageW(e, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
            }
        }
    } else {
        let f = with(|a| a.find.take()).flatten();
        if let Some(f) = f {
            unsafe {
                let _ = f.Stop();
            }
        }
        if let Some((_, c)) = current_web() {
            unsafe {
                let _ = c.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
            }
        }
    }
}

/// ページ内検索（`dir`: 0 は新しく始める、1 は次、-1 は前）。WebView2 が古くて使えなければ知らせる。
fn find(dir: i32) {
    let Some((term, existing, env)) =
        with(|a| (text_of(a.find_bar[0]), a.find.clone(), a.env.clone()))
    else {
        return;
    };
    if term.is_empty() {
        return;
    }
    let Some((w, _)) = current_web() else { return };
    unsafe {
        if dir != 0
            && let Some(f) = &existing
        {
            let _ = if dir > 0 {
                f.FindNext()
            } else {
                f.FindPrevious()
            };
            update_find_info(f);
            return;
        }
        let (Ok(w28), Some(Ok(env15))) = (
            w.cast::<ICoreWebView2_28>(),
            env.map(|e| e.cast::<ICoreWebView2Environment15>()),
        ) else {
            with(|a| {
                set_text(a.find_bar[3], "この WebView2 では使えません");
            });
            return;
        };
        let (Ok(f), Ok(opts)) = (w28.Find(), env15.CreateFindOptions()) else {
            return;
        };
        let _ = opts.SetFindTerm(&HSTRING::from(term));
        let _ = opts.SetShouldHighlightAllMatches(true);
        let f2 = f.clone();
        let handler = FindStartCompletedHandler::create(Box::new(move |_r: Result<()>| {
            update_find_info(&f2);
            Ok(())
        }));
        let _ = f.Start(&opts, &handler);
        with(|a| a.find = Some(f));
    }
}

fn update_find_info(f: &ICoreWebView2Find) {
    let (mut idx, mut count) = (0i32, 0i32);
    unsafe {
        let _ = f.ActiveMatchIndex(&mut idx);
        let _ = f.MatchCount(&mut count);
    }
    with(|a| {
        let s = if count <= 0 {
            "見つかりません".to_owned()
        } else {
            format!("{} / {count}", idx.max(0) + 1)
        };
        set_text(a.find_bar[3], &s);
    });
}

fn print() {
    let Some((w, _)) = current_web() else { return };
    unsafe {
        match w.cast::<ICoreWebView2_16>() {
            Ok(w16) => {
                let _ = w16.ShowPrintUI(COREWEBVIEW2_PRINT_DIALOG_KIND_BROWSER);
            }
            Err(_) => {
                let _ = w.ExecuteScript(w!("window.print()"), None);
            }
        }
    }
}

/// 別のプロセスとして新しいウィンドウを開く（`profile` のプロキシ）。
fn new_window(profile: Option<&str>) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    if let Some(p) = profile {
        cmd.arg("--profile").arg(p);
    }
    if let Err(e) = cmd.spawn()
        && let Some(f) = with(|a| a.frame)
    {
        error_box(f, &format!("新しいウィンドウを開けません: {e}"));
    }
}

/// このウィンドウのプロキシのプロファイルを切り替える（環境を作り直し、タブを同じ URL で開き直す）。
fn switch_profile(name: &str) {
    let Some((frame, profile, same, count)) = with(|a| {
        let p = a.profiles.get(name).cloned();
        (a.frame, p, a.profile.name == name, a.tabs.len())
    }) else {
        return;
    };
    let Some(profile) = profile else { return };
    if same {
        return;
    }
    let ok = unsafe {
        MessageBoxW(
            Some(frame),
            &HSTRING::from(format!(
                "プロキシを「{}」に切り替えます。\n開いている {count} 個のタブは同じ URL で開き直します（入力中の内容は失われます）。\n\nよろしいですか？",
                profile.describe()
            )),
            w!("yybrowser"),
            MB_OKCANCEL | MB_ICONQUESTION,
        )
    } == IDOK;
    if !ok {
        return;
    }
    let controllers = with(|a| {
        a.profile = profile;
        a.find = None;
        let mut cs = Vec::new();
        for t in &mut a.tabs {
            t.pending = Some(t.url.clone());
            t.webview = None;
            if let Some(c) = t.controller.take() {
                cs.push(c);
            }
        }
        cs
    })
    .unwrap_or_default();
    for c in controllers {
        unsafe {
            let _ = c.Close();
        }
    }
    rebuild_menu();
    with(|a| a.update_proxy_label());
    start_environment();
}

fn rebuild_menu() {
    with(|a| unsafe {
        if a.fullscreen.is_some() {
            return;
        }
        if let Ok(m) = create_menu(&a.profiles, &a.profile.name) {
            let old = GetMenu(a.frame);
            let _ = SetMenu(a.frame, Some(m));
            if !old.0.is_null() {
                let _ = DestroyMenu(old);
            }
        }
    });
}

fn proxy_settings() {
    let Some((frame, list, current)) =
        with(|a| (a.frame, a.profiles.clone(), a.profile.name.clone()))
    else {
        return;
    };
    let Some(edited) = proxydlg::edit(frame, list) else {
        return;
    };
    if let Err(e) = edited.save(&profiles_path()) {
        error_box(frame, &format!("保存できません: {e}"));
        return;
    }
    // 今のプロファイルが書き換わったら切り替え直す
    let changed = edited.get(&current).cloned();
    let current_changed = with(|a| {
        a.profiles = edited.clone();
        match &changed {
            Some(p) if *p != a.profile => true,
            None => {
                // 消された: 起動時のものへ
                true
            }
            _ => false,
        }
    })
    .unwrap_or(false);
    rebuild_menu();
    if current_changed {
        let target = changed
            .map(|p| p.name)
            .unwrap_or_else(|| edited.startup().name);
        // 名前が同じでも中身が変わったら作り直す
        with(|a| a.profile.name = String::new());
        switch_profile(&target);
    }
}

/// 押されている修飾キー（1: Ctrl、2: Shift、4: Alt）。
fn modifiers() -> u8 {
    let down = |vk: VIRTUAL_KEY| unsafe { GetKeyState(vk.0 as i32) } < 0;
    (down(VK_CONTROL) as u8) | ((down(VK_SHIFT) as u8) << 1) | ((down(VK_MENU) as u8) << 2)
}

/// ブラウザのショートカットになり得るキーか（ページの中で押したときに横取りする）。
fn is_shortcut(vk: u16) -> bool {
    let m = modifiers();
    let ctrl = m & 1 != 0;
    let alt = m & 4 != 0;
    let k = VIRTUAL_KEY(vk);
    if ctrl {
        return matches!(
            k,
            VK_T | VK_W
                | VK_D
                | VK_N
                | VK_L
                | VK_F
                | VK_P
                | VK_R
                | VK_TAB
                | VK_OEM_PLUS
                | VK_OEM_MINUS
                | VK_ADD
                | VK_SUBTRACT
                | VK_0
                | VK_NUMPAD0
        ) || (VK_1.0..=VK_9.0).contains(&vk)
            || (k == VK_O && m & 2 != 0);
    }
    if alt {
        return matches!(k, VK_LEFT | VK_RIGHT | VK_HOME | VK_D);
    }
    matches!(k, VK_F1 | VK_F3 | VK_F5 | VK_F6 | VK_F11 | VK_F12)
        || (k == VK_ESCAPE && with(|a| a.find_visible || a.fullscreen.is_some()).unwrap_or(false))
}

/// ショートカットを行う。行ったら `true`。
fn shortcut(vk: u16, mods: u8) -> bool {
    let ctrl = mods & 1 != 0;
    let shift = mods & 2 != 0;
    let alt = mods & 4 != 0;
    let k = VIRTUAL_KEY(vk);
    match (ctrl, alt, k) {
        (true, _, VK_T) if shift => command(ID_REOPEN_TAB),
        (true, _, VK_T) => command(ID_NEW_TAB),
        (true, _, VK_W) => command(ID_CLOSE_TAB),
        (true, _, VK_D) => command(ID_BM_ADD),
        (true, _, VK_O) if shift => command(ID_BM_MANAGE),
        (true, _, VK_N) => command(ID_NEW_WINDOW),
        (true, _, VK_L) => command_focus_address(),
        (true, _, VK_F) => command(ID_FIND),
        (true, _, VK_P) => command(ID_PRINT),
        (true, _, VK_R) => command(ID_RELOAD),
        (true, _, VK_OEM_PLUS) | (true, _, VK_ADD) => command(ID_ZOOM_IN),
        (true, _, VK_OEM_MINUS) | (true, _, VK_SUBTRACT) => command(ID_ZOOM_OUT),
        (true, _, VK_0) | (true, _, VK_NUMPAD0) => command(ID_ZOOM_RESET),
        (true, _, VK_TAB) => {
            let (cur, n) = with(|a| (a.current, a.tabs.len())).unwrap_or((0, 0));
            if n > 0 {
                let next = if shift {
                    (cur + n - 1) % n
                } else {
                    (cur + 1) % n
                };
                select_tab(next);
            }
        }
        (true, _, k) if (VK_1.0..=VK_9.0).contains(&k.0) => {
            let n = with(|a| a.tabs.len()).unwrap_or(0);
            let want = if k == VK_9 {
                n.saturating_sub(1)
            } else {
                (k.0 - VK_1.0) as usize
            };
            if want < n {
                select_tab(want);
            }
        }
        (false, true, VK_LEFT) => command(ID_BACK),
        (false, true, VK_RIGHT) => command(ID_FORWARD),
        (false, true, VK_HOME) => command(ID_HOME),
        (false, true, VK_D) => command_focus_address(),
        (false, false, VK_F1) => command(ID_HELP),
        (false, false, VK_F3) => find(if shift { -1 } else { 1 }),
        (false, false, VK_F5) => command(ID_RELOAD),
        (false, false, VK_F6) => command_focus_address(),
        (false, false, VK_F11) => command(ID_FULLSCREEN),
        (false, false, VK_F12) => command(ID_DEVTOOLS),
        (false, false, VK_ESCAPE) => {
            if with(|a| a.find_visible).unwrap_or(false) {
                show_find(false);
            } else if with(|a| a.fullscreen.is_some()).unwrap_or(false) {
                set_fullscreen(false);
            } else {
                return false;
            }
        }
        _ => return false,
    }
    true
}

fn command_focus_address() {
    if let Some(h) = with(|a| a.address) {
        unsafe {
            let _ = SetFocus(Some(h));
            SendMessageW(h, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
    }
}

/// 自前の部品（アドレスバーなど）にフォーカスがあるときのキー。
fn key_hook(msg: &MSG) -> bool {
    let vk = msg.wParam.0 as u16;
    let Some((address, find_edit)) = with(|a| (a.address, a.find_bar[0])) else {
        return false;
    };
    let mods = modifiers();
    if msg.hwnd == address {
        if vk == VK_RETURN.0 {
            go_address();
            return true;
        }
        if vk == VK_ESCAPE.0 {
            with(|a| a.refresh_chrome());
            if let Some((_, c)) = current_web() {
                unsafe {
                    let _ = c.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
                }
            }
            return true;
        }
    }
    if msg.hwnd == find_edit && vk == VK_RETURN.0 {
        let started = with(|a| a.find.is_some()).unwrap_or(false);
        find(if !started {
            0
        } else if mods & 2 != 0 {
            -1
        } else {
            1
        });
        return true;
    }
    if is_shortcut(vk) {
        return shortcut(vk, mods);
    }
    false
}

fn command(id: u16) {
    match id {
        ID_NEW_TAB => new_tab(None),
        ID_CLOSE_TAB => {
            if let Some(i) = with(|a| a.current) {
                close_tab(i);
            }
        }
        ID_REOPEN_TAB => {
            if let Some(u) = with(|a| a.closed.pop()).flatten() {
                new_tab(Some(&u));
            }
        }
        ID_NEW_WINDOW => {
            let p = with(|a| a.profile.name.clone());
            // コマンドラインのその場限りのプロファイルは一覧にないので、既定で開く
            let known = with(|a| a.profiles.get(&a.profile.name).is_some()).unwrap_or(false);
            new_window(if known { p.as_deref() } else { None });
        }
        ID_BACK => {
            if let Some((w, _)) = current_web() {
                unsafe {
                    let _ = w.GoBack();
                }
            }
        }
        ID_FORWARD => {
            if let Some((w, _)) = current_web() {
                unsafe {
                    let _ = w.GoForward();
                }
            }
        }
        ID_RELOAD => {
            let loading =
                with(|a| a.tabs.get(a.current).is_some_and(|t| t.loading)).unwrap_or(false);
            if let Some((w, _)) = current_web() {
                unsafe {
                    let _ = if loading { w.Stop() } else { w.Reload() };
                }
            }
        }
        ID_HOME => {
            if let Some(h) = with(|a| a.config.browser.home.clone()) {
                open_in_current(&h);
            }
        }
        ID_FIND => show_find(true),
        ID_FIND_NEXT => find(1),
        ID_FIND_PREV => find(-1),
        ID_FIND_CLOSE => show_find(false),
        ID_PRINT => print(),
        ID_ZOOM_IN => zoom(1),
        ID_ZOOM_OUT => zoom(-1),
        ID_ZOOM_RESET => zoom(0),
        ID_FULLSCREEN => {
            let on = with(|a| a.fullscreen.is_none()).unwrap_or(false);
            set_fullscreen(on);
        }
        ID_DEVTOOLS => {
            let allowed = with(|a| a.config.browser.devtools).unwrap_or(false);
            if allowed && let Some((w, _)) = current_web() {
                unsafe {
                    let _ = w.OpenDevToolsWindow();
                }
            }
        }
        ID_BADGE => show_badge_details(),
        ID_SHIELD => shield_menu(),
        ID_STAR | ID_BM_ADD => bookmark_page(),
        ID_BM_MANAGE => manage_bookmarks(),
        id if (ID_BM_BASE..ID_BM_BASE + bookmarkui::MENU_MAX as u16).contains(&id) => {
            let url = with(|a| {
                a.bookmarks
                    .items
                    .get((id - ID_BM_BASE) as usize)
                    .map(|b| b.url.clone())
            })
            .flatten();
            if let Some(u) = url {
                open_in_current(&u);
            }
        }
        ID_AB_TOGGLE => toggle_adblock(),
        ID_AB_SITE => toggle_adblock_site(),
        ID_AB_UPDATE => update_filters(false, true),
        ID_AB_LISTS => edit_filter_lists(),
        ID_PROXY => {
            // プロキシのボタン: メニューを出す
            let Some((frame, btn, profiles, cur)) = with(|a| {
                (
                    a.frame,
                    a.proxy_btn,
                    a.profiles.clone(),
                    a.profile.name.clone(),
                )
            }) else {
                return;
            };
            unsafe {
                if let Ok(m) = CreatePopupMenu() {
                    fill_proxy_menu(m, &profiles, &cur);
                    let mut rc = RECT::default();
                    let _ = GetWindowRect(btn, &mut rc);
                    let cmd = TrackPopupMenu(
                        m,
                        TPM_RETURNCMD | TPM_RIGHTALIGN,
                        rc.right,
                        rc.bottom,
                        None,
                        frame,
                        None,
                    );
                    let _ = DestroyMenu(m);
                    if cmd.0 != 0 {
                        command(cmd.0 as u16);
                    }
                }
            }
        }
        ID_PROXY_SETTINGS => proxy_settings(),
        ID_SETTINGS => {
            if let Some(p) = Config::default_path() {
                if !p.exists() {
                    if let Some(d) = p.parent() {
                        let _ = std::fs::create_dir_all(d);
                    }
                    let _ = std::fs::write(&p, Config::default_file_contents());
                }
                let target = HSTRING::from(p.as_os_str());
                unsafe {
                    windows::Win32::UI::Shell::ShellExecuteW(
                        None,
                        w!("open"),
                        &target,
                        None,
                        None,
                        SW_SHOWNORMAL,
                    );
                }
            }
        }
        ID_HELP => {
            let _ = crate::help::show(None);
        }
        ID_ABOUT => {
            if let Some(f) = with(|a| a.frame) {
                info_box(
                    f,
                    &format!(
                        "yybrowser {}\n\nOS とは別のプロキシを指定できるタブブラウザ（表示は WebView2）。",
                        env!("CARGO_PKG_VERSION")
                    ),
                );
            }
        }
        ID_EXIT => {
            if let Some(f) = with(|a| a.frame) {
                unsafe {
                    let _ = PostMessageW(Some(f), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
        }
        _ if (ID_PROFILE_BASE..ID_PROFILE_BASE + 90).contains(&id) => {
            let name = with(|a| {
                a.profiles
                    .profiles
                    .get((id - ID_PROFILE_BASE) as usize)
                    .map(|p| p.name.clone())
            })
            .flatten();
            if let Some(n) = name {
                switch_profile(&n);
            }
        }
        _ if (ID_WINDOW_BASE..ID_WINDOW_BASE + 90).contains(&id) => {
            let name = with(|a| {
                a.profiles
                    .profiles
                    .get((id - ID_WINDOW_BASE) as usize)
                    .map(|p| p.name.clone())
            })
            .flatten();
            if let Some(n) = name {
                new_window(Some(&n));
            }
        }
        _ => {}
    }
}

extern "system" fn frame_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            with(|a| a.layout());
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as u16;
            command(id);
            LRESULT(0)
        }
        WM_NOTIFY => {
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if hdr.idFrom == ID_TABS as usize && hdr.code == TCN_SELCHANGE {
                let i = with(|a| unsafe { SendMessageW(a.tabs_hwnd, TCM_GETCURSEL, None, None).0 })
                    .unwrap_or(0);
                if i >= 0 {
                    select_tab(i as usize);
                }
            }
            LRESULT(0)
        }
        crate::tabclose::WM_APP_CLOSE_TAB => {
            close_tab(wparam.0);
            LRESULT(0)
        }
        crate::tabclose::WM_APP_TAB_MENU => {
            let n = with(|a| a.tabs.len()).unwrap_or(0);
            if let Some(choice) = crate::tabclose::menu(hwnd, wparam.0, n) {
                // 後ろから閉じる（番号がずれないように）
                for i in choice.targets(wparam.0, n).into_iter().rev() {
                    close_tab(i);
                }
            }
            LRESULT(0)
        }
        WM_APP_ENV_READY => {
            on_env_ready(wparam.0 as u64);
            LRESULT(0)
        }
        WM_APP_OPEN_TAB => {
            let url = unsafe { Box::from_raw(lparam.0 as *mut String) };
            new_tab(Some(&url));
            LRESULT(0)
        }
        WM_APP_SHORTCUT => {
            shortcut(wparam.0 as u16, lparam.0 as u8);
            LRESULT(0)
        }
        WM_INITMENUPOPUP if wparam.0 as isize == AB_MENU.with(|c| c.get()) => {
            let m = HMENU(wparam.0 as *mut _);
            unsafe {
                while GetMenuItemCount(Some(m)) > 0 {
                    let _ = DeleteMenu(m, 0, MF_BYPOSITION);
                }
            }
            with(|a| fill_adblock_menu(m, a));
            LRESULT(0)
        }
        WM_INITMENUPOPUP if wparam.0 as isize == BM_MENU.with(|c| c.get()) => {
            let m = HMENU(wparam.0 as *mut _);
            let list = bookmarkui::load();
            unsafe {
                while GetMenuItemCount(Some(m)) > 0 {
                    let _ = DeleteMenu(m, 0, MF_BYPOSITION);
                }
                let _ = AppendMenuW(
                    m,
                    MF_STRING,
                    ID_BM_ADD as usize,
                    w!("このページをブックマーク(&A)...\tCtrl+D"),
                );
                let _ = AppendMenuW(
                    m,
                    MF_STRING,
                    ID_BM_MANAGE as usize,
                    w!("ブックマークの管理(&M)...\tCtrl+Shift+O"),
                );
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
            }
            bookmarkui::fill_menu(m, &list, ID_BM_BASE);
            with(|a| a.bookmarks = list);
            LRESULT(0)
        }
        adblock::WM_APP_ADBLOCK => {
            let m = unsafe { Box::from_raw(lparam.0 as *mut adblock::AdMsg) };
            on_adblock_msg(*m);
            LRESULT(0)
        }
        WM_APP_BADGE => {
            with(|a| {
                a.badge_visible = wparam.0 != 0;
                a.layout();
            });
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = (wparam.0 & 0xffff) as u32;
            with(|a| a.dpi = dpi.max(96));
            let rc = unsafe { &*(lparam.0 as *const RECT) };
            unsafe {
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
            LRESULT(0)
        }
        WM_MOVE | WM_MOVING => {
            // WebView2 のポップアップ（候補の一覧など）の位置を合わせる
            let cs: Vec<ICoreWebView2Controller> =
                with(|a| a.tabs.iter().filter_map(|t| t.controller.clone()).collect())
                    .unwrap_or_default();
            for c in cs {
                unsafe {
                    let _ = c.NotifyParentWindowPositionChanged();
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_command_line() {
        let list = ProfileList::default();
        let (p, urls) = parse_args(&["https://example.com".into()], &list).unwrap();
        assert_eq!(p.name, list.startup().name);
        assert_eq!(urls, ["https://example.com"]);
        let (p, _) = parse_args(&["--profile".into(), "直接".into()], &list).unwrap();
        assert_eq!(p.mode, yy_browser::ProxyMode::Direct);
        let (p, _) =
            parse_args(&["--proxy".into(), "socks5://127.0.0.1:1080".into()], &list).unwrap();
        assert_eq!(p.server, "socks5://127.0.0.1:1080");
        assert!(parse_args(&["--profile".into(), "ない".into()], &list).is_err());
        assert!(parse_args(&["--proxy".into()], &list).is_err());
    }

    /// プロファイルの起動引数で作った WebView2 の環境でページを開き、読み込みが終わるまで待つ。
    /// 読み込めたかを返す（WebView2 を使えなければ `None`）。
    fn open_with(
        profile: &ProxyProfile,
        folder: &std::path::Path,
        parent: HWND,
        url: &str,
    ) -> Option<bool> {
        use std::rc::Rc;
        use std::time::Duration;
        let create = crate::preview::create_environment_fn().ok()?;
        let options = CoreWebView2EnvironmentOptions::default();
        unsafe {
            options.set_additional_browser_arguments(profile.browser_args().unwrap());
        }
        let options: ICoreWebView2EnvironmentOptions = options.into();
        let result: Rc<RefCell<Option<bool>>> = Rc::default();
        let failed: Rc<std::cell::Cell<bool>> = Rc::default();
        let (r, f) = (result.clone(), failed.clone());
        let url = url.to_owned();
        let profile = profile.clone();
        let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
            move |res: Result<()>, env: Option<ICoreWebView2Environment>| {
                let Some(env) = env.filter(|_| res.is_ok()) else {
                    f.set(true);
                    return Ok(());
                };
                let (r, f, url, profile) = (r.clone(), f.clone(), url.clone(), profile.clone());
                let on_controller = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                    move |res: Result<()>, c: Option<ICoreWebView2Controller>| {
                        let Some(c) = c.filter(|_| res.is_ok()) else {
                            f.set(true);
                            return Ok(());
                        };
                        let w = unsafe { c.CoreWebView2()? };
                        let r2 = r.clone();
                        let mut token = 0i64;
                        let profile = profile.clone();
                        unsafe {
                            // 本物と同じ見立てで、開発者用証明書なら許す
                            w.cast::<ICoreWebView2_14>()?.add_ServerCertificateErrorDetected(
                                &ServerCertificateErrorDetectedEventHandler::create(Box::new(
                                    move |_, args| {
                                        if let Some(args) = args {
                                            let (uri, pem) = cert_error_parts(&args);
                                            if let DevCertCheck::Match(..) =
                                                check_dev_cert(&profile, &uri, &pem)
                                            {
                                                args.SetAction(
                                                    COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW,
                                                )?;
                                            }
                                        }
                                        Ok(())
                                    },
                                )),
                                &mut token,
                            )?;
                            w.add_NavigationCompleted(
                                &NavigationCompletedEventHandler::create(Box::new(
                                    move |_, args| {
                                        let mut ok = windows::core::BOOL(0);
                                        if let Some(a) = args {
                                            let _ = a.IsSuccess(&mut ok);
                                        }
                                        *r2.borrow_mut() = Some(ok.as_bool());
                                        Ok(())
                                    },
                                )),
                                &mut token,
                            )?;
                        }
                        // 環境・表示を手放さないよう、終わるまで持っておく
                        std::mem::forget(c);
                        unsafe { w.Navigate(&HSTRING::from(url.as_str()))? };
                        Ok(())
                    },
                ));
                unsafe {
                    env.CreateCoreWebView2Controller(parent, &on_controller)?;
                }
                std::mem::forget(env);
                Ok(())
            },
        ));
        let folder_w = HSTRING::from(folder.as_os_str());
        let hr = unsafe {
            create(
                PCWSTR::null(),
                PCWSTR(folder_w.as_ptr()),
                options.as_raw(),
                handler.as_raw(),
            )
        };
        if hr.is_err() {
            return None;
        }
        crate::preview::testing::pump_until(Duration::from_secs(60), || {
            result.borrow().is_some() || failed.get()
        });
        if failed.get() {
            return None;
        }
        *result.borrow()
    }

    /// 「指定」のプロファイルでは要求が試験用のプロキシを通り、「直接」では通らないことを確かめる。
    /// WebView2 ランタイムがなければ飛ばす（`YY_REQUIRE_WEBVIEW2=1` なら失敗にする）。
    #[test]
    fn pages_go_through_the_profile_proxy_in_webview2() {
        use std::io::{Read, Write};
        use std::sync::{Arc, Mutex};
        let required = std::env::var_os("YY_REQUIRE_WEBVIEW2").is_some();
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        // 試験用の HTTP プロキシ: 要求の 1 行目を覚えて、小さなページを返す
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let text = String::from_utf8_lossy(&buf);
                if let Some(line) = text.lines().next() {
                    seen2.lock().unwrap().push(line.to_owned());
                }
                let body = "<html><head><title>via proxy</title></head><body>ok</body></html>";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let parent = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("yybrowser proxy test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                640,
                480,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        };
        let dir = std::env::temp_dir().join(format!("yybrowser-test-{}", std::process::id()));
        let url = "http://yybrowser-proxy-test.invalid/hello";
        let manual = ProxyProfile {
            name: "test proxy".into(),
            mode: yy_browser::ProxyMode::Manual,
            server: format!("127.0.0.1:{port}"),
            ..ProxyProfile::default()
        };
        let Some(ok) = open_with(&manual, &dir.join(manual.data_folder_name()), parent, url) else {
            assert!(!required, "WebView2 を使えません");
            eprintln!("WebView2 を使えないので飛ばします");
            return;
        };
        assert!(
            ok,
            "プロキシ経由で読み込めませんでした: {:?}",
            seen.lock().unwrap()
        );
        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .any(|l| l.starts_with("GET http://yybrowser-proxy-test.invalid/hello")),
            "{:?}",
            seen.lock().unwrap()
        );
        // 直接: 存在しないホストなので読み込めず、試験用のプロキシにも来ない。
        // 先のページの後追いの要求（favicon など）が届くことがあるので、件数ではなく別の URL で見分ける
        let direct_url = "http://yybrowser-direct-test.invalid/hello";
        let direct = ProxyProfile::new("direct", yy_browser::ProxyMode::Direct);
        let ok = open_with(
            &direct,
            &dir.join(direct.data_folder_name()),
            parent,
            direct_url,
        )
        .unwrap();
        assert!(!ok);
        assert!(
            !seen
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.contains("yybrowser-direct-test")),
            "{:?}",
            seen.lock().unwrap()
        );
    }

    /// 試験用の HTTP プロキシ（要求の 1 行目を覚えて、小さなページを返す）。（ポート, 覚えた行）
    fn test_proxy() -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                if let Some(line) = String::from_utf8_lossy(&buf).lines().next() {
                    seen2.lock().unwrap().push(line.to_owned());
                }
                let body = "<html><head><title>via proxy</title></head><body>ok</body></html>";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (port, seen)
    }

    /// ドメインごとのプロキシ（PAC を data: の URL で渡す）・ホストの転送（ポートも替える）・
    /// 開発者用証明書（指紋が一致したときだけ許す）を、本物の WebView2 で確かめる。
    #[test]
    fn rules_host_maps_and_dev_certificates_in_webview2() {
        use std::io::{Read, Write};
        use std::sync::{Arc, Mutex};
        let required = std::env::var_os("YY_REQUIRE_WEBVIEW2").is_some();
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        // 開発者用証明書で TLS を話す試験用のサーバー（転送先）
        let host = "www.yybrowser-devcert.test";
        let cert = yy_browser::rules::generate_dev_cert(host).unwrap();
        let config = {
            use rustls::pki_types::pem::PemObject;
            use rustls::pki_types::{CertificateDer, PrivateKeyDer};
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let certs = vec![CertificateDer::from_pem_slice(cert.cert_pem.as_bytes()).unwrap()];
            let key = PrivateKeyDer::from_pem_slice(cert.key_pem.as_bytes()).unwrap();
            Arc::new(
                rustls::ServerConfig::builder_with_provider(provider)
                    .with_safe_default_protocol_versions()
                    .unwrap()
                    .with_no_client_auth()
                    .with_single_cert(certs, key)
                    .unwrap(),
            )
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let tls_port = listener.local_addr().unwrap().port();
        let hosts_seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let hs = hosts_seen.clone();
        std::thread::spawn(move || {
            for tcp in listener.incoming().flatten() {
                let config = config.clone();
                let hs = hs.clone();
                std::thread::spawn(move || {
                    let Ok(conn) = rustls::ServerConnection::new(config) else {
                        return;
                    };
                    let mut s = rustls::StreamOwned::new(conn, tcp);
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match s.read(&mut chunk) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(h) = text.lines().find_map(|l| {
                        l.strip_prefix("Host: ")
                            .or_else(|| l.strip_prefix("host: "))
                    }) {
                        hs.lock().unwrap().push(h.trim().to_owned());
                    }
                    let body = "<html><head><title>dev</title></head><body>dev cert</body></html>";
                    let _ = write!(
                        s,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = s.flush();
                    s.conn.send_close_notify();
                    let _ = s.flush();
                });
            }
        });
        let (proxy_port, proxy_seen) = test_proxy();
        let parent = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("yybrowser rules test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                640,
                480,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        };
        let dir = std::env::temp_dir().join(format!("yybrowser-rules-{}", std::process::id()));
        let mut profile = ProxyProfile::new("rules", yy_browser::ProxyMode::Direct);
        profile.rules = yy_browser::rules::parse_rules(&format!(
            "yybrowser-rule-test.invalid = 127.0.0.1:{proxy_port}"
        ))
        .unwrap();
        profile.hosts = vec![yy_browser::HostMap {
            host: host.into(),
            address: format!("127.0.0.1:{tls_port}"),
            cert_sha256: cert.sha256.clone(),
        }];
        // 転送 + 開発者用証明書: https://www.yybrowser-devcert.test/ が 127.0.0.1:<port> に届く
        let url = format!("https://{host}/");
        let Some(ok) = open_with(&profile, &dir.join("a"), parent, &url) else {
            assert!(!required, "WebView2 を使えません");
            eprintln!("WebView2 を使えないので飛ばします");
            return;
        };
        assert!(
            ok,
            "開発者用証明書のサーバーを開けません: {:?}",
            hosts_seen.lock().unwrap()
        );
        assert!(
            hosts_seen.lock().unwrap().iter().any(|h| h == host),
            "{:?}",
            hosts_seen.lock().unwrap()
        );
        // 規則（PAC）: 当てはまるホストは試験用のプロキシを通る
        let ok = open_with(
            &profile,
            &dir.join("b"),
            parent,
            "http://yybrowser-rule-test.invalid/rule",
        )
        .unwrap();
        assert!(
            ok,
            "規則のプロキシを通りません: {:?}",
            proxy_seen.lock().unwrap()
        );
        assert!(
            proxy_seen
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.starts_with("GET http://yybrowser-rule-test.invalid/rule")),
            "{:?}",
            proxy_seen.lock().unwrap()
        );
        // 当てはまらないホストは直接（存在しないので読み込めず、プロキシにも来ない）
        let ok = open_with(
            &profile,
            &dir.join("c"),
            parent,
            "http://yybrowser-other-test.invalid/",
        )
        .unwrap();
        assert!(!ok);
        assert!(
            !proxy_seen
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.contains("yybrowser-other-test")),
            "{:?}",
            proxy_seen.lock().unwrap()
        );
        // 指紋が違えば許さない
        let mut wrong = profile.clone();
        wrong.hosts[0].cert_sha256 = vec!["00"; 32].join(":");
        let ok = open_with(&wrong, &dir.join("d"), parent, &url).unwrap();
        assert!(!ok, "指紋が違うのに開けました");
    }

    #[test]
    fn checks_dev_certificates() {
        let cert = yy_browser::rules::generate_dev_cert("dev.example").unwrap();
        let mut p = ProxyProfile::new("p", yy_browser::ProxyMode::Direct);
        p.hosts = yy_browser::rules::parse_hosts(&format!(
            "dev.example = 127.0.0.1:8443 cert={}\nplain.example = 127.0.0.1:9443",
            cert.sha256
        ))
        .unwrap();
        assert_eq!(
            check_dev_cert(&p, "https://dev.example/x", &cert.cert_pem),
            DevCertCheck::Match("dev.example".into(), cert.sha256.clone())
        );
        let other = yy_browser::rules::generate_dev_cert("dev.example").unwrap();
        assert!(matches!(
            check_dev_cert(&p, "https://dev.example/", &other.cert_pem),
            DevCertCheck::Mismatch(..)
        ));
        assert_eq!(
            check_dev_cert(&p, "https://plain.example/", &cert.cert_pem),
            DevCertCheck::NotPinned
        );
        assert_eq!(
            check_dev_cert(&p, "https://elsewhere.example/", &cert.cert_pem),
            DevCertCheck::NotPinned
        );
    }

    /// 広告ブロック: 試験の中で立てた HTTP サーバーのページで、規則に当てはまる画像の要求が届かないこと・
    /// ホスト向けの規則と汎用の規則（class・id を集めるスクリプト経由）で要素が隠れることを、本物の
    /// WebView2 で確かめる（本物と同じ部品 `adblock::decide`・`on_dom_loaded`・`on_message` を使う）。
    #[test]
    fn blocks_ads_and_hides_elements_in_webview2() {
        use std::io::{Read, Write};
        use std::rc::Rc;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;
        let required = std::env::var_os("YY_REQUIRE_WEBVIEW2").is_some();
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let paths: Arc<Mutex<Vec<String>>> = Arc::default();
        let p2 = paths.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let text = String::from_utf8_lossy(&buf).to_string();
                let path = text
                    .lines()
                    .next()
                    .and_then(|l| l.split(' ').nth(1))
                    .unwrap_or("")
                    .to_owned();
                p2.lock().unwrap().push(path.clone());
                let (ctype, body): (&str, Vec<u8>) = if path == "/" {
                    (
                        "text/html",
                        b"<html><head><title>ads</title></head><body>\
                          <img src=\"/ads/banner.png\"><img src=\"/img/ok.png\">\
                          <div class=\"ad-box\">ad</div><div class=\"side-ad\">side</div>\
                          <div class=\"content\">content</div></body></html>"
                            .to_vec(),
                    )
                } else {
                    ("image/gif", b"GIF89a\x01\x00\x01\x00\x00\x00\x00;".to_vec())
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        let blocker = Arc::new(yy_adblock::AdBlocker::build(vec![(
            "test".into(),
            "/ads/banner.png\n127.0.0.1##.side-ad\n##.ad-box\n".into(),
        )]));
        let parent = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("yybrowser adblock test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                640,
                480,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        };
        let url = format!("http://127.0.0.1:{port}/");
        let folder = std::env::temp_dir().join(format!("yybrowser-adblock-{}", std::process::id()));
        let Ok(create) = crate::preview::create_environment_fn() else {
            assert!(!required, "WebView2 を使えません");
            return;
        };
        let options: ICoreWebView2EnvironmentOptions =
            CoreWebView2EnvironmentOptions::default().into();
        let web: Rc<RefCell<Option<ICoreWebView2>>> = Rc::default();
        let done: Rc<std::cell::Cell<bool>> = Rc::default();
        let failed: Rc<std::cell::Cell<bool>> = Rc::default();
        let (w1, d1, f1, b1, u1) = (
            web.clone(),
            done.clone(),
            failed.clone(),
            blocker.clone(),
            url.clone(),
        );
        let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
            move |res: Result<()>, env: Option<ICoreWebView2Environment>| {
                let Some(env) = env.filter(|_| res.is_ok()) else {
                    f1.set(true);
                    return Ok(());
                };
                let (w1, d1, f1, b1, u1) =
                    (w1.clone(), d1.clone(), f1.clone(), b1.clone(), u1.clone());
                let env2 = env.clone();
                let on_controller = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                    move |res: Result<()>, c: Option<ICoreWebView2Controller>| {
                        let Some(c) = c.filter(|_| res.is_ok()) else {
                            f1.set(true);
                            return Ok(());
                        };
                        let w = unsafe { c.CoreWebView2()? };
                        let mut token = 0i64;
                        let page: Rc<RefCell<Option<yy_adblock::PageCosmetic>>> = Rc::default();
                        adblock::set_request_filter(&w, true);
                        let (b2, u2, e2) = (b1.clone(), u1.clone(), env2.clone());
                        let (b3, u3, pg3) = (b1.clone(), u1.clone(), page.clone());
                        let (b4, pg4) = (b1.clone(), page.clone());
                        let d2 = d1.clone();
                        unsafe {
                            w.add_WebResourceRequested(
                                &WebResourceRequestedEventHandler::create(Box::new(
                                    move |_, args| {
                                        if let Some(args) = args {
                                            let uri = take_string(|p| args.Request()?.Uri(p));
                                            let mut ctx =
                                                COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
                                            let _ = args.ResourceContext(&mut ctx);
                                            if adblock::decide(&b2, &u2, &uri, ctx) {
                                                adblock::block(&e2, &args);
                                            }
                                        }
                                        Ok(())
                                    },
                                )),
                                &mut token,
                            )?;
                            w.cast::<ICoreWebView2_2>()?.add_DOMContentLoaded(
                                &DOMContentLoadedEventHandler::create(Box::new(
                                    move |sender, _| {
                                        if let Some(w) = sender {
                                            *pg3.borrow_mut() =
                                                Some(adblock::on_dom_loaded(&b3, &w, &u3));
                                        }
                                        Ok(())
                                    },
                                )),
                                &mut token,
                            )?;
                            w.add_WebMessageReceived(
                                &WebMessageReceivedEventHandler::create(Box::new(
                                    move |sender, args| {
                                        if let (Some(w), Some(args)) = (sender, args) {
                                            let msg =
                                                take_string(|p| args.TryGetWebMessageAsString(p));
                                            if let Some(page) = pg4.borrow().as_ref() {
                                                adblock::on_message(&b4, page, &w, &msg);
                                            }
                                        }
                                        Ok(())
                                    },
                                )),
                                &mut token,
                            )?;
                            w.add_NavigationCompleted(
                                &NavigationCompletedEventHandler::create(Box::new(move |_, _| {
                                    d2.set(true);
                                    Ok(())
                                })),
                                &mut token,
                            )?;
                        }
                        *w1.borrow_mut() = Some(w.clone());
                        std::mem::forget(c);
                        unsafe { w.Navigate(&HSTRING::from(u1.as_str()))? };
                        Ok(())
                    },
                ));
                unsafe {
                    env.CreateCoreWebView2Controller(parent, &on_controller)?;
                }
                std::mem::forget(env);
                Ok(())
            },
        ));
        let folder_w = HSTRING::from(folder.as_os_str());
        let hr = unsafe {
            create(
                PCWSTR::null(),
                PCWSTR(folder_w.as_ptr()),
                options.as_raw(),
                handler.as_raw(),
            )
        };
        if hr.is_err() {
            assert!(!required, "WebView2 を使えません");
            return;
        }
        crate::preview::testing::pump_until(Duration::from_secs(60), || done.get() || failed.get());
        if failed.get() {
            assert!(!required, "WebView2 を使えません");
            return;
        }
        let w = web.borrow().clone().expect("WebView2");
        // 汎用の規則はメッセージの往復の後で効くので、少し待ちながら確かめる
        let eval = |script: &str| -> String {
            let out: Rc<RefCell<Option<String>>> = Rc::default();
            let o = out.clone();
            let h = ExecuteScriptCompletedHandler::create(Box::new(move |_, json| {
                *o.borrow_mut() = Some(json);
                Ok(())
            }));
            unsafe {
                let _ = w.ExecuteScript(&HSTRING::from(script), &h);
            }
            crate::preview::testing::pump_until(Duration::from_secs(10), || out.borrow().is_some());
            out.borrow_mut().take().unwrap_or_default()
        };
        let probe = "['.ad-box','.side-ad','.content'].map(s=>getComputedStyle(document.querySelector(s)).display).join('|')";
        let mut got = String::new();
        for _ in 0..30 {
            got = eval(probe);
            if got == "\"none|none|block\"" {
                break;
            }
            crate::preview::testing::pump_until(Duration::from_millis(300), || false);
        }
        assert_eq!(got, "\"none|none|block\"", "要素が隠れません");
        let seen = paths.lock().unwrap().clone();
        assert!(seen.iter().any(|p| p == "/img/ok.png"), "{seen:?}");
        assert!(
            !seen.iter().any(|p| p.starts_with("/ads/")),
            "広告の要求が届きました: {seen:?}"
        );
    }
}

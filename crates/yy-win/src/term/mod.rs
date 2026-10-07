//! ターミナル（yyterm。12 章）。
//!
//! エディタと同じクレートを使う別のアプリ。画面の中身は `yy-term`、手元のシェルは ConPTY、
//! SSH は `yy-ssh`（OpenSSH を使わない）、フォントは同梱の UDEV Gothic、ワークスペースは
//! エディタと同じ `*.yyworkspace`、接続設定・ホスト鍵・保存したパスワードもエディタと共通。
//!
//! ウィンドウ構成: フレーム（メニュー・ステータスバー）の左にワークスペースのサイドバー、
//! 右に上からタブ、端末の画面（1 つのビューで選んでいるタブを描く）。
//!
//! シェルの出力は読み取り用のスレッドが受け取り、チャネルに入れてフレームに知らせる
//! （[`WM_APP_TERM_EVENT`]）。UI スレッドで端末に流し込んで描き直す。入力はタブごとの
//! 書き込み用のスレッドが送る（UI スレッドを止めない）。

mod ftdlg;
mod macros;
mod paint;
mod printer;
mod pty;
mod sidebar;
mod tn3270;

use std::cell::RefCell;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, Sender, bounded};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT, ScreenToClient,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, PWSTR, Result, w};
use yy_config::Config;
use yy_config::workspace::{self, Workspace};
use yy_remote::RemoteUri;
use yy_remote::uri::Target;
use yy_term::{Button, Key, Mods, MouseEvent, MouseMode, Pos, Terminal};

use self::paint::{Painter, Scene};
use self::pty::Backend;
use self::sidebar::{ID_TREE, Sidebar};
use crate::util::{Context, error_box, info_box};
use crate::{hiword, loword};

const FRAME_CLASS: PCWSTR = w!("YYTermFrame");
const VIEW_CLASS: PCWSTR = w!("YYTermView");

/// シェルの出力・終了が届いた
const WM_APP_TERM_EVENT: u32 = WM_APP + 60;
/// リモートのフォルダの中身を読む（`WPARAM` はツリーの世代、`LPARAM` は項目の番号）
const WM_APP_TERM_REMOTE_DIR: u32 = WM_APP + 61;
/// 端末の画面にフォーカスを移す
const WM_APP_TERM_FOCUS: u32 = WM_APP + 62;
/// 端末の画面のフォーカスが変わった（`WPARAM` が 1 なら得た）
const WM_APP_TERM_FOCUS_CHANGED: u32 = WM_APP + 63;
/// 並べ直す（大きさの変更が、状態を借りている間に届いた場合）
const WM_APP_TERM_LAYOUT: u32 = WM_APP + 64;
/// 端末と一緒に開くプリンターのタブを開く（`pending_printers`）
const WM_APP_TERM_PRINTER: u32 = WM_APP + 65;
/// プリンターのジョブの区切り（PRINT-EOJ のないホスト）を見るタイマー
const TIMER_PRINTER: usize = 0x3287;
/// マクロのスレッドからの頼みごとがある
const WM_APP_TERM_MACRO: u32 = WM_APP + 66;
/// マクロがロック中に入力しようとした
const LOCKED: &str =
    "キーボードがロックされています（wait_unlocked() でホストの応答を待ってから入力してください）";

const ID_TABS: u16 = 2001;
const ID_VIEW: u16 = 2002;
const ID_STATUS: u16 = 2003;

// メニュー
const ID_NEW_TAB: u16 = 3001;
const ID_SSH: u16 = 3002;
const ID_CLOSE_TAB: u16 = 3003;
const ID_EXIT: u16 = 3004;
const ID_TN3270: u16 = 3005;
const ID_TN_TRANSFER: u16 = 3006;
const ID_TN_CANCEL: u16 = 3007;
const ID_TN_PRINTER: u16 = 3050;
const ID_PR_PRINT: u16 = 3051;
const ID_PR_PDF: u16 = 3052;
const ID_PR_TEXT: u16 = 3053;
const ID_MACRO_RUN: u16 = 3060;
const ID_MACRO_STOP: u16 = 3061;
const ID_MACRO_RECORD: u16 = 3062;
const ID_MACRO_FOLDER: u16 = 3063;
const ID_COPY: u16 = 3010;
const ID_PASTE: u16 = 3011;
const ID_CLEAR: u16 = 3012;
const ID_SIDEBAR: u16 = 3020;
const ID_ZOOM_IN: u16 = 3021;
const ID_ZOOM_OUT: u16 = 3022;
const ID_ZOOM_RESET: u16 = 3023;
const ID_NEXT_TAB: u16 = 3024;
const ID_PREV_TAB: u16 = 3025;
const ID_WS_NEW: u16 = 3030;
const ID_WS_OPEN: u16 = 3031;
const ID_WS_SAVE_AS: u16 = 3032;
const ID_WS_ADD: u16 = 3033;
const ID_WS_ADD_REMOTE: u16 = 3034;
const ID_WS_USE_AGENT: u16 = 3035;
const ID_HELP_KEYS: u16 = 3040;
const ID_SETTINGS: u16 = 3041;
const ID_REMOTE_LOG: u16 = 3042;
const ID_FORGET: u16 = 3043;
const ID_ABOUT: u16 = 3044;

// サイドバーの右クリックのメニュー
const CM_OPEN_HERE: u32 = 1;
const CM_OPEN_EDITOR: u32 = 2;
const CM_COPY_PATH: u32 = 3;
const CM_REFRESH: u32 = 4;
const CM_REMOVE: u32 = 5;
const CM_ADD: u32 = 6;
const CM_ADD_REMOTE: u32 = 7;

/// サイドバーの幅（96 DPI でのピクセル）
const SIDEBAR_WIDTH: i32 = 240;

/// シェルを動かす場所。
#[derive(Clone, Debug)]
enum Place {
    /// 手元（フォルダ）
    Local(Option<PathBuf>),
    /// SSH の接続先（フォルダ。`None` ならホーム）
    Remote {
        target: Target,
        dir: Option<Vec<u8>>,
    },
    /// 3270 のホスト（14 章）
    Tn3270(tn3270::Target3270),
}

impl Place {
    fn label(&self) -> String {
        match self {
            Place::Local(Some(d)) => workspace::name_of(d),
            Place::Local(None) => "ターミナル".into(),
            Place::Remote { target, dir: None } => target.to_string(),
            Place::Remote {
                target,
                dir: Some(d),
            } => format!(
                "{} [{target}]",
                yy_proto::display_path(yy_proto::file_name(d))
            ),
            Place::Tn3270(t) if t.printer => match t.lu.as_deref().or(t.associate.as_deref()) {
                Some(lu) => format!("{} [3287 {lu}]", t.label),
                None => format!("{} [3287]", t.label),
            },
            Place::Tn3270(t) => match &t.lu {
                Some(lu) => format!("{} [3270 {lu}]", t.label),
                None => format!("{} [3270]", t.label),
            },
        }
    }
}

/// 読み取り用のスレッドからの知らせ。
enum Event {
    Output(u64, Vec<u8>),
    Exit(u64, Option<u32>),
    /// ポートフォワーディングの中の出来事（その接続先のタブに表示する）
    Notice(Target, String),
}

/// ポートフォワーディングの知らせをタブに表示する形（OpenSSH が端末に出す警告のように）。
fn notice_bytes(ok: bool, text: &str) -> Vec<u8> {
    let color = if ok { "36" } else { "33" };
    format!("\x1b[{color}m[yyterm] {text}\x1b[0m\r\n").into_bytes()
}

/// 1 つのタブ（1 つのシェル）。
struct Tab {
    id: u64,
    term: Terminal,
    /// さかのぼって表示している行数（0 なら最新）
    back: usize,
    /// スクロールバックに送られた行の数（表示位置を保つのに使う）
    seen_pushed: u64,
    place: Place,
    /// 選択の始点と終点
    anchor: Option<Pos>,
    selection: Option<(Pos, Pos)>,
    selecting: bool,
    /// マウスの報告で押しているボタンと、最後に報告したセル
    mouse_button: Option<Button>,
    mouse_cell: (usize, usize),
    input: Sender<Vec<u8>>,
    resize: Arc<dyn Fn(u16, u16) + Send + Sync>,
    kill: Arc<dyn Fn() + Send + Sync>,
    /// 終わった（終了コード）
    exited: Option<Option<u32>>,
    size: (u16, u16),
    /// 3270 のタブ（`term` は 3270 の画面を写したもの）
    tn: Option<Box<tn3270::Tn3270>>,
}

impl Tab {
    fn title(&self) -> String {
        let t = self.term.title().trim();
        let s = if t.is_empty() {
            self.place.label()
        } else {
            t.to_owned()
        };
        let mut out: String = s.chars().take(40).collect();
        if s.chars().count() > 40 {
            out.push('…');
        }
        out
    }

    fn send(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            let _ = self.input.send(bytes);
        }
    }
}

struct TermApp {
    frame: HWND,
    view: HWND,
    tabbar: HWND,
    status: HWND,
    sidebar: Sidebar,
    painter: Painter,
    config: Config,
    shell: Vec<String>,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    pending: Arc<AtomicBool>,
    high_surrogate: Option<u16>,
    /// 端末の大きさ（桁, 行）
    grid: (usize, usize),
    /// 表示しているステータスの案内
    status_text: String,
    /// 3270 のキーパッド（ホストにしかないキーのボタン）
    keypad: tn3270::Keypad,
    /// 3270 の接続の記録（`logs\tn3270.log`）
    tn_log: Option<std::sync::Arc<yy_remote::log::TransferLog>>,
    /// 3270 のファイル転送の記録（`logs\tn3270-transfer.log`）
    ft_log: Option<std::sync::Arc<yy_remote::log::TransferLog>>,
    /// 前回のファイル転送の指定（ダイアログの初期値）
    ft_last: Option<ftdlg::Choice>,
    /// 端末と一緒に開くプリンター（`WM_APP_TERM_PRINTER` で開く）
    pending_printers: Vec<tn3270::Target3270>,
    /// 受け取った印刷の通し番号
    print_jobs: usize,
    /// 実行中のマクロ（同時に 1 つ）
    macro_run: Option<macros::MacroRun>,
    /// 操作の記録（タブの ID と記録）
    recorder: Option<(u64, yy_3270_macro::Recorder)>,
    /// マクロの記録（`logs\tn3270-macro.log`）
    macro_log: Option<std::sync::Arc<yy_remote::log::TransferLog>>,
}

thread_local! {
    static APP: RefCell<Option<TermApp>> = const { RefCell::new(None) };
    /// フレームのウィンドウ（状態を借りられないときの知らせ先）
    static FRAME: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
    /// エラーをメッセージボックスで知らせない（テスト）
    static QUIET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 状態を借りられなかった処理を後でやり直すよう、フレームに頼む。
fn retry_later(msg: u32) {
    let frame = FRAME.with(|f| f.get());
    if frame != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(frame as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

/// アプリの状態を使う（モーダルな処理の間は借りたままにしないこと）。
fn with<R>(f: impl FnOnce(&mut TermApp) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

/// ステータスバーの案内（接続の進みなど）。空なら端末の大きさを表示する。
fn set_status(text: &str) {
    with(|a| {
        a.status_text = text.to_owned();
        a.update_status();
    });
}

/// ターミナルを起動し、ウィンドウが閉じられるまでメッセージループを回す。
///
/// `initial` はコマンドラインの引数（フォルダ、`ssh://接続先/パス`、`ユーザー@ホスト`）。
/// `ssh` は SSH の接続の実装（なければ SSH は使えない）。
pub fn run_terminal(
    initial: Option<String>,
    ssh: Option<yy_remote::ConnectorFactory>,
) -> Result<()> {
    crate::util::set_app_name("yyterm");
    let r = run_inner(initial, ssh);
    if let Err(e) = &r {
        error_box(
            HWND::default(),
            &format!("起動できませんでした。\n{}", crate::util::describe_error(e)),
        );
    }
    r
}

fn run_inner(initial: Option<String>, ssh: Option<yy_remote::ConnectorFactory>) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .context("CoInitializeEx")?;
    }
    create(initial_place(initial.as_deref()), ssh, None)?;
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_KEYDOWN && shortcut(&msg) {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    shutdown();
    Ok(())
}

/// 残っているシェルを終わらせ、状態を捨てる。
fn shutdown() {
    let tabs = with(|a| std::mem::take(&mut a.tabs)).unwrap_or_default();
    for t in tabs {
        (t.kill)();
    }
    APP.with(|a| a.borrow_mut().take());
    FRAME.with(|f| f.set(0));
}

/// ウィンドウを作り、最初のタブを `place` で開く。`shell` は手元のシェルのコマンド
/// （`None` なら設定、テストでは指定する）。
fn create(
    place: Place,
    ssh: Option<yy_remote::ConnectorFactory>,
    shell: Option<Vec<String>>,
) -> Result<HWND> {
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES | ICC_TAB_CLASSES | ICC_TREEVIEW_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let (icon, icon_small) = crate::app_icons(instance);
        let frame_class = WNDCLASSEXW {
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
        // 同じプロセスで 2 回目（テスト）なら登録済み
        const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;
        let registered = |atom: u16| {
            if atom == 0 {
                let e = windows::core::Error::from_thread();
                if e.code() != windows::core::HRESULT::from_win32(ERROR_CLASS_ALREADY_EXISTS) {
                    return Err(e);
                }
            }
            Ok(())
        };
        registered(RegisterClassExW(&frame_class))?;
        let view_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(view_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_IBEAM)?,
            lpszClassName: VIEW_CLASS,
            ..Default::default()
        };
        registered(RegisterClassExW(&view_class))?;

        let (config, config_error) = Config::load();
        crate::font::register_gdi();
        let frame = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            FRAME_CLASS,
            w!("yyterm"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            Some(create_menu()?),
            Some(instance),
            None,
        )
        .context("CreateWindowExW(frame)")?;
        let tabbar = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            WC_TABCONTROLW,
            None,
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WINDOW_STYLE(TCS_FOCUSNEVER),
            0,
            0,
            0,
            0,
            Some(frame),
            Some(HMENU(ID_TABS as isize as *mut _)),
            Some(instance),
            None,
        )
        .context("CreateWindowExW(tabs)")?;
        // エディタと同じ閉じるボタン（「×」・中ボタン）と右クリックのメニュー
        crate::tabclose::install(tabbar, frame);
        let dpi = GetDpiForWindow(frame).max(96);
        let font = crate::util::ui_font(dpi);
        SendMessageW(
            tabbar,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
        let view = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            VIEW_CLASS,
            None,
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_TABSTOP,
            0,
            0,
            0,
            0,
            Some(frame),
            Some(HMENU(ID_VIEW as isize as *mut _)),
            Some(instance),
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
            Some(instance),
            None,
        )
        .context("CreateWindowExW(status)")?;
        let sidebar = Sidebar::create(frame, instance)?;
        let keypad = tn3270::Keypad::create(
            frame,
            instance,
            crate::util::ui_font(GetDpiForWindow(frame).max(96)),
        );

        let family = if config.terminal.font_family.trim().is_empty() {
            config.editor.font_family.clone()
        } else {
            config.terminal.font_family.clone()
        };
        let size = if config.terminal.font_size > 0.0 {
            config.terminal.font_size
        } else {
            config.editor.font_size
        };
        let painter = Painter::new(&family, size, GetDpiForWindow(view).max(96))?;
        crate::remote::install(
            crate::remote::RemoteState::new(ssh, config.remote.clone()),
            crate::remote::Host {
                frame,
                status: set_status,
            },
        );
        // ワークスペースのリモートのフォルダの一覧にエージェントを使うか（設定。メニューで切り替える）。
        // 使わなければ SFTP（接続先に何も置かない）。シェルにはどちらでもエージェントを使わない
        crate::remote::set_use_agent(config.terminal.use_agent);
        check_use_agent(frame);
        let shell = if let Some(s) = shell {
            s
        } else if config.terminal.shell.is_empty() {
            pty::default_shell()
        } else {
            config.terminal.shell.clone()
        };
        let (tx, rx) = bounded(1024);
        let app = TermApp {
            frame,
            view,
            tabbar,
            status,
            sidebar,
            painter,
            config,
            shell,
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            tx,
            rx,
            pending: Arc::new(AtomicBool::new(false)),
            high_surrogate: None,
            grid: (80, 24),
            status_text: String::new(),
            keypad,
            tn_log: None,
            ft_log: None,
            ft_last: None,
            pending_printers: Vec::new(),
            print_jobs: 0,
            macro_run: None,
            recorder: None,
            macro_log: None,
        };
        APP.with(|a| *a.borrow_mut() = Some(app));
        FRAME.with(|f| f.set(frame.0 as isize));
        // 端末を 100 × 30 程度の大きさで開く
        with(|a| a.initial_size());
        layout();
        let _ = ShowWindow(frame, SW_SHOWDEFAULT);
        let _ = SetFocus(Some(view));
        if let Some(e) = config_error {
            error_box(
                frame,
                &format!("設定ファイルを読めませんでした（既定値で起動します）。\n{e}"),
            );
        }
        open_tab(place);
        Ok(frame)
    }
}

/// コマンドラインの引数から、最初のタブの場所を決める。
fn initial_place(arg: Option<&str>) -> Place {
    let home = || std::env::var_os("USERPROFILE").map(PathBuf::from);
    let Some(a) = arg.map(str::trim).filter(|a| !a.is_empty()) else {
        return Place::Local(home());
    };
    if let Some(u) = RemoteUri::parse(a) {
        return Place::Remote {
            target: u.target(),
            dir: Some(u.path),
        };
    }
    let p = Path::new(a);
    if p.is_dir() {
        return Place::Local(Some(p.to_owned()));
    }
    if !a.contains(['\\', '/'])
        && let Some(t) = Target::parse(a)
    {
        return Place::Remote {
            target: t,
            dir: None,
        };
    }
    Place::Local(home())
}

fn create_menu() -> Result<HMENU> {
    unsafe {
        let item = |m: HMENU, id: u16, text: PCWSTR| AppendMenuW(m, MF_STRING, id as usize, text);
        let sep = |m: HMENU| AppendMenuW(m, MF_SEPARATOR, 0, None);
        let bar = CreateMenu()?;
        let file = CreatePopupMenu()?;
        item(file, ID_NEW_TAB, w!("新しいタブ(&T)\tCtrl+Shift+T"))?;
        item(file, ID_SSH, w!("SSH で接続(&S)...\tCtrl+Shift+O"))?;
        item(file, ID_TN3270, w!("3270 で接続(&M)...\tCtrl+Shift+M"))?;
        item(
            file,
            ID_TN_TRANSFER,
            w!("3270 のファイル転送(&I)...\tCtrl+Shift+I"),
        )?;
        item(file, ID_TN_CANCEL, w!("3270 のファイル転送を取り消す(&N)"))?;
        item(file, ID_TN_PRINTER, w!("3270 のプリンターを接続(&P)"))?;
        item(file, ID_PR_PRINT, w!("受け取った印刷を印刷(&R)..."))?;
        item(file, ID_PR_PDF, w!("受け取った印刷を PDF で保存(&D)..."))?;
        item(
            file,
            ID_PR_TEXT,
            w!("受け取った印刷をテキストで保存(&X)..."),
        )?;
        sep(file)?;
        item(file, ID_CLOSE_TAB, w!("タブを閉じる(&C)\tCtrl+Shift+W"))?;
        item(file, ID_EXIT, w!("終了(&X)"))?;
        let edit = CreatePopupMenu()?;
        item(edit, ID_COPY, w!("コピー(&C)\tCtrl+Shift+C"))?;
        item(edit, ID_PASTE, w!("貼り付け(&P)\tCtrl+Shift+V"))?;
        sep(edit)?;
        item(edit, ID_CLEAR, w!("スクロールバックを消去(&L)"))?;
        let view = CreatePopupMenu()?;
        item(view, ID_SIDEBAR, w!("ワークスペース(&W)\tCtrl+Shift+E"))?;
        sep(view)?;
        item(view, ID_NEXT_TAB, w!("次のタブ(&N)\tCtrl+Tab"))?;
        item(view, ID_PREV_TAB, w!("前のタブ(&P)\tCtrl+Shift+Tab"))?;
        sep(view)?;
        item(view, ID_ZOOM_IN, w!("文字を大きく(&I)\tCtrl++"))?;
        item(view, ID_ZOOM_OUT, w!("文字を小さく(&O)\tCtrl+-"))?;
        item(view, ID_ZOOM_RESET, w!("文字の大きさを戻す(&R)\tCtrl+0"))?;
        let ws = CreatePopupMenu()?;
        item(ws, ID_WS_NEW, w!("新しいワークスペース(&N)"))?;
        item(ws, ID_WS_OPEN, w!("ワークスペースを開く(&O)..."))?;
        item(ws, ID_WS_SAVE_AS, w!("名前を付けて保存(&A)..."))?;
        sep(ws)?;
        item(ws, ID_WS_ADD, w!("フォルダを追加(&F)..."))?;
        item(ws, ID_WS_ADD_REMOTE, w!("リモートのフォルダを追加(&R)..."))?;
        sep(ws)?;
        item(
            ws,
            ID_WS_USE_AGENT,
            w!("リモートのフォルダの一覧に接続先のエージェントを使う(&G)"),
        )?;
        let mac = CreatePopupMenu()?;
        item(mac, ID_MACRO_RUN, w!("マクロを実行(&R)...\tCtrl+Shift+F5"))?;
        item(mac, ID_MACRO_STOP, w!("マクロを止める(&S)\tCtrl+Break"))?;
        sep(mac)?;
        item(
            mac,
            ID_MACRO_RECORD,
            w!("操作の記録を開始・終了(&C)\tCtrl+Shift+F6"),
        )?;
        item(mac, ID_MACRO_FOLDER, w!("マクロのフォルダを開く(&F)"))?;
        let help = CreatePopupMenu()?;
        item(help, ID_HELP_KEYS, w!("キーボードショートカット(&K)"))?;
        item(help, ID_SETTINGS, w!("設定ファイルを開く(&S)"))?;
        item(help, ID_REMOTE_LOG, w!("リモート接続の記録を開く(&R)"))?;
        item(
            help,
            ID_FORGET,
            w!("保存したリモート接続のパスワードを削除(&P)..."),
        )?;
        sep(help)?;
        item(help, ID_ABOUT, w!("バージョン情報(&A)"))?;
        AppendMenuW(bar, MF_POPUP, file.0 as usize, w!("ファイル(&F)"))?;
        AppendMenuW(bar, MF_POPUP, edit.0 as usize, w!("編集(&E)"))?;
        AppendMenuW(bar, MF_POPUP, view.0 as usize, w!("表示(&V)"))?;
        AppendMenuW(bar, MF_POPUP, ws.0 as usize, w!("ワークスペース(&W)"))?;
        AppendMenuW(bar, MF_POPUP, mac.0 as usize, w!("マクロ(&A)"))?;
        AppendMenuW(bar, MF_POPUP, help.0 as usize, w!("ヘルプ(&H)"))?;
        Ok(bar)
    }
}

/// 子ウィンドウを並べる（アプリの状態を借りずに呼ぶ）。
fn layout() {
    let Some((sidebar, rects, keypad)) = with(|a| {
        let (s, r) = a.layout_rects();
        let k = a.keypad_visible();
        (s, r, k)
    }) else {
        retry_later(WM_APP_TERM_LAYOUT);
        return;
    };
    let keypad_buttons = with(|a| a.keypad.buttons.clone()).unwrap_or_default();
    unsafe {
        for b in keypad_buttons {
            let _ = ShowWindow(b, if keypad { SW_SHOW } else { SW_HIDE });
        }
        let _ = ShowWindow(rects[0].0, if sidebar { SW_SHOW } else { SW_HIDE });
        for (hwnd, r) in rects {
            let _ = MoveWindow(
                hwnd,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                true,
            );
        }
    }
}

/// 端末の画面にフォーカスを移す（アプリの状態を借りずに呼ぶ）。
fn focus_view() {
    if let Some(v) = with(|a| a.view) {
        unsafe {
            let _ = SetFocus(Some(v));
        }
    }
}

/// キーの状態。
fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(i32::from(vk.0)) < 0 }
}

fn mods() -> Mods {
    Mods {
        shift: key_down(VK_SHIFT),
        alt: key_down(VK_MENU),
        ctrl: key_down(VK_CONTROL),
    }
}

/// どのウィンドウにフォーカスがあっても使うショートカット。処理したら `true`。
fn shortcut(msg: &MSG) -> bool {
    let vk = VIRTUAL_KEY(msg.wParam.0 as u16);
    let m = mods();
    let frame = with(|a| a.frame).unwrap_or_default();
    let cmd = match (m.ctrl, m.shift, vk) {
        (true, true, VK_T) => ID_NEW_TAB,
        (true, true, VK_W) => ID_CLOSE_TAB,
        (true, true, VK_C) => ID_COPY,
        (true, true, VK_V) => ID_PASTE,
        (true, true, VK_E) => ID_SIDEBAR,
        (true, true, VK_O) => ID_SSH,
        (true, true, VK_M) => ID_TN3270,
        (true, true, VK_I) => ID_TN_TRANSFER,
        (true, true, VK_F5) => ID_MACRO_RUN,
        (true, true, VK_F6) => ID_MACRO_RECORD,
        (true, false, VK_TAB) => ID_NEXT_TAB,
        (true, true, VK_TAB) => ID_PREV_TAB,
        (true, false, VK_OEM_PLUS | VK_ADD) => ID_ZOOM_IN,
        (true, false, VK_OEM_MINUS | VK_SUBTRACT) => ID_ZOOM_OUT,
        (true, false, VK_0 | VK_NUMPAD0) => ID_ZOOM_RESET,
        (true, false, VK_INSERT) => ID_COPY,
        (false, true, VK_INSERT) => ID_PASTE,
        _ => return false,
    };
    unsafe {
        let _ = PostMessageW(Some(frame), WM_COMMAND, WPARAM(cmd as usize), LPARAM(0));
    }
    true
}

// ---- タブ ---------------------------------------------------------------------------

/// `place` で新しいタブを開く（接続中の問い合わせのため、アプリの状態を借りずに呼ぶ）。
fn open_tab(place: Place) {
    open_tab_forwarding(place, &[]);
}

/// `place` で新しいタブを開く。SSH の接続先なら、接続設定・`~/.ssh/config` と `extra` の
/// ポートフォワーディングを（その接続でまだ始めていなければ）始め、結果をタブの先頭に表示する。
/// 始められなくても警告を表示するだけで、シェルはそのまま使う。
fn open_tab_forwarding(place: Place, extra: &[yy_remote::Forward]) {
    let Some((frame, size, shell, term, tx, pending)) = with(|a| {
        (
            a.frame,
            (a.grid.0 as u16, a.grid.1 as u16),
            a.shell.clone(),
            a.config.terminal.term.clone(),
            a.tx.clone(),
            a.pending.clone(),
        )
    }) else {
        return;
    };
    let mut reports: Vec<crate::remote::ForwardReport> = Vec::new();
    let backend = match &place {
        Place::Tn3270(t) => tn3270::connect(t, &set_status)
            .map_err(|e| format!("3270 のホスト {} に接続できませんでした。\n{e}", t.uri())),
        Place::Local(dir) => {
            let dir = dir.as_deref().filter(|d| d.is_dir());
            pty::spawn_local(&shell, dir, size)
        }
        Place::Remote { target, dir } => {
            if !crate::remote::available() {
                Err("この yyterm には SSH の機能が組み込まれていません。".to_owned())
            } else {
                match crate::remote::transport(target, &set_status) {
                    Ok(t) => {
                        // フォルダを指定したときは、そこでログインシェルを起動する
                        let command = dir.as_ref().map(|d| {
                            let mut c = b"cd ".to_vec();
                            c.extend_from_slice(&yy_remote::shell_quote(d));
                            c.extend_from_slice(br#" && exec "${SHELL:-/bin/sh}" -l"#);
                            c
                        });
                        let r = t
                            .shell(&term, size, command.as_deref())
                            .map(Backend::from)
                            .map_err(|e| format!("{target} でシェルを起動できませんでした。\n{e}"));
                        set_status("");
                        if r.is_ok() {
                            // フォワーディングの中の出来事は、その接続先のタブに表示する
                            let frame_raw = frame.0 as isize;
                            let target2 = target.clone();
                            let note: yy_remote::ForwardNote = Arc::new(move |text: &str| {
                                if tx
                                    .send(Event::Notice(target2.clone(), text.to_owned()))
                                    .is_ok()
                                    && !pending.swap(true, Ordering::AcqRel)
                                {
                                    unsafe {
                                        let _ = PostMessageW(
                                            Some(HWND(frame_raw as *mut _)),
                                            WM_APP_TERM_EVENT,
                                            WPARAM(0),
                                            LPARAM(0),
                                        );
                                    }
                                }
                            });
                            reports = crate::remote::start_forwards(target, &t, extra, note);
                        }
                        r
                    }
                    Err(e) => Err(e),
                }
            }
        }
    };
    match backend {
        Ok(b) => {
            with(|a| {
                a.add_tab(place, b);
                if let Some(t) = a.tabs.last_mut() {
                    for (ok, text) in &reports {
                        t.term.feed(&notice_bytes(*ok, text));
                    }
                }
            });
            if let Some((_, w)) = reports.iter().find(|(ok, _)| !ok) {
                set_status(w);
            }
        }
        Err(e) => {
            if QUIET.with(|q| q.get()) {
                set_status(&e);
                return;
            }
            error_box(frame, &e);
            // タブがなければ閉じる
            if with(|a| a.tabs.is_empty()) == Some(true) {
                unsafe {
                    let _ = DestroyWindow(frame);
                }
            }
        }
    }
}

impl TermApp {
    fn dpi(&self) -> i32 {
        unsafe { GetDpiForWindow(self.frame).max(96) as i32 }
    }

    /// 最初の大きさ（端末が 100 × 30 程度になるように）。
    fn initial_size(&mut self) {
        let s = self.painter.dpi() / 96.0;
        let w = ((self.painter.cell_w * 100.0 + 40.0) * s) as i32
            + if self.sidebar.visible {
                SIDEBAR_WIDTH * self.dpi() / 96
            } else {
                0
            };
        let h = ((self.painter.cell_h * 30.0 + 110.0) * s) as i32;
        unsafe {
            let _ = SetWindowPos(self.frame, None, 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER);
        }
    }

    fn add_tab(&mut self, place: Place, b: Backend) {
        let id = self.next_id;
        self.next_id += 1;
        let (cols, rows) = self.grid;
        let mut term = Terminal::new(cols, rows, self.config.terminal.scrollback as usize);
        term.ambiguous_wide = self.config.terminal.ambiguous_wide;
        let Backend {
            input,
            output,
            resize,
            wait,
            kill,
        } = b;
        let (in_tx, in_rx) = bounded::<Vec<u8>>(256);
        // 書き込み用のスレッド
        std::thread::spawn(move || {
            let mut input = input;
            while let Ok(data) = in_rx.recv() {
                if input.write_all(&data).and_then(|()| input.flush()).is_err() {
                    break;
                }
            }
        });
        // 終了を待つスレッドと、読み取り用のスレッド
        let waiter = std::thread::spawn(wait);
        let tx = self.tx.clone();
        let pending = self.pending.clone();
        let frame = self.frame.0 as isize;
        std::thread::spawn(move || {
            let notify = || {
                if !pending.swap(true, Ordering::AcqRel) {
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(frame as *mut _)),
                            WM_APP_TERM_EVENT,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                }
            };
            let mut output = output;
            let mut buf = vec![0u8; 64 << 10];
            loop {
                match output.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(Event::Output(id, buf[..n].to_vec())).is_err() {
                            return;
                        }
                        notify();
                    }
                }
            }
            let code = waiter.join().ok().flatten();
            let _ = tx.send(Event::Exit(id, code));
            notify();
        });
        let tab = Tab {
            id,
            term,
            back: 0,
            seen_pushed: 0,
            place,
            anchor: None,
            selection: None,
            selecting: false,
            mouse_button: None,
            mouse_cell: (usize::MAX, usize::MAX),
            input: in_tx,
            resize: Arc::from(resize),
            kill: Arc::from(kill),
            exited: None,
            size: (cols as u16, rows as u16),
            tn: None,
        };
        let mut tab = tab;
        if let Place::Tn3270(target) = &tab.place {
            let mut tn = tn3270::Tn3270::new(target.clone());
            tn.view_rows = self.grid.1;
            tab.term = tn3270::render(&tn, false);
            tab.tn = Some(Box::new(tn));
            if target.printer {
                unsafe {
                    SetTimer(Some(self.frame), TIMER_PRINTER, 1000, None);
                }
            }
        }
        let label = crate::util::wide(&tab_label(&tab));
        self.tabs.push(tab);
        let index = self.tabs.len() - 1;
        unsafe {
            let item = TCITEMW {
                mask: TCIF_TEXT,
                pszText: PWSTR(label.as_ptr() as *mut _),
                ..Default::default()
            };
            SendMessageW(
                self.tabbar,
                TCM_INSERTITEMW,
                Some(WPARAM(index)),
                Some(LPARAM(&item as *const _ as isize)),
            );
        }
        self.activate(index);
    }

    fn activate(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.active = index;
        unsafe {
            // 3270 のタブではキーパッドを出す（並べ直しは状態を借りずに行う）
            let _ = PostMessageW(Some(self.frame), WM_APP_TERM_LAYOUT, WPARAM(0), LPARAM(0));
            SendMessageW(self.tabbar, TCM_SETCURSEL, Some(WPARAM(index)), None);
            // フォーカスは後で移す（ここで移すと通知が状態を借りられない）
            let _ = PostMessageW(Some(self.frame), WM_APP_TERM_FOCUS, WPARAM(0), LPARAM(0));
        }
        self.update_title();
        self.update_scrollbar();
        self.invalidate();
    }

    fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let t = self.tabs.remove(index);
        (t.kill)();
        unsafe {
            SendMessageW(self.tabbar, TCM_DELETEITEM, Some(WPARAM(index)), None);
        }
        if self.tabs.is_empty() {
            unsafe {
                let _ = PostMessageW(Some(self.frame), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            return;
        }
        let next = if self.active > index || self.active >= self.tabs.len() {
            self.active.saturating_sub(1)
        } else {
            self.active
        };
        self.activate(next.min(self.tabs.len() - 1));
    }

    /// タブ `keep` の右クリックのメニューで選んだタブ（`targets`）を閉じて、`keep` を表に出す。
    fn close_tabs(&mut self, keep: usize, targets: &[usize]) {
        let mut keep = keep;
        // 右から閉じる（残すタブの番号が変わらないように）
        for &i in targets.iter().rev() {
            if i >= self.tabs.len() {
                continue;
            }
            let t = self.tabs.remove(i);
            (t.kill)();
            unsafe {
                SendMessageW(self.tabbar, TCM_DELETEITEM, Some(WPARAM(i)), None);
            }
            if i < keep {
                keep -= 1;
            }
        }
        if self.tabs.is_empty() {
            unsafe {
                let _ = PostMessageW(Some(self.frame), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            return;
        }
        self.activate(keep.min(self.tabs.len() - 1));
    }

    fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.view), None, false);
        }
    }

    fn update_title(&self) {
        let ws = self
            .sidebar
            .title()
            .map(|t| format!(" [{t}]"))
            .unwrap_or_default();
        let title = match self.tab() {
            Some(t) => format!("{} - yyterm{ws}", t.title()),
            None => format!("yyterm{ws}"),
        };
        unsafe {
            let _ = SetWindowTextW(self.frame, &windows::core::HSTRING::from(title));
        }
    }

    fn update_tab_label(&self, index: usize) {
        let Some(t) = self.tabs.get(index) else {
            return;
        };
        let label = crate::util::wide(&tab_label(t));
        unsafe {
            let item = TCITEMW {
                mask: TCIF_TEXT,
                pszText: PWSTR(label.as_ptr() as *mut _),
                ..Default::default()
            };
            SendMessageW(
                self.tabbar,
                TCM_SETITEMW,
                Some(WPARAM(index)),
                Some(LPARAM(&item as *const _ as isize)),
            );
        }
    }

    fn update_status(&self) {
        let text = if self.status_text.is_empty() {
            let (c, r) = self.grid;
            match self.tab() {
                Some(t) if t.back > 0 => format!("{c} × {r}　{} 行さかのぼって表示中", t.back),
                _ => format!("{c} × {r}"),
            }
        } else {
            self.status_text.clone()
        };
        let w = crate::util::wide(&text);
        unsafe {
            SendMessageW(
                self.status,
                SB_SETTEXTW,
                Some(WPARAM(0)),
                Some(LPARAM(w.as_ptr() as isize)),
            );
        }
    }

    fn update_scrollbar(&self) {
        let Some(t) = self.tab() else { return };
        let hist = t.term.history_len();
        let rows = t.term.rows();
        let si = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            // 常に表示して（使えないときは淡色）、画面の幅が変わらないようにする
            fMask: SIF_ALL | SIF_DISABLENOSCROLL,
            nMin: 0,
            nMax: (hist + rows).saturating_sub(1) as i32,
            nPage: rows as u32,
            nPos: (hist - t.back.min(hist)) as i32,
            nTrackPos: 0,
        };
        unsafe {
            SetScrollInfo(self.view, SB_VERT, &si, true);
        }
    }

    /// 子ウィンドウの位置（サイドバー、タブ、画面）。並べるのは [`layout`] で、アプリの状態を
    /// 借りずに行う（大きさの変更の通知がすぐに届くため）。
    /// 3270 のキーパッドを出すか（3270 のタブを表示していて、設定で出す）。
    fn keypad_visible(&self) -> bool {
        self.config.tn3270.keypad
            && self
                .tab()
                .and_then(|t| t.tn.as_ref())
                .is_some_and(|tn| !tn.target.printer)
    }

    fn layout_rects(&self) -> (bool, Vec<(HWND, RECT)>) {
        let mut rc = RECT::default();
        let mut sr = RECT::default();
        unsafe {
            let _ = GetClientRect(self.frame, &mut rc);
            SendMessageW(self.status, WM_SIZE, None, None);
            let _ = GetWindowRect(self.status, &mut sr);
        }
        let dpi = self.dpi();
        let status_h = sr.bottom - sr.top;
        let height = (rc.bottom - status_h).max(0);
        let side = if self.sidebar.visible {
            (SIDEBAR_WIDTH * dpi / 96).min(rc.right / 2)
        } else {
            0
        };
        let gap = if side > 0 { 4 * dpi / 96 } else { 0 };
        let tab_h = 28 * dpi / 96;
        let x = side + gap;
        let keypad_w = if self.keypad_visible() {
            tn3270::Keypad::width(dpi).min(rc.right / 3)
        } else {
            0
        };
        let w = (rc.right - x - keypad_w).max(0);
        let r = |left: i32, top: i32, width: i32, height: i32| RECT {
            left,
            top,
            right: left + width,
            bottom: top + height,
        };
        let mut rects = vec![
            (self.sidebar.tree, r(0, 0, side, height)),
            (self.tabbar, r(x, 0, w + keypad_w, tab_h)),
            (self.view, r(x, tab_h, w, (height - tab_h).max(0))),
        ];
        if keypad_w > 0 {
            rects.extend(
                self.keypad
                    .rects(r(x + w, tab_h, keypad_w, (height - tab_h).max(0)), dpi),
            );
        }
        (self.sidebar.visible, rects)
    }

    /// 画面の大きさが変わった。
    fn on_view_size(&mut self, width: i32, height: i32) {
        self.painter.resize_target(width as u32, height as u32);
        let (cols, rows) = self.painter.grid_size(width, height);
        self.grid = (cols, rows);
        for t in &mut self.tabs {
            // 3270 の画面の大きさはホストが決める（プリンターの一覧は画面の行数に合わせる）
            if let Some(tn) = t.tn.as_mut() {
                if tn.target.printer && tn.view_rows != rows {
                    tn.view_rows = rows;
                    t.term = tn3270::render(tn, t.exited.is_some());
                }
                continue;
            }
            t.term.resize(cols, rows);
            let size = (cols as u16, rows as u16);
            if size != t.size && t.exited.is_none() {
                t.size = size;
                (t.resize)(size.0, size.1);
            }
        }
        self.update_scrollbar();
        self.update_status();
        self.invalidate();
    }

    fn relayout_view(&mut self) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.view, &mut rc);
        }
        self.on_view_size(rc.right, rc.bottom);
    }

    fn paint(&mut self) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.view, &mut rc);
        }
        let focused = unsafe { GetFocus() } == self.view;
        let tab = self.tabs.get(self.active);
        let scene = tab.map(|t| Scene {
            term: &t.term,
            back: t.back,
            selection: t.selection,
            focused,
        });
        let _ = self
            .painter
            .paint(self.view, rc.right as u32, rc.bottom as u32, scene.as_ref());
    }

    /// シェルの出力・終了を端末に流す。
    fn on_events(&mut self) {
        self.pending.store(false, Ordering::Release);
        let mut active_dirty = false;
        let mut budget = 8usize << 20;
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                Event::Output(id, data) => {
                    budget = budget.saturating_sub(data.len());
                    let Some(i) = self.tabs.iter().position(|t| t.id == id) else {
                        continue;
                    };
                    if self.tabs[i].tn.is_some() {
                        self.tn_receive(i, &data);
                        if i == self.active {
                            active_dirty = true;
                        }
                        continue;
                    }
                    let t = &mut self.tabs[i];
                    t.term.feed(&data);
                    let resp = t.term.take_responses();
                    t.send(resp);
                    // さかのぼって表示しているときは、同じ行を表示し続ける
                    let pushed = t.term.pushed_lines();
                    if t.back > 0 {
                        t.back =
                            (t.back + (pushed - t.seen_pushed) as usize).min(t.term.history_len());
                    }
                    t.seen_pushed = pushed;
                    let _ = t.term.take_bell();
                    if t.term.take_title_changed() {
                        self.update_tab_label(i);
                        if i == self.active {
                            self.update_title();
                        }
                    }
                    if i == self.active {
                        active_dirty = true;
                    }
                }
                Event::Notice(target, text) => {
                    // 表示しているタブが同じ接続先ならそこに、でなければ最初のその接続先のタブに
                    let same = |t: &Tab| matches!(&t.place, Place::Remote { target: x, .. } if x.same(&target));
                    let i = self
                        .tabs
                        .get(self.active)
                        .filter(|t| same(t))
                        .map(|_| self.active)
                        .or_else(|| self.tabs.iter().position(same));
                    match i {
                        Some(i) => {
                            let t = &mut self.tabs[i];
                            t.term.feed(b"\r\n");
                            t.term.feed(&notice_bytes(false, &text));
                            if i == self.active {
                                active_dirty = true;
                            }
                        }
                        None => {
                            self.status_text = text;
                            self.update_status();
                        }
                    }
                }
                Event::Exit(id, code) => {
                    let Some(i) = self.tabs.iter().position(|t| t.id == id) else {
                        continue;
                    };
                    let t = &mut self.tabs[i];
                    t.exited = Some(code);
                    if let Some(tn) = &mut t.tn {
                        let mut events = tn.session.abandon_transfer(
                            "切断されました（IND$FILE は途中から再開できません。接続し直して最初から転送してください）",
                        );
                        if let Some(job) = tn.session.flush_print() {
                            events.push(yy_3270::Event::PrintJob(job));
                        }
                        t.term = tn3270::render(tn, true);
                        let label = t.place.label();
                        self.tn_note(&format!("{label}: 切断されました"));
                        self.tn_events(i, events);
                        self.macro_notify(i);
                        if i == self.active {
                            active_dirty = true;
                        }
                        continue;
                    }
                    let code = code
                        .map(|c| format!("（終了コード {c}）"))
                        .unwrap_or_default();
                    t.term.feed(
                        format!(
                            "\r\n\x1b[0;93m[プロセスが終了しました{code}。Enter キーでタブを閉じます]\x1b[0m"
                        )
                        .as_bytes(),
                    );
                    if i == self.active {
                        active_dirty = true;
                    }
                }
            }
            if budget == 0 {
                // 残りは次の機会に（UI の操作を止めない）
                if !self.pending.swap(true, Ordering::AcqRel) {
                    unsafe {
                        let _ =
                            PostMessageW(Some(self.frame), WM_APP_TERM_EVENT, WPARAM(0), LPARAM(0));
                    }
                }
                break;
            }
        }
        if active_dirty {
            self.update_scrollbar();
            self.invalidate();
        }
    }

    // ---- 3270 のタブ -------------------------------------------------------------

    /// 3270 の接続の記録に 1 行書く（`logs\tn3270.log`）。
    fn tn_log_line(&mut self, text: &str) {
        let log = self.tn_log.get_or_insert_with(|| {
            let path = yy_config::config_dir().map(|d| d.join("logs").join("tn3270.log"));
            std::sync::Arc::new(yy_remote::log::TransferLog::new(
                path,
                crate::remote::local_clock,
            ))
        });
        log.line(None, text);
    }

    /// ファイル転送の記録に 1 行書き、ステータスに出す（`logs\tn3270-transfer.log`）。
    fn ft_note(&mut self, text: &str) {
        let log = self.ft_log.get_or_insert_with(|| {
            let path = yy_config::config_dir().map(|d| d.join("logs").join("tn3270-transfer.log"));
            std::sync::Arc::new(yy_remote::log::TransferLog::new(
                path,
                crate::remote::local_clock,
            ))
        });
        log.line(None, text);
        self.status_text = text.to_owned();
        self.update_status();
    }

    /// 知らせをステータスと記録に出す。
    fn tn_note(&mut self, text: &str) {
        self.tn_log_line(text);
        self.status_text = text.to_owned();
        self.update_status();
    }

    /// 3270 のセッションの出来事を記録・表示する。
    fn tn_events(&mut self, i: usize, events: Vec<yy_3270::Event>) {
        use yy_3270::Event as E;
        let label = self.tabs[i].place.label();
        let mut rerender = false;
        for e in events {
            match e {
                E::Negotiation(s) => self.tn_log_line(&format!("{label}: {s}")),
                E::Mode(m) => {
                    self.tn_note(&format!("{label}: {} で接続しました", m.label()));
                    self.macro_on_connect(i);
                }
                E::Device(d) => {
                    self.tn_log_line(&format!("{label}: LU {d} が割り当てられました"));
                    self.update_tab_label(i);
                    // 設定で `printer = "auto"` なら、対応するプリンターも開く
                    let auto = self.tabs[i]
                        .tn
                        .as_ref()
                        .filter(|tn| tn.target.auto_printer && !tn.target.printer)
                        .map(|tn| tn.target.printer_target(Some(&d)));
                    match auto {
                        Some(Ok(p)) => {
                            self.pending_printers.push(p);
                            unsafe {
                                let _ = PostMessageW(
                                    Some(self.frame),
                                    WM_APP_TERM_PRINTER,
                                    WPARAM(0),
                                    LPARAM(0),
                                );
                            }
                        }
                        Some(Err(e)) => self.tn_note(&format!("{label}: {e}")),
                        None => {}
                    }
                }
                E::PrintJob(job) => {
                    self.tn_print_job(i, job);
                    rerender = true;
                }
                E::DeviceRejected(r) => {
                    if let Some(tn) = self.tabs[i].tn.as_mut() {
                        tn.message = format!("LU を使えません: {r}");
                    }
                    self.tn_note(&format!("{label}: LU を使えません: {r}"));
                }
                E::Text(t) => {
                    if let Some(tn) = self.tabs[i].tn.as_mut() {
                        tn.message = t.clone();
                    }
                    self.tn_note(&format!("{label}: {t}"));
                }
                E::Alarm => unsafe {
                    let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(MB_OK);
                },
                E::Transfer(ev) => {
                    if let Some(result) = self.tn_transfer_event(i, ev) {
                        self.macro_transfer_done(i, result);
                    }
                    rerender = true;
                }
            }
        }
        if rerender {
            let t = &mut self.tabs[i];
            if let Some(tn) = &t.tn {
                t.term = tn3270::render(tn, t.exited.is_some());
            }
            if i == self.active {
                self.invalidate();
            }
        }
    }

    /// ファイル転送の進み具合・結果。終わったら成否と知らせを返す。
    fn tn_transfer_event(&mut self, i: usize, ev: yy_3270::FtEvent) -> Option<(bool, String)> {
        use yy_3270::FtEvent;
        let label = self.tabs[i].place.label();
        let tn = self.tabs[i].tn.as_mut()?;
        match ev {
            FtEvent::Started => {
                tn.message = "転送を始めました".into();
                self.ft_note(&format!("{label}: ホストが転送を始めました"));
            }
            FtEvent::Progress(n) => {
                let text = format!("転送中 {}", crate::util::human_size(n));
                tn.message = text.clone();
                self.status_text = format!("{label}: {text}");
                self.update_status();
            }
            FtEvent::Done { ok, message } => {
                let Some(job) = tn.ft.take() else {
                    tn.message = message.clone();
                    self.ft_note(&format!("{label}: {message}"));
                    return Some((false, message));
                };
                let secs = job.started.elapsed().as_secs_f64();
                let what = format!(
                    "{} {} {}",
                    job.choice.request.host_file,
                    match job.choice.request.direction {
                        yy_3270::ind_file::Direction::Receive => "→",
                        yy_3270::ind_file::Direction::Send => "←",
                    },
                    job.choice.local.display()
                );
                let mut done = false;
                let text = match tn3270::finish(&job, ok) {
                    Ok(size) if ok => {
                        done = true;
                        let open = job.choice.open_after
                            && job.choice.request.direction
                                == yy_3270::ind_file::Direction::Receive;
                        if open {
                            open_in_editor(self.frame, &job.choice.local);
                        }
                        format!(
                            "転送しました（{}、{secs:.1} 秒）: {what}  {message}",
                            crate::util::human_size(size)
                        )
                    }
                    Ok(_) => format!("転送できませんでした: {what}  {message}"),
                    Err(e) => format!("転送の後始末に失敗しました: {what}  {e}"),
                };
                tn.message = message;
                self.ft_note(&format!("{label}: {text}"));
                return Some((done, text));
            }
        }
        None
    }

    /// プリンターのタブ `i` が印刷を受け取った: 一覧に加え、設定の出力先に出す。
    fn tn_print_job(&mut self, i: usize, job: yy_3270::PrintJob) {
        if job.is_empty() {
            return;
        }
        self.print_jobs += 1;
        let number = self.print_jobs;
        let label = self.tabs[i].place.label();
        let clock = crate::remote::local_clock();
        let time = clock.get(11..19).unwrap_or(&clock).to_owned();
        let pc = &self.config.tn3270.printer;
        let output = match printer::Output::from_config(&pc.output) {
            printer::Output::Ask => String::new(),
            // PDF のプリンターは保存先を尋ねる（ここでは尋ねられない）ので、output = "pdf" を使う
            printer::Output::Printer
                if pc
                    .printer_name
                    .trim()
                    .eq_ignore_ascii_case(printer::PDF_PRINTER) =>
            {
                "印刷できませんでした: PDF にするときは設定の output を \"pdf\" にしてください"
                    .into()
            }
            printer::Output::Printer => {
                match printer::print(&job, &pc.printer_name, None, &format!("{label} #{number}")) {
                    Ok(n) => format!("{n} ページを印刷しました"),
                    Err(e) => format!("印刷できませんでした: {e}"),
                }
            }
            out @ (printer::Output::Pdf | printer::Output::Text) => {
                let stamp: String = clock
                    .get(..19)
                    .unwrap_or(&clock)
                    .chars()
                    .filter(char::is_ascii_digit)
                    .collect();
                let ext = if out == printer::Output::Pdf {
                    "pdf"
                } else {
                    "txt"
                };
                let path = printer::output_folder(&pc.folder)
                    .join(printer::output_name(&label, &stamp, number, ext));
                let r = if out == printer::Output::Pdf {
                    printer::save_pdf(&job, &path, &format!("{label} #{number}")).map(|_| ())
                } else {
                    printer::save_text(&job, &path)
                };
                match r {
                    Ok(()) => format!("{} に保存しました", path.display()),
                    Err(e) => format!("保存できませんでした: {e}"),
                }
            }
        };
        let pages = job.pages.len();
        self.ft_note(&format!(
            "{label}: 印刷 #{number}（{pages} ページ、{} バイト）を受け取りました。{}",
            job.bytes,
            if output.is_empty() {
                "一覧に置きました"
            } else {
                output.as_str()
            }
        ));
        if let Some(tn) = self.tabs[i].tn.as_mut() {
            tn.jobs.push(printer::StoredJob {
                number,
                time,
                job,
                output,
            });
        }
    }

    /// PRINT-EOJ のないホスト: 一定時間データが来なければジョブを終える（タイマーから）。
    fn tn_print_tick(&mut self) {
        let timeout = std::time::Duration::from_secs(self.config.tn3270.printer.eoj_timeout.max(1));
        for i in 0..self.tabs.len() {
            let Some(tn) = self.tabs[i].tn.as_mut() else {
                continue;
            };
            if !tn.target.printer || !tn.session.printing() || tn.last_data.elapsed() < timeout {
                continue;
            }
            if let Some(job) = tn.session.flush_print() {
                self.tn_events(i, vec![yy_3270::Event::PrintJob(job)]);
            }
        }
    }

    /// 表示しているプリンターのタブの、番号 `number` の印刷。
    fn stored_job(&self, number: usize) -> Option<(String, yy_3270::PrintJob)> {
        let t = self.tab()?;
        let tn = t.tn.as_ref()?;
        let j = tn.jobs.iter().find(|j| j.number == number)?;
        Some((format!("{} #{}", t.place.label(), j.number), j.job.clone()))
    }

    /// 表示しているプリンターのタブの印刷に、出した先を書く。
    fn set_job_output(&mut self, number: usize, output: String) {
        let active = self.active;
        if let Some(tn) = self.tabs.get_mut(active).and_then(|t| t.tn.as_mut())
            && let Some(j) = tn.jobs.iter_mut().find(|j| j.number == number)
        {
            j.output = output.clone();
            let t = &mut self.tabs[active];
            if let Some(tn) = &t.tn {
                t.term = tn3270::render(tn, t.exited.is_some());
            }
        }
        self.ft_note(&format!("印刷 #{number}: {output}"));
        self.invalidate();
    }

    // ---- マクロ ---------------------------------------------------------------------

    /// マクロの記録に 1 行書く（`logs\tn3270-macro.log`）。
    fn macro_log_line(&mut self, text: &str) {
        let log = self.macro_log.get_or_insert_with(|| {
            let path = yy_config::config_dir().map(|d| d.join("logs").join("tn3270-macro.log"));
            std::sync::Arc::new(yy_remote::log::TransferLog::new(
                path,
                crate::remote::local_clock,
            ))
        });
        log.line(None, text);
    }

    /// マクロを動かしているタブの位置。
    fn macro_tab(&self) -> Option<usize> {
        let id = self.macro_run.as_ref()?.tab_id;
        self.tabs.iter().position(|t| t.id == id)
    }

    /// タブ `i` の画面が変わった: そのタブのマクロを起こす。
    fn macro_notify(&self, i: usize) {
        if let Some(m) = &self.macro_run
            && self.tabs.get(i).is_some_and(|t| t.id == m.tab_id)
        {
            m.generation.bump();
        }
    }

    /// タブ `i` でマクロを始める。
    fn macro_start(&mut self, i: usize, path: &Path) -> std::result::Result<(), String> {
        if self.macro_run.is_some() {
            return Err("ほかのマクロを実行中です".into());
        }
        let t = self.tabs.get(i).ok_or("タブがありません")?;
        let tn = t.tn.as_ref().ok_or("3270 のタブではありません")?;
        if tn.target.printer {
            return Err("プリンターのタブではマクロを実行できません".into());
        }
        let script = std::fs::read_to_string(path)
            .map_err(|e| format!("{} を読めません: {e}", path.display()))?;
        yy_3270_macro::check(&script).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mc = &self.config.tn3270.macros;
        let out_dir = macros::output_folder(&mc.output);
        let run = macros::start(
            t.id,
            &name,
            script,
            out_dir,
            mc.timeout,
            self.frame,
            WM_APP_TERM_MACRO,
        );
        let label = t.place.label();
        self.macro_run = Some(run);
        self.macro_log_line(&format!("{label}: 開始 {}", path.display()));
        self.tn_note(&format!(
            "{label}: マクロ {name} を実行しています（Ctrl+Break で停止）"
        ));
        let t = &mut self.tabs[i];
        if let Some(tn) = t.tn.as_mut() {
            tn.message = format!("マクロ実行中: {name}");
            t.term = tn3270::render(tn, t.exited.is_some());
        }
        self.invalidate();
        Ok(())
    }

    /// 接続したら実行するマクロ（設定の `on_connect`）。
    fn macro_on_connect(&mut self, i: usize) {
        let Some(name) = self.tabs[i]
            .tn
            .as_ref()
            .filter(|tn| !tn.target.printer)
            .and_then(|tn| tn.target.on_connect.clone())
        else {
            return;
        };
        let folder = macros::macro_folder(&self.config.tn3270.macros.folder);
        let path = macros::resolve(folder.as_deref(), &name);
        if let Err(e) = self.macro_start(i, &path) {
            let label = self.tabs[i].place.label();
            self.tn_note(&format!("{label}: 接続時のマクロを実行できません: {e}"));
        }
    }

    fn macro_stop(&mut self) {
        match &self.macro_run {
            Some(m) => {
                m.stop.store(true, std::sync::atomic::Ordering::Relaxed);
                m.generation.bump();
                if let Some(tx) = self
                    .macro_run
                    .as_mut()
                    .and_then(|m| m.transfer_reply.take())
                {
                    let _ = tx.send(Err("停止しました".into()));
                }
                // 転送中なら取り消す
                if let Some(i) = self.macro_tab()
                    && let Some(tn) = self.tabs[i].tn.as_mut()
                    && tn.session.transferring()
                {
                    let events = tn.session.cancel_transfer();
                    self.tn_events(i, events);
                }
                self.tn_note("マクロを止めています…");
            }
            None => self.tn_note("実行中のマクロはありません"),
        }
    }

    /// マクロの画面の写し。
    fn macro_snapshot(&self) -> std::result::Result<yy_3270_macro::Snapshot, String> {
        let i = self.macro_tab().ok_or("タブが閉じられました")?;
        let t = &self.tabs[i];
        let tn = t.tn.as_ref().ok_or("3270 のタブではありません")?;
        Ok(yy_3270_macro::Snapshot::of(&tn.session, t.exited.is_none()))
    }

    /// マクロの入力（`secret` なら記録に値を残さない）。
    fn macro_type(&mut self, text: &str, secret: bool) -> std::result::Result<(), String> {
        let i = self.macro_tab().ok_or("タブが閉じられました")?;
        if self.tabs[i].exited.is_some() {
            return Err("切断されています".into());
        }
        // 先打ちはしない: ロック中に入れた文字は捨てられるので、誤りにする
        if self.tabs[i]
            .tn
            .as_ref()
            .is_some_and(|tn| tn.session.oia().lock != yy_3270::Lock::None)
        {
            return Err(LOCKED.into());
        }
        let hidden = self.tabs[i].tn.as_ref().is_some_and(|tn| {
            let s = tn.session.screen();
            s.field_attr(s.cursor).is_some_and(|fa| {
                fa & yy_3270::codes::FA_DISPLAY_MASK == yy_3270::codes::FA_NONDISPLAY
            })
        });
        self.macro_log_line(&format!(
            "type {}",
            if secret || hidden {
                "******".to_owned()
            } else {
                format!("{text:?}")
            }
        ));
        for c in text.chars() {
            self.tn_key_at(i, yy_3270::Key::Char(c));
            let lock = self.tabs[i].tn.as_ref().map(|tn| tn.session.oia().lock);
            if let Some(yy_3270::Lock::Operator(e)) = lock {
                self.tn_key_at(i, yy_3270::Key::Reset);
                return Err(format!("入力できません（{}）", e.label()));
            }
        }
        Ok(())
    }

    /// マクロの操作（UI のスレッドで、ダイアログを出さないもの）。返事は `tx` に送る
    /// （転送は終わってから）。
    fn macro_act(
        &mut self,
        op: yy_3270_macro::Op,
        tx: std::sync::mpsc::Sender<std::result::Result<yy_3270_macro::Answer, String>>,
    ) {
        use yy_3270_macro::{Answer, Op};
        let Some(i) = self.macro_tab() else {
            let _ = tx.send(Err("タブが閉じられました".into()));
            return;
        };
        let r: std::result::Result<Answer, String> = match op {
            Op::Key(k) => {
                self.macro_log_line(&format!(
                    "key {}",
                    yy_3270_macro::record::key_name(&k).unwrap_or_default()
                ));
                let locked = self.tabs[i]
                    .tn
                    .as_ref()
                    .is_some_and(|tn| tn.session.oia().lock != yy_3270::Lock::None);
                if self.tabs[i].exited.is_some() {
                    Err("切断されています".into())
                } else if locked
                    && !matches!(
                        k,
                        yy_3270::Key::Reset | yy_3270::Key::Attn | yy_3270::Key::SysReq
                    )
                {
                    Err(LOCKED.into())
                } else {
                    self.tn_key_at(i, k);
                    Ok(Answer::Done)
                }
            }
            Op::Type(text) => self.macro_type(&text, false).map(|()| Answer::Done),
            Op::MoveTo(r, c) => {
                let size = self.tabs[i]
                    .tn
                    .as_ref()
                    .map(|tn| (tn.session.screen().rows, tn.session.screen().cols));
                match size {
                    Some((rows, cols)) if r <= rows && c <= cols => {
                        self.tn_key_at(i, yy_3270::Key::MoveTo((r - 1) * cols + c - 1));
                        Ok(Answer::Done)
                    }
                    _ => Err(format!("{r} 行 {c} 桁は画面の外です")),
                }
            }
            Op::Transfer { request, local } => {
                let choice = ftdlg::Choice {
                    request,
                    local,
                    open_after: false,
                };
                self.macro_log_line(&format!("transfer {}", choice.request.command()));
                match self.tn_start_transfer_at(i, choice) {
                    Ok(()) => {
                        if let Some(m) = self.macro_run.as_mut() {
                            m.transfer_reply = Some(tx);
                        }
                        return;
                    }
                    Err(e) => Err(e),
                }
            }
            Op::PrintScreen => self.macro_print_screen(i).map(|()| Answer::Done),
            Op::Log(text) => {
                let label = self.tabs[i].place.label();
                self.macro_log_line(&format!("{label}: {text}"));
                self.status_text = format!("マクロ: {text}");
                self.update_status();
                Ok(Answer::Done)
            }
            // ダイアログを出すものは呼び出し側で扱う
            Op::Password(_) | Op::Ask(_) | Op::Message(_) => Ok(Answer::Done),
        };
        let _ = tx.send(r);
    }

    /// マクロの転送が終わった。
    fn macro_transfer_done(&mut self, i: usize, (ok, message): (bool, String)) {
        if self.macro_tab() != Some(i) {
            return;
        }
        if let Some(tx) = self
            .macro_run
            .as_mut()
            .and_then(|m| m.transfer_reply.take())
        {
            let _ = tx.send(Ok(yy_3270_macro::Answer::Transfer { ok, message }));
        }
    }

    /// 画面を印刷の出力先へ（`print_screen()`）。
    fn macro_print_screen(&mut self, i: usize) -> std::result::Result<(), String> {
        let t = &self.tabs[i];
        let tn = t.tn.as_ref().ok_or("3270 のタブではありません")?;
        let snap = yy_3270_macro::Snapshot::of(&tn.session, true);
        let job = yy_3270::PrintJob {
            pages: vec![
                snap.lines()
                    .into_iter()
                    .map(|l| l.trim_end().to_owned())
                    .collect(),
            ],
            columns: snap.cols(),
            lines_per_page: 0,
            bytes: 0,
        };
        let label = t.place.label();
        let pc = &self.config.tn3270.printer;
        let clock = crate::remote::local_clock();
        let stamp: String = clock
            .chars()
            .filter(char::is_ascii_digit)
            .take(14)
            .collect();
        let r = match printer::Output::from_config(&pc.output) {
            out @ (printer::Output::Pdf | printer::Output::Text) => {
                let ext = if out == printer::Output::Pdf {
                    "pdf"
                } else {
                    "txt"
                };
                let path = printer::output_folder(&pc.folder)
                    .join(printer::output_name(&label, &stamp, 0, ext));
                if out == printer::Output::Pdf {
                    printer::save_pdf(&job, &path, &label).map(|_| path.display().to_string())
                } else {
                    printer::save_text(&job, &path).map(|()| path.display().to_string())
                }
            }
            _ if pc
                .printer_name
                .trim()
                .eq_ignore_ascii_case(printer::PDF_PRINTER) =>
            {
                Err("PDF にするときは設定の output を \"pdf\" にしてください".into())
            }
            _ => printer::print(&job, &pc.printer_name, None, &label)
                .map(|_| "プリンター".to_owned()),
        };
        r.map(|to| self.macro_log_line(&format!("print_screen → {to}")))
    }

    /// マクロが終わった。
    fn macro_finished(&mut self, r: &std::result::Result<(), yy_3270_macro::MacroError>) -> String {
        let Some(run) = self.macro_run.take() else {
            return String::new();
        };
        let text = match r {
            Ok(()) => format!("マクロ {} が終わりました", run.name),
            Err(e) if e.stopped => format!("マクロ {} を止めました", run.name),
            Err(e) => format!("マクロ {} のエラー: {e}", run.name),
        };
        self.macro_log_line(&text);
        if let Some(i) = self.tabs.iter().position(|t| t.id == run.tab_id) {
            let t = &mut self.tabs[i];
            if let Some(tn) = t.tn.as_mut() {
                tn.message = text.clone();
                t.term = tn3270::render(tn, t.exited.is_some());
            }
            self.invalidate();
        }
        self.tn_note(&text);
        text
    }

    /// 表示している 3270 のタブのファイル転送を始める。
    fn tn_start_transfer(&mut self, choice: ftdlg::Choice) -> std::result::Result<(), String> {
        self.tn_start_transfer_at(self.active, choice)
    }

    /// 3270 のタブ `active` のファイル転送を始める。
    fn tn_start_transfer_at(
        &mut self,
        active: usize,
        choice: ftdlg::Choice,
    ) -> std::result::Result<(), String> {
        let label = self
            .tabs
            .get(active)
            .map(|t| t.place.label())
            .unwrap_or_default();
        let t = self.tabs.get_mut(active).ok_or("3270 のタブがありません")?;
        if t.exited.is_some() {
            return Err("切断されています".into());
        }
        let tn = t.tn.as_mut().ok_or("3270 のタブではありません")?;
        let (local, part) = tn3270::prepare(&choice)?;
        let mut o = match tn.session.transfer(&choice.request, local) {
            Ok(o) => o,
            Err(e) => {
                if let Some(p) = &part {
                    let _ = std::fs::remove_file(p);
                }
                return Err(e);
            }
        };
        let command = choice.request.command();
        tn.message = "転送のコマンドを送りました".into();
        tn.ft = Some(tn3270::FtJob {
            choice: choice.clone(),
            part,
            started: std::time::Instant::now(),
        });
        let term = tn3270::render(tn, false);
        t.send(std::mem::take(&mut o.send));
        t.selection = None;
        t.term = term;
        self.ft_last = Some(choice);
        self.ft_note(&format!("{label}: 開始 {command}"));
        self.invalidate();
        Ok(())
    }

    /// 表示している 3270 のタブのファイル転送を取り消す。
    fn tn_cancel_transfer(&mut self) {
        let active = self.active;
        let Some(tn) = self.tabs.get_mut(active).and_then(|t| t.tn.as_mut()) else {
            return;
        };
        if !tn.session.transferring() {
            self.tn_note("実行中のファイル転送はありません");
            return;
        }
        let events = tn.session.cancel_transfer();
        tn.message = "取り消しています…".into();
        self.tn_events(active, events);
        let t = &mut self.tabs[active];
        if let Some(tn) = &t.tn {
            t.term = tn3270::render(tn, t.exited.is_some());
        }
        self.invalidate();
    }

    /// 3270 のタブ `i` がホストからデータを受け取った。
    fn tn_receive(&mut self, i: usize, data: &[u8]) {
        let t = &mut self.tabs[i];
        let Some(tn) = t.tn.as_mut() else { return };
        tn.last_data = std::time::Instant::now();
        let mut o = tn.session.receive(data);
        let term = o.changed.then(|| tn3270::render(tn, false));
        t.send(std::mem::take(&mut o.send));
        if let Some(term) = term {
            t.term = term;
        }
        self.tn_events(i, o.events);
        self.macro_notify(i);
        if i == self.active {
            // 画面の大きさ（モデル）が変わったらキーパッド・画面を並べ直す
            self.invalidate();
        }
    }

    /// 表示している 3270 のタブにキーを送る。
    fn tn_key(&mut self, k: yy_3270::Key) {
        let active = self.active;
        let Some(t) = self.tabs.get(active) else {
            return;
        };
        // マクロの実行中は利用者のキーを止める
        if t.exited.is_none() && self.macro_run.as_ref().is_some_and(|m| m.tab_id == t.id) {
            self.status_text =
                "マクロの実行中です（Ctrl+Break かマクロ メニューで止められます）".into();
            self.update_status();
            return;
        }
        // 操作の記録
        if let Some((id, rec)) = self.recorder.as_mut()
            && *id == t.id
            && t.exited.is_none()
            && let Some(tn) = t.tn.as_ref()
        {
            let s = tn.session.screen();
            let hidden = s.field_attr(s.cursor).is_some_and(|fa| {
                fa & yy_3270::codes::FA_DISPLAY_MASK == yy_3270::codes::FA_NONDISPLAY
            });
            rec.key(&k, hidden, s.cols);
        }
        self.tn_key_at(active, k);
    }

    /// 3270 のタブ `i` にキーを送る（利用者の入力・マクロ）。
    fn tn_key_at(&mut self, active: usize, k: yy_3270::Key) {
        let Some(t) = self.tabs.get_mut(active) else {
            return;
        };
        // 切断したタブは Enter・Esc（Clear）で閉じる
        if t.exited.is_some() {
            if matches!(k, yy_3270::Key::Enter | yy_3270::Key::Clear) {
                self.close_tab(active);
            }
            return;
        }
        let Some(tn) = t.tn.as_mut() else { return };
        // プリンターには入力しない
        if tn.target.printer {
            return;
        }
        let mut o = tn.session.key(k);
        let term = o.changed.then(|| tn3270::render(tn, false));
        t.send(std::mem::take(&mut o.send));
        if let Some(term) = term {
            t.selection = None;
            t.term = term;
            self.invalidate();
        }
        self.tn_events(active, o.events);
    }

    /// 表示しているタブが 3270 か。
    fn is_tn(&self) -> bool {
        self.tab().is_some_and(|t| t.tn.is_some())
    }

    /// 入力をシェルに送る（最新の表示に戻し、選択を解く）。
    fn send_input(&mut self, bytes: Vec<u8>) {
        let Some(t) = self.tab_mut() else { return };
        if t.exited.is_some() {
            return;
        }
        let changed = t.back != 0 || t.selection.is_some();
        t.back = 0;
        t.selection = None;
        t.send(bytes);
        if changed {
            self.update_scrollbar();
            self.update_status();
            self.invalidate();
        }
    }

    fn send_key(&mut self, key: Key, m: Mods) {
        // 終わったタブは Enter・Esc で閉じる
        if self.tab().is_some_and(|t| t.exited.is_some()) {
            if matches!(key, Key::Enter | Key::Escape) {
                self.close_tab(self.active);
            }
            return;
        }
        let Some(t) = self.tab() else { return };
        let bytes = yy_term::keys::encode(key, m, t.term.modes());
        self.send_input(bytes);
    }

    fn copy(&mut self) -> bool {
        let Some(t) = self.tab() else { return false };
        let Some((a, b)) = t.selection else {
            return false;
        };
        let text = t.term.text(a, b).replace('\n', "\r\n");
        crate::clipboard::set_text(self.frame, &text, false)
    }

    fn paste(&mut self) {
        let Some((text, _)) = crate::clipboard::get_text(self.frame) else {
            return;
        };
        if let Some(t) = self.tabs.get_mut(self.active)
            && let Some(tn) = t.tn.as_mut()
        {
            // プリンターには入力しない
            if tn.target.printer {
                return;
            }
            if self.macro_run.as_ref().is_some_and(|m| m.tab_id == t.id) {
                return;
            }
            if let Some((id, rec)) = self.recorder.as_mut()
                && *id == t.id
            {
                rec.paste(&text);
            }
            let o = tn.session.paste(&text);
            if o.changed {
                t.selection = None;
                t.term = tn3270::render(tn, t.exited.is_some());
                self.invalidate();
            }
            return;
        }
        let Some(t) = self.tab() else { return };
        let bytes = yy_term::keys::paste(&text, t.term.modes());
        self.send_input(bytes);
    }

    fn scroll_view(&mut self, lines: isize) {
        let Some(t) = self.tab_mut() else { return };
        let hist = t.term.history_len() as isize;
        let back = (t.back as isize + lines).clamp(0, hist) as usize;
        if back != t.back {
            t.back = back;
            self.update_scrollbar();
            self.update_status();
            self.invalidate();
        }
    }

    fn zoom(&mut self, pt: Option<f32>) {
        let size = match pt {
            Some(d) => self.painter.size_pt() + d,
            None => {
                if self.config.terminal.font_size > 0.0 {
                    self.config.terminal.font_size
                } else {
                    self.config.editor.font_size
                }
            }
        };
        if self.painter.set_size(size).is_ok() {
            self.relayout_view();
        }
    }

    /// マウスの位置の端末の上の位置。
    fn pos_at(&self, x: i32, y: i32, round: bool) -> Option<Pos> {
        let t = self.tab()?;
        let (r, c) = self.painter.cell_at(x, y, round);
        let rows = t.term.rows() as isize;
        let row = r.clamp(0, rows - 1) as u64;
        let col = c.clamp(0, t.term.cols() as isize) as usize;
        let back = t.back.min(t.term.history_len()) as u64;
        Some(Pos {
            line: t.term.screen_line(0) - back + row,
            col,
        })
    }

    /// マウスの操作を報告するか（プログラムが要求していて、Shift を押していない）。
    fn reports_mouse(&self) -> bool {
        self.tab()
            .is_some_and(|t| t.term.modes().mouse != MouseMode::Off && t.exited.is_none())
            && !key_down(VK_SHIFT)
    }

    fn report_mouse(&mut self, ev: MouseEvent, x: i32, y: i32) {
        let (r, c) = self.painter.cell_at(x, y, false);
        let Some(t) = self.tab_mut() else { return };
        let row = r.clamp(0, t.term.rows() as isize - 1) as usize;
        let col = c.clamp(0, t.term.cols() as isize - 1) as usize;
        if matches!(ev, MouseEvent::Move(_)) && t.mouse_cell == (row, col) {
            return;
        }
        t.mouse_cell = (row, col);
        let m = mods();
        if let Some(bytes) = yy_term::keys::mouse(ev, col, row, m, t.term.modes()) {
            t.send(bytes);
        }
    }

    fn set_ime_position(&self) {
        let Some(t) = self.tab() else { return };
        let (row, col) = t.term.cursor();
        let (x, y) = self
            .painter
            .cell_px(row + t.back.min(t.term.history_len()), col);
        crate::ime::set_position(self.view, x, y, self.painter.line_px());
    }

    fn save_workspace(&mut self) {
        if let Err(e) = self.sidebar.save() {
            self.status_text = e;
            self.update_status();
        }
        self.update_title();
    }
}

// ---- フレーム -------------------------------------------------------------------------

extern "system" fn frame_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            layout();
            LRESULT(0)
        }
        WM_APP_TERM_LAYOUT => {
            layout();
            with(|a| a.relayout_view());
            LRESULT(0)
        }
        WM_APP_TERM_PRINTER => {
            let targets = with(|a| std::mem::take(&mut a.pending_printers)).unwrap_or_default();
            for t in targets {
                open_tab(Place::Tn3270(t));
            }
            LRESULT(0)
        }
        WM_APP_TERM_MACRO => {
            macro_pump(hwnd);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_PRINTER => {
            with(|a| a.tn_print_tick());
            LRESULT(0)
        }
        WM_SETFOCUS | WM_APP_TERM_FOCUS => {
            focus_view();
            LRESULT(0)
        }
        WM_COMMAND => {
            command(hwnd, loword(wparam.0) as u16);
            LRESULT(0)
        }
        WM_NOTIFY => {
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if hdr.idFrom == ID_TABS as usize && hdr.code == TCN_SELCHANGE {
                with(|a| {
                    let i = unsafe { SendMessageW(a.tabbar, TCM_GETCURSEL, None, None).0 };
                    a.activate(i.max(0) as usize);
                });
                return LRESULT(0);
            }
            if hdr.idFrom == ID_TREE as usize {
                return on_tree_notify(hwnd, hdr, lparam);
            }
            crate::default_proc(hwnd, msg, wparam, lparam)
        }
        WM_APP_TERM_EVENT => {
            with(|a| a.on_events());
            LRESULT(0)
        }
        WM_APP_TERM_REMOTE_DIR => {
            load_remote_dir(hwnd, wparam.0, lparam.0 as usize);
            LRESULT(0)
        }
        crate::tabclose::WM_APP_CLOSE_TAB => {
            with(|a| a.close_tab(wparam.0));
            LRESULT(0)
        }
        crate::tabclose::WM_APP_TAB_MENU => {
            tab_menu(hwnd, wparam.0);
            LRESULT(0)
        }
        m if m == crate::remote::WM_APP_REMOTE_PROMPT => {
            crate::remote::on_prompt(hwnd, lparam);
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let rc = unsafe { &*(lparam.0 as *const RECT) };
            with(|a| a.painter.set_dpi(hiword(wparam.0)));
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
            layout();
            with(|a| a.relayout_view());
            LRESULT(0)
        }
        WM_CLOSE => {
            let running =
                with(|a| a.tabs.iter().filter(|t| t.exited.is_none()).count()).unwrap_or(0);
            if running > 1 {
                let text =
                    format!("{running} 個のタブでシェルが動いています。すべて終了しますか？");
                let r = unsafe {
                    MessageBoxW(
                        Some(hwnd),
                        &windows::core::HSTRING::from(text),
                        w!("yyterm"),
                        MB_OKCANCEL | MB_ICONQUESTION,
                    )
                };
                if r != IDOK {
                    return LRESULT(0);
                }
            }
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => crate::default_proc(hwnd, msg, wparam, lparam),
    }
}

fn command(hwnd: HWND, id: u16) {
    match id {
        ID_NEW_TAB => {
            // 選んでいるタブと同じ場所（手元のフォルダは分からないのでホーム）
            let place = with(|a| match a.tab().map(|t| &t.place) {
                Some(p @ Place::Remote { .. }) => p.clone(),
                Some(p @ Place::Local(Some(_))) => p.clone(),
                Some(p @ Place::Tn3270(_)) => p.clone(),
                _ => initial_place(None),
            })
            .unwrap_or_else(|| initial_place(None));
            open_tab(place);
        }
        ID_SSH => cmd_ssh(hwnd),
        ID_TN3270 => cmd_tn3270(hwnd),
        ID_TN_TRANSFER => cmd_tn_transfer(hwnd),
        ID_TN_PRINTER => cmd_tn_printer(hwnd),
        ID_PR_PRINT | ID_PR_PDF | ID_PR_TEXT => cmd_print_output(hwnd, id),
        ID_MACRO_RUN => cmd_macro_run(hwnd),
        ID_MACRO_STOP => {
            with(|a| a.macro_stop());
        }
        ID_MACRO_RECORD => cmd_macro_record(hwnd),
        ID_MACRO_FOLDER => {
            let folder = with(|a| macros::macro_folder(&a.config.tn3270.macros.folder)).flatten();
            if let Some(f) = folder {
                let _ = std::fs::create_dir_all(&f);
                let _ = std::process::Command::new("explorer.exe").arg(&f).spawn();
            }
        }
        ID_TN_CANCEL => {
            with(|a| a.tn_cancel_transfer());
        }
        id if tn3270::keypad_key(id).is_some() => {
            if let Some(k) = tn3270::keypad_key(id) {
                with(|a| a.tn_key(k));
            }
            // ボタンからフォーカスを画面に戻す
            focus_view();
        }
        ID_WS_USE_AGENT => {
            crate::remote::set_use_agent(!crate::remote::use_agent());
            check_use_agent(hwnd);
            set_status(if crate::remote::use_agent() {
                "リモートのフォルダの一覧に、接続先のエージェントを使います（次に開くフォルダから）"
            } else {
                "リモートのフォルダの一覧に SFTP を使います（接続先に何も置きません。次に開くフォルダから）"
            });
        }
        ID_CLOSE_TAB => {
            with(|a| a.close_tab(a.active));
        }
        ID_EXIT => unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        ID_COPY => {
            with(|a| {
                if a.copy() {
                    if let Some(t) = a.tab_mut() {
                        t.selection = None;
                    }
                    a.invalidate();
                }
            });
        }
        ID_PASTE => {
            with(|a| a.paste());
        }
        ID_CLEAR => {
            with(|a| {
                if let Some(t) = a.tab_mut() {
                    t.term.feed(b"\x1b[3J");
                    t.back = 0;
                    t.selection = None;
                }
                a.update_scrollbar();
                a.invalidate();
            });
        }
        ID_SIDEBAR => {
            let Some((visible, tree)) = with(|a| {
                a.sidebar.visible = !a.sidebar.visible;
                (a.sidebar.visible, a.sidebar.tree)
            }) else {
                return;
            };
            layout();
            if visible {
                unsafe {
                    let _ = SetFocus(Some(tree));
                }
            } else {
                focus_view();
            }
        }
        ID_NEXT_TAB | ID_PREV_TAB => {
            with(|a| {
                let n = a.tabs.len();
                if n > 1 {
                    let i = if id == ID_NEXT_TAB {
                        (a.active + 1) % n
                    } else {
                        (a.active + n - 1) % n
                    };
                    a.activate(i);
                }
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
        ID_WS_NEW => {
            with(|a| {
                a.sidebar.switch_to(
                    workspace::config_file(workspace::TERMINAL_UNTITLED_FILE),
                    Workspace::default(),
                );
                a.save_workspace();
            });
        }
        ID_WS_OPEN => {
            if let Some(file) = crate::app::workspacemode::pick_workspace_file(hwnd, false, None) {
                match Workspace::load(&file) {
                    Ok(ws) => {
                        with(|a| {
                            a.sidebar.switch_to(Some(file.clone()), ws);
                            a.sidebar.visible = true;
                            let _ =
                                workspace::set_last_used_in(workspace::TERMINAL_LAST_FILE, &file);
                            a.update_title();
                        });
                        layout();
                    }
                    Err(e) => error_box(hwnd, &format!("{}\n\n{e}", file.display())),
                }
            }
        }
        ID_WS_SAVE_AS => {
            let current = with(|a| a.sidebar.file.clone()).flatten();
            if let Some(file) =
                crate::app::workspacemode::pick_workspace_file(hwnd, true, current.as_deref())
            {
                with(|a| {
                    a.sidebar.file = Some(file);
                    a.save_workspace();
                });
            }
        }
        ID_WS_ADD => cmd_add_folder(hwnd),
        ID_WS_ADD_REMOTE => cmd_add_remote_folder(hwnd),
        ID_HELP_KEYS => info_box(hwnd, KEYS_HELP),
        ID_SETTINGS => {
            let Some(path) = Config::default_path() else {
                return;
            };
            if !path.exists() {
                let _ = path
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| std::fs::write(&path, Config::default_file_contents()));
            }
            open_in_editor(hwnd, &path);
        }
        ID_REMOTE_LOG => match crate::remote::log_path() {
            Some(p) if p.exists() => open_in_editor(hwnd, &p),
            _ => info_box(
                hwnd,
                "リモート接続の記録はまだありません。SSH の接続先に接続すると記録します。",
            ),
        },
        ID_FORGET => crate::remote::forget_passwords(hwnd),
        ID_ABOUT => info_box(
            hwnd,
            &format!(
                "yyterm {}\n\nyyeditor と同じ部品で作ったターミナル（ConPTY・組み込みの SSH）。\n\
                 同梱フォント: UDEV Gothic（SIL Open Font License 1.1）",
                env!("CARGO_PKG_VERSION")
            ),
        ),
        _ => {}
    }
}

const KEYS_HELP: &str = "\
Ctrl+Shift+T\t新しいタブ
Ctrl+Shift+O\tSSH で接続
Ctrl+Shift+W\tタブを閉じる
Ctrl+Tab / Ctrl+Shift+Tab\tタブの切り替え
Ctrl+Shift+C / Ctrl+Insert\tコピー
Ctrl+Shift+V / Shift+Insert\t貼り付け
右クリック\t選択していればコピー、していなければ貼り付け
ダブルクリック\t単語を選択
Shift+PageUp / PageDown\tスクロールバックを表示
Ctrl+Shift+E\tワークスペース（サイドバー）
Ctrl++ / Ctrl+- / Ctrl+0\t文字の大きさ

プログラムがマウスを使っているとき（vim・tmux など）は、Shift を押しながら選択します。";

/// 「SSH で接続」。
fn cmd_ssh(hwnd: HWND) {
    if !crate::remote::available() {
        info_box(hwnd, "この yyterm には SSH の機能が組み込まれていません。");
        return;
    }
    let initial = crate::remote::last()
        .map(|u| u.target().to_string())
        .unwrap_or_default();
    let Some(input) = crate::goto::prompt_text(
        hwnd,
        "SSH で接続",
        "接続先（ユーザー@ホスト:ポート、または ~/.ssh/config の Host の名前）。\
         ポートフォワーディングは -L 8080:localhost:80・-R 9000:localhost:3000・-D 1080 を続けて書けます:",
        &initial,
    ) else {
        return;
    };
    let (text, forwards) = match yy_remote::forward::split_target_and_forwards(&input) {
        Ok(v) => v,
        Err(e) => {
            error_box(hwnd, &format!("入力を読めません。\n{e}"));
            return;
        }
    };
    let text = text.trim();
    let place = if let Some(u) = RemoteUri::parse(text) {
        Place::Remote {
            target: u.target(),
            dir: Some(u.path),
        }
    } else if let Some(t) = Target::parse(text) {
        Place::Remote {
            target: t,
            dir: None,
        }
    } else {
        error_box(hwnd, &format!("接続先（{text}）を読めません。"));
        return;
    };
    open_tab_forwarding(place, &forwards);
}

fn cmd_add_folder(hwnd: HWND) {
    if let Some(dir) = crate::grepdlg::browse_folder(hwnd) {
        with(|a| {
            a.sidebar.add_folder(&workspace::normalize(&dir));
            a.save_workspace();
        });
        layout();
    }
}

/// タブの文字列（閉じるボタンの場所を空ける）。
fn tab_label(t: &Tab) -> String {
    let mut label = t.title();
    if label.chars().count() > 32 {
        label = label.chars().take(29).collect::<String>() + "...";
    }
    label + crate::tabclose::LABEL_PAD
}

/// タブの右クリックのメニュー（閉じる・ほかのタブ・右側・左側を閉じる）。まとめて閉じるとき、
/// シェルが動いているタブがあれば確かめる。
fn tab_menu(hwnd: HWND, index: usize) {
    use crate::tabclose::TabMenu;
    let Some(count) = with(|a| a.tabs.len()) else {
        return;
    };
    let Some(which) = crate::tabclose::menu(hwnd, index, count) else {
        return;
    };
    let targets = which.targets(index, count);
    if targets.is_empty() {
        return;
    }
    if which != TabMenu::Close {
        let running = with(|a| {
            targets
                .iter()
                .filter(|&&i| a.tabs.get(i).is_some_and(|t| t.exited.is_none()))
                .count()
        })
        .unwrap_or(0);
        if running > 0 {
            let r = unsafe {
                MessageBoxW(
                    Some(hwnd),
                    &windows::core::HSTRING::from(format!(
                        "{} 個のタブを閉じます。そのうち {running} 個ではシェルが動いています（終了させます）。よろしいですか？",
                        targets.len()
                    )),
                    w!("yyterm"),
                    MB_OKCANCEL | MB_ICONQUESTION,
                )
            };
            if r != IDOK {
                return;
            }
        }
    }
    with(|a| a.close_tabs(index, &targets));
}

/// メニューの「エージェントを使う」の印を今の設定に合わせる。
fn check_use_agent(frame: HWND) {
    unsafe {
        let on = crate::remote::use_agent();
        CheckMenuItem(
            GetMenu(frame),
            u32::from(ID_WS_USE_AGENT),
            (MF_BYCOMMAND | if on { MF_CHECKED } else { MF_UNCHECKED }).0,
        );
    }
}

/// 「3270 で接続」: 接続先を尋ねて 3270 のタブを開く。
fn cmd_tn3270(hwnd: HWND) {
    let names: Vec<String> =
        with(|a| a.config.tn3270.host.keys().cloned().collect()).unwrap_or_default();
    let hint = if names.is_empty() {
        String::new()
    } else {
        format!("（設定の名前: {}）", names.join("、"))
    };
    let Some(input) = crate::goto::prompt_text(
        hwnd,
        "3270 で接続",
        &format!("接続先（ホスト[:ポート]、tn3270://LU名@ホスト:ポート、または設定の名前）{hint}:"),
        "",
    ) else {
        return;
    };
    let target = with(|a| tn3270::Target3270::parse(&input, &a.config));
    match target {
        Some(Ok(t)) => open_tab(Place::Tn3270(t)),
        Some(Err(e)) => error_box(hwnd, &e),
        None => {}
    }
}

/// 「3270 のファイル転送」: 転送の指定を尋ねて始める。
fn cmd_tn_transfer(hwnd: HWND) {
    let Some(Some((last, ccsid, busy))) = with(|a| {
        let tn = a.tab()?.tn.as_ref()?;
        let ccsid = tn.session.ccsid();
        let last = a
            .ft_last
            .clone()
            .unwrap_or_else(|| ftdlg::Choice::initial(ccsid));
        Some((last, ccsid, tn.session.transferring()))
    }) else {
        info_box(hwnd, "3270 のタブを表示してから選んでください。");
        return;
    };
    if busy {
        info_box(
            hwnd,
            "ファイル転送を実行中です。終わるのを待つか、取り消してください。",
        );
        return;
    }
    // 前回の「端末で変換」は今のタブの CCSID で
    let mut last = last;
    if let yy_3270::ind_file::Mode::Text(_) = last.request.mode {
        last.request.mode = yy_3270::ind_file::Mode::Text(ccsid);
    }
    let Some(choice) = ftdlg::show(hwnd, last, ccsid) else {
        return;
    };
    if let Some(Err(e)) = with(|a| a.tn_start_transfer(choice)) {
        error_box(hwnd, &format!("ファイル転送を始められません。\n{e}"));
    }
    focus_view();
}

/// マクロのスレッドからの頼みごとを処理する（ダイアログを出すものは状態を借りずに）。
fn macro_pump(hwnd: HWND) {
    use macros::Request;
    use yy_3270_macro::{Answer, Op};
    use yy_remote::PasswordStore;
    loop {
        let Some(Some(req)) = with(|a| a.macro_run.as_ref().and_then(|m| m.rx.try_recv().ok()))
        else {
            return;
        };
        match req {
            Request::Snapshot(tx) => {
                let s = with(|a| a.macro_snapshot()).unwrap_or_else(|| Err("終了しました".into()));
                let _ = tx.send(s);
            }
            Request::Act(Op::Ask(q), tx) => {
                let r = crate::goto::prompt_text(hwnd, "マクロ", &q, "");
                let _ = tx.send(Ok(Answer::Text(r)));
            }
            Request::Act(Op::Message(m), tx) => {
                info_box(hwnd, &m);
                let _ = tx.send(Ok(Answer::Done));
            }
            Request::Act(Op::Password(name), tx) => {
                let store = crate::credstore::WindowsCredentials;
                let key = macros::password_key(&name);
                let password = match store.load(&key) {
                    Some(s) => Some(s.password),
                    None => crate::goto::prompt_secret_with_check(
                        hwnd,
                        "マクロのパスワード",
                        &format!("「{name}」のパスワード（マクロには渡さず、画面のフィールドに入れます）:"),
                        "資格情報マネージャーに保存する",
                    )
                    .map(|(p, save)| {
                        if save {
                            let _ = store.save(
                                &key,
                                &yy_remote::SavedPassword {
                                    user: String::new(),
                                    password: p.clone(),
                                },
                            );
                        }
                        p
                    }),
                };
                let r = match password {
                    Some(p) => with(|a| a.macro_type(&p, true))
                        .unwrap_or_else(|| Err("終了しました".into()))
                        .map(|()| Answer::Done),
                    None => Err("パスワードの入力が取り消されました".into()),
                };
                let _ = tx.send(r);
            }
            Request::Act(op, tx) => {
                with(|a| a.macro_act(op, tx));
            }
            Request::Finished(r) => {
                let text = with(|a| a.macro_finished(&r)).unwrap_or_default();
                if let Err(e) = &r
                    && !e.stopped
                {
                    error_box(hwnd, &text);
                }
            }
        }
    }
}

/// 「マクロを実行」: マクロのファイルを選んで、表示している 3270 のタブで実行する。
fn cmd_macro_run(hwnd: HWND) {
    let Some(Some(folder)) = with(|a| {
        let tn = a.tab()?.tn.as_ref()?;
        (!tn.target.printer).then(|| macros::macro_folder(&a.config.tn3270.macros.folder))
    }) else {
        info_box(hwnd, "3270 の端末のタブを表示してから選んでください。");
        return;
    };
    if with(|a| a.macro_run.is_some()) == Some(true) {
        info_box(hwnd, "ほかのマクロを実行中です。止めてから選んでください。");
        return;
    }
    let Some(path) = macros::pick_file(hwnd, folder.as_deref(), None) else {
        return;
    };
    if let Some(Err(e)) = with(|a| a.macro_start(a.active, &path)) {
        error_box(hwnd, &format!("マクロを実行できません。\n{e}"));
    }
    focus_view();
}

/// 「操作の記録を開始・終了」。
fn cmd_macro_record(hwnd: HWND) {
    let recording = with(|a| a.recorder.is_some()).unwrap_or(false);
    if !recording {
        let started = with(|a| {
            let t = a.tab()?;
            let tn = t.tn.as_ref()?;
            if tn.target.printer {
                return None;
            }
            let id = t.id;
            a.recorder = Some((id, yy_3270_macro::Recorder::new()));
            a.tn_note("操作の記録を始めました（もう一度選ぶと終えて保存します）");
            Some(())
        })
        .flatten();
        if started.is_none() {
            info_box(hwnd, "3270 の端末のタブを表示してから選んでください。");
        }
        return;
    }
    let Some(Some((id, rec))) = with(|a| a.recorder.take()) else {
        return;
    };
    if rec.is_empty() {
        with(|a| a.tn_note("記録した操作はありません"));
        return;
    }
    let (label, folder) = with(|a| {
        (
            a.tabs
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.place.label())
                .unwrap_or_default(),
            macros::macro_folder(&a.config.tn3270.macros.folder),
        )
    })
    .unwrap_or_default();
    let password_name = if rec.has_password() {
        crate::goto::prompt_text(
            hwnd,
            "記録したマクロ",
            "パスワードの入力は記録していません。password(\"名前\") に使う資格情報の名前:",
            &label,
        )
        .unwrap_or_else(|| label.clone())
    } else {
        String::new()
    };
    let Some(path) = macros::pick_file(hwnd, folder.as_deref(), Some("recorded.rhai")) else {
        return;
    };
    let script = rec.finish(&label, &password_name);
    match std::fs::write(&path, script) {
        Ok(()) => {
            with(|a| {
                a.tn_note(&format!(
                    "記録したマクロを {} に保存しました",
                    path.display()
                ))
            });
            open_in_editor(hwnd, &path);
        }
        Err(e) => error_box(hwnd, &format!("{} に保存できません: {e}", path.display())),
    }
}

/// 「3270 のプリンターを接続」: 表示している 3270 の端末に対応するプリンターのタブを開く。
fn cmd_tn_printer(hwnd: HWND) {
    let target = with(|a| {
        let tn = a.tab()?.tn.as_ref()?;
        if tn.target.printer {
            return Some(Err(
                "プリンターのタブではなく、3270 の端末のタブで選んでください。".to_owned(),
            ));
        }
        Some(tn.target.printer_target(tn.session.oia().device.as_deref()))
    })
    .flatten();
    match target {
        Some(Ok(t)) => open_tab(Place::Tn3270(t)),
        Some(Err(e)) => error_box(hwnd, &e),
        None => info_box(hwnd, "3270 の端末のタブを表示してから選んでください。"),
    }
}

/// 受け取った印刷を印刷する・PDF やテキストで保存する（表示しているプリンターのタブ）。
fn cmd_print_output(hwnd: HWND, id: u16) {
    let Some(Some((last, printer_name))) = with(|a| {
        let tn = a.tab()?.tn.as_ref()?;
        if !tn.target.printer {
            return None;
        }
        Some((
            tn.jobs.last().map(|j| j.number),
            a.config.tn3270.printer.printer_name.clone(),
        ))
    }) else {
        info_box(
            hwnd,
            "3270 のプリンターのタブを表示してから選んでください。",
        );
        return;
    };
    let Some(last) = last else {
        info_box(hwnd, "まだ印刷を受け取っていません。");
        return;
    };
    let Some(number) = crate::goto::prompt_line(hwnd, "印刷の番号（#）:", last as u64)
    else {
        return;
    };
    let number = number as usize;
    let Some(Some((title, job))) = with(|a| a.stored_job(number)) else {
        error_box(hwnd, &format!("印刷 #{number} はこのタブにありません。"));
        return;
    };
    let result = match id {
        ID_PR_PRINT => {
            let name = if printer_name.trim().is_empty() {
                printer::default_printer().unwrap_or_default()
            } else {
                printer_name
            };
            printer::print(&job, &name, None, &title)
                .map(|n| format!("{n} ページを {name} に印刷しました"))
        }
        _ => {
            let pdf = id == ID_PR_PDF;
            let ext = if pdf { "pdf" } else { "txt" };
            let Some(path) =
                printer::pick_save(hwnd, &printer::output_name(&title, "", number, ext), pdf)
            else {
                return;
            };
            if pdf {
                printer::save_pdf(&job, &path, &title)
                    .map(|_| format!("{} に保存しました", path.display()))
            } else {
                printer::save_text(&job, &path)
                    .map(|()| format!("{} に保存しました", path.display()))
            }
        }
    };
    match result {
        Ok(msg) => {
            with(|a| a.set_job_output(number, msg));
        }
        Err(e) => error_box(hwnd, &e),
    }
}

fn cmd_add_remote_folder(hwnd: HWND) {
    if !crate::remote::available() {
        info_box(hwnd, "この yyterm には SSH の機能が組み込まれていません。");
        return;
    }
    let Some(p) = crate::remotedlg::show(
        hwnd,
        crate::remotedlg::Mode::Folder,
        crate::remote::last(),
        None,
        false,
    ) else {
        return;
    };
    crate::remote::set_last(p.uri.clone());
    with(|a| {
        a.sidebar.add_folder(&PathBuf::from(p.uri.to_string()));
        a.save_workspace();
    });
    layout();
}

/// ファイルをエディタ（yyeditor）で開く。
fn open_in_editor(hwnd: HWND, path: &Path) {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("yyeditor.exe")));
    let r = match exe.filter(|e| e.is_file()) {
        Some(exe) => std::process::Command::new(exe)
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string()),
        None => Err("yyeditor.exe が yyterm.exe と同じフォルダにありません。".to_owned()),
    };
    if let Err(e) = r {
        error_box(
            hwnd,
            &format!("{} を開けませんでした。\n{e}", path.display()),
        );
    }
}

// ---- サイドバー -------------------------------------------------------------------------

/// サイドバーの項目を開く（フォルダはそこでターミナルを開き、ファイルはエディタで開く）。
fn activate_node(hwnd: HWND, path: PathBuf, is_dir: bool) {
    if !is_dir {
        open_in_editor(hwnd, &path);
        return;
    }
    let place = match sidebar::remote_uri(&path) {
        Some(u) => Place::Remote {
            target: u.target(),
            dir: Some(u.path),
        },
        None => Place::Local(Some(path)),
    };
    open_tab(place);
}

fn on_tree_notify(hwnd: HWND, hdr: &NMHDR, lparam: LPARAM) -> LRESULT {
    match hdr.code {
        TVN_ITEMEXPANDINGW => {
            let nm = unsafe { &*(lparam.0 as *const NMTREEVIEWW) };
            if nm.action == TVE_EXPAND {
                let pending = with(|a| {
                    a.sidebar
                        .expanding(nm.itemNew.hItem)
                        .map(|i| (a.sidebar.generation, i))
                })
                .flatten();
                if let Some((generation, index)) = pending {
                    // リモートのフォルダは読んでから展開する（ここでは接続しない）
                    unsafe {
                        let _ = PostMessageW(
                            Some(hwnd),
                            WM_APP_TERM_REMOTE_DIR,
                            WPARAM(generation),
                            LPARAM(index as isize),
                        );
                    }
                    return LRESULT(1);
                }
            }
            LRESULT(0)
        }
        NM_DBLCLK => match with(|a| a.sidebar.selected_node()).flatten() {
            Some((_, path, is_dir, _)) => {
                activate_node(hwnd, path, is_dir);
                LRESULT(1)
            }
            None => LRESULT(0),
        },
        TVN_KEYDOWN => {
            let nm = unsafe { &*(lparam.0 as *const NMTVKEYDOWN) };
            if nm.wVKey == VK_RETURN.0 {
                if let Some((_, path, is_dir, _)) = with(|a| a.sidebar.selected_node()).flatten() {
                    activate_node(hwnd, path, is_dir);
                }
                return LRESULT(1);
            }
            if nm.wVKey == VK_DELETE.0 {
                with(|a| {
                    if let Some((_, path, _, true)) = a.sidebar.selected_node() {
                        a.sidebar.remove_folder(&path);
                        a.save_workspace();
                    }
                });
                return LRESULT(1);
            }
            LRESULT(0)
        }
        NM_RCLICK => {
            context_menu(hwnd);
            LRESULT(1)
        }
        _ => LRESULT(0),
    }
}

fn context_menu(hwnd: HWND) {
    let Some(tree) = with(|a| a.sidebar.tree) else {
        return;
    };
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
        // 右クリックした項目を選ぶ
        let mut client = pt;
        let _ = ScreenToClient(tree, &mut client);
        let mut hit = TVHITTESTINFO {
            pt: client,
            ..Default::default()
        };
        let item = SendMessageW(
            tree,
            TVM_HITTEST,
            None,
            Some(LPARAM(&mut hit as *mut _ as isize)),
        )
        .0;
        if item != 0 {
            SendMessageW(
                tree,
                TVM_SELECTITEM,
                Some(WPARAM(TVGN_CARET as usize)),
                Some(LPARAM(item)),
            );
        }
    }
    let node = with(|a| a.sidebar.selected_node()).flatten();
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        return;
    };
    unsafe {
        let item = |id: u32, text: PCWSTR| AppendMenuW(menu, MF_STRING, id as usize, text);
        if let Some((_, _, is_dir, root)) = &node {
            if *is_dir {
                let _ = item(CM_OPEN_HERE, w!("ここでターミナルを開く(&T)"));
                let _ = item(CM_REFRESH, w!("最新の情報に更新(&R)"));
            } else {
                let _ = item(CM_OPEN_EDITOR, w!("エディタで開く(&E)"));
            }
            let _ = item(CM_COPY_PATH, w!("パスをコピー(&C)"));
            if *root {
                let _ = item(CM_REMOVE, w!("ワークスペースから外す(&D)"));
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        }
        let _ = item(CM_ADD, w!("フォルダを追加(&F)..."));
        let _ = item(CM_ADD_REMOTE, w!("リモートのフォルダを追加(&S)..."));
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        let cmd = cmd.0 as u32;
        match (cmd, node) {
            (CM_ADD, _) => cmd_add_folder(hwnd),
            (CM_ADD_REMOTE, _) => cmd_add_remote_folder(hwnd),
            (CM_OPEN_HERE, Some((_, path, _, _))) => activate_node(hwnd, path, true),
            (CM_OPEN_EDITOR, Some((_, path, _, _))) => open_in_editor(hwnd, &path),
            (CM_COPY_PATH, Some((_, path, _, _))) => {
                let _ = crate::clipboard::set_text(hwnd, &path.to_string_lossy(), false);
            }
            (CM_REFRESH, Some(_)) => {
                with(|a| a.sidebar.rebuild());
            }
            (CM_REMOVE, Some((_, path, _, _))) => {
                with(|a| {
                    a.sidebar.remove_folder(&path);
                    a.save_workspace();
                });
            }
            _ => {}
        }
    }
}

/// リモートのフォルダの中身を読んでサイドバーに並べる（接続中はアプリの状態を借りない）。
fn load_remote_dir(hwnd: HWND, generation: usize, index: usize) {
    let Some(uri) = with(|a| a.sidebar.pending_remote(generation, index)).flatten() else {
        return;
    };
    let mut listed = crate::remote::list_dir(&uri);
    // 中身がフォルダ 1 つだけなら束ねて（`a/b`。VS Code と同じ）、その中を読む
    for _ in 0..workspace::COMPACT_DEPTH {
        let only = match &listed {
            Ok((entries, 0)) => match entries.as_slice() {
                [e] if e.is_dir => e.clone(),
                _ => break,
            },
            _ => break,
        };
        let Some(next) = with(|a| a.sidebar.merge_single(generation, index, &only)).flatten()
        else {
            break;
        };
        listed = crate::remote::list_dir(&next);
    }
    match listed {
        Ok((entries, _)) => {
            with(|a| {
                if a.sidebar.pending_remote(generation, index).is_some() {
                    a.sidebar.add_children(index, entries);
                    a.sidebar.expand(index);
                }
            });
        }
        Err(e) => error_box(hwnd, &format!("フォルダを開けません。\n{e}")),
    }
}

// ---- 端末の画面 -------------------------------------------------------------------------

/// 文字以外のキー（WM_KEYDOWN で送るもの）。
fn special_key(vk: VIRTUAL_KEY) -> Option<Key> {
    Some(match vk {
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        VK_INSERT => Key::Insert,
        VK_DELETE => Key::Delete,
        v if (VK_F1.0..=VK_F12.0).contains(&v.0) => Key::F((v.0 - VK_F1.0 + 1) as u8),
        _ => return None,
    })
}

fn point(lparam: LPARAM) -> (i32, i32) {
    (
        (lparam.0 & 0xffff) as i16 as i32,
        ((lparam.0 >> 16) & 0xffff) as i16 as i32,
    )
}

extern "system" fn view_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
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
            if with(|a| a.on_view_size(w, h)).is_none() {
                retry_later(WM_APP_TERM_LAYOUT);
            }
            LRESULT(0)
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            // ほかの処理の中でフォーカスが変わると状態を借りられないので、後で処理する
            unsafe {
                let _ = PostMessageW(
                    Some(hwnd),
                    WM_APP_TERM_FOCUS_CHANGED,
                    WPARAM(usize::from(msg == WM_SETFOCUS)),
                    LPARAM(0),
                );
            }
            LRESULT(0)
        }
        WM_APP_TERM_FOCUS_CHANGED => {
            let focused = wparam.0 == 1;
            with(|a| {
                if let Some(t) = a.tab()
                    && let Some(r) = t.term.focus_report(focused)
                {
                    t.send(r.to_vec());
                }
                a.invalidate();
            });
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = VIRTUAL_KEY(wparam.0 as u16);
            let m = mods();
            // 3270 のタブは独自のキーの割り当て（14 章 7）
            if with(|a| a.is_tn()) == Some(true) {
                // マクロの実行中は Ctrl+Break（Pause）で止める
                if (vk == VK_CANCEL || vk == VK_PAUSE)
                    && with(|a| a.macro_tab() == Some(a.active)) == Some(true)
                {
                    with(|a| a.macro_stop());
                    return LRESULT(0);
                }
                if let Some(k) = tn3270::map_key(vk, m) {
                    with(|a| a.tn_key(k));
                    return LRESULT(0);
                }
                return crate::default_proc(hwnd, msg, wparam, lparam);
            }
            // Shift+PageUp などはスクロールバックを見る
            if m.shift && !m.ctrl && !m.alt {
                let page = with(|a| a.grid.1 as isize).unwrap_or(24);
                let alt_screen =
                    with(|a| a.tab().is_some_and(|t| t.term.modes().alt_screen)) == Some(true);
                if !alt_screen {
                    let lines = match vk {
                        VK_PRIOR => Some(page),
                        VK_NEXT => Some(-page),
                        VK_UP => Some(1),
                        VK_DOWN => Some(-1),
                        _ => None,
                    };
                    if let Some(l) = lines {
                        with(|a| a.scroll_view(l));
                        return LRESULT(0);
                    }
                }
            }
            if m.ctrl && vk == VK_SPACE {
                with(|a| a.send_input(vec![0]));
                // 続く WM_CHAR（空白）を捨てる
                unsafe {
                    let mut msg = MSG::default();
                    let _ = PeekMessageW(&mut msg, Some(hwnd), WM_CHAR, WM_CHAR, PM_REMOVE);
                }
                return LRESULT(0);
            }
            if let Some(key) = special_key(vk)
                && !(msg == WM_SYSKEYDOWN && vk == VK_F4)
            {
                with(|a| a.send_key(key, m));
                return LRESULT(0);
            }
            crate::default_proc(hwnd, msg, wparam, lparam)
        }
        WM_CHAR | WM_SYSCHAR => {
            let unit = wparam.0 as u16;
            let alt = msg == WM_SYSCHAR;
            with(|a| {
                // サロゲート ペアは 2 回に分けて届く
                if (0xd800..0xdc00).contains(&unit) {
                    a.high_surrogate = Some(unit);
                    return;
                }
                let c = match a.high_surrogate.take() {
                    Some(hi) if (0xdc00..0xe000).contains(&unit) => {
                        char::decode_utf16([hi, unit]).next().and_then(|r| r.ok())
                    }
                    _ => char::from_u32(u32::from(unit)),
                };
                let Some(c) = c else { return };
                // 3270 のタブ: 文字だけを入れる（Enter・Tab などは WM_KEYDOWN で処理した）
                if a.is_tn() {
                    if !c.is_control() && !alt {
                        a.tn_key(yy_3270::Key::Char(c));
                    }
                    return;
                }
                let m = Mods { alt, ..mods() };
                let key = match c {
                    '\r' => Key::Enter,
                    '\u{8}' => Key::Backspace,
                    '\u{7f}' => {
                        // Ctrl+Backspace
                        a.send_key(Key::Backspace, Mods { ctrl: true, ..m });
                        return;
                    }
                    '\t' => Key::Tab,
                    '\u{1b}' => Key::Escape,
                    c if (c as u32) < 0x20 => {
                        // Ctrl+文字は Windows が制御文字にしている
                        let mut v = Vec::new();
                        if alt {
                            v.push(0x1b);
                        }
                        v.push(c as u8);
                        a.send_input(v);
                        return;
                    }
                    c => Key::Char(c),
                };
                let m = if matches!(key, Key::Char(_)) {
                    // 文字は Ctrl・Shift を反映済み
                    Mods {
                        alt,
                        ..Mods::default()
                    }
                } else {
                    m
                };
                a.send_key(key, m);
            });
            LRESULT(0)
        }
        WM_IME_STARTCOMPOSITION => {
            with(|a| a.set_ime_position());
            crate::default_proc(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            unsafe {
                let _ = SetFocus(Some(hwnd));
                SetCapture(hwnd);
            }
            let (x, y) = point(lparam);
            with(|a| {
                if a.reports_mouse() {
                    a.report_mouse(MouseEvent::Press(Button::Left), x, y);
                    if let Some(t) = a.tab_mut() {
                        t.mouse_button = Some(Button::Left);
                    }
                    return;
                }
                if msg == WM_LBUTTONDBLCLK {
                    let Some(p) = a.pos_at(x, y, false) else {
                        return;
                    };
                    if let Some(t) = a.tab_mut() {
                        let (s, e) = t.term.word_at(p);
                        t.selection = (s != e).then_some((s, e));
                        t.selecting = false;
                    }
                    if a.config.terminal.copy_on_select {
                        a.copy();
                    }
                    a.invalidate();
                    return;
                }
                let p = a.pos_at(x, y, true);
                if let Some(t) = a.tab_mut() {
                    t.anchor = p;
                    t.selection = None;
                    t.selecting = true;
                }
                a.invalidate();
            });
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = point(lparam);
            with(|a| {
                if a.reports_mouse() {
                    let b = a.tab().and_then(|t| t.mouse_button);
                    a.report_mouse(MouseEvent::Move(b), x, y);
                    return;
                }
                let selecting = a.tab().is_some_and(|t| t.selecting);
                if !selecting {
                    return;
                }
                // 画面の外まで引っ張ったらスクロールする
                let mut rc = RECT::default();
                unsafe {
                    let _ = GetClientRect(a.view, &mut rc);
                }
                if y < 0 {
                    a.scroll_view(1);
                } else if y > rc.bottom {
                    a.scroll_view(-1);
                }
                let p = a.pos_at(x, y, true);
                if let Some(t) = a.tab_mut()
                    && let (Some(anchor), Some(p)) = (t.anchor, p)
                {
                    t.selection = (anchor != p).then_some((anchor, p));
                }
                a.invalidate();
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            let (x, y) = point(lparam);
            with(|a| {
                if a.tab().is_some_and(|t| t.mouse_button.is_some()) {
                    a.report_mouse(MouseEvent::Release(Button::Left), x, y);
                    if let Some(t) = a.tab_mut() {
                        t.mouse_button = None;
                    }
                    return;
                }
                if let Some(t) = a.tab_mut() {
                    t.selecting = false;
                }
                // 3270: ドラッグしていなければ、クリックした位置にカーソルを移す
                if a.is_tn() && a.tab().is_some_and(|t| t.selection.is_none()) {
                    let (r, c) = a.painter.cell_at(x, y, false);
                    let addr = a.tab().and_then(|t| {
                        let s = t.tn.as_ref()?.session.screen();
                        (r >= 0 && c >= 0 && (r as usize) < s.rows && (c as usize) < s.cols)
                            .then(|| r as usize * s.cols + c as usize)
                    });
                    if let Some(addr) = addr {
                        a.tn_key(yy_3270::Key::MoveTo(addr));
                    }
                }
                if a.config.terminal.copy_on_select
                    && a.tab().is_some_and(|t| t.selection.is_some())
                {
                    a.copy();
                }
            });
            LRESULT(0)
        }
        WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            let (x, y) = point(lparam);
            let button = if msg == WM_RBUTTONDOWN {
                Button::Right
            } else {
                Button::Middle
            };
            with(|a| {
                if a.reports_mouse() {
                    a.report_mouse(MouseEvent::Press(button), x, y);
                    return;
                }
                // 選択していればコピー、していなければ貼り付け（コンソールと同じ）
                if button == Button::Right && a.copy() {
                    if let Some(t) = a.tab_mut() {
                        t.selection = None;
                    }
                    a.invalidate();
                } else {
                    a.paste();
                }
            });
            LRESULT(0)
        }
        WM_RBUTTONUP | WM_MBUTTONUP => {
            let (x, y) = point(lparam);
            let button = if msg == WM_RBUTTONUP {
                Button::Right
            } else {
                Button::Middle
            };
            with(|a| {
                if a.reports_mouse() {
                    a.report_mouse(MouseEvent::Release(button), x, y);
                }
            });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = hiword(wparam.0) as u16 as i16 as i32;
            let m = mods();
            if m.ctrl {
                with(|a| a.zoom(Some(if delta > 0 { 1.0 } else { -1.0 })));
                return LRESULT(0);
            }
            let mut pt = POINT {
                x: (lparam.0 & 0xffff) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xffff) as i16 as i32,
            };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            let lines = (delta * 3 / 120).clamp(-30, 30);
            with(|a| {
                if a.reports_mouse() {
                    let b = if lines > 0 {
                        Button::WheelUp
                    } else {
                        Button::WheelDown
                    };
                    for _ in 0..lines.unsigned_abs().max(1) {
                        a.report_mouse(MouseEvent::Press(b), pt.x, pt.y);
                        if let Some(t) = a.tab_mut() {
                            t.mouse_cell = (usize::MAX, usize::MAX);
                        }
                    }
                    return;
                }
                let alt_screen = a.tab().is_some_and(|t| t.term.modes().alt_screen);
                if alt_screen {
                    // less・vim などはカーソルキーで送る
                    let key = if lines > 0 { Key::Up } else { Key::Down };
                    for _ in 0..lines.unsigned_abs() {
                        a.send_key(key, Mods::default());
                    }
                } else {
                    a.scroll_view(lines as isize);
                }
            });
            LRESULT(0)
        }
        WM_VSCROLL => {
            let code = SCROLLBAR_COMMAND(loword(wparam.0) as i32);
            with(|a| {
                let Some(t) = a.tab() else { return };
                let hist = t.term.history_len() as isize;
                let page = t.term.rows() as isize;
                let pos = hist - t.back.min(hist as usize) as isize;
                let new_pos = match code {
                    SB_LINEUP => pos - 1,
                    SB_LINEDOWN => pos + 1,
                    SB_PAGEUP => pos - page,
                    SB_PAGEDOWN => pos + page,
                    SB_TOP => 0,
                    SB_BOTTOM => hist,
                    SB_THUMBTRACK | SB_THUMBPOSITION => {
                        let mut si = SCROLLINFO {
                            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                            fMask: SIF_TRACKPOS,
                            ..Default::default()
                        };
                        unsafe {
                            let _ = GetScrollInfo(a.view, SB_VERT, &mut si);
                        }
                        si.nTrackPos as isize
                    }
                    _ => pos,
                };
                a.scroll_view(pos - new_pos.clamp(0, hist));
            });
            LRESULT(0)
        }
        _ => crate::default_proc(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// メッセージを処理しながら `done` を待つ。
    fn pump_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        let mut msg = MSG::default();
        while start.elapsed() < limit {
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// ウィンドウを作り、ConPTY のシェルの出力が端末の画面に届くこと（Windows の CI で確かめる）。
    #[test]
    fn runs_a_shell_in_the_window() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        QUIET.with(|q| q.set(true));
        let dir = std::env::temp_dir();
        let frame = create(
            Place::Local(Some(dir)),
            None,
            Some(vec![
                "cmd.exe".into(),
                "/c".into(),
                "echo hello-yyterm& echo 日本語".into(),
            ]),
        )
        .unwrap();
        let screen = || {
            with(|a| {
                let t = a.tabs.first()?;
                let text: String = (0..t.term.rows())
                    .map(|r| t.term.screen_row(r).text() + "\n")
                    .collect();
                Some((text, t.exited))
            })
            .flatten()
        };
        let ok = pump_until(Duration::from_secs(30), || {
            screen().is_some_and(|(text, exited)| text.contains("hello-yyterm") && exited.is_some())
        });
        let (text, exited) = screen().expect("a tab");
        assert!(ok, "{text}");
        assert!(text.contains("日本語"), "{text}");
        assert_eq!(exited, Some(Some(0)));
        // 画面の大きさから端末の大きさが決まっている
        let (cols, rows) = with(|a| a.grid).unwrap();
        assert!(cols >= 20 && rows >= 5, "{cols}x{rows}");
        // 終わったタブは Enter で閉じ、タブがなくなるとウィンドウを閉じる
        with(|a| a.send_key(Key::Enter, Mods::default()));
        assert!(pump_until(Duration::from_secs(5), || unsafe {
            !IsWindow(Some(frame)).as_bool()
        }));
        shutdown();
    }
}

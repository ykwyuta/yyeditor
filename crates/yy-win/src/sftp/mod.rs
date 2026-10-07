//! ファイル転送のアプリ（yysftp。13 章）。
//!
//! エクスプローラーのように、左に接続先とフォルダのツリー、右にフォルダの中身（詳細表示）、
//! 上にアドレスバーと「戻る・進む・上へ」、下に転送の一覧と記録を置く。手元のファイルは
//! エクスプローラーからドラッグ＆ドロップするか「アップロード」で送り、選んだ項目は
//! 「ダウンロード」で受け取る。
//!
//! 一覧・フォルダの操作は SFTP（`yy_remote::sftp`）、転送はレジュームできる転送の仕組み
//! （`yy_remote::xfer`。SFTP・SCP）を使う。接続（組み込みの SSH・踏み台・プロキシ・ホスト鍵・
//! 保存したパスワード・接続の記録）、同梱フォント、設定、ダイアログはエディタ・ターミナルと共通。

mod queue;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{
    DragAcceptFiles, DragFinish, DragQueryFileW, HDROP, SHFILEINFOW, SHGFI_SMALLICON,
    SHGFI_SYSICONINDEX, SHGFI_TYPENAME, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, PWSTR, Result, w};
use yy_config::Config;
use yy_remote::RemoteFs;
use yy_remote::RemoteUri;
use yy_remote::sftp::Attrs;
use yy_remote::uri::Target;
use yy_remote::xfer::Protocol;

use crate::util::{Context, error_box, info_box};
use crate::{hiword, loword};

const FRAME_CLASS: PCWSTR = w!("YYSftpFrame");

/// 転送の進み・記録が届いた
pub(super) const WM_APP_XFER_EVENT: u32 = WM_APP + 70;
/// ツリーのフォルダの中身を読む（`WPARAM` はツリーの世代、`LPARAM` は項目の番号）
const WM_APP_TREE_LOAD: u32 = WM_APP + 71;
/// 一覧を開き直す（ツリーで選んだ項目へ）
const WM_APP_TREE_GO: u32 = WM_APP + 72;
/// 送り終えたファイルのフォルダを表示していれば読み直す
const WM_APP_XFER_REFRESH: u32 = WM_APP + 74;

// 子ウィンドウ
const ID_TREE: u16 = 4001;
const ID_LIST: u16 = 4002;
const ID_ADDRESS: u16 = 4003;
const ID_STATUS: u16 = 4004;
const ID_BOTTOM_TABS: u16 = 4005;
const ID_JOBS: u16 = 4006;
const ID_LOG: u16 = 4007;
// ツールバーのボタン（メニューと同じ番号）
const ID_BACK: u16 = 4101;
const ID_FORWARD: u16 = 4102;
const ID_UP: u16 = 4103;
const ID_REFRESH: u16 = 4104;
const ID_UPLOAD: u16 = 4105;
const ID_DOWNLOAD: u16 = 4106;
const ID_NEW_FOLDER: u16 = 4107;
const ID_DELETE: u16 = 4108;
const ID_GO: u16 = 4109;
// メニュー
const ID_CONNECT: u16 = 4201;
const ID_EXIT: u16 = 4202;
const ID_UPLOAD_FOLDER: u16 = 4203;
const ID_DOWNLOAD_TO: u16 = 4204;
const ID_RENAME: u16 = 4205;
const ID_SELECT_ALL: u16 = 4206;
const ID_COPY_PATH: u16 = 4207;
const ID_HIDDEN: u16 = 4208;
const ID_PROTO_SFTP: u16 = 4209;
const ID_PROTO_SCP: u16 = 4210;
const ID_PAUSE: u16 = 4211;
const ID_RESUME: u16 = 4212;
const ID_CANCEL_JOB: u16 = 4213;
const ID_CLEAR_DONE: u16 = 4214;
const ID_RESUME_ALL: u16 = 4215;
const ID_TERMINAL: u16 = 4216;
const ID_EDITOR: u16 = 4217;
const ID_CONNECT_LOG: u16 = 4218;
const ID_TRANSFER_LOG: u16 = 4219;
const ID_FORGET: u16 = 4220;
const ID_SETTINGS: u16 = 4221;
const ID_ABOUT: u16 = 4222;
const ID_SHOW_JOBS: u16 = 4223;
const ID_SHOW_LOG: u16 = 4224;
const ID_FOCUS_ADDRESS: u16 = 4225;
const ID_DISCONNECT: u16 = 4226;
const ID_OPEN_LOCAL: u16 = 4227;
const ID_USE_AGENT: u16 = 4228;

/// ツールバーのボタン（番号, 文字）
const BUTTONS: [(u16, &str); 8] = [
    (ID_BACK, "← 戻る"),
    (ID_FORWARD, "進む →"),
    (ID_UP, "↑ 上へ"),
    (ID_REFRESH, "更新"),
    (ID_UPLOAD, "アップロード"),
    (ID_DOWNLOAD, "ダウンロード"),
    (ID_NEW_FOLDER, "新しいフォルダ"),
    (ID_DELETE, "削除"),
];

/// 表示している場所。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Loc {
    target: Target,
    path: Vec<u8>,
}

impl Loc {
    fn uri(&self) -> RemoteUri {
        RemoteUri {
            user: self.target.user.clone(),
            host: self.target.host.clone(),
            port: self.target.port,
            path: self.path.clone(),
        }
    }

    fn child(&self, name: &[u8]) -> Loc {
        Loc {
            target: self.target.clone(),
            path: yy_remote::join_remote(&self.path, name),
        }
    }

    fn parent(&self) -> Option<Loc> {
        if self.path == b"/" || self.path.is_empty() {
            return None;
        }
        let i = self.path.iter().rposition(|&b| b == b'/')?;
        Some(Loc {
            target: self.target.clone(),
            path: if i == 0 {
                b"/".to_vec()
            } else {
                self.path[..i].to_vec()
            },
        })
    }

    /// アドレスバーの表示（`ユーザー@ホスト:/パス`）。
    fn address(&self) -> String {
        format!("{}:{}", self.target, yy_remote::display(&self.path))
    }
}

/// フォルダの 1 項目。
#[derive(Clone, Debug)]
struct Item {
    name: Vec<u8>,
    attrs: Attrs,
}

impl Item {
    fn is_dir(&self) -> bool {
        self.attrs.is_dir()
    }
}

/// ツリーの項目（接続先、またはフォルダ）。
struct Node {
    target: Target,
    /// フォルダ（接続先そのものは `None`）
    path: Option<Vec<u8>>,
    item: HTREEITEM,
    loaded: bool,
}

/// 接続先ごとの一覧用の SFTP。
struct Browser {
    target: Target,
    /// 一覧・ファイル操作（エージェントか SFTP）
    fs: Arc<dyn RemoteFs>,
    transport: Arc<dyn yy_remote::Transport>,
    home: Vec<u8>,
}

struct App {
    frame: HWND,
    tree: HWND,
    list: HWND,
    address: HWND,
    status: HWND,
    buttons: Vec<HWND>,
    go: HWND,
    config: Config,
    protocol: Protocol,
    show_hidden: bool,
    browsers: Vec<Browser>,
    loc: Option<Loc>,
    items: Vec<Item>,
    back: Vec<Loc>,
    forward: Vec<Loc>,
    /// 並べ替え（列, 昇順）
    sort: (i32, bool),
    /// 拡張子ごとのアイコンと種類の名前
    icons: HashMap<String, (i32, String)>,
    folder_icon: i32,
    nodes: Vec<Node>,
    generation: usize,
    queue: queue::Queue,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

fn set_status(text: &str) {
    with(|a| a.set_status(text));
}

/// ファイル転送のアプリを起動し、ウィンドウが閉じられるまでメッセージループを回す。
///
/// `initial` はコマンドラインの引数（`ssh://接続先/パス` か `ユーザー@ホスト`）。
pub fn run_sftp(initial: Option<String>, ssh: Option<yy_remote::ConnectorFactory>) -> Result<()> {
    crate::util::set_app_name("yysftp");
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
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES
                | ICC_TAB_CLASSES
                | ICC_TREEVIEW_CLASSES
                | ICC_LISTVIEW_CLASSES
                | ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
    }
    let frame = create(ssh)?;
    // 前回中断した転送
    queue::offer_resume(frame);
    if let Some(loc) = initial.as_deref().and_then(parse_location) {
        go(loc, true);
    }
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if (msg.message == WM_KEYDOWN || msg.message == WM_SYSKEYDOWN) && shortcut(&msg) {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    with(|a| a.queue.shutdown());
    APP.with(|a| a.borrow_mut().take());
    Ok(())
}

/// `ssh://接続先/パス`・`接続先:/パス`・`接続先` を読む（パスがなければホーム）。
fn parse_location(s: &str) -> Option<Loc> {
    let s = s.trim();
    if let Some(u) = RemoteUri::parse(s) {
        return Some(Loc {
            target: u.target(),
            path: u.path,
        });
    }
    // `ssh://接続先`（パスなし）はホーム
    if let Some(rest) = s.strip_prefix("ssh://") {
        return Target::parse(rest.trim_end_matches('/')).map(|target| Loc {
            target,
            path: Vec::new(),
        });
    }
    // `ユーザー@ホスト:/パス`（ポートと区別するため、パスは / で始まるもの）
    if let Some((t, p)) = s.split_once(":/") {
        let target = Target::parse(t)?;
        return Some(Loc {
            target,
            path: format!("/{p}").into_bytes(),
        });
    }
    Target::parse(s).map(|target| Loc {
        target,
        path: Vec::new(),
    })
}

fn create(ssh: Option<yy_remote::ConnectorFactory>) -> Result<HWND> {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
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
        let (config, config_error) = Config::load();
        crate::font::register_gdi();
        let frame = CreateWindowExW(
            WS_EX_ACCEPTFILES,
            FRAME_CLASS,
            w!("yysftp"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100,
            760,
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
                    0,
                    0,
                    Some(frame),
                    Some(HMENU(id as isize as *mut _)),
                    Some(instance),
                    None,
                );
                if let Ok(h) = h {
                    SendMessageW(
                        h,
                        WM_SETFONT,
                        Some(WPARAM(ui_font.0 as usize)),
                        Some(LPARAM(1)),
                    );
                }
                h
            };
        let mut buttons = Vec::new();
        for (id, text) in BUTTONS {
            let t = HSTRING::from(text);
            buttons.push(child(
                w!("BUTTON"),
                PCWSTR(t.as_ptr()),
                WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
                WINDOW_EX_STYLE::default(),
                id,
            )?);
        }
        let address = child(
            w!("EDIT"),
            PCWSTR::null(),
            WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            WS_EX_CLIENTEDGE,
            ID_ADDRESS,
        )?;
        let go = child(
            w!("BUTTON"),
            w!("移動"),
            WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
            WINDOW_EX_STYLE::default(),
            ID_GO,
        )?;
        let tree = child(
            WC_TREEVIEWW,
            PCWSTR::null(),
            WS_TABSTOP
                | WINDOW_STYLE(
                    TVS_HASBUTTONS | TVS_LINESATROOT | TVS_SHOWSELALWAYS | TVS_FULLROWSELECT,
                ),
            WS_EX_CLIENTEDGE,
            ID_TREE,
        )?;
        let list = child(
            WC_LISTVIEWW,
            PCWSTR::null(),
            WS_TABSTOP
                | WINDOW_STYLE(
                    LVS_REPORT | LVS_SHOWSELALWAYS | LVS_EDITLABELS | LVS_SHAREIMAGELISTS,
                ),
            WS_EX_CLIENTEDGE,
            ID_LIST,
        )?;
        let status = child(
            STATUSCLASSNAMEW,
            PCWSTR::null(),
            WINDOW_STYLE(SBARS_SIZEGRIP),
            WINDOW_EX_STYLE::default(),
            ID_STATUS,
        )?;
        let _ = SetWindowTheme(tree, w!("Explorer"), None);
        let _ = SetWindowTheme(list, w!("Explorer"), None);
        SendMessageW(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(0)),
            Some(LPARAM(
                (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_HEADERDRAGDROP) as isize,
            )),
        );
        SendMessageW(
            tree,
            TVM_SETEXTENDEDSTYLE,
            Some(WPARAM(TVS_EX_DOUBLEBUFFER as usize)),
            Some(LPARAM(TVS_EX_DOUBLEBUFFER as isize)),
        );
        // システムの小さいアイコン（エクスプローラーと同じ）
        let mut info = SHFILEINFOW::default();
        let images = SHGetFileInfoW(
            w!("folder"),
            FILE_ATTRIBUTE_DIRECTORY,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
        );
        let folder_icon = info.iIcon;
        if images != 0 {
            SendMessageW(
                list,
                LVM_SETIMAGELIST,
                Some(WPARAM(LVSIL_SMALL as usize)),
                Some(LPARAM(images as isize)),
            );
            SendMessageW(
                tree,
                TVM_SETIMAGELIST,
                Some(WPARAM(TVSIL_NORMAL as usize)),
                Some(LPARAM(images as isize)),
            );
        }
        for (i, (text, width, right)) in [
            ("名前", 300, false),
            ("更新日時", 150, false),
            ("種類", 140, false),
            ("サイズ", 100, true),
            ("属性", 100, false),
        ]
        .iter()
        .enumerate()
        {
            let t = crate::util::wide(text);
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT,
                fmt: if *right { LVCFMT_RIGHT } else { LVCFMT_LEFT },
                cx: width * dpi as i32 / 96,
                pszText: PWSTR(t.as_ptr() as *mut _),
                ..Default::default()
            };
            SendMessageW(
                list,
                LVM_INSERTCOLUMNW,
                Some(WPARAM(i)),
                Some(LPARAM(&col as *const _ as isize)),
            );
        }
        let queue = queue::Queue::create(frame, instance, ui_font, dpi)?;
        crate::remote::install(
            crate::remote::RemoteState::new(ssh, config.remote.clone()),
            crate::remote::Host {
                frame,
                status: set_status,
            },
        );
        // エージェントを使うか（設定。メニューで切り替えられる）
        crate::remote::set_use_agent(config.transfer.use_agent);
        DragAcceptFiles(frame, true);
        let protocol = if config.transfer.protocol.eq_ignore_ascii_case("scp") {
            Protocol::Scp
        } else {
            Protocol::Sftp
        };
        let app = App {
            frame,
            tree,
            list,
            address,
            status,
            buttons,
            go,
            show_hidden: config.transfer.show_hidden,
            config,
            protocol,
            browsers: Vec::new(),
            loc: None,
            items: Vec::new(),
            back: Vec::new(),
            forward: Vec::new(),
            sort: (0, true),
            icons: HashMap::new(),
            folder_icon,
            nodes: Vec::new(),
            generation: 0,
            queue,
        };
        APP.with(|a| *a.borrow_mut() = Some(app));
        with(|a| {
            a.fill_tree();
            a.update_menu();
            a.update_buttons();
            a.set_status("");
        });
        layout();
        SetTimer(Some(frame), 1, 1000, None);
        let _ = ShowWindow(frame, SW_SHOWDEFAULT);
        if let Some(e) = config_error {
            error_box(
                frame,
                &format!("設定ファイルを読めませんでした（既定値で起動します）。\n{e}"),
            );
        }
        Ok(frame)
    }
}

fn create_menu() -> Result<HMENU> {
    unsafe {
        let item = |m: HMENU, id: u16, text: PCWSTR| AppendMenuW(m, MF_STRING, id as usize, text);
        let sep = |m: HMENU| AppendMenuW(m, MF_SEPARATOR, 0, None);
        let bar = CreateMenu()?;
        let file = CreatePopupMenu()?;
        item(file, ID_CONNECT, w!("接続(&C)...\tCtrl+N"))?;
        item(file, ID_DISCONNECT, w!("切断(&D)"))?;
        sep(file)?;
        item(file, ID_TERMINAL, w!("ターミナルで開く(&T)"))?;
        item(file, ID_EDITOR, w!("エディタで開く(&E)"))?;
        sep(file)?;
        item(file, ID_EXIT, w!("終了(&X)"))?;
        let edit = CreatePopupMenu()?;
        item(edit, ID_SELECT_ALL, w!("すべて選択(&A)\tCtrl+A"))?;
        item(edit, ID_RENAME, w!("名前の変更(&M)\tF2"))?;
        item(edit, ID_DELETE, w!("削除(&D)\tDel"))?;
        item(edit, ID_NEW_FOLDER, w!("新しいフォルダ(&N)\tCtrl+Shift+N"))?;
        sep(edit)?;
        item(edit, ID_COPY_PATH, w!("パスをコピー(&C)"))?;
        let view = CreatePopupMenu()?;
        item(view, ID_REFRESH, w!("最新の情報に更新(&R)\tF5"))?;
        item(view, ID_HIDDEN, w!("隠しファイルを表示(&H)"))?;
        sep(view)?;
        item(view, ID_BACK, w!("戻る(&B)\tAlt+←"))?;
        item(view, ID_FORWARD, w!("進む(&F)\tAlt+→"))?;
        item(view, ID_UP, w!("上のフォルダ(&U)\tAlt+↑"))?;
        item(view, ID_FOCUS_ADDRESS, w!("アドレスバー(&A)\tCtrl+L"))?;
        sep(view)?;
        item(view, ID_SHOW_JOBS, w!("転送の一覧(&J)"))?;
        item(view, ID_SHOW_LOG, w!("転送の記録(&L)"))?;
        let xfer = CreatePopupMenu()?;
        item(xfer, ID_UPLOAD, w!("ファイルをアップロード(&U)...\tCtrl+U"))?;
        item(xfer, ID_UPLOAD_FOLDER, w!("フォルダをアップロード(&F)..."))?;
        item(xfer, ID_DOWNLOAD, w!("ダウンロード(&D)\tCtrl+D"))?;
        item(
            xfer,
            ID_DOWNLOAD_TO,
            w!("保存先を選んでダウンロード(&S)..."),
        )?;
        sep(xfer)?;
        item(xfer, ID_PROTO_SFTP, w!("SFTP で転送する"))?;
        item(xfer, ID_PROTO_SCP, w!("SCP で転送する"))?;
        sep(xfer)?;
        item(
            xfer,
            ID_USE_AGENT,
            w!("接続先にエージェントを置いて使う(&G)（一覧と SHA-256 の照合）"),
        )?;
        sep(xfer)?;
        item(xfer, ID_PAUSE, w!("一時停止(&P)"))?;
        item(xfer, ID_RESUME, w!("再開(&R)"))?;
        item(xfer, ID_RESUME_ALL, w!("すべて再開(&A)"))?;
        item(xfer, ID_CANCEL_JOB, w!("取り消し(&C)"))?;
        item(xfer, ID_CLEAR_DONE, w!("完了したものを一覧から消す(&L)"))?;
        let help = CreatePopupMenu()?;
        item(help, ID_TRANSFER_LOG, w!("転送の記録を開く(&T)"))?;
        item(help, ID_CONNECT_LOG, w!("リモート接続の記録を開く(&R)"))?;
        item(
            help,
            ID_FORGET,
            w!("保存したリモート接続のパスワードを削除(&P)..."),
        )?;
        item(help, ID_SETTINGS, w!("設定ファイルを開く(&S)"))?;
        sep(help)?;
        item(help, ID_ABOUT, w!("バージョン情報(&A)"))?;
        AppendMenuW(bar, MF_POPUP, file.0 as usize, w!("ファイル(&F)"))?;
        AppendMenuW(bar, MF_POPUP, edit.0 as usize, w!("編集(&E)"))?;
        AppendMenuW(bar, MF_POPUP, view.0 as usize, w!("表示(&V)"))?;
        AppendMenuW(bar, MF_POPUP, xfer.0 as usize, w!("転送(&T)"))?;
        AppendMenuW(bar, MF_POPUP, help.0 as usize, w!("ヘルプ(&H)"))?;
        Ok(bar)
    }
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(i32::from(vk.0)) < 0 }
}

/// どこにフォーカスがあっても使うショートカット。処理したら `true`。
fn shortcut(msg: &MSG) -> bool {
    let vk = VIRTUAL_KEY(msg.wParam.0 as u16);
    let (ctrl, shift) = (key_down(VK_CONTROL), key_down(VK_SHIFT));
    let Some((frame, list, address)) = with(|a| (a.frame, a.list, a.address)) else {
        return false;
    };
    // アドレスバーで Enter
    if msg.hwnd == address && vk == VK_RETURN {
        post_command(frame, ID_GO);
        return true;
    }
    let in_list = msg.hwnd == list;
    if msg.message == WM_SYSKEYDOWN {
        // Alt+←・→・↑（戻る・進む・上へ）
        let cmd = match vk {
            VK_LEFT => ID_BACK,
            VK_RIGHT => ID_FORWARD,
            VK_UP => ID_UP,
            _ => return false,
        };
        post_command(frame, cmd);
        return true;
    }
    let cmd = match (ctrl, shift, vk) {
        (false, false, VK_F5) => ID_REFRESH,
        (true, false, VK_N) => ID_CONNECT,
        (true, false, VK_U) => ID_UPLOAD,
        (true, false, VK_D) => ID_DOWNLOAD,
        (true, false, VK_L) => ID_FOCUS_ADDRESS,
        (true, true, VK_N) => ID_NEW_FOLDER,
        (true, false, VK_A) if in_list => ID_SELECT_ALL,
        (false, false, VK_F2) if in_list => ID_RENAME,
        (false, false, VK_DELETE) if in_list => ID_DELETE,
        (false, false, VK_BACK) if in_list => ID_UP,
        (false, false, VK_RETURN) if in_list => {
            open_selected();
            return true;
        }
        _ => return false,
    };
    post_command(frame, cmd);
    true
}

fn post_command(frame: HWND, id: u16) {
    unsafe {
        let _ = PostMessageW(Some(frame), WM_COMMAND, WPARAM(id as usize), LPARAM(0));
    }
}

/// 子ウィンドウを並べる（状態を借りずに動かす）。
fn layout() {
    let Some(rects) = with(|a| a.layout_rects()) else {
        return;
    };
    unsafe {
        for (h, r) in rects {
            let _ = MoveWindow(h, r.left, r.top, r.right - r.left, r.bottom - r.top, true);
        }
    }
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT {
        left: x,
        top: y,
        right: x + w.max(0),
        bottom: y + h.max(0),
    }
}

impl App {
    fn dpi(&self) -> i32 {
        unsafe { GetDpiForWindow(self.frame).max(96) as i32 }
    }

    fn layout_rects(&self) -> Vec<(HWND, RECT)> {
        let mut rc = RECT::default();
        let mut sr = RECT::default();
        unsafe {
            let _ = GetClientRect(self.frame, &mut rc);
            SendMessageW(self.status, WM_SIZE, None, None);
            let _ = GetWindowRect(self.status, &mut sr);
        }
        let s = |v: i32| v * self.dpi() / 96;
        let height = rc.bottom - (sr.bottom - sr.top);
        let pad = s(4);
        let bar_h = s(28);
        let mut out = Vec::new();
        // ツールバーの行: ボタン、アドレスバー、移動
        let mut x = pad;
        for (i, b) in self.buttons.iter().enumerate() {
            let w = if i < 4 { s(64) } else { s(92) };
            out.push((*b, rect(x, pad, w, bar_h)));
            x += w + s(2);
        }
        let go_w = s(52);
        out.push((
            self.address,
            rect(
                x + pad,
                pad + s(2),
                rc.right - x - go_w - pad * 3,
                bar_h - s(4),
            ),
        ));
        out.push((self.go, rect(rc.right - go_w - pad, pad, go_w, bar_h)));
        let top = pad * 2 + bar_h;
        // 下に転送の一覧と記録（3 割）
        let bottom_h = ((height - top) * 3 / 10).max(s(120));
        let main_h = height - top - bottom_h - pad;
        let tree_w = s(240).min(rc.right / 3);
        out.push((self.tree, rect(pad, top, tree_w, main_h)));
        out.push((
            self.list,
            rect(pad * 2 + tree_w, top, rc.right - tree_w - pad * 3, main_h),
        ));
        out.extend(self.queue.layout(
            rect(pad, top + main_h + pad, rc.right - pad * 2, bottom_h - pad),
            self.dpi(),
        ));
        out
    }

    fn set_status(&mut self, text: &str) {
        let text = if text.is_empty() {
            match &self.loc {
                Some(l) => {
                    let selected = self.selected();
                    let size: u64 = selected
                        .iter()
                        .filter_map(|&i| self.items.get(i))
                        .filter(|it| !it.is_dir())
                        .filter_map(|it| it.attrs.size)
                        .sum();
                    let sel = if selected.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "　{} 個の項目を選択（{}）",
                            selected.len(),
                            yy_remote::xfer::human(size)
                        )
                    };
                    format!(
                        "{}　{} 個の項目{sel}　転送: {}　エージェント: {}",
                        l.target,
                        self.items.len(),
                        self.protocol.name(),
                        if crate::remote::use_agent() {
                            "使う"
                        } else {
                            "使わない（SFTP）"
                        }
                    )
                }
                None => {
                    "接続先を選ぶか、アドレスバーに ユーザー@ホスト:/パス を入力してください".into()
                }
            }
        } else {
            text.to_owned()
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

    fn update_menu(&self) {
        unsafe {
            let menu = GetMenu(self.frame);
            let check = |id: u16, on: bool| {
                CheckMenuItem(
                    menu,
                    u32::from(id),
                    (MF_BYCOMMAND | if on { MF_CHECKED } else { MF_UNCHECKED }).0,
                );
            };
            check(ID_PROTO_SFTP, self.protocol == Protocol::Sftp);
            check(ID_PROTO_SCP, self.protocol == Protocol::Scp);
            check(ID_HIDDEN, self.show_hidden);
            check(ID_USE_AGENT, crate::remote::use_agent());
        }
    }

    fn update_buttons(&self) {
        let enable = |id: u16, on: bool| unsafe {
            if let Some(i) = BUTTONS.iter().position(|(b, _)| *b == id) {
                let _ = EnableWindow(self.buttons[i], on);
            }
        };
        let has = self.loc.is_some();
        enable(ID_BACK, !self.back.is_empty());
        enable(ID_FORWARD, !self.forward.is_empty());
        enable(ID_UP, self.loc.as_ref().and_then(Loc::parent).is_some());
        for id in [ID_REFRESH, ID_UPLOAD, ID_NEW_FOLDER] {
            enable(id, has);
        }
        let sel = !self.selected().is_empty();
        enable(ID_DOWNLOAD, sel);
        enable(ID_DELETE, sel);
    }

    fn update_title(&self) {
        let title = match &self.loc {
            Some(l) => format!("{} - yysftp", l.address()),
            None => "yysftp".into(),
        };
        unsafe {
            let _ = SetWindowTextW(self.frame, &HSTRING::from(title));
        }
    }

    // ---- 一覧 ------------------------------------------------------------

    fn browser(&self, target: &Target) -> Option<&Browser> {
        self.browsers
            .iter()
            .find(|b| b.target.same(target) && !b.fs.is_closed() && !b.transport.is_closed())
    }

    /// 選んでいる項目（一覧の並び順）。
    fn selected(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut i = -1isize;
        loop {
            i = unsafe {
                SendMessageW(
                    self.list,
                    LVM_GETNEXTITEM,
                    Some(WPARAM(i as usize)),
                    Some(LPARAM(LVNI_SELECTED as isize)),
                )
                .0
            };
            if i < 0 {
                break;
            }
            out.push(i as usize);
        }
        out
    }

    /// 拡張子のアイコンと種類の名前（エクスプローラーと同じもの）。
    fn icon_for(&mut self, name: &str, dir: bool) -> (i32, String) {
        if dir {
            return (self.folder_icon, "ファイル フォルダー".into());
        }
        let ext = Path::new(name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if let Some(v) = self.icons.get(&ext) {
            return v.clone();
        }
        let mut info = SHFILEINFOW::default();
        let probe = crate::util::wide(&format!("x.{ext}"));
        unsafe {
            SHGetFileInfoW(
                PCWSTR(probe.as_ptr()),
                FILE_ATTRIBUTE_NORMAL,
                Some(&mut info),
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES | SHGFI_TYPENAME,
            );
        }
        let n = info.szTypeName.iter().position(|&c| c == 0).unwrap_or(0);
        let mut kind = String::from_utf16_lossy(&info.szTypeName[..n]);
        if kind.is_empty() {
            kind = if ext.is_empty() {
                "ファイル".into()
            } else {
                format!("{} ファイル", ext.to_uppercase())
            };
        }
        let v = (info.iIcon, kind);
        self.icons.insert(ext, v.clone());
        v
    }

    fn sort_items(&mut self) {
        let (col, asc) = self.sort;
        self.items.sort_by(|a, b| {
            // フォルダが先
            b.is_dir().cmp(&a.is_dir()).then_with(|| {
                let o = match col {
                    1 => a.attrs.mtime.cmp(&b.attrs.mtime),
                    3 => a.attrs.size.cmp(&b.attrs.size),
                    _ => yy_config::workspace::natural_cmp(
                        &String::from_utf8_lossy(&a.name),
                        &String::from_utf8_lossy(&b.name),
                    ),
                };
                if asc { o } else { o.reverse() }
            })
        });
    }

    /// 一覧を作り直す。
    fn fill_list(&mut self) {
        self.sort_items();
        unsafe {
            SendMessageW(self.list, WM_SETREDRAW, Some(WPARAM(0)), None);
            SendMessageW(self.list, LVM_DELETEALLITEMS, None, None);
        }
        let items = self.items.clone();
        for (i, it) in items.iter().enumerate() {
            let name = yy_remote::display(&it.name);
            let (icon, kind) = self.icon_for(&name, it.is_dir());
            let cols = [
                name,
                it.attrs
                    .mtime
                    .map(|t| local_time(u64::from(t)))
                    .unwrap_or_default(),
                if it.attrs.is_symlink() {
                    "シンボリック リンク".into()
                } else {
                    kind
                },
                if it.is_dir() {
                    String::new()
                } else {
                    it.attrs.size.map(size_kb).unwrap_or_default()
                },
                it.attrs.mode_string(),
            ];
            let text = crate::util::wide(&cols[0]);
            let lv = LVITEMW {
                mask: LVIF_TEXT | LVIF_IMAGE | LVIF_PARAM,
                iItem: i as i32,
                pszText: PWSTR(text.as_ptr() as *mut _),
                iImage: icon,
                lParam: LPARAM(i as isize),
                ..Default::default()
            };
            unsafe {
                SendMessageW(
                    self.list,
                    LVM_INSERTITEMW,
                    None,
                    Some(LPARAM(&lv as *const _ as isize)),
                );
            }
            for (c, t) in cols.iter().enumerate().skip(1) {
                set_cell(self.list, i, c, t);
            }
        }
        unsafe {
            SendMessageW(self.list, WM_SETREDRAW, Some(WPARAM(1)), None);
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(self.list), None, true);
        }
        let addr = self.loc.as_ref().map(Loc::address).unwrap_or_default();
        unsafe {
            let _ = SetWindowTextW(self.address, &HSTRING::from(addr));
        }
        self.update_title();
        self.update_buttons();
        self.set_status("");
    }

    // ---- ツリー ----------------------------------------------------------

    /// ツリーの根（接続先）を並べる: 設定の接続先、接続中・最近の接続先。
    fn fill_tree(&mut self) {
        unsafe {
            SendMessageW(self.tree, TVM_DELETEITEM, None, Some(LPARAM(TVI_ROOT.0)));
        }
        self.nodes.clear();
        self.generation += 1;
        let mut targets: Vec<String> =
            crate::remote::with_state(|r| r.known_targets()).unwrap_or_default();
        for name in self.config.remote.host.keys() {
            if !targets.iter().any(|t| t.eq_ignore_ascii_case(name)) {
                targets.push(name.clone());
            }
        }
        for b in &self.browsers {
            let s = b.target.to_string();
            if !targets.iter().any(|t| t.eq_ignore_ascii_case(&s)) {
                targets.push(s);
            }
        }
        for t in targets {
            if let Some(target) = Target::parse(&t) {
                self.add_node(TVI_ROOT, target, None, &t);
            }
        }
    }

    fn add_node(
        &mut self,
        parent: HTREEITEM,
        target: Target,
        path: Option<Vec<u8>>,
        label: &str,
    ) -> HTREEITEM {
        let index = self.nodes.len();
        let mut text: Vec<u16> = label.encode_utf16().chain([0]).collect();
        let ins = TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 {
                itemex: TVITEMEXW {
                    mask: TVIF_TEXT | TVIF_PARAM | TVIF_CHILDREN | TVIF_IMAGE | TVIF_SELECTEDIMAGE,
                    pszText: PWSTR(text.as_mut_ptr()),
                    cChildren: TVITEMEXW_CHILDREN(1),
                    lParam: LPARAM(index as isize),
                    iImage: self.folder_icon,
                    iSelectedImage: self.folder_icon,
                    ..Default::default()
                },
            },
        };
        let item = unsafe {
            HTREEITEM(
                SendMessageW(
                    self.tree,
                    TVM_INSERTITEMW,
                    None,
                    Some(LPARAM(&ins as *const _ as isize)),
                )
                .0,
            )
        };
        self.nodes.push(Node {
            target,
            path,
            item,
            loaded: false,
        });
        item
    }

    fn node_of(&self, item: HTREEITEM) -> Option<usize> {
        let mut tv = TVITEMEXW {
            mask: TVIF_PARAM | TVIF_HANDLE,
            hItem: item,
            ..Default::default()
        };
        let ok = unsafe {
            SendMessageW(
                self.tree,
                TVM_GETITEMW,
                None,
                Some(LPARAM(&mut tv as *mut _ as isize)),
            )
            .0
        };
        (ok != 0 && (tv.lParam.0 as usize) < self.nodes.len()).then_some(tv.lParam.0 as usize)
    }

    fn mark_no_children(&self, item: HTREEITEM) {
        let tv = TVITEMEXW {
            mask: TVIF_CHILDREN | TVIF_HANDLE,
            hItem: item,
            cChildren: TVITEMEXW_CHILDREN(0),
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                self.tree,
                TVM_SETITEMW,
                None,
                Some(LPARAM(&tv as *const _ as isize)),
            );
        }
    }
}

/// 一覧のセル（列 `col`）の文字を設定する。
fn set_cell(list: HWND, row: usize, col: usize, text: &str) {
    let t = crate::util::wide(text);
    let lv = LVITEMW {
        iSubItem: col as i32,
        pszText: PWSTR(t.as_ptr() as *mut _),
        ..Default::default()
    };
    unsafe {
        SendMessageW(
            list,
            LVM_SETITEMTEXTW,
            Some(WPARAM(row)),
            Some(LPARAM(&lv as *const _ as isize)),
        );
    }
}

/// エクスプローラーと同じ「1,234 KB」の表示。
fn size_kb(n: u64) -> String {
    format!("{} KB", crate::util::group_digits(n.div_ceil(1024)))
}

/// UNIX 時刻（秒）を手元の日時の表示にする。
fn local_time(secs: u64) -> String {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let t = (secs + 11_644_473_600) * 10_000_000;
    let ft = FILETIME {
        dwLowDateTime: t as u32,
        dwHighDateTime: (t >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
        {
            return String::new();
        }
    }
    format!(
        "{:04}/{:02}/{:02} {:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute
    )
}

// ---- 接続と移動 -------------------------------------------------------------------

/// `target` の一覧・ファイル操作（エージェントを使う設定ならエージェント、でなければ SFTP。
/// なければ接続する。接続中はアプリの状態を借りない）。
fn browser(target: &Target) -> std::result::Result<(Arc<dyn RemoteFs>, Vec<u8>), String> {
    if let Some(b) = with(|a| a.browser(target).map(|b| (b.fs.clone(), b.home.clone()))).flatten() {
        return Ok(b);
    }
    let fs = crate::remote::fs(target, &set_status)?;
    // 転送に使う接続（エージェントのセッションの接続か、同じ SSH の接続）
    let t = crate::remote::transport(target, &set_status)?;
    let home = fs.home().to_vec();
    with(|a| {
        a.browsers.retain(|b| !b.target.same(target));
        a.browsers.push(Browser {
            target: target.clone(),
            fs: fs.clone(),
            transport: t,
            home: home.clone(),
        });
        a.set_status("");
    });
    Ok((fs, home))
}

/// 接続先でファイル操作をする（待つ間は進みを表示する。接続が切れていたら 1 回だけ接続し直す）。
fn remote_op<T: Send + 'static>(
    target: &Target,
    label: &str,
    f: impl Fn(&dyn RemoteFs) -> std::io::Result<T> + Send + Sync + 'static,
) -> std::result::Result<T, String> {
    let f = Arc::new(f);
    for attempt in 0..2 {
        let (fs, _) = browser(target)?;
        set_status(label);
        let g = f.clone();
        let s = fs.clone();
        let r = crate::remote::wait(&set_status, move |_| g(s.as_ref()));
        set_status("");
        match r {
            Ok(v) => return Ok(v),
            Err(e) if attempt == 0 && (fs.is_closed() || yy_remote::xfer::retryable(&e, None)) => {
                // 切れた接続を捨てて、もう一度
                with(|a| a.browsers.retain(|b| !b.target.same(target)));
                crate::remote::with_state(|_| ());
                continue;
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("接続できませんでした".into())
}

/// `loc` を一覧に表示する（パスが空ならホーム）。`push` なら戻る履歴に入れる。
fn go(mut loc: Loc, push: bool) {
    let frame = with(|a| a.frame).unwrap_or_default();
    let home = match browser(&loc.target) {
        Ok((_, h)) => h,
        Err(e) => {
            error_box(frame, &e);
            return;
        }
    };
    if loc.path.is_empty() {
        loc.path = home;
    }
    let path = loc.path.clone();
    let listed = remote_op(
        &loc.target,
        "フォルダを読んでいます…（Esc で中止）",
        move |s| {
            let real = s.real_path(&path)?;
            let entries = s.entries(&real)?;
            Ok((real, entries))
        },
    );
    let (real, entries) = match listed {
        Ok(v) => v,
        Err(e) => {
            error_box(frame, &format!("{} を開けません。\n{e}", loc.address()));
            return;
        }
    };
    loc.path = real;
    crate::remote::set_last(loc.uri());
    with(|a| {
        if push
            && let Some(cur) = a.loc.take()
            && cur != loc
        {
            a.back.push(cur);
            a.forward.clear();
        }
        a.items = entries
            .into_iter()
            .filter(|e| a.show_hidden || !e.name.starts_with(b"."))
            .map(|e| Item {
                name: e.name,
                attrs: e.attrs,
            })
            .collect();
        let known = a
            .nodes
            .iter()
            .any(|n| n.path.is_none() && n.target.same(&loc.target));
        a.loc = Some(loc.clone());
        if !known {
            let label = loc.target.to_string();
            a.add_node(TVI_ROOT, loc.target.clone(), None, &label);
        }
        a.fill_list();
    });
}

fn refresh() {
    if let Some(loc) = with(|a| a.loc.clone()).flatten() {
        go(loc, false);
    }
}

/// 選んだ項目を開く（フォルダは中へ、ファイルはダウンロード）。
fn open_selected() {
    let Some((loc, items)) = with(|a| {
        let loc = a.loc.clone()?;
        let items: Vec<Item> = a
            .selected()
            .iter()
            .filter_map(|&i| a.items.get(i).cloned())
            .collect();
        Some((loc, items))
    })
    .flatten() else {
        return;
    };
    match items.as_slice() {
        [one] if one.is_dir() || one.attrs.is_symlink() => go(loc.child(&one.name), true),
        [] => {}
        _ => queue::download_selected(None),
    }
}

extern "system" fn frame_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            layout();
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = loword(wparam.0) as u16;
            // ボタン・メニュー（通知の BN_CLICKED は 0）
            if hiword(wparam.0) == 0 || lparam.0 != 0 {
                command(hwnd, id);
            }
            LRESULT(0)
        }
        WM_NOTIFY => {
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            match hdr.idFrom as u16 {
                ID_LIST => on_list_notify(hwnd, hdr, lparam),
                ID_TREE => on_tree_notify(hwnd, hdr, lparam),
                ID_BOTTOM_TABS | ID_JOBS => queue::on_notify(hwnd, hdr, lparam),
                _ => crate::default_proc(hwnd, msg, wparam, lparam),
            }
        }
        WM_DROPFILES => {
            let hdrop = HDROP(wparam.0 as *mut _);
            let mut files = Vec::new();
            unsafe {
                let count = DragQueryFileW(hdrop, u32::MAX, None);
                for i in 0..count {
                    let len = DragQueryFileW(hdrop, i, None) as usize;
                    let mut buf = vec![0u16; len + 1];
                    DragQueryFileW(hdrop, i, Some(&mut buf));
                    files.push(PathBuf::from(String::from_utf16_lossy(&buf[..len])));
                }
                DragFinish(hdrop);
            }
            queue::upload(hwnd, files);
            LRESULT(0)
        }
        // 知らせの取りこぼし（状態を借りている間に届いた）は、タイマーで拾う
        WM_APP_XFER_EVENT | WM_TIMER => {
            with(|a| a.queue.on_events());
            LRESULT(0)
        }
        WM_APP_RENAME => {
            on_rename(hwnd, wparam, lparam);
            LRESULT(0)
        }
        WM_APP_XFER_REFRESH => {
            if with(|a| a.queue.take_refresh(a.loc.as_ref())) == Some(true) {
                refresh();
            }
            LRESULT(0)
        }
        WM_APP_TREE_LOAD => {
            load_tree(hwnd, wparam.0, lparam.0 as usize);
            LRESULT(0)
        }
        WM_APP_TREE_GO => {
            if let Some(loc) = with(|a| {
                let n = a.nodes.get(lparam.0 as usize)?;
                Some(Loc {
                    target: n.target.clone(),
                    path: n.path.clone().unwrap_or_default(),
                })
            })
            .flatten()
            {
                go(loc, true);
            }
            LRESULT(0)
        }
        m if m == crate::remote::WM_APP_REMOTE_PROMPT => {
            crate::remote::on_prompt(hwnd, lparam);
            LRESULT(0)
        }
        WM_CLOSE => {
            if with(|a| a.queue.is_running()) == Some(true) {
                let r = unsafe {
                    MessageBoxW(
                        Some(hwnd),
                        w!(
                            "転送中です。中断して終了しますか？\n（次に起動したときに、続きから再開できます）"
                        ),
                        w!("yysftp"),
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
        ID_BACK => {
            let Some(loc) = with(|a| {
                let l = a.back.pop()?;
                if let Some(cur) = a.loc.clone() {
                    a.forward.push(cur);
                }
                Some(l)
            })
            .flatten() else {
                return;
            };
            go(loc, false);
        }
        ID_FORWARD => {
            let Some(loc) = with(|a| {
                let l = a.forward.pop()?;
                if let Some(cur) = a.loc.clone() {
                    a.back.push(cur);
                }
                Some(l)
            })
            .flatten() else {
                return;
            };
            go(loc, false);
        }
        ID_UP => {
            if let Some(p) = with(|a| a.loc.as_ref().and_then(Loc::parent)).flatten() {
                go(p, true);
            }
        }
        ID_REFRESH => refresh(),
        ID_GO => {
            let Some((address, cur)) = with(|a| {
                let mut buf = [0u16; 2048];
                let n = unsafe { GetWindowTextW(a.address, &mut buf) } as usize;
                (String::from_utf16_lossy(&buf[..n]), a.loc.clone())
            }) else {
                return;
            };
            let text = address.trim();
            // `/パス` だけなら同じ接続先
            let loc = match (text.starts_with('/'), cur) {
                (true, Some(c)) => Some(Loc {
                    target: c.target,
                    path: text.as_bytes().to_vec(),
                }),
                _ => parse_location(text),
            };
            match loc {
                Some(l) => go(l, true),
                None => error_box(
                    hwnd,
                    &format!("場所（{text}）を読めません。\n例: yamada@build01:/home/yamada"),
                ),
            }
        }
        ID_FOCUS_ADDRESS => {
            if let Some(h) = with(|a| a.address) {
                unsafe {
                    let _ = SetFocus(Some(h));
                    SendMessageW(h, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                }
            }
        }
        ID_CONNECT => cmd_connect(hwnd),
        ID_DISCONNECT => {
            with(|a| {
                if let Some(l) = a.loc.take() {
                    a.browsers.retain(|b| !b.target.same(&l.target));
                }
                a.items.clear();
                a.back.clear();
                a.forward.clear();
                a.fill_list();
            });
        }
        ID_EXIT => unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        ID_SELECT_ALL => {
            if let Some(list) = with(|a| a.list) {
                let lv = LVITEMW {
                    stateMask: LVIS_SELECTED,
                    state: LVIS_SELECTED,
                    ..Default::default()
                };
                unsafe {
                    SendMessageW(
                        list,
                        LVM_SETITEMSTATE,
                        Some(WPARAM(usize::MAX)),
                        Some(LPARAM(&lv as *const _ as isize)),
                    );
                }
            }
        }
        ID_RENAME => {
            if let Some((list, sel)) = with(|a| (a.list, a.selected())) {
                if let [i] = sel.as_slice() {
                    unsafe {
                        let _ = SetFocus(Some(list));
                        SendMessageW(list, LVM_EDITLABELW, Some(WPARAM(*i)), None);
                    }
                }
            }
        }
        ID_DELETE => cmd_delete(hwnd),
        ID_NEW_FOLDER => cmd_new_folder(hwnd),
        ID_COPY_PATH => {
            let text = with(|a| {
                let loc = a.loc.clone()?;
                let sel = a.selected();
                let paths: Vec<String> = if sel.is_empty() {
                    vec![yy_remote::display(&loc.path)]
                } else {
                    sel.iter()
                        .filter_map(|&i| a.items.get(i))
                        .map(|it| yy_remote::display(&yy_remote::join_remote(&loc.path, &it.name)))
                        .collect()
                };
                Some(paths.join("\r\n"))
            })
            .flatten();
            if let Some(t) = text {
                let _ = crate::clipboard::set_text(hwnd, &t, false);
            }
        }
        ID_HIDDEN => {
            with(|a| {
                a.show_hidden = !a.show_hidden;
                a.update_menu();
            });
            refresh();
        }
        ID_USE_AGENT => {
            let on = !crate::remote::use_agent();
            crate::remote::set_use_agent(on);
            with(|a| {
                // 一覧の接続を作り直す（エージェントか SFTP か）
                a.browsers.clear();
                a.update_menu();
            });
            if with(|a| a.loc.is_some()) == Some(true) {
                refresh();
            } else {
                set_status("");
            }
        }
        ID_PROTO_SFTP | ID_PROTO_SCP => {
            with(|a| {
                a.protocol = if id == ID_PROTO_SCP {
                    Protocol::Scp
                } else {
                    Protocol::Sftp
                };
                a.update_menu();
                a.set_status("");
            });
        }
        ID_UPLOAD => {
            let files = pick_files(hwnd);
            if !files.is_empty() {
                queue::upload(hwnd, files);
            }
        }
        ID_UPLOAD_FOLDER => {
            if let Some(d) = crate::grepdlg::browse_folder(hwnd) {
                queue::upload(hwnd, vec![d]);
            }
        }
        ID_DOWNLOAD => queue::download_selected(None),
        ID_DOWNLOAD_TO => {
            if let Some(d) = crate::grepdlg::browse_folder(hwnd) {
                queue::download_selected(Some(d));
            }
        }
        ID_PAUSE | ID_RESUME | ID_RESUME_ALL | ID_CANCEL_JOB | ID_CLEAR_DONE | ID_SHOW_JOBS
        | ID_SHOW_LOG | ID_OPEN_LOCAL => queue::command(hwnd, id),
        ID_TERMINAL | ID_EDITOR => open_in_app(hwnd, id == ID_TERMINAL),
        ID_TRANSFER_LOG => match queue::log_path() {
            Some(p) if p.exists() => open_in_app_path(hwnd, "yyeditor.exe", &p.to_string_lossy()),
            _ => info_box(hwnd, "転送の記録はまだありません。"),
        },
        ID_CONNECT_LOG => match crate::remote::log_path() {
            Some(p) if p.exists() => open_in_app_path(hwnd, "yyeditor.exe", &p.to_string_lossy()),
            _ => info_box(hwnd, "リモート接続の記録はまだありません。"),
        },
        ID_FORGET => crate::remote::forget_passwords(hwnd),
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
            open_in_app_path(hwnd, "yyeditor.exe", &path.to_string_lossy());
        }
        ID_ABOUT => info_box(
            hwnd,
            &format!(
                "yysftp {}\n\nyyeditor・yyterm と同じ部品で作ったファイル転送（SFTP・SCP）。\n\
                 切断されても再接続して続きから送れます（レジューム）。\n\
                 転送の記録: {}",
                env!("CARGO_PKG_VERSION"),
                queue::log_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ),
        ),
        _ => {}
    }
}

/// 「接続」: 接続先を尋ねてホームを開く。
fn cmd_connect(hwnd: HWND) {
    if !crate::remote::available() {
        info_box(hwnd, "この yysftp には SSH の機能が組み込まれていません。");
        return;
    }
    let initial = crate::remote::last()
        .map(|u| u.target().to_string())
        .unwrap_or_default();
    let Some(text) = crate::goto::prompt_text(
        hwnd,
        "接続",
        "接続先（ユーザー@ホスト:ポート、~/.ssh/config の Host の名前。:/パス も付けられます）:",
        &initial,
    ) else {
        return;
    };
    match parse_location(&text) {
        Some(l) => go(l, true),
        None => error_box(hwnd, &format!("接続先（{text}）を読めません。")),
    }
}

fn cmd_delete(hwnd: HWND) {
    let Some((loc, items)) = with(|a| {
        let loc = a.loc.clone()?;
        let items: Vec<Item> = a
            .selected()
            .iter()
            .filter_map(|&i| a.items.get(i).cloned())
            .collect();
        Some((loc, items))
    })
    .flatten() else {
        return;
    };
    if items.is_empty() {
        return;
    }
    let names: Vec<String> = items
        .iter()
        .take(10)
        .map(|i| yy_remote::display(&i.name))
        .collect();
    let more = if items.len() > 10 {
        format!("\n…ほか {} 件", items.len() - 10)
    } else {
        String::new()
    };
    let text = format!(
        "{} の次の {} 個の項目を削除しますか？（フォルダは中身ごと。元に戻せません）\n\n{}{more}",
        loc.address(),
        items.len(),
        names.join("\n")
    );
    let r = unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(text),
            w!("yysftp"),
            MB_OKCANCEL | MB_ICONWARNING,
        )
    };
    if r != IDOK {
        return;
    }
    let paths: Vec<Vec<u8>> = items
        .iter()
        .map(|i| yy_remote::join_remote(&loc.path, &i.name))
        .collect();
    let r = remote_op(
        &loc.target,
        "削除しています…（Esc で中止）",
        move |s| {
            for p in &paths {
                s.remove(p, true)?;
            }
            Ok(())
        },
    );
    if let Err(e) = r {
        error_box(hwnd, &format!("削除できませんでした。\n{e}"));
    }
    refresh();
}

fn cmd_new_folder(hwnd: HWND) {
    let Some(loc) = with(|a| a.loc.clone()).flatten() else {
        return;
    };
    let Some(name) =
        crate::goto::prompt_text(hwnd, "新しいフォルダ", "フォルダの名前:", "新しいフォルダ")
    else {
        return;
    };
    let name = name.trim().to_owned();
    if let Err(e) = yy_config::workspace::check_name(&name, true) {
        error_box(hwnd, &e);
        return;
    }
    let path = yy_remote::join_remote(&loc.path, name.as_bytes());
    if let Err(e) = remote_op(
        &loc.target,
        "フォルダを作っています…",
        move |s| s.make_dir(&path),
    ) {
        error_box(hwnd, &format!("フォルダを作れませんでした。\n{e}"));
    }
    refresh();
}

/// 名前の変更（一覧の項目の名前の編集が終わった）。
fn rename_item(hwnd: HWND, index: usize, new_name: &str) {
    let Some((loc, item)) = with(|a| Some((a.loc.clone()?, a.items.get(index)?.clone()))).flatten()
    else {
        return;
    };
    let new_name = new_name.trim();
    if new_name.is_empty() || new_name.as_bytes() == item.name.as_slice() {
        return;
    }
    if let Err(e) = yy_config::workspace::check_name(new_name, true) {
        error_box(hwnd, &e);
        return;
    }
    let from = yy_remote::join_remote(&loc.path, &item.name);
    let to = yy_remote::join_remote(&loc.path, new_name.as_bytes());
    if let Err(e) = remote_op(&loc.target, "名前を変えています…", move |s| {
        if s.try_stat(&to)?.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "同じ名前の項目があります",
            ));
        }
        s.rename(&from, &to)
    }) {
        error_box(hwnd, &format!("名前を変えられませんでした。\n{e}"));
    }
    refresh();
}

/// 選んでいるフォルダをターミナル（yyterm）で、ファイルをエディタ（yyeditor）で開く。
fn open_in_app(hwnd: HWND, terminal: bool) {
    let Some((loc, item)) = with(|a| {
        let loc = a.loc.clone()?;
        let item = a.selected().first().and_then(|&i| a.items.get(i).cloned());
        Some((loc, item))
    })
    .flatten() else {
        return;
    };
    let uri = match &item {
        Some(it) if !terminal || it.is_dir() => loc.child(&it.name).uri(),
        _ => loc.uri(),
    };
    if !terminal && item.as_ref().is_none_or(|i| i.is_dir()) {
        info_box(hwnd, "エディタで開くファイルを選んでください。");
        return;
    }
    let exe = if terminal {
        "yyterm.exe"
    } else {
        "yyeditor.exe"
    };
    open_in_app_path(hwnd, exe, &uri.to_string());
}

/// 同じフォルダのアプリ（yyeditor・yyterm）で開く。
fn open_in_app_path(hwnd: HWND, exe: &str, arg: &str) {
    let path = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(exe)))
        .filter(|p| p.is_file());
    let r = match path {
        Some(p) => std::process::Command::new(p)
            .arg(arg)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string()),
        None => Err(format!("{exe} が yysftp.exe と同じフォルダにありません。")),
    };
    if let Err(e) = r {
        error_box(hwnd, &e);
    }
}

/// 送るファイルを選ぶ（複数選べる）。
fn pick_files(owner: HWND) -> Vec<PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::{
        FOS_ALLOWMULTISELECT, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
    };
    let mut out = Vec::new();
    unsafe {
        let Ok(d) =
            CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
        else {
            return out;
        };
        if let Ok(o) = d.GetOptions() {
            let _ = d.SetOptions(o | FOS_ALLOWMULTISELECT);
        }
        let _ = d.SetTitle(w!("アップロードするファイル"));
        if d.Show(Some(owner)).is_err() {
            return out;
        }
        let Ok(items) = d.GetResults() else {
            return out;
        };
        let n = items.GetCount().unwrap_or(0);
        for i in 0..n {
            if let Ok(item) = items.GetItemAt(i)
                && let Ok(name) = item.GetDisplayName(SIGDN_FILESYSPATH)
            {
                if let Ok(s) = name.to_string() {
                    out.push(PathBuf::from(s));
                }
                CoTaskMemFree(Some(name.0 as *const _));
            }
        }
    }
    out
}

fn on_list_notify(hwnd: HWND, hdr: &NMHDR, lparam: LPARAM) -> LRESULT {
    match hdr.code {
        NM_DBLCLK => {
            open_selected();
            LRESULT(0)
        }
        LVN_ITEMCHANGED => {
            with(|a| {
                a.update_buttons();
                a.set_status("");
            });
            LRESULT(0)
        }
        LVN_COLUMNCLICK => {
            let nm = unsafe { &*(lparam.0 as *const NMLISTVIEW) };
            with(|a| {
                let col = nm.iSubItem;
                a.sort = if a.sort.0 == col {
                    (col, !a.sort.1)
                } else {
                    (col, true)
                };
                a.fill_list();
            });
            LRESULT(0)
        }
        LVN_BEGINLABELEDITW => LRESULT(0),
        LVN_ENDLABELEDITW => {
            let nm = unsafe { &*(lparam.0 as *const NMLVDISPINFOW) };
            if !nm.item.pszText.is_null() {
                let text = unsafe { nm.item.pszText.to_string().unwrap_or_default() };
                let index = nm.item.iItem as usize;
                // 通知の中では接続しない
                let text = Box::new(text);
                unsafe {
                    let _ = PostMessageW(
                        Some(hwnd),
                        WM_APP_RENAME,
                        WPARAM(index),
                        LPARAM(Box::into_raw(text) as isize),
                    );
                }
            }
            LRESULT(0)
        }
        NM_RCLICK => {
            list_menu(hwnd);
            LRESULT(1)
        }
        _ => LRESULT(0),
    }
}

/// 名前の変更（`WPARAM` は項目、`LPARAM` は `Box<String>`）
const WM_APP_RENAME: u32 = WM_APP + 73;

fn list_menu(hwnd: HWND) {
    let Some((has_loc, sel, one_dir)) = with(|a| {
        let sel = a.selected();
        let one_dir = match sel.as_slice() {
            [i] => a.items.get(*i).is_some_and(Item::is_dir),
            _ => false,
        };
        (a.loc.is_some(), sel.len(), one_dir)
    }) else {
        return;
    };
    if !has_loc {
        return;
    }
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
        let Ok(menu) = CreatePopupMenu() else { return };
        let item = |id: u16, text: &str| {
            let t = crate::util::wide(text);
            AppendMenuW(menu, MF_STRING, id as usize, PCWSTR(t.as_ptr()))
        };
        let sep = || AppendMenuW(menu, MF_SEPARATOR, 0, None);
        if sel > 0 {
            if one_dir {
                let _ = item(ID_TERMINAL, "ターミナルで開く(&T)");
            } else if sel == 1 {
                let _ = item(ID_EDITOR, "エディタで開く(&E)");
            }
            let _ = item(ID_DOWNLOAD, "ダウンロード(&D)");
            let _ = item(ID_DOWNLOAD_TO, "保存先を選んでダウンロード(&S)...");
            let _ = sep();
            if sel == 1 {
                let _ = item(ID_RENAME, "名前の変更(&M)");
            }
            let _ = item(ID_DELETE, "削除(&X)");
            let _ = item(ID_COPY_PATH, "パスをコピー(&C)");
            let _ = sep();
        } else {
            let _ = item(ID_TERMINAL, "このフォルダをターミナルで開く(&T)");
            let _ = item(ID_COPY_PATH, "パスをコピー(&C)");
            let _ = sep();
        }
        let _ = item(ID_UPLOAD, "ファイルをアップロード(&U)...");
        let _ = item(ID_UPLOAD_FOLDER, "フォルダをアップロード(&F)...");
        let _ = item(ID_NEW_FOLDER, "新しいフォルダ(&N)...");
        let _ = item(ID_REFRESH, "最新の情報に更新(&R)");
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
        if cmd.0 != 0 {
            command(hwnd, cmd.0 as u16);
        }
    }
}

fn on_tree_notify(hwnd: HWND, hdr: &NMHDR, lparam: LPARAM) -> LRESULT {
    match hdr.code {
        TVN_ITEMEXPANDINGW => {
            let nm = unsafe { &*(lparam.0 as *const NMTREEVIEWW) };
            if nm.action == TVE_EXPAND {
                let pending = with(|a| {
                    let i = a.node_of(nm.itemNew.hItem)?;
                    (!a.nodes[i].loaded).then_some((a.generation, i))
                })
                .flatten();
                if let Some((g, i)) = pending {
                    // 通知の中では接続しない（読んでから開く）
                    unsafe {
                        let _ = PostMessageW(
                            Some(hwnd),
                            WM_APP_TREE_LOAD,
                            WPARAM(g),
                            LPARAM(i as isize),
                        );
                    }
                    return LRESULT(1);
                }
            }
            LRESULT(0)
        }
        TVN_SELCHANGEDW => {
            let nm = unsafe { &*(lparam.0 as *const NMTREEVIEWW) };
            // 操作で選んだときだけ（プログラムからの選択では移動しない）
            if nm.action != TVC_UNKNOWN
                && let Some(i) = with(|a| a.node_of(nm.itemNew.hItem)).flatten()
            {
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_APP_TREE_GO, WPARAM(0), LPARAM(i as isize));
                }
            }
            LRESULT(0)
        }
        _ => LRESULT(0),
    }
}

/// ツリーの項目の中身（フォルダ）を読む。接続先の項目は「ホーム」と「/」を並べる。
fn load_tree(hwnd: HWND, generation: usize, index: usize) {
    let Some((target, path, item)) = with(|a| {
        if a.generation != generation {
            return None;
        }
        let n = a.nodes.get(index)?;
        (!n.loaded).then(|| (n.target.clone(), n.path.clone(), n.item))
    })
    .flatten() else {
        return;
    };
    let children: Vec<(String, Vec<u8>)> = match &path {
        None => match browser(&target) {
            Ok((_, home)) => vec![
                (format!("ホーム（{}）", yy_remote::display(&home)), home),
                ("/（ルート）".into(), b"/".to_vec()),
            ],
            Err(e) => {
                error_box(hwnd, &e);
                return;
            }
        },
        Some(p) => {
            let p = p.clone();
            let show_hidden = with(|a| a.show_hidden).unwrap_or(false);
            match remote_op(&target, "フォルダを読んでいます…", move |s| {
                s.entries(&p)
            }) {
                Ok(entries) => {
                    let mut dirs: Vec<(String, Vec<u8>)> = entries
                        .into_iter()
                        .filter(|e| e.attrs.is_dir() && (show_hidden || !e.name.starts_with(b".")))
                        .map(|e| {
                            let full = yy_remote::join_remote(
                                path.as_deref().unwrap_or_default(),
                                &e.name,
                            );
                            (yy_remote::display(&e.name), full)
                        })
                        .collect();
                    dirs.sort_by(|a, b| yy_config::workspace::natural_cmp(&a.0, &b.0));
                    dirs
                }
                Err(e) => {
                    error_box(hwnd, &format!("フォルダを開けません。\n{e}"));
                    return;
                }
            }
        }
    };
    with(|a| {
        if a.generation != generation || a.nodes.get(index).is_none_or(|n| n.loaded) {
            return;
        }
        a.nodes[index].loaded = true;
        if children.is_empty() {
            a.mark_no_children(item);
            return;
        }
        for (label, p) in children {
            a.add_node(item, target.clone(), Some(p), &label);
        }
    });
    if let Some(tree) = with(|a| a.tree) {
        unsafe {
            SendMessageW(
                tree,
                TVM_EXPAND,
                Some(WPARAM(TVE_EXPAND.0 as usize)),
                Some(LPARAM(item.0)),
            );
        }
    }
}

/// 名前の変更の続き（[`WM_APP_RENAME`]）。
fn on_rename(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
    let text = unsafe { Box::from_raw(lparam.0 as *mut String) };
    rename_item(hwnd, wparam.0, &text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_locations() {
        let l = parse_location("yamada@build01:/home/yamada/src").unwrap();
        assert_eq!(l.target.to_string(), "yamada@build01");
        assert_eq!(l.path, b"/home/yamada/src");
        assert_eq!(l.address(), "yamada@build01:/home/yamada/src");
        let l = parse_location("ssh://yamada@build01:2222/tmp/x").unwrap();
        assert_eq!(l.target.port, Some(2222));
        assert_eq!(l.path, b"/tmp/x");
        // パスがなければホーム
        let l = parse_location("build01").unwrap();
        assert!(l.path.is_empty());
        assert!(parse_location("").is_none());
        let l = parse_location("ssh://build01").unwrap();
        assert_eq!(l.target.host, "build01");
        assert!(l.path.is_empty());
    }

    #[test]
    fn parents() {
        let l = parse_location("h:/a/b").unwrap();
        let p = l.parent().unwrap();
        assert_eq!(p.path, b"/a");
        assert_eq!(p.parent().unwrap().path, b"/");
        assert!(p.parent().unwrap().parent().is_none());
        assert_eq!(p.child(b"c").path, b"/a/c");
    }
}

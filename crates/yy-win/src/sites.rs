//! Shared native connection manager for yyeditor, yyterm and yysftp.
use crate::{
    goto::{CLASS_BUTTON, CLASS_STATIC, Template},
    util::{error_box, info_box},
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::*,
    },
    core::HSTRING,
};
use yy_config::{
    Config, RemoteHost, TerminalMultiplexer, TransferProtocol,
    connections::{self, Profiles},
};

pub(crate) const ID_MANAGE: u16 = 62003;
const LIST: u16 = 100;
const NEW: u16 = 101;
const EDIT: u16 = 102;
const CLONE: u16 = 103;
const DELETE: u16 = 104;
const PAGE: u16 = 105;
const COMBO: u16 = 0x85;
const LISTBOX: u16 = 0x83;
const LABEL_BASE: u16 = 200;
const FIELD_BASE: u16 = 210;

fn control(hwnd: HWND, id: u16) -> HWND {
    unsafe { GetDlgItem(Some(hwnd), id as i32).unwrap_or_default() }
}
fn set(hwnd: HWND, id: u16, text: &str) {
    unsafe {
        let _ = SetWindowTextW(control(hwnd, id), &HSTRING::from(text));
    }
}
fn text(hwnd: HWND, id: u16) -> String {
    unsafe {
        let item = control(hwnd, id);
        let mut buf = vec![0; GetWindowTextLengthW(item).max(0) as usize + 1];
        let n = GetWindowTextW(item, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}
fn add_combo(hwnd: HWND, id: u16, values: &[&str]) {
    unsafe {
        let item = control(hwnd, id);
        SendMessageW(item, CB_RESETCONTENT, None, None);
        for value in values {
            SendMessageW(
                item,
                CB_ADDSTRING,
                None,
                Some(LPARAM(HSTRING::from(*value).as_ptr() as isize)),
            );
        }
    }
}

struct Manager {
    names: Vec<String>,
    changed: bool,
}
pub(crate) fn show(owner: HWND) -> bool {
    let mut template = Template::dialog("SSH接続先の管理（yyシリーズ共通）", 360, 234);
    template.item(
        (WS_TABSTOP | WS_BORDER | WS_VSCROLL).0 | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
        7,
        7,
        272,
        185,
        LIST,
        LISTBOX,
        "",
    );
    for (id, label, y) in [
        (NEW, "新規…", 7),
        (EDIT, "編集…", 29),
        (CLONE, "複製…", 51),
        (DELETE, "削除", 73),
    ] {
        template.item(
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            285,
            y,
            68,
            17,
            id,
            CLASS_BUTTON,
            label,
        );
    }
    template.item(
        0,
        7,
        196,
        346,
        14,
        0,
        CLASS_STATIC,
        "保存後の新しい接続から反映します。接続中の端末は維持します。",
    );
    template.item(
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        285,
        213,
        68,
        16,
        2,
        CLASS_BUTTON,
        "閉じる",
    );
    let aligned = template.aligned();
    let mut state = Manager {
        names: Vec::new(),
        changed: false,
    };
    unsafe {
        let _ = DialogBoxIndirectParamW(
            None,
            aligned.as_ptr().cast(),
            Some(owner),
            Some(manager_proc),
            LPARAM(&mut state as *mut _ as isize),
        );
    }
    state.changed
}
unsafe fn manager<'a>(hwnd: HWND) -> &'a mut Manager {
    unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Manager) }
}
fn reload(hwnd: HWND) {
    let (config, error) = Config::load();
    if let Some(error) = error {
        error_box(hwnd, &error.to_string());
        return;
    }
    let st = unsafe { manager(hwnd) };
    st.names = config.remote.host.keys().cloned().collect();
    unsafe {
        let list = control(hwnd, LIST);
        SendMessageW(list, LB_RESETCONTENT, None, None);
        for name in &st.names {
            SendMessageW(
                list,
                LB_ADDSTRING,
                None,
                Some(LPARAM(HSTRING::from(name.as_str()).as_ptr() as isize)),
            );
        }
        SendMessageW(list, LB_SETCURSEL, Some(WPARAM(0)), None);
    }
}
extern "system" fn manager_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lp.0);
                reload(hwnd);
                1
            }
            WM_COMMAND => {
                let id = crate::loword(wp.0) as u16;
                if id == 2 {
                    let _ = EndDialog(hwnd, 0);
                    return 1;
                }
                let edit_double = id == LIST && crate::hiword(wp.0) == LBN_DBLCLK;
                if !matches!(id, NEW | EDIT | CLONE | DELETE) && !edit_double {
                    return 0;
                }
                let index = SendMessageW(control(hwnd, LIST), LB_GETCURSEL, None, None).0;
                let selected = usize::try_from(index)
                    .ok()
                    .and_then(|i| manager(hwnd).names.get(i).cloned());
                if id != NEW && selected.is_none() {
                    return 1;
                }
                let Some(path) = connections::default_path() else {
                    error_box(hwnd, "設定フォルダーを取得できません");
                    return 1;
                };
                let (config, error) = Config::load();
                if let Some(error) = error {
                    error_box(hwnd, &error.to_string());
                    return 1;
                }
                let result = if id == DELETE {
                    let name = selected.unwrap();
                    if MessageBoxW(
                        Some(hwnd),
                        &HSTRING::from(format!("接続設定「{name}」を削除しますか？")),
                        &HSTRING::from("SSH接続先"),
                        MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
                    ) != IDYES
                    {
                        return 1;
                    }
                    Profiles::edit(&path, &name, None).map(|_| true)
                } else {
                    let host = selected
                        .as_ref()
                        .filter(|_| id != NEW)
                        .and_then(|n| config.remote.host.get(n))
                        .cloned()
                        .unwrap_or(RemoteHost {
                            protocol: Some(TransferProtocol::Sftp),
                            ..Default::default()
                        });
                    let name = if id == CLONE || id == NEW {
                        String::new()
                    } else {
                        selected.clone().unwrap_or_default()
                    };
                    match edit(hwnd, name, host, id == EDIT || edit_double) {
                        Some((name, host)) => {
                            if (id == NEW || id == CLONE)
                                && config
                                    .remote
                                    .host
                                    .keys()
                                    .any(|n| n.eq_ignore_ascii_case(&name))
                            {
                                error_box(
                                    hwnd,
                                    "同じ名前の接続設定があります。別の接続名を指定してください。",
                                );
                                return 1;
                            }
                            Profiles::edit(&path, &name, Some(host)).map(|_| true)
                        }
                        None => Ok(false),
                    }
                };
                match result {
                    Ok(changed) => {
                        manager(hwnd).changed |= changed;
                        if changed {
                            reload(hwnd);
                        }
                    }
                    Err(e) => error_box(hwnd, &e),
                }
                1
            }
            _ => 0,
        }
    }
}

// Six fields per category, with drop-down choices for enumerations.
const LABELS: [&str; 36] = [
    "接続名",
    "ホスト名／IP",
    "ポート（空欄=SSH設定）",
    "ユーザー名（加工せず送信）",
    "転送方式",
    "開始フォルダー（リモート）",
    "秘密鍵ファイル",
    "踏み台（ProxyJump）",
    "踏み台の中継コマンド",
    "プロキシURL / none",
    "専用エージェント",
    "エージェント配置先",
    "SFTPサブシステム",
    "SFTPサーバーコマンド",
    "ディレクトリ一覧コマンド",
    "一覧形式",
    "ポート転送（;区切り）",
    "",
    "端末の永続セッション",
    "セッション名プレフィクス",
    "",
    "",
    "",
    "",
    "接続タイムアウト（秒）",
    "死活確認（秒、0=無効）",
    "未応答上限回数",
    "公開鍵認証",
    "対話認証（MFA等）",
    "パスワード認証",
    "暗号（優先順、,区切り）",
    "鍵交換（優先順、,区切り）",
    "ホスト鍵（優先順、,区切り）",
    "MAC（優先順、,区切り）",
    "圧縮",
    "execをPTYなしshellで実行",
];
const PAGES: [&str; 6] = [
    "基本",
    "秘密鍵・接続経路",
    "SFTP・SCP／一覧・ポート転送",
    "端末・セッション",
    "SSH認証・接続維持",
    "SSH暗号・互換性",
];
fn choices(field: usize) -> &'static [&'static str] {
    match field {
        4 => &["", "sftp", "scp"],
        10 => &["", "false", "true"],
        15 => &["", "lines", "nul"],
        18 => &["none", "tmux", "screen"],
        27..=29 | 34..=35 => &["false", "true"],
        _ => &[],
    }
}
struct Draft {
    host: RemoteHost,
    values: Vec<String>,
    page: usize,
    fixed_name: bool,
    result: Option<(String, RemoteHost)>,
}
fn option(s: &str) -> Option<String> {
    if s.trim().is_empty() {
        None
    } else {
        Some(s.to_owned())
    }
}
impl Draft {
    fn new(name: String, host: RemoteHost, fixed_name: bool) -> Self {
        let mut values = vec![String::new(); LABELS.len()];
        values[0] = name;
        for (i, v) in [
            (1, &host.hostname),
            (3, &host.user),
            (5, &host.remote_directory),
            (6, &host.identity_file),
            (7, &host.proxy_jump),
            (8, &host.jump_command),
            (9, &host.proxy),
            (11, &host.agent_dir),
            (12, &host.sftp_subsystem),
            (13, &host.sftp_server),
            (14, &host.list_command),
            (15, &host.list_format),
        ] {
            values[i] = v.clone().unwrap_or_default();
        }
        values[2] = host.port.map(|n| n.to_string()).unwrap_or_default();
        values[4] = match host.protocol {
            None => "",
            Some(TransferProtocol::Sftp) => "sftp",
            Some(TransferProtocol::Scp) => "scp",
        }
        .into();
        values[10] = host.use_agent.map(|b| b.to_string()).unwrap_or_default();
        values[16] = host.forward.join(";");
        values[18] = match host.terminal_multiplexer {
            TerminalMultiplexer::None => "none",
            TerminalMultiplexer::Tmux => "tmux",
            TerminalMultiplexer::Screen => "screen",
        }
        .into();
        values[19] = host.terminal_session_prefix.clone();
        values[24] = host.ssh.connect_timeout_secs.to_string();
        values[25] = host
            .ssh
            .keepalive_secs
            .map(|n| n.to_string())
            .unwrap_or_default();
        values[26] = host.ssh.keepalive_max.to_string();
        values[27] = host.ssh.public_key_auth.to_string();
        values[28] = host.ssh.keyboard_interactive_auth.to_string();
        values[29] = host.ssh.password_auth.to_string();
        values[30] = host.ssh.ciphers.join(",");
        values[31] = host.ssh.key_exchange.join(",");
        values[32] = host.ssh.host_key_algorithms.join(",");
        values[33] = host.ssh.macs.join(",");
        values[34] = host.ssh.compression.to_string();
        values[35] = host.ssh.exec_via_shell.to_string();
        Self {
            host,
            values,
            page: 0,
            fixed_name,
            result: None,
        }
    }
    fn build(&self) -> Result<(String, RemoteHost), String> {
        let mut h = self.host.clone();
        let v = &self.values;
        for (i, dest) in [
            (1, &mut h.hostname),
            (3, &mut h.user),
            (5, &mut h.remote_directory),
            (6, &mut h.identity_file),
            (7, &mut h.proxy_jump),
            (8, &mut h.jump_command),
            (9, &mut h.proxy),
            (11, &mut h.agent_dir),
            (12, &mut h.sftp_subsystem),
            (13, &mut h.sftp_server),
            (14, &mut h.list_command),
            (15, &mut h.list_format),
        ] {
            *dest = option(&v[i]);
        }
        h.port = option(&v[2])
            .map(|s| {
                s.parse::<u16>()
                    .map_err(|_| "ポートは1～65535です".to_string())
            })
            .transpose()?;
        h.protocol = match v[4].as_str() {
            "" => None,
            "sftp" => Some(TransferProtocol::Sftp),
            "scp" => Some(TransferProtocol::Scp),
            _ => return Err("転送方式はsftpまたはscpです".into()),
        };
        h.use_agent = option(&v[10])
            .map(|s| {
                s.parse::<bool>()
                    .map_err(|_| "エージェント指定はtrue/falseです".to_string())
            })
            .transpose()?;
        h.forward = v[16]
            .split(';')
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().into())
            .collect();
        h.terminal_multiplexer = match v[18].as_str() {
            "" | "none" => TerminalMultiplexer::None,
            "tmux" => TerminalMultiplexer::Tmux,
            "screen" => TerminalMultiplexer::Screen,
            _ => return Err("端末セッションはnone/tmux/screenです".into()),
        };
        h.terminal_session_prefix = v[19].clone();
        for (index, target) in [
            (24, &mut h.ssh.connect_timeout_secs),
            (26, &mut h.ssh.keepalive_max),
        ] {
            *target = v[index]
                .parse()
                .map_err(|_| format!("{}は整数です", LABELS[index]))?;
        }
        h.ssh.keepalive_secs = option(&v[25])
            .map(|s| {
                s.parse()
                    .map_err(|_| "死活確認の秒数が不正です".to_string())
            })
            .transpose()?;
        for (index, target) in [
            (27, &mut h.ssh.public_key_auth),
            (28, &mut h.ssh.keyboard_interactive_auth),
            (29, &mut h.ssh.password_auth),
            (34, &mut h.ssh.compression),
            (35, &mut h.ssh.exec_via_shell),
        ] {
            *target = v[index]
                .parse()
                .map_err(|_| format!("{}はtrue/falseです", LABELS[index]))?;
        }
        for (index, target) in [
            (30, &mut h.ssh.ciphers),
            (31, &mut h.ssh.key_exchange),
            (32, &mut h.ssh.host_key_algorithms),
            (33, &mut h.ssh.macs),
        ] {
            *target = v[index]
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().into())
                .collect();
        }
        let (_, errors) = yy_remote::forward::parse_all(&h.forward);
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        if let Some(jumps) = &h.proxy_jump
            && !jumps.eq_ignore_ascii_case("none")
        {
            for hop in jumps.split(',') {
                if yy_remote::uri::Target::parse(hop.trim()).is_none() {
                    return Err(format!("踏み台の指定が不正です: {hop}"));
                }
            }
            if h.proxy
                .as_ref()
                .is_some_and(|p| !p.eq_ignore_ascii_case("none"))
            {
                return Err(
                    "ProxyJumpの接続に使うプロキシは、最初の踏み台の接続設定へ登録してください。"
                        .into(),
                );
            }
        }
        if let Some(proxy) = &h.proxy
            && yy_remote::proxy::Proxy::parse(proxy)?.is_some_and(|p| p.password.is_some())
        {
            return Err("プロキシURLにパスワードを保存せず、ユーザー名だけ指定してください。接続時に入力できます。".into());
        }
        if let Some(command) = &h.jump_command {
            yy_remote::ssh_config::jump_command(command, "host.invalid", 22)?;
        }
        connections::validate_host(&h)?;
        let mut test = Profiles::default();
        test.hosts.insert(v[0].clone(), h.clone());
        test.validate()?;
        Ok((v[0].clone(), h))
    }
}
fn edit_template() -> Template {
    let mut t = Template::dialog("SSH接続設定", 400, 230);
    t.item(
        WS_TABSTOP.0 | CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0,
        7,
        7,
        386,
        140,
        PAGE,
        COMBO,
        "",
    );
    for row in 0..6 {
        let y = 33 + row as i16 * 24;
        t.item(0, 7, y + 3, 137, 12, LABEL_BASE + row, CLASS_STATIC, "");
        t.item(
            (WS_TABSTOP | WS_BORDER | WS_VSCROLL).0 | CBS_DROPDOWN as u32 | CBS_AUTOHSCROLL as u32,
            146,
            y,
            247,
            120,
            FIELD_BASE + row,
            COMBO,
            "",
        );
    }
    t.item(0, 7, 179, 386, 25, 0, CLASS_STATIC, "空欄は既存設定／既定値を使用。パスワードは接続時に入力します。\nSecureCube EssentialはSCPを選択。SFTP指定時はエージェント不要。");
    t.item(
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        272,
        209,
        58,
        16,
        1,
        CLASS_BUTTON,
        "保存",
    );
    t.item(
        WS_TABSTOP.0,
        335,
        209,
        58,
        16,
        2,
        CLASS_BUTTON,
        "キャンセル",
    );
    t
}

fn edit(
    owner: HWND,
    name: String,
    host: RemoteHost,
    fixed_name: bool,
) -> Option<(String, RemoteHost)> {
    let aligned = edit_template().aligned();
    let mut draft = Draft::new(name, host, fixed_name);
    unsafe {
        let _ = DialogBoxIndirectParamW(
            None,
            aligned.as_ptr().cast(),
            Some(owner),
            Some(edit_proc),
            LPARAM(&mut draft as *mut _ as isize),
        );
    }
    draft.result
}
unsafe fn draft<'a>(hwnd: HWND) -> &'a mut Draft {
    unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Draft) }
}
fn stash(hwnd: HWND) {
    let st = unsafe { draft(hwnd) };
    for row in 0..6 {
        st.values[st.page * 6 + row] = text(hwnd, FIELD_BASE + row as u16);
    }
}
fn fill(hwnd: HWND) {
    let st = unsafe { draft(hwnd) };
    for row in 0..6 {
        let field = st.page * 6 + row;
        set(hwnd, LABEL_BASE + row as u16, LABELS[field]);
        add_combo(hwnd, FIELD_BASE + row as u16, choices(field));
        set(hwnd, FIELD_BASE + row as u16, &st.values[field]);
        unsafe {
            let visible = if LABELS[field].is_empty() {
                SW_HIDE
            } else {
                SW_SHOW
            };
            let _ = ShowWindow(control(hwnd, LABEL_BASE + row as u16), visible);
            let _ = ShowWindow(control(hwnd, FIELD_BASE + row as u16), visible);
            let _ = EnableWindow(
                control(hwnd, FIELD_BASE + row as u16),
                !(field == 0 && st.fixed_name),
            );
        }
    }
}
extern "system" fn edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lp.0);
                add_combo(hwnd, PAGE, &PAGES);
                SendMessageW(control(hwnd, PAGE), CB_SETCURSEL, Some(WPARAM(0)), None);
                fill(hwnd);
                1
            }
            WM_COMMAND => {
                match crate::loword(wp.0) as u16 {
                    PAGE if crate::hiword(wp.0) == CBN_SELCHANGE => {
                        stash(hwnd);
                        draft(hwnd).page =
                            SendMessageW(control(hwnd, PAGE), CB_GETCURSEL, None, None)
                                .0
                                .max(0) as usize;
                        fill(hwnd);
                    }
                    1 => {
                        stash(hwnd);
                        match draft(hwnd).build() {
                            Ok(result) => {
                                draft(hwnd).result = Some(result);
                                let _ = EndDialog(hwnd, 1);
                            }
                            Err(e) => error_box(hwnd, &e),
                        }
                    }
                    2 => {
                        let _ = EndDialog(hwnd, 0);
                    }
                    _ => return 0,
                }
                1
            }
            _ => 0,
        }
    }
}

pub(crate) fn manage(owner: HWND) -> Option<yy_config::RemoteConfig> {
    if !show(owner) {
        return None;
    }
    let (config, error) = Config::load();
    if let Some(error) = error {
        error_box(owner, &error.to_string());
        return None;
    }
    crate::remote::reload_settings(config.remote.clone());
    info_box(
        owner,
        "接続先設定を保存しました。接続中の端末は維持し、新しく開始する接続に反映します。",
    );
    Some(config.remote)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn draft_round_trip_keeps_advanced_fields_and_gateway_username() {
        let host = RemoteHost {
            user: Some("operator@target@realm".into()),
            protocol: Some(TransferProtocol::Scp),
            forward: vec!["L 8080:localhost:80".into()],
            sftp_server: Some("sudo /usr/lib/openssh/sftp-server".into()),
            ..Default::default()
        };
        let draft = Draft::new("gateway".into(), host.clone(), true);
        assert_eq!(draft.build().unwrap(), ("gateway".into(), host));
    }

    #[test]
    fn native_dialog_preserves_fields_across_pages_including_forwarding_and_tmux() {
        let host = RemoteHost {
            protocol: Some(TransferProtocol::Scp),
            proxy_jump: Some("gateway".into()),
            forward: vec![
                "L 8080:localhost:80".into(),
                "R 9000:localhost:9001".into(),
                "D 1080".into(),
            ],
            terminal_multiplexer: TerminalMultiplexer::Tmux,
            terminal_session_prefix: "work-".into(),
            ..Default::default()
        };
        let mut draft = Draft::new("target".into(), host.clone(), true);
        let template = edit_template().aligned();
        unsafe {
            let hwnd = CreateDialogIndirectParamW(
                None,
                template.as_ptr().cast(),
                None,
                Some(edit_proc),
                LPARAM(&mut draft as *mut _ as isize),
            )
            .unwrap();
            assert!(!hwnd.is_invalid());
            for page in [1, 2, 3, 4, 5, 0] {
                SendMessageW(control(hwnd, PAGE), CB_SETCURSEL, Some(WPARAM(page)), None);
                SendMessageW(
                    hwnd,
                    WM_COMMAND,
                    Some(WPARAM(PAGE as usize | (CBN_SELCHANGE as usize) << 16)),
                    Some(LPARAM(control(hwnd, PAGE).0 as isize)),
                );
            }
            stash(hwnd);
            let result = draft.build();
            let _ = DestroyWindow(hwnd);
            assert_eq!(result.unwrap(), ("target".into(), host));
        }
    }
}

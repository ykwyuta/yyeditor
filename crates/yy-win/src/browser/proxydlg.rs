//! プロキシのプロファイルの編集（19 章 3.2）。メモリ上のダイアログテンプレートから作る
//! （[`crate::goto::Template`]）。

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    BST_CHECKED, CheckDlgButton, EM_LINEFROMCHAR, IsDlgButtonChecked,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;
use yy_browser::rules::{format_hosts, format_rules, parse_hosts, parse_rules};
use yy_browser::{ProfileList, ProxyMode, ProxyProfile};

use crate::goto::{CLASS_BUTTON, CLASS_EDIT, CLASS_STATIC, Template};

const CLASS_COMBO: u16 = 0x0085;

const ID_LIST: u16 = 100;
const ID_NEW: u16 = 101;
const ID_DELETE: u16 = 102;
const ID_NAME: u16 = 103;
const ID_MODE: u16 = 104;
const ID_SERVER: u16 = 105;
const ID_BYPASS: u16 = 106;
const ID_PAC: u16 = 107;
const ID_DEFAULT: u16 = 108;
const ID_RULES: u16 = 109;
const ID_HOSTS: u16 = 110;
const ID_MAKE_CERT: u16 = 111;
const ID_PIN_CERT: u16 = 112;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;

struct State {
    list: ProfileList,
    /// 表示中のプロファイル（`list.profiles` の番号）
    current: usize,
    /// OK で閉じたときの結果
    done: bool,
    /// 読めなかった欄の理由（`store` で。OK のときに出す）
    parse_error: Option<String>,
}

/// プロファイルの一覧を編集する。OK なら編集後の一覧（確かめ済み）。
pub(super) fn edit(owner: HWND, list: ProfileList) -> Option<ProfileList> {
    let mut t = Template::dialog("プロキシの設定", 300, 334);
    let label = |t: &mut Template, y: i16, text: &str| {
        t.item(0, 7, y + 2, 60, 10, 0xFFFF, CLASS_STATIC, text);
    };
    label(&mut t, 7, "プロファイル");
    t.item(
        (WS_TABSTOP | WS_VSCROLL).0 | CBS_DROPDOWNLIST as u32,
        70,
        7,
        120,
        120,
        ID_LIST,
        CLASS_COMBO,
        "",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        196,
        6,
        46,
        14,
        ID_NEW,
        CLASS_BUTTON,
        "新規",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        246,
        6,
        46,
        14,
        ID_DELETE,
        CLASS_BUTTON,
        "削除",
    );
    label(&mut t, 30, "名前");
    t.item(
        (WS_BORDER | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32,
        70,
        30,
        222,
        13,
        ID_NAME,
        CLASS_EDIT,
        "",
    );
    label(&mut t, 50, "やり方");
    t.item(
        (WS_TABSTOP | WS_VSCROLL).0 | CBS_DROPDOWNLIST as u32,
        70,
        50,
        120,
        100,
        ID_MODE,
        CLASS_COMBO,
        "",
    );
    label(&mut t, 72, "プロキシ");
    t.item(
        (WS_BORDER | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32,
        70,
        72,
        222,
        13,
        ID_SERVER,
        CLASS_EDIT,
        "",
    );
    t.item(
        0,
        70,
        87,
        222,
        10,
        0xFFFF,
        CLASS_STATIC,
        "例: 127.0.0.1:8888・socks5://127.0.0.1:1080・http=h:p;https=h:p",
    );
    label(&mut t, 102, "除くホスト");
    t.item(
        (WS_BORDER | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32,
        70,
        102,
        222,
        13,
        ID_BYPASS,
        CLASS_EDIT,
        "",
    );
    t.item(
        0,
        70,
        117,
        222,
        10,
        0xFFFF,
        CLASS_STATIC,
        "; 区切り。例: <local>;*.example.co.jp;192.168.0.0/16",
    );
    label(&mut t, 132, "PAC の URL");
    t.item(
        (WS_BORDER | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32,
        70,
        132,
        222,
        13,
        ID_PAC,
        CLASS_EDIT,
        "",
    );
    let multi = (WS_BORDER | WS_TABSTOP | WS_VSCROLL).0
        | (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32;
    label(&mut t, 152, "ドメインごと");
    t.item(multi, 70, 152, 222, 42, ID_RULES, CLASS_EDIT, "");
    t.item(
        0,
        70,
        196,
        222,
        18,
        0xFFFF,
        CLASS_STATIC,
        "1 行に 1 つ。上から順に当てはめる（「使わない」「指定」のとき）。\n例: *.corp.example.jp = 10.0.0.1:8080 ／ example.org = direct",
    );
    label(&mut t, 218, "ホストの転送");
    t.item(multi, 70, 218, 222, 42, ID_HOSTS, CLASS_EDIT, "");
    t.item(
        0,
        70,
        262,
        222,
        18,
        0xFFFF,
        CLASS_STATIC,
        "例: www.example.com = 127.0.0.1:8443（https://www.example.com/ を 127.0.0.1:8443 へ）。\ncert=指紋 で開発者用証明書を受け入れる",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        70,
        282,
        110,
        14,
        ID_MAKE_CERT,
        CLASS_BUTTON,
        "開発者用証明書を作る…",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        184,
        282,
        108,
        14,
        ID_PIN_CERT,
        CLASS_BUTTON,
        "証明書ファイルを登録…",
    );
    t.item(
        WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
        70,
        300,
        222,
        12,
        ID_DEFAULT,
        CLASS_BUTTON,
        "起動するときにこのプロファイルを使う",
    );
    t.item(
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        188,
        314,
        50,
        14,
        IDOK_,
        CLASS_BUTTON,
        "保存",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        242,
        314,
        50,
        14,
        IDCANCEL_,
        CLASS_BUTTON,
        "キャンセル",
    );
    let aligned = t.aligned();
    let current = list
        .profiles
        .iter()
        .position(|p| p.name == list.default)
        .unwrap_or(0);
    let mut state = State {
        list,
        current,
        done: false,
        parse_error: None,
    };
    unsafe {
        DialogBoxIndirectParamW(
            None,
            aligned.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            Some(dialog_proc),
            LPARAM(&mut state as *mut State as isize),
        );
    }
    state.done.then_some(state.list)
}

fn get_text(dlg: HWND, id: u16) -> String {
    unsafe {
        let h = GetDlgItem(Some(dlg), id as i32).unwrap_or_default();
        let n = GetWindowTextLengthW(h);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..got])
    }
}

fn set_text(dlg: HWND, id: u16, s: &str) {
    unsafe {
        let _ = SetDlgItemTextW(dlg, id as i32, &HSTRING::from(s));
    }
}

fn combo_sel(dlg: HWND, id: u16) -> isize {
    unsafe { SendDlgItemMessageW(dlg, id as i32, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 }
}

/// 一覧の組み合わせボックスを作り直す。
fn fill_list(dlg: HWND, st: &State) {
    unsafe {
        SendDlgItemMessageW(dlg, ID_LIST as i32, CB_RESETCONTENT, WPARAM(0), LPARAM(0));
        for p in &st.list.profiles {
            let s = HSTRING::from(p.name.as_str());
            SendDlgItemMessageW(
                dlg,
                ID_LIST as i32,
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(s.as_ptr() as isize),
            );
        }
        SendDlgItemMessageW(
            dlg,
            ID_LIST as i32,
            CB_SETCURSEL,
            WPARAM(st.current),
            LPARAM(0),
        );
    }
}

/// 表示中のプロファイルを欄に出す。
fn show(dlg: HWND, st: &State) {
    let Some(p) = st.list.profiles.get(st.current) else {
        return;
    };
    set_text(dlg, ID_NAME, &p.name);
    set_text(dlg, ID_SERVER, &p.server);
    set_text(dlg, ID_BYPASS, &p.bypass);
    set_text(dlg, ID_PAC, &p.pac_url);
    set_text(dlg, ID_RULES, &format_rules(&p.rules));
    set_text(dlg, ID_HOSTS, &format_hosts(&p.hosts));
    let mode = ProxyMode::ALL
        .iter()
        .position(|m| *m == p.mode)
        .unwrap_or(0);
    unsafe {
        SendDlgItemMessageW(dlg, ID_MODE as i32, CB_SETCURSEL, WPARAM(mode), LPARAM(0));
        let _ = CheckDlgButton(
            dlg,
            ID_DEFAULT as i32,
            if st.list.default == p.name {
                BST_CHECKED
            } else {
                windows::Win32::UI::Controls::DLG_BUTTON_CHECK_STATE(0)
            },
        );
    }
    enable_fields(dlg, p.mode);
}

/// やり方に合わせて欄を使える・使えないにする。
fn enable_fields(dlg: HWND, mode: ProxyMode) {
    unsafe {
        for (id, on) in [
            (ID_SERVER, mode == ProxyMode::Manual),
            (ID_BYPASS, mode == ProxyMode::Manual),
            (ID_PAC, mode == ProxyMode::Pac),
        ] {
            if let Ok(h) = GetDlgItem(Some(dlg), id as i32) {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(h, on);
            }
        }
    }
}

/// 欄の内容を表示中のプロファイルに書き戻す（確かめはしない）。
fn store(dlg: HWND, st: &mut State) {
    let mode_i = combo_sel(dlg, ID_MODE).max(0) as usize;
    let checked = unsafe { IsDlgButtonChecked(dlg, ID_DEFAULT as i32) } == BST_CHECKED.0;
    let Some(p) = st.list.profiles.get_mut(st.current) else {
        return;
    };
    let old_name = p.name.clone();
    p.name = get_text(dlg, ID_NAME).trim().to_owned();
    p.mode = ProxyMode::ALL.get(mode_i).copied().unwrap_or_default();
    p.server = get_text(dlg, ID_SERVER).trim().to_owned();
    p.bypass = get_text(dlg, ID_BYPASS).trim().to_owned();
    p.pac_url = get_text(dlg, ID_PAC).trim().to_owned();
    st.parse_error = None;
    match parse_rules(&get_text(dlg, ID_RULES)) {
        Ok(r) => p.rules = r,
        Err(e) => st.parse_error = Some(format!("「{}」のドメインごとのプロキシ: {e}", p.name)),
    }
    match parse_hosts(&get_text(dlg, ID_HOSTS)) {
        Ok(h) => p.hosts = h,
        Err(e) => {
            st.parse_error
                .get_or_insert(format!("「{}」のホストの転送: {e}", p.name));
        }
    }
    if checked || st.list.default == old_name {
        st.list.default = if checked {
            p.name.clone()
        } else {
            String::new()
        };
    }
}

/// すべてのプロファイルを確かめる（だめなら、そのプロファイルを表示して理由を返す）。
fn check_all(st: &mut State) -> Result<(), String> {
    for (i, p) in st.list.profiles.iter().enumerate() {
        if let Err(e) = p.validate() {
            st.current = i;
            return Err(format!("「{}」: {e}", p.name));
        }
        if st.list.profiles[..i].iter().any(|q| q.name == p.name) {
            st.current = i;
            return Err(format!(
                "「{}」という名前のプロファイルが 2 つあります",
                p.name
            ));
        }
    }
    if st.list.get(&st.list.default).is_none() {
        st.list.default = st.list.profiles[0].name.clone();
    }
    Ok(())
}

extern "system" fn dialog_proc(dlg: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(dlg, GWLP_USERDATA, lparam.0);
                let st = &*(lparam.0 as *const State);
                for m in ProxyMode::ALL {
                    let s = HSTRING::from(m.label());
                    SendDlgItemMessageW(
                        dlg,
                        ID_MODE as i32,
                        CB_ADDSTRING,
                        WPARAM(0),
                        LPARAM(s.as_ptr() as isize),
                    );
                }
                fill_list(dlg, st);
                show(dlg, st);
                1
            }
            WM_COMMAND => {
                let st = &mut *(GetWindowLongPtrW(dlg, GWLP_USERDATA) as *mut State);
                let id = (wparam.0 & 0xffff) as u16;
                let code = ((wparam.0 >> 16) & 0xffff) as u32;
                match id {
                    ID_LIST if code == CBN_SELCHANGE => {
                        store(dlg, st);
                        if let Some(e) = st.parse_error.take() {
                            fill_list(dlg, st);
                            crate::util::error_box(
                                dlg,
                                &format!("{e}\n（この欄は前の内容のままです）"),
                            );
                        }
                        let sel = combo_sel(dlg, ID_LIST);
                        if sel >= 0 {
                            st.current = sel as usize;
                        }
                        fill_list(dlg, st);
                        show(dlg, st);
                    }
                    ID_MODE if code == CBN_SELCHANGE => {
                        let m = ProxyMode::ALL
                            .get(combo_sel(dlg, ID_MODE).max(0) as usize)
                            .copied()
                            .unwrap_or_default();
                        enable_fields(dlg, m);
                    }
                    ID_NEW => {
                        store(dlg, st);
                        let mut n = 1;
                        let name = loop {
                            let cand = format!("新しいプロファイル {n}");
                            if st.list.get(&cand).is_none() {
                                break cand;
                            }
                            n += 1;
                        };
                        st.list.profiles.push(ProxyProfile {
                            name,
                            mode: ProxyMode::Manual,
                            server: "127.0.0.1:8080".into(),
                            ..ProxyProfile::default()
                        });
                        st.current = st.list.profiles.len() - 1;
                        fill_list(dlg, st);
                        show(dlg, st);
                        if let Ok(h) = GetDlgItem(Some(dlg), ID_NAME as i32) {
                            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(h));
                        }
                    }
                    ID_DELETE => {
                        if st.list.profiles.len() <= 1 {
                            crate::util::info_box(dlg, "最後のプロファイルは消せません。");
                        } else {
                            let name = st.list.profiles[st.current].name.clone();
                            let _ = st.list.remove(&name);
                            st.current = st.current.min(st.list.profiles.len() - 1);
                            fill_list(dlg, st);
                            show(dlg, st);
                        }
                    }
                    ID_MAKE_CERT => make_cert(dlg),
                    ID_PIN_CERT => pin_cert(dlg),
                    IDOK_ => {
                        store(dlg, st);
                        if let Some(e) = st.parse_error.take() {
                            crate::util::error_box(dlg, &e);
                            return 1;
                        }
                        match check_all(st) {
                            Ok(()) => {
                                st.done = true;
                                let _ = EndDialog(dlg, IDOK_ as isize);
                            }
                            Err(e) => {
                                fill_list(dlg, st);
                                show(dlg, st);
                                crate::util::error_box(dlg, &e);
                            }
                        }
                    }
                    IDCANCEL_ => {
                        let _ = EndDialog(dlg, IDCANCEL_ as isize);
                    }
                    _ => {}
                }
                1
            }
            _ => 0,
        }
    }
}

/// 転送の欄の、カーソルのある行（番号と内容）。
fn caret_line(dlg: HWND) -> (usize, Vec<String>) {
    let text = get_text(dlg, ID_HOSTS);
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let n = unsafe {
        SendDlgItemMessageW(
            dlg,
            ID_HOSTS as i32,
            EM_LINEFROMCHAR,
            WPARAM(usize::MAX),
            LPARAM(0),
        )
        .0
    }
    .max(0) as usize;
    (n, lines)
}

/// 転送の欄のカーソルの行を読む（だめなら理由を出して `None`）。
fn caret_mapping(dlg: HWND) -> Option<(usize, Vec<String>, yy_browser::HostMap)> {
    let (n, lines) = caret_line(dlg);
    let line = lines.get(n).cloned().unwrap_or_default();
    match parse_hosts(&line) {
        Ok(mut v) if v.len() == 1 => Some((n, lines, v.remove(0))),
        Ok(_) => {
            crate::util::info_box(
                dlg,
                "「ホストの転送」の欄で、証明書を使う行（例: www.example.com = 127.0.0.1:8443）に\
                 カーソルを置いてから押してください。",
            );
            None
        }
        Err(e) => {
            crate::util::error_box(dlg, &e);
            None
        }
    }
}

/// カーソルの行に指紋を書き込む。
fn set_pin(dlg: HWND, n: usize, mut lines: Vec<String>, mut m: yy_browser::HostMap, fp: String) {
    m.cert_sha256 = fp;
    let formatted = format_hosts(std::slice::from_ref(&m));
    if n < lines.len() {
        lines[n] = formatted;
    } else {
        lines.push(formatted);
    }
    set_text(dlg, ID_HOSTS, &lines.join("\r\n"));
}

/// 開発者用証明書を作り（自己署名）、カーソルの行に指紋を書き込む。
fn make_cert(dlg: HWND) {
    let Some(generate) = super::DEV_CERT.with(|d| d.get()) else {
        crate::util::error_box(
            dlg,
            "このビルドの yybrowser では開発者用証明書を作れません。",
        );
        return;
    };
    let Some((n, lines, m)) = caret_mapping(dlg) else {
        return;
    };
    if !m.cert_sha256.is_empty() {
        let ok = unsafe {
            MessageBoxW(
                Some(dlg),
                &HSTRING::from(format!(
                    "{} にはもう開発者用証明書（{}）が登録されています。作り直しますか？\n\
                     （転送先のサーバーの証明書も入れ替える必要があります）",
                    m.host, m.cert_sha256
                )),
                windows::core::w!("yybrowser"),
                MB_OKCANCEL | MB_ICONQUESTION,
            )
        } == IDOK;
        if !ok {
            return;
        }
    }
    let cert = match generate(&m.host) {
        Ok(c) => c,
        Err(e) => {
            crate::util::error_box(dlg, &format!("証明書を作れません: {e}"));
            return;
        }
    };
    let dir = super::dev_cert_dir();
    let (crt, key) = match yy_browser::rules::write_dev_cert(&dir, &m.host, &cert) {
        Ok(v) => v,
        Err(e) => {
            crate::util::error_box(
                dlg,
                &format!("証明書を保存できません: {}: {e}", dir.display()),
            );
            return;
        }
    };
    let host = m.host.clone();
    let address = m.address.clone();
    set_pin(dlg, n, lines, m, cert.sha256.clone());
    crate::util::info_box(
        dlg,
        &format!(
            "{host} の開発者用証明書を作りました。\n\n証明書: {}\n秘密鍵: {}\nSHA-256: {}\n\n\
             転送先（{address}）のサーバーに、この証明書と秘密鍵を設定してください。\n\
             yybrowser は、サーバーがこの証明書を出したときだけ証明書のエラーを許し、アドレスバーに\
             「開発者用証明書を利用中」と表示します。OS の証明書ストアには入れません。\n\
             （保存すると有効になります）",
            crt.display(),
            key.display(),
            cert.sha256
        ),
    );
}

/// 既にある証明書のファイル（PEM・DER）の指紋を、カーソルの行に登録する。
fn pin_cert(dlg: HWND) {
    let Some((n, lines, m)) = caret_mapping(dlg) else {
        return;
    };
    let Some(path) = crate::fm::pick_file(
        dlg,
        (
            "証明書 (*.crt;*.pem;*.cer;*.der)",
            "*.crt;*.pem;*.cer;*.der",
        ),
        Some(&super::dev_cert_dir()),
        None,
    ) else {
        return;
    };
    let fp = std::fs::read(&path)
        .ok()
        .and_then(|b| yy_browser::rules::cert_fingerprint(&b));
    match fp {
        Some(fp) => {
            set_pin(dlg, n, lines, m, fp.clone());
            crate::util::info_box(
                dlg,
                &format!(
                    "{} を登録しました。\nSHA-256: {fp}\n（保存すると有効になります）",
                    path.display()
                ),
            );
        }
        None => crate::util::error_box(
            dlg,
            &format!("{} は証明書（PEM・DER）として読めません。", path.display()),
        ),
    }
}

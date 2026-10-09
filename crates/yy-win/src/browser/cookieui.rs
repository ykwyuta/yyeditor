//! Cookie の編集（開発者用。19 章 4.5）: 表示中のページの URL に送られる Cookie の一覧と、追加・編集・削除。
//! WebView2 の `ICoreWebView2CookieManager`（そのプロファイルのデータ）を使う。

use std::cell::RefCell;
use std::rc::Rc;

use webview2_com::GetCookiesCompletedHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, SYSTEMTIME, WPARAM};
use windows::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};
use windows::Win32::UI::Controls::{
    BST_CHECKED, BST_UNCHECKED, CheckDlgButton, EM_SETCUEBANNER, IsDlgButtonChecked,
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, w};
use yy_browser::cookies::{CookieFields, DateTime, SameSite, parse_datetime};

use crate::goto::{CLASS_BUTTON, CLASS_EDIT, CLASS_STATIC, Template};
use crate::preview::take_string;

const CLASS_LISTBOX: u16 = 0x0083;
const CLASS_COMBO: u16 = 0x0085;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;
const NO_ID: u16 = 0xFFFF;

/// 手元の時刻の日時 → UNIX 時間の秒。
fn local_to_unix(d: &DateTime) -> Option<f64> {
    let local = SYSTEMTIME {
        wYear: d.year,
        wMonth: d.month,
        wDay: d.day,
        wHour: d.hour,
        wMinute: d.minute,
        wSecond: d.second,
        ..Default::default()
    };
    let mut utc = SYSTEMTIME::default();
    let mut ft = FILETIME::default();
    unsafe {
        TzSpecificLocalTimeToSystemTime(None, &local, &mut utc).ok()?;
        SystemTimeToFileTime(&utc, &mut ft).ok()?;
    }
    let t = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    Some((t / 10_000_000) as f64 - 11_644_473_600.0)
}

/// UNIX 時間の秒 → 手元の時刻の `2026-10-09 14:23`。
fn unix_to_local(t: f64) -> String {
    super::historyui::local_time(t.max(0.0) as u64)
}

/// WebView2 の Cookie → 欄。
pub(super) fn fields_of(c: &ICoreWebView2Cookie) -> CookieFields {
    unsafe {
        let mut expires = -1f64;
        let mut session = windows::core::BOOL(0);
        let mut http_only = windows::core::BOOL(0);
        let mut secure = windows::core::BOOL(0);
        let mut ss = COREWEBVIEW2_COOKIE_SAME_SITE_KIND::default();
        let _ = c.Expires(&mut expires);
        let _ = c.IsSession(&mut session);
        let _ = c.IsHttpOnly(&mut http_only);
        let _ = c.IsSecure(&mut secure);
        let _ = c.SameSite(&mut ss);
        CookieFields {
            name: take_string(|p| c.Name(p)),
            value: take_string(|p| c.Value(p)),
            domain: take_string(|p| c.Domain(p)),
            path: take_string(|p| c.Path(p)),
            expires: (!session.as_bool() && expires >= 0.0).then_some(expires),
            http_only: http_only.as_bool(),
            secure: secure.as_bool(),
            same_site: match ss {
                COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE => SameSite::None,
                COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT => SameSite::Strict,
                _ => SameSite::Lax,
            },
        }
    }
}

/// 欄から Cookie を作って入れる（同じ名前・ドメイン・パスのものは置き換わる）。
pub(super) fn apply(m: &ICoreWebView2CookieManager, f: &CookieFields) -> windows::core::Result<()> {
    unsafe {
        let c = m.CreateCookie(
            &HSTRING::from(f.name.trim()),
            &HSTRING::from(f.value.as_str()),
            &HSTRING::from(f.domain.trim()),
            &HSTRING::from(f.path.trim()),
        )?;
        if let Some(t) = f.expires {
            c.SetExpires(t)?;
        }
        c.SetIsHttpOnly(f.http_only)?;
        c.SetIsSecure(f.secure)?;
        c.SetSameSite(match f.same_site {
            SameSite::None => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE,
            SameSite::Lax => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX,
            SameSite::Strict => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT,
        })?;
        m.AddOrUpdateCookie(&c)
    }
}

// ---- 一覧 ----------------------------------------------------------------------------------

const L_LIST: u16 = 100;
const L_ADD: u16 = 101;
const L_EDIT: u16 = 102;
const L_DELETE: u16 = 103;
const L_DELETE_ALL: u16 = 104;
const L_RELOAD: u16 = 105;
const L_COUNT: u16 = 106;

struct ListState {
    manager: ICoreWebView2CookieManager,
    uri: String,
    cookies: Rc<RefCell<Vec<ICoreWebView2Cookie>>>,
    /// 変えたか（閉じたらページを読み込み直すかを尋ねる）
    changed: bool,
}

/// Cookie の一覧の画面。変えたら `true`。
pub(super) fn show(owner: HWND, manager: ICoreWebView2CookieManager, uri: String) -> bool {
    let host = yy_browser::rules::url_host_port(&uri)
        .map(|(_, h, _)| h)
        .unwrap_or_else(|| uri.clone());
    let mut t = Template::dialog(&format!("Cookie の編集 — {host}"), 470, 250);
    t.item(
        0,
        7,
        7,
        380,
        10,
        NO_ID,
        CLASS_STATIC,
        "このページの URL に送られる Cookie（このプロファイル）",
    );
    t.item(
        (WS_BORDER | WS_TABSTOP | WS_VSCROLL | WS_HSCROLL).0
            | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
        7,
        20,
        384,
        204,
        L_LIST,
        CLASS_LISTBOX,
        "",
    );
    let mut y = 20;
    for (id, text) in [
        (L_ADD, "追加..."),
        (L_EDIT, "編集..."),
        (L_DELETE, "削除"),
        (L_DELETE_ALL, "すべて削除"),
        (L_RELOAD, "読み直す"),
    ] {
        t.item(
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            397,
            y,
            66,
            14,
            id,
            CLASS_BUTTON,
            text,
        );
        y += 17;
    }
    t.item(0, 7, 231, 300, 10, L_COUNT, CLASS_STATIC, "");
    t.item(
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        413,
        229,
        50,
        14,
        IDCANCEL_,
        CLASS_BUTTON,
        "閉じる",
    );
    let aligned = t.aligned();
    let mut st = ListState {
        manager,
        uri,
        cookies: Rc::default(),
        changed: false,
    };
    unsafe {
        DialogBoxIndirectParamW(
            None,
            aligned.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            Some(list_proc),
            LPARAM(&mut st as *mut ListState as isize),
        );
    }
    st.changed
}

/// Cookie を取り直して一覧に出す（取れたら非同期で出す）。
fn reload(dlg: HWND, st: &ListState) {
    let cookies = st.cookies.clone();
    let handler = GetCookiesCompletedHandler::create(Box::new(move |r, list| {
        let mut v = Vec::new();
        if let (Ok(()), Some(list)) = (r, list) {
            let mut n = 0u32;
            unsafe {
                let _ = list.Count(&mut n);
                for i in 0..n {
                    if let Ok(c) = list.GetValueAtIndex(i) {
                        v.push(c);
                    }
                }
            }
        }
        *cookies.borrow_mut() = v;
        if unsafe { IsWindow(Some(dlg)) }.as_bool() {
            fill(dlg, &cookies.borrow());
        }
        Ok(())
    }));
    unsafe {
        let _ = st
            .manager
            .GetCookies(&HSTRING::from(st.uri.as_str()), &handler);
    }
}

fn fill(dlg: HWND, cookies: &[ICoreWebView2Cookie]) {
    unsafe {
        let keep = SendDlgItemMessageW(dlg, L_LIST as i32, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
        SendDlgItemMessageW(dlg, L_LIST as i32, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
        let mut widest = 0;
        for c in cookies {
            let s = fields_of(c).describe(unix_to_local);
            widest = widest.max(s.chars().count());
            let s = HSTRING::from(s);
            SendDlgItemMessageW(
                dlg,
                L_LIST as i32,
                LB_ADDSTRING,
                WPARAM(0),
                LPARAM(s.as_ptr() as isize),
            );
        }
        SendDlgItemMessageW(
            dlg,
            L_LIST as i32,
            LB_SETHORIZONTALEXTENT,
            WPARAM(widest * 14),
            LPARAM(0),
        );
        let sel = if keep >= 0 && (keep as usize) < cookies.len() {
            keep as usize
        } else {
            0
        };
        SendDlgItemMessageW(dlg, L_LIST as i32, LB_SETCURSEL, WPARAM(sel), LPARAM(0));
        let _ = SetDlgItemTextW(
            dlg,
            L_COUNT as i32,
            &HSTRING::from(format!("{} 個", cookies.len())),
        );
    }
}

fn selected(dlg: HWND, st: &ListState) -> Option<ICoreWebView2Cookie> {
    let i =
        unsafe { SendDlgItemMessageW(dlg, L_LIST as i32, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    (i >= 0)
        .then(|| st.cookies.borrow().get(i as usize).cloned())
        .flatten()
}

extern "system" fn list_proc(dlg: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(dlg, GWLP_USERDATA, lparam.0);
                let st = &*(lparam.0 as *const ListState);
                reload(dlg, st);
                1
            }
            WM_COMMAND => {
                let st = &mut *(GetWindowLongPtrW(dlg, GWLP_USERDATA) as *mut ListState);
                let id = (wparam.0 & 0xffff) as u16;
                let code = ((wparam.0 >> 16) & 0xffff) as u32;
                match id {
                    L_ADD => {
                        let (scheme, host) = yy_browser::rules::url_host_port(&st.uri)
                            .map(|(s, h, _)| (s, h))
                            .unwrap_or_default();
                        let blank = CookieFields {
                            domain: host,
                            path: "/".into(),
                            secure: scheme == "https",
                            ..CookieFields::default()
                        };
                        if let Some(f) = edit(dlg, blank, false) {
                            match apply(&st.manager, &f) {
                                Ok(()) => st.changed = true,
                                Err(e) => crate::util::error_box(
                                    dlg,
                                    &format!("入れられません: {}", e.message()),
                                ),
                            }
                            reload(dlg, st);
                        }
                    }
                    L_EDIT => edit_selected(dlg, st),
                    L_LIST if code == LBN_DBLCLK => edit_selected(dlg, st),
                    L_DELETE => {
                        if let Some(c) = selected(dlg, st) {
                            let _ = st.manager.DeleteCookie(&c);
                            st.changed = true;
                            reload(dlg, st);
                        }
                    }
                    L_DELETE_ALL => {
                        let ok = MessageBoxW(
                            Some(dlg),
                            w!(
                                "このページの URL に送られる Cookie をすべて消しますか？\n（ログインが外れることがあります）"
                            ),
                            w!("yybrowser"),
                            MB_OKCANCEL | MB_ICONQUESTION,
                        ) == IDOK;
                        if ok {
                            let all: Vec<ICoreWebView2Cookie> = st.cookies.borrow().clone();
                            for c in &all {
                                let _ = st.manager.DeleteCookie(c);
                            }
                            st.changed = true;
                            reload(dlg, st);
                        }
                    }
                    L_RELOAD => reload(dlg, st),
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

fn edit_selected(dlg: HWND, st: &mut ListState) {
    let Some(old) = selected(dlg, st) else {
        return;
    };
    let before = fields_of(&old);
    let Some(f) = edit(dlg, before.clone(), true) else {
        return;
    };
    // 名前・ドメイン・パスを変えたら、前のものは消す（別の Cookie になるため）
    let renamed = f.name != before.name || f.domain != before.domain || f.path != before.path;
    match apply(&st.manager, &f) {
        Ok(()) => {
            if renamed {
                unsafe {
                    let _ = st.manager.DeleteCookie(&old);
                }
            }
            st.changed = true;
        }
        Err(e) => crate::util::error_box(dlg, &format!("入れられません: {}", e.message())),
    }
    reload(dlg, st);
}

// ---- 1 つの編集 ----------------------------------------------------------------------------

const E_NAME: u16 = 200;
const E_VALUE: u16 = 201;
const E_DOMAIN: u16 = 202;
const E_PATH: u16 = 203;
const E_EXPIRY: u16 = 204;
const E_DATE: u16 = 205;
const E_HTTPONLY: u16 = 206;
const E_SECURE: u16 = 207;
const E_SAMESITE: u16 = 208;

struct EditState {
    fields: CookieFields,
    existing: bool,
    result: Option<CookieFields>,
}

/// Cookie を 1 つ足す・直す画面。
fn edit(owner: HWND, fields: CookieFields, existing: bool) -> Option<CookieFields> {
    let title = if existing {
        "Cookie の編集"
    } else {
        "Cookie の追加"
    };
    let mut t = Template::dialog(title, 320, 196);
    let label =
        |t: &mut Template, y: i16, s: &str| t.item(0, 7, y + 2, 56, 10, NO_ID, CLASS_STATIC, s);
    let edit_style = (WS_BORDER | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32;
    label(&mut t, 7, "名前");
    t.item(edit_style, 66, 7, 247, 13, E_NAME, CLASS_EDIT, "");
    label(&mut t, 25, "値");
    t.item(
        (WS_BORDER | WS_TABSTOP | WS_VSCROLL).0 | (ES_MULTILINE | ES_AUTOVSCROLL) as u32,
        66,
        25,
        247,
        34,
        E_VALUE,
        CLASS_EDIT,
        "",
    );
    label(&mut t, 65, "ドメイン");
    t.item(edit_style, 66, 65, 247, 13, E_DOMAIN, CLASS_EDIT, "");
    label(&mut t, 83, "パス");
    t.item(edit_style, 66, 83, 247, 13, E_PATH, CLASS_EDIT, "");
    label(&mut t, 101, "期限");
    t.item(
        (WS_TABSTOP | WS_VSCROLL).0 | CBS_DROPDOWNLIST as u32,
        66,
        101,
        110,
        80,
        E_EXPIRY,
        CLASS_COMBO,
        "",
    );
    t.item(edit_style, 180, 101, 133, 13, E_DATE, CLASS_EDIT, "");
    label(&mut t, 121, "属性");
    t.item(
        WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
        66,
        121,
        70,
        12,
        E_HTTPONLY,
        CLASS_BUTTON,
        "HttpOnly",
    );
    t.item(
        WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
        140,
        121,
        70,
        12,
        E_SECURE,
        CLASS_BUTTON,
        "Secure",
    );
    label(&mut t, 139, "SameSite");
    t.item(
        (WS_TABSTOP | WS_VSCROLL).0 | CBS_DROPDOWNLIST as u32,
        66,
        139,
        80,
        80,
        E_SAMESITE,
        CLASS_COMBO,
        "",
    );
    t.item(
        0,
        7,
        157,
        306,
        10,
        NO_ID,
        CLASS_STATIC,
        "ドメインの先頭に「.」を付けると、サブドメインにも送られます。",
    );
    t.item(
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        209,
        175,
        50,
        14,
        IDOK_,
        CLASS_BUTTON,
        "保存",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        263,
        175,
        50,
        14,
        IDCANCEL_,
        CLASS_BUTTON,
        "キャンセル",
    );
    let aligned = t.aligned();
    let mut st = EditState {
        fields,
        existing,
        result: None,
    };
    unsafe {
        DialogBoxIndirectParamW(
            None,
            aligned.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            Some(edit_proc),
            LPARAM(&mut st as *mut EditState as isize),
        );
    }
    st.result
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

fn combo(dlg: HWND, id: u16, items: &[&str], sel: usize) {
    unsafe {
        for s in items {
            let s = HSTRING::from(*s);
            SendDlgItemMessageW(
                dlg,
                id as i32,
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(s.as_ptr() as isize),
            );
        }
        SendDlgItemMessageW(dlg, id as i32, CB_SETCURSEL, WPARAM(sel), LPARAM(0));
    }
}

fn combo_sel(dlg: HWND, id: u16) -> usize {
    unsafe {
        SendDlgItemMessageW(dlg, id as i32, CB_GETCURSEL, WPARAM(0), LPARAM(0))
            .0
            .max(0) as usize
    }
}

fn check(dlg: HWND, id: u16, on: bool) {
    unsafe {
        let _ = CheckDlgButton(dlg, id as i32, if on { BST_CHECKED } else { BST_UNCHECKED });
    }
}

fn checked(dlg: HWND, id: u16) -> bool {
    unsafe { IsDlgButtonChecked(dlg, id as i32) == BST_CHECKED.0 }
}

fn date_enable(dlg: HWND) {
    unsafe {
        if let Ok(h) = GetDlgItem(Some(dlg), E_DATE as i32) {
            let _ = EnableWindow(h, combo_sel(dlg, E_EXPIRY) == 1);
        }
    }
}

extern "system" fn edit_proc(dlg: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(dlg, GWLP_USERDATA, lparam.0);
                let st = &*(lparam.0 as *const EditState);
                let f = &st.fields;
                set_text(dlg, E_NAME, &f.name);
                set_text(dlg, E_VALUE, &f.value);
                set_text(dlg, E_DOMAIN, &f.domain);
                set_text(dlg, E_PATH, &f.path);
                combo(
                    dlg,
                    E_EXPIRY,
                    &["セッション", "日時を指定"],
                    f.expires.is_some() as usize,
                );
                let date = match f.expires {
                    Some(t) => unix_to_local(t),
                    // 指定にしたときの既定: 1 年後
                    None => unix_to_local(yy_adblock::lists::now() as f64 + 365.0 * 86_400.0),
                };
                set_text(dlg, E_DATE, &date);
                let cue = HSTRING::from("例: 2026-12-31 23:59");
                SendDlgItemMessageW(
                    dlg,
                    E_DATE as i32,
                    EM_SETCUEBANNER,
                    WPARAM(1),
                    LPARAM(cue.as_ptr() as isize),
                );
                check(dlg, E_HTTPONLY, f.http_only);
                check(dlg, E_SECURE, f.secure);
                let labels: Vec<&str> = SameSite::ALL.iter().map(|s| s.label()).collect();
                combo(
                    dlg,
                    E_SAMESITE,
                    &labels,
                    SameSite::ALL
                        .iter()
                        .position(|s| *s == f.same_site)
                        .unwrap_or(1),
                );
                date_enable(dlg);
                if st.existing {
                    // 値を直すことが多いので値の欄へ
                    if let Ok(h) = GetDlgItem(Some(dlg), E_VALUE as i32) {
                        let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(h));
                    }
                    return 0;
                }
                1
            }
            WM_COMMAND => {
                let st = &mut *(GetWindowLongPtrW(dlg, GWLP_USERDATA) as *mut EditState);
                let id = (wparam.0 & 0xffff) as u16;
                let code = ((wparam.0 >> 16) & 0xffff) as u32;
                match id {
                    E_EXPIRY if code == CBN_SELCHANGE => date_enable(dlg),
                    IDOK_ => {
                        let expires = if combo_sel(dlg, E_EXPIRY) == 1 {
                            let text = get_text(dlg, E_DATE);
                            match parse_datetime(&text).and_then(|d| local_to_unix(&d)) {
                                Some(t) => Some(t),
                                None => {
                                    crate::util::error_box(
                                        dlg,
                                        &format!(
                                            "期限「{text}」が読めません（例: 2026-12-31 23:59）"
                                        ),
                                    );
                                    return 1;
                                }
                            }
                        } else {
                            None
                        };
                        let f = CookieFields {
                            name: get_text(dlg, E_NAME).trim().to_owned(),
                            value: get_text(dlg, E_VALUE).replace(['\r', '\n'], ""),
                            domain: get_text(dlg, E_DOMAIN).trim().to_owned(),
                            path: get_text(dlg, E_PATH).trim().to_owned(),
                            expires,
                            http_only: checked(dlg, E_HTTPONLY),
                            secure: checked(dlg, E_SECURE),
                            same_site: SameSite::ALL[combo_sel(dlg, E_SAMESITE).min(2)],
                        };
                        match f.validate() {
                            Ok(()) => {
                                st.result = Some(f);
                                let _ = EndDialog(dlg, IDOK_ as isize);
                            }
                            Err(e) => crate::util::error_box(dlg, &e),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_local_times_both_ways() {
        let d = parse_datetime("2026-10-09 14:23").unwrap();
        let t = local_to_unix(&d).unwrap();
        assert_eq!(unix_to_local(t), "2026-10-09 14:23");
    }
}

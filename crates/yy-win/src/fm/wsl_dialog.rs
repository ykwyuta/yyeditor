//! Native Linux properties dialog; WSL work never blocks its message loop.
use super::*;
use crate::goto::{CLASS_BUTTON, CLASS_EDIT, CLASS_STATIC, Template};
use std::{sync::mpsc, thread::JoinHandle};
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::{Controls::*, Input::KeyboardAndMouse::EnableWindow, WindowsAndMessaging::*},
};
use windows::core::HSTRING;
use yy_remote::{PasswordStore, SavedPassword};

const OWNER: i32 = 101;
const GROUP: i32 = 102;
const MODE: i32 = 103;
const SUDO: i32 = 104;
const PASSWORD: i32 = 105;
const INFO: i32 = 106;
const FORGET: i32 = 107;
const RELOAD: i32 = 108;
const BITS: [u32; 12] = [
    0o400, 0o200, 0o100, 0o40, 0o20, 0o10, 0o4, 0o2, 0o1, 0o4000, 0o2000, 0o1000,
];

enum Reply {
    Read(io::Result<Info>),
    Applied(io::Result<Permissions>, Option<String>),
}
struct Dialog {
    loc: Location,
    info: Option<Info>,
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    reload: HWND,
    reload_msg: u32,
}
impl Drop for Dialog {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn template() -> Template {
    let mut t = Template::dialog("WSLのオーナー・パーミッション", 300, 253);
    t.item(0, 7, 7, 35, 12, 200, CLASS_STATIC, "対象");
    t.item(
        WS_BORDER.0 | ES_READONLY as u32 | ES_AUTOHSCROLL as u32,
        45,
        5,
        248,
        14,
        201,
        CLASS_EDIT,
        "",
    );
    for (label, id, y) in [
        ("オーナー（名前・UID）", OWNER, 28),
        ("グループ（名前・GID）", GROUP, 48),
        ("権限（8進数）", MODE, 68),
    ] {
        t.item(0, 7, y, 105, 12, (id + 400) as u16, CLASS_STATIC, label);
        t.item(
            (WS_TABSTOP | WS_BORDER).0 | ES_AUTOHSCROLL as u32,
            115,
            y - 2,
            178,
            14,
            id as u16,
            CLASS_EDIT,
            "",
        );
    }
    for (label, x) in [("読取", 115), ("書込", 175), ("実行", 235)] {
        t.item(0, x, 87, 50, 12, (x + 400) as u16, CLASS_STATIC, label);
    }
    for (r, label) in ["所有者", "グループ", "その他"].iter().enumerate() {
        let y = 103 + r as i16 * 16;
        t.item(0, 7, y, 95, 12, 220 + r as u16, CLASS_STATIC, label);
        for c in 0..3 {
            t.item(
                WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
                115 + c as i16 * 60,
                y,
                42,
                12,
                300 + (r * 3 + c) as u16,
                CLASS_BUTTON,
                "",
            );
        }
    }
    for (i, label) in ["setuid", "setgid", "sticky"].iter().enumerate() {
        t.item(
            WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
            7 + i as i16 * 98,
            153,
            95,
            12,
            309 + i as u16,
            CLASS_BUTTON,
            label,
        );
    }
    t.item(
        WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
        7,
        171,
        286,
        12,
        SUDO as u16,
        CLASS_BUTTON,
        "sudoで変更（パスワードはWindows資格情報に保存）",
    );
    t.item(0, 7, 191, 105, 12, 230, CLASS_STATIC, "sudoパスワード");
    t.item(
        (WS_TABSTOP | WS_BORDER).0 | ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32,
        115,
        189,
        178,
        14,
        PASSWORD as u16,
        CLASS_EDIT,
        "",
    );
    t.item(
        (WS_TABSTOP | WS_BORDER | WS_VSCROLL).0 | ES_MULTILINE as u32 | ES_READONLY as u32,
        7,
        209,
        286,
        23,
        INFO as u16,
        CLASS_EDIT,
        "WSLの情報を読み込んでいます…",
    );
    for (label, id, x, width) in [
        ("保存済みPWを削除", FORGET, 7, 88),
        ("再読込", RELOAD, 101, 54),
        ("適用", 1, 161, 62),
        ("閉じる", 2, 229, 64),
    ] {
        t.item(
            WS_TABSTOP.0
                | if id == 1 {
                    BS_DEFPUSHBUTTON as u32
                } else {
                    BS_PUSHBUTTON as u32
                },
            x,
            234,
            width,
            14,
            id as u16,
            CLASS_BUTTON,
            label,
        );
    }
    t
}
fn text(hwnd: HWND, id: i32) -> String {
    unsafe {
        let h = GetDlgItem(Some(hwnd), id).unwrap();
        let mut buf = vec![0u16; GetWindowTextLengthW(h).max(0) as usize + 1];
        let length = GetWindowTextW(h, &mut buf);
        let value = String::from_utf16_lossy(&buf[..length as usize]);
        if id == PASSWORD {
            for unit in &mut buf {
                std::ptr::write_volatile(unit, 0);
            }
            std::sync::atomic::compiler_fence(Ordering::SeqCst);
        }
        value
    }
}
fn set(hwnd: HWND, id: i32, value: &str) {
    unsafe {
        let _ = SetDlgItemTextW(
            hwnd,
            id,
            &HSTRING::from(value.replace("\r\n", "\n").replace('\n', "\r\n")),
        );
    }
}
fn busy(hwnd: HWND, on: bool, ready: bool) {
    unsafe {
        for id in [OWNER, GROUP, MODE, SUDO, PASSWORD, FORGET, RELOAD, 1]
            .into_iter()
            .chain(300..312)
        {
            if let Ok(h) = GetDlgItem(Some(hwnd), id) {
                let _ = EnableWindow(h, !on && (ready || matches!(id, FORGET | RELOAD)));
            }
        }
    }
}
fn update_bits(hwnd: HWND, value: u32) {
    unsafe {
        for (i, bit) in BITS.iter().enumerate() {
            let _ = CheckDlgButton(
                hwnd,
                300 + i as i32,
                if value & bit != 0 {
                    BST_CHECKED
                } else {
                    BST_UNCHECKED
                },
            );
        }
    }
}
fn start_read(hwnd: HWND, s: &mut Dialog) {
    if s.thread.is_some() {
        return;
    }
    busy(hwnd, true, false);
    set(hwnd, INFO, "WSLの情報を読み込んでいます…");
    let (loc, tx, cancel) = (s.loc.clone(), s.tx.clone(), s.cancel.clone());
    s.thread = Some(std::thread::spawn(move || {
        let _ = tx.send(Reply::Read(read(&loc, &cancel)));
    }));
}
fn show_info(hwnd: HWND, s: &Dialog) {
    if let Some(info) = &s.info {
        let p = &info.permissions;
        set(hwnd, OWNER, &p.uid.to_string());
        set(hwnd, GROUP, &p.gid.to_string());
        set(hwnd, MODE, &format!("{:04o}", p.mode));
        update_bits(hwnd, p.mode);
        let saved = crate::credstore::WindowsCredentials
            .load(&s.loc.credential_key(&info.user))
            .map(|saved| {
                drop(Secret::new(saved.password));
                true
            })
            .unwrap_or(false);
        set(
            hwnd,
            INFO,
            &format!(
                "{} ({}) : {} ({}) ／ {:04o}\n{}",
                p.owner,
                p.uid,
                p.group,
                p.gid,
                p.mode,
                if saved {
                    "パスワードは保存済み。空欄のまま使用できます。"
                } else {
                    "入力したsudoパスワードは適用成功後に保存します。"
                }
            ),
        );
    }
}
fn apply_clicked(hwnd: HWND, s: &mut Dialog) {
    let Some(info) = &s.info else {
        return;
    };
    if s.thread.is_some() {
        return;
    }
    let change = match Change::new(
        &text(hwnd, OWNER),
        &text(hwnd, GROUP),
        &text(hwnd, MODE),
        &info.permissions,
    ) {
        Ok(c) => c,
        Err(e) => {
            set(hwnd, INFO, &e.to_string());
            return;
        }
    };
    let sudo = unsafe { IsDlgButtonChecked(hwnd, SUDO) == BST_CHECKED.0 };
    let key = s.loc.credential_key(&info.user);
    let mut secret = None;
    if sudo {
        let entered = Secret::new(text(hwnd, PASSWORD));
        secret = if entered.0.is_empty() {
            crate::credstore::WindowsCredentials
                .load(&key)
                .filter(|saved| saved.user == info.user)
                .map(|saved| Secret::new(saved.password))
                .or(Some(entered))
        } else {
            Some(entered)
        };
    }
    set(hwnd, PASSWORD, "");
    busy(hwnd, true, true);
    set(
        hwnd,
        INFO,
        "変更を適用し、実際の所有者・権限を確認しています…",
    );
    let (loc, user, tx, cancel) = (
        s.loc.clone(),
        info.user.clone(),
        s.tx.clone(),
        s.cancel.clone(),
    );
    s.thread = Some(std::thread::spawn(move || {
        let result = apply(&loc, &user, &change, secret.as_ref(), &cancel);
        let mut warning = None;
        if result.is_ok() {
            if let Some(secret) = secret.as_ref().filter(|secret| !secret.0.is_empty()) {
                let saved = SavedPassword {
                    user,
                    password: secret.text().to_owned(),
                };
                if crate::credstore::WindowsCredentials
                    .save(&key, &saved)
                    .is_err()
                {
                    warning =
                        Some("変更済みですが、Windows資格情報への保存に失敗しました。".into());
                }
                drop(Secret::new(saved.password));
            }
        } else if sudo {
            crate::credstore::WindowsCredentials.delete(&key);
        }
        let _ = tx.send(Reply::Applied(result, warning));
    }));
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, lp.0);
            let s = &mut *(lp.0 as *mut Dialog);
            set(hwnd, 201, &format!("{}:{}", s.loc.distro, s.loc.linux));
            let _ = CheckDlgButton(hwnd, SUDO, BST_CHECKED);
            for (id, limit) in [(OWNER, 128), (GROUP, 128), (MODE, 4), (PASSWORD, 1024)] {
                if let Ok(control) = GetDlgItem(Some(hwnd), id) {
                    SendMessageW(control, EM_SETLIMITTEXT, Some(WPARAM(limit)), None);
                }
            }
            SetTimer(Some(hwnd), 1, 40, None);
            if s.info.is_some() {
                show_info(hwnd, s);
                busy(hwnd, false, true);
            } else {
                start_read(hwnd, s);
            }
            return 1;
        }
        // Text and checkbox notifications are reentrant while fields are set.
        // They never borrow the state used by the outer dialog callback.
        if msg == WM_COMMAND {
            let (id, code) = ((wp.0 & 0xffff) as i32, (wp.0 >> 16) as u32);
            if id == MODE && code == EN_CHANGE {
                if let Ok(mode) = yy_files::wsl::mode(&text(hwnd, MODE)) {
                    update_bits(hwnd, mode);
                }
                return 1;
            }
            if (300..=311).contains(&id) && code == BN_CLICKED {
                let mode: u32 = BITS
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| IsDlgButtonChecked(hwnd, 300 + *i as i32) == BST_CHECKED.0)
                    .map(|(_, bit)| *bit)
                    .sum();
                set(hwnd, MODE, &format!("{mode:04o}"));
                return 1;
            }
            if !matches!(id, 1 | 2 | FORGET | RELOAD) || code != BN_CLICKED {
                return 0;
            }
        } else if !matches!(msg, WM_TIMER | WM_CLOSE | WM_DESTROY) {
            return 0;
        }
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Dialog;
        if p.is_null() {
            return 0;
        }
        let s = &mut *p;
        match msg {
            WM_TIMER => {
                while let Ok(reply) = s.rx.try_recv() {
                    if let Some(thread) = s.thread.take() {
                        let _ = thread.join();
                    }
                    match reply {
                        Reply::Read(Ok(info)) => {
                            s.info = Some(info);
                            show_info(hwnd, s);
                        }
                        Reply::Read(Err(e)) => {
                            s.info = None;
                            set(hwnd, INFO, &e.to_string());
                        }
                        Reply::Applied(Ok(permissions), warning) => {
                            if let Some(info) = &mut s.info {
                                info.permissions = permissions;
                            }
                            show_info(hwnd, s);
                            if let Some(w) = warning {
                                set(hwnd, INFO, &w);
                            } else {
                                set(hwnd, INFO, "変更と実際の権限確認が完了しました。");
                            }
                            let _ =
                                PostMessageW(Some(s.reload), s.reload_msg, WPARAM(0), LPARAM(0));
                        }
                        Reply::Applied(Err(e), _) => {
                            set(
                                hwnd,
                                INFO,
                                &format!(
                                    "{e}\n必要ならパスワードを入れ直して、再読込してください。"
                                ),
                            );
                        }
                    }
                    busy(
                        hwnd,
                        false,
                        s.info.as_ref().is_some_and(|i| !i.permissions.symlink()),
                    );
                }
                1
            }
            WM_COMMAND => {
                let id = (wp.0 & 0xffff) as i32;
                match id {
                    1 => apply_clicked(hwnd, s),
                    2 => {
                        s.cancel.store(true, Ordering::Relaxed);
                        let _ = EndDialog(hwnd, 0);
                    }
                    FORGET => {
                        if let Some(info) = &s.info {
                            crate::credstore::WindowsCredentials
                                .delete(&s.loc.credential_key(&info.user));
                            set(hwnd, INFO, "保存済みのsudoパスワードを削除しました。");
                        }
                    }
                    RELOAD => start_read(hwnd, s),
                    _ => {}
                }
                1
            }
            WM_CLOSE => {
                s.cancel.store(true, Ordering::Relaxed);
                let _ = EndDialog(hwnd, 0);
                1
            }
            WM_DESTROY => {
                let _ = KillTimer(Some(hwnd), 1);
                0
            }
            _ => 0,
        }
    }
}
pub(crate) fn open(owner: HWND, path: &Path, reload: HWND, reload_msg: u32) {
    let Some(loc) = location(path) else {
        return;
    };
    let (tx, rx) = mpsc::channel();
    let mut state = Dialog {
        loc,
        info: None,
        tx,
        rx,
        cancel: Arc::new(AtomicBool::new(false)),
        thread: None,
        reload,
        reload_msg,
    };
    let t = template().aligned();
    unsafe {
        let _ = DialogBoxIndirectParamW(
            None,
            t.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            Some(proc),
            LPARAM(&mut state as *mut Dialog as isize),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::{Foundation::RECT, Graphics::Gdi::*};
    #[test]
    fn permission_controls_fit_and_checkbox_and_octal_edits_agree() {
        let (tx, rx) = mpsc::channel();
        let mut state = Dialog {
            loc: Location {
                distro: "yy-test-layout".into(),
                linux: "/file".into(),
            },
            info: Some(Info {
                user: "fixture".into(),
                permissions: Permissions {
                    uid: 1000,
                    gid: 1000,
                    mode: 0o640,
                    owner: "fixture".into(),
                    group: "fixture".into(),
                    kind: "regular file".into(),
                    fingerprint: "1:1".into(),
                },
            }),
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            thread: None,
            reload: HWND::default(),
            reload_msg: 0,
        };
        let template = template().aligned();
        unsafe {
            let hwnd = CreateDialogIndirectParamW(
                None,
                template.as_ptr().cast(),
                None,
                Some(proc),
                LPARAM(&mut state as *mut Dialog as isize),
            )
            .unwrap();
            struct Window(HWND);
            impl Drop for Window {
                fn drop(&mut self) {
                    unsafe {
                        let _ = DestroyWindow(self.0);
                    }
                }
            }
            let _window = Window(hwnd);
            let mut client = RECT::default();
            GetClientRect(hwnd, &mut client).unwrap();
            assert!(
                client.right <= 640 && client.bottom <= 520,
                "dialog must fit a small display: {client:?}"
            );
            for id in [
                200, 230, 501, 502, 503, OWNER, GROUP, MODE, SUDO, PASSWORD, INFO, FORGET, RELOAD,
                1, 2,
            ]
            .into_iter()
            .chain(300..312)
            {
                let Ok(control) = GetDlgItem(Some(hwnd), id) else {
                    continue;
                };
                let mut rect = RECT::default();
                GetWindowRect(control, &mut rect).unwrap();
                let mut top = windows::Win32::Foundation::POINT {
                    x: rect.left,
                    y: rect.top,
                };
                let _ = ScreenToClient(hwnd, &mut top);
                assert!(
                    top.x >= 0
                        && top.y >= 0
                        && top.x + rect.right - rect.left <= client.right
                        && top.y + rect.bottom - rect.top <= client.bottom,
                    "control {id} does not fit"
                );
                if [200, 230, 501, 502, 503, SUDO, FORGET, RELOAD, 1, 2].contains(&id) {
                    let caption = text(hwnd, id);
                    let dc = GetDC(Some(control));
                    let font = HFONT(SendMessageW(control, WM_GETFONT, None, None).0 as *mut _);
                    let old = SelectObject(dc, font.into());
                    let chars: Vec<_> = caption.encode_utf16().collect();
                    let mut extent = windows::Win32::Foundation::SIZE::default();
                    GetTextExtentPoint32W(dc, &chars, &mut extent).unwrap();
                    SelectObject(dc, old);
                    ReleaseDC(Some(control), dc);
                    let inset = if id == SUDO { 20 } else { 4 };
                    assert!(
                        extent.cx + inset <= rect.right - rect.left
                            && extent.cy <= rect.bottom - rect.top,
                        "clipped control {id}: {caption}"
                    );
                }
            }
            assert_eq!(text(hwnd, MODE), "0640");
            SendMessageW(GetDlgItem(Some(hwnd), 308).unwrap(), BM_CLICK, None, None);
            assert_eq!(text(hwnd, MODE), "0641");
            set(hwnd, MODE, "0755");
            for (i, bit) in BITS.iter().enumerate() {
                assert_eq!(
                    IsDlgButtonChecked(hwnd, 300 + i as i32) == BST_CHECKED.0,
                    0o755 & bit != 0
                );
            }
            assert!(
                GetWindowLongW(GetDlgItem(Some(hwnd), PASSWORD).unwrap(), GWL_STYLE) as u32
                    & ES_PASSWORD as u32
                    != 0
            );
        }
    }
}

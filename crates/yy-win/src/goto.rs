//! 「行へ移動」ダイアログ。
//!
//! リソースファイルを使わず、メモリ上のダイアログテンプレートから
//! `DialogBoxIndirectParamW` で表示する（モーダルループ・Enter/Esc/Tab は OS の
//! ダイアログマネージャーに任せる）。

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::EM_SETSEL;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;

const ID_EDIT: u16 = 100;
const ID_LABEL: u16 = 101;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;

/// ダイアログとの間で受け渡す値。
struct State {
    prompt: String,
    text: String,
}

/// DLGTEMPLATE / DLGITEMTEMPLATE をバイト列として組み立てる。
struct Template(Vec<u16>);

impl Template {
    fn dword(&mut self, v: u32) {
        self.0.push(v as u16);
        self.0.push((v >> 16) as u16);
    }

    fn string(&mut self, s: &str) {
        self.0.extend(s.encode_utf16());
        self.0.push(0);
    }

    fn align_dword(&mut self) {
        if self.0.len() % 2 == 1 {
            self.0.push(0);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn item(
        &mut self,
        style: u32,
        x: i16,
        y: i16,
        cx: i16,
        cy: i16,
        id: u16,
        class: u16,
        text: &str,
    ) {
        self.align_dword();
        self.dword(style | (WS_CHILD | WS_VISIBLE).0);
        self.dword(0);
        for v in [x, y, cx, cy] {
            self.0.push(v as u16);
        }
        self.0.push(id);
        self.0.push(0xFFFF);
        self.0.push(class);
        self.string(text);
        self.0.push(0);
    }
}

const CLASS_BUTTON: u16 = 0x0080;
const CLASS_EDIT: u16 = 0x0081;
const CLASS_STATIC: u16 = 0x0082;

fn build_template() -> Template {
    let mut t = Template(Vec::with_capacity(256));
    let style = WS_POPUP.0
        | WS_CAPTION.0
        | WS_SYSMENU.0
        | DS_MODALFRAME as u32
        | DS_SETFONT as u32
        | DS_CENTER as u32;
    t.dword(style);
    t.dword(0);
    t.0.push(4); // コントロール数
    for v in [0i16, 0, 200, 64] {
        t.0.push(v as u16);
    }
    t.0.push(0); // メニューなし
    t.0.push(0); // 既定のクラス
    t.string("行へ移動");
    t.0.push(9); // フォントサイズ
    t.string("MS Shell Dlg");
    t.item(0, 7, 7, 186, 10, ID_LABEL, CLASS_STATIC, "");
    t.item(
        (WS_BORDER | WS_TABSTOP).0 | (ES_NUMBER | ES_AUTOHSCROLL) as u32,
        7,
        20,
        186,
        13,
        ID_EDIT,
        CLASS_EDIT,
        "",
    );
    t.item(
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        89,
        42,
        50,
        14,
        IDOK_,
        CLASS_BUTTON,
        "OK",
    );
    t.item(
        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
        143,
        42,
        50,
        14,
        IDCANCEL_,
        CLASS_BUTTON,
        "キャンセル",
    );
    t
}

/// 行番号（1 始まり）の入力を求める。キャンセルされたら `None`。
pub(crate) fn prompt_line(owner: HWND, prompt: &str, initial: u64) -> Option<u64> {
    // テンプレートは DWORD 境界に置く必要があるため u32 の領域にコピーする
    let words = build_template().0;
    let mut aligned = vec![0u32; words.len().div_ceil(2)];
    for (i, w) in words.iter().enumerate() {
        aligned[i / 2] |= (*w as u32) << ((i % 2) * 16);
    }
    let mut state = State {
        prompt: prompt.to_owned(),
        text: initial.to_string(),
    };
    let r = unsafe {
        DialogBoxIndirectParamW(
            None,
            aligned.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            Some(dialog_proc),
            LPARAM(&mut state as *mut State as isize),
        )
    };
    if r != IDOK_ as isize {
        return None;
    }
    state.text.trim().parse::<u64>().ok().filter(|n| *n >= 1)
}

extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let state = &*(lparam.0 as *const State);
                let _ =
                    SetDlgItemTextW(hwnd, ID_LABEL as i32, &HSTRING::from(state.prompt.as_str()));
                let _ = SetDlgItemTextW(hwnd, ID_EDIT as i32, &HSTRING::from(state.text.as_str()));
                if let Ok(edit) = GetDlgItem(Some(hwnd), ID_EDIT as i32) {
                    SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                }
                // 既定のフォーカス（最初の WS_TABSTOP = 入力欄）に任せる
                1
            }
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                if id == IDOK_ {
                    let state = &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State);
                    let mut buf = [0u16; 64];
                    let n = GetDlgItemTextW(hwnd, ID_EDIT as i32, &mut buf) as usize;
                    state.text = String::from_utf16_lossy(&buf[..n]);
                    let _ = EndDialog(hwnd, IDOK_ as isize);
                    1
                } else if id == IDCANCEL_ {
                    let _ = EndDialog(hwnd, IDCANCEL_ as isize);
                    1
                } else {
                    0
                }
            }
            _ => 0,
        }
    }
}

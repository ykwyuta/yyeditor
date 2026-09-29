//! 検索・置換バー（ビューの上に表示するインラインのバー。05 章）。
//!
//! バーの中の操作は WM_COMMAND としてフレームウィンドウに送り、処理は app.rs で行う
//! （アプリ状態を借用している間にコントロールから通知が来ても再入しないように、すべて Post する）。

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{DeleteObject, HFONT};
use windows::Win32::UI::Controls::EM_SETSEL;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, Result, w};
use yy_core::Query;

use crate::FINDBAR_CLASS;
use crate::util::Context;

// フレームに送るコマンド
pub(crate) const ID_FIND_NEXT_BTN: u16 = 520;
pub(crate) const ID_FIND_PREV_BTN: u16 = 521;
pub(crate) const ID_FIND_CLOSE_BTN: u16 = 522;
pub(crate) const ID_REPLACE_BTN: u16 = 523;
pub(crate) const ID_REPLACE_ALL_BTN: u16 = 524;
pub(crate) const ID_CASE: u16 = 525;
pub(crate) const ID_WORD: u16 = 526;
pub(crate) const ID_REGEX: u16 = 527;
pub(crate) const ID_IN_SELECTION: u16 = 528;
pub(crate) const ID_PATTERN: u16 = 529;
pub(crate) const ID_REPLACEMENT: u16 = 530;
pub(crate) const ID_TOGGLE_REPLACE: u16 = 531;
pub(crate) const ID_SELECT_MATCHES: u16 = 532;
pub(crate) const ID_GREP_BTN: u16 = 533;

/// 1 行の高さ・余白などの基準（96 DPI でのピクセル）
const ROW: i32 = 26;
const PAD: i32 = 4;

pub(crate) struct FindBar {
    pub hwnd: HWND,
    pattern: HWND,
    replacement: HWND,
    label_find: HWND,
    label_replace: HWND,
    prev: HWND,
    next: HWND,
    case: HWND,
    word: HWND,
    regex: HWND,
    in_selection: HWND,
    close: HWND,
    replace: HWND,
    replace_all: HWND,
    /// 下の行: メニューの中にある検索の機能のボタン
    toggle_replace: HWND,
    select_matches: HWND,
    grep: HWND,
    font: HFONT,
    dpi: u32,
    pub visible: bool,
    pub replace_mode: bool,
}

fn scale(v: i32, dpi: u32) -> i32 {
    v * dpi as i32 / 96
}

fn create_font(dpi: u32) -> HFONT {
    crate::util::ui_font(dpi)
}

impl FindBar {
    pub(crate) fn create(parent: HWND, dpi: u32) -> Result<FindBar> {
        unsafe {
            let hinstance = GetWindowLongPtrW(parent, GWLP_HINSTANCE);
            let hinstance = Some(windows::Win32::Foundation::HINSTANCE(hinstance as *mut _));
            let hwnd = CreateWindowExW(
                WS_EX_CONTROLPARENT,
                FINDBAR_CLASS,
                None,
                WS_CHILD | WS_CLIPCHILDREN,
                0,
                0,
                0,
                0,
                Some(parent),
                None,
                hinstance,
                None,
            )
            .context("CreateWindowExW(findbar)")?;
            let child = |class: PCWSTR, text: PCWSTR, style: WINDOW_STYLE, ex, id: u16| {
                CreateWindowExW(
                    ex,
                    class,
                    text,
                    WS_CHILD | WS_VISIBLE | style,
                    0,
                    0,
                    0,
                    0,
                    Some(hwnd),
                    Some(HMENU(id as usize as *mut _)),
                    hinstance,
                    None,
                )
            };
            let edit = WINDOW_STYLE((ES_AUTOHSCROLL as u32) | WS_TABSTOP.0);
            let button = WINDOW_STYLE(BS_PUSHBUTTON as u32 | WS_TABSTOP.0);
            let check = WINDOW_STYLE(BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0);
            let none = WINDOW_EX_STYLE(0);
            let bar = FindBar {
                hwnd,
                label_find: child(w!("STATIC"), w!("検索:"), WINDOW_STYLE(0), none, 0)?,
                pattern: child(w!("EDIT"), w!(""), edit, WS_EX_CLIENTEDGE, ID_PATTERN)?,
                prev: child(w!("BUTTON"), w!("↑ 前"), button, none, ID_FIND_PREV_BTN)?,
                next: child(w!("BUTTON"), w!("↓ 次"), button, none, ID_FIND_NEXT_BTN)?,
                case: child(w!("BUTTON"), w!("大/小文字"), check, none, ID_CASE)?,
                word: child(w!("BUTTON"), w!("単語"), check, none, ID_WORD)?,
                regex: child(w!("BUTTON"), w!("正規表現"), check, none, ID_REGEX)?,
                in_selection: child(
                    w!("BUTTON"),
                    w!("選択範囲のみ置換"),
                    check,
                    none,
                    ID_IN_SELECTION,
                )?,
                close: child(w!("BUTTON"), w!("×"), button, none, ID_FIND_CLOSE_BTN)?,
                label_replace: child(w!("STATIC"), w!("置換:"), WINDOW_STYLE(0), none, 0)?,
                replacement: child(w!("EDIT"), w!(""), edit, WS_EX_CLIENTEDGE, ID_REPLACEMENT)?,
                replace: child(w!("BUTTON"), w!("置換"), button, none, ID_REPLACE_BTN)?,
                replace_all: child(
                    w!("BUTTON"),
                    w!("すべて置換"),
                    button,
                    none,
                    ID_REPLACE_ALL_BTN,
                )?,
                toggle_replace: child(
                    w!("BUTTON"),
                    w!("置換を表示 ▼"),
                    button,
                    none,
                    ID_TOGGLE_REPLACE,
                )?,
                select_matches: child(
                    w!("BUTTON"),
                    w!("一致箇所をすべて選択"),
                    button,
                    none,
                    ID_SELECT_MATCHES,
                )?,
                grep: child(
                    w!("BUTTON"),
                    w!("フォルダ内を検索 (Grep)..."),
                    button,
                    none,
                    ID_GREP_BTN,
                )?,
                font: create_font(dpi),
                dpi,
                visible: false,
                replace_mode: false,
            };
            bar.apply_font();
            Ok(bar)
        }
    }

    fn controls(&self) -> [HWND; 16] {
        [
            self.label_find,
            self.pattern,
            self.prev,
            self.next,
            self.case,
            self.word,
            self.regex,
            self.in_selection,
            self.close,
            self.label_replace,
            self.replacement,
            self.replace,
            self.replace_all,
            self.toggle_replace,
            self.select_matches,
            self.grep,
        ]
    }

    fn apply_font(&self) {
        for c in self.controls() {
            unsafe {
                SendMessageW(
                    c,
                    WM_SETFONT,
                    Some(WPARAM(self.font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
        }
    }

    pub(crate) fn set_dpi(&mut self, dpi: u32) {
        if dpi == self.dpi {
            return;
        }
        let old = self.font;
        self.dpi = dpi;
        self.font = create_font(dpi);
        self.apply_font();
        unsafe {
            let _ = DeleteObject(old.into());
        }
    }

    /// バーの高さ（表示していなければ 0）。
    pub(crate) fn height(&self) -> i32 {
        if !self.visible {
            return 0;
        }
        // 検索の行、置換の行（置換のときだけ）、ボタンの行
        let rows = if self.replace_mode { 3 } else { 2 };
        scale(ROW * rows + PAD * 2, self.dpi)
    }

    /// 幅 `width` に合わせてコントロールを並べる。
    pub(crate) fn layout(&self, width: i32) {
        let s = |v: i32| scale(v, self.dpi);
        let h = self.height();
        unsafe {
            let _ = MoveWindow(self.hwnd, 0, 0, width, h, true);
            let row_h = s(ROW) - s(4);
            let y1 = s(PAD);
            let y2 = s(PAD + ROW);
            let label_w = s(40);
            // 右側のボタン群の幅
            let right = s(64) * 2 + s(90) + s(56) + s(80) + s(130) + s(28) + s(8) * 7;
            let edit_w = (width - label_w - right - s(PAD) * 2).max(s(120));
            let mut x = s(PAD);
            let place = |c: HWND, x: &mut i32, y: i32, w: i32| {
                let _ = MoveWindow(c, *x, y, w, row_h, true);
                *x += w + s(8);
            };
            place(self.label_find, &mut x, y1 + s(4), label_w - s(8));
            place(self.pattern, &mut x, y1, edit_w);
            let buttons_x = x;
            place(self.prev, &mut x, y1, s(64));
            place(self.next, &mut x, y1, s(64));
            place(self.case, &mut x, y1, s(90));
            place(self.word, &mut x, y1, s(56));
            place(self.regex, &mut x, y1, s(80));
            place(self.in_selection, &mut x, y1, s(130));
            place(self.close, &mut x, y1, s(28));
            let mut x = s(PAD);
            place(self.label_replace, &mut x, y2 + s(4), label_w - s(8));
            place(self.replacement, &mut x, y2, edit_w);
            let mut x = buttons_x;
            place(self.replace, &mut x, y2, s(64));
            place(self.replace_all, &mut x, y2, s(90));
            // ボタンの行（検索欄の下にそろえる）
            let y3 = if self.replace_mode {
                s(PAD + ROW * 2)
            } else {
                y2
            };
            let mut x = s(PAD) + label_w;
            place(self.toggle_replace, &mut x, y3, s(110));
            place(self.select_matches, &mut x, y3, s(150));
            place(self.grep, &mut x, y3, s(180));
            let label = if self.replace_mode {
                w!("置換を隠す ▲")
            } else {
                w!("置換を表示 ▼")
            };
            let _ = SetWindowTextW(self.toggle_replace, label);
            let show = if self.replace_mode { SW_SHOW } else { SW_HIDE };
            for c in [
                self.label_replace,
                self.replacement,
                self.replace,
                self.replace_all,
                self.in_selection,
            ] {
                let _ = ShowWindow(c, show);
            }
        }
    }

    /// バーを表示して検索欄にフォーカスを移す。`initial` があれば検索欄に入れる。
    pub(crate) fn show(&mut self, replace_mode: bool, initial: Option<&str>) {
        self.visible = true;
        self.replace_mode = replace_mode;
        unsafe {
            if let Some(t) = initial {
                let _ = SetWindowTextW(self.pattern, &HSTRING::from(t));
            }
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetFocus(Some(self.pattern));
            SendMessageW(self.pattern, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
    }

    /// 置換の行の表示を切り替える。表示したら置換欄にフォーカスを移す。
    pub(crate) fn toggle_replace(&mut self) {
        self.replace_mode = !self.replace_mode;
        unsafe {
            let target = if self.replace_mode {
                self.replacement
            } else {
                self.pattern
            };
            let _ = SetFocus(Some(target));
        }
    }

    pub(crate) fn hide(&mut self) {
        self.visible = false;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn text(hwnd: HWND) -> String {
        unsafe {
            let len = GetWindowTextLengthW(hwnd).max(0) as usize;
            let mut buf = vec![0u16; len + 1];
            let n = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
            String::from_utf16_lossy(&buf[..n])
        }
    }

    fn checked(hwnd: HWND) -> bool {
        unsafe { SendMessageW(hwnd, BM_GETCHECK, None, None).0 == 1 }
    }

    pub(crate) fn query(&self) -> Query {
        Query {
            pattern: FindBar::text(self.pattern),
            regex: FindBar::checked(self.regex),
            case_sensitive: FindBar::checked(self.case),
            whole_word: FindBar::checked(self.word),
        }
    }

    pub(crate) fn replacement_text(&self) -> String {
        FindBar::text(self.replacement)
    }

    pub(crate) fn selection_only(&self) -> bool {
        self.replace_mode && FindBar::checked(self.in_selection)
    }

    /// フォーカスがバーの中にあるか。
    pub(crate) fn has_focus(&self) -> bool {
        if !self.visible {
            return false;
        }
        unsafe {
            let f = GetFocus();
            !f.is_invalid() && (f == self.hwnd || IsChild(self.hwnd, f).as_bool())
        }
    }

    /// フォーカスが置換欄にあるか。
    pub(crate) fn replacement_focused(&self) -> bool {
        unsafe { GetFocus() == self.replacement }
    }
}

impl Drop for FindBar {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.font.into());
        }
    }
}

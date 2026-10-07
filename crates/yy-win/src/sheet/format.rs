//! セルの書式（15 章 11）: 表示形式・塗りつぶし・文字の色・太字・斜体・配置・罫線。
//!
//! 書式は選んだ範囲（列全体・行全体はそのまま。絞り込み中は見えている行の続きごと）に、書式の層として
//! 付ける（Undo できる）。

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::Dialogs::{
    CC_ANYCOLOR, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW,
};
use windows::Win32::UI::Controls::{
    BST_CHECKED, BST_UNCHECKED, CheckDlgButton, IsDlgButtonChecked,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;
use yy_numfmt::{DateSystem, FmtValue};
use yy_sheet::style::{
    BorderPreset, HAlign, Line, LineStyle, NO_COLOR, Rect, Rgb, Style, border_layers,
};

use super::*;
use crate::goto::{CLASS_BUTTON, CLASS_STATIC, Template};

const CLASS_COMBOBOX: u16 = 0x0085;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;
/// 入力もできるコンボボックス
const CBS_DROPDOWN_: u32 = 0x2;
const CBN_EDITCHANGE_: u32 = 5;

/// 絞り込み中に、見えている行から作る範囲の数の上限。
const RUN_LIMIT: usize = 100_000;

/// 表示形式の候補。
pub(super) const PRESETS: [(&str, &str); 17] = [
    ("General", "標準"),
    ("0", "数値"),
    ("#,##0", "桁区切り"),
    ("#,##0.00", "桁区切り（小数 2 桁）"),
    ("0%", "パーセント"),
    ("0.00%", "パーセント（小数 2 桁）"),
    ("\"¥\"#,##0", "通貨"),
    ("0.00E+00", "指数"),
    ("yyyy/m/d", "日付"),
    ("yyyy年m月d日", "日付（年月日）"),
    ("ggge年m月d日", "和暦"),
    ("m/d", "月/日"),
    ("h:mm", "時刻"),
    ("h:mm:ss", "時刻（秒）"),
    ("yyyy/m/d h:mm", "日付と時刻"),
    ("[h]:mm:ss", "経過時間"),
    ("@", "文字列"),
];

fn colorref(c: Rgb) -> COLORREF {
    COLORREF(((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF))
}

fn from_colorref(c: COLORREF) -> Rgb {
    let v = c.0;
    ((v & 0xFF) << 16) | (v & 0xFF00) | ((v >> 16) & 0xFF)
}

thread_local! {
    /// 色の選択のダイアログの「作成した色」
    static CUSTOM: std::cell::RefCell<[COLORREF; 16]> =
        const { std::cell::RefCell::new([COLORREF(0x00FF_FFFF); 16]) };
}

/// 色を選ぶ（キャンセルなら `None`）。
pub(super) fn pick_color(owner: HWND, initial: Rgb) -> Option<Rgb> {
    CUSTOM.with(|custom| {
        let mut custom = custom.borrow_mut();
        let mut cc = CHOOSECOLORW {
            lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: owner,
            rgbResult: colorref(if initial == NO_COLOR { 0 } else { initial }),
            lpCustColors: custom.as_mut_ptr(),
            Flags: CC_RGBINIT | CC_FULLOPEN | CC_ANYCOLOR,
            ..Default::default()
        };
        unsafe { ChooseColorW(&mut cc) }
            .as_bool()
            .then(|| from_colorref(cc.rgbResult))
    })
}

// ---- 選択範囲 ----------------------------------------------------------------------

/// 選択範囲を書式の範囲（絞り込みをしないときの行）にする。
fn selection_rects(a: &App) -> std::result::Result<Vec<Rect>, String> {
    let (t, l, b, r) = a.selection();
    let s = a.sheet();
    // 列全体・行全体を選んだなら、これから増える行・列にも効くように
    let (whole_rows, whole_cols) = a.whole;
    let right = if whole_cols { u32::MAX } else { r };
    if whole_rows {
        return Ok(vec![Rect::new(0, l, u64::MAX, right)]);
    }
    if s.view.rows.is_none() {
        return Ok(vec![Rect::new(t, l, b, right)]);
    }
    // 絞り込み・並べ替え中: 見えている行の、元の行で続いている部分ごと
    let mut out: Vec<Rect> = Vec::new();
    for row in t..=b {
        let src = s.source_row(row);
        match out.last_mut() {
            Some(last) if last.bottom + 1 == src => last.bottom = src,
            _ => {
                if out.len() >= RUN_LIMIT {
                    return Err(format!(
                        "絞り込み・並べ替えの表示中に書式を付けられる範囲は {} か所までです。\
                         範囲を狭めるか、表示を解除してから付けてください。",
                        crate::util::group_digits(RUN_LIMIT as u64)
                    ));
                }
                out.push(Rect::new(src, l, src, right));
            }
        }
    }
    Ok(out)
}

/// 選択範囲に書式の層を付ける（`layers` は範囲ごとに付ける層を返す）。
fn apply(layers: impl Fn(Rect) -> Vec<(Rect, Style)>) {
    with(|a| {
        a.end_edit(true);
        let rects = match selection_rects(a) {
            Ok(r) => r,
            Err(e) => {
                info_box(a.frame, &e);
                return;
            }
        };
        let sheet = a.sheet;
        let _ = a.doc.edit(|b, _| {
            let st = &mut b.sheets[sheet].styles;
            for r in rects {
                for (rect, style) in layers(r) {
                    st.set(rect, style);
                }
            }
            Ok(())
        });
        a.after_edit();
    });
}

/// 選択範囲に同じ書式を付ける。
pub(super) fn apply_style(style: Style) {
    apply(|r| vec![(r, style.clone())]);
}

/// 選択範囲に罫線を付ける。
pub(super) fn apply_border(preset: BorderPreset, line: Line) {
    apply(|r| border_layers(r, preset, line));
}

/// 選択範囲の書式を既定に戻す。
pub(super) fn clear_format() {
    with(|a| {
        a.end_edit(true);
        let rects = match selection_rects(a) {
            Ok(r) => r,
            Err(e) => {
                info_box(a.frame, &e);
                return;
            }
        };
        let sheet = a.sheet;
        let _ = a.doc.edit(|b, _| {
            for r in rects {
                b.sheets[sheet].styles.clear(r);
            }
            Ok(())
        });
        a.after_edit();
    });
}

/// アクティブなセルの（フレーム, 書式, 表示形式, 値, 日付の基準）。
type Active = (HWND, Style, Option<Arc<str>>, Value, DateSystem);

fn active_style() -> Option<Active> {
    with(|a| {
        let (r, c) = a.cur;
        let s = a.sheet();
        (
            a.frame,
            s.style_at(r, c),
            s.format_at(r, c),
            s.get(&a.ctx, r, c).unwrap_or_default(),
            a.sys(),
        )
    })
}

/// 太字・斜体を切り替える（アクティブなセルの逆にする）。
pub(super) fn toggle(bold: bool) {
    let Some((_, st, ..)) = active_style() else {
        return;
    };
    let style = if bold {
        Style {
            bold: Some(!st.bold.unwrap_or(false)),
            ..Style::default()
        }
    } else {
        Style {
            italic: Some(!st.italic.unwrap_or(false)),
            ..Style::default()
        }
    };
    apply_style(style);
}

/// 塗りつぶし・文字の色を選んで付ける。
pub(super) fn choose_color(fill: bool) {
    let Some((frame, st, ..)) = active_style() else {
        return;
    };
    let initial = if fill {
        st.fill.unwrap_or(0xFFFF00)
    } else {
        st.color.unwrap_or(0xC00000)
    };
    if let Some(c) = pick_color(frame, initial) {
        apply_style(if fill {
            Style {
                fill: Some(c),
                ..Style::default()
            }
        } else {
            Style {
                color: Some(c),
                ..Style::default()
            }
        });
    }
}

/// 表示形式を付ける。
pub(super) fn set_number_format(code: &str) {
    apply_style(Style {
        num_fmt: Some(Arc::from(code)),
        ..Style::default()
    });
}

// ---- セルの書式設定のダイアログ ----------------------------------------------------------

const D_FMT: u16 = 10;
const D_PREVIEW: u16 = 11;
const D_BOLD: u16 = 12;
const D_ITALIC: u16 = 13;
const D_ALIGN: u16 = 14;
const D_FILL: u16 = 15;
const D_FILL_NONE: u16 = 16;
const D_FILL_LABEL: u16 = 17;
const D_COLOR: u16 = 18;
const D_COLOR_AUTO: u16 = 19;
const D_COLOR_LABEL: u16 = 20;
const D_BORDER: u16 = 21;
const D_LINE: u16 = 22;
const D_LINE_COLOR: u16 = 23;
const D_LINE_LABEL: u16 = 24;

const ALIGNS: [(Option<HAlign>, &str); 5] = [
    (None, "変更しない"),
    (Some(HAlign::General), "標準"),
    (Some(HAlign::Left), "左"),
    (Some(HAlign::Center), "中央"),
    (Some(HAlign::Right), "右"),
];

const BORDERS: [(Option<BorderPreset>, &str); 8] = [
    (None, "変更しない"),
    (Some(BorderPreset::None), "罫線なし"),
    (Some(BorderPreset::All), "格子"),
    (Some(BorderPreset::Outline), "外枠"),
    (Some(BorderPreset::Top), "上"),
    (Some(BorderPreset::Bottom), "下"),
    (Some(BorderPreset::Left), "左"),
    (Some(BorderPreset::Right), "右"),
];

const LINES: [(LineStyle, &str); 6] = [
    (LineStyle::Thin, "細線"),
    (LineStyle::Medium, "中線"),
    (LineStyle::Thick, "太線"),
    (LineStyle::Dotted, "点線"),
    (LineStyle::Dashed, "破線"),
    (LineStyle::Double, "二重線"),
];

/// 色の選び方。
#[derive(Clone, Copy, PartialEq)]
enum Pick {
    Keep,
    Clear,
    Rgb(Rgb),
}

impl Pick {
    fn label(self, clear: &str) -> String {
        match self {
            Pick::Keep => "変更しない".into(),
            Pick::Clear => clear.into(),
            Pick::Rgb(c) => format!("#{c:06X}"),
        }
    }

    fn value(self) -> Option<Rgb> {
        match self {
            Pick::Keep => None,
            Pick::Clear => Some(NO_COLOR),
            Pick::Rgb(c) => Some(c),
        }
    }
}

struct FormatState {
    initial_fmt: String,
    initial_bold: bool,
    initial_italic: bool,
    sample: Value,
    sys: DateSystem,
    fill: Pick,
    color: Pick,
    line_color: Rgb,
    current_fill: Rgb,
    current_color: Rgb,
    /// 結果（書式と罫線）
    result: Option<(Style, Option<(BorderPreset, Line)>)>,
}

/// セルの書式設定（Ctrl+1）。
pub(super) fn format_dialog() {
    let Some((frame, st, fmt, value, sys)) = active_style() else {
        return;
    };
    let mut t = Template::dialog("セルの書式設定", 260, 196);
    let label = |t: &mut Template, x, y, cx, id, text: &str| {
        t.item(0, x, y, cx, 10, id, CLASS_STATIC, text);
    };
    let button = |t: &mut Template, x, y, cx, id, text: &str| {
        t.item(
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            x,
            y,
            cx,
            14,
            id,
            CLASS_BUTTON,
            text,
        );
    };
    let combo = |t: &mut Template, x, y, cx, id, style: u32| {
        t.item(
            (WS_TABSTOP | WS_VSCROLL).0 | style,
            x,
            y,
            cx,
            160,
            id,
            CLASS_COMBOBOX,
            "",
        );
    };
    label(&mut t, 7, 9, 50, 0, "表示形式:");
    combo(
        &mut t,
        60,
        7,
        193,
        D_FMT,
        CBS_DROPDOWN_ | CBS_AUTOHSCROLL as u32,
    );
    label(&mut t, 7, 26, 50, 0, "例:");
    label(&mut t, 60, 26, 193, D_PREVIEW, "");
    t.item(
        WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
        7,
        42,
        50,
        10,
        D_BOLD,
        CLASS_BUTTON,
        "太字",
    );
    t.item(
        WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
        60,
        42,
        50,
        10,
        D_ITALIC,
        CLASS_BUTTON,
        "斜体",
    );
    label(&mut t, 7, 60, 50, 0, "横の配置:");
    combo(&mut t, 60, 58, 90, D_ALIGN, CBS_DROPDOWNLIST as u32);
    label(&mut t, 7, 80, 50, 0, "塗りつぶし:");
    button(&mut t, 60, 77, 60, D_FILL, "色を選ぶ...");
    button(&mut t, 124, 77, 40, D_FILL_NONE, "なし");
    label(&mut t, 170, 80, 83, D_FILL_LABEL, "");
    label(&mut t, 7, 98, 50, 0, "文字の色:");
    button(&mut t, 60, 95, 60, D_COLOR, "色を選ぶ...");
    button(&mut t, 124, 95, 40, D_COLOR_AUTO, "自動");
    label(&mut t, 170, 98, 83, D_COLOR_LABEL, "");
    label(&mut t, 7, 120, 50, 0, "罫線:");
    combo(&mut t, 60, 118, 90, D_BORDER, CBS_DROPDOWNLIST as u32);
    label(&mut t, 7, 138, 50, 0, "線:");
    combo(&mut t, 60, 136, 60, D_LINE, CBS_DROPDOWNLIST as u32);
    button(&mut t, 124, 135, 60, D_LINE_COLOR, "線の色...");
    label(&mut t, 190, 138, 63, D_LINE_LABEL, "");
    t.item(
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        149,
        176,
        50,
        14,
        IDOK_,
        CLASS_BUTTON,
        "OK",
    );
    button(&mut t, 203, 176, 50, IDCANCEL_, "キャンセル");

    let mut state = FormatState {
        initial_fmt: fmt.as_deref().unwrap_or("General").to_owned(),
        initial_bold: st.bold.unwrap_or(false),
        initial_italic: st.italic.unwrap_or(false),
        sample: match value {
            Value::Number(_) => value,
            _ => Value::Number(1234.5),
        },
        sys,
        fill: Pick::Keep,
        color: Pick::Keep,
        line_color: 0,
        current_fill: st.fill_rgb().unwrap_or(0xFFFF00),
        current_color: st.color_rgb().unwrap_or(0xC00000),
        result: None,
    };
    let aligned = t.aligned();
    unsafe {
        DialogBoxIndirectParamW(
            None,
            aligned.as_ptr() as *const DLGTEMPLATE,
            Some(frame),
            Some(format_proc),
            LPARAM(&mut state as *mut FormatState as isize),
        );
    }
    let Some((style, border)) = state.result else {
        return;
    };
    // 書式と罫線を 1 回の編集で（Undo 1 回で戻る）
    apply(|r| {
        let mut v = Vec::new();
        if !style.is_empty() {
            v.push((r, style.clone()));
        }
        if let Some((preset, line)) = border {
            v.extend(border_layers(r, preset, line));
        }
        v
    });
}

fn send(hwnd: HWND, id: u16, msg: u32, w: usize, l: isize) -> isize {
    unsafe { SendDlgItemMessageW(hwnd, id as i32, msg, WPARAM(w), LPARAM(l)).0 }
}

fn set_text(hwnd: HWND, id: u16, s: &str) {
    unsafe {
        let _ = SetDlgItemTextW(hwnd, id as i32, &HSTRING::from(s));
    }
}

fn get_text(hwnd: HWND, id: u16) -> String {
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetDlgItemTextW(hwnd, id as i32, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn add(hwnd: HWND, id: u16, s: &str) {
    let w = HSTRING::from(s);
    send(hwnd, id, CB_ADDSTRING, 0, w.as_ptr() as isize);
}

fn preview(hwnd: HWND, st: &FormatState, code: &str) {
    let text = match &st.sample {
        Value::Number(n) => {
            let f = yy_numfmt::format::parsed(code).format(FmtValue::Number(*n), st.sys);
            if f.overflow { "#####".into() } else { f.text }
        }
        _ => String::new(),
    };
    set_text(hwnd, D_PREVIEW, &text);
}

fn refresh_labels(hwnd: HWND, st: &FormatState) {
    set_text(hwnd, D_FILL_LABEL, &st.fill.label("なし"));
    set_text(hwnd, D_COLOR_LABEL, &st.color.label("自動"));
    set_text(hwnd, D_LINE_LABEL, &format!("#{:06X}", st.line_color));
}

extern "system" fn format_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = &*(lparam.0 as *const FormatState);
                for (code, _) in PRESETS {
                    add(hwnd, D_FMT, code);
                }
                set_text(hwnd, D_FMT, &st.initial_fmt);
                preview(hwnd, st, &st.initial_fmt);
                for (id, on) in [(D_BOLD, st.initial_bold), (D_ITALIC, st.initial_italic)] {
                    let _ = CheckDlgButton(
                        hwnd,
                        id as i32,
                        if on { BST_CHECKED } else { BST_UNCHECKED },
                    );
                }
                for (_, name) in ALIGNS {
                    add(hwnd, D_ALIGN, name);
                }
                send(hwnd, D_ALIGN, CB_SETCURSEL, 0, 0);
                for (_, name) in BORDERS {
                    add(hwnd, D_BORDER, name);
                }
                send(hwnd, D_BORDER, CB_SETCURSEL, 0, 0);
                for (_, name) in LINES {
                    add(hwnd, D_LINE, name);
                }
                send(hwnd, D_LINE, CB_SETCURSEL, 0, 0);
                refresh_labels(hwnd, st);
                1
            }
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let code = crate::hiword(wparam.0);
                let st = &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut FormatState);
                match id {
                    D_FMT if code == CBN_EDITCHANGE_ => {
                        preview(hwnd, st, &get_text(hwnd, D_FMT));
                        1
                    }
                    D_FMT if code == CBN_SELCHANGE => {
                        let i = send(hwnd, D_FMT, CB_GETCURSEL, 0, 0);
                        if let Some((c, _)) = usize::try_from(i).ok().and_then(|i| PRESETS.get(i)) {
                            preview(hwnd, st, c);
                        }
                        1
                    }
                    D_FILL | D_COLOR | D_LINE_COLOR => {
                        let initial = match id {
                            D_FILL => st.current_fill,
                            D_COLOR => st.current_color,
                            _ => st.line_color,
                        };
                        if let Some(c) = pick_color(hwnd, initial) {
                            match id {
                                D_FILL => st.fill = Pick::Rgb(c),
                                D_COLOR => st.color = Pick::Rgb(c),
                                _ => st.line_color = c,
                            }
                            refresh_labels(hwnd, st);
                        }
                        1
                    }
                    D_FILL_NONE => {
                        st.fill = Pick::Clear;
                        refresh_labels(hwnd, st);
                        1
                    }
                    D_COLOR_AUTO => {
                        st.color = Pick::Clear;
                        refresh_labels(hwnd, st);
                        1
                    }
                    IDOK_ => {
                        let mut style = Style::default();
                        let fmt = get_text(hwnd, D_FMT).trim().to_owned();
                        if !fmt.is_empty() && fmt != st.initial_fmt {
                            style.num_fmt = Some(Arc::from(fmt.as_str()));
                        }
                        let bold = IsDlgButtonChecked(hwnd, D_BOLD as i32) == BST_CHECKED.0;
                        if bold != st.initial_bold {
                            style.bold = Some(bold);
                        }
                        let italic = IsDlgButtonChecked(hwnd, D_ITALIC as i32) == BST_CHECKED.0;
                        if italic != st.initial_italic {
                            style.italic = Some(italic);
                        }
                        let at =
                            |id| usize::try_from(send(hwnd, id, CB_GETCURSEL, 0, 0)).unwrap_or(0);
                        style.align = ALIGNS.get(at(D_ALIGN)).and_then(|a| a.0);
                        style.fill = st.fill.value();
                        style.color = st.color.value();
                        let border = BORDERS.get(at(D_BORDER)).and_then(|b| b.0).map(|p| {
                            (
                                p,
                                Line {
                                    style: LINES.get(at(D_LINE)).map_or(LineStyle::Thin, |l| l.0),
                                    color: st.line_color,
                                },
                            )
                        });
                        st.result = Some((style, border));
                        let _ = EndDialog(hwnd, IDOK_ as isize);
                        1
                    }
                    IDCANCEL_ => {
                        let _ = EndDialog(hwnd, IDCANCEL_ as isize);
                        1
                    }
                    _ => 0,
                }
            }
            _ => 0,
        }
    }
}

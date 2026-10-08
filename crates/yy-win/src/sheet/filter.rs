//! 絞り込み・並べ替えのダイアログ（15 章 9・12）。
//!
//! - 列の絞り込み: 値の一覧（件数付き。検索で絞って選ぶ）か、条件（2 つまで。かつ・または）。
//!   列見出しのボタンから開く。昇順・降順の並べ替えもここから選べる。
//! - 並べ替え: キーを複数（列と昇順・降順）。上のキーが優先。
//! - 絞り込みの段階: 付けた順の一覧（残りの行数付き）。段階を外す・順を変える。
//!
//! ダイアログはメモリ上のテンプレート（`goto::Template`）から作る。

use std::collections::HashSet;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{BST_CHECKED, CheckRadioButton, IsDlgButtonChecked};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;
use yy_numfmt::{DateSystem, Parsed};
use yy_sheet::query::{Cmp, ColFilter, Cond, Key, SortKey, TextOp};

use crate::goto::{CLASS_BUTTON, CLASS_EDIT, CLASS_STATIC, Template};
use crate::util::group_digits;

const CLASS_LISTBOX: u16 = 0x0083;
const CLASS_COMBOBOX: u16 = 0x0085;

const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;

// ---- 条件と画面の項目 ------------------------------------------------------------------

/// 条件の種類（画面の選択肢）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    None,
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Between,
    Contains,
    NotContains,
    BeginsWith,
    EndsWith,
    Wildcard,
    Top,
    Bottom,
    TopPercent,
    BottomPercent,
    AboveAverage,
    BelowAverage,
    Blank,
    NonBlank,
}

const OPS: [(Op, &str); 21] = [
    (Op::None, "（なし）"),
    (Op::Eq, "等しい"),
    (Op::Ne, "等しくない"),
    (Op::Gt, "より大きい"),
    (Op::Ge, "以上"),
    (Op::Lt, "より小さい"),
    (Op::Le, "以下"),
    (Op::Between, "範囲（以上・以下）"),
    (Op::Contains, "含む"),
    (Op::NotContains, "含まない"),
    (Op::BeginsWith, "で始まる"),
    (Op::EndsWith, "で終わる"),
    (Op::Wildcard, "ワイルドカード（* ?）"),
    (Op::Top, "上位 N 件"),
    (Op::Bottom, "下位 N 件"),
    (Op::TopPercent, "上位 N %"),
    (Op::BottomPercent, "下位 N %"),
    (Op::AboveAverage, "平均より上"),
    (Op::BelowAverage, "平均より下"),
    (Op::Blank, "空"),
    (Op::NonBlank, "空でない"),
];

fn op_index(op: Op) -> usize {
    OPS.iter().position(|(o, _)| *o == op).unwrap_or(0)
}

/// 画面の 1 つの条件（種類と値 2 つ）。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Line {
    pub op: Op,
    pub a: String,
    pub b: String,
}

impl Line {
    fn none() -> Line {
        Line {
            op: Op::None,
            a: String::new(),
            b: String::new(),
        }
    }
}

fn number(s: &str, sys: DateSystem) -> Result<f64, String> {
    match yy_numfmt::parse_input(s, sys) {
        Parsed::Number(n, _) => Ok(n),
        _ => Err(format!("「{s}」は数値・日付ではありません")),
    }
}

fn count(s: &str) -> Result<u64, String> {
    s.trim()
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("「{s}」は 1 以上の整数ではありません"))
}

/// 画面の条件 → 列の条件。`Op::None` なら `Ok(None)`。
pub(crate) fn build_line(l: &Line, sys: DateSystem) -> Result<Option<Cond>, String> {
    let a = l.a.trim();
    let text = |op: TextOp, negate: bool| Cond::Text {
        op,
        pattern: a.to_owned(),
        negate,
    };
    let num = |op: Cmp| -> Result<Cond, String> {
        Ok(Cond::Number {
            op,
            value: number(a, sys)?,
        })
    };
    Ok(Some(match l.op {
        Op::None => return Ok(None),
        Op::Eq | Op::Ne => {
            let ne = l.op == Op::Ne;
            match yy_numfmt::parse_input(a, sys) {
                Parsed::Number(n, _) => Cond::Number {
                    op: if ne { Cmp::Ne } else { Cmp::Eq },
                    value: n,
                },
                _ if a.contains(['*', '?']) => text(TextOp::Wildcard, ne),
                _ => text(TextOp::Equals, ne),
            }
        }
        Op::Gt => num(Cmp::Gt)?,
        Op::Ge => num(Cmp::Ge)?,
        Op::Lt => num(Cmp::Lt)?,
        Op::Le => num(Cmp::Le)?,
        Op::Between => {
            let (x, y) = (number(a, sys)?, number(l.b.trim(), sys)?);
            Cond::Between(x.min(y), x.max(y))
        }
        Op::Contains => text(TextOp::Contains, false),
        Op::NotContains => text(TextOp::Contains, true),
        Op::BeginsWith => text(TextOp::BeginsWith, false),
        Op::EndsWith => text(TextOp::EndsWith, false),
        Op::Wildcard => text(TextOp::Wildcard, false),
        Op::Top | Op::Bottom | Op::TopPercent | Op::BottomPercent => Cond::Top {
            n: count(a)?,
            bottom: matches!(l.op, Op::Bottom | Op::BottomPercent),
            percent: matches!(l.op, Op::TopPercent | Op::BottomPercent),
        },
        Op::AboveAverage => Cond::Average { above: true },
        Op::BelowAverage => Cond::Average { above: false },
        Op::Blank => Cond::Blank,
        Op::NonBlank => Cond::NonBlank,
    }))
}

/// 2 つの条件を合わせる（`or` なら「または」）。
pub(crate) fn build(
    first: &Line,
    second: &Line,
    or: bool,
    sys: DateSystem,
) -> Result<Option<Cond>, String> {
    let x = build_line(first, sys)?;
    let y = build_line(second, sys)?;
    Ok(match (x, y) {
        (None, None) => None,
        (Some(c), None) | (None, Some(c)) => Some(c),
        (Some(a), Some(b)) => Some(if or {
            Cond::Or(Box::new(a), Box::new(b))
        } else {
            Cond::And(Box::new(a), Box::new(b))
        }),
    })
}

fn num_text(n: f64) -> String {
    yy_numfmt::general(n)
}

/// 列の条件 → 画面の条件（値の一覧は扱わない）。
fn line_of(c: &Cond) -> Option<Line> {
    let l = |op: Op, a: String| Line {
        op,
        a,
        b: String::new(),
    };
    Some(match c {
        Cond::Number { op, value } => l(
            match op {
                Cmp::Eq => Op::Eq,
                Cmp::Ne => Op::Ne,
                Cmp::Gt => Op::Gt,
                Cmp::Ge => Op::Ge,
                Cmp::Lt => Op::Lt,
                Cmp::Le => Op::Le,
            },
            num_text(*value),
        ),
        Cond::Between(x, y) => Line {
            op: Op::Between,
            a: num_text(*x),
            b: num_text(*y),
        },
        Cond::Text {
            op,
            pattern,
            negate,
        } => l(
            match (op, negate) {
                (TextOp::Equals, false) => Op::Eq,
                (TextOp::Equals, true) => Op::Ne,
                (TextOp::Contains, false) => Op::Contains,
                (TextOp::Contains, true) => Op::NotContains,
                (TextOp::BeginsWith, _) => Op::BeginsWith,
                (TextOp::EndsWith, _) => Op::EndsWith,
                (TextOp::Wildcard, false) => Op::Wildcard,
                (TextOp::Wildcard, true) => Op::Ne,
            },
            pattern.clone(),
        ),
        Cond::Top { n, bottom, percent } => l(
            match (bottom, percent) {
                (false, false) => Op::Top,
                (true, false) => Op::Bottom,
                (false, true) => Op::TopPercent,
                (true, true) => Op::BottomPercent,
            },
            n.to_string(),
        ),
        Cond::Average { above: true } => l(Op::AboveAverage, String::new()),
        Cond::Average { above: false } => l(Op::BelowAverage, String::new()),
        Cond::Blank => l(Op::Blank, String::new()),
        Cond::NonBlank => l(Op::NonBlank, String::new()),
        Cond::Values { .. } | Cond::And(..) | Cond::Or(..) => return None,
    })
}

/// 列の条件 → 画面の 2 つの条件と「または」。値の一覧や入れ子は `None`。
pub(crate) fn lines_of(c: &Cond) -> Option<(Line, Line, bool)> {
    match c {
        Cond::And(a, b) => Some((line_of(a)?, line_of(b)?, false)),
        Cond::Or(a, b) => Some((line_of(a)?, line_of(b)?, true)),
        c => Some((line_of(c)?, Line::none(), false)),
    }
}

/// 条件の短い説明（ステータスバー・段階の一覧）。
pub(crate) fn describe(c: &Cond) -> String {
    match c {
        Cond::Values { values, blanks } => {
            let n = values.len() + *blanks as usize;
            let first = values.iter().next().map(|k| match k {
                Key::Number(b) => num_text(f64::from_bits(*b)),
                Key::Text(s) => s.clone(),
                Key::Bool(b) => (if *b { "TRUE" } else { "FALSE" }).into(),
                Key::Error(_) => "エラー".into(),
            });
            match (first, n) {
                (Some(f), 1) => format!("= {f}"),
                (Some(f), n) => format!("= {f} ほか {} 個", n - 1),
                (None, _) => "= 空".into(),
            }
        }
        Cond::And(a, b) => format!("{} かつ {}", describe(a), describe(b)),
        Cond::Or(a, b) => format!("{} または {}", describe(a), describe(b)),
        c => match line_of(c) {
            Some(l) => {
                let name = OPS[op_index(l.op)].1;
                match l.op {
                    Op::Between => format!("{}〜{}", l.a, l.b),
                    Op::Top | Op::Bottom | Op::TopPercent | Op::BottomPercent => {
                        name.replace('N', &l.a)
                    }
                    _ if l.a.is_empty() => name.to_owned(),
                    _ => format!("{name} {}", l.a),
                }
            }
            None => String::new(),
        },
    }
}

// ---- テンプレートの部品 --------------------------------------------------------------

pub(super) fn dlg_text(hwnd: HWND, id: u16) -> String {
    let mut buf = vec![0u16; 4096];
    let n = unsafe { GetDlgItemTextW(hwnd, id as i32, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn set_dlg_text(hwnd: HWND, id: u16, s: &str) {
    unsafe {
        let _ = SetDlgItemTextW(hwnd, id as i32, &HSTRING::from(s));
    }
}

pub(super) fn send(hwnd: HWND, id: u16, msg: u32, w: usize, l: isize) -> isize {
    unsafe { SendDlgItemMessageW(hwnd, id as i32, msg, WPARAM(w), LPARAM(l)).0 }
}

pub(super) fn add_string(hwnd: HWND, id: u16, msg: u32, s: &str) -> isize {
    let w = HSTRING::from(s);
    send(hwnd, id, msg, 0, w.as_ptr() as isize)
}

pub(super) fn message(hwnd: HWND, text: &str) {
    unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(text),
            &HSTRING::from("yysheet"),
            MB_OK | MB_ICONWARNING,
        );
    }
}

pub(super) fn button(
    t: &mut Template,
    x: i16,
    y: i16,
    cx: i16,
    id: u16,
    text: &str,
    default: bool,
) {
    let style = if default {
        BS_DEFPUSHBUTTON
    } else {
        BS_PUSHBUTTON
    };
    t.item(
        WS_TABSTOP.0 | style as u32,
        x,
        y,
        cx,
        14,
        id,
        CLASS_BUTTON,
        text,
    );
}

pub(super) fn label(t: &mut Template, x: i16, y: i16, cx: i16, id: u16, text: &str) {
    t.item(0, x, y, cx, 10, id, CLASS_STATIC, text);
}

pub(super) fn combo(t: &mut Template, x: i16, y: i16, cx: i16, id: u16) {
    t.item(
        (WS_TABSTOP | WS_VSCROLL).0 | CBS_DROPDOWNLIST as u32,
        x,
        y,
        cx,
        160,
        id,
        CLASS_COMBOBOX,
        "",
    );
}

pub(super) fn edit(t: &mut Template, x: i16, y: i16, cx: i16, id: u16) {
    t.item(
        (WS_BORDER | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32,
        x,
        y,
        cx,
        13,
        id,
        CLASS_EDIT,
        "",
    );
}

pub(super) fn run<S>(t: &Template, owner: HWND, state: &mut S, proc_: DLGPROC) -> isize {
    let aligned = t.aligned();
    unsafe {
        DialogBoxIndirectParamW(
            None,
            aligned.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            proc_,
            LPARAM(state as *mut S as isize),
        )
    }
}

pub(super) unsafe fn state<'a, S>(hwnd: HWND) -> &'a mut S {
    unsafe { &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut S) }
}

// ---- 列の絞り込み -------------------------------------------------------------------

/// 値の一覧の 1 項目。
pub(crate) struct Entry {
    pub label: String,
    /// `None` は空のセル
    pub key: Option<Key>,
    pub count: u64,
}

/// 列の絞り込みのダイアログの結果。
pub(crate) enum FilterChoice {
    Cancel,
    /// 列の条件を外す
    Clear,
    SortAsc,
    SortDesc,
    Set(Cond),
}

const F_SORT_ASC: u16 = 10;
const F_SORT_DESC: u16 = 11;
const F_SEARCH: u16 = 12;
const F_LIST: u16 = 13;
const F_ALL: u16 = 14;
const F_NONE: u16 = 15;
const F_INFO: u16 = 16;
const F_OP1: u16 = 17;
const F_A1: u16 = 18;
const F_B1: u16 = 19;
const F_AND: u16 = 20;
const F_OR: u16 = 21;
const F_OP2: u16 = 22;
const F_A2: u16 = 23;
const F_B2: u16 = 24;
const F_CLEAR: u16 = 25;

struct FilterState {
    entries: Vec<Entry>,
    checked: Vec<bool>,
    info: String,
    lines: (Line, Line, bool),
    sys: DateSystem,
    choice: FilterChoice,
}

/// 列の絞り込みのダイアログ。`entries` は値の一覧（値の順。空のセルは最後）、`cut` なら一覧は
/// 途中まで。`current` は今の列の条件。
pub(crate) fn filter_dialog(
    owner: HWND,
    column: &str,
    entries: Vec<Entry>,
    cut: bool,
    current: Option<&Cond>,
    sys: DateSystem,
) -> FilterChoice {
    let mut t = Template::dialog(&format!("絞り込み: {column}"), 280, 262);
    button(&mut t, 7, 7, 80, F_SORT_ASC, "昇順に並べ替え", false);
    button(&mut t, 91, 7, 80, F_SORT_DESC, "降順に並べ替え", false);
    label(&mut t, 7, 28, 60, 0, "値で絞り込む:");
    label(&mut t, 7, 42, 24, 0, "検索:");
    edit(&mut t, 33, 40, 240, F_SEARCH);
    t.item(
        (WS_BORDER | WS_TABSTOP | WS_VSCROLL).0
            | (LBS_MULTIPLESEL | LBS_NOTIFY | LBS_USETABSTOPS | LBS_NOINTEGRALHEIGHT) as u32,
        7,
        57,
        266,
        96,
        F_LIST,
        CLASS_LISTBOX,
        "",
    );
    button(&mut t, 7, 157, 54, F_ALL, "すべて選択", false);
    button(&mut t, 65, 157, 54, F_NONE, "すべて解除", false);
    label(&mut t, 125, 160, 148, F_INFO, "");
    label(
        &mut t,
        7,
        180,
        266,
        0,
        "条件で絞り込む（条件を選ぶと値の一覧より優先）:",
    );
    combo(&mut t, 7, 192, 96, F_OP1);
    edit(&mut t, 107, 192, 82, F_A1);
    edit(&mut t, 193, 192, 80, F_B1);
    t.item(
        (WS_TABSTOP | WS_GROUP).0 | BS_AUTORADIOBUTTON as u32,
        7,
        209,
        40,
        10,
        F_AND,
        CLASS_BUTTON,
        "かつ",
    );
    t.item(
        BS_AUTORADIOBUTTON as u32,
        51,
        209,
        50,
        10,
        F_OR,
        CLASS_BUTTON,
        "または",
    );
    combo(&mut t, 7, 222, 96, F_OP2);
    edit(&mut t, 107, 222, 82, F_A2);
    edit(&mut t, 193, 222, 80, F_B2);
    button(&mut t, 7, 242, 70, F_CLEAR, "条件を外す", false);
    button(&mut t, 169, 242, 50, IDOK_, "OK", true);
    button(&mut t, 223, 242, 50, IDCANCEL_, "キャンセル", false);

    let kinds = entries.len();
    let checked = match current {
        Some(Cond::Values { values, blanks }) => entries
            .iter()
            .map(|e| match &e.key {
                Some(k) => values.contains(k),
                None => *blanks,
            })
            .collect(),
        _ => vec![true; kinds],
    };
    let lines = current
        .and_then(lines_of)
        .unwrap_or_else(|| (Line::none(), Line::none(), false));
    let info = if cut {
        format!("先頭の {} 種類", group_digits(kinds as u64))
    } else {
        format!("{} 種類", group_digits(kinds as u64))
    };
    let mut st = FilterState {
        entries,
        checked,
        info,
        lines,
        sys,
        choice: FilterChoice::Cancel,
    };
    run(&t, owner, &mut st, Some(filter_proc));
    st.choice
}

fn fill_list(hwnd: HWND, st: &FilterState) {
    let search = dlg_text(hwnd, F_SEARCH).to_lowercase();
    unsafe {
        if let Ok(list) = GetDlgItem(Some(hwnd), F_LIST as i32) {
            SendMessageW(list, WM_SETREDRAW, Some(WPARAM(0)), None);
        }
    }
    send(hwnd, F_LIST, LB_RESETCONTENT, 0, 0);
    for (i, e) in st.entries.iter().enumerate() {
        if !search.is_empty() && !e.label.to_lowercase().contains(&search) {
            continue;
        }
        let at = add_string(
            hwnd,
            F_LIST,
            LB_ADDSTRING,
            &format!("{}\t{}", e.label, group_digits(e.count)),
        );
        if at < 0 {
            break;
        }
        send(hwnd, F_LIST, LB_SETITEMDATA, at as usize, i as isize);
        if st.checked[i] {
            send(hwnd, F_LIST, LB_SETSEL, 1, at);
        }
    }
    unsafe {
        if let Ok(list) = GetDlgItem(Some(hwnd), F_LIST as i32) {
            SendMessageW(list, WM_SETREDRAW, Some(WPARAM(1)), None);
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(list), None, true);
        }
    }
}

/// 一覧の選択を `checked` に写す（見えている項目だけ）。
fn read_list(hwnd: HWND, st: &mut FilterState) {
    let n = send(hwnd, F_LIST, LB_GETCOUNT, 0, 0).max(0) as usize;
    for at in 0..n {
        let i = send(hwnd, F_LIST, LB_GETITEMDATA, at, 0) as usize;
        if let Some(c) = st.checked.get_mut(i) {
            *c = send(hwnd, F_LIST, LB_GETSEL, at, 0) > 0;
        }
    }
}

fn read_line(hwnd: HWND, op: u16, a: u16, b: u16) -> Line {
    let i = send(hwnd, op, CB_GETCURSEL, 0, 0).max(0) as usize;
    Line {
        op: OPS.get(i).map_or(Op::None, |o| o.0),
        a: dlg_text(hwnd, a),
        b: dlg_text(hwnd, b),
    }
}

/// 値を使わない条件では値の欄を、範囲でなければ 2 つ目の欄を使えなくする。
fn enable_values(hwnd: HWND, op: u16, a: u16, b: u16) {
    let i = send(hwnd, op, CB_GETCURSEL, 0, 0).max(0) as usize;
    let o = OPS.get(i).map_or(Op::None, |o| o.0);
    let uses_a = !matches!(
        o,
        Op::None | Op::AboveAverage | Op::BelowAverage | Op::Blank | Op::NonBlank
    );
    unsafe {
        if let Ok(h) = GetDlgItem(Some(hwnd), a as i32) {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(h, uses_a);
        }
        if let Ok(h) = GetDlgItem(Some(hwnd), b as i32) {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(h, o == Op::Between);
        }
    }
}

extern "system" fn filter_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = state::<FilterState>(hwnd);
                // 値と件数の間のタブ位置（ダイアログ単位）
                let stops = [200i32];
                send(hwnd, F_LIST, LB_SETTABSTOPS, 1, stops.as_ptr() as isize);
                fill_list(hwnd, st);
                set_dlg_text(hwnd, F_INFO, &st.info);
                for (op, a, b, l) in [
                    (F_OP1, F_A1, F_B1, &st.lines.0),
                    (F_OP2, F_A2, F_B2, &st.lines.1),
                ] {
                    for (_, name) in OPS {
                        add_string(hwnd, op, CB_ADDSTRING, name);
                    }
                    send(hwnd, op, CB_SETCURSEL, op_index(l.op), 0);
                    set_dlg_text(hwnd, a, &l.a);
                    set_dlg_text(hwnd, b, &l.b);
                    enable_values(hwnd, op, a, b);
                }
                let _ = CheckRadioButton(
                    hwnd,
                    F_AND as i32,
                    F_OR as i32,
                    if st.lines.2 { F_OR } else { F_AND } as i32,
                );
                1
            }
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let code = crate::hiword(wparam.0);
                let st = state::<FilterState>(hwnd);
                match id {
                    F_SEARCH if code == EN_CHANGE => {
                        fill_list(hwnd, st);
                        1
                    }
                    F_LIST if code == LBN_SELCHANGE => {
                        read_list(hwnd, st);
                        1
                    }
                    F_ALL | F_NONE => {
                        send(hwnd, F_LIST, LB_SETSEL, (id == F_ALL) as usize, -1);
                        read_list(hwnd, st);
                        1
                    }
                    F_OP1 if code == CBN_SELCHANGE => {
                        enable_values(hwnd, F_OP1, F_A1, F_B1);
                        1
                    }
                    F_OP2 if code == CBN_SELCHANGE => {
                        enable_values(hwnd, F_OP2, F_A2, F_B2);
                        1
                    }
                    F_SORT_ASC | F_SORT_DESC => {
                        st.choice = if id == F_SORT_ASC {
                            FilterChoice::SortAsc
                        } else {
                            FilterChoice::SortDesc
                        };
                        let _ = EndDialog(hwnd, IDOK_ as isize);
                        1
                    }
                    F_CLEAR => {
                        st.choice = FilterChoice::Clear;
                        let _ = EndDialog(hwnd, IDOK_ as isize);
                        1
                    }
                    IDOK_ => {
                        let first = read_line(hwnd, F_OP1, F_A1, F_B1);
                        let second = read_line(hwnd, F_OP2, F_A2, F_B2);
                        let or = IsDlgButtonChecked(hwnd, F_OR as i32) == BST_CHECKED.0;
                        match build(&first, &second, or, st.sys) {
                            Err(e) => {
                                message(hwnd, &e);
                                return 1;
                            }
                            Ok(Some(c)) => st.choice = FilterChoice::Set(c),
                            Ok(None) => {
                                if st.checked.iter().all(|&c| c) {
                                    st.choice = FilterChoice::Clear;
                                } else if !st.checked.iter().any(|&c| c) {
                                    message(hwnd, "値を 1 つ以上選んでください。");
                                    return 1;
                                } else {
                                    let mut values = HashSet::new();
                                    let mut blanks = false;
                                    for (e, &c) in st.entries.iter().zip(&st.checked) {
                                        if c {
                                            match &e.key {
                                                Some(k) => {
                                                    values.insert(k.clone());
                                                }
                                                None => blanks = true,
                                            }
                                        }
                                    }
                                    st.choice = FilterChoice::Set(Cond::Values { values, blanks });
                                }
                            }
                        }
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

// ---- 並べ替え ---------------------------------------------------------------------

const S_LIST: u16 = 10;
const S_COL: u16 = 11;
const S_ORDER: u16 = 12;
const S_ADD: u16 = 13;
const S_CHANGE: u16 = 14;
const S_REMOVE: u16 = 15;
const S_UP: u16 = 16;
const S_DOWN: u16 = 17;
const S_CLEAR: u16 = 18;

struct SortState {
    columns: Vec<String>,
    keys: Vec<SortKey>,
    /// 初めに選んでおく列
    first: usize,
}

/// 並べ替えのダイアログ。`columns` は表の列の名前。キャンセルなら `None`（空のキーは並べ替えの解除）。
pub(crate) fn sort_dialog(
    owner: HWND,
    columns: Vec<String>,
    keys: &[SortKey],
    first: u32,
) -> Option<Vec<SortKey>> {
    let mut t = Template::dialog("並べ替え", 260, 196);
    label(&mut t, 7, 7, 246, 0, "キー（上が優先）:");
    t.item(
        (WS_BORDER | WS_TABSTOP | WS_VSCROLL).0 | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
        7,
        19,
        190,
        100,
        S_LIST,
        CLASS_LISTBOX,
        "",
    );
    button(&mut t, 203, 19, 50, S_UP, "上へ", false);
    button(&mut t, 203, 37, 50, S_DOWN, "下へ", false);
    button(&mut t, 203, 55, 50, S_REMOVE, "削除", false);
    button(&mut t, 203, 73, 50, S_CLEAR, "すべて削除", false);
    label(&mut t, 7, 126, 40, 0, "列:");
    combo(&mut t, 7, 138, 130, S_COL);
    combo(&mut t, 141, 138, 56, S_ORDER);
    button(&mut t, 203, 137, 50, S_ADD, "追加", false);
    button(&mut t, 203, 155, 50, S_CHANGE, "変更", false);
    button(&mut t, 149, 176, 50, IDOK_, "OK", true);
    button(&mut t, 203, 176, 50, IDCANCEL_, "キャンセル", false);
    let first = (first as usize).min(columns.len().saturating_sub(1));
    let mut keys = keys.to_vec();
    keys.retain(|k| (k.col as usize) < columns.len());
    let mut st = SortState {
        columns,
        keys,
        first,
    };
    let r = run(&t, owner, &mut st, Some(sort_proc));
    (r == IDOK_ as isize).then_some(st.keys)
}

fn fill_keys(hwnd: HWND, st: &SortState, select: Option<usize>) {
    send(hwnd, S_LIST, LB_RESETCONTENT, 0, 0);
    for (i, k) in st.keys.iter().enumerate() {
        let name = st.columns.get(k.col as usize).map_or("?", |s| s.as_str());
        add_string(
            hwnd,
            S_LIST,
            LB_ADDSTRING,
            &format!(
                "{}. {}（{}）",
                i + 1,
                name,
                if k.desc { "降順" } else { "昇順" }
            ),
        );
    }
    if let Some(i) = select {
        send(hwnd, S_LIST, LB_SETCURSEL, i, 0);
    }
}

fn selected(hwnd: HWND) -> Option<usize> {
    let i = send(hwnd, S_LIST, LB_GETCURSEL, 0, 0);
    (i >= 0).then_some(i as usize)
}

fn read_key(hwnd: HWND) -> Option<SortKey> {
    let col = send(hwnd, S_COL, CB_GETCURSEL, 0, 0);
    let desc = send(hwnd, S_ORDER, CB_GETCURSEL, 0, 0) == 1;
    (col >= 0).then_some(SortKey {
        col: col as u32,
        desc,
    })
}

extern "system" fn sort_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = state::<SortState>(hwnd);
                for c in &st.columns {
                    add_string(hwnd, S_COL, CB_ADDSTRING, c);
                }
                add_string(hwnd, S_ORDER, CB_ADDSTRING, "昇順");
                add_string(hwnd, S_ORDER, CB_ADDSTRING, "降順");
                send(hwnd, S_COL, CB_SETCURSEL, st.first, 0);
                send(hwnd, S_ORDER, CB_SETCURSEL, 0, 0);
                fill_keys(hwnd, st, None);
                1
            }
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let code = crate::hiword(wparam.0);
                let st = state::<SortState>(hwnd);
                match id {
                    S_LIST if code == LBN_SELCHANGE => {
                        if let Some(k) = selected(hwnd).and_then(|i| st.keys.get(i)) {
                            send(hwnd, S_COL, CB_SETCURSEL, k.col as usize, 0);
                            send(hwnd, S_ORDER, CB_SETCURSEL, k.desc as usize, 0);
                        }
                        1
                    }
                    S_ADD => {
                        if let Some(k) = read_key(hwnd) {
                            // 同じ列のキーは 1 つだけ
                            st.keys.retain(|x| x.col != k.col);
                            st.keys.push(k);
                            fill_keys(hwnd, st, Some(st.keys.len() - 1));
                        }
                        1
                    }
                    S_CHANGE => {
                        if let (Some(i), Some(k)) = (selected(hwnd), read_key(hwnd)) {
                            st.keys[i] = k;
                            let mut seen = HashSet::new();
                            let mut j = 0;
                            st.keys.retain(|x| {
                                j += 1;
                                seen.insert(x.col) || j - 1 == i
                            });
                            fill_keys(hwnd, st, Some(i.min(st.keys.len().saturating_sub(1))));
                        }
                        1
                    }
                    S_REMOVE => {
                        if let Some(i) = selected(hwnd) {
                            st.keys.remove(i);
                            let next = (!st.keys.is_empty()).then(|| i.min(st.keys.len() - 1));
                            fill_keys(hwnd, st, next);
                        }
                        1
                    }
                    S_CLEAR => {
                        st.keys.clear();
                        fill_keys(hwnd, st, None);
                        1
                    }
                    S_UP | S_DOWN => {
                        if let Some(i) = selected(hwnd) {
                            let j = if id == S_UP {
                                i.checked_sub(1)
                            } else {
                                (i + 1 < st.keys.len()).then_some(i + 1)
                            };
                            if let Some(j) = j {
                                st.keys.swap(i, j);
                                fill_keys(hwnd, st, Some(j));
                            }
                        }
                        1
                    }
                    IDOK_ => {
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

// ---- 絞り込みの段階 -------------------------------------------------------------------

const G_LIST: u16 = 10;
const G_REMOVE: u16 = 11;
const G_UP: u16 = 12;
const G_DOWN: u16 = 13;
const G_CLEAR: u16 = 14;

struct StageState {
    columns: Vec<String>,
    filters: Vec<ColFilter>,
    /// 開いたときの段階ごとの残りの行数（順を変えたら出さない）
    counts: Vec<u64>,
    changed: bool,
}

/// 絞り込みの段階のダイアログ。キャンセルなら `None`。
pub(crate) fn stages_dialog(
    owner: HWND,
    columns: Vec<String>,
    filters: &[ColFilter],
    counts: &[u64],
) -> Option<Vec<ColFilter>> {
    let mut t = Template::dialog("絞り込みの段階", 300, 160);
    label(
        &mut t,
        7,
        7,
        286,
        0,
        "付けた順の段階（すべてを満たす行を表示。→ のあとは残りの行数）:",
    );
    t.item(
        (WS_BORDER | WS_TABSTOP | WS_VSCROLL | WS_HSCROLL).0
            | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
        7,
        19,
        230,
        116,
        G_LIST,
        CLASS_LISTBOX,
        "",
    );
    button(&mut t, 243, 19, 50, G_UP, "上へ", false);
    button(&mut t, 243, 37, 50, G_DOWN, "下へ", false);
    button(&mut t, 243, 55, 50, G_REMOVE, "外す", false);
    button(&mut t, 243, 73, 50, G_CLEAR, "すべて外す", false);
    button(&mut t, 189, 140, 50, IDOK_, "OK", true);
    button(&mut t, 243, 140, 50, IDCANCEL_, "キャンセル", false);
    let mut st = StageState {
        columns,
        filters: filters.to_vec(),
        counts: counts.to_vec(),
        changed: false,
    };
    let r = run(&t, owner, &mut st, Some(stages_proc));
    (r == IDOK_ as isize).then_some(st.filters)
}

fn fill_stages(hwnd: HWND, st: &StageState, select: Option<usize>) {
    send(hwnd, G_LIST, LB_RESETCONTENT, 0, 0);
    for (i, f) in st.filters.iter().enumerate() {
        let name = st.columns.get(f.col as usize).map_or("?", |s| s.as_str());
        let rest = match st.counts.get(i) {
            Some(n) if !st.changed => format!(" → {} 行", group_digits(*n)),
            _ => String::new(),
        };
        add_string(
            hwnd,
            G_LIST,
            LB_ADDSTRING,
            &format!("{}. {} {}{rest}", i + 1, name, describe(&f.cond)),
        );
    }
    if let Some(i) = select {
        send(hwnd, G_LIST, LB_SETCURSEL, i, 0);
    }
}

extern "system" fn stages_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = state::<StageState>(hwnd);
                send(hwnd, G_LIST, LB_SETHORIZONTALEXTENT, 2000, 0);
                fill_stages(hwnd, st, None);
                1
            }
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let st = state::<StageState>(hwnd);
                let sel = {
                    let i = send(hwnd, G_LIST, LB_GETCURSEL, 0, 0);
                    (i >= 0).then_some(i as usize)
                };
                match id {
                    G_REMOVE => {
                        if let Some(i) = sel {
                            st.filters.remove(i);
                            st.changed = true;
                            let next =
                                (!st.filters.is_empty()).then(|| i.min(st.filters.len() - 1));
                            fill_stages(hwnd, st, next);
                        }
                        1
                    }
                    G_CLEAR => {
                        st.filters.clear();
                        st.changed = true;
                        fill_stages(hwnd, st, None);
                        1
                    }
                    G_UP | G_DOWN => {
                        if let Some(i) = sel {
                            let j = if id == G_UP {
                                i.checked_sub(1)
                            } else {
                                (i + 1 < st.filters.len()).then_some(i + 1)
                            };
                            if let Some(j) = j {
                                st.filters.swap(i, j);
                                st.changed = true;
                                fill_stages(hwnd, st, Some(j));
                            }
                        }
                        1
                    }
                    IDOK_ => {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn line(op: Op, a: &str) -> Line {
        Line {
            op,
            a: a.into(),
            b: String::new(),
        }
    }

    #[test]
    fn builds_conditions() {
        let sys = DateSystem::D1900;
        assert!(matches!(
            build_line(&line(Op::Eq, "10"), sys),
            Ok(Some(Cond::Number { op: Cmp::Eq, value })) if value == 10.0
        ));
        assert!(matches!(
            build_line(&line(Op::Eq, "東*"), sys),
            Ok(Some(Cond::Text {
                op: TextOp::Wildcard,
                negate: false,
                ..
            }))
        ));
        assert!(matches!(
            build_line(&line(Op::Ge, "2026/10/7"), sys),
            Ok(Some(Cond::Number { op: Cmp::Ge, value })) if value == 46302.0
        ));
        assert!(build_line(&line(Op::Gt, "abc"), sys).is_err());
        assert!(build_line(&line(Op::Top, "0"), sys).is_err());
        let between = Line {
            op: Op::Between,
            a: "9".into(),
            b: "3".into(),
        };
        assert!(matches!(
            build_line(&between, sys),
            Ok(Some(Cond::Between(a, b))) if a == 3.0 && b == 9.0
        ));
        let c = build(&line(Op::Gt, "1"), &line(Op::Contains, "x"), true, sys)
            .unwrap()
            .unwrap();
        assert!(matches!(c, Cond::Or(..)));
        let (a, b, or) = lines_of(&c).unwrap();
        assert_eq!((a.op, b.op, or), (Op::Gt, Op::Contains, true));
        assert_eq!(describe(&c), "より大きい 1 または 含む x");
        assert_eq!(
            describe(&build_line(&line(Op::Top, "5"), sys).unwrap().unwrap()),
            "上位 5 件"
        );
        assert!(
            build(&Line::none(), &Line::none(), false, sys)
                .unwrap()
                .is_none()
        );
    }
}

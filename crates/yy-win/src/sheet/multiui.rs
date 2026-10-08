//! マルチレイアウトの固定長ファイルの UI（15 章 6.5）。
//!
//! - データ > マルチレイアウトの設定: 名前を付けたレイアウト（コピーブック）を登録する。空のシートなら
//!   レイアウトの列（「レイアウト」）と項目の列（「項目1」…）を作る（行ごとに入力する）。
//! - ファイル > 固定長ファイルを開く（マルチレイアウト）: レイアウトを登録してから取り込む。どの行も
//!   レイアウト未確定（背景が赤）で、項目1 に元のバイト、項目2 に文字として読んだ内容が入る。
//! - データ > 行のレイアウトを指定: 選んだ行のレイアウトを選ぶ（行のバイト列をそのレイアウトで読み直す）。
//!   レイアウトの列に名前を入力・貼り付けしても同じ。

use std::path::Path;
use std::sync::Arc;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateFontW, DeleteObject, HFONT, HGDIOBJ};
use windows::Win32::UI::Controls::{BST_CHECKED, CheckDlgButton, IsDlgButtonChecked};
use windows::Win32::UI::WindowsAndMessaging::*;
use yy_cobol::{Charset, Codec};
use yy_sheet::fixed::{self, FixedSpec, RecordSep};

use super::filter::{add_string, button, combo, edit, label, message, run, send, state};
use super::fixedui::{crlf, dlg_text_long, load_copybook, pick_file, remember};
use super::*;
use crate::goto::{CLASS_BUTTON, CLASS_EDIT, Template};

const CLASS_LISTBOX: u16 = 0x0083;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;
const M_LIST: u16 = 10;
const M_ADD: u16 = 11;
const M_DEL: u16 = 12;
const M_NAME: u16 = 13;
const M_LOAD: u16 = 14;
const M_TEXT: u16 = 15;
const M_CHARSET: u16 = 16;
const M_SEP: u16 = 17;
const M_LE: u16 = 18;
const M_SUMMARY: u16 = 19;
const M_LEN: u16 = 20;
const M_CATLOAD: u16 = 21;
const M_CATSAVE: u16 = 22;

struct MultiState {
    layouts: Vec<(String, String)>,
    /// 1 行のデータ長（入力の文字列。必須）
    data_len: String,
    cur: usize,
    charset: Charset,
    /// `None` は自動（開くときだけ）
    sep: Option<RecordSep>,
    little_endian: bool,
    /// 開くファイルの見本（先頭のバイト列・大きさ）
    open: Option<(Vec<u8>, u64)>,
    font: HFONT,
    /// 欄に値を入れている間（変更の通知を無視する）
    loading: bool,
    result: Option<FixedSpec>,
}

/// 初期値（レイアウト・1 行のデータ長・文字コード・区切り・2 進数の並び）。データ長は、マルチレイアウトの
/// シートを設定し直すときだけ入れておく（新しく作るときは必ず入力してもらう）。
type Initial = (
    Vec<(String, String)>,
    String,
    Charset,
    Option<RecordSep>,
    bool,
);

fn initial() -> Initial {
    // データ > レイアウトカタログから当てる
    if let Some(def) = super::catalogui::take_preset() {
        let last = with(|a| a.fixed_last.clone()).flatten();
        return (
            def.layouts.clone(),
            def.data_len.map(|n| n.to_string()).unwrap_or_default(),
            def.charset
                .or(last.as_ref().map(|s| s.codec.charset))
                .unwrap_or(Charset::Ms932),
            def.separator.or(last.as_ref().map(|s| s.separator)),
            def.little_endian
                .or(last.as_ref().map(|s| s.codec.little_endian))
                .unwrap_or(false),
        );
    }
    // レイアウトはシートごと: 今のシートになければ、文字コード・区切りだけ最後に使ったものから
    let (spec, last) =
        with(|a| (a.sheet().fixed.as_deref().cloned(), a.fixed_last.clone())).unwrap_or_default();
    if spec.is_none()
        && let Some(s) = last
    {
        return (
            vec![("LAYOUT1".into(), String::new())],
            String::new(),
            s.codec.charset,
            Some(s.separator),
            s.codec.little_endian,
        );
    }
    match spec {
        Some(s) if s.is_multi() => (
            s.multi
                .iter()
                .map(|m| (m.name.to_string(), m.copybook.to_string()))
                .collect(),
            s.data_len.to_string(),
            s.codec.charset,
            Some(s.separator),
            s.codec.little_endian,
        ),
        Some(s) => (
            vec![("LAYOUT1".into(), s.copybook.to_string())],
            String::new(),
            s.codec.charset,
            Some(s.separator),
            s.codec.little_endian,
        ),
        None => (
            vec![("LAYOUT1".into(), String::new())],
            String::new(),
            Charset::Ms932,
            None,
            false,
        ),
    }
}

fn read_controls(hwnd: HWND, st: &mut MultiState) {
    st.data_len = dlg_text_long(hwnd, M_LEN);
    if let Some(l) = st.layouts.get_mut(st.cur) {
        l.0 = dlg_text_long(hwnd, M_NAME);
        l.1 = dlg_text_long(hwnd, M_TEXT);
    }
    let cs = send(hwnd, M_CHARSET, CB_GETCURSEL, 0, 0).max(0) as usize;
    st.charset = Charset::all().get(cs).copied().unwrap_or(Charset::Ms932);
    let i = send(hwnd, M_SEP, CB_GETCURSEL, 0, 0).max(0) as usize;
    st.sep = if st.open.is_some() {
        i.checked_sub(1).map(|k| RecordSep::ALL[k])
    } else {
        Some(RecordSep::ALL[i.min(RecordSep::ALL.len() - 1)])
    };
    st.little_endian = unsafe { IsDlgButtonChecked(hwnd, M_LE as i32) } == BST_CHECKED.0;
}

/// ダイアログの値から設定を作る（自動の区切りは見本から推定）。
fn spec_of(st: &MultiState) -> std::result::Result<FixedSpec, String> {
    let codec = Codec {
        charset: st.charset,
        little_endian: st.little_endian,
    };
    let len: usize = match st.data_len.trim() {
        "" => 0,
        t => t
            .parse()
            .map_err(|_| format!("1 行のデータ長「{t}」はバイト数（整数）で入力してください"))?,
    };
    let mut spec =
        FixedSpec::new_multi(&st.layouts, len, codec, st.sep.unwrap_or(RecordSep::Crlf))?;
    if st.sep.is_none()
        && let Some((sample, _)) = &st.open
    {
        spec.separator = fixed::detect_separator(sample, spec.data_len);
    }
    Ok(spec)
}

fn update_summary(hwnd: HWND, st: &mut MultiState) {
    if st.loading {
        return;
    }
    read_controls(hwnd, st);
    let mut lines = Vec::new();
    match spec_of(st) {
        Ok(spec) => {
            lines.push(format!("1 行のデータ長: {} バイト", spec.data_len));
            if st.sep.is_none() {
                lines.push(format!("区切りの推定: {}", spec.separator.label()));
            }
            if let Some((_, len)) = &st.open {
                let (n, rest) = fixed::record_count(*len, spec.data_len, spec.separator);
                let mut l = format!("ファイル: {n} 行");
                if rest > 0 {
                    l.push_str(&format!(
                        "（末尾の {rest} バイトは行になりません。データ長・区切りを確かめてください）"
                    ));
                }
                lines.push(l);
            }
            for m in &spec.multi {
                let rest = spec.data_len - m.layout.record_len;
                lines.push(format!(
                    "{}: レコード {} バイト・項目 {} 個{}",
                    m.name,
                    m.layout.record_len,
                    m.layout.fields.len(),
                    if rest > 0 {
                        format!("（残りの {rest} バイトは空白）")
                    } else {
                        String::new()
                    }
                ));
                for w in &m.layout.warnings {
                    lines.push(format!("　注意: {w}"));
                }
            }
            lines.push(format!(
                "項目の列: {} 個（いちばん多いレイアウトの項目の数）",
                spec.max_fields()
            ));
            if st.open.is_some() {
                lines.push(
                    "取り込んだ行はどれもレイアウト未確定です。行のレイアウトを指定すると、そのレイアウトで読み直します。"
                        .into(),
                );
            }
        }
        Err(e) => lines.push(format!("レイアウトを読めません: {e}")),
    }
    unsafe {
        let _ = SetDlgItemTextW(hwnd, M_SUMMARY as i32, &HSTRING::from(lines.join("\r\n")));
    }
}

fn refresh_list(hwnd: HWND, st: &MultiState) {
    send(hwnd, M_LIST, LB_RESETCONTENT, 0, 0);
    for (n, _) in &st.layouts {
        let label = if n.trim().is_empty() {
            "（名前なし）"
        } else {
            n
        };
        add_string(hwnd, M_LIST, LB_ADDSTRING, label);
    }
    send(hwnd, M_LIST, LB_SETCURSEL, st.cur, 0);
}

fn load_current(hwnd: HWND, st: &mut MultiState) {
    st.loading = true;
    let (name, text) = st.layouts.get(st.cur).cloned().unwrap_or_default();
    unsafe {
        let _ = SetDlgItemTextW(hwnd, M_NAME as i32, &HSTRING::from(name));
        let _ = SetDlgItemTextW(hwnd, M_TEXT as i32, &HSTRING::from(crlf(&text)));
    }
    st.loading = false;
    update_summary(hwnd, st);
}

/// マルチレイアウトのダイアログ。`open` は開くファイルの見本。
fn multi_dialog(owner: HWND, open: Option<(Vec<u8>, u64)>, title: &str) -> Option<FixedSpec> {
    let (layouts, data_len, charset, sep, le) = initial();
    let mut t = Template::dialog(title, 460, 360);
    label(&mut t, 7, 8, 118, 0, "1 行のデータ長（バイト・必須）:");
    edit(&mut t, 127, 6, 50, M_LEN);
    label(
        &mut t,
        183,
        8,
        270,
        0,
        "どのレイアウトもこの長さに収め、短いレイアウトの残りは空白で書きます。",
    );
    let y = 20;
    label(&mut t, 7, 8 + y, 110, 0, "レイアウト:");
    t.item(
        (WS_BORDER | WS_TABSTOP | WS_VSCROLL).0 | LBS_NOTIFY as u32,
        7,
        20 + y,
        110,
        152,
        M_LIST,
        CLASS_LISTBOX,
        "",
    );
    button(&mut t, 7, 174 + y, 53, M_ADD, "追加(&A)", false);
    button(&mut t, 64, 174 + y, 53, M_DEL, "削除(&D)", false);
    label(&mut t, 125, 8 + y, 30, 0, "名前:");
    edit(&mut t, 155, 6 + y, 140, M_NAME);
    button(
        &mut t,
        340,
        4 + y,
        113,
        M_LOAD,
        "ファイルから読み込む(&F)...",
        false,
    );
    let multi = (WS_BORDER | WS_TABSTOP | WS_VSCROLL | WS_HSCROLL).0
        | (ES_MULTILINE | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_WANTRETURN) as u32;
    t.item(multi, 125, 22 + y, 328, 166, M_TEXT, CLASS_EDIT, "");
    label(&mut t, 7, 198 + y, 50, 0, "文字コード:");
    combo(&mut t, 55, 196 + y, 175, M_CHARSET);
    label(&mut t, 240, 198 + y, 70, 0, "レコードの区切り:");
    combo(&mut t, 308, 196 + y, 145, M_SEP);
    t.item(
        WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
        7,
        214 + y,
        446,
        10,
        M_LE,
        CLASS_BUTTON,
        "2 進数（COMP・BINARY・COMP-5）と浮動小数点（COMP-1・COMP-2）をリトルエンディアンで読み書きする",
    );
    t.item(
        (WS_BORDER | WS_VSCROLL | WS_HSCROLL).0
            | (ES_MULTILINE | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_READONLY) as u32,
        7,
        228 + y,
        446,
        88,
        M_SUMMARY,
        CLASS_EDIT,
        "",
    );
    button(
        &mut t,
        7,
        321 + y,
        104,
        M_CATLOAD,
        "カタログから読み込む(&G)...",
        false,
    );
    button(
        &mut t,
        115,
        321 + y,
        90,
        M_CATSAVE,
        "カタログに保存(&V)...",
        false,
    );
    button(&mut t, 346, 321 + y, 50, IDOK_, "OK", true);
    button(&mut t, 403, 321 + y, 50, IDCANCEL_, "キャンセル", false);
    let mut st = MultiState {
        layouts,
        data_len,
        cur: 0,
        charset,
        sep: if open.is_some() {
            None
        } else {
            sep.or(Some(RecordSep::Crlf))
        },
        little_endian: le,
        open,
        font: HFONT::default(),
        loading: false,
        result: None,
    };
    run(&t, owner, &mut st, Some(multi_proc));
    st.result
}

extern "system" fn multi_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = state::<MultiState>(hwnd);
                let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as i32;
                st.font = CreateFontW(
                    -12 * dpi / 96,
                    0,
                    0,
                    0,
                    400,
                    0,
                    0,
                    0,
                    windows::Win32::Graphics::Gdi::DEFAULT_CHARSET,
                    windows::Win32::Graphics::Gdi::OUT_DEFAULT_PRECIS,
                    windows::Win32::Graphics::Gdi::CLIP_DEFAULT_PRECIS,
                    windows::Win32::Graphics::Gdi::CLEARTYPE_QUALITY,
                    0,
                    w!("MS Gothic"),
                );
                for id in [M_TEXT, M_SUMMARY] {
                    send(hwnd, id, WM_SETFONT, st.font.0 as usize, 1);
                }
                send(hwnd, M_TEXT, EM_LIMITTEXT, 0, 0);
                st.loading = true;
                for (i, c) in Charset::all().iter().enumerate() {
                    add_string(hwnd, M_CHARSET, CB_ADDSTRING, &c.label());
                    if *c == st.charset {
                        send(hwnd, M_CHARSET, CB_SETCURSEL, i, 0);
                    }
                }
                let open = st.open.is_some();
                if open {
                    add_string(hwnd, M_SEP, CB_ADDSTRING, "自動（推定する）");
                }
                for r in RecordSep::ALL {
                    add_string(hwnd, M_SEP, CB_ADDSTRING, r.label());
                }
                let sel = match st.sep {
                    None => 0,
                    Some(r) => {
                        RecordSep::ALL.iter().position(|x| *x == r).unwrap_or(0) + open as usize
                    }
                };
                send(hwnd, M_SEP, CB_SETCURSEL, sel, 0);
                if st.little_endian {
                    let _ = CheckDlgButton(hwnd, M_LE as i32, BST_CHECKED);
                }
                let _ = SetDlgItemTextW(hwnd, M_LEN as i32, &HSTRING::from(st.data_len.as_str()));
                st.loading = false;
                refresh_list(hwnd, st);
                load_current(hwnd, st);
                // 1 行のデータ長を最初に入力してもらう
                if st.data_len.trim().is_empty()
                    && let Ok(h) = GetDlgItem(Some(hwnd), M_LEN as i32)
                {
                    let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(h));
                    return 0;
                }
                1
            }
            WM_DESTROY if GetWindowLongPtrW(hwnd, GWLP_USERDATA) != 0 => {
                let st = state::<MultiState>(hwnd);
                let _ = DeleteObject(HGDIOBJ(st.font.0));
                0
            }
            WM_COMMAND if GetWindowLongPtrW(hwnd, GWLP_USERDATA) == 0 => 0,
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let code = crate::hiword(wparam.0);
                let st = state::<MultiState>(hwnd);
                match id {
                    M_LIST if code == LBN_SELCHANGE => {
                        read_controls(hwnd, st);
                        let i = send(hwnd, M_LIST, LB_GETCURSEL, 0, 0);
                        if let Ok(i) = usize::try_from(i)
                            && i < st.layouts.len()
                        {
                            st.cur = i;
                            load_current(hwnd, st);
                        }
                        1
                    }
                    M_NAME if code == EN_CHANGE => {
                        if !st.loading {
                            update_summary(hwnd, st);
                            refresh_list(hwnd, st);
                        }
                        1
                    }
                    M_LEN | M_TEXT if code == EN_CHANGE => {
                        update_summary(hwnd, st);
                        1
                    }
                    M_CHARSET | M_SEP if code == CBN_SELCHANGE => {
                        update_summary(hwnd, st);
                        1
                    }
                    M_LE => {
                        update_summary(hwnd, st);
                        1
                    }
                    M_ADD => {
                        read_controls(hwnd, st);
                        let mut k = st.layouts.len() + 1;
                        while st
                            .layouts
                            .iter()
                            .any(|l| l.0.eq_ignore_ascii_case(&format!("LAYOUT{k}")))
                        {
                            k += 1;
                        }
                        st.layouts.push((format!("LAYOUT{k}"), String::new()));
                        st.cur = st.layouts.len() - 1;
                        refresh_list(hwnd, st);
                        load_current(hwnd, st);
                        if let Ok(h) = GetDlgItem(Some(hwnd), M_NAME as i32) {
                            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(h));
                        }
                        1
                    }
                    M_DEL => {
                        if st.layouts.len() > 1 {
                            st.layouts.remove(st.cur);
                            st.cur = st.cur.min(st.layouts.len() - 1);
                            refresh_list(hwnd, st);
                            load_current(hwnd, st);
                        }
                        1
                    }
                    M_LOAD => {
                        if let Some(text) = load_copybook(hwnd) {
                            let _ =
                                SetDlgItemTextW(hwnd, M_TEXT as i32, &HSTRING::from(crlf(&text)));
                            update_summary(hwnd, st);
                        }
                        1
                    }
                    M_CATLOAD => {
                        read_controls(hwnd, st);
                        if let Some((name, def)) = super::catalogui::pick(hwnd) {
                            let offset = st.open.is_some() as usize;
                            st.loading = true;
                            super::catalogui::set_codec_controls(
                                hwnd,
                                &def,
                                (M_CHARSET, M_SEP, M_LE),
                                offset,
                            );
                            if def.is_multi() {
                                // マルチレイアウトの定義: レイアウトとデータ長を入れ替える
                                st.layouts = def.layouts.clone();
                                if let Some(n) = def.data_len {
                                    let _ = SetDlgItemTextW(
                                        hwnd,
                                        M_LEN as i32,
                                        &HSTRING::from(n.to_string()),
                                    );
                                }
                                st.cur = 0;
                            } else {
                                // 単一のレイアウト: レイアウトの 1 つとして足す（今のが空なら置き換える）
                                let base = match def.layouts.first() {
                                    Some((n, _)) if !n.trim().is_empty() => n.trim().to_string(),
                                    _ => name
                                        .rsplit('/')
                                        .next()
                                        .unwrap_or(&name)
                                        .split('.')
                                        .next()
                                        .unwrap_or("LAYOUT")
                                        .to_uppercase(),
                                };
                                let mut n = base.clone();
                                let mut k = 2;
                                while st.layouts.iter().any(|l| l.0.eq_ignore_ascii_case(&n)) {
                                    n = format!("{base}{k}");
                                    k += 1;
                                }
                                let entry = (n, def.copybook().to_string());
                                if st.layouts.len() == 1 && st.layouts[0].1.trim().is_empty() {
                                    st.layouts[0] = entry;
                                    st.cur = 0;
                                } else {
                                    st.layouts.push(entry);
                                    st.cur = st.layouts.len() - 1;
                                }
                            }
                            st.loading = false;
                            refresh_list(hwnd, st);
                            load_current(hwnd, st);
                        }
                        1
                    }
                    M_CATSAVE => {
                        read_controls(hwnd, st);
                        match spec_of(st) {
                            Ok(spec) => {
                                let def = yy_sheet::catalog::LayoutDef::from_spec(&spec, "");
                                if let Some(name) = super::catalogui::save(hwnd, def) {
                                    set_status(&format!(
                                        "レイアウトをカタログの {name} に保存しました"
                                    ));
                                }
                            }
                            Err(e) => message(
                                hwnd,
                                &format!(
                                    "設定を読めないので保存できません。
{e}"
                                ),
                            ),
                        }
                        1
                    }
                    IDOK_ => {
                        read_controls(hwnd, st);
                        match spec_of(st) {
                            Ok(s) => {
                                st.result = Some(s);
                                let _ = EndDialog(hwnd, IDOK_ as isize);
                            }
                            Err(e) => {
                                message(hwnd, &format!("設定できません。\n{e}"));
                                if st.data_len.trim().is_empty()
                                    && let Ok(h) = GetDlgItem(Some(hwnd), M_LEN as i32)
                                {
                                    let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(
                                        Some(h),
                                    );
                                }
                            }
                        }
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

/// データ > マルチレイアウトの設定（今のシート）。
pub(super) fn multi_layout_dialog() {
    let Some(frame) = with(|a| {
        a.end_edit(true);
        a.frame
    }) else {
        return;
    };
    let Some(spec) = multi_dialog(frame, None, "マルチレイアウトの設定") else {
        return;
    };
    remember(&spec);
    let added = with(|a| {
        if a.sheet().view.rows.is_some() {
            info_box(
                a.frame,
                "絞り込み・並べ替えの表示中は設定できません。解除してから行ってください。",
            );
            return None;
        }
        let sheet = a.sheet;
        let mut added = 0;
        let mut bad = None;
        let res = a.doc.edit(|b, ctx| {
            added = fixed::apply_multi(ctx, &mut b.sheets[sheet], &spec).map_err(|e| {
                bad = Some(e.clone());
                std::io::Error::other(e)
            })?;
            Ok(())
        });
        a.after_edit();
        a.sync_formula();
        a.invalidate();
        match (res, bad) {
            (Ok(()), _) => Some(added),
            (Err(_), Some(e)) => {
                info_box(a.frame, &e);
                None
            }
            (Err(e), None) => {
                error_box(a.frame, &format!("設定できませんでした: {e}"));
                None
            }
        }
    })
    .flatten();
    if let Some(n) = added {
        set_status(&format!(
            "マルチレイアウト（{} 種）を設定しました（項目の列を {n} 個足しました）。A 列に行のレイアウトの名前を入れてから、項目を入力します",
            spec.multi.len()
        ));
    }
}

/// ファイル > 固定長ファイルを開く（マルチレイアウト。`add` なら、今の文書にシートとして追加）。
pub(super) fn open_multi(add: bool) {
    if !add && !confirm_discard() {
        return;
    }
    let Some((frame, ctx)) = with(|a| (a.frame, a.ctx.clone())) else {
        return;
    };
    let Some(path) = pick_file(
        frame,
        &[
            ("固定長ファイル (*.dat;*.txt;*.bin)", "*.dat;*.txt;*.bin"),
            ("すべてのファイル (*.*)", "*.*"),
        ],
    ) else {
        return;
    };
    open_multi_path(frame, ctx, &path, add);
}

fn open_multi_path(frame: HWND, ctx: Arc<SheetCtx>, path: &Path, add: bool) {
    let read = (|| -> std::io::Result<(Vec<u8>, u64)> {
        use std::io::Read;
        let f = std::fs::File::open(path)?;
        let len = f.metadata()?.len();
        let mut sample = Vec::new();
        f.take(1 << 20).read_to_end(&mut sample)?;
        Ok((sample, len))
    })();
    let open = match read {
        Ok(x) => x,
        Err(e) => {
            error_box(
                frame,
                &format!("{} を開けませんでした。\n{e}", path.display()),
            );
            return;
        }
    };
    let title = format!(
        "固定長ファイルを開く（マルチレイアウト） - {}",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    );
    let Some(spec) = multi_dialog(frame, Some(open), &title) else {
        return;
    };
    remember(&spec);
    let p = path.to_owned();
    let s2 = spec.clone();
    let ctx2 = ctx.clone();
    set_status(&format!(
        "{} を取り込んでいます…（Esc で中止）",
        path.display()
    ));
    let r = crate::remote::wait(&set_status, move |w| {
        fixed::import_multi(&ctx2, &p, &s2, &|done, total| {
            w.report(format!(
                "取り込み中… {}%（Esc で中止）",
                done * 100 / total.max(1)
            ));
            !w.cancelled()
        })
    });
    match r {
        Ok((sheet, rep)) => {
            with(|a| a.place_fixed_sheet(sheet, path, add));
            set_status(&format!(
                "{} レコードを取り込みました（{}・区切り {}）。どの行もレイアウト未確定です。A 列にレイアウトの名前を入れるか、データ > 行のレイアウトを指定 で選んでください",
                crate::util::group_digits(rep.records),
                spec.codec.charset.name(),
                spec.separator.label()
            ));
        }
        Err(e) => {
            set_status("");
            if e.kind() != std::io::ErrorKind::Interrupted {
                error_box(
                    frame,
                    &format!("{} を取り込めませんでした。\n{e}", path.display()),
                );
            }
        }
    }
}

const R_LIST: u16 = 10;

struct RowState {
    names: Vec<String>,
    result: Option<String>,
}

extern "system" fn row_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = state::<RowState>(hwnd);
                add_string(hwnd, R_LIST, LB_ADDSTRING, "（レイアウト未確定に戻す）");
                for n in &st.names {
                    add_string(hwnd, R_LIST, LB_ADDSTRING, n);
                }
                send(hwnd, R_LIST, LB_SETCURSEL, 1.min(st.names.len()), 0);
                1
            }
            WM_COMMAND if GetWindowLongPtrW(hwnd, GWLP_USERDATA) == 0 => 0,
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let code = crate::hiword(wparam.0);
                let st = state::<RowState>(hwnd);
                let ok = id == IDOK_ || (id == R_LIST && code == LBN_DBLCLK);
                if ok {
                    let i = send(hwnd, R_LIST, LB_GETCURSEL, 0, 0);
                    if let Ok(i) = usize::try_from(i) {
                        st.result = Some(match i {
                            0 => String::new(),
                            k => st.names.get(k - 1).cloned().unwrap_or_default(),
                        });
                        let _ = EndDialog(hwnd, IDOK_ as isize);
                    }
                    return 1;
                }
                if id == IDCANCEL_ {
                    let _ = EndDialog(hwnd, IDCANCEL_ as isize);
                    return 1;
                }
                0
            }
            _ => 0,
        }
    }
}

/// データ > 行のレイアウトを指定（選んだ行）。
pub(super) fn row_layout_dialog() {
    let Some((frame, all, names, rows)) = with(|a| {
        a.end_edit(true);
        let s = a.sheet();
        let all: Vec<String> = s
            .fixed
            .as_deref()
            .map(|f| f.multi.iter().map(|m| m.name.to_string()).collect())
            .unwrap_or_default();
        let (t, _, b, _) = a.selection();
        let last = s.extent().0.max(t + 1) - 1;
        let rows = (t, b.min(last));
        // 選んだどの行もデコードエラーなしで読めるレイアウトだけを選択肢にする
        let mut names = all.clone();
        for r in rows.0..=rows.1 {
            if names.is_empty() {
                break;
            }
            if matches!(s.place(r, 0), yy_sheet::Place::Header(_)) {
                continue;
            }
            let ok = fixed::usable_layouts(&a.ctx, s, r).unwrap_or_default();
            names.retain(|n| ok.iter().any(|o| **o == **n));
        }
        (a.frame, all, names, rows)
    }) else {
        return;
    };
    if all.is_empty() {
        info_box(
            frame,
            "このシートはマルチレイアウトではありません。\nデータ > マルチレイアウトの設定 で、レイアウトを登録してください。",
        );
        return;
    }
    if names.is_empty() {
        info_box(
            frame,
            &format!(
                "選んだ行をデコードエラーなしで読めるレイアウトがありません（登録しているレイアウト: {}）。\n\
                 1 行ずつ選ぶか、レイアウト（コピーブック）を確かめてください。レイアウト未確定には戻せます。",
                all.join("・")
            ),
        );
    }
    let mut t = Template::dialog("行のレイアウトを指定", 200, 190);
    label(
        &mut t,
        7,
        7,
        186,
        0,
        &format!(
            "選んだ {} 行をデコードエラーなしで読めるレイアウト:",
            rows.1 - rows.0 + 1
        ),
    );
    t.item(
        (WS_BORDER | WS_TABSTOP | WS_VSCROLL).0 | LBS_NOTIFY as u32,
        7,
        20,
        186,
        140,
        R_LIST,
        CLASS_LISTBOX,
        "",
    );
    button(&mut t, 89, 168, 50, IDOK_, "OK", true);
    button(&mut t, 143, 168, 50, IDCANCEL_, "キャンセル", false);
    let mut st = RowState {
        names,
        result: None,
    };
    run(&t, frame, &mut st, Some(row_proc));
    let Some(name) = st.result else {
        return;
    };
    with(|a| {
        let sheet = a.sheet;
        let mut bad = None;
        let res = a.doc.edit(|b, ctx| {
            let sh = &mut b.sheets[sheet];
            for r in rows.0..=rows.1 {
                if matches!(sh.place(r, 0), yy_sheet::Place::Header(_)) {
                    continue;
                }
                if let Err(e) = fixed::set_row_layout(ctx, sh, r, &name) {
                    bad = Some(e.clone());
                    return Err(std::io::Error::other(e));
                }
            }
            Ok(())
        });
        a.after_edit();
        a.sync_formula();
        a.invalidate();
        match (res, bad) {
            (Ok(()), _) => set_status(&format!(
                "{} 行のレイアウトを{}にしました",
                rows.1 - rows.0 + 1,
                if name.is_empty() {
                    "未確定".to_string()
                } else {
                    format!(" {name} ")
                }
            )),
            (Err(_), Some(e)) => error_box(a.frame, &e),
            (Err(e), None) => error_box(a.frame, &format!("指定できませんでした: {e}")),
        }
    });
}

//! 大量のデータの操作の UI（15 章 9.4）: 検索（Ctrl+F・F3）、置換（Ctrl+H）、重複の削除、列の型の変更。
//!
//! 処理は `yy_sheet::bulk` がチャンクごとに並列に行い、ここではダイアログとバックグラウンドの実行、
//! 結果の反映（Undo できる編集）を受け持つ。どれも表（大量のデータ）の列が対象。

use std::collections::HashSet;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{BST_CHECKED, CheckDlgButton, IsDlgButtonChecked};
use windows::Win32::UI::WindowsAndMessaging::*;
use yy_sheet::bulk::{self, Convert, Replace};
use yy_sheet::{Table, View};

use super::filter::{button, dlg_text, edit, label, message, run, send, state};
use super::view::{col_label, current, run_bg};
use super::*;
use crate::goto::{CLASS_BUTTON, CLASS_STATIC, Template};

const CLASS_LISTBOX: u16 = 0x0083;
const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;

/// 表の行 → 格子の行（絞り込みで隠れていれば `None`）。
fn grid_row_of(s: &yy_sheet::Sheet, table_row: u64) -> Option<u64> {
    let head = s.table.header as u64;
    match &s.view.rows {
        None => Some(table_row + head),
        Some(rows) => rows
            .iter()
            .position(|&r| r as u64 == table_row)
            .map(|i| i as u64 + head),
    }
}

// ---- 検索 --------------------------------------------------------------------------

/// 検索の文字列を尋ねて探す（Ctrl+F）。
pub(super) fn find_dialog() {
    let Some((frame, last)) = with(|a| {
        a.end_edit(true);
        (
            a.frame,
            a.last_find
                .as_ref()
                .map(|f| f.find.clone())
                .unwrap_or_default(),
        )
    }) else {
        return;
    };
    let Some(text) = crate::goto::prompt_text(
        frame,
        "検索",
        "表から探す文字列（大文字・小文字は区別しません）:",
        &last,
    ) else {
        return;
    };
    if text.is_empty() {
        return;
    }
    with(|a| {
        a.last_find = Some(Replace {
            find: text,
            with: String::new(),
            regex: false,
            case: false,
            whole: false,
        })
    });
    find_next();
}

/// 前の検索の次を探す（F3）。
pub(super) fn find_next() {
    let Some(Some((q, table, from))) = with(|a| {
        a.end_edit(true);
        let q = a.last_find.clone()?;
        let s = a.sheet();
        // 見出しの上や表の外からは、表の先頭から（先頭のセルも含めて）
        let from = match s.place(a.cur.0, a.cur.1) {
            yy_sheet::Place::Data(r, c) => Some((r, c)),
            _ => None,
        };
        Some((q, s.table.clone(), from))
    }) else {
        find_dialog();
        return;
    };
    if table.cols() == 0 {
        set_status("検索する表がありません");
        return;
    }
    let Some(ctx) = with(|a| a.ctx.clone()) else {
        return;
    };
    let t2 = table.clone();
    let q2 = q.clone();
    let start = from.unwrap_or((u64::MAX, u32::MAX));
    let found = run_bg("探しています…", move || {
        let from = if start.0 == u64::MAX {
            // 先頭のセルから探す: 先頭のセルの「前」から
            None
        } else {
            Some(start)
        };
        match from {
            Some(f) => bulk::find_next(&ctx, &t2, f, &q2),
            None => {
                // (0,0) 自体も候補にする
                let first = t2.columns[0].get(&ctx, 0)?;
                let rep = bulk::Replacer::new(&q2).map_err(std::io::Error::other)?;
                if !first.is_empty() && rep.is_match(&first.general_text()) {
                    return Ok(Some((0, 0)));
                }
                bulk::find_next(&ctx, &t2, (0, 0), &q2)
            }
        }
        .map_err(std::io::Error::other)
    });
    let Some(found) = found else {
        return;
    };
    match found {
        None => set_status(&format!("「{}」は見つかりませんでした", q.find)),
        Some((tr, c)) => {
            with(|a| match grid_row_of(a.sheet(), tr) {
                Some(g) => {
                    a.move_to(g, c, false);
                    set_status(&format!("「{}」が見つかりました（F3 で次）", q.find));
                }
                None => set_status(&format!(
                    "「{}」は表の {} 行目にありますが、絞り込みで隠れています",
                    q.find,
                    tr + 1
                )),
            });
        }
    }
}

// ---- 置換 --------------------------------------------------------------------------

const R_FIND: u16 = 10;
const R_WITH: u16 = 11;
const R_REGEX: u16 = 12;
const R_CASE: u16 = 13;
const R_WHOLE: u16 = 14;
const R_ALL_COLS: u16 = 15;

struct ReplaceState {
    initial: Replace,
    selection: String,
    result: Option<(Replace, bool)>,
}

/// 置換のダイアログ（Ctrl+H）。選んだ列（または表のすべての列）の文字列を置換する。
pub(super) fn replace_dialog() {
    let Some((sheet, table, _, ctx)) = current() else {
        return;
    };
    let Some((frame, cols, last)) = with(|a| {
        let (_, l, _, r) = a.selection();
        let n = a.sheet().table.cols();
        let cols: Vec<u32> = (l..=r.min(n.saturating_sub(1)))
            .filter(|&c| c < n)
            .collect();
        (a.frame, cols, a.last_find.clone())
    }) else {
        return;
    };
    if table.cols() == 0 {
        info_box(frame, "置換する表がありません。");
        return;
    }
    let mut t = Template::dialog("置換", 260, 140);
    label(&mut t, 7, 9, 60, 0, "検索する文字列:");
    edit(&mut t, 70, 7, 183, R_FIND);
    label(&mut t, 7, 27, 60, 0, "置換後の文字列:");
    edit(&mut t, 70, 25, 183, R_WITH);
    let check = |t: &mut Template, x, y, cx, id, text: &str| {
        t.item(
            WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
            x,
            y,
            cx,
            10,
            id,
            CLASS_BUTTON,
            text,
        );
    };
    check(
        &mut t,
        7,
        45,
        246,
        R_REGEX,
        "正規表現（置換後の文字列で $1 などの組を使える）",
    );
    check(&mut t, 7, 59, 120, R_CASE, "大文字と小文字を区別する");
    check(&mut t, 130, 59, 120, R_WHOLE, "セル全体が一致するもの");
    check(
        &mut t,
        7,
        77,
        246,
        R_ALL_COLS,
        "表のすべての列（外すと選んだ列だけ）",
    );
    t.item(0, 7, 93, 246, 20, 0, CLASS_STATIC, "");
    button(&mut t, 149, 120, 50, IDOK_, "すべて置換", true);
    button(&mut t, 203, 120, 50, IDCANCEL_, "キャンセル", false);
    let names: Vec<String> = cols.iter().map(|&c| col_label(&table, c)).collect();
    let mut st = ReplaceState {
        initial: last.unwrap_or(Replace {
            find: String::new(),
            with: String::new(),
            regex: false,
            case: false,
            whole: false,
        }),
        selection: if names.is_empty() {
            "選んだ列: なし（表のすべての列を置換します）".into()
        } else {
            format!("選んだ列: {}", names.join("、"))
        },
        result: None,
    };
    run(&t, frame, &mut st, Some(replace_proc));
    let Some((rep, all)) = st.result else {
        return;
    };
    let cols: Vec<u32> = if all || cols.is_empty() {
        (0..table.cols()).collect()
    } else {
        cols
    };
    with(|a| a.last_find = Some(rep.clone()));
    let t2 = table.clone();
    let Some((new_table, n)) = run_bg("置換しています…", move || {
        bulk::replace(&ctx, &t2, &cols, &rep).map_err(std::io::Error::other)
    }) else {
        return;
    };
    if n == 0 {
        set_status("一致するセルはありませんでした");
        return;
    }
    replace_table(sheet, &table, new_table, false);
    set_status(&format!(
        "{} セルを置換しました（元に戻すは Ctrl+Z）",
        crate::util::group_digits(n)
    ));
}

extern "system" fn replace_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = state::<ReplaceState>(hwnd);
                let set = |id: u16, s: &str| {
                    let _ = SetDlgItemTextW(hwnd, id as i32, &windows::core::HSTRING::from(s));
                };
                set(R_FIND, &st.initial.find);
                set(R_WITH, &st.initial.with);
                for (id, on) in [
                    (R_REGEX, st.initial.regex),
                    (R_CASE, st.initial.case),
                    (R_WHOLE, st.initial.whole),
                ] {
                    if on {
                        let _ = CheckDlgButton(hwnd, id as i32, BST_CHECKED);
                    }
                }
                if let Ok(h) = GetDlgItem(Some(hwnd), 0) {
                    let _ = SetWindowTextW(h, &windows::core::HSTRING::from(st.selection.as_str()));
                }
                1
            }
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let st = state::<ReplaceState>(hwnd);
                match id {
                    IDOK_ => {
                        let on = |id: u16| IsDlgButtonChecked(hwnd, id as i32) == BST_CHECKED.0;
                        let rep = Replace {
                            find: dlg_text(hwnd, R_FIND),
                            with: dlg_text(hwnd, R_WITH),
                            regex: on(R_REGEX),
                            case: on(R_CASE),
                            whole: on(R_WHOLE),
                        };
                        if let Err(e) = bulk::Replacer::new(&rep) {
                            message(hwnd, &e);
                            return 1;
                        }
                        st.result = Some((rep, on(R_ALL_COLS)));
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

/// 表を置き換える（Undo できる）。`rows_changed` なら絞り込み・並べ替えの表示を外す。
fn replace_table(sheet: usize, old: &Table, new_table: Table, rows_changed: bool) {
    with(|a| {
        if a.doc.book.sheets.get(sheet).map(|s| s.table.rows) != Some(old.rows) {
            set_status("処理の間に表が変わったので、結果を捨てました");
            return;
        }
        let _ = a.doc.edit(|b, _| {
            let s = &mut b.sheets[sheet];
            s.table = new_table;
            if rows_changed {
                s.view = View::default();
            }
            s.formulas.touch_all();
            Ok(())
        });
        if rows_changed {
            a.top = 0;
            a.cur = (0, a.cur.1);
            a.anchor = a.cur;
        }
        a.after_edit();
    });
}

// ---- 重複の削除 ----------------------------------------------------------------------

const D_LIST: u16 = 10;
const D_ALL: u16 = 11;
const D_NONE: u16 = 12;

struct DupState {
    names: Vec<String>,
    initial: HashSet<u32>,
    result: Option<Vec<u32>>,
}

/// 重複の削除: 選んだ列の値の組が前の行と同じ行を消す（大文字・小文字は区別しない）。
pub(super) fn remove_duplicates() {
    let Some((sheet, table, _, ctx)) = current() else {
        return;
    };
    let Some((frame, sel)) = with(|a| {
        let (_, l, _, r) = a.selection();
        (a.frame, (l, r))
    }) else {
        return;
    };
    if table.cols() == 0 || table.rows == 0 {
        info_box(frame, "重複を削除する表がありません。");
        return;
    }
    let names: Vec<String> = (0..table.cols()).map(|c| col_label(&table, c)).collect();
    // 1 列だけ選んでいればすべての列、そうでなければ選んだ列を初めに選んでおく
    let initial: HashSet<u32> = if sel.0 == sel.1 {
        (0..table.cols()).collect()
    } else {
        (sel.0..=sel.1).filter(|&c| c < table.cols()).collect()
    };
    let mut t = Template::dialog("重複の削除", 220, 196);
    label(
        &mut t,
        7,
        7,
        206,
        0,
        "値の組で比べる列（同じ組の 2 つ目からの行を消します）:",
    );
    t.item(
        (WS_BORDER | WS_TABSTOP | WS_VSCROLL).0
            | (LBS_MULTIPLESEL | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
        7,
        20,
        206,
        130,
        D_LIST,
        CLASS_LISTBOX,
        "",
    );
    button(&mut t, 7, 155, 54, D_ALL, "すべて選択", false);
    button(&mut t, 65, 155, 54, D_NONE, "すべて解除", false);
    button(&mut t, 109, 176, 50, IDOK_, "OK", true);
    button(&mut t, 163, 176, 50, IDCANCEL_, "キャンセル", false);
    let mut st = DupState {
        names,
        initial,
        result: None,
    };
    run(&t, frame, &mut st, Some(dup_proc));
    let Some(cols) = st.result else {
        return;
    };
    let t2 = table.clone();
    let Some(result) = run_bg("重複を探しています…", move || {
        let keep = bulk::unique_rows(&ctx, &t2, &cols)?;
        if keep.len() as u64 == t2.rows {
            return Ok(None);
        }
        Ok(Some((
            bulk::pick(&ctx, &t2, &keep)?,
            t2.rows - keep.len() as u64,
        )))
    }) else {
        return;
    };
    match result {
        None => set_status("重複する行はありませんでした"),
        Some((new_table, removed)) => {
            replace_table(sheet, &table, new_table, true);
            set_status(&format!(
                "重複する {} 行を削除しました（元に戻すは Ctrl+Z）",
                crate::util::group_digits(removed)
            ));
        }
    }
}

extern "system" fn dup_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = state::<DupState>(hwnd);
                for (i, n) in st.names.iter().enumerate() {
                    let w = windows::core::HSTRING::from(n.as_str());
                    let at = send(hwnd, D_LIST, LB_ADDSTRING, 0, w.as_ptr() as isize);
                    if st.initial.contains(&(i as u32)) {
                        send(hwnd, D_LIST, LB_SETSEL, 1, at);
                    }
                }
                1
            }
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let st = state::<DupState>(hwnd);
                match id {
                    D_ALL | D_NONE => {
                        send(hwnd, D_LIST, LB_SETSEL, (id == D_ALL) as usize, -1);
                        1
                    }
                    IDOK_ => {
                        let n = send(hwnd, D_LIST, LB_GETCOUNT, 0, 0).max(0) as usize;
                        let cols: Vec<u32> = (0..n)
                            .filter(|&i| send(hwnd, D_LIST, LB_GETSEL, i, 0) > 0)
                            .map(|i| i as u32)
                            .collect();
                        if cols.is_empty() {
                            message(hwnd, "列を 1 つ以上選んでください。");
                            return 1;
                        }
                        st.result = Some(cols);
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

// ---- 列の型の変更 --------------------------------------------------------------------

/// 選んだ列（表の列）の型を変える。
pub(super) fn convert_columns(to: Convert) {
    let Some((sheet, table, _, ctx)) = current() else {
        return;
    };
    let Some((frame, cols, sys)) = with(|a| {
        let (_, l, _, r) = a.selection();
        let n = a.sheet().table.cols();
        let cols: Vec<u32> = (l..=r).filter(|&c| c < n).collect();
        (a.frame, cols, a.sys())
    }) else {
        return;
    };
    if cols.is_empty() {
        info_box(frame, "表の列を選んでください。");
        return;
    }
    let t2 = table.clone();
    let Some((new_table, n)) = run_bg("列の型を変えています…", move || {
        let mut columns = (*t2.columns).clone();
        let mut total = 0;
        for &c in &cols {
            let (nc, k) = bulk::convert(&ctx, &t2.columns[c as usize], to, sys)?;
            if k > 0 {
                columns[c as usize] = nc;
                total += k;
            }
        }
        Ok((
            Table {
                columns: std::sync::Arc::new(columns),
                ..t2
            },
            total,
        ))
    }) else {
        return;
    };
    if n == 0 {
        set_status("変える値はありませんでした");
        return;
    }
    replace_table(sheet, &table, new_table, false);
    set_status(&format!(
        "{} セルを{}にしました（元に戻すは Ctrl+Z）",
        crate::util::group_digits(n),
        match to {
            Convert::Number => "数値",
            Convert::Text => "文字列",
        }
    ));
}

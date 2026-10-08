//! 区切りを指定して開く・書き出す（15 章 6.7）。
//!
//! 区切り文字（タブ・`|`・US など）とレコードの終わり（改行・CR・RS など）を、候補から選ぶか書いて指定する。
//! 制御コードは `<US>`・`<RS>`・`<TAB>`・`<CR>`・`<LF>`・`<NUL>`・`<0x1F>`、または `\t`・`\x1F` で書ける
//! （[`yy_delimited::parse_bytes`]）。

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{BST_CHECKED, CheckDlgButton, IsDlgButtonChecked};
use windows::Win32::UI::WindowsAndMessaging::*;
use yy_delimited::{Dialect, describe_bytes, parse_bytes};
use yy_encoding::Encoding;
use yy_sheet::csv::{CsvOptions, ExportOptions, RecordEnd, check_separators};

use super::filter::{add_string, button, combo, label, message, run, send, state};
use super::*;
use crate::goto::{CLASS_BUTTON, Template};

const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;
/// ダイアログの部品の種類（組み合わせボックス）
const CLASS_COMBOBOX: u16 = 0x0085;

pub(super) const ID_OPEN_DELIMITED: u16 = 210;
pub(super) const ID_EXPORT_DELIMITED: u16 = 211;

const D_ENCODING: u16 = 1001;
const D_DELIM: u16 = 1002;
const D_END: u16 = 1003;
const D_QUOTE: u16 = 1004;
const D_HEADER: u16 = 1005;
const D_TEXT: u16 = 1006;
const D_BOM: u16 = 1007;

/// 区切り文字の候補（表示・バイト列）。
const DELIMS: [(&str, &[u8]); 6] = [
    ("カンマ ,", b","),
    ("タブ <TAB>", b"\t"),
    ("セミコロン ;", b";"),
    ("パイプ |", b"|"),
    ("空白 <SP>", b" "),
    ("US（ASCII の区切り） <US>", b"\x1f"),
];

/// レコードの終わりの候補（開くとき）。`None` は改行（LF・CR LF）。
const OPEN_ENDS: [(&str, Option<&[u8]>); 6] = [
    ("改行（LF・CR LF）", None),
    ("CR だけ <CR>", Some(b"\r")),
    ("RS（ASCII の区切り） <RS>", Some(b"\x1e")),
    ("GS <GS>", Some(b"\x1d")),
    ("NUL <NUL>", Some(b"\x00")),
    (
        "CR LF だけ（LF だけは値の中の文字） <CR><LF>",
        Some(b"\r\n"),
    ),
];

/// レコードの終わりの候補（書き出すとき）。`None` と真偽は改行（CR LF か）。
const EXPORT_ENDS: [(&str, Option<&[u8]>, bool); 6] = [
    ("CR LF（Windows）", None, true),
    ("LF（Unix）", None, false),
    ("CR だけ <CR>", Some(b"\r"), false),
    ("RS（ASCII の区切り） <RS>", Some(b"\x1e"), false),
    ("GS <GS>", Some(b"\x1d"), false),
    ("NUL <NUL>", Some(b"\x00"), false),
];

const QUOTES: [(&str, Option<u8>); 3] = [
    ("\"（RFC 4180）", Some(b'"')),
    ("'", Some(b'\'')),
    ("使わない", None),
];

/// ダイアログで決めたこと。
#[derive(Clone)]
struct Settings {
    dialect: Dialect,
    end: RecordEnd,
    /// 改行で終わるとき CR LF で書く
    crlf: bool,
    /// `None` は自動判別（開くとき）
    encoding: Option<Encoding>,
    header: bool,
    all_text: bool,
    bom: bool,
}

struct DlgState {
    export: bool,
    init: Settings,
    result: Option<Settings>,
}

/// ファイル・ワークスペースのメニューの操作（扱ったら `true`）。
pub(super) fn command(id: u16) -> bool {
    match id {
        ID_OPEN_DELIMITED => {
            if !confirm_discard() {
                return true;
            }
            let Some(frame) = with(|a| a.frame) else {
                return true;
            };
            if let Some(p) = pick_open(frame) {
                open_delimited(&p);
            }
        }
        ID_EXPORT_DELIMITED => {
            export_delimited();
        }
        _ => return false,
    }
    true
}

fn pick_open(owner: HWND) -> Option<PathBuf> {
    super::fixedui::pick_file(
        owner,
        &[
            (
                "区切り文字形式 (*.csv;*.tsv;*.txt;*.dat)",
                "*.csv;*.tsv;*.txt;*.dat",
            ),
            ("すべてのファイル (*.*)", "*.*"),
        ],
    )
}

/// 区切りを尋ねてから、`path`（手元のファイル）を取り込む。
pub(super) fn open_delimited(path: &Path) {
    let Some(frame) = with(|a| a.frame) else {
        return;
    };
    // 推定した値を初めに出す
    let guess = yy_sheet::csv::preview(path).ok();
    let init = match &guess {
        Some(pv) => Settings {
            dialect: pv.options.dialect,
            end: pv.options.record_end.clone(),
            crlf: true,
            encoding: None,
            header: pv.options.header,
            all_text: false,
            bom: false,
        },
        None => Settings {
            dialect: Dialect::csv(),
            end: RecordEnd::Newline,
            crlf: true,
            encoding: None,
            header: true,
            all_text: false,
            bom: false,
        },
    };
    let title = format!(
        "区切りを指定して開く - {}",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    );
    let Some(s) = dialog(frame, &title, false, init) else {
        return;
    };
    let pv = match yy_sheet::csv::preview_as(path, Some(s.dialect), Some(s.end), s.encoding) {
        Ok(p) => p,
        Err(e) => {
            error_box(
                frame,
                &format!("{} を開けませんでした。\n{e}", path.display()),
            );
            return;
        }
    };
    let opts = CsvOptions {
        header: s.header,
        all_text: s.all_text,
        // 見出しを変えたら、型は見出しを除いた見本で推定し直す
        types: if s.header == pv.options.header {
            pv.options.types.clone()
        } else {
            Vec::new()
        },
        ..pv.options
    };
    import_csv(path, opts);
}

/// ファイル > 区切りを指定して書き出し。
fn export_delimited() -> bool {
    let Some((frame, path, origin)) = with(|a| {
        a.end_edit(true);
        (a.frame, a.doc.path.clone(), a.origin.clone())
    }) else {
        return false;
    };
    let Some(target) = show_save(frame, path.as_deref(), true) else {
        return false;
    };
    let base = default_export(&origin, path.as_deref(), &target);
    let init = Settings {
        dialect: base.dialect,
        end: base.record_end.clone(),
        crlf: base.crlf,
        encoding: Some(base.encoding),
        header: true,
        all_text: false,
        bom: base.bom,
    };
    let title = format!(
        "区切りを指定して書き出す - {}",
        target
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    );
    let Some(s) = dialog(frame, &title, true, init) else {
        return false;
    };
    let opts = ExportOptions {
        dialect: s.dialect,
        encoding: s.encoding.unwrap_or(Encoding::Utf8),
        record_end: s.end,
        crlf: s.crlf,
        bom: s.bom,
        ..base
    };
    export_csv_with(Some(target), Some(opts))
}

fn dialog(owner: HWND, title: &str, export: bool, init: Settings) -> Option<Settings> {
    let (w, h) = (320i16, 170i16);
    let mut t = Template::dialog(title, w, h);
    let x = 92;
    let cw = w - x - 7;
    label(&mut t, 7, 9, 82, 0, "文字コード:");
    combo(&mut t, x, 7, cw, D_ENCODING);
    label(&mut t, 7, 27, 82, 0, "区切り文字:");
    editable_combo(&mut t, x, 25, cw, D_DELIM);
    label(&mut t, 7, 45, 82, 0, "レコードの終わり:");
    editable_combo(&mut t, x, 43, cw, D_END);
    label(&mut t, 7, 63, 82, 0, "引用符:");
    combo(&mut t, x, 61, 120, D_QUOTE);
    if export {
        check(&mut t, x, 80, D_BOM, "BOM を付ける（Unicode のとき）");
    } else {
        check(&mut t, x, 80, D_HEADER, "1 行目を見出しにする");
        check(&mut t, x, 93, D_TEXT, "すべて文字列として取り込む");
    }
    label(
        &mut t,
        7,
        110,
        w - 14,
        0,
        "候補から選ぶか、書いて指定します。制御コードは <US> <RS> <TAB> <CR> <LF> <NUL>",
    );
    label(
        &mut t,
        7,
        121,
        w - 14,
        0,
        "<0x1F> のように名前・16 進数で、または \\t \\x1F のように書けます（例: <US>、<RS>、||）。",
    );
    let ok = if export { "書き出す" } else { "開く" };
    button(&mut t, w - 114, h - 21, 50, IDOK_, ok, true);
    button(&mut t, w - 57, h - 21, 50, IDCANCEL_, "キャンセル", false);
    let mut st = DlgState {
        export,
        init,
        result: None,
    };
    run(&t, owner, &mut st, Some(dialog_proc));
    st.result
}

/// 書き込める組み合わせボックス（候補から選ぶか書く）。
fn editable_combo(t: &mut Template, x: i16, y: i16, cx: i16, id: u16) {
    t.item(
        (WS_TABSTOP | WS_VSCROLL).0 | (CBS_DROPDOWN | CBS_AUTOHSCROLL) as u32,
        x,
        y,
        cx,
        160,
        id,
        CLASS_COMBOBOX,
        "",
    );
}

fn check(t: &mut Template, x: i16, y: i16, id: u16, text: &str) {
    t.item(
        WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
        x,
        y,
        210,
        10,
        id,
        CLASS_BUTTON,
        text,
    );
}

fn item_text(hwnd: HWND, id: u16) -> String {
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetDlgItemTextW(hwnd, id as i32, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn set_item_text(hwnd: HWND, id: u16, text: &str) {
    unsafe {
        let _ = SetDlgItemTextW(hwnd, id as i32, &windows::core::HSTRING::from(text));
    }
}

/// 文字コードの欄の項目（開くときは先頭が「自動判別」）。
fn encodings(export: bool) -> Vec<Option<Encoding>> {
    let mut v: Vec<Option<Encoding>> = if export { Vec::new() } else { vec![None] };
    v.extend(Encoding::all().into_iter().map(Some));
    v
}

/// 区切り文字の欄の文字をバイト列にする（候補の表示ならその値）。
fn delim_of(text: &str) -> std::result::Result<Vec<u8>, String> {
    let text = text.trim_end_matches(['\r', '\n']);
    if let Some((_, b)) = DELIMS.iter().find(|(l, _)| *l == text) {
        return Ok(b.to_vec());
    }
    parse_bytes(text).map_err(|e| format!("区切り文字: {e}"))
}

/// レコードの終わりの欄の文字（`(終わり, CR LF か)`）。
fn end_of(text: &str, export: bool) -> std::result::Result<(RecordEnd, bool), String> {
    let text = text.trim_end_matches(['\r', '\n']);
    if export {
        if let Some((_, b, crlf)) = EXPORT_ENDS.iter().find(|(l, _, _)| *l == text) {
            return Ok((
                b.map_or(RecordEnd::Newline, |b| RecordEnd::Custom(b.to_vec())),
                *crlf,
            ));
        }
    } else if let Some((_, b)) = OPEN_ENDS.iter().find(|(l, _)| *l == text) {
        return Ok((
            b.map_or(RecordEnd::Newline, |b| RecordEnd::Custom(b.to_vec())),
            true,
        ));
    }
    let b = parse_bytes(text).map_err(|e| format!("レコードの終わり: {e}"))?;
    // CR LF・LF と書いたら改行として扱う（書き出し）
    Ok(match (export, b.as_slice()) {
        (true, b"\r\n") => (RecordEnd::Newline, true),
        (true, b"\n") => (RecordEnd::Newline, false),
        _ => (RecordEnd::Custom(b), true),
    })
}

/// 初めに出す区切り文字の欄の文字。
fn delim_text(d: &[u8]) -> String {
    DELIMS
        .iter()
        .find(|(_, b)| *b == d)
        .map_or_else(|| describe_bytes(d), |(l, _)| (*l).to_owned())
}

/// 初めに出すレコードの終わりの欄の文字。
fn end_text(end: &RecordEnd, crlf: bool, export: bool) -> String {
    let custom = end.custom();
    if export {
        EXPORT_ENDS
            .iter()
            .find(|(_, b, c)| *b == custom && (custom.is_some() || *c == crlf))
            .map_or_else(
                || describe_bytes(custom.unwrap_or(b"\r\n")),
                |(l, _, _)| (*l).to_owned(),
            )
    } else {
        OPEN_ENDS.iter().find(|(_, b)| *b == custom).map_or_else(
            || describe_bytes(custom.unwrap_or(b"\n")),
            |(l, _)| (*l).to_owned(),
        )
    }
}

/// 欄の値から設定を作る（正しくなければ知らせる文）。
fn read_settings(hwnd: HWND, st: &DlgState) -> std::result::Result<Settings, String> {
    let delim = delim_of(&item_text(hwnd, D_DELIM))?;
    let (end, crlf) = end_of(&item_text(hwnd, D_END), st.export)?;
    let qi = send(hwnd, D_QUOTE, CB_GETCURSEL, 0, 0).max(0) as usize;
    let quote = QUOTES.get(qi).map_or(Some(b'"'), |q| q.1);
    let dialect = Dialect::with_any_delimiter(&delim, quote).ok_or_else(|| {
        if delim.len() > 4 {
            "区切り文字は 4 バイトまでにしてください。".to_owned()
        } else {
            "区切り文字に引用符を含められません。".to_owned()
        }
    })?;
    check_separators(&dialect, &end)?;
    let ei = send(hwnd, D_ENCODING, CB_GETCURSEL, 0, 0).max(0) as usize;
    let encoding = encodings(st.export).get(ei).copied().flatten();
    let checked = |id: u16| unsafe { IsDlgButtonChecked(hwnd, id as i32) } == BST_CHECKED.0;
    Ok(Settings {
        dialect,
        end,
        crlf,
        encoding,
        header: checked(D_HEADER),
        all_text: checked(D_TEXT),
        bom: checked(D_BOM),
    })
}

extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = state::<DlgState>(hwnd);
                let init = st.init.clone();
                for (i, e) in encodings(st.export).iter().enumerate() {
                    let text = e.map_or_else(|| "自動判別".to_owned(), |e| e.label());
                    add_string(hwnd, D_ENCODING, CB_ADDSTRING, &text);
                    if *e == init.encoding {
                        send(hwnd, D_ENCODING, CB_SETCURSEL, i, 0);
                    }
                }
                for (l, _) in DELIMS {
                    add_string(hwnd, D_DELIM, CB_ADDSTRING, l);
                }
                set_item_text(hwnd, D_DELIM, &delim_text(init.dialect.delimiter()));
                if st.export {
                    for (l, _, _) in EXPORT_ENDS {
                        add_string(hwnd, D_END, CB_ADDSTRING, l);
                    }
                } else {
                    for (l, _) in OPEN_ENDS {
                        add_string(hwnd, D_END, CB_ADDSTRING, l);
                    }
                }
                set_item_text(hwnd, D_END, &end_text(&init.end, init.crlf, st.export));
                for (i, (l, q)) in QUOTES.iter().enumerate() {
                    add_string(hwnd, D_QUOTE, CB_ADDSTRING, l);
                    if *q == init.dialect.quote {
                        send(hwnd, D_QUOTE, CB_SETCURSEL, i, 0);
                    }
                }
                let set = |id: u16, on: bool| {
                    if on {
                        let _ = CheckDlgButton(hwnd, id as i32, BST_CHECKED);
                    }
                };
                set(D_HEADER, init.header);
                set(D_TEXT, init.all_text);
                set(D_BOM, init.bom);
                1
            }
            WM_COMMAND if GetWindowLongPtrW(hwnd, GWLP_USERDATA) == 0 => 0,
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let st = state::<DlgState>(hwnd);
                match id {
                    IDOK_ => match read_settings(hwnd, st) {
                        Ok(s) => {
                            st.result = Some(s);
                            let _ = EndDialog(hwnd, IDOK_ as isize);
                            1
                        }
                        Err(e) => {
                            message(hwnd, &e);
                            1
                        }
                    },
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

    #[test]
    fn reads_presets_and_typed_separators() {
        assert_eq!(delim_of("タブ <TAB>").unwrap(), b"\t");
        assert_eq!(delim_of("<US>").unwrap(), b"\x1f");
        assert_eq!(delim_of("||").unwrap(), b"||");
        assert!(delim_of("").is_err());
        assert_eq!(
            end_of("改行（LF・CR LF）", false).unwrap().0,
            RecordEnd::Newline
        );
        assert_eq!(
            end_of("<RS>", false).unwrap().0,
            RecordEnd::Custom(vec![0x1e])
        );
        assert_eq!(
            end_of("LF（Unix）", true).unwrap(),
            (RecordEnd::Newline, false)
        );
        assert_eq!(
            end_of("<CR><LF>", true).unwrap(),
            (RecordEnd::Newline, true)
        );
        assert_eq!(
            end_of("\\x1d", true).unwrap().0,
            RecordEnd::Custom(vec![0x1d])
        );
        // 初めに出す文字は、読み直すと同じ値
        for d in [&b","[..], b"\t", b"\x1f", b"~|~"] {
            assert_eq!(delim_of(&delim_text(d)).unwrap(), d);
        }
        for (end, crlf) in [
            (RecordEnd::Newline, true),
            (RecordEnd::Newline, false),
            (RecordEnd::Custom(vec![0x1e]), true),
            (RecordEnd::Custom(b"\x03\x04".to_vec()), true),
        ] {
            let (e, c) = end_of(&end_text(&end, crlf, true), true).unwrap();
            assert_eq!(e, end);
            if end == RecordEnd::Newline {
                assert_eq!(c, crlf);
            }
            assert_eq!(end_of(&end_text(&end, crlf, false), false).unwrap().0, end);
        }
    }
}

//! 固定長ファイルの UI（15 章 6.5）。
//!
//! - ファイル > 固定長ファイルを開く: データのファイルを選び、レイアウトのダイアログ（コピーブック・
//!   文字コード・レコードの区切り〔自動〕・2 進数の並び）で項目の一覧・レコードの数・1 件目の値を
//!   確かめてから、バックグラウンドで取り込む。
//! - データ > 固定長のレイアウト: 今のシートにレイアウトを当てる（固定長ファイルの作成。空のシートなら
//!   項目の列を作る。Undo できる）。
//! - ファイル > 固定長ファイルに書き出し・上書き保存: 文字コード・区切り・2 進数の並びを選んで書く
//!   （開いたときと別の文字コードにもできる）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateFontW, DeleteObject, HFONT, HGDIOBJ};
use windows::Win32::UI::Controls::{BST_CHECKED, CheckDlgButton, IsDlgButtonChecked};
use windows::Win32::UI::WindowsAndMessaging::*;
use yy_cobol::{Charset, Codec};
use yy_sheet::fixed::{self, FixedSpec, RecordSep};

use super::filter::{add_string, button, combo, label, message, run, send, state};
use super::*;
use crate::goto::{CLASS_BUTTON, CLASS_EDIT, Template};

const IDOK_: u16 = 1;
const IDCANCEL_: u16 = 2;
const L_TEXT: u16 = 10;
const L_LOAD: u16 = 11;
const L_CHARSET: u16 = 12;
const L_SEP: u16 = 13;
const L_LE: u16 = 14;
const L_SUMMARY: u16 = 15;

/// ダイアログの使い道。
enum Purpose {
    /// ファイルを開く（先頭のバイト列・ファイルの大きさ）
    Open { sample: Vec<u8>, len: u64 },
    /// 今のシートに当てる
    Apply,
}

struct LayoutState {
    purpose: Purpose,
    copybook: String,
    charset: Charset,
    /// `None` は自動（開くときだけ）
    sep: Option<RecordSep>,
    little_endian: bool,
    font: HFONT,
    result: Option<FixedSpec>,
}

/// 最後に使った設定（同じ実行の間、次のダイアログの初期値にする）。
pub(super) fn remember(spec: &FixedSpec) {
    with(|a| a.fixed_last = Some(spec.clone()));
}

fn initial() -> (String, Charset, Option<RecordSep>, bool) {
    with(|a| {
        a.sheet()
            .fixed
            .as_deref()
            .cloned()
            .or_else(|| a.fixed_last.clone())
    })
    .flatten()
    .map(|s| {
        (
            s.copybook.to_string(),
            s.codec.charset,
            Some(s.separator),
            s.codec.little_endian,
        )
    })
    .unwrap_or_else(|| (String::new(), Charset::Ms932, None, false))
}

pub(super) fn crlf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// ダイアログの値から設定を作る（自動の区切りは見本から推定）。
fn spec_of(st: &LayoutState) -> std::result::Result<FixedSpec, String> {
    let codec = Codec {
        charset: st.charset,
        little_endian: st.little_endian,
    };
    let mut spec = FixedSpec::new(&st.copybook, codec, st.sep.unwrap_or(RecordSep::None))?;
    if st.sep.is_none()
        && let Purpose::Open { sample, .. } = &st.purpose
    {
        spec.separator = fixed::detect_separator(sample, spec.layout.record_len);
    }
    Ok(spec)
}

fn read_controls(hwnd: HWND, st: &mut LayoutState) {
    st.copybook = dlg_text_long(hwnd, L_TEXT);
    let cs = send(hwnd, L_CHARSET, CB_GETCURSEL, 0, 0);
    st.charset = Charset::all()
        .get(cs.max(0) as usize)
        .copied()
        .unwrap_or(Charset::Ms932);
    let i = send(hwnd, L_SEP, CB_GETCURSEL, 0, 0).max(0) as usize;
    st.sep = match st.purpose {
        Purpose::Open { .. } => i.checked_sub(1).map(|k| RecordSep::ALL[k]),
        Purpose::Apply => Some(RecordSep::ALL[i.min(RecordSep::ALL.len() - 1)]),
    };
    st.little_endian = unsafe { IsDlgButtonChecked(hwnd, L_LE as i32) } == BST_CHECKED.0;
}

/// 長い文字列も読む（`dlg_text` は 4096 文字まで）。
pub(super) fn dlg_text_long(hwnd: HWND, id: u16) -> String {
    unsafe {
        let Ok(h) = GetDlgItem(Some(hwnd), id as i32) else {
            return String::new();
        };
        let n = GetWindowTextLengthW(h).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(h, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..got])
    }
}

fn update_summary(hwnd: HWND, st: &mut LayoutState) {
    read_controls(hwnd, st);
    let text = if st.copybook.trim().is_empty() {
        "コピーブック（COBOL のデータ記述）を入力するか、「ファイルから読み込む」で読み込んでください。\r\n\
         例:\r\n       01  CUST-REC.\r\n           05  CUST-ID    PIC 9(6).\r\n           05  NAME       PIC X(20).\r\n           05  BALANCE    PIC S9(7)V99 COMP-3."
            .to_string()
    } else {
        match spec_of(st) {
            Ok(spec) => {
                let sample = match &st.purpose {
                    Purpose::Open { sample, len } => Some((sample.as_slice(), *len)),
                    Purpose::Apply => None,
                };
                let mut d = fixed::describe(&spec, sample);
                if st.sep.is_none() {
                    d = format!("区切りの推定: {}\r\n{d}", spec.separator.label());
                }
                d
            }
            Err(e) => format!("レイアウトを読めません: {e}"),
        }
    };
    unsafe {
        let _ = SetDlgItemTextW(hwnd, L_SUMMARY as i32, &HSTRING::from(text));
    }
}

/// レイアウトのダイアログ。
fn layout_dialog_with(owner: HWND, purpose: Purpose, title: &str) -> Option<FixedSpec> {
    let (copybook, charset, sep, le) = initial();
    let mut t = Template::dialog(title, 430, 330);
    label(
        &mut t,
        7,
        8,
        250,
        0,
        "コピーブック（COBOL のレイアウト定義。貼り付けもできます）:",
    );
    button(
        &mut t,
        310,
        4,
        113,
        L_LOAD,
        "ファイルから読み込む(&F)...",
        false,
    );
    let multi = (WS_BORDER | WS_TABSTOP | WS_VSCROLL | WS_HSCROLL).0
        | (ES_MULTILINE | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_WANTRETURN) as u32;
    t.item(multi, 7, 20, 416, 110, L_TEXT, CLASS_EDIT, "");
    label(&mut t, 7, 140, 50, 0, "文字コード:");
    combo(&mut t, 55, 138, 175, L_CHARSET);
    label(&mut t, 240, 140, 70, 0, "レコードの区切り:");
    combo(&mut t, 308, 138, 115, L_SEP);
    t.item(
        WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
        7,
        156,
        416,
        10,
        L_LE,
        CLASS_BUTTON,
        "2 進数（COMP・BINARY・COMP-5）と浮動小数点（COMP-1・COMP-2）をリトルエンディアンで読み書きする",
    );
    t.item(
        (WS_BORDER | WS_VSCROLL | WS_HSCROLL).0
            | (ES_MULTILINE | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_READONLY) as u32,
        7,
        170,
        416,
        135,
        L_SUMMARY,
        CLASS_EDIT,
        "",
    );
    button(&mut t, 316, 311, 50, IDOK_, "OK", true);
    button(&mut t, 373, 311, 50, IDCANCEL_, "キャンセル", false);
    let mut st = LayoutState {
        sep: match purpose {
            Purpose::Open { .. } => None,
            Purpose::Apply => sep.or(Some(RecordSep::Crlf)),
        },
        purpose,
        copybook,
        charset,
        little_endian: le,
        font: HFONT::default(),
        result: None,
    };
    run(&t, owner, &mut st, Some(layout_proc));
    st.result
}

/// コピーブックのファイルを読む（文字コードを推定して）。
pub(super) fn load_copybook(owner: HWND) -> Option<String> {
    let path = pick_file(
        owner,
        &[
            (
                "コピーブック (*.cpy;*.cbl;*.cob;*.txt)",
                "*.cpy;*.cbl;*.cob;*.txt",
            ),
            ("すべてのファイル (*.*)", "*.*"),
        ],
    )?;
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            message(
                owner,
                &format!("{} を読めませんでした。\n{e}", path.display()),
            );
            return None;
        }
    };
    let det = yy_encoding::detect(&bytes, true);
    let (text, _) = yy_encoding::decode_all(det.encoding, &bytes[det.bom_len..], false);
    Some(String::from_utf8_lossy(&text).into_owned())
}

extern "system" fn layout_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = state::<LayoutState>(hwnd);
                // 等幅のフォント（桁をそろえる）
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
                for id in [L_TEXT, L_SUMMARY] {
                    send(hwnd, id, WM_SETFONT, st.font.0 as usize, 1);
                }
                send(hwnd, L_TEXT, EM_LIMITTEXT, 0, 0);
                let stops: [u32; 4] = [24, 48, 150, 235];
                send(
                    hwnd,
                    L_SUMMARY,
                    EM_SETTABSTOPS,
                    stops.len(),
                    stops.as_ptr() as isize,
                );
                for (i, c) in Charset::all().iter().enumerate() {
                    add_string(hwnd, L_CHARSET, CB_ADDSTRING, &c.label());
                    if *c == st.charset {
                        send(hwnd, L_CHARSET, CB_SETCURSEL, i, 0);
                    }
                }
                let open = matches!(st.purpose, Purpose::Open { .. });
                if open {
                    add_string(hwnd, L_SEP, CB_ADDSTRING, "自動（推定する）");
                }
                for r in RecordSep::ALL {
                    add_string(hwnd, L_SEP, CB_ADDSTRING, r.label());
                }
                let sel = match st.sep {
                    None => 0,
                    Some(r) => {
                        RecordSep::ALL.iter().position(|x| *x == r).unwrap_or(0) + open as usize
                    }
                };
                send(hwnd, L_SEP, CB_SETCURSEL, sel, 0);
                if st.little_endian {
                    let _ = CheckDlgButton(hwnd, L_LE as i32, BST_CHECKED);
                }
                // 文字列は最後に（変更の通知で文字コードなどを読むので）
                let text = crlf(&st.copybook);
                let _ = SetDlgItemTextW(hwnd, L_TEXT as i32, &HSTRING::from(text));
                update_summary(hwnd, st);
                1
            }
            WM_DESTROY if GetWindowLongPtrW(hwnd, GWLP_USERDATA) != 0 => {
                let st = state::<LayoutState>(hwnd);
                let _ = DeleteObject(HGDIOBJ(st.font.0));
                0
            }
            // 初期化の前の通知は無視する
            WM_COMMAND if GetWindowLongPtrW(hwnd, GWLP_USERDATA) == 0 => 0,
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let code = crate::hiword(wparam.0);
                let st = state::<LayoutState>(hwnd);
                match id {
                    L_TEXT if code == EN_CHANGE => {
                        update_summary(hwnd, st);
                        1
                    }
                    L_CHARSET | L_SEP if code == CBN_SELCHANGE => {
                        update_summary(hwnd, st);
                        1
                    }
                    L_LE => {
                        update_summary(hwnd, st);
                        1
                    }
                    L_LOAD => {
                        if let Some(text) = load_copybook(hwnd) {
                            let _ =
                                SetDlgItemTextW(hwnd, L_TEXT as i32, &HSTRING::from(crlf(&text)));
                            update_summary(hwnd, st);
                        }
                        1
                    }
                    IDOK_ => {
                        read_controls(hwnd, st);
                        let spec = match spec_of(st) {
                            Ok(s) => s,
                            Err(e) => {
                                message(hwnd, &format!("レイアウトを読めません。\n{e}"));
                                return 1;
                            }
                        };
                        if let Purpose::Open { len, .. } = &st.purpose {
                            let (n, rest) =
                                fixed::record_count(*len, spec.layout.record_len, spec.separator);
                            if rest > 0 {
                                let r = MessageBoxW(
                                    Some(hwnd),
                                    &HSTRING::from(format!(
                                        "ファイルの大きさがレコード長（{} バイト・区切り {}）で割り切れません。\n\
                                         {n} レコードを取り込み、末尾の {rest} バイトは読みません。続けますか？",
                                        spec.layout.record_len,
                                        spec.separator.label()
                                    )),
                                    w!("yysheet"),
                                    MB_YESNO | MB_ICONWARNING,
                                );
                                if r != IDYES {
                                    return 1;
                                }
                            }
                        }
                        st.result = Some(spec);
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

/// ファイルを選ぶ（`filters` は（名前, パターン））。
pub(super) fn pick_file(owner: HWND, filters: &[(&str, &str)]) -> Option<PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog};
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let names: Vec<(HSTRING, HSTRING)> = filters
            .iter()
            .map(|(n, p)| (HSTRING::from(*n), HSTRING::from(*p)))
            .collect();
        let specs: Vec<COMDLG_FILTERSPEC> = names
            .iter()
            .map(|(n, p)| COMDLG_FILTERSPEC {
                pszName: PCWSTR(n.as_ptr()),
                pszSpec: PCWSTR(p.as_ptr()),
            })
            .collect();
        let _ = dialog.SetFileTypes(&specs);
        dialog.Show(Some(owner)).ok()?;
        file_dialog_result(dialog.GetResult().ok()?)
    }
}

/// 保存先を選ぶ（固定長ファイル）。
fn pick_save(owner: HWND, current: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::{FileSaveDialog, IFileSaveDialog};
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = [
            COMDLG_FILTERSPEC {
                pszName: w!("固定長ファイル (*.dat)"),
                pszSpec: w!("*.dat"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("すべてのファイル (*.*)"),
                pszSpec: w!("*.*"),
            },
        ];
        let _ = dialog.SetFileTypes(&filters);
        let _ = dialog.SetDefaultExtension(w!("dat"));
        let name = current
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "data.dat".into());
        let _ = dialog.SetFileName(&HSTRING::from(name));
        dialog.Show(Some(owner)).ok()?;
        file_dialog_result(dialog.GetResult().ok()?)
    }
}

/// ファイル > 固定長ファイルを開く。
pub(super) fn open_fixed() {
    if !confirm_discard() {
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
    open_fixed_path(frame, ctx, &path);
}

/// 固定長ファイルを開く（レイアウトを尋ねる）。
pub(super) fn open_fixed_path(frame: HWND, ctx: Arc<SheetCtx>, path: &Path) {
    // 見本（先頭 1 MB）
    let read = (|| -> std::io::Result<(Vec<u8>, u64)> {
        use std::io::Read;
        let f = std::fs::File::open(path)?;
        let len = f.metadata()?.len();
        let mut sample = Vec::new();
        f.take(1 << 20).read_to_end(&mut sample)?;
        Ok((sample, len))
    })();
    let (sample, len) = match read {
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
        "固定長ファイルを開く - {}",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    );
    let Some(spec) = layout_dialog_with(frame, Purpose::Open { sample, len }, &title) else {
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
        fixed::import(&ctx2, &p, &s2, &|done, total| {
            w.report(format!(
                "取り込み中… {}%（Esc で中止）",
                done * 100 / total.max(1)
            ));
            !w.cancelled()
        })
    });
    match r {
        Ok((sheet, rep)) => {
            let mut doc = Document::with_book(
                ctx,
                Workbook {
                    sheets: vec![sheet],
                    date_system: DateSystem::D1900,
                },
            );
            doc.path = Some(path.to_owned());
            with(|a| a.set_document(doc, Origin::Fixed));
            let mut msg = format!(
                "{} レコード × {} 項目を取り込みました（{}・区切り {}）",
                crate::util::group_digits(rep.records),
                spec.layout.fields.len(),
                spec.codec.charset.name(),
                spec.separator.label()
            );
            if rep.invalid > 0 {
                msg.push_str(&format!(
                    "。数値として読めない項目 {} 個は X'…' で表示しています（そのまま書き戻せます）",
                    crate::util::group_digits(rep.invalid)
                ));
            }
            if rep.remainder > 0 {
                msg.push_str(&format!(
                    "。末尾の {} バイトは読みませんでした",
                    rep.remainder
                ));
            }
            set_status(&msg);
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

/// データ > 固定長のレイアウト（今のシートに当てる。固定長ファイルの作成）。
pub(super) fn layout_dialog() {
    let Some(frame) = with(|a| {
        a.end_edit(true);
        a.frame
    }) else {
        return;
    };
    let Some(spec) = layout_dialog_with(frame, Purpose::Apply, "固定長のレイアウト")
    else {
        return;
    };
    remember(&spec);
    let added = with(|a| {
        if a.sheet().view.rows.is_some() {
            info_box(
                a.frame,
                "絞り込み・並べ替えの表示中はレイアウトを当てられません。解除してから行ってください。",
            );
            return None;
        }
        let sheet = a.sheet;
        let mut added = 0;
        let res = a.doc.edit(|b, ctx| {
            added = fixed::apply_layout(ctx, &mut b.sheets[sheet], &spec)?;
            b.sheets[sheet].formulas.touch_all();
            Ok(())
        });
        a.after_edit();
        match res {
            Ok(()) => Some(added),
            Err(e) => {
                error_box(a.frame, &format!("レイアウトを当てられませんでした: {e}"));
                None
            }
        }
    })
    .flatten();
    if let Some(n) = added {
        set_status(&format!(
            "レイアウトを当てました（レコード {} バイト・項目 {} 個。列を {n} 個足しました）。\
             2 行目から入力し、ファイル > 固定長ファイルに書き出しで保存します",
            spec.layout.record_len,
            spec.layout.fields.len()
        ));
    }
}

const W_CHARSET: u16 = 10;
const W_SEP: u16 = 11;
const W_LE: u16 = 12;

struct WriteState {
    spec: FixedSpec,
    result: Option<FixedSpec>,
}

/// 書き出しの設定（文字コード・区切り・2 進数の並び）。
fn write_dialog(owner: HWND, spec: &FixedSpec) -> Option<FixedSpec> {
    let mut t = Template::dialog("固定長ファイルに書き出す", 300, 105);
    label(&mut t, 7, 9, 70, 0, "文字コード:");
    combo(&mut t, 80, 7, 213, W_CHARSET);
    label(&mut t, 7, 27, 70, 0, "レコードの区切り:");
    combo(&mut t, 80, 25, 213, W_SEP);
    t.item(
        WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
        7,
        45,
        286,
        10,
        W_LE,
        CLASS_BUTTON,
        "2 進数と浮動小数点をリトルエンディアンで書く",
    );
    label(
        &mut t,
        7,
        60,
        286,
        0,
        "文字・ゾーン 10 進数の符号・浮動小数点の形は、選んだ文字コードに合わせて書きます。",
    );
    button(&mut t, 186, 84, 50, IDOK_, "書き出す", true);
    button(&mut t, 243, 84, 50, IDCANCEL_, "キャンセル", false);
    let mut st = WriteState {
        spec: spec.clone(),
        result: None,
    };
    run(&t, owner, &mut st, Some(write_proc));
    st.result
}

extern "system" fn write_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
                let st = state::<WriteState>(hwnd);
                for (i, c) in Charset::all().iter().enumerate() {
                    add_string(hwnd, W_CHARSET, CB_ADDSTRING, &c.label());
                    if *c == st.spec.codec.charset {
                        send(hwnd, W_CHARSET, CB_SETCURSEL, i, 0);
                    }
                }
                for (i, r) in RecordSep::ALL.iter().enumerate() {
                    add_string(hwnd, W_SEP, CB_ADDSTRING, r.label());
                    if *r == st.spec.separator {
                        send(hwnd, W_SEP, CB_SETCURSEL, i, 0);
                    }
                }
                if st.spec.codec.little_endian {
                    let _ = CheckDlgButton(hwnd, W_LE as i32, BST_CHECKED);
                }
                1
            }
            WM_COMMAND if GetWindowLongPtrW(hwnd, GWLP_USERDATA) == 0 => 0,
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let st = state::<WriteState>(hwnd);
                match id {
                    IDOK_ => {
                        let cs = send(hwnd, W_CHARSET, CB_GETCURSEL, 0, 0).max(0) as usize;
                        let sp = send(hwnd, W_SEP, CB_GETCURSEL, 0, 0).max(0) as usize;
                        let mut s = st.spec.clone();
                        s.codec = Codec {
                            charset: Charset::all().get(cs).copied().unwrap_or(Charset::Ms932),
                            little_endian: IsDlgButtonChecked(hwnd, W_LE as i32) == BST_CHECKED.0,
                        };
                        s.separator = RecordSep::ALL.get(sp).copied().unwrap_or(RecordSep::None);
                        st.result = Some(s);
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

/// 固定長ファイルに書き出す（`target` がなければ尋ねる）。文字コードなどは毎回選ぶ。
pub(super) fn export_fixed(target: Option<PathBuf>) -> bool {
    let Some((frame, path, ctx, sheet_ix, sheet, origin)) = with(|a| {
        a.end_edit(true);
        (
            a.frame,
            a.doc.path.clone(),
            a.ctx.clone(),
            a.sheet,
            a.sheet().clone(),
            a.origin.clone(),
        )
    }) else {
        return false;
    };
    let Some(spec) = sheet.fixed.as_deref().cloned() else {
        info_box(
            frame,
            "このシートには固定長のレイアウトがありません。\n\
             データ > 固定長のレイアウト で、コピーブックを設定してください。",
        );
        return false;
    };
    let Some(spec) = write_dialog(frame, &spec) else {
        return false;
    };
    let target = match target {
        Some(t) => t,
        None => {
            let cur = if matches!(origin, Origin::Fixed) {
                path.clone()
            } else {
                None
            };
            match pick_save(frame, cur.as_deref()) {
                Some(t) => t,
                None => return false,
            }
        }
    };
    let mut sheet = sheet;
    let mut order = None;
    if let Some(rows) = sheet.view.rows.clone() {
        let r = unsafe {
            MessageBoxW(
                Some(frame),
                &HSTRING::from(
                    "絞り込み・並べ替えの表示中です。\n\n\
                     はい: 見えている行を表示の順に書き出す\n\
                     いいえ: すべての行を元の順に書き出す",
                ),
                w!("yysheet"),
                MB_YESNOCANCEL | MB_ICONQUESTION,
            )
        };
        match r {
            IDYES => order = Some(rows),
            IDNO => sheet.view = yy_sheet::View::default(),
            _ => return false,
        }
    }
    let t = target.clone();
    let s2 = spec.clone();
    let r = crate::remote::wait(&set_status, move |w| {
        let order = order.as_deref().map(Vec::as_slice);
        fixed::export(&ctx, &sheet, &t, &s2, order, &|done, total| {
            w.report(format!("書き出し中… {}%", done * 100 / total.max(1)));
            !w.cancelled()
        })
    });
    match r {
        Ok(rep) => {
            let mut msg = format!(
                "{} レコードを書き出しました（{}・区切り {}）",
                crate::util::group_digits(rep.records),
                spec.codec.charset.name(),
                spec.separator.label()
            );
            if rep.undetermined > 0 {
                let first = rep
                    .first_undetermined
                    .map(|r| format!("（最初は {} 行目）", r + 2))
                    .unwrap_or_default();
                let note = format!(
                    "レイアウト未確定の行が {} 行あります{first}。取り込んだときの元のバイトのある行はそのまま書き、ない行は書いていません。",
                    crate::util::group_digits(rep.undetermined)
                );
                msg.push_str("（レイアウト未確定の行があります）");
                info_box(frame, &note);
            }
            let is = rep.issues;
            if is.total() > 0 {
                let mut lines = vec![
                    msg.clone(),
                    String::new(),
                    "次の値は書き換えて書きました:".into(),
                ];
                for (n, what) in [
                    (is.overflow, "桁あふれ（上の桁を落とした）"),
                    (is.truncated, "長すぎる文字列（後ろを切った）"),
                    (is.unencodable, "文字コードにない文字（? にした）"),
                    (is.not_number, "数値の項目に数値でない値（0 にした）"),
                    (is.negative, "符号なしの項目に負の値（絶対値にした）"),
                ] {
                    if n > 0 {
                        lines.push(format!("・{what}: {} 個", crate::util::group_digits(n)));
                    }
                }
                if let Some((row, field)) = &rep.first {
                    lines.push(format!("\n最初の場所: {} 行目の {field}", row + 2));
                }
                msg.push_str("（書き換えた値があります）");
                info_box(frame, &lines.join("\n"));
            }
            set_status(&msg);
            remember(&spec);
            with(|a| {
                // 書いた設定をシートに残し、開いたファイル（か新しい文書）なら保存済みにする
                if let Some(s) = a.doc.book.sheets.get_mut(sheet_ix) {
                    s.fixed = Some(Arc::new(spec.clone()));
                }
                if matches!(a.origin, Origin::Fixed | Origin::New)
                    && (a.doc.path.is_none() || a.doc.path.as_deref() == Some(target.as_path()))
                {
                    a.doc.path = Some(target.clone());
                    a.origin = Origin::Fixed;
                    a.doc.dirty = false;
                }
                a.update_title();
            });
            true
        }
        Err(e) => {
            set_status("");
            if e.kind() != std::io::ErrorKind::Interrupted {
                error_box(
                    frame,
                    &format!("{} に書き出せませんでした。\n{e}", target.display()),
                );
            }
            false
        }
    }
}

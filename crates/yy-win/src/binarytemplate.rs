//! Binary template editor and hierarchical result browser.
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetMonitorInfoW, HFONT, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Interface, PWSTR, w};
use yy_buffer::Snapshot;
use yy_core::binary_template::{self, Field};

use crate::goto::{CLASS_BUTTON, CLASS_EDIT, CLASS_STATIC, Template};

const SOURCE: u16 = 100;
const TREE: u16 = 101;
const STATUS: u16 = 102;
const RUN: u16 = 103;
const OPEN: u16 = 104;
const SAVE: u16 = 105;
const SAMPLE: u16 = 106;
const DETAILS: u16 = 107;
const SELECT: u16 = 1;
const CLOSE: u16 = 2;
thread_local! { static LAST_SOURCE: RefCell<String> = RefCell::new(binary_template::EXAMPLE.into()); }
thread_local! { static IN_CALLBACK: Cell<bool> = const { Cell::new(false) }; }
struct CallbackGuard;
impl Drop for CallbackGuard {
    fn drop(&mut self) {
        IN_CALLBACK.with(|busy| busy.set(false));
    }
}

struct State {
    data: Snapshot,
    source: String,
    fields: Vec<Field>,
    flat: Vec<Field>,
    selection: Option<Range<u64>>,
    tree: HWND,
    source_label: HWND,
    result_label: HWND,
    valid: bool,
    sample: usize,
    font: HFONT,
    font_dpi: u32,
}
impl Drop for State {
    fn drop(&mut self) {
        if !self.font.is_invalid() {
            unsafe {
                let _ = DeleteObject(self.font.into());
            }
        }
    }
}

fn template() -> Template {
    let mut t = Template::dialog("バイナリテンプレート", 580, 360);
    // Resizing is handled in WM_SIZE; the initial bounds are fitted to the monitor.
    let style = u32::from(t.0[0]) | (u32::from(t.0[1]) << 16) | WS_THICKFRAME.0 | WS_MAXIMIZEBOX.0;
    t.0[0] = style as u16;
    t.0[1] = (style >> 16) as u16;
    for (id, label) in [
        (OPEN, "開く..."),
        (SAVE, "保存..."),
        (SAMPLE, "サンプル"),
        (RUN, "解析する"),
        (SELECT, "範囲を選択"),
        (CLOSE, "閉じる"),
    ] {
        t.item(
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            0,
            0,
            60,
            16,
            id,
            CLASS_BUTTON,
            label,
        );
    }
    t.item(
        (WS_TABSTOP | WS_BORDER | WS_VSCROLL | WS_HSCROLL).0
            | (ES_MULTILINE | ES_WANTRETURN | ES_AUTOHSCROLL | ES_AUTOVSCROLL) as u32,
        0,
        0,
        100,
        100,
        SOURCE,
        CLASS_EDIT,
        "",
    );
    t.item(0, 0, 0, 100, 18, STATUS, CLASS_STATIC, "");
    t.item(0, 0, 0, 100, 18, DETAILS, CLASS_STATIC, "");
    t
}

pub(crate) fn show(owner: HWND, data: Snapshot) -> Option<Range<u64>> {
    let mut s = State {
        data,
        source: LAST_SOURCE.with(|s| s.borrow().clone()),
        fields: Vec::new(),
        flat: Vec::new(),
        selection: None,
        tree: HWND::default(),
        source_label: HWND::default(),
        result_label: HWND::default(),
        valid: false,
        sample: 0,
        font: HFONT::default(),
        font_dpi: 0,
    };
    let t = template().aligned();
    unsafe {
        DialogBoxIndirectParamW(
            None,
            t.as_ptr() as *const DLGTEMPLATE,
            Some(owner),
            Some(proc),
            LPARAM(&mut s as *mut State as isize),
        );
    }
    LAST_SOURCE.with(|last| *last.borrow_mut() = std::mem::take(&mut s.source));
    s.selection.take()
}

fn text(hwnd: HWND, id: u16, value: &str) {
    unsafe {
        let _ = SetDlgItemTextW(hwnd, id as i32, &HSTRING::from(value));
    }
}
fn source(hwnd: HWND) -> String {
    unsafe {
        let edit = GetDlgItem(Some(hwnd), SOURCE as i32).unwrap_or_default();
        let mut buf = vec![0u16; (GetWindowTextLengthW(edit).max(0) as usize + 1).min(1_048_577)];
        let n = GetWindowTextW(edit, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}
fn move_to(hwnd: HWND, x: i32, y: i32, width: i32, height: i32) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            x,
            y,
            width.max(0),
            height.max(0),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}
fn layout(hwnd: HWND, s: &mut State) {
    unsafe {
        let mut r = RECT::default();
        let _ = GetClientRect(hwnd, &mut r);
        let dpi = GetDpiForWindow(hwnd)
            .max(96)
            .min((r.right.max(1) as u32 * 96 / 560).max(96))
            .min((r.bottom.max(1) as u32 * 96 / 480).max(96));
        if dpi != s.font_dpi {
            let new_font = crate::util::ui_font(dpi);
            for id in [
                SOURCE, STATUS, DETAILS, RUN, OPEN, SAVE, SAMPLE, SELECT, CLOSE,
            ] {
                if let Ok(c) = GetDlgItem(Some(hwnd), id as i32) {
                    SendMessageW(
                        c,
                        WM_SETFONT,
                        Some(WPARAM(new_font.0 as usize)),
                        Some(LPARAM(1)),
                    );
                }
            }
            for c in [s.tree, s.source_label, s.result_label] {
                SendMessageW(
                    c,
                    WM_SETFONT,
                    Some(WPARAM(new_font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
            if !s.font.is_invalid() {
                let _ = DeleteObject(s.font.into());
            }
            s.font = new_font;
            s.font_dpi = dpi;
        }
        let scale = dpi as f64 / 96.0;
        let d = |n: i32| (f64::from(n) * scale).round() as i32;
        let (pad, gap, button) = (d(12), d(8), d(32));
        let width = (r.right - 2 * pad).max(0);
        let bottom = r.bottom - pad - button;
        let set = |id, x, y, w, h| {
            if let Ok(c) = GetDlgItem(Some(hwnd), id as i32) {
                move_to(c, x, y, w, h);
            }
        };
        let bw = ((width - 3 * gap) / 4).min(d(110));
        for (i, id) in [OPEN, SAVE, SAMPLE, RUN].iter().enumerate() {
            set(*id, pad + i as i32 * (bw + gap), pad, bw, button);
        }
        set(STATUS, pad, pad + button + gap, width, d(40));
        let top = pad + button + gap + d(42);
        let pane_height = (bottom - gap - d(48) - top).max(0);
        if width >= d(760) {
            let half = (width - gap) / 2;
            move_to(s.source_label, pad, top, half, d(22));
            move_to(s.result_label, pad + half + gap, top, half, d(22));
            set(SOURCE, pad, top + d(24), half, pane_height - d(24));
            move_to(
                s.tree,
                pad + half + gap,
                top + d(24),
                half,
                pane_height - d(24),
            );
        } else {
            let half = (pane_height - gap) / 2;
            move_to(s.source_label, pad, top, width, d(22));
            set(SOURCE, pad, top + d(24), width, half - d(24));
            move_to(s.result_label, pad, top + half + gap, width, d(22));
            move_to(s.tree, pad, top + half + gap + d(24), width, half - d(24));
        }
        set(DETAILS, pad, bottom - gap - d(46), width, d(44));
        let close = d(90).min(width / 3);
        let select = d(150).min(width - close - gap);
        set(CLOSE, r.right - pad - close, bottom, close, button);
        set(
            SELECT,
            r.right - pad - close - gap - select,
            bottom,
            select,
            button,
        );
    }
}
fn selected(s: &State) -> Option<&Field> {
    if !s.valid {
        return None;
    }
    unsafe {
        let item = SendMessageW(
            s.tree,
            TVM_GETNEXTITEM,
            Some(WPARAM(TVGN_CARET as usize)),
            Some(LPARAM(0)),
        );
        if item.0 == 0 {
            return None;
        }
        let mut v = TVITEMW {
            mask: TVIF_PARAM,
            hItem: HTREEITEM(item.0),
            ..Default::default()
        };
        if SendMessageW(
            s.tree,
            TVM_GETITEMW,
            None,
            Some(LPARAM(&mut v as *mut _ as isize)),
        )
        .0 == 0
        {
            return None;
        }
        s.flat.get(v.lParam.0 as usize)
    }
}
fn update_selection(hwnd: HWND, s: &State) {
    if let Some(f) = selected(s) {
        text(
            hwnd,
            DETAILS,
            &format!(
                "{} : {}    0x{:X}～0x{:X}（{}バイト）\r\n{}",
                f.name,
                f.ty,
                f.range.start,
                f.range.end,
                f.range.end - f.range.start,
                f.value
            ),
        );
    } else {
        text(
            hwnd,
            DETAILS,
            "解析結果のフィールドを選ぶと、位置・長さ・値を確認できます。",
        );
    }
}
fn insert_fields(s: &mut State, fields: &[Field], parent: HTREEITEM) {
    for f in fields {
        let index = s.flat.len();
        // Keep only the selected node's metadata, avoiding duplicate nested trees.
        s.flat.push(Field {
            children: Vec::new(),
            ..f.clone()
        });
        let label = format!(
            "{} : {}   @0x{:X} +{}   {}",
            f.name,
            f.ty,
            f.range.start,
            f.range.end - f.range.start,
            f.value
        );
        let mut label = crate::util::wide(&label);
        let ins = TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 {
                item: TVITEMW {
                    mask: TVIF_TEXT | TVIF_PARAM,
                    pszText: PWSTR(label.as_mut_ptr()),
                    lParam: LPARAM(index as isize),
                    ..Default::default()
                },
            },
        };
        let item = unsafe {
            SendMessageW(
                s.tree,
                TVM_INSERTITEMW,
                None,
                Some(LPARAM(&ins as *const _ as isize)),
            )
        };
        insert_fields(s, &f.children, HTREEITEM(item.0));
    }
}
fn run(hwnd: HWND, s: &mut State) {
    s.source = source(hwnd);
    s.valid = false;
    s.flat.clear();
    s.fields.clear();
    unsafe {
        SendMessageW(s.tree, TVM_DELETEITEM, None, Some(LPARAM(TVI_ROOT.0)));
    }
    match binary_template::compile(&s.source).and_then(|t| t.run(&s.data)) {
        Ok(fields) => {
            insert_fields(s, &fields, TVI_ROOT);
            s.valid = true;
            s.fields = fields;
            text(
                hwnd,
                STATUS,
                &format!(
                    "解析完了 — {}フィールド / {}バイト。結果を選び「範囲を選択」で16進数表示へ移動します。",
                    s.flat.len(),
                    s.data.len()
                ),
            );
        }
        Err(e) => text(hwnd, STATUS, &format!("解析できません: {e}")),
    }
    update_selection(hwnd, s);
}
fn pick_file(owner: HWND, save: bool) -> Option<PathBuf> {
    unsafe {
        let d: IFileDialog = if save {
            let d: IFileSaveDialog =
                CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            d.cast().ok()?
        } else {
            let d: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            d.cast().ok()?
        };
        let _ = d.SetFileTypes(&[
            COMDLG_FILTERSPEC {
                pszName: w!("バイナリテンプレート (*.yybt;*.bt)"),
                pszSpec: w!("*.yybt;*.bt"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("すべてのファイル"),
                pszSpec: w!("*.*"),
            },
        ]);
        if save {
            let _ = d.SetDefaultExtension(w!("yybt"));
            let _ = d.SetFileName(w!("template.yybt"));
        }
        d.Show(Some(owner)).ok()?;
        let item = d.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(name.0 as *const _));
        path
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    if IN_CALLBACK.with(|busy| busy.replace(true)) {
        return 0;
    }
    let _guard = CallbackGuard;
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        if ptr.is_null() {
            return 0;
        }
        // Do not keep this reference across nested file dialogs or messages sent to the source edit.
        match msg {
            WM_INITDIALOG => {
                let s = &mut *ptr;
                let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                    dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                    dwICC: ICC_TREEVIEW_CLASSES,
                });
                let font = SendMessageW(hwnd, WM_GETFONT, None, None);
                s.tree = CreateWindowExW(
                    WS_EX_CLIENTEDGE,
                    WC_TREEVIEWW,
                    w!(""),
                    WS_CHILD
                        | WS_VISIBLE
                        | WS_TABSTOP
                        | WS_HSCROLL
                        | WS_VSCROLL
                        | WINDOW_STYLE(
                            TVS_HASBUTTONS | TVS_HASLINES | TVS_LINESATROOT | TVS_SHOWSELALWAYS,
                        ),
                    0,
                    0,
                    100,
                    100,
                    Some(hwnd),
                    Some(HMENU(TREE as usize as *mut _)),
                    None,
                    None,
                )
                .unwrap_or_default();
                let label = |caption| {
                    CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("STATIC"),
                        caption,
                        WS_CHILD | WS_VISIBLE,
                        0,
                        0,
                        100,
                        20,
                        Some(hwnd),
                        None,
                        None,
                        None,
                    )
                    .unwrap_or_default()
                };
                s.source_label = label(w!("テンプレート（C風構文 / UTF-8）"));
                s.result_label = label(w!("解析結果 — 名前・型・位置・長さ・値"));
                for c in [s.tree, s.source_label, s.result_label] {
                    SendMessageW(
                        c,
                        WM_SETFONT,
                        Some(WPARAM(font.0 as usize)),
                        Some(LPARAM(1)),
                    );
                }
                SendDlgItemMessageW(
                    hwnd,
                    SOURCE as i32,
                    EM_SETLIMITTEXT,
                    WPARAM(1_048_576),
                    LPARAM(0),
                );
                let initial = s.source.clone();
                text(hwnd, SOURCE, &initial);
                let mut mi = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi)
                    .as_bool()
                {
                    let dpi = GetDpiForWindow(hwnd).max(96) as i32;
                    let width = (1000 * dpi / 96).min(mi.rcWork.right - mi.rcWork.left);
                    let height = (700 * dpi / 96).min(mi.rcWork.bottom - mi.rcWork.top);
                    move_to(
                        hwnd,
                        mi.rcWork.left + (mi.rcWork.right - mi.rcWork.left - width) / 2,
                        mi.rcWork.top + (mi.rcWork.bottom - mi.rcWork.top - height) / 2,
                        width,
                        height,
                    );
                }
                layout(hwnd, &mut *ptr);
                text(
                    hwnd,
                    STATUS,
                    "C風の型宣言・struct・配列・if・for・whileに対応します。010 Editorの全構文との互換性はありません。",
                );
                update_selection(hwnd, &*ptr);
                1
            }
            WM_SIZE => {
                layout(hwnd, &mut *ptr);
                1
            }
            WM_GETMINMAXINFO => {
                let limits = &mut *(lparam.0 as *mut MINMAXINFO);
                let mut mi = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi)
                    .as_bool()
                {
                    limits.ptMinTrackSize.x = 640.min(mi.rcWork.right - mi.rcWork.left);
                    limits.ptMinTrackSize.y = 480.min(mi.rcWork.bottom - mi.rcWork.top);
                }
                1
            }
            WM_DESTROY => {
                if !(*ptr).font.is_invalid() {
                    let _ = DeleteObject((*ptr).font.into());
                    (*ptr).font = HFONT::default();
                }
                0
            }
            WM_COMMAND => {
                let id = crate::loword(wparam.0) as u16;
                let notification = crate::hiword(wparam.0);
                if id == SOURCE && notification == EN_CHANGE {
                    (*ptr).valid = false;
                    text(
                        hwnd,
                        STATUS,
                        "テンプレートを変更しました。「解析する」で結果を更新してください。",
                    );
                    return 1;
                }
                match id {
                    RUN => run(hwnd, &mut *ptr),
                    SELECT => {
                        if let Some(f) = selected(&*ptr) {
                            (*ptr).selection = Some(f.range.clone());
                            (*ptr).source = source(hwnd);
                            let _ = EndDialog(hwnd, 1);
                        } else {
                            text(
                                hwnd,
                                STATUS,
                                "解析を実行し、移動するフィールドを選択してください。",
                            );
                        }
                    }
                    CLOSE => {
                        (*ptr).source = source(hwnd);
                        let _ = EndDialog(hwnd, 0);
                    }
                    OPEN | SAVE => {
                        if let Some(path) = pick_file(hwnd, id == SAVE) {
                            if id == SAVE {
                                let content = source(hwnd);
                                match std::fs::write(&path, content) {
                                    Ok(()) => text(hwnd, STATUS, "テンプレートを保存しました。"),
                                    Err(e) => text(hwnd, STATUS, &format!("保存できません: {e}")),
                                }
                            } else {
                                let result = std::fs::metadata(&path).and_then(|m| {
                                    if m.len() > 1_048_576 {
                                        Err(std::io::Error::other(
                                            "テンプレートは1 MiB以下にしてください",
                                        ))
                                    } else {
                                        std::fs::read_to_string(&path)
                                    }
                                });
                                match result {
                                    Ok(content) => {
                                        (*ptr).valid = false;
                                        text(hwnd, SOURCE, content.trim_start_matches('\u{feff}'));
                                        text(
                                            hwnd,
                                            STATUS,
                                            "テンプレートを読み込みました。「解析する」で結果を更新してください。",
                                        );
                                    }
                                    Err(e) => text(hwnd, STATUS, &format!("開けません: {e}")),
                                }
                            }
                        }
                    }
                    SAMPLE => {
                        let samples = [
                            binary_template::EXAMPLE,
                            include_str!("../../../demo/binary-templates/bmp.yybt"),
                            include_str!("../../../demo/binary-templates/wav.yybt"),
                        ];
                        (*ptr).sample = ((*ptr).sample + 1) % samples.len();
                        (*ptr).valid = false;
                        text(hwnd, SOURCE, samples[(*ptr).sample]);
                        text(
                            hwnd,
                            STATUS,
                            "サンプルを読み込みました。「解析する」で結果を更新してください。",
                        );
                    }
                    _ => return 0,
                }
                1
            }
            WM_NOTIFY => {
                let hdr = &*(lparam.0 as *const NMHDR);
                if hdr.idFrom == TREE as usize && hdr.code == TVN_SELCHANGEDW {
                    update_selection(hwnd, &*ptr);
                    return 1;
                }
                0
            }
            WM_CLOSE => {
                (*ptr).source = source(hwnd);
                let _ = EndDialog(hwnd, 0);
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
    fn real_dialog_parses_selects_clears_errors_and_fits_small_screen() {
        std::thread::spawn(|| unsafe {
            let mut state = State {
                data: Snapshot::from_bytes(vec![2, 0, 5, 9]),
                source: String::new(),
                fields: Vec::new(),
                flat: Vec::new(),
                selection: None,
                tree: HWND::default(),
                source_label: HWND::default(),
                result_label: HWND::default(),
                valid: false,
                sample: 0,
                font: HFONT::default(),
                font_dpi: 0,
            };
            let t = template().aligned();
            let hwnd = CreateDialogIndirectParamW(
                None,
                t.as_ptr() as *const DLGTEMPLATE,
                None,
                Some(proc),
                LPARAM(&mut state as *mut State as isize),
            )
            .unwrap();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                IN_CALLBACK.with(|busy| busy.set(true));
                let _guard = CallbackGuard;
                text(
                    hwnd,
                    SOURCE,
                    "typedef struct { ushort count; uchar values[count]; } HEADER; HEADER header;",
                );
                run(hwnd, &mut state);
                assert!(state.valid);
                assert_eq!(state.flat.len(), 5);
                assert_eq!(state.fields[0].range, 0..4);
                let root = SendMessageW(
                    state.tree,
                    TVM_GETNEXTITEM,
                    Some(WPARAM(TVGN_ROOT as usize)),
                    Some(LPARAM(0)),
                );
                assert_ne!(root.0, 0);
                SendMessageW(
                    state.tree,
                    TVM_SELECTITEM,
                    Some(WPARAM(TVGN_CARET as usize)),
                    Some(LPARAM(root.0)),
                );
                assert_eq!(selected(&state).map(|f| f.range.clone()), Some(0..4));
                update_selection(hwnd, &state);
                for (width, height) in [(640, 480), (1000, 700)] {
                    move_to(hwnd, 0, 0, width, height);
                    layout(hwnd, &mut state);
                    let mut client = RECT::default();
                    GetClientRect(hwnd, &mut client).unwrap();
                    let controls = [
                        SOURCE, STATUS, DETAILS, RUN, OPEN, SAVE, SAMPLE, SELECT, CLOSE,
                    ]
                    .map(|id| GetDlgItem(Some(hwnd), id as i32).unwrap());
                    for c in controls.into_iter().chain([
                        state.tree,
                        state.source_label,
                        state.result_label,
                    ]) {
                        let mut r = RECT::default();
                        GetWindowRect(c, &mut r).unwrap();
                        windows::Win32::Graphics::Gdi::MapWindowPoints(
                            None,
                            Some(hwnd),
                            std::slice::from_raw_parts_mut(
                                &mut r as *mut RECT as *mut windows::Win32::Foundation::POINT,
                                2,
                            ),
                        );
                        assert!(
                            r.left >= 0
                                && r.top >= 0
                                && r.right <= client.right
                                && r.bottom <= client.bottom
                                && r.right > r.left
                                && r.bottom > r.top,
                            "control outside client {width}x{height}: {r:?} / {client:?}"
                        );
                    }
                }
                text(hwnd, SOURCE, "uint incomplete;");
                state.data = Snapshot::from_bytes(vec![1]);
                run(hwnd, &mut state);
                assert!(!state.valid);
                assert!(state.flat.is_empty());
                assert!(selected(&state).is_none());
            }));
            DestroyWindow(hwnd).unwrap();
            if let Err(e) = outcome {
                std::panic::resume_unwind(e);
            }
        })
        .join()
        .unwrap();
    }
}

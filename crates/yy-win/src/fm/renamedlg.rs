//! 一括リネームの画面（2026-10-27 の提案 7 章）。
//!
//! 上に規則（置き換え・ひな形・連番・大文字と小文字・スクリプト）、下にプレビュー（元の名前・新しい
//! 名前・フォルダ・状態）。入力を変えるたびに少し待ってから作業スレッドで全件を計算し直す（メタデータは
//! 作業スレッドで読み、覚えておく）。問題（使えない名前・重複・既にある・スクリプトのエラー）があれば
//! 「実行」できない。実行は `yy_files::rename::execute`（2 段・失敗したら元に戻す）で、記録を
//! `renames\rename-日時.csv` に残し、「元に戻す」で戻せる。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{HFONT, InvalidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, PWSTR, w};
use yy_files::jobs::Dirs;
use yy_files::props::Props;
use yy_files::rename::{Case, Item, Outcome, Rules, Status, Target};

use crate::util::{error_box, info_box};
use crate::{hiword, loword};

const CLASS: PCWSTR = w!("YYFmRename");
const WM_APP_PREVIEW: u32 = WM_APP + 110;
const WM_APP_DONE: u32 = WM_APP + 111;
const TIMER_PREVIEW: usize = 1;

const ID_FIND: u16 = 7001;
const ID_REPLACE: u16 = 7002;
const ID_REGEX: u16 = 7003;
const ID_CASE_SENS: u16 = 7004;
const ID_ALL: u16 = 7005;
const ID_TARGET: u16 = 7006;
const ID_TEMPLATE: u16 = 7007;
const ID_INSERT: u16 = 7008;
const ID_START: u16 = 7009;
const ID_STEP: u16 = 7010;
const ID_PER_FOLDER: u16 = 7011;
const ID_CASE: u16 = 7012;
const ID_SCRIPT: u16 = 7013;
const ID_LIST: u16 = 7014;
const ID_INFO: u16 = 7015;
const ID_RUN: u16 = 7016;
const ID_RUN_SKIP: u16 = 7017;
const ID_UNDO: u16 = 7018;
const ID_CLOSE: u16 = 7019;

struct Dlg {
    hwnd: HWND,
    owner: HWND,
    reload_msg: u32,
    dirs: Dirs,
    dpi: u32,
    /// 部品（行ごと。幅 0 は残り）
    rows: Vec<Vec<(HWND, i32)>>,
    script: HWND,
    list: HWND,
    info: HWND,
    buttons: Vec<HWND>,
    find: HWND,
    replace: HWND,
    regex: HWND,
    case_sens: HWND,
    all: HWND,
    target: HWND,
    template: HWND,
    start: HWND,
    step: HWND,
    per_folder: HWND,
    case: HWND,
    items: Arc<Vec<Item>>,
    outcomes: Vec<Outcome>,
    error: Option<String>,
    generation: u64,
    busy: bool,
    props: Arc<Mutex<HashMap<PathBuf, Props>>>,
    last_log: Option<PathBuf>,
}

thread_local! {
    static DLG: RefCell<Option<Dlg>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Dlg) -> R) -> Option<R> {
    DLG.with(|d| d.try_borrow_mut().ok()?.as_mut().map(f))
}

fn text_of(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(h, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..got])
    }
}

fn set_text(h: HWND, s: &str) {
    unsafe {
        let _ = SetWindowTextW(h, &HSTRING::from(s));
    }
}

fn checked(h: HWND) -> bool {
    unsafe { SendMessageW(h, BM_GETCHECK, None, None).0 == 1 }
}

fn combo_index(h: HWND) -> usize {
    unsafe { SendMessageW(h, CB_GETCURSEL, None, None).0.max(0) as usize }
}

/// モードなしのダイアログのキー（Tab・Esc）。処理したら `true`。
pub(super) fn dialog_message(msg: &MSG) -> bool {
    let Some(h) = with(|d| d.hwnd) else {
        return false;
    };
    unsafe { IsDialogMessageW(h, msg).as_bool() }
}

/// 画面を開く（開いていれば項目を入れ替える）。`reload_msg` は実行の後で `owner` に送る。
pub(super) fn open(owner: HWND, items: Vec<Item>, dirs: Dirs, reload_msg: u32) {
    if let Some(h) = with(|d| {
        d.items = Arc::new(items.clone());
        d.outcomes.clear();
        d.hwnd
    }) {
        unsafe {
            let _ = SetForegroundWindow(h);
        }
        schedule();
        return;
    }
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else {
            return;
        };
        let instance = module.into();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(
                (windows::Win32::Graphics::Gdi::COLOR_BTNFACE.0 + 1) as usize as *mut _,
            ),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&class);
        let dpi = GetDpiForWindow(owner).max(96);
        let d = |v: i32| v * dpi as i32 / 96;
        let root = GetAncestor(owner, GA_ROOT);
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            CLASS,
            &HSTRING::from(format!("一括リネーム（{} 個）", items.len())),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            d(1000),
            d(720),
            Some(root),
            None,
            Some(instance),
            None,
        ) else {
            return;
        };
        let font: HFONT = crate::util::ui_font(dpi);
        let child =
            |class: PCWSTR, text: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, id: u16| {
                let h = CreateWindowExW(
                    ex,
                    class,
                    &HSTRING::from(text),
                    WS_CHILD | WS_VISIBLE | style,
                    0,
                    0,
                    10,
                    10,
                    Some(hwnd),
                    Some(HMENU(id as usize as *mut _)),
                    Some(instance),
                    None,
                )
                .unwrap_or_default();
                SendMessageW(
                    h,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
                h
            };
        let label = |t: &str| child(w!("STATIC"), t, WINDOW_STYLE(0x200), WINDOW_EX_STYLE(0), 0);
        let edit = |id: u16, t: &str| {
            child(
                w!("EDIT"),
                t,
                WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                WS_EX_CLIENTEDGE,
                id,
            )
        };
        let check = |t: &str, id: u16| {
            child(
                w!("BUTTON"),
                t,
                WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
                WINDOW_EX_STYLE(0),
                id,
            )
        };
        let button = |t: &str, id: u16| {
            child(
                w!("BUTTON"),
                t,
                WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
                WINDOW_EX_STYLE(0),
                id,
            )
        };
        let combo = |id: u16, items: &[&str]| {
            let h = child(
                w!("COMBOBOX"),
                "",
                WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
                WINDOW_EX_STYLE(0),
                id,
            );
            for t in items {
                SendMessageW(
                    h,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(HSTRING::from(*t).as_ptr() as isize)),
                );
            }
            SendMessageW(h, CB_SETCURSEL, Some(WPARAM(0)), None);
            h
        };
        let find = edit(ID_FIND, "");
        let replace = edit(ID_REPLACE, "");
        let regex = check("正規表現", ID_REGEX);
        let case_sens = check("大文字と小文字を区別", ID_CASE_SENS);
        let all = check("すべて", ID_ALL);
        SendMessageW(all, BM_SETCHECK, Some(WPARAM(1)), None);
        let target = combo(ID_TARGET, &["名前（拡張子を除く）", "名前全体", "拡張子"]);
        let template = edit(ID_TEMPLATE, "{name}{ext}");
        let insert = button("挿入 ▾", ID_INSERT);
        let start = edit(ID_START, "1");
        let step = edit(ID_STEP, "1");
        let per_folder = check("フォルダごとに振り直す", ID_PER_FOLDER);
        let case = combo(
            ID_CASE,
            &[
                "そのまま",
                "すべて小文字",
                "すべて大文字",
                "語の頭だけ大文字",
                "拡張子だけ小文字",
            ],
        );
        let script = child(
            w!("EDIT"),
            "",
            WS_TABSTOP
                | WS_VSCROLL
                | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32),
            WS_EX_CLIENTEDGE,
            ID_SCRIPT,
        );
        let cue = crate::util::wide(
            "例: if ext == \".jpg\" { pad(n, 3) + \"_\" + stem + ext }（何も返さなければそのまま）",
        );
        SendMessageW(
            script,
            EM_SETCUEBANNER,
            Some(WPARAM(1)),
            Some(LPARAM(cue.as_ptr() as isize)),
        );
        let list = child(
            WC_LISTVIEWW,
            "",
            WS_TABSTOP | WINDOW_STYLE(LVS_REPORT | LVS_SHOWSELALWAYS | LVS_OWNERDATA),
            WS_EX_CLIENTEDGE,
            ID_LIST,
        );
        let _ = SetWindowTheme(list, w!("Explorer"), None);
        SendMessageW(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(0)),
            Some(LPARAM(
                (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP) as isize,
            )),
        );
        for (i, (t, wdt)) in [
            ("元の名前", 260),
            ("新しい名前", 300),
            ("フォルダ", 240),
            ("状態", 200),
        ]
        .iter()
        .enumerate()
        {
            let s = crate::util::wide(t);
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH,
                cx: d(*wdt),
                pszText: PWSTR(s.as_ptr() as *mut _),
                ..Default::default()
            };
            SendMessageW(
                list,
                LVM_INSERTCOLUMNW,
                Some(WPARAM(i)),
                Some(LPARAM(&col as *const _ as isize)),
            );
        }
        let info = child(
            w!("STATIC"),
            "",
            WINDOW_STYLE(0x200),
            WINDOW_EX_STYLE(0),
            ID_INFO,
        );
        let run = button("実行", ID_RUN);
        let run_skip = button("問題を除いて実行", ID_RUN_SKIP);
        let undo = button("元に戻す...", ID_UNDO);
        let close = button("閉じる", ID_CLOSE);
        let rows = vec![
            vec![
                (label("検索"), 40),
                (find, 0),
                (label("置換"), 40),
                (replace, 0),
                (regex, 90),
                (case_sens, 160),
                (all, 70),
                (target, 170),
            ],
            vec![(label("ひな形"), 50), (template, 0), (insert, 80)],
            vec![
                (label("連番の始まり"), 90),
                (start, 70),
                (label("増分"), 40),
                (step, 60),
                (per_folder, 180),
                (label("大文字・小文字"), 100),
                (case, 180),
            ],
            vec![(label("スクリプト（Rhai。任意）"), 200)],
        ];
        let dlg = Dlg {
            hwnd,
            owner,
            reload_msg,
            dirs,
            dpi,
            rows,
            script,
            list,
            info,
            buttons: vec![run, run_skip, undo, close],
            find,
            replace,
            regex,
            case_sens,
            all,
            target,
            template,
            start,
            step,
            per_folder,
            case,
            items: Arc::new(items),
            outcomes: Vec::new(),
            error: None,
            generation: 0,
            busy: false,
            props: Arc::default(),
            last_log: None,
        };
        DLG.with(|c| *c.borrow_mut() = Some(dlg));
        with(|d| d.layout());
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(Some(find));
        schedule();
    }
}

impl Dlg {
    fn d(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }

    fn layout(&self) {
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut rc);
            let pad = self.d(6);
            let h = self.d(24);
            let mut y = pad;
            for row in &self.rows {
                let fixed: i32 = row.iter().map(|(_, w)| self.d(*w) + self.d(4)).sum();
                let fills = row.iter().filter(|(_, w)| *w == 0).count().max(1) as i32;
                let fill = ((rc.right - 2 * pad - fixed) / fills).max(self.d(60));
                let mut x = pad;
                for (hw, w) in row {
                    let wd = if *w == 0 { fill } else { self.d(*w) };
                    let hh = if is_combo(*hw) { self.d(200) } else { h };
                    let _ = MoveWindow(*hw, x, y, wd, hh, true);
                    x += wd + self.d(4);
                }
                y += h + self.d(4);
            }
            let script_h = self.d(80);
            let _ = MoveWindow(self.script, pad, y, rc.right - 2 * pad, script_h, true);
            y += script_h + pad;
            let bottom_h = h + 2 * pad;
            let list_h = (rc.bottom - y - bottom_h).max(self.d(60));
            let _ = MoveWindow(self.list, pad, y, rc.right - 2 * pad, list_h, true);
            y += list_h + pad;
            let widths = [80, 140, 100, 80];
            let total: i32 = widths.iter().map(|w| self.d(*w) + self.d(4)).sum();
            let _ = MoveWindow(
                self.info,
                pad,
                y,
                (rc.right - 2 * pad - total).max(0),
                h,
                true,
            );
            let mut x = rc.right - pad - total + self.d(4);
            for (b, w) in self.buttons.iter().zip(widths) {
                let _ = MoveWindow(*b, x, y, self.d(w), h, true);
                x += self.d(w) + self.d(4);
            }
        }
    }

    fn rules(&self) -> Rules {
        Rules {
            find: text_of(self.find),
            replace: text_of(self.replace),
            regex: checked(self.regex),
            case_sensitive: checked(self.case_sens),
            all: checked(self.all),
            target: match combo_index(self.target) {
                1 => Target::Name,
                2 => Target::Ext,
                _ => Target::Stem,
            },
            template: text_of(self.template),
            start: text_of(self.start).trim().parse().unwrap_or(1),
            step: text_of(self.step).trim().parse().unwrap_or(1),
            per_folder: checked(self.per_folder),
            case: match combo_index(self.case) {
                1 => Case::Lower,
                2 => Case::Upper,
                3 => Case::Title,
                4 => Case::LowerExt,
                _ => Case::Keep,
            },
            script: text_of(self.script),
        }
    }

    fn update_info(&self) {
        let text = if let Some(e) = &self.error {
            format!("規則の誤り: {e}")
        } else if self.busy && self.outcomes.is_empty() {
            "計算しています…".into()
        } else {
            let changed = self
                .outcomes
                .iter()
                .filter(|o| o.status == Status::Ok)
                .count();
            let problems = self
                .outcomes
                .iter()
                .filter(|o| o.status.is_problem())
                .count();
            let mut s = format!(
                "{} 個のうち {changed} 個の名前が変わります",
                self.items.len()
            );
            if problems > 0 {
                s.push_str(&format!("。問題 {problems} 個（赤い行）"));
            }
            if self.busy {
                s.push_str("（計算しています…）");
            }
            s
        };
        set_text(self.info, &text);
        let problems = self.outcomes.iter().any(|o| o.status.is_problem());
        let changed = self.outcomes.iter().any(|o| o.status == Status::Ok);
        unsafe {
            let _ = EnableWindow(
                self.buttons[0],
                !self.busy && self.error.is_none() && changed && !problems,
            );
            let _ = EnableWindow(
                self.buttons[1],
                !self.busy && self.error.is_none() && changed && problems,
            );
        }
    }

    fn cell(&self, row: usize, col: usize) -> String {
        let Some(item) = self.items.get(row) else {
            return String::new();
        };
        let o = self.outcomes.get(row);
        match col {
            0 => item
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            1 => o.map(|o| o.name.clone()).unwrap_or_default(),
            2 => item
                .path
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            3 => o.map(|o| o.status.label()).unwrap_or_default(),
            _ => String::new(),
        }
    }
}

fn is_combo(h: HWND) -> bool {
    let mut buf = [0u16; 32];
    let n = unsafe { GetClassNameW(h, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n]).eq_ignore_ascii_case("ComboBox")
}

/// 少し待ってからプレビューを計算し直す。
fn schedule() {
    with(|d| unsafe {
        SetTimer(Some(d.hwnd), TIMER_PREVIEW, 200, None);
    });
}

fn compute() {
    let Some((hwnd, generation, rules, items, props)) = with(|d| {
        d.generation += 1;
        d.busy = true;
        d.update_info();
        (
            d.hwnd.0 as isize,
            d.generation,
            d.rules(),
            d.items.clone(),
            d.props.clone(),
        )
    }) else {
        return;
    };
    let tz = super::tz_offset();
    let now = super::now_ns();
    std::thread::spawn(move || {
        let r = (|| -> Result<Vec<Outcome>, String> {
            let c = rules.compile()?;
            if c.needs_props {
                let mut cache = props.lock().map_err(|_| "メタデータの記録が壊れました")?;
                for it in items.iter() {
                    if !it.dir && !cache.contains_key(&it.path) {
                        cache.insert(it.path.clone(), yy_files::props::read(&it.path));
                    }
                }
            }
            let cache = props.lock().map_err(|_| "メタデータの記録が壊れました")?;
            Ok(yy_files::rename::preview(
                &c,
                &items,
                &|i| cache.get(&items[i].path),
                &|p| std::fs::symlink_metadata(p).is_ok(),
                tz,
                now,
            ))
        })();
        let p = Box::into_raw(Box::new((generation, r)));
        unsafe {
            if PostMessageW(
                Some(HWND(hwnd as *mut _)),
                WM_APP_PREVIEW,
                WPARAM(0),
                LPARAM(p as isize),
            )
            .is_err()
            {
                drop(Box::from_raw(p));
            }
        }
    });
}

fn on_preview(generation: u64, r: Result<Vec<Outcome>, String>) {
    with(|d| {
        if generation != d.generation {
            return;
        }
        d.busy = false;
        match r {
            Ok(o) => {
                d.outcomes = o;
                d.error = None;
            }
            Err(e) => {
                d.outcomes.clear();
                d.error = Some(e);
            }
        }
        unsafe {
            SendMessageW(
                d.list,
                LVM_SETITEMCOUNT,
                Some(WPARAM(d.items.len())),
                Some(LPARAM(LVSICF_NOSCROLL as isize)),
            );
            let _ = InvalidateRect(Some(d.list), None, false);
        }
        d.update_info();
    });
}

fn run(skip_problems: bool) {
    let Some((hwnd, moves)) = with(|d| {
        (
            d.hwnd,
            yy_files::rename::moves(&d.items, &d.outcomes, skip_problems),
        )
    }) else {
        return;
    };
    let moves = match moves {
        Ok(m) if !m.is_empty() => m,
        Ok(_) => return,
        Err(n) => {
            error_box(hwnd, &format!("問題のある項目が {n} 個あります。"));
            return;
        }
    };
    let r = unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(format!(
                "{} 個の名前を変えます。よろしいですか？（あとで「元に戻す」で戻せます）",
                moves.len()
            )),
            w!("一括リネーム"),
            MB_OKCANCEL | MB_ICONQUESTION,
        )
    };
    if r != IDOK {
        return;
    }
    with(|d| {
        d.busy = true;
        d.update_info();
    });
    let h = hwnd.0 as isize;
    std::thread::spawn(move || {
        let local = yy_files::Local;
        let r = yy_files::rename::execute(
            &moves,
            &|a, b| yy_files::Fs::rename_new(&local, a, b),
            &mut |_| {},
        )
        .map_err(|e| e.to_string());
        let p = Box::into_raw(Box::new((r, moves)));
        unsafe {
            if PostMessageW(
                Some(HWND(h as *mut _)),
                WM_APP_DONE,
                WPARAM(0),
                LPARAM(p as isize),
            )
            .is_err()
            {
                drop(Box::from_raw(p));
            }
        }
    });
}

fn on_done(r: Result<yy_files::rename::Log, String>, moves: Vec<(PathBuf, PathBuf)>) {
    let Some((hwnd, owner, reload, dir)) = with(|d| {
        d.busy = false;
        (d.hwnd, d.owner, d.reload_msg, d.dirs.renames())
    }) else {
        return;
    };
    match r {
        Ok(log) => {
            let stamp = super::local_stamp().replace([' ', ':'], "-");
            let path = dir.join(format!("rename-{stamp}.csv"));
            let saved = yy_files::rename::write_log(&path, &log);
            // 項目のパスを新しいものに（続けて直せる）
            let map: HashMap<PathBuf, PathBuf> = moves.into_iter().collect();
            with(|d| {
                let items: Vec<Item> = d
                    .items
                    .iter()
                    .map(|it| {
                        let mut it = it.clone();
                        if let Some(n) = map.get(&it.path) {
                            it.path = n.clone();
                        } else if let Some((old, new)) =
                            map.iter().find(|(old, _)| it.path.starts_with(old))
                        {
                            // 名前を変えたフォルダの中
                            if let Ok(rest) = it.path.strip_prefix(old) {
                                it.path = new.join(rest);
                            }
                        }
                        it
                    })
                    .collect();
                d.items = Arc::new(items);
                d.props.lock().map(|mut c| c.clear()).ok();
                d.last_log = Some(path.clone());
            });
            unsafe {
                let _ = PostMessageW(Some(owner), reload, WPARAM(0), LPARAM(0));
            }
            let mut msg = format!("{} 個の名前を変えました。", map.len());
            if let Err(e) = saved {
                msg.push_str(&format!("\n記録を保存できませんでした: {e}"));
            }
            set_text(with(|d| d.info).unwrap_or_default(), &msg);
            schedule();
        }
        Err(e) => {
            with(|d| d.update_info());
            error_box(
                hwnd,
                &format!("名前を変えられませんでした（変えたものは元に戻しました）。\n{e}"),
            );
            schedule();
        }
    }
}

fn undo() {
    let Some((hwnd, owner, reload, dir, last)) = with(|d| {
        (
            d.hwnd,
            d.owner,
            d.reload_msg,
            d.dirs.renames(),
            d.last_log.clone(),
        )
    }) else {
        return;
    };
    let path = super::pick_file(
        hwnd,
        ("一括リネームの記録 (*.csv)", "*.csv"),
        Some(last.as_deref().and_then(|p| p.parent()).unwrap_or(&dir)),
        None,
    );
    let Some(path) = path else { return };
    let log = match yy_files::rename::read_log(&path) {
        Ok(l) => l,
        Err(e) => {
            error_box(hwnd, &format!("{}\n{e}", path.display()));
            return;
        }
    };
    let local = yy_files::Local;
    match yy_files::rename::undo(
        &log,
        &|a, b| yy_files::Fs::rename_new(&local, a, b),
        &mut |_| {},
    ) {
        Ok(()) => {
            unsafe {
                let _ = PostMessageW(Some(owner), reload, WPARAM(0), LPARAM(0));
            }
            let n: usize = log.iter().map(Vec::len).sum();
            // 項目のパスを元に戻す
            let map: HashMap<PathBuf, PathBuf> = log
                .iter()
                .flatten()
                .map(|(a, b)| (b.clone(), a.clone()))
                .collect();
            with(|d| {
                let items: Vec<Item> = d
                    .items
                    .iter()
                    .map(|it| {
                        let mut it = it.clone();
                        if let Some(o) = map.get(&it.path) {
                            it.path = o.clone();
                        }
                        it
                    })
                    .collect();
                d.items = Arc::new(items);
            });
            info_box(hwnd, &format!("{n} 個の名前を元に戻しました。"));
            schedule();
        }
        Err(e) => error_box(hwnd, &format!("元に戻せませんでした。\n{e}")),
    }
}

fn insert_menu() {
    let Some((hwnd, template, btn)) = with(|d| (d.hwnd, d.template, d.rows[1][2].0)) else {
        return;
    };
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return };
        let mut tokens: Vec<String> = Vec::new();
        for (t, desc) in yy_files::rename::TEMPLATE_HELP {
            tokens.push((*t).to_owned());
            let _ = AppendMenuW(
                m,
                MF_STRING,
                tokens.len(),
                &HSTRING::from(format!("{t}\t{desc}")),
            );
        }
        let meta = CreatePopupMenu().unwrap_or_default();
        for (k, desc) in yy_files::props::KEYS {
            let t = if k.ends_with(".date") || k.ends_with(".created") || k.ends_with(".modified") {
                format!("{{{k}:%Y%m%d}}")
            } else {
                format!("{{{k}}}")
            };
            tokens.push(t.clone());
            let _ = AppendMenuW(
                meta,
                MF_STRING,
                tokens.len(),
                &HSTRING::from(format!("{t}\t{desc}")),
            );
        }
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(m, MF_POPUP, meta.0 as usize, w!("メタデータ(&M)"));
        let mut rc = RECT::default();
        let _ = GetWindowRect(btn, &mut rc);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD, rc.left, rc.bottom, None, hwnd, None).0 as usize;
        let _ = DestroyMenu(m);
        if cmd >= 1
            && let Some(t) = tokens.get(cmd - 1)
        {
            let _ = SetFocus(Some(template));
            let w = crate::util::wide(t);
            SendMessageW(
                template,
                EM_REPLACESEL,
                Some(WPARAM(1)),
                Some(LPARAM(w.as_ptr() as isize)),
            );
        }
    }
}

extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            with(|d| d.layout());
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let mm = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            mm.ptMinTrackSize.x = 700;
            mm.ptMinTrackSize.y = 420;
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = loword(wparam.0) as u16;
            let code = hiword(wparam.0);
            match id {
                ID_FIND | ID_REPLACE | ID_TEMPLATE | ID_START | ID_STEP | ID_SCRIPT
                    if code == EN_CHANGE =>
                {
                    schedule()
                }
                ID_REGEX | ID_CASE_SENS | ID_ALL | ID_PER_FOLDER if code == BN_CLICKED => {
                    schedule()
                }
                ID_TARGET | ID_CASE if code == CBN_SELCHANGE => schedule(),
                ID_INSERT => insert_menu(),
                ID_RUN => run(false),
                ID_RUN_SKIP => run(true),
                ID_UNDO => undo(),
                ID_CLOSE => unsafe {
                    let _ = DestroyWindow(hwnd);
                },
                x if x == IDCANCEL.0 as u16 => unsafe {
                    let _ = DestroyWindow(hwnd);
                },
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), wparam.0);
            }
            compute();
            LRESULT(0)
        }
        WM_NOTIFY => {
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if hdr.idFrom as u16 != ID_LIST {
                return LRESULT(0);
            }
            match hdr.code {
                LVN_GETDISPINFOW => {
                    let nm = unsafe { &mut *(lparam.0 as *mut NMLVDISPINFOW) };
                    if nm.item.mask.0 & LVIF_TEXT.0 != 0
                        && nm.item.cchTextMax > 0
                        && !nm.item.pszText.is_null()
                    {
                        let text = with(|d| {
                            d.cell(
                                nm.item.iItem.max(0) as usize,
                                nm.item.iSubItem.max(0) as usize,
                            )
                        })
                        .unwrap_or_default();
                        let buf = unsafe {
                            std::slice::from_raw_parts_mut(
                                nm.item.pszText.0,
                                nm.item.cchTextMax as usize,
                            )
                        };
                        let mut n = 0;
                        for u in text.encode_utf16().take(buf.len() - 1) {
                            buf[n] = u;
                            n += 1;
                        }
                        buf[n] = 0;
                    }
                    LRESULT(0)
                }
                NM_CUSTOMDRAW => {
                    let cd = unsafe { &mut *(lparam.0 as *mut NMLVCUSTOMDRAW) };
                    match cd.nmcd.dwDrawStage {
                        CDDS_PREPAINT => LRESULT(CDRF_NOTIFYITEMDRAW as isize),
                        CDDS_ITEMPREPAINT => LRESULT(CDRF_NOTIFYSUBITEMDRAW as isize),
                        s if s.0 == CDDS_ITEMPREPAINT.0 | CDDS_SUBITEM.0 => {
                            let row = cd.nmcd.dwItemSpec;
                            let status =
                                with(|d| d.outcomes.get(row).map(|o| o.status.clone())).flatten();
                            cd.clrText = match (status, cd.iSubItem) {
                                (Some(s), 1 | 3) if s.is_problem() => COLORREF(0x0000_00c0),
                                (Some(Status::Ok), 1) => COLORREF(0x00a0_4000),
                                (Some(Status::Same), 1) => COLORREF(0x0080_8080),
                                _ => COLORREF(0x0000_0000),
                            };
                            LRESULT(CDRF_NEWFONT as isize)
                        }
                        _ => LRESULT(CDRF_DODEFAULT as isize),
                    }
                }
                _ => LRESULT(0),
            }
        }
        WM_APP_PREVIEW => {
            let (generation, r) =
                *unsafe { Box::from_raw(lparam.0 as *mut (u64, Result<Vec<Outcome>, String>)) };
            on_preview(generation, r);
            LRESULT(0)
        }
        WM_APP_DONE => {
            let (r, moves) = *unsafe {
                Box::from_raw(
                    lparam.0
                        as *mut (
                            Result<yy_files::rename::Log, String>,
                            Vec<(PathBuf, PathBuf)>,
                        ),
                )
            };
            on_done(r, moves);
            LRESULT(0)
        }
        WM_DESTROY => {
            DLG.with(|d| {
                if let Ok(mut d) = d.try_borrow_mut() {
                    *d = None;
                }
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

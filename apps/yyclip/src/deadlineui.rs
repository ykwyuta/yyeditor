//! Resident deadline manager and notifications. No clipboard state is changed here.
use crate::businessdays::{Adjustment, Calendar};
use crate::deadlines::{MAX_SNOOZE, Repeat, Store, Style, Task};
use chrono::{Datelike, Days, Local, NaiveDate, NaiveDateTime, Timelike};
use std::cell::RefCell;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI, SystemParametersInfoForDpi,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Error, HRESULT, HSTRING, PCWSTR, Result, w};

const CLASS: PCWSTR = w!("YYClipDeadlineManager");
const ALERT_CLASS: PCWSTR = w!("YYClipDeadlineAlert");
const TIMER: usize = 51;
const LIST: usize = 201;
const FILTER: usize = 202;
const TITLE: usize = 203;
const DUE: usize = 204;
const MEMO: usize = 205;
const INTERVAL: usize = 206;
const STYLE: usize = 207;
const REPEAT: usize = 208;
const SAVE: usize = 209;
const NEW: usize = 210;
const COMPLETE: usize = 211;
const DELETE: usize = 212;
const TEST: usize = 213;
const OPEN: usize = 214;
const DISMISS: usize = 215;
const HEADING: usize = 216;
const SUBTITLE: usize = 217;
const SUMMARY: usize = 218;
const BACK: usize = 219;
const EDIT: usize = 220;
const EMPTY: usize = 221;
const HOUR: usize = 222;
const MINUTE: usize = 223;
const ADJUSTMENT: usize = 224;
const IMPORT_CALENDAR: usize = 225;
const CALENDAR_INFO: usize = 226;
const SNOOZE: usize = 227;
const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}
const BACKGROUND: COLORREF = rgb(245, 247, 251);
const INK: COLORREF = rgb(30, 41, 59);
const MUTED: COLORREF = rgb(100, 116, 139);
const BLUE: COLORREF = rgb(37, 99, 235);

struct State {
    hwnd: HWND,
    resident: HWND,
    store: Store,
    rows: Vec<u64>,
    editing: Option<u64>,
    controls: Vec<(usize, HWND)>,
    labels: Vec<HWND>,
    font: HFONT,
    heading_font: HFONT,
    editor_open: bool,
    baseline: Option<(Vec<String>, [usize; 5])>,
    alerts: Vec<(HWND, HFONT)>,
    alert_tasks: Vec<u64>,
    scroll: i32,
    refreshing: bool,
    last_error: Option<String>,
    display_day: Option<chrono::NaiveDate>,
}

thread_local! { static STATE: RefCell<Option<State>> = const { RefCell::new(None) }; }

fn with<T>(f: impl FnOnce(&State) -> T) -> Option<T> {
    STATE.with(|cell| cell.try_borrow().ok()?.as_ref().map(f))
}
fn with_mut<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    STATE.with(|cell| cell.try_borrow_mut().ok()?.as_mut().map(f))
}
fn control(id: usize) -> HWND {
    with(|s| s.controls.iter().find(|c| c.0 == id).map(|c| c.1))
        .flatten()
        .unwrap_or_default()
}
fn text(hwnd: HWND) -> String {
    unsafe {
        let mut buffer = vec![0; GetWindowTextLengthW(hwnd).max(0) as usize + 1];
        let length = GetWindowTextW(hwnd, &mut buffer);
        String::from_utf16_lossy(&buffer[..length.max(0) as usize])
    }
}
fn set(id: usize, value: &str) {
    unsafe {
        let _ = SetWindowTextW(control(id), &HSTRING::from(value));
    }
}
fn selection(id: usize) -> usize {
    unsafe { SendMessageW(control(id), CB_GETCURSEL, None, None).0.max(0) as usize }
}
fn select(id: usize, at: usize) {
    unsafe {
        SendMessageW(control(id), CB_SETCURSEL, Some(WPARAM(at)), None);
    }
}
fn set_due(due: NaiveDateTime) {
    let date = SYSTEMTIME {
        wYear: due.year() as u16,
        wMonth: due.month() as u16,
        wDay: due.day() as u16,
        ..Default::default()
    };
    unsafe {
        SendMessageW(
            control(DUE),
            DTM_SETSYSTEMTIME,
            Some(WPARAM(GDT_VALID.0 as usize)),
            Some(LPARAM(&date as *const _ as isize)),
        );
    }
    select(HOUR, due.hour() as usize);
    select(MINUTE, due.minute() as usize);
}
fn due_value() -> std::io::Result<NaiveDateTime> {
    let fail = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "カレンダーで日付を選び、時・分を指定してください。",
        )
    };
    let mut date = SYSTEMTIME::default();
    let state = unsafe {
        SendMessageW(
            control(DUE),
            DTM_GETSYSTEMTIME,
            None,
            Some(LPARAM(&mut date as *mut _ as isize)),
        )
        .0
    };
    if state != GDT_VALID.0 as isize {
        return Err(fail());
    }
    let selected =
        |id| u32::try_from(unsafe { SendMessageW(control(id), CB_GETCURSEL, None, None).0 }).ok();
    NaiveDate::from_ymd_opt(date.wYear.into(), date.wMonth.into(), date.wDay.into())
        .and_then(|d| d.and_hms_opt(selected(HOUR)?, selected(MINUTE)?, 0))
        .ok_or_else(fail)
}
fn adjustment() -> Adjustment {
    match selection(ADJUSTMENT) {
        1 => Adjustment::Previous,
        2 => Adjustment::Next,
        _ => Adjustment::None,
    }
}
fn calendar_info() {
    let summary = with(|s| {
        let (first, last) = s.store.calendar().years();
        format!(
            "休業日CSV: {}日 / {first}–{last}",
            s.store.calendar().holidays.len(),
        )
    })
    .unwrap_or_default();
    let preview = match due_value() {
        Ok(due) => match with(|s| s.store.calendar().adjust(due, adjustment())) {
            Some(Ok(due)) => format!("保存後の期限: {}", due.format("%Y-%m-%d %H:%M")),
            _ => "対象年の休業日CSVを登録してください".into(),
        },
        Err(_) => "日付・時刻を選択してください".into(),
    };
    set(CALENDAR_INFO, &format!("{summary}\r\n{preview}"));
}
fn error(message: &str) {
    let hwnd = with(|s| s.hwnd).unwrap_or_default();
    unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(message),
            w!("yyclip 期限管理"),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub fn start(resident: HWND) -> Result<()> {
    let path = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| Error::new(E_FAIL, "APPDATA がありません"))?
        .join("yyclip/deadlines.json");
    let store = Store::open(path)
        .map_err(|e| Error::new(E_FAIL, format!("期限データを読み込めません: {e}")))?;
    start_store(resident, store)
}

fn start_store(resident: HWND, mut store: Store) -> Result<()> {
    store.restart_notifications();
    unsafe {
        let instance = GetModuleHandleW(None)?.into();
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES | ICC_DATE_CLASSES,
        })
        .ok()?;
        for (name, proc) in [
            (
                CLASS,
                Some(manager_proc as unsafe extern "system" fn(_, _, _, _) -> _),
            ),
            (
                ALERT_CLASS,
                Some(alert_proc as unsafe extern "system" fn(_, _, _, _) -> _),
            ),
        ] {
            if RegisterClassExW(&WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: proc,
                hInstance: instance,
                lpszClassName: name,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hbrBackground: GetSysColorBrush(COLOR_WINDOW),
                ..Default::default()
            }) == 0
            {
                return Err(Error::from_thread());
            }
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("yyclip 期限管理 — 今日・明日の予定"),
            WS_OVERLAPPEDWINDOW | WS_VSCROLL,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            860,
            650,
            None,
            None,
            Some(instance),
            None,
        )?;
        let mut controls = Vec::new();
        let child = |class: PCWSTR, label: &str, style: WINDOW_STYLE, id: usize| {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                &HSTRING::from(label),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | style
                    | if [TITLE, DUE, MEMO, INTERVAL, LIST].contains(&id) {
                        WS_BORDER
                    } else {
                        WINDOW_STYLE(0)
                    },
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(id as *mut _)),
                Some(instance),
                None,
            )
        };
        controls.push((
            FILTER,
            child(
                w!("COMBOBOX"),
                "",
                WINDOW_STYLE(CBS_DROPDOWNLIST as u32) | WS_VSCROLL,
                FILTER,
            )?,
        ));
        controls.push((
            LIST,
            child(
                WC_LISTVIEWW,
                "",
                WINDOW_STYLE(LVS_REPORT | LVS_SINGLESEL) | WS_VSCROLL | WS_HSCROLL,
                LIST,
            )?,
        ));
        for id in [TITLE, DUE, MEMO, INTERVAL] {
            if id == DUE {
                controls.push((
                    DUE,
                    child(
                        DATETIMEPICK_CLASSW,
                        "",
                        WINDOW_STYLE(DTS_SHORTDATECENTURYFORMAT),
                        DUE,
                    )?,
                ));
                for time_id in [HOUR, MINUTE] {
                    controls.push((
                        time_id,
                        child(
                            w!("COMBOBOX"),
                            "",
                            WINDOW_STYLE(CBS_DROPDOWNLIST as u32) | WS_VSCROLL,
                            time_id,
                        )?,
                    ));
                }
                continue;
            }
            let style = if id == MEMO {
                WINDOW_STYLE(ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32) | WS_VSCROLL
            } else {
                WINDOW_STYLE(ES_AUTOHSCROLL as u32)
            };
            controls.push((id, child(w!("EDIT"), "", style, id)?));
        }
        for id in [STYLE, REPEAT, ADJUSTMENT] {
            controls.push((
                id,
                child(
                    w!("COMBOBOX"),
                    "",
                    WINDOW_STYLE(CBS_DROPDOWNLIST as u32) | WS_VSCROLL,
                    id,
                )?,
            ));
        }
        for (id, label) in [
            (NEW, "＋ 作業を追加"),
            (SAVE, "保存"),
            (COMPLETE, "完了／再開"),
            (DELETE, "削除"),
            (TEST, "通知を試す"),
            (EDIT, "編集"),
            (BACK, "← 一覧へ"),
            (IMPORT_CALENDAR, "休業日CSV"),
        ] {
            controls.push((
                id,
                child(w!("BUTTON"), label, WINDOW_STYLE(BS_OWNERDRAW as u32), id)?,
            ));
        }
        let mut labels = Vec::new();
        for label in [
            "タイトル *",
            "期限 *",
            "メモ",
            "通知間隔（分）",
            "通知スタイル",
            "繰り返し",
            "営業日補正",
        ] {
            labels.push(CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                &HSTRING::from(label),
                WS_CHILD | WS_VISIBLE,
                0,
                0,
                10,
                10,
                Some(hwnd),
                None,
                Some(instance),
                None,
            )?);
        }
        for (id, value) in [
            (HEADING, "期限と定期作業"),
            (
                SUBTITLE,
                "今日と明日の予定を確認し、完了した作業を整理できます。",
            ),
            (SUMMARY, ""),
            (CALENDAR_INFO, ""),
            (
                EMPTY,
                "予定はまだありません。\r\n「＋ 作業を追加」から登録してください。",
            ),
        ] {
            controls.push((
                id,
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    &HSTRING::from(value),
                    WS_CHILD
                        | WS_VISIBLE
                        | if id == EMPTY {
                            WINDOW_STYLE(1 /* SS_CENTER */)
                        } else {
                            WINDOW_STYLE(0)
                        },
                    0,
                    0,
                    10,
                    10,
                    Some(hwnd),
                    Some(HMENU(id as *mut _)),
                    Some(instance),
                    None,
                )?,
            ));
        }
        STATE.with(|c| {
            *c.borrow_mut() = Some(State {
                hwnd,
                resident,
                store,
                rows: Vec::new(),
                editing: None,
                controls,
                labels,
                font: HFONT::default(),
                heading_font: HFONT::default(),
                editor_open: false,
                baseline: None,
                alerts: Vec::new(),
                alert_tasks: Vec::new(),
                scroll: 0,
                refreshing: false,
                last_error: None,
                display_day: None,
            })
        });
        SendMessageW(
            control(DUE),
            DTM_SETFORMATW,
            None,
            Some(LPARAM(w!("yyyy/MM/dd").as_ptr() as isize)),
        );
        let range = [
            SYSTEMTIME {
                wYear: 1900,
                wMonth: 1,
                wDay: 1,
                ..Default::default()
            },
            SYSTEMTIME {
                wYear: 9998,
                wMonth: 12,
                wDay: 31,
                wHour: 23,
                wMinute: 59,
                wSecond: 59,
                ..Default::default()
            },
        ];
        SendMessageW(
            control(DUE),
            DTM_SETRANGE,
            Some(WPARAM((GDTR_MIN | GDTR_MAX) as usize)),
            Some(LPARAM(range.as_ptr() as isize)),
        );
        for (id, count, unit) in [(HOUR, 24, "時"), (MINUTE, 60, "分")] {
            for n in 0..count {
                let option = HSTRING::from(format!("{n:02}{unit}"));
                SendMessageW(
                    control(id),
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(option.as_ptr() as isize)),
                );
            }
        }
        for (id, options) in [
            (
                FILTER,
                vec!["今日・明日・期限超過", "すべての未完了", "完了済み"],
            ),
            (
                STYLE,
                vec!["全ディスプレイの赤いアラート", "Windows通知", "両方"],
            ),
            (
                REPEAT,
                vec![
                    "単発",
                    "毎日",
                    "毎週（同じ曜日）",
                    "毎月（同じ日、月末補正）",
                ],
            ),
            (
                ADJUSTMENT,
                vec!["補正しない", "前の営業日へ", "次の営業日へ"],
            ),
        ] {
            for option in options {
                SendMessageW(
                    control(id),
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(HSTRING::from(option).as_ptr() as isize)),
                );
            }
            select(id, 0);
        }
        let list = control(LIST);
        SendMessageW(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(
                (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as usize,
            )),
            Some(LPARAM(
                (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize,
            )),
        );
        let _ = SetWindowTheme(list, w!("Explorer"), PCWSTR::null());
        SendMessageW(
            list,
            LVM_SETBKCOLOR,
            None,
            Some(LPARAM(rgb(255, 255, 255).0 as isize)),
        );
        SendMessageW(
            list,
            LVM_SETTEXTBKCOLOR,
            None,
            Some(LPARAM(rgb(255, 255, 255).0 as isize)),
        );
        SendMessageW(list, LVM_SETTEXTCOLOR, None, Some(LPARAM(INK.0 as isize)));
        for (i, (name, width)) in [
            ("状態", 78),
            ("期限", 150),
            ("タイトル", 220),
            ("繰り返し", 85),
            ("通知間隔", 90),
            ("メモ", 240),
        ]
        .into_iter()
        .enumerate()
        {
            let name = HSTRING::from(name);
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH,
                pszText: windows::core::PWSTR(name.as_ptr() as *mut _),
                cx: width,
                ..Default::default()
            };
            SendMessageW(
                list,
                LVM_INSERTCOLUMNW,
                Some(WPARAM(i)),
                Some(LPARAM(&col as *const _ as isize)),
            );
        }
        font_and_layout();
        initial_bounds(hwnd);
        clear_form();
        refresh();
        if SetTimer(Some(hwnd), TIMER, 30_000, None) == 0 {
            shutdown();
            return Err(Error::from_thread());
        }
        show();
        tick();
    }
    Ok(())
}

pub fn show() {
    if let Some(hwnd) = with(|s| s.hwnd) {
        refresh();
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}
pub fn dialog_message(message: &MSG) -> bool {
    with(|s| s.hwnd).is_some_and(|h| unsafe {
        IsWindowVisible(h).as_bool()
            && (message.hwnd == h || IsChild(h, message.hwnd).as_bool())
            && IsDialogMessageW(h, message).as_bool()
    })
}
pub fn shutdown() {
    close_alerts();
    let hwnd = with(|s| s.hwnd);
    if let Some(hwnd) = hwnd {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
    let state = STATE.with(|c| c.borrow_mut().take());
    if let Some(s) = state {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(s.font.0));
            let _ = DeleteObject(HGDIOBJ(s.heading_font.0));
        }
    }
}

fn clear_form() {
    with_mut(|s| s.editing = None);
    let item = LVITEMW {
        stateMask: LVIS_SELECTED,
        ..Default::default()
    };
    unsafe {
        SendMessageW(
            control(LIST),
            LVM_SETITEMSTATE,
            Some(WPARAM(usize::MAX)),
            Some(LPARAM(&item as *const _ as isize)),
        );
    }
    set(TITLE, "");
    set_due(Local::now().date_naive().and_hms_opt(18, 0, 0).unwrap());
    set(MEMO, "");
    set(INTERVAL, "15");
    select(STYLE, 2);
    select(REPEAT, 0);
    select(ADJUSTMENT, 2);
    calendar_info();
}
fn selected_task() -> Option<Task> {
    let row = unsafe {
        SendMessageW(
            control(LIST),
            LVM_GETNEXTITEM,
            Some(WPARAM(usize::MAX)),
            Some(LPARAM(LVNI_SELECTED as isize)),
        )
        .0
    };
    if row < 0 {
        return None;
    }
    with(|s| {
        s.rows
            .get(row as usize)
            .and_then(|id| s.store.tasks().iter().find(|t| t.id == *id))
            .cloned()
    })
    .flatten()
}
fn load_selected() {
    if let Some(t) = selected_task() {
        with_mut(|s| s.editing = Some(t.id));
        set(TITLE, &t.title);
        set_due(t.nominal_due.unwrap_or(t.due));
        select(
            ADJUSTMENT,
            match t.adjustment {
                Adjustment::None => 0,
                Adjustment::Previous => 1,
                Adjustment::Next => 2,
            },
        );
        calendar_info();
        set(MEMO, &t.memo);
        set(INTERVAL, &t.interval.to_string());
        select(
            STYLE,
            match t.style {
                Style::Alert => 0,
                Style::Windows => 1,
                Style::Both => 2,
            },
        );
        select(
            REPEAT,
            match t.repeat {
                Repeat::Once => 0,
                Repeat::Daily => 1,
                Repeat::Weekly => 2,
                Repeat::Monthly => 3,
            },
        );
    }
}
fn form() -> std::io::Result<Task> {
    let fail = |message: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, message);
    let due = due_value()?;
    let mut task = Task::new(text(control(TITLE)), due);
    task.adjustment = adjustment();
    task.nominal_due = (task.adjustment != Adjustment::None).then_some(due);
    task.due = with(|s| s.store.calendar().adjust(due, task.adjustment)).unwrap_or(Ok(due))?;
    task.memo = text(control(MEMO));
    let interval_text = text(control(INTERVAL));
    task.interval = if interval_text.trim().is_empty() {
        "15"
    } else {
        interval_text.trim()
    }
    .parse()
    .map_err(|_| fail("通知間隔は整数の分で入力してください。"))?;
    task.style = match selection(STYLE) {
        0 => Style::Alert,
        1 => Style::Windows,
        _ => Style::Both,
    };
    task.repeat = match selection(REPEAT) {
        1 => Repeat::Daily,
        2 => Repeat::Weekly,
        3 => Repeat::Monthly,
        _ => Repeat::Once,
    };
    if let Some(Some(old)) = with(|s| {
        s.editing
            .and_then(|id| s.store.tasks().iter().find(|t| t.id == id))
            .cloned()
    }) {
        task.id = old.id;
        task.completed = old.completed;
        task.last_completed = old.last_completed;
        if old.due == task.due && old.nominal_due.unwrap_or(old.due) == due {
            task.monthly_day = old.monthly_day;
            task.last_notified = old.last_notified;
            task.snoozed_until = old.snoozed_until;
        }
    }
    task.validate()?;
    Ok(task)
}
fn refresh() {
    let filter = selection(FILTER);
    let today = Local::now().date_naive();
    with_mut(|s| s.display_day = Some(today));
    let tomorrow = today.checked_add_days(Days::new(1)).unwrap_or(today);
    let tasks = with(|s| {
        let mut tasks: Vec<_> = s
            .store
            .tasks()
            .iter()
            .filter(|t| match filter {
                0 => !t.completed && t.due.date() <= tomorrow,
                1 => !t.completed,
                _ => t.completed,
            })
            .cloned()
            .collect();
        tasks.sort_by_key(|t| (t.due, t.id));
        tasks
    })
    .unwrap_or_default();
    with_mut(|s| s.rows = tasks.iter().map(|t| t.id).collect());
    let list = control(LIST);
    with_mut(|s| s.refreshing = true);
    unsafe {
        SendMessageW(list, LVM_DELETEALLITEMS, None, None);
    }
    for (row, t) in tasks.iter().enumerate() {
        let status = if t.completed {
            "完了"
        } else if t.due < Local::now().naive_local() {
            "期限超過"
        } else if t.due.date() == today {
            "今日"
        } else if t.due.date() == tomorrow {
            "明日"
        } else {
            "予定"
        };
        let repeat = match t.repeat {
            Repeat::Once => "単発",
            Repeat::Daily => "毎日",
            Repeat::Weekly => "毎週",
            Repeat::Monthly => "毎月",
        };
        for (column, value) in [
            status.to_owned(),
            t.due.format("%Y-%m-%d %H:%M").to_string(),
            t.title.clone(),
            repeat.to_owned(),
            format!("{}分", t.interval),
            t.memo.replace(['\r', '\n'], " "),
        ]
        .iter()
        .enumerate()
        {
            let value = HSTRING::from(value);
            let item = LVITEMW {
                mask: LVIF_TEXT,
                iItem: row as i32,
                iSubItem: column as i32,
                pszText: windows::core::PWSTR(value.as_ptr() as *mut _),
                ..Default::default()
            };
            unsafe {
                SendMessageW(
                    list,
                    if column == 0 {
                        LVM_INSERTITEMW
                    } else {
                        LVM_SETITEMW
                    },
                    None,
                    Some(LPARAM(&item as *const _ as isize)),
                );
            }
        }
    }
    if let Some(Some(editing)) = with(|s| s.editing) {
        if let Some(row) = tasks.iter().position(|t| t.id == editing) {
            let item = LVITEMW {
                state: LVIS_SELECTED,
                stateMask: LVIS_SELECTED,
                ..Default::default()
            };
            unsafe {
                SendMessageW(
                    list,
                    LVM_SETITEMSTATE,
                    Some(WPARAM(row)),
                    Some(LPARAM(&item as *const _ as isize)),
                );
            }
        }
    }
    with_mut(|s| s.refreshing = false);
    let counts = with(|s| {
        let pending: Vec<_> = s.store.tasks().iter().filter(|t| !t.completed).collect();
        (
            pending.iter().filter(|t| t.due.date() == today).count(),
            pending.iter().filter(|t| t.due.date() == tomorrow).count(),
            pending
                .iter()
                .filter(|t| t.due < Local::now().naive_local())
                .count(),
        )
    })
    .unwrap_or_default();
    set(
        SUMMARY,
        &format!(
            "今日  {}件     明日  {}件     期限超過  {}件",
            counts.0, counts.1, counts.2
        ),
    );
    let selected = selected_task().is_some();
    for id in [EDIT, COMPLETE, DELETE] {
        unsafe {
            let _ = EnableWindow(control(id), selected);
        }
    }
    layout();
}

fn form_snapshot() -> (Vec<String>, [usize; 5]) {
    (
        [TITLE, DUE, MEMO, INTERVAL]
            .into_iter()
            .map(|id| text(control(id)))
            .collect(),
        [
            selection(STYLE),
            selection(REPEAT),
            selection(HOUR),
            selection(MINUTE),
            selection(ADJUSTMENT),
        ],
    )
}
fn set_mode(editor: bool) {
    let baseline = editor.then(form_snapshot);
    with_mut(|s| {
        s.editor_open = editor;
        s.scroll = 0;
        s.baseline = baseline;
    });
    set(
        HEADING,
        if editor {
            if with(|s| s.editing.is_some()).unwrap_or(false) {
                "作業を編集"
            } else {
                "新しい作業"
            }
        } else {
            "期限と定期作業"
        },
    );
    set(
        SUBTITLE,
        if editor {
            "カレンダーと時刻で期限を指定し、営業日補正を選べます。"
        } else {
            "今日と明日の予定を確認し、完了した作業を整理できます。"
        },
    );
    layout();
}

fn command(id: usize) {
    match id {
        IMPORT_CALENDAR => import_calendar(),
        NEW => {
            clear_form();
            set_mode(true);
            unsafe {
                let _ = SetFocus(Some(control(TITLE)));
            }
        }
        EDIT => {
            if selected_task().is_some() {
                load_selected();
                set_mode(true);
                unsafe {
                    let _ = SetFocus(Some(control(TITLE)));
                }
            }
        }
        BACK => {
            let dirty = with(|s| s.baseline.as_ref() != Some(&form_snapshot())).unwrap_or(false);
            if !dirty
                || unsafe {
                    MessageBoxW(
                        with(|s| s.hwnd),
                        w!("入力中の変更を破棄して一覧へ戻りますか？"),
                        w!("yyclip"),
                        MB_YESNO | MB_ICONQUESTION,
                    )
                } == IDYES
            {
                set_mode(false);
            }
        }
        SAVE => match form() {
            Ok(task) => {
                let result = with_mut(|s| s.store.save(task));
                match result {
                    Some(Ok(id)) => {
                        with_mut(|s| s.editing = Some(id));
                        set_mode(false);
                        refresh();
                        close_alerts();
                    }
                    Some(Err(e)) => error(&e.to_string()),
                    None => {}
                }
            }
            Err(e) => error(&e.to_string()),
        },
        COMPLETE | DELETE => {
            let Some(task) = selected_task() else {
                error("一覧で対象の作業を選択してください。");
                return;
            };
            if id == DELETE
                && unsafe {
                    MessageBoxW(
                        with(|s| s.hwnd),
                        &HSTRING::from(format!(
                            "「{}」を削除しますか？ 定期作業は今後の通知も停止します。",
                            task.title
                        )),
                        w!("yyclip"),
                        MB_YESNO | MB_ICONQUESTION,
                    )
                } != IDYES
            {
                return;
            }
            let result = with_mut(|s| {
                if id == COMPLETE {
                    if task.completed {
                        let mut task = task.clone();
                        task.completed = false;
                        task.last_notified = None;
                        task.snoozed_until = None;
                        s.store.save(task).map(|_| ())
                    } else {
                        s.store.complete(task.id, Local::now().naive_local())
                    }
                } else {
                    s.store.remove(task.id)
                }
            });
            match result {
                Some(Ok(())) => {
                    refresh();
                    clear_form();
                    close_alerts();
                }
                Some(Err(e)) => error(&e.to_string()),
                None => {}
            }
        }
        TEST => match form() {
            Ok(mut task) => {
                task.id = 0; // Preview actions must never postpone a saved task.
                if let Err(e) = notify(&[task]) {
                    error(&e.to_string());
                }
            }
            Err(e) => error(&e.to_string()),
        },
        FILTER => refresh(),
        _ => {}
    }
}

struct CalendarPicker {
    // Release COM interfaces before uninitializing the apartment.
    dialog: IFileOpenDialog,
    _apartment: crate::resident::ComApartment,
}
impl CalendarPicker {
    fn new() -> Result<Self> {
        let apartment = crate::resident::ComApartment::new()
            .ok_or_else(|| Error::new(E_FAIL, "CSV選択画面を初期化できません。"))?;
        let dialog: IFileOpenDialog =
            unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)? };
        unsafe {
            dialog.SetOptions(dialog.GetOptions()? | FOS_FORCEFILESYSTEM)?;
            dialog.SetFileTypes(&[COMDLG_FILTERSPEC {
                pszName: w!("休業日CSV (*.csv)"),
                pszSpec: w!("*.csv"),
            }])?;
        }
        Ok(Self {
            dialog,
            _apartment: apartment,
        })
    }
}
fn import_calendar() {
    let hwnd = with(|s| s.hwnd).unwrap_or_default();
    let chosen = (|| -> Result<std::path::PathBuf> {
        unsafe {
            let picker = CalendarPicker::new()?;
            let dialog = &picker.dialog;
            dialog.Show(Some(hwnd))?;
            let name = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = name.to_string();
            CoTaskMemFree(Some(name.0 as *const _));
            Ok(std::path::PathBuf::from(path?))
        }
    })();
    let path = match chosen {
        Ok(p) => p,
        Err(e) if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) => return,
        Err(e) => {
            error(&format!("CSVを選択できません: {e}"));
            return;
        }
    };
    let parsed = (|| -> std::io::Result<Calendar> {
        if std::fs::metadata(&path)?.len() > 4 * 1024 * 1024 {
            return Err(std::io::Error::other("CSVは4 MiB以下にしてください。"));
        }
        Calendar::from_csv(
            &std::fs::read(&path)?,
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        )
    })();
    let calendar = match parsed {
        Ok(c) => c,
        Err(e) => {
            error(&e.to_string());
            return;
        }
    };
    let confirm = format!(
        "{}\n休業日 {}日を登録し、現在のCSVを差し替えます。\n営業日補正を設定した未完了タスクも補正し直します。\n\n会社の休業日を追加する場合は、祝日も含めたCSVを指定してください。",
        calendar.name,
        calendar.holidays.len()
    );
    if unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(confirm),
            w!("休業日CSVの登録"),
            MB_YESNO | MB_ICONQUESTION,
        )
    } != IDYES
    {
        return;
    }
    match with_mut(|s| s.store.register_calendar(calendar)) {
        Some(Ok(())) => {
            refresh();
            close_alerts();
            calendar_info();
        }
        Some(Err(e)) => error(&e.to_string()),
        None => (),
    }
}

fn tick() {
    tick_at(Local::now().naive_local(), Local::now().timestamp());
}

fn tick_at(local: NaiveDateTime, utc: i64) {
    if with(|s| s.display_day != Some(local.date())).unwrap_or(false) {
        refresh();
    }
    let tasks = with(|s| {
        s.store
            .tasks()
            .iter()
            .filter(|t| t.needs_notification(local, utc))
            .cloned()
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    if tasks.is_empty() {
        return;
    }
    let result = notify(&tasks).and_then(|()| {
        with_mut(|s| {
            s.store
                .notified(&tasks.iter().map(|t| t.id).collect::<Vec<_>>(), utc)
        })
        .unwrap_or_else(|| Err(std::io::Error::other("期限管理が終了しています。")))
        .map_err(|e| Error::new(E_FAIL, e.to_string()))
    });
    if let Err(e) = result {
        let message = e.to_string();
        if with(|s| s.last_error.as_ref() != Some(&message)).unwrap_or(false) {
            with_mut(|s| s.last_error = Some(message.clone()));
            error(&message);
        }
    } else {
        with_mut(|s| s.last_error = None);
    }
}

fn close_alerts() {
    let handles = with_mut(|s| {
        s.alert_tasks.clear();
        std::mem::take(&mut s.alerts)
    })
    .unwrap_or_default();
    for (h, font) in handles {
        unsafe {
            let _ = DestroyWindow(h);
            let _ = DeleteObject(HGDIOBJ(font.0));
        }
    }
}

fn notify(tasks: &[Task]) -> Result<()> {
    let red: Vec<_> = tasks.iter().filter(|t| t.style != Style::Windows).collect();
    let native: Vec<_> = tasks.iter().filter(|t| t.style != Style::Alert).collect();
    if !red.is_empty() {
        show_alerts(&red)?;
    }
    if !native.is_empty() {
        let summary = native
            .iter()
            .take(4)
            .map(|t| format!("{} {}", t.due.format("%m/%d %H:%M"), t.title))
            .collect::<Vec<_>>()
            .join("\n");
        let resident = with(|s| s.resident).unwrap_or_default();
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: resident,
            uID: 1,
            uFlags: NIF_INFO,
            dwInfoFlags: NIIF_WARNING,
            ..Default::default()
        };
        let title = format!("yyclip 期限通知（{}件）", native.len());
        copy_wide(&mut data.szInfoTitle, &title);
        copy_wide(&mut data.szInfo, &summary);
        if !unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool() {
            return Err(Error::from_thread());
        }
    }
    Ok(())
}
fn copy_wide<const N: usize>(out: &mut [u16; N], value: &str) {
    let mut at = 0;
    for c in value.chars() {
        if at + c.len_utf16() >= N {
            break;
        }
        at += c.encode_utf16(&mut out[at..]).len();
    }
}

fn show_alerts(tasks: &[&Task]) -> Result<()> {
    close_alerts();
    with_mut(|s| {
        s.alert_tasks = tasks
            .iter()
            .filter_map(|t| (t.id != 0).then_some(t.id))
            .collect()
    });
    let mut monitors = Vec::<(RECT, u32)>::new();
    unsafe extern "system" fn enumerate(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            let (mut x, mut y) = (96, 96);
            let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) };
            unsafe { &mut *(data.0 as *mut Vec<(RECT, u32)>) }.push((info.rcWork, x.max(96)));
        }
        BOOL(1)
    }
    if !unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(enumerate),
            LPARAM(&mut monitors as *mut _ as isize),
        )
    }
    .as_bool()
    {
        return Err(Error::from_thread());
    }
    if monitors.is_empty() {
        return Err(Error::new(E_FAIL, "ディスプレイを取得できません。"));
    }
    let first = tasks[0];
    let title: String = first.title.chars().take(10).collect();
    let title = if first.title.chars().count() > 10 {
        format!("{title}…")
    } else {
        title
    };
    let message = format!(
        "{}\r\n{} · {}件",
        title,
        first.due.format("%m/%d %H:%M"),
        tasks.len(),
    );
    let instance = unsafe { GetModuleHandleW(None) }?.into();
    for (work, dpi) in monitors {
        let factor = layout_scale(dpi);
        let scale = |n: i32| (n * factor / 1000).max(1);
        let width = scale(210).min(work.right - work.left);
        let height = scale(112).min(work.bottom - work.top);
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                ALERT_CLASS,
                w!("yyclip 期限アラート"),
                WS_POPUP | WS_BORDER,
                work.right - width,
                work.top,
                width,
                height,
                None,
                None,
                Some(instance),
                None,
            )
        }?;
        let font = make_font(dpi);
        with_mut(|s| s.alerts.push((hwnd, font)));
        let label = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                &HSTRING::from(&message),
                WS_CHILD | WS_VISIBLE,
                scale(10),
                scale(7),
                width - scale(20),
                scale(40),
                Some(hwnd),
                None,
                Some(instance),
                None,
            )
        }?;
        unsafe {
            SendMessageW(label, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
        }
        let half = (width - scale(28)) / 2;
        let delay = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("COMBOBOX"),
                w!(""),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | WS_VSCROLL
                    | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
                scale(10),
                height - scale(62),
                half,
                scale(180),
                Some(hwnd),
                Some(HMENU(SNOOZE as *mut _)),
                Some(instance),
                None,
            )
        }?;
        unsafe {
            SendMessageW(delay, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
            for minutes in 0..=MAX_SNOOZE {
                let option = HSTRING::from(if minutes == 0 {
                    "通常間隔".into()
                } else {
                    format!("{minutes}分後")
                });
                SendMessageW(
                    delay,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(option.as_ptr() as isize)),
                );
            }
            SendMessageW(delay, CB_SETCURSEL, Some(WPARAM(0)), None);
        }
        for (id, title, x, y, w) in [
            (
                DISMISS,
                "一時消去",
                scale(18) + half,
                height - scale(62),
                half,
            ),
            (
                OPEN,
                "一覧・完了",
                scale(10),
                height - scale(33),
                width - scale(20),
            ),
        ] {
            let b = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("BUTTON"),
                    &HSTRING::from(title),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                    x,
                    y,
                    w,
                    scale(25),
                    Some(hwnd),
                    Some(HMENU(id as *mut _)),
                    Some(instance),
                    None,
                )
            }?;
            unsafe {
                SendMessageW(b, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
            }
        }
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }
    Ok(())
}

fn dismiss_alert(hwnd: HWND) {
    let font = with_mut(|s| {
        let font = s
            .alerts
            .iter()
            .position(|entry| entry.0 == hwnd)
            .map(|index| s.alerts.remove(index).1);
        if s.alerts.is_empty() {
            s.alert_tasks.clear();
        }
        font
    })
    .flatten();
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    if let Some(font) = font {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(font.0));
        }
    }
}
fn snooze_alert(hwnd: HWND, utc: i64) {
    let minutes = unsafe {
        GetDlgItem(Some(hwnd), SNOOZE as i32)
            .map(|combo| SendMessageW(combo, CB_GETCURSEL, None, None).0)
            .unwrap_or(-1)
    };
    if minutes == 0 {
        close_alerts();
        return;
    }
    if !(1..=MAX_SNOOZE as isize).contains(&minutes) {
        error("次回通知は1～90分後で選択してください。");
        return;
    }
    let ids = with(|s| s.alert_tasks.clone()).unwrap_or_default();
    if ids.is_empty() {
        close_alerts();
        return;
    } // Unsaved notification preview.
    match with_mut(|s| s.store.snooze(&ids, minutes as u32, utc)) {
        Some(Ok(())) => close_alerts(),
        Some(Err(e)) => error(&e.to_string()),
        None => (),
    }
}
fn font_spec(dpi: u32) -> LOGFONTW {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    if unsafe {
        SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            metrics.cbSize,
            Some(&mut metrics as *mut _ as *mut _),
            0,
            dpi,
        )
    }
    .is_ok()
    {
        return metrics.lfMessageFont;
    }
    let mut lf = LOGFONTW {
        lfHeight: -(16 * dpi as i32 / 96),
        ..Default::default()
    };
    copy_wide(&mut lf.lfFaceName, "Segoe UI");
    lf
}
fn make_font(dpi: u32) -> HFONT {
    unsafe { CreateFontIndirectW(&font_spec(dpi)) }
}
fn layout_scale(dpi: u32) -> i32 {
    ((dpi as i32 * 1000) / 96).max(font_spec(dpi).lfHeight.abs() * 1000 / 16)
}

fn font_and_layout() {
    let Some(hwnd) = with(|s| s.hwnd) else {
        return;
    };
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let font = make_font(dpi);
    let old = with_mut(|s| {
        let old = s.font;
        s.font = font;
        old
    });
    let handles = with(|s| {
        s.controls
            .iter()
            .map(|c| c.1)
            .chain(s.labels.iter().copied())
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    for h in handles {
        unsafe {
            SendMessageW(
                h,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }
    unsafe {
        SendMessageW(
            control(DUE),
            DTM_SETMCFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
    }
    let mut spec = font_spec(dpi);
    spec.lfHeight = spec.lfHeight * 3 / 2;
    spec.lfWeight = 600;
    let heading = unsafe { CreateFontIndirectW(&spec) };
    let previous = with_mut(|s| {
        let previous = s.heading_font;
        s.heading_font = heading;
        previous
    });
    unsafe {
        SendMessageW(
            control(HEADING),
            WM_SETFONT,
            Some(WPARAM(heading.0 as usize)),
            Some(LPARAM(1)),
        );
    }
    if let Some(previous) = previous {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(previous.0));
        }
    }
    if let Some(old) = old {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(old.0));
        }
    }
    layout();
}
fn initial_bounds(hwnd: HWND) {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) }
        .as_bool()
    {
        return;
    }
    let work = info.rcWork;
    let factor = layout_scale(unsafe { GetDpiForWindow(hwnd) }.max(96));
    let width = (900 * factor / 1000).min((work.right - work.left - 32).max(1));
    let height = (650 * factor / 1000).min((work.bottom - work.top - 32).max(1));
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            work.left + (work.right - work.left - width) / 2,
            work.top + (work.bottom - work.top - height) / 2,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn layout() {
    let Some((hwnd, editor, rows_empty)) = with(|s| (s.hwnd, s.editor_open, s.rows.is_empty()))
    else {
        return;
    };
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    let factor = layout_scale(unsafe { GetDpiForWindow(hwnd) }.max(96));
    let px = |n: i32| n * factor / 1000;
    let (width, height) = (r.right, r.bottom);
    let margin = px(24).min(width / 16).max(6);
    let inner = (width - margin * 2).max(1);
    let compact = height < px(480);
    let tiny = height < px(340);
    let wide = inner >= px(600);
    let form_height = if wide {
        if compact { 472 } else { 536 }
    } else {
        704
    };
    let virtual_height = if editor {
        height.max(px(form_height + 72))
    } else {
        height
    };
    let scroll = with_mut(|s| {
        s.scroll = s.scroll.clamp(0, (virtual_height - height).max(0));
        s.scroll
    })
    .unwrap_or(0);
    let info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
        nMin: 0,
        nMax: virtual_height - 1,
        nPage: height.max(0) as u32,
        nPos: scroll,
        ..Default::default()
    };
    unsafe {
        SetScrollInfo(hwnd, SB_VERT, &info, true);
    }
    let handles = with(|s| s.controls.clone()).unwrap_or_default();
    for (id, h) in handles {
        let visible = match id {
            HEADING => true,
            SUBTITLE => !compact || editor,
            SUMMARY => !editor && !tiny,
            FILTER | LIST | NEW | EDIT | COMPLETE | DELETE => !editor,
            EMPTY => !editor && rows_empty && height > px(350),
            _ => editor,
        };
        unsafe {
            let _ = ShowWindow(h, if visible { SW_SHOWNA } else { SW_HIDE });
        }
    }
    let labels = with(|s| s.labels.clone()).unwrap_or_default();
    for &label in &labels {
        unsafe {
            let _ = ShowWindow(label, if editor { SW_SHOWNA } else { SW_HIDE });
        }
    }
    let place = |id, x, y, w, h| unsafe {
        let _ = MoveWindow(control(id), x, y - scroll, w, h, true);
    };
    place(
        HEADING,
        margin,
        px(if tiny { 10 } else { 18 }),
        inner,
        px(34),
    );
    place(SUBTITLE, margin, px(58), inner, px(34));
    let footer_y = (height - px(54)).max(0);
    if editor {
        let start = if compact { 80 } else { 104 };
        let gap = px(16);
        let half = (inner - gap) / 2;
        let row = |label_index: usize, id, x, y, w, h| {
            unsafe {
                let _ = MoveWindow(labels[label_index], x, px(y) - scroll, w, px(20), true);
            }
            place(
                id,
                x,
                px(y + 24),
                w,
                if [STYLE, REPEAT, ADJUSTMENT].contains(&id) {
                    px(180)
                } else {
                    px(h)
                },
            );
        };
        row(0, TITLE, margin, start, inner, 32);
        let due_row = |y: i32, w: i32| {
            let time_width = px(64);
            let time_gap = px(6);
            let date_width = (w - time_width * 2 - time_gap * 2).max(1);
            row(1, DUE, margin, y, w, 32);
            place(DUE, margin, px(y + 24), date_width, px(32));
            place(
                HOUR,
                margin + date_width + time_gap,
                px(y + 24),
                time_width,
                px(180),
            );
            place(
                MINUTE,
                margin + date_width + time_gap * 2 + time_width,
                px(y + 24),
                time_width,
                px(180),
            );
        };
        let business_y = if wide {
            let step = if compact { 62 } else { 74 };
            due_row(start + step, half);
            row(5, REPEAT, margin + half + gap, start + step, half, 32);
            let memo_y = start + step * 2;
            let memo_h = if compact { 40 } else { 64 };
            row(2, MEMO, margin, memo_y, inner, memo_h);
            row(3, INTERVAL, margin, memo_y + memo_h + 40, half, 32);
            row(
                4,
                STYLE,
                margin + half + gap,
                memo_y + memo_h + 40,
                half,
                32,
            );
            memo_y + memo_h + 112
        } else {
            due_row(start + 72, inner);
            row(5, REPEAT, margin, start + 144, inner, 32);
            row(2, MEMO, margin, start + 216, inner, 64);
            row(3, INTERVAL, margin, start + 320, inner, 32);
            row(4, STYLE, margin, start + 392, inner, 32);
            start + 464
        };
        row(6, ADJUSTMENT, margin, business_y, inner - px(140), 32);
        place(
            IMPORT_CALENDAR,
            margin + inner - px(128),
            px(business_y + 24),
            px(128),
            px(32),
        );
        place(CALENDAR_INFO, margin, px(business_y + 62), inner, px(44));
        let bw = ((inner - px(16)) / 3).max(1);
        for (i, id) in [BACK, TEST, SAVE].into_iter().enumerate() {
            unsafe {
                let _ = MoveWindow(
                    control(id),
                    margin + i as i32 * (bw + px(8)),
                    footer_y,
                    bw,
                    px(34),
                    true,
                );
            }
        }
    } else {
        let summary_y = if compact { 58 } else { 105 };
        place(SUMMARY, margin, px(summary_y), inner, px(28));
        let toolbar = if tiny { 56 } else { summary_y + 42 };
        let stacked = inner < px(520) && !tiny;
        let add_width = px(if tiny { 112 } else { 160 });
        let filter_w = if stacked {
            inner
        } else {
            (inner - add_width - px(16)).min(px(350))
        };
        place(FILTER, margin, px(toolbar), filter_w, px(190));
        place(
            NEW,
            if stacked {
                margin
            } else {
                width - margin - add_width
            },
            px(toolbar + if stacked { 42 } else { 0 }),
            if stacked { inner } else { add_width },
            px(34),
        );
        let list_top = px(toolbar + if stacked { 88 } else { 48 });
        let list_height = (footer_y - list_top - px(12)).max(1);
        place(LIST, margin, list_top, inner, list_height);
        place(
            EMPTY,
            margin + px(16),
            list_top + (list_height - px(62)).max(0) / 2,
            inner - px(32),
            px(62),
        );
        let bw = ((inner - px(16)) / 3).min(px(140)).max(1);
        for (i, id) in [EDIT, COMPLETE, DELETE].into_iter().enumerate() {
            unsafe {
                let _ = MoveWindow(
                    control(id),
                    margin + i as i32 * (bw + px(8)),
                    footer_y,
                    bw,
                    px(34),
                    true,
                );
            }
        }
        let extras = inner >= px(650);
        let status = px(64);
        let due = px(136);
        let repeat = if extras { px(88) } else { 0 };
        let interval = if extras { px(74) } else { 0 };
        let title = (inner - status - due - repeat - interval - px(6)).max(px(70));
        for (column, w) in [status, due, title, repeat, interval, 0]
            .into_iter()
            .enumerate()
        {
            unsafe {
                SendMessageW(
                    control(LIST),
                    LVM_SETCOLUMNWIDTH,
                    Some(WPARAM(column)),
                    Some(LPARAM(w as isize)),
                );
            }
        }
    }
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

unsafe fn draw_button(draw: &DRAWITEMSTRUCT) {
    let primary = draw.CtlID == SAVE as u32 || draw.CtlID == NEW as u32;
    let disabled = draw.itemState.0 & ODS_DISABLED.0 != 0;
    let pressed = draw.itemState.0 & ODS_SELECTED.0 != 0;
    let background = if disabled {
        rgb(226, 232, 240)
    } else if primary {
        if pressed { rgb(29, 78, 216) } else { BLUE }
    } else {
        rgb(255, 255, 255)
    };
    let foreground = if disabled {
        MUTED
    } else if primary {
        rgb(255, 255, 255)
    } else if draw.CtlID == DELETE as u32 {
        rgb(185, 28, 28)
    } else {
        INK
    };
    unsafe {
        SetDCBrushColor(draw.hDC, background);
        let old_brush = SelectObject(draw.hDC, GetStockObject(DC_BRUSH));
        let pen = CreatePen(
            PS_SOLID,
            1,
            if primary {
                background
            } else {
                rgb(203, 213, 225)
            },
        );
        let old_pen = SelectObject(draw.hDC, HGDIOBJ(pen.0));
        let r = draw.rcItem;
        let _ = RoundRect(draw.hDC, r.left, r.top, r.right, r.bottom, 10, 10);
        SelectObject(draw.hDC, old_pen);
        let _ = DeleteObject(HGDIOBJ(pen.0));
        SelectObject(draw.hDC, old_brush);
        SetBkMode(draw.hDC, TRANSPARENT);
        SetTextColor(draw.hDC, foreground);
        let font = with(|s| s.font).unwrap_or_default();
        let old_font = SelectObject(draw.hDC, HGDIOBJ(font.0));
        let mut value: Vec<u16> = text(draw.hwndItem).encode_utf16().collect();
        let mut bounds = r;
        DrawTextW(
            draw.hDC,
            &mut value,
            &mut bounds,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        SelectObject(draw.hDC, old_font);
        if draw.itemState.0 & ODS_FOCUS.0 != 0 {
            let mut focus = r;
            let _ = InflateRect(&mut focus, -4, -4);
            let _ = DrawFocusRect(draw.hDC, &focus);
        }
    }
}

unsafe extern "system" fn manager_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_CTLCOLORSTATIC => {
            let dc = HDC(w.0 as *mut _);
            let id = unsafe { GetDlgCtrlID(HWND(l.0 as *mut _)) } as usize;
            unsafe {
                SetTextColor(
                    dc,
                    if id == HEADING {
                        INK
                    } else if id == SUMMARY {
                        BLUE
                    } else {
                        MUTED
                    },
                );
                let background = if id == EMPTY {
                    rgb(255, 255, 255)
                } else {
                    BACKGROUND
                };
                SetBkColor(dc, background);
                SetDCBrushColor(dc, background);
                SetBkMode(dc, TRANSPARENT);
            }
            LRESULT(unsafe { GetStockObject(DC_BRUSH) }.0 as isize)
        }
        WM_CTLCOLOREDIT => {
            let dc = HDC(w.0 as *mut _);
            unsafe {
                SetTextColor(dc, INK);
                SetBkColor(dc, rgb(255, 255, 255));
                SetDCBrushColor(dc, rgb(255, 255, 255));
            }
            LRESULT(unsafe { GetStockObject(DC_BRUSH) }.0 as isize)
        }
        WM_ERASEBKGND => {
            let dc = HDC(w.0 as *mut _);
            let mut bounds = RECT::default();
            unsafe {
                let _ = GetClientRect(hwnd, &mut bounds);
                SetDCBrushColor(dc, BACKGROUND);
                FillRect(dc, &bounds, HBRUSH(GetStockObject(DC_BRUSH).0));
            }
            LRESULT(1)
        }
        WM_DRAWITEM => {
            if l.0 != 0 {
                unsafe {
                    draw_button(&*(l.0 as *const DRAWITEMSTRUCT));
                }
            }
            LRESULT(1)
        }
        WM_COMMAND => {
            let id = w.0 & 0xffff;
            let code = (w.0 >> 16) & 0xffff;
            if [HOUR, MINUTE, ADJUSTMENT].contains(&id) && code == CBN_SELCHANGE as usize {
                calendar_info();
            }
            if code == BN_CLICKED as usize || (id == FILTER && code == CBN_SELCHANGE as usize) {
                command(id);
            }
            LRESULT(0)
        }
        WM_NOTIFY => {
            if l.0 != 0 {
                let header = unsafe { &*(l.0 as *const NMHDR) };
                if header.idFrom == DUE && header.code == DTN_DATETIMECHANGE {
                    calendar_info();
                }
                if header.idFrom == LIST && header.code == NM_DBLCLK {
                    command(EDIT);
                }
                if header.idFrom == LIST
                    && header.code == LVN_ITEMCHANGED
                    && with(|s| !s.refreshing).unwrap_or(false)
                {
                    let change = unsafe { &*(l.0 as *const NMLISTVIEW) };
                    if change.uNewState & LVIS_SELECTED.0 != 0
                        && change.uOldState & LVIS_SELECTED.0 == 0
                    {
                        load_selected();
                        for id in [EDIT, COMPLETE, DELETE] {
                            unsafe {
                                let _ = EnableWindow(control(id), true);
                            }
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_TIMER if w.0 == TIMER => {
            tick();
            LRESULT(0)
        }
        WM_VSCROLL => {
            let mut info = SCROLLINFO {
                cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                fMask: SIF_ALL,
                ..Default::default()
            };
            unsafe {
                let _ = GetScrollInfo(hwnd, SB_VERT, &mut info);
            }
            let next = match w.0 as u32 & 0xffff {
                0 => info.nPos - 30,
                1 => info.nPos + 30,
                2 => info.nPos - info.nPage as i32,
                3 => info.nPos + info.nPage as i32,
                4 | 5 => info.nTrackPos,
                6 => 0,
                7 => info.nMax,
                _ => info.nPos,
            };
            with_mut(|s| s.scroll = next);
            layout();
            LRESULT(0)
        }
        WM_SIZE => {
            layout();
            LRESULT(0)
        }
        WM_DPICHANGED => {
            if l.0 != 0 {
                let r = unsafe { &*(l.0 as *const RECT) };
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
            font_and_layout();
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            if l.0 != 0 {
                let info = unsafe { &mut *(l.0 as *mut MINMAXINFO) };
                info.ptMinTrackSize = POINT { x: 620, y: 490 };
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

unsafe extern "system" fn alert_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(w.0 as *mut _);
            unsafe {
                SetTextColor(hdc, COLORREF(0xffffff));
                SetBkColor(hdc, COLORREF(0x2020bc));
                SetDCBrushColor(hdc, COLORREF(0x2020bc));
            }
            LRESULT(unsafe { red_brush() }.0 as isize)
        }
        WM_ERASEBKGND => {
            let mut r = RECT::default();
            let hdc = HDC(w.0 as *mut _);
            unsafe {
                let _ = GetClientRect(hwnd, &mut r);
                SetDCBrushColor(hdc, COLORREF(0x2020bc));
                FillRect(hdc, &r, red_brush());
            }
            LRESULT(1)
        }
        WM_COMMAND => {
            match w.0 & 0xffff {
                OPEN => show(),
                DISMISS => {
                    snooze_alert(hwnd, Local::now().timestamp());
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            dismiss_alert(hwnd);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

unsafe fn red_brush() -> HBRUSH {
    // System-owned DC brush has no per-window allocation to leak.
    let brush = unsafe { GetStockObject(DC_BRUSH) };
    HBRUSH(brush.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Interactive, isolated preview for reviewing the real notification UI.
    #[test]
    #[ignore = "opens isolated preview windows and captures the actual alert pixels"]
    fn red_alert_preview_for_capture() {
        struct Cleanup(HWND, HWND);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                shutdown();
                unsafe {
                    let _ = DestroyWindow(self.0);
                    let _ = SetForegroundWindow(self.1);
                }
            }
        }
        let previous = unsafe { GetForegroundWindow() };
        let resident = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("yyclip isolated preview"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        let _cleanup = Cleanup(resident, previous);
        let dir = tempfile::tempdir().unwrap();
        start_store(
            resident,
            Store::open(dir.path().join("preview.json")).unwrap(),
        )
        .unwrap();
        let today = Local::now().date_naive();
        let mut tasks = Vec::new();
        for (title, hour) in [
            ("見積書を提出する", 17),
            ("週次レポートを確認する", 18),
            ("バックアップ結果を確認する", 19),
        ] {
            let mut task = Task::new(title.into(), today.and_hms_opt(hour, 0, 0).unwrap());
            task.style = Style::Alert;
            let id = with_mut(|s| s.store.save(task)).unwrap().unwrap();
            tasks.push(
                with(|s| s.store.tasks().iter().find(|t| t.id == id).unwrap().clone()).unwrap(),
            );
        }
        refresh();
        let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("target");
        let manager = with(|s| s.hwnd).unwrap();
        wait(|| unsafe {
            let dc = GetDC(Some(manager));
            let color = GetPixel(dc, 4, 4);
            ReleaseDC(Some(manager), dc);
            color == BACKGROUND
        });
        capture_alert(manager, &output.join("yyclip-deadline-manager.bmp")).unwrap();
        command(NEW);
        set(TITLE, "見積書を提出する");
        set(MEMO, "金額と提出先を確認してから送付する。");
        wait(|| unsafe {
            let dc = GetDC(Some(manager));
            let color = GetPixel(dc, 4, 4);
            ReleaseDC(Some(manager), dc);
            color == BACKGROUND
        });
        capture_alert(manager, &output.join("yyclip-deadline-editor.bmp")).unwrap();
        set_mode(false);
        notify(&tasks).unwrap();
        let alert = with(|s| s.alerts[0].0).unwrap();
        wait(|| unsafe {
            let dc = GetDC(Some(alert));
            let color = GetPixel(dc, 5, 5);
            ReleaseDC(Some(alert), dc);
            color == COLORREF(0x2020bc)
        });
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("target/yyclip-deadline-alert.bmp");
        capture_alert(alert, &path).unwrap();
        println!("SCREENSHOT: {}", path.display());
    }

    fn capture_alert(hwnd: HWND, path: &std::path::Path) -> Result<()> {
        struct Capture {
            window: HWND,
            source: HDC,
            memory: HDC,
            bitmap: HBITMAP,
            old: HGDIOBJ,
        }
        impl Drop for Capture {
            fn drop(&mut self) {
                unsafe {
                    SelectObject(self.memory, self.old);
                    let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
                    let _ = DeleteDC(self.memory);
                    ReleaseDC(Some(self.window), self.source);
                }
            }
        }
        unsafe {
            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect)?;
            let (width, height) = (rect.right, rect.bottom);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: 40,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let source = GetDC(Some(hwnd));
            let memory = CreateCompatibleDC(Some(source));
            let mut bits = std::ptr::null_mut();
            let bitmap =
                match CreateDIBSection(Some(source), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                    Ok(bitmap) => bitmap,
                    Err(error) => {
                        let _ = DeleteDC(memory);
                        ReleaseDC(Some(hwnd), source);
                        return Err(error);
                    }
                };
            let old = SelectObject(memory, HGDIOBJ(bitmap.0));
            let _capture = Capture {
                window: hwnd,
                source,
                memory,
                bitmap,
                old,
            };
            BitBlt(memory, 0, 0, width, height, Some(source), 0, 0, SRCCOPY)?;
            let size = width as usize * height as usize * 4;
            let mut bmp = Vec::with_capacity(54 + size);
            bmp.extend_from_slice(b"BM");
            bmp.extend_from_slice(&((54 + size) as u32).to_le_bytes());
            bmp.extend_from_slice(&[0; 4]);
            bmp.extend_from_slice(&54u32.to_le_bytes());
            bmp.extend_from_slice(&40u32.to_le_bytes());
            bmp.extend_from_slice(&width.to_le_bytes());
            bmp.extend_from_slice(&(-height).to_le_bytes());
            bmp.extend_from_slice(&1u16.to_le_bytes());
            bmp.extend_from_slice(&32u16.to_le_bytes());
            bmp.extend_from_slice(&[0; 24]);
            bmp.extend_from_slice(std::slice::from_raw_parts(bits as *const u8, size));
            std::fs::write(path, bmp).map_err(|e| Error::new(E_FAIL, e.to_string()))?;
        }
        Ok(())
    }

    fn wait(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            unsafe {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            if condition() {
                return;
            }
            std::thread::yield_now();
        }
        panic!(
            "deadline UI timed out; rows={:?}, alerts={:?}",
            with(|s| s.rows.clone()),
            with(|s| s.alerts.len())
        );
    }

    fn select_task(id: u64) {
        let row = with(|s| s.rows.iter().position(|candidate| *candidate == id))
            .flatten()
            .expect("named task is present");
        let item = LVITEMW {
            state: LVIS_SELECTED,
            stateMask: LVIS_SELECTED,
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                control(LIST),
                LVM_SETITEMSTATE,
                Some(WPARAM(row)),
                Some(LPARAM(&item as *const _ as isize)),
            );
        }
        wait(|| with(|s| s.editing) == Some(Some(id)));
    }

    fn assert_controls_fit(ids: &[usize]) {
        let hwnd = with(|s| s.hwnd).unwrap();
        let mut client = RECT::default();
        let mut origin = POINT::default();
        unsafe {
            GetClientRect(hwnd, &mut client).unwrap();
            assert!(ClientToScreen(hwnd, &mut origin).as_bool());
        }
        let mut handles: Vec<_> = ids.iter().map(|id| (*id, control(*id))).collect();
        if with(|s| s.editor_open).unwrap_or(false) {
            handles.extend(
                with(|s| {
                    s.labels
                        .iter()
                        .enumerate()
                        .map(|(i, h)| (300 + i, *h))
                        .collect::<Vec<_>>()
                })
                .unwrap(),
            );
        }
        for (id, h) in handles {
            let mut bounds = RECT::default();
            unsafe {
                assert!(IsWindowVisible(h).as_bool(), "control {id} must be visible");
                GetWindowRect(h, &mut bounds).unwrap();
            }
            assert!(
                bounds.left >= origin.x
                    && bounds.top >= origin.y
                    && bounds.right <= origin.x + client.right
                    && bounds.bottom <= origin.y + client.bottom,
                "control {id} exceeds client area: {bounds:?}, client={client:?}, origin={origin:?}"
            );
            if id >= 300
                || [
                    HEADING, SUBTITLE, SUMMARY, NEW, EDIT, COMPLETE, DELETE, BACK, TEST, SAVE,
                ]
                .contains(&id)
            {
                let dc = unsafe { GetDC(Some(h)) };
                let font = with(|s| {
                    if id == HEADING {
                        s.heading_font
                    } else {
                        s.font
                    }
                })
                .unwrap();
                let previous = unsafe { SelectObject(dc, HGDIOBJ(font.0)) };
                let mut size = SIZE::default();
                let caption: Vec<_> = text(h).encode_utf16().collect();
                unsafe {
                    assert!(GetTextExtentPoint32W(dc, &caption, &mut size).as_bool());
                    SelectObject(dc, previous);
                    ReleaseDC(Some(h), dc);
                }
                assert!(
                    size.cx + 8 <= bounds.right - bounds.left
                        && size.cy <= bounds.bottom - bounds.top,
                    "caption of {id} is clipped: {size:?}, {bounds:?}"
                );
            }
        }
    }

    #[test]
    fn deadline_manager_saves_filters_notifies_on_all_monitors_and_completes() {
        struct Cleanup {
            resident: HWND,
            previous: HWND,
        }
        impl Drop for Cleanup {
            fn drop(&mut self) {
                shutdown();
                unsafe {
                    let data = NOTIFYICONDATAW {
                        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                        hWnd: self.resident,
                        uID: 1,
                        ..Default::default()
                    };
                    let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                    let _ = DestroyWindow(self.resident);
                    if !self.previous.0.is_null() {
                        let _ = SetForegroundWindow(self.previous);
                    }
                }
            }
        }
        let previous = unsafe { GetForegroundWindow() };
        let resident = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("yyclip isolated test"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        let _cleanup = Cleanup { resident, previous };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deadlines.json");
        {
            let picker =
                CalendarPicker::new().expect("CSV picker initializes its own COM apartment");
            assert!(
                unsafe { picker.dialog.GetOptions() }
                    .unwrap()
                    .contains(FOS_FORCEFILESYSTEM)
            );
        }
        let today = Local::now().date_naive();
        let mut seeded = Store::open(path.clone()).unwrap();
        let mut overdue = Task::new(
            "起動直後の再通知".into(),
            today.pred_opt().unwrap().and_hms_opt(18, 0, 0).unwrap(),
        );
        overdue.style = Style::Alert;
        let overdue_id = seeded.save(overdue).unwrap();
        seeded
            .notified(&[overdue_id], Local::now().timestamp())
            .unwrap();
        start_store(resident, Store::open(path.clone()).unwrap()).unwrap();
        assert_controls_fit(&[
            HEADING, SUBTITLE, SUMMARY, FILTER, LIST, NEW, EDIT, COMPLETE, DELETE,
        ]);
        command(NEW);
        assert_controls_fit(&[
            HEADING,
            SUBTITLE,
            TITLE,
            DUE,
            HOUR,
            MINUTE,
            MEMO,
            INTERVAL,
            STYLE,
            REPEAT,
            ADJUSTMENT,
            IMPORT_CALENDAR,
            CALENDAR_INFO,
            BACK,
            TEST,
            SAVE,
        ]);
        assert_eq!(
            due_value().unwrap().time(),
            chrono::NaiveTime::from_hms_opt(18, 0, 0).unwrap()
        );
        unsafe {
            assert_eq!(SendMessageW(control(HOUR), CB_GETCOUNT, None, None).0, 24);
            assert_eq!(SendMessageW(control(MINUTE), CB_GETCOUNT, None, None).0, 60);
            let mut info = DATETIMEPICKERINFO {
                cbSize: std::mem::size_of::<DATETIMEPICKERINFO>() as u32,
                ..Default::default()
            };
            SendMessageW(
                control(DUE),
                DTM_GETDATETIMEPICKERINFO,
                None,
                Some(LPARAM(&mut info as *mut _ as isize)),
            );
            let x = (info.rcButton.left + info.rcButton.right) / 2;
            let y = (info.rcButton.top + info.rcButton.bottom) / 2;
            let position = LPARAM(((y as u32) << 16 | (x as u32 & 0xffff)) as isize);
            SendMessageW(
                control(DUE),
                WM_LBUTTONDOWN,
                Some(WPARAM(1)),
                Some(position),
            );
            SendMessageW(control(DUE), WM_LBUTTONUP, None, Some(position));
            wait(|| SendMessageW(control(DUE), DTM_GETMONTHCAL, None, None).0 != 0);
            SendMessageW(control(DUE), DTM_CLOSEMONTHCAL, None, None);
        }
        set(TITLE, "日付選択の試験");
        set_due(
            NaiveDate::from_ymd_opt(2028, 2, 29)
                .unwrap()
                .and_hms_opt(23, 59, 0)
                .unwrap(),
        );
        assert_eq!(
            form().unwrap().due,
            NaiveDate::from_ymd_opt(2028, 2, 29)
                .unwrap()
                .and_hms_opt(23, 59, 0)
                .unwrap()
        );
        let before = form_snapshot();
        select(MINUTE, 0);
        assert_ne!(before, form_snapshot());
        set_due(
            NaiveDate::from_ymd_opt(2026, 11, 3)
                .unwrap()
                .and_hms_opt(18, 37, 0)
                .unwrap(),
        );
        select(ADJUSTMENT, 1);
        assert_eq!(
            form().unwrap().due.date(),
            NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()
        );
        select(ADJUSTMENT, 2);
        assert_eq!(
            form().unwrap().due.date(),
            NaiveDate::from_ymd_opt(2026, 11, 4).unwrap()
        );
        clear_form();
        set_mode(false);
        assert!(
            with(|s| !s.alerts.is_empty()).unwrap(),
            "startup must notify even inside the previous interval"
        );
        with_mut(|s| s.store.complete(overdue_id, Local::now().naive_local()))
            .unwrap()
            .unwrap();
        close_alerts();
        refresh();
        assert_eq!(text(control(INTERVAL)), "15");
        assert_eq!(selection(STYLE), 2);
        assert!(form().is_err(), "a title is required");
        let tomorrow = today.checked_add_days(Days::new(1)).unwrap();
        set(TITLE, "対象の作業😀");
        unsafe {
            SendMessageW(control(HOUR), CB_SETCURSEL, Some(WPARAM(usize::MAX)), None);
        }
        assert!(form().is_err(), "a deadline is required");
        set_due(tomorrow.and_hms_opt(18, 37, 0).unwrap());
        select(ADJUSTMENT, 0);
        set(MEMO, "保存するメモ");
        set(INTERVAL, "4");
        assert!(form().is_err(), "the minimum interval is five minutes");
        set(INTERVAL, "");
        assert_eq!(form().unwrap().interval, 15);
        set(INTERVAL, "5");
        select(STYLE, 0);
        command(SAVE);
        let id = with(|s| {
            s.store
                .tasks()
                .iter()
                .find(|t| t.title == "対象の作業😀")
                .unwrap()
                .id
        })
        .unwrap();
        wait(|| {
            Store::open(path.clone())
                .unwrap()
                .tasks()
                .iter()
                .any(|t| t.id == id && t.memo == "保存するメモ")
        });
        let mut later = Task::new(
            "一覧外の作業".into(),
            tomorrow
                .checked_add_days(Days::new(5))
                .unwrap()
                .and_hms_opt(10, 0, 0)
                .unwrap(),
        );
        later.style = Style::Alert;
        with_mut(|s| s.store.save(later)).unwrap().unwrap();
        refresh();
        assert_eq!(with(|s| s.rows.clone()).unwrap(), vec![id]);
        select(FILTER, 1);
        refresh();
        assert_eq!(with(|s| s.rows.len()), Some(2));
        select(FILTER, 0);
        refresh();
        select_task(id);
        assert_eq!(text(control(MEMO)), "保存するメモ");
        assert_eq!(
            due_value().unwrap(),
            tomorrow.and_hms_opt(18, 37, 0).unwrap()
        );
        // Timer refresh must preserve an unsaved edit.
        set(TITLE, "編集中の名前");
        refresh();
        assert_eq!(text(control(TITLE)), "編集中の名前");
        let morning = tomorrow.and_hms_opt(9, 0, 0).unwrap();
        tick_at(morning, 1000);
        wait(|| with(|s| !s.alerts.is_empty()).unwrap_or(false));
        let alerts = with(|s| s.alerts.clone()).unwrap();
        assert_eq!(alerts.len(), unsafe { GetSystemMetrics(SM_CMONITORS) }
            as usize);
        for (alert, _) in &alerts {
            let mut rect = RECT::default();
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            unsafe {
                GetWindowRect(*alert, &mut rect).unwrap();
                assert!(
                    GetMonitorInfoW(
                        MonitorFromWindow(*alert, MONITOR_DEFAULTTONEAREST),
                        &mut info
                    )
                    .as_bool()
                );
            }
            assert_eq!((rect.right, rect.top), (info.rcWork.right, info.rcWork.top));
            wait(|| unsafe {
                let dc = GetDC(Some(*alert));
                let color = GetPixel(dc, 5, 5);
                ReleaseDC(Some(*alert), dc);
                color == COLORREF(0x2020bc)
            });
        }
        close_alerts();
        tick_at(morning, 1299);
        assert_eq!(with(|s| s.alerts.len()), Some(0));
        tick_at(morning, 1300);
        assert!(with(|s| !s.alerts.is_empty()).unwrap());
        let alert = with(|s| s.alerts[0].0).unwrap();
        let delay = unsafe { GetDlgItem(Some(alert), SNOOZE as i32) }.unwrap();
        unsafe {
            assert_eq!(SendMessageW(delay, CB_GETCOUNT, None, None).0, 91);
            SendMessageW(delay, CB_SHOWDROPDOWN, Some(WPARAM(1)), None);
            assert_ne!(SendMessageW(delay, CB_GETDROPPEDSTATE, None, None).0, 0);
            SendMessageW(delay, CB_SETCURSEL, Some(WPARAM(90)), None);
            SendMessageW(delay, CB_SHOWDROPDOWN, Some(WPARAM(0)), None);
        }
        snooze_alert(alert, 1300);
        assert_eq!(with(|s| s.alerts.len()), Some(0));
        let saved = Store::open(path.clone()).unwrap();
        let task = saved.tasks().iter().find(|t| t.id == id).unwrap();
        assert_eq!(task.snoozed_until, Some(6700));
        assert_eq!(task.interval, 5);
        tick_at(morning, 6699);
        assert_eq!(with(|s| s.alerts.len()), Some(0));
        tick_at(morning, 6700);
        assert!(with(|s| !s.alerts.is_empty()).unwrap());
        assert_eq!(
            with(|s| s
                .store
                .tasks()
                .iter()
                .find(|t| t.id == id)
                .unwrap()
                .snoozed_until),
            Some(None)
        );
        close_alerts();
        tick_at(morning, 6999);
        assert_eq!(with(|s| s.alerts.len()), Some(0));
        tick_at(morning, 7000);
        assert!(with(|s| !s.alerts.is_empty()).unwrap());
        close_alerts();
        // Native Windows notification path uses only a test-owned tray icon.
        unsafe {
            let data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: resident,
                uID: 1,
                uFlags: NIF_ICON,
                hIcon: LoadIconW(None, IDI_APPLICATION).unwrap(),
                ..Default::default()
            };
            assert!(Shell_NotifyIconW(NIM_ADD, &data).as_bool());
        }
        let mut native = Task::new("yyclip 通知試験".into(), morning);
        native.style = Style::Windows;
        notify(&[native]).unwrap();
        select_task(id);
        command(COMPLETE);
        assert!(
            Store::open(path.clone())
                .unwrap()
                .tasks()
                .iter()
                .find(|t| t.id == id)
                .unwrap()
                .completed
        );
        tick_at(morning, 99999);
        assert_eq!(with(|s| s.alerts.len()), Some(0));
        select(FILTER, 2);
        refresh();
        select_task(id);
        command(COMPLETE);
        assert!(
            !Store::open(path)
                .unwrap()
                .tasks()
                .iter()
                .find(|t| t.id == id)
                .unwrap()
                .completed
        );
        let hwnd = with(|s| s.hwnd).unwrap();
        unsafe {
            SetWindowPos(hwnd, None, 0, 0, 640, 480, SWP_NOZORDER | SWP_NOACTIVATE).unwrap();
        }
        set_mode(false);
        assert_controls_fit(&[HEADING, FILTER, LIST, NEW, EDIT, COMPLETE, DELETE]);
        command(NEW);
        with_mut(|s| s.scroll = i32::MAX);
        layout();
        let mut button = RECT::default();
        let mut client = RECT::default();
        unsafe {
            GetWindowRect(control(SAVE), &mut button).unwrap();
            GetClientRect(hwnd, &mut client).unwrap();
            let mut origin = POINT::default();
            assert!(ClientToScreen(hwnd, &mut origin).as_bool());
            assert!(button.bottom <= origin.y + client.bottom);
        }
        set_mode(false);
        initial_bounds(hwnd);
        let ids = with(|s| s.store.tasks().iter().map(|t| t.id).collect::<Vec<_>>()).unwrap();
        for id in ids {
            with_mut(|s| s.store.remove(id)).unwrap().unwrap();
        }
        refresh();
        assert_controls_fit(&[
            HEADING, SUBTITLE, SUMMARY, FILTER, LIST, EMPTY, NEW, EDIT, COMPLETE, DELETE,
        ]);
        assert!(!unsafe { IsWindowVisible(control(TITLE)) }.as_bool());
    }
}

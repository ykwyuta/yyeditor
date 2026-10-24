//! Debounced native folder filters. Cached source indices keep selection independent of paging.
use super::*;
use windows::Win32::UI::Controls::EM_SETLIMITTEXT;
use yy_merge_core::folders::filter::{Filter, SELECTIONS};
const QUERY: u16 = 250;
const STATUS: u16 = 251;
const CLEAR: u16 = 252;
const MODE: u16 = 254;
pub(crate) struct Controls {
    query: HWND,
    status: HWND,
    clear: HWND,
    label: HWND,
}
fn value(hwnd: HWND) -> String {
    unsafe {
        let mut text = vec![0u16; GetWindowTextLengthW(hwnd).max(0) as usize + 1];
        let count = GetWindowTextW(hwnd, &mut text);
        String::from_utf16_lossy(&text[..count.max(0) as usize])
    }
}
impl Controls {
    pub(crate) fn create(parent: HWND, font: HFONT) -> Result<Self> {
        let controls = Self {
            label: child(
                parent,
                w!("COMBOBOX"),
                "",
                MODE,
                WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
            )?,
            query: child(
                parent,
                w!("EDIT"),
                "",
                QUERY,
                WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            )?,
            status: child(
                parent,
                w!("COMBOBOX"),
                "",
                STATUS,
                WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
            )?,
            clear: child(parent, w!("BUTTON"), "解除", CLEAR, WS_TABSTOP)?,
        };
        controls.set_font(font);
        unsafe {
            SendMessageW(controls.query, EM_SETLIMITTEXT, Some(WPARAM(512)), None);
            for label in ["部分一致", "ワイルドカード"] {
                SendMessageW(
                    controls.label,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(HSTRING::from(label).as_ptr() as isize)),
                );
            }
            SendMessageW(controls.label, CB_SETCURSEL, Some(WPARAM(0)), None);
            for selection in SELECTIONS {
                SendMessageW(
                    controls.status,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(HSTRING::from(selection.label()).as_ptr() as isize)),
                );
            }
            SendMessageW(controls.status, CB_SETCURSEL, Some(WPARAM(0)), None);
            for hwnd in controls.handles() {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
        Ok(controls)
    }
    fn handles(&self) -> [HWND; 4] {
        [self.label, self.query, self.status, self.clear]
    }
    pub(crate) fn set_font(&self, font: HFONT) {
        unsafe {
            for hwnd in self.handles() {
                SendMessageW(
                    hwnd,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
        }
    }
}
pub(crate) fn layout_controls(a: &App, top: i32, width: i32, dpi: i32) -> i32 {
    let visible = a.folder_mode && a.folders.as_ref().is_some_and(|f| !f.view_plan);
    unsafe {
        for hwnd in a.folder_filter.handles() {
            let _ = ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
        }
        if !visible {
            return 0;
        }
        let unit = |n| n * dpi / 96;
        let query_width = (width - unit(120 + 220 + 65 + 22)).max(1);
        let _ = MoveWindow(
            a.folder_filter.label,
            unit(6),
            top,
            unit(116),
            unit(150),
            true,
        );
        let _ = MoveWindow(
            a.folder_filter.query,
            unit(124),
            top,
            query_width,
            unit(27),
            true,
        );
        let _ = MoveWindow(
            a.folder_filter.status,
            unit(130) + query_width,
            top,
            unit(220),
            unit(260),
            true,
        );
        let _ = MoveWindow(
            a.folder_filter.clear,
            width - unit(65),
            top,
            unit(59),
            unit(27),
            true,
        );
        unit(34)
    }
}
pub(crate) fn sync_controls(a: &mut App) {
    let Some(f) = &mut a.folders else {
        return;
    };
    if !f.controls_dirty {
        return;
    }
    f.controls_dirty = false;
    text(a.folder_filter.query, f.filter.query());
    unsafe {
        SendMessageW(
            a.folder_filter.label,
            CB_SETCURSEL,
            Some(WPARAM(usize::from(f.filter.glob().is_some()))),
            None,
        );
        SendMessageW(
            a.folder_filter.status,
            CB_SETCURSEL,
            Some(WPARAM(
                SELECTIONS
                    .iter()
                    .position(|s| *s == f.filter.status())
                    .unwrap_or(0),
            )),
            None,
        );
    }
}
pub(crate) fn notification(id: u16, code: u32) -> bool {
    if !matches!(id, QUERY | STATUS | CLEAR | MODE) {
        return false;
    }
    app(|a| {
        if !a.folder_mode || a.folders.as_ref().is_none_or(|f| f.view_plan) {
            return;
        }
        if id == QUERY && code == EN_CHANGE {
            a.folders.as_mut().unwrap().filter_due =
                Some(Instant::now() + Duration::from_millis(250));
        } else if matches!(id, STATUS | MODE) && code == CBN_SELCHANGE {
            apply(a);
        } else if id == CLEAR && code == BN_CLICKED {
            let f = a.folders.as_mut().unwrap();
            f.filter = Default::default();
            f.filter_due = None;
            f.controls_dirty = true;
            rebuild_filter(f);
            refresh(a);
        }
    });
    true
}
pub(crate) fn apply(a: &mut App) {
    if a.folders.is_none() {
        return;
    }
    let query = value(a.folder_filter.query);
    let selection = unsafe { SendMessageW(a.folder_filter.status, CB_GETCURSEL, None, None).0 };
    let selection = SELECTIONS
        .get(selection as usize)
        .copied()
        .unwrap_or_default();
    let f = a.folders.as_mut().unwrap();
    f.filter_due = None;
    let wildcard = unsafe { SendMessageW(a.folder_filter.label, CB_GETCURSEL, None, None).0 == 1 };
    match if wildcard && !query.is_empty() {
        Filter::wildcard(query, selection)
    } else {
        Filter::new(query, selection)
    } {
        Ok(filter) => {
            f.filter = filter;
            rebuild_filter(f);
        }
        Err(e) => {
            text(a.status, &e);
            return;
        }
    }
    if a.folder_mode && !f.view_plan {
        refresh(a);
    }
}
pub(crate) fn enter(a: &mut App, hwnd: HWND) -> bool {
    if !a.folder_mode
        || hwnd != a.folder_filter.query
        || a.folders.as_ref().is_none_or(|f| f.view_plan)
    {
        return false;
    }
    apply(a);
    true
}
pub(super) fn tick(a: &mut App) {
    if a.folders
        .as_ref()
        .and_then(|f| f.filter_due)
        .is_some_and(|due| Instant::now() >= due)
    {
        apply(a);
    }
}
pub(super) fn smoke(a: &mut App, dir: &std::path::Path) -> Result<()> {
    let initial_count = a.folders.as_ref().unwrap().entries.len();
    text(a.folder_filter.query, "DIFFERENT.TXT");
    apply(a);
    unsafe {
        if SendMessageW(a.list, LB_GETCOUNT, None, None).0 != 1 {
            return Err("path filter did not narrow the native list".into());
        }
        SendMessageW(a.list, LB_SETCURSEL, Some(WPARAM(0)), None);
    }
    if selected_pair(a)?
        .left
        .as_ref()
        .unwrap()
        .file_name()
        .unwrap()
        != "different.txt"
    {
        return Err("filtered selection lost its source path".into());
    }
    unsafe {
        SendMessageW(a.folder_filter.label, CB_SETCURSEL, Some(WPARAM(1)), None);
    }
    text(a.folder_filter.query, "*different.?xt");
    apply(a);
    if a.folders.as_ref().unwrap().visible.len() != 1
        || a.folders.as_ref().unwrap().filter.glob() != Some("*different.?xt")
        || selected_pair(a).is_ok()
    {
        return Err("wildcard mode failed to filter/reset selection".into());
    }
    text(a.folder_filter.query, &"?".repeat(257));
    apply(a);
    if a.folders.as_ref().unwrap().visible.len() != 1
        || a.folders.as_ref().unwrap().filter.glob() != Some("*different.?xt")
    {
        return Err("invalid wildcard replaced active results".into());
    }
    unsafe {
        SendMessageW(a.folder_filter.label, CB_SETCURSEL, Some(WPARAM(0)), None);
    }
    text(a.folder_filter.query, "same.txt");
    unsafe {
        SendMessageW(a.folder_filter.status, CB_SETCURSEL, Some(WPARAM(2)), None);
    }
    if !enter(a, a.folder_filter.query)
        || exit_code(a) != Some(1)
        || a.folders.as_ref().unwrap().visible.len() != 1
    {
        return Err("filter changed the overall comparison exit status".into());
    }
    text(a.folder_filter.query, "no-such-path");
    a.folders.as_mut().unwrap().filter_due = Some(Instant::now() + Duration::from_secs(1));
    tick(a);
    if a.folders.as_ref().unwrap().visible.len() != 1 {
        return Err("filter debounce applied before deadline".into());
    }
    a.folders.as_mut().unwrap().filter_due = Some(Instant::now());
    tick(a);
    if !a.folders.as_ref().unwrap().visible.is_empty() || exit_code(a) != Some(1) {
        return Err("zero matches hid an overall difference".into());
    }
    a.folders.as_mut().unwrap().entries.push(
        Entry {
            path: "hidden-error.txt".into(),
            status: Status::Error,
            file_pair: false,
            directory: false,
            detail: Some("test read error".into()),
        }
        .into(),
    );
    text(a.folder_filter.query, "same.txt");
    apply(a);
    if exit_code(a) != Some(2) || !value(a.status).contains("未比較1") {
        return Err("hidden error was lost from folder status".into());
    }
    let f = a.folders.as_mut().unwrap();
    f.entries.pop();
    if f.entries.len() != initial_count {
        return Err("filter changed original rows".into());
    }
    f.filter = Default::default();
    f.controls_dirty = true;
    rebuild_filter(f);
    refresh(a);
    std::fs::write(dir.join("folder-filter-smoke.txt"),"PASS: native literal/wildcard path/status filters, invalid wildcard preserves active results, debounce and Enter, zero matches preserve overall differences, hidden errors remain incomplete, source selection retained, reset preserves rows\n").map_err(|e|e.to_string())
}
pub(super) fn smoke_three(a: &mut App) -> Result<()> {
    text(a.folder_filter.query, "");
    unsafe {
        SendMessageW(a.folder_filter.status, CB_SETCURSEL, Some(WPARAM(5)), None);
    }
    apply(a);
    let f = a.folders.as_ref().unwrap();
    let index = f
        .visible
        .iter()
        .position(|i| f.entries[*i].three == Some(ThreeStatus::BothChanged))
        .ok_or("review filter lost both-changed row")?;
    unsafe {
        SendMessageW(a.list, LB_SETCURSEL, Some(WPARAM(index)), None);
    }
    if selected_pair(a)?.base.is_none() {
        return Err("three-way filtered selection lost BASE".into());
    }
    // The following real plan smoke must still process all inputs with no visible rows.
    text(a.folder_filter.query, "no-visible-rows");
    apply(a);
    if !a.folders.as_ref().unwrap().visible.is_empty() {
        return Err("three-way empty filter failed".into());
    }
    Ok(())
}

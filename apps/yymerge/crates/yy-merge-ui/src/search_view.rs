//! Non-mutating pane search backed by the YY series' shared search engine.
use super::*;
use std::ops::Range;
use windows::Win32::UI::Controls::{BST_CHECKED, EM_SETLIMITTEXT};
use yy_core::{Query, Searcher};

const QUERY: u16 = 260;
const PANE_CHOICE: u16 = 261;
const REGEX: u16 = 262;
const CASE: u16 = 263;
const WORD: u16 = 264;
const NAMES: [&str; 4] = ["LEFT", "RIGHT", "BASE", "RESULT"];
pub(super) struct Controls {
    label: HWND,
    query: HWND,
    pane: HWND,
    regex: HWND,
    case: HWND,
    word: HWND,
    prev: HWND,
    next: HWND,
    close: HWND,
    pub(super) info: HWND,
}
impl Controls {
    pub(super) fn create(parent: HWND, font: HFONT) -> Result<Self> {
        let check = |title, id| {
            child(
                parent,
                w!("BUTTON"),
                title,
                id,
                WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
            )
        };
        let controls = Self {
            label: child(
                parent,
                w!("STATIC"),
                "検索",
                265,
                WINDOW_STYLE(SS_LEFTNOWORDWRAP),
            )?,
            query: child(
                parent,
                w!("EDIT"),
                "",
                QUERY,
                WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            )?,
            pane: child(
                parent,
                w!("COMBOBOX"),
                "",
                PANE_CHOICE,
                WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
            )?,
            regex: check("正規表現", REGEX)?,
            case: check("大小区別", CASE)?,
            word: check("単語単位", WORD)?,
            prev: child(parent, w!("BUTTON"), "前", FIND_PREV, WS_TABSTOP)?,
            next: child(parent, w!("BUTTON"), "次", FIND_NEXT, WS_TABSTOP)?,
            close: child(parent, w!("BUTTON"), "閉じる", FIND_CLOSE, WS_TABSTOP)?,
            info: child(
                parent,
                w!("STATIC"),
                "Enter: 次 / Shift+Enter: 前",
                266,
                WINDOW_STYLE(SS_LEFTNOWORDWRAP),
            )?,
        };
        controls.set_font(font);
        unsafe {
            SendMessageW(controls.query, EM_SETLIMITTEXT, Some(WPARAM(512)), None);
            for h in controls.handles() {
                let _ = ShowWindow(h, SW_HIDE);
            }
        }
        Ok(controls)
    }
    fn handles(&self) -> [HWND; 10] {
        [
            self.label, self.query, self.pane, self.regex, self.case, self.word, self.prev,
            self.next, self.close, self.info,
        ]
    }
    pub(super) fn set_font(&self, font: HFONT) {
        unsafe {
            for h in self.handles() {
                SendMessageW(
                    h,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
        }
    }
    pub(super) fn query(&self) -> Query {
        let mut value = vec![0u16; unsafe { GetWindowTextLengthW(self.query) }.max(0) as usize + 1];
        let n = unsafe { GetWindowTextW(self.query, &mut value) }.max(0) as usize;
        let checked =
            |h| unsafe { SendMessageW(h, BM_GETCHECK, None, None).0 } == BST_CHECKED.0 as isize;
        Query {
            pattern: String::from_utf16_lossy(&value[..n]),
            regex: checked(self.regex),
            case_sensitive: checked(self.case),
            whole_word: checked(self.word),
        }
    }
    pub(super) fn query_handle(&self) -> HWND {
        self.query
    }
    pub(super) fn option_handles(&self) -> [HWND; 3] {
        [self.regex, self.case, self.word]
    }
}
#[derive(Default)]
pub(super) struct State {
    pub(super) visible: bool,
    pub(super) active: usize,
    pane_count: usize,
    job: Option<Job>,
    last: Option<Last>,
}
struct Last {
    query: Query,
    pane: usize,
    generation: u64,
    version: u64,
    hit: Range<u64>,
}
type SearchResult = Result<Option<(Range<u64>, bool)>>;
struct Job {
    receiver: mpsc::Receiver<SearchResult>,
    cancel: Arc<Cancellation>,
    query: Query,
    pane: usize,
    generation: u64,
    version: u64,
    selection: yy_core::Selection,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
pub(super) fn cancel(a: &mut App) {
    replace_view::cancel(a);
    if a.search.job.take().is_some() {
        text(a.search_controls.info, "検索を中止しました");
    }
}
pub(super) fn reset(a: &mut App) {
    cancel(a);
    a.replace.visible = false;
    a.search.last = None;
    a.search.active = 0;
    text(a.search_controls.info, "Enter: 次 / Shift+Enter: 前");
}
pub(super) fn focus(a: &mut App, index: usize) {
    if index != a.search.active {
        cancel(a);
        a.search.active = index;
        a.search.last = None;
        text(a.search_controls.info, "Enter: 次 / Shift+Enter: 前");
        unsafe {
            SendMessageW(
                a.search_controls.pane,
                CB_SETCURSEL,
                Some(WPARAM(index)),
                None,
            );
        }
        replace_view::update_menu(a);
    }
}
pub(super) fn layout_controls(a: &mut App, top: i32, width: i32, dpi: i32) -> i32 {
    if a.folder_mode {
        cancel(a);
    }
    let visible = a.search.visible && !a.folder_mode;
    let count = a.editors.len();
    unsafe {
        if a.search.pane_count != count {
            a.search.pane_count = count;
            a.search.active = a.search.active.min(count.saturating_sub(1));
            SendMessageW(a.search_controls.pane, CB_RESETCONTENT, None, None);
            for name in NAMES.iter().take(count) {
                SendMessageW(
                    a.search_controls.pane,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(HSTRING::from(*name).as_ptr() as isize)),
                );
            }
        }
        SendMessageW(
            a.search_controls.pane,
            CB_SETCURSEL,
            Some(WPARAM(a.search.active)),
            None,
        );
        for h in a.search_controls.handles() {
            let _ = ShowWindow(h, if visible { SW_SHOW } else { SW_HIDE });
        }
        let replacement_height = replace_view::layout_controls(a, top + 64 * dpi / 96, width, dpi);
        if !visible {
            return 0;
        }
        let unit = |n| n * dpi / 96;
        let query_width = (width - unit(304)).max(1);
        for (h, x, y, w, hgt) in [
            (
                a.search_controls.label,
                unit(6),
                top + unit(4),
                unit(36),
                unit(25),
            ),
            (
                a.search_controls.query,
                unit(46),
                top,
                query_width,
                unit(27),
            ),
            (
                a.search_controls.pane,
                unit(52) + query_width,
                top,
                unit(100),
                unit(200),
            ),
            (
                a.search_controls.prev,
                width - unit(146),
                top,
                unit(42),
                unit(27),
            ),
            (
                a.search_controls.next,
                width - unit(98),
                top,
                unit(42),
                unit(27),
            ),
            (
                a.search_controls.close,
                width - unit(50),
                top,
                unit(44),
                unit(27),
            ),
            (
                a.search_controls.regex,
                unit(6),
                top + unit(31),
                unit(100),
                unit(25),
            ),
            (
                a.search_controls.case,
                unit(106),
                top + unit(31),
                unit(100),
                unit(25),
            ),
            (
                a.search_controls.word,
                unit(206),
                top + unit(31),
                unit(100),
                unit(25),
            ),
            (
                a.search_controls.info,
                unit(316),
                top + unit(34),
                (width - unit(324)).max(1),
                unit(24),
            ),
        ] {
            let _ = MoveWindow(h, x, y, w, hgt, true);
        }
        unit(64) + replacement_height
    }
}
pub(super) fn update_menu(a: &App) {
    replace_view::update_menu(a);
    unsafe {
        for id in FIND_OPEN..=FIND_CLOSE {
            let enabled = if id == FIND_CLOSE {
                a.search.visible && !a.folder_mode
            } else {
                !a.folder_mode && a.receiver.is_none()
            };
            let _ = EnableMenuItem(
                GetMenu(a.hwnd),
                id as u32,
                MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
            );
        }
    }
}
pub(super) fn command(a: &mut App, id: u16) -> Result<()> {
    if id == FIND_CLOSE {
        cancel(a);
        a.search.visible = false;
        a.replace.visible = false;
        layout(a);
        if let Some(pane) = a.panes.get(a.search.active) {
            unsafe {
                let _ = SetFocus(Some(*pane));
            }
        }
        return Ok(());
    }
    if a.folder_mode || a.receiver.is_some() {
        return Err("ファイル比較が完了してから検索してください。".into());
    }
    a.search.visible = true;
    layout(a);
    if id == FIND_OPEN || a.search_controls.query().pattern.is_empty() {
        unsafe {
            let _ = SetFocus(Some(a.search_controls.query));
            SendMessageW(
                a.search_controls.query,
                windows::Win32::UI::Controls::EM_SETSEL,
                Some(WPARAM(0)),
                Some(LPARAM(-1)),
            );
        }
        return Ok(());
    }
    start(a, id != FIND_PREV)
}
pub(super) fn notification(id: u16, code: u32) -> bool {
    if !matches!(id, QUERY | PANE_CHOICE | REGEX | CASE | WORD) {
        return false;
    }
    app(|a| {
        if id == PANE_CHOICE && code == CBN_SELCHANGE {
            let index = unsafe { SendMessageW(a.search_controls.pane, CB_GETCURSEL, None, None).0 };
            if index >= 0 && (index as usize) < a.editors.len() {
                focus(a, index as usize);
            }
        } else if id == QUERY && code == EN_CHANGE
            || matches!(id, REGEX | CASE | WORD) && code == BN_CLICKED
        {
            cancel(a);
            text(a.search_controls.info, "Enter: 次 / Shift+Enter: 前");
        }
    });
    true
}
pub(super) fn key(a: &mut App, hwnd: HWND, key: usize, shift: bool) -> Option<Result<()>> {
    if !a.search.visible || a.folder_mode || !a.search_controls.handles().contains(&hwnd) {
        return None;
    }
    match key {
        9 => Some(tab(a, hwnd, shift)),
        13 => Some(start(a, !shift)),
        27 => Some(command(a, FIND_CLOSE)),
        _ => None,
    }
}
pub(super) fn tab(a: &mut App, hwnd: HWND, shift: bool) -> Result<()> {
    let mut targets = vec![
        a.search_controls.query,
        a.search_controls.pane,
        a.search_controls.prev,
        a.search_controls.next,
        a.search_controls.close,
        a.search_controls.regex,
        a.search_controls.case,
        a.search_controls.word,
    ];
    replace_view::tab_targets(a, &mut targets);
    let index = targets.iter().position(|h| *h == hwnd).unwrap_or(0);
    let next = (index + if shift { targets.len() - 1 } else { 1 }) % targets.len();
    unsafe {
        let _ = SetFocus(Some(targets[next]));
    }
    Ok(())
}
fn start(a: &mut App, forward: bool) -> Result<()> {
    if a.folder_mode || a.receiver.is_some() {
        return Err("比較中またはフォルダー表示では本文を検索できません。".into());
    }
    let query = a.search_controls.query();
    if query.pattern.is_empty() {
        text(a.search_controls.info, "検索文字列を入力してください");
        return Ok(());
    }
    let pane = a.search.active.min(a.editors.len() - 1);
    let editor = &a.editors[pane];
    let selection = *editor.document.selections().primary();
    let version = editor.document.version();
    let generation = a.generation;
    let repeated = a.search.last.as_ref().is_some_and(|last| {
        last.query == query
            && last.pane == pane
            && last.generation == generation
            && last.version == version
            && last.hit == selection.range()
    });
    let snap = editor.document.snapshot().clone();
    let mut from = selection.start();
    let mut force_wrap = false;
    if forward && repeated {
        from = selection.end();
        if selection.is_empty() {
            if from == snap.len() {
                force_wrap = true;
            } else {
                let first = snap
                    .chunks(from..(from + 1).min(snap.len()))
                    .next()
                    .and_then(|b| b.first())
                    .copied()
                    .unwrap_or(0);
                from += match first {
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf4 => 4,
                    _ => 1,
                };
            }
        }
    }
    cancel(a);
    let cancel = Arc::new(Cancellation::default());
    let worker_cancel = cancel.clone();
    let worker_query = query.clone();
    let (tx, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            worker_cancel.check()?;
            let searcher = Searcher::new(&worker_query).map_err(|e| e.to_string())?;
            worker_cancel.check()?;
            let mut step = |_| worker_cancel.check().is_ok();
            let found = if force_wrap {
                searcher
                    .find_next(&snap, 0..snap.len(), 0, &mut step)
                    .map(|m| m.map(|m| (m, true)))
            } else if !forward {
                // Shared find_prev intentionally excludes its upper bound. Include an
                // EOF zero-width match when wrapping from the beginning of the file.
                (|| {
                    if let Some(hit) = searcher.find_prev(&snap, 0..snap.len(), from, &mut step)? {
                        return Ok(Some((hit, false)));
                    }
                    let last = match searcher.find_next(
                        &snap,
                        snap.len()..snap.len(),
                        snap.len(),
                        &mut step,
                    )? {
                        Some(hit) => Some(hit),
                        None => searcher.find_prev(&snap, 0..snap.len(), snap.len(), &mut step)?,
                    };
                    Ok(last.filter(|hit| hit.start >= from).map(|hit| (hit, true)))
                })()
            } else {
                searcher.find_wrapping(&snap, from, forward, &mut step)
            };
            worker_cancel.check()?;
            found.map_err(|_| "cancelled".into())
        })();
        let _ = tx.send(result);
    });
    a.search.job = Some(Job {
        receiver,
        cancel,
        query,
        pane,
        generation,
        version,
        selection,
    });
    text(a.search_controls.info, "検索中… [停止] で中止できます");
    Ok(())
}
pub(super) fn tick(a: &mut App) {
    let Some(job) = &a.search.job else {
        return;
    };
    let event = match job.receiver.try_recv() {
        Ok(result) => result,
        Err(mpsc::TryRecvError::Empty) => return,
        Err(mpsc::TryRecvError::Disconnected) => Err("検索ワーカーが終了しました".into()),
    };
    let job = a.search.job.take().unwrap();
    if a.folder_mode
        || a.receiver.is_some()
        || a.generation != job.generation
        || a.search.active != job.pane
        || a.search_controls.query() != job.query
        || a.editors.get(job.pane).is_none_or(|e| {
            e.document.version() != job.version
                || *e.document.selections().primary() != job.selection
        })
    {
        text(
            a.search_controls.info,
            "文書・選択が変わりました。再検索してください",
        );
        return;
    }
    match event {
        Err(e) => text(a.search_controls.info, e.lines().next().unwrap_or(&e)),
        Ok(None) => {
            a.search.last = None;
            text(a.search_controls.info, "一致する文字列はありません");
        }
        Ok(Some((hit, wrapped))) => {
            let editor = &mut a.editors[job.pane];
            editor
                .document
                .set_selections(yy_core::SelectionSet::single(yy_core::Selection::new(
                    hit.start, hit.end,
                )));
            editor.viewport.top =
                yy_core::motion::line_start(editor.document.snapshot(), hit.start);
            if let Some(view) = &mut editor.aligned {
                view.top = view.row_for_offset(hit.start);
            }
            editor.follow_caret = true;
            editor.reveal_caret();
            let line = editor
                .document
                .snapshot()
                .chunks(0..hit.start)
                .map(|b| b.iter().filter(|c| **c == b'\n').count())
                .sum::<usize>()
                + 1;
            a.search.last = Some(Last {
                query: job.query.clone(),
                pane: job.pane,
                generation: job.generation,
                version: job.version,
                hit,
            });
            sync_sources(a, job.pane);
            invalidate(a.panes[job.pane]);
            text(
                a.search_controls.info,
                &format!(
                    "{} {line}行目{}",
                    NAMES[job.pane],
                    if wrapped {
                        "（端から再検索）"
                    } else {
                        ""
                    }
                ),
            );
            unsafe {
                let _ = SetFocus(Some(a.panes[job.pane]));
            }
        }
    }
}

pub(super) fn smoke(a: &mut App, dir: &std::path::Path) -> Result<()> {
    use windows::Win32::UI::Controls::BST_UNCHECKED;
    let wait = |a: &mut App| -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while a.search.job.is_some() {
            tick(a);
            if Instant::now() > deadline {
                return Err("search smoke timeout".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    };
    let options = |a: &mut App, pattern: &str, regex: bool, case: bool, word: bool| {
        cancel(a);
        a.search.last = None;
        text(a.search_controls.query, pattern);
        for (h, checked) in [
            (a.search_controls.regex, regex),
            (a.search_controls.case, case),
            (a.search_controls.word, word),
        ] {
            unsafe {
                SendMessageW(
                    h,
                    BM_SETCHECK,
                    Some(WPARAM(
                        if checked { BST_CHECKED } else { BST_UNCHECKED }.0 as usize,
                    )),
                    None,
                );
            }
        }
    };
    let select = |a: &mut App, pane: usize, range: Range<u64>| {
        focus(a, pane);
        a.editors[pane]
            .document
            .set_selections(yy_core::SelectionSet::single(yy_core::Selection::new(
                range.start,
                range.end,
            )));
    };
    let expect = |a: &App, pane: usize, range: Range<u64>| -> Result<()> {
        let found = a.editors[pane].document.selections().primary().range();
        if found != range {
            return Err(format!(
                "search pane {pane}: expected {range:?}, got {found:?}"
            ));
        }
        Ok(())
    };
    let flags = (
        a.manual,
        a.saved,
        a.edited,
        a.exit,
        a.history.len(),
        a.redo.len(),
    );
    let unresolved = a
        .loaded
        .as_ref()
        .unwrap()
        .merged
        .as_ref()
        .unwrap()
        .unresolved();
    command(a, FIND_OPEN)?;
    // Exercise the actual aligned diff/merge panes before using controlled fixtures.
    for pane in 0..a.editors.len() {
        let before = a.editors[pane].text();
        let first = before
            .chars()
            .next()
            .ok_or("empty search smoke pane")?
            .to_string();
        options(a, &first, false, true, false);
        select(a, pane, 0..0);
        start(a, true)?;
        wait(a)?;
        expect(a, pane, 0..first.len() as u64)?;
        if a.editors[pane].text() != before {
            return Err("search modified source".into());
        }
    }
    let dpi = unsafe { GetDpiForWindow(a.hwnd) };
    let fixture = "猫 cat scatter CAT a.b\r\ncat\n";
    let fixtures = (0..4)
        .map(|_| editor(fixture, dpi))
        .collect::<Result<Vec<_>>>()?;
    let original = std::mem::replace(&mut a.editors, fixtures);
    options(a, "cat", false, false, true);
    select(a, 0, 0..0);
    for expected in [4..7, 16..19, 25..28, 4..7] {
        start(a, true)?;
        wait(a)?;
        expect(a, 0, expected)?;
    }
    start(a, false)?;
    wait(a)?;
    expect(a, 0, 25..28)?;
    options(a, "CAT", false, true, true);
    select(a, 0, 0..0);
    start(a, true)?;
    wait(a)?;
    expect(a, 0, 16..19)?;
    options(a, "a.b", false, true, false);
    select(a, 1, 0..0);
    start(a, true)?;
    wait(a)?;
    expect(a, 1, 20..23)?;
    options(a, "^cat$", true, false, false);
    select(a, 2, 0..0);
    start(a, true)?;
    wait(a)?;
    expect(a, 2, 25..28)?;
    options(a, "a\\.b$", true, true, false);
    select(a, 2, 0..0);
    start(a, true)?;
    wait(a)?;
    expect(a, 2, 20..23)?;
    options(a, "猫", false, true, false);
    select(a, 3, 0..0);
    start(a, true)?;
    wait(a)?;
    expect(a, 3, 0..3)?;
    // A zero-width match advances by a complete UTF-8 code point and wraps at EOF.
    a.editors[0] = editor("猫x", dpi)?;
    options(a, "^|$", true, true, false);
    select(a, 0, 0..0);
    for expected in [0..0, 4..4, 0..0] {
        start(a, true)?;
        wait(a)?;
        expect(a, 0, expected)?;
    }
    start(a, false)?;
    wait(a)?;
    expect(a, 0, 4..4)?;
    a.editors[0] = editor("", dpi)?;
    options(a, "^$", true, true, false);
    select(a, 0, 0..0);
    for forward in [true, true, false] {
        start(a, forward)?;
        wait(a)?;
        expect(a, 0, 0..0)?;
        if a.search.last.is_none() {
            return Err("empty document anchor not found".into());
        }
    }
    options(a, "[", true, false, false);
    select(a, 1, 1..1);
    start(a, true)?;
    wait(a)?;
    expect(a, 1, 1..1)?;
    if a.search.last.is_some() {
        return Err("invalid regex accepted".into());
    }
    options(a, "missing", false, false, false);
    start(a, true)?;
    wait(a)?;
    expect(a, 1, 1..1)?;
    // All stale-result checks run before draining the worker, independent of its speed.
    options(a, "cat", false, false, false);
    select(a, 1, 0..0);
    start(a, true)?;
    select(a, 1, 2..2);
    wait(a)?;
    expect(a, 1, 2..2)?;
    select(a, 1, 0..0);
    start(a, true)?;
    if !a.editors[1].document.insert_text("edit", false) {
        return Err("search stale fixture edit failed".into());
    }
    let after_edit = a.editors[1].document.selections().primary().range();
    wait(a)?;
    expect(a, 1, after_edit)?;
    select(a, 1, 0..0);
    start(a, true)?;
    text(a.search_controls.query, "different");
    wait(a)?;
    expect(a, 1, 0..0)?;
    options(a, "cat", false, false, false);
    start(a, true)?;
    a.generation += 1;
    wait(a)?;
    expect(a, 1, 0..0)?;
    start(a, true)?;
    focus(a, 2);
    if a.search.job.is_some() {
        return Err("pane change did not cancel search".into());
    }
    select(a, 2, 0..0);
    start(a, true)?;
    cancel(a);
    tick(a);
    expect(a, 2, 0..0)?;
    // Keyboard navigation and close preserve editor state and release the worker.
    key(a, a.search_controls.query, 9, false).unwrap()?;
    if unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() } != a.search_controls.pane
    {
        return Err("search Tab navigation failed".into());
    }
    key(a, a.search_controls.pane, 9, true).unwrap()?;
    if unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() } != a.search_controls.query
    {
        return Err("search Shift+Tab navigation failed".into());
    }
    start(a, true)?;
    key(a, a.search_controls.query, 27, false).unwrap()?;
    if a.search.visible || a.search.job.is_some() {
        return Err("search close did not hide/cancel".into());
    }
    a.editors = original;
    reset(a);
    options(a, "", false, false, false);
    layout(a);
    if flags
        != (
            a.manual,
            a.saved,
            a.edited,
            a.exit,
            a.history.len(),
            a.redo.len(),
        )
        || unresolved
            != a.loaded
                .as_ref()
                .unwrap()
                .merged
                .as_ref()
                .unwrap()
                .unresolved()
    {
        return Err("search changed merge/save/history/exit state".into());
    }
    std::fs::write(dir.join("search-smoke.txt"),"PASS: aligned LEFT/RIGHT/BASE/RESULT, Unicode/literal/regex/case/whole-word, CRLF anchors, forward/backward wrap, zero-width UTF-8/EOF advance, invalid regex/no match, stale selection/document/query/generation, pane change/cancel/close, Tab/Shift+Tab, source and merge/save/history/exit unchanged\n").map_err(|e|e.to_string())
}

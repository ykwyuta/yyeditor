//! Guarded RESULT-only replacement, sharing YY search and editor undo.
use super::*;
use std::io::{self, Write};
use std::ops::Range;
use yy_core::{Query, Replacement, Searcher, Selection, SelectionSet};

const VALUE: u16 = 267;
const EXPAND: u16 = 269;
pub(super) struct Controls {
    label: HWND,
    value: HWND,
    one: HWND,
    all: HWND,
    expand: HWND,
}
impl Controls {
    pub(super) fn create(parent: HWND, font: HFONT) -> Result<Self> {
        let controls = Self {
            label: child(
                parent,
                w!("STATIC"),
                "置換後",
                268,
                WINDOW_STYLE(SS_LEFTNOWORDWRAP),
            )?,
            value: child(
                parent,
                w!("EDIT"),
                "",
                VALUE,
                WS_TABSTOP | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            )?,
            one: child(parent, w!("BUTTON"), "選択を置換", REPLACE_ONE, WS_TABSTOP)?,
            all: child(parent, w!("BUTTON"), "全置換", REPLACE_ALL, WS_TABSTOP)?,
            expand: child(
                parent,
                w!("BUTTON"),
                "置換構文",
                EXPAND,
                WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
            )?,
        };
        controls.set_font(font);
        unsafe {
            SendMessageW(
                controls.value,
                windows::Win32::UI::Controls::EM_SETLIMITTEXT,
                Some(WPARAM(512)),
                None,
            );
            for h in controls.handles() {
                let _ = ShowWindow(h, SW_HIDE);
            }
        }
        Ok(controls)
    }
    fn handles(&self) -> [HWND; 5] {
        [self.label, self.value, self.expand, self.one, self.all]
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
    fn expand(&self) -> bool {
        unsafe { SendMessageW(self.expand, BM_GETCHECK, None, None).0 != 0 }
    }
    fn value(&self) -> String {
        let mut value = vec![0u16; unsafe { GetWindowTextLengthW(self.value) }.max(0) as usize + 1];
        let n = unsafe { GetWindowTextW(self.value, &mut value) }.max(0) as usize;
        String::from_utf16_lossy(&value[..n])
    }
}
#[derive(Default)]
pub(super) struct State {
    pub(super) visible: bool,
    job: Option<Job>,
}
struct Job {
    receiver: mpsc::Receiver<Result<Prepared>>,
    cancel: Arc<Cancellation>,
    generation: u64,
    version: u64,
    selection: Selection,
    query: Query,
    value: String,
    expand: bool,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
struct Prepared {
    bytes: Vec<u8>,
    count: u64,
    caret: u64,
}
#[derive(Clone, Copy, Default)]
struct Options {
    all: bool,
    expand: bool,
}
struct Budget<'a> {
    bytes: Vec<u8>,
    limit: usize,
    cancel: &'a Cancellation,
}
impl Write for Budget<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.cancel.check().map_err(io::Error::other)?;
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other(
                "置換結果が32 MiBの上限を超えるため反映しません",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn prepare(
    query: &Query,
    value: &str,
    snap: &yy_buffer::Snapshot,
    selection: Range<u64>,
    options: Options,
    cancel: &Cancellation,
    limit: usize,
) -> Result<Prepared> {
    let Options { all, expand } = options;
    cancel.check()?;
    if expand && !query.regex {
        return Err("置換構文を使う場合は検索の[正規表現]も有効にしてください".into());
    }
    if query.pattern.is_empty() {
        return Err("検索文字列を入力してください".into());
    }
    if value.contains('\0') {
        return Err("置換後にNULは指定できません".into());
    }
    let searcher = Searcher::new(query).map_err(|e| e.to_string())?;
    let replacement = if expand {
        Replacement::parse(value, &searcher).map_err(|e| e.to_string())?
    } else {
        Replacement::literal(value)
    };
    let mut step = |_| cancel.check().is_ok();
    if !all
        && searcher
            .find_next(snap, 0..snap.len(), selection.start, &mut step)
            .map_err(|_| "置換を中止しました".to_string())?
            != Some(selection.clone())
    {
        return Err(
            "現在の選択が検索条件に一致しません。RESULTで検索してから置換してください".into(),
        );
    }
    let range = if all {
        0..snap.len()
    } else {
        selection.clone()
    };
    let mut out = Budget {
        bytes: Vec::new(),
        limit,
        cancel,
    };
    let count =
        yy_search::rewrite_streaming(&searcher, snap, range, &replacement, &mut out, &mut step)
            .map_err(|e| e.to_string())?;
    cancel.check()?;
    if !all && count != 1 {
        return Err("選択範囲を単一の一致として置換できません".into());
    }
    let caret = if all {
        0
    } else {
        (out.bytes.len() as u64)
            .checked_sub(snap.len() - selection.end)
            .ok_or("置換後のカーソル位置が不正です")?
    };
    Ok(Prepared {
        bytes: out.bytes,
        count,
        caret,
    })
}
fn ready(a: &App) -> bool {
    !a.folder_mode
        && a.receiver.is_none()
        && a.editors.len() == 4
        && a.loaded.as_ref().is_some_and(|data| data.merged.is_some())
        && !a.editors[3].document.is_busy()
        && a.editors[3].composition.is_none()
}
fn info(a: &App, message: &str) {
    text(a.search_controls.info, message);
}
pub(super) fn cancel(a: &mut App) {
    if a.replace.job.take().is_some() {
        info(a, "置換を中止しました（本文は変更していません）");
    }
    update_menu(a);
}
pub(super) fn update_menu(a: &App) {
    unsafe {
        let enabled = ready(a);
        let _ = EnableMenuItem(
            GetMenu(a.hwnd),
            REPLACE_OPEN as u32,
            MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
        );
        for h in [a.replace_controls.one, a.replace_controls.all] {
            let _ = EnableWindow(
                h,
                enabled && a.search.active == 3 && a.replace.job.is_none(),
            );
        }
    }
}
pub(super) fn layout_controls(a: &mut App, top: i32, width: i32, dpi: i32) -> i32 {
    let visible = a.replace.visible && a.search.visible && !a.folder_mode;
    unsafe {
        for h in a.replace_controls.handles() {
            let _ = ShowWindow(h, if visible { SW_SHOW } else { SW_HIDE });
        }
        if !visible {
            return 0;
        }
        let unit = |n| n * dpi / 96;
        for (h, x, y, w, hgt) in [
            (
                a.replace_controls.label,
                unit(6),
                top + unit(4),
                unit(60),
                unit(25),
            ),
            (
                a.replace_controls.value,
                unit(70),
                top,
                (width - unit(384)).max(1),
                unit(27),
            ),
            (
                a.replace_controls.expand,
                width - unit(308),
                top,
                unit(96),
                unit(27),
            ),
            (
                a.replace_controls.one,
                width - unit(208),
                top,
                unit(110),
                unit(27),
            ),
            (
                a.replace_controls.all,
                width - unit(92),
                top,
                unit(86),
                unit(27),
            ),
        ] {
            let _ = MoveWindow(h, x, y, w, hgt, true);
        }
        unit(33)
    }
}
pub(super) fn tab_targets(a: &App, targets: &mut Vec<HWND>) {
    if a.replace.visible {
        targets.extend([a.replace_controls.value, a.replace_controls.expand]);
        if ready(a) && a.search.active == 3 && a.replace.job.is_none() {
            targets.extend([a.replace_controls.one, a.replace_controls.all]);
        }
    }
}
pub(super) fn notification(id: u16, code: u32) -> bool {
    if !matches!(id, VALUE | EXPAND) {
        return false;
    }
    if (id == VALUE && code == EN_CHANGE) || (id == EXPAND && code == BN_CLICKED) {
        app(|a| {
            cancel(a);
            info(
                a,
                "RESULTのみ置換。[置換構文]は正規表現のキャプチャ・改行等を展開します",
            );
        });
    }
    true
}
pub(super) fn key(a: &mut App, hwnd: HWND, key: usize, shift: bool) -> Option<Result<()>> {
    if !a.replace.visible
        || !a.search.visible
        || a.folder_mode
        || !a.replace_controls.handles().contains(&hwnd)
    {
        return None;
    }
    // Search bar owns common navigation; invoke it with one of its controls.
    match key {
        9 => Some(search_view::tab(a, hwnd, shift)),
        13 => Some(command(
            a,
            if hwnd == a.replace_controls.all {
                REPLACE_ALL
            } else {
                REPLACE_ONE
            },
        )),
        27 => Some(search_view::command(a, FIND_CLOSE)),
        _ => None,
    }
}
pub(super) fn command(a: &mut App, id: u16) -> Result<()> {
    if !ready(a) {
        return Err("置換は処理中でない3方向マージのRESULTだけで使用できます".into());
    }
    if id == REPLACE_OPEN {
        search_view::command(a, FIND_OPEN)?;
        search_view::focus(a, 3);
        a.replace.visible = true;
        layout(a);
        info(
            a,
            "RESULTのみ置換。[置換構文]は正規表現のキャプチャ・改行等を展開します",
        );
        return Ok(());
    }
    if a.search.active != 3 {
        return Err("RESULTを対象に選択してください。LEFT / RIGHT / BASEは読み取り専用です".into());
    }
    let query = a.search_controls.query();
    let value = a.replace_controls.value();
    let expand = a.replace_controls.expand();
    if query.pattern.is_empty() {
        return Err("検索文字列を入力してください".into());
    }
    search_view::cancel(a);
    let snapshot = a.editors[3].document.snapshot().clone();
    let selection = *a.editors[3].document.selections().primary();
    let (generation, version) = (a.generation, a.editors[3].document.version());
    let cancel = Arc::new(Cancellation::default());
    let worker_cancel = cancel.clone();
    let worker_query = query.clone();
    let worker_value = value.clone();
    let (tx, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = prepare(
            &worker_query,
            &worker_value,
            &snapshot,
            selection.range(),
            Options {
                all: id == REPLACE_ALL,
                expand,
            },
            &worker_cancel,
            yy_merge_core::MAX_BYTES as usize,
        );
        let _ = tx.send(result);
    });
    a.replace.job = Some(Job {
        receiver,
        cancel,
        generation,
        version,
        selection,
        query,
        value,
        expand,
    });
    info(a, "置換を計算中… [停止] で中止できます");
    update_menu(a);
    Ok(())
}
pub(super) fn tick(a: &mut App) {
    let Some(job) = &a.replace.job else {
        return;
    };
    let result = match job.receiver.try_recv() {
        Ok(result) => result,
        Err(mpsc::TryRecvError::Empty) => return,
        Err(mpsc::TryRecvError::Disconnected) => Err("置換ワーカーが終了しました".into()),
    };
    let job = a.replace.job.take().unwrap();
    update_menu(a);
    if !ready(a)
        || a.search.active != 3
        || a.generation != job.generation
        || a.editors[3].document.version() != job.version
        || *a.editors[3].document.selections().primary() != job.selection
        || a.search_controls.query() != job.query
        || a.replace_controls.value() != job.value
        || a.replace_controls.expand() != job.expand
    {
        info(a, "文書・選択・条件が変わったため置換を反映しません");
        return;
    }
    let result = result.and_then(|prepared| apply(a, prepared));
    match result {
        Ok(count) => info(
            a,
            &format!("{count}件を置換。手動編集の完了確定とRESULT保存は別操作です"),
        ),
        Err(e) => info(a, &e),
    }
}
fn apply(a: &mut App, prepared: Prepared) -> Result<u64> {
    let Prepared {
        bytes,
        count,
        caret,
    } = prepared;
    if count == 0 {
        return Ok(0);
    }
    let snap = a.editors[3].document.snapshot();
    if snap.len() == bytes.len() as u64
        && snap
            .chunks(0..snap.len())
            .flatten()
            .copied()
            .eq(bytes.iter().copied())
    {
        return Ok(count);
    }
    if bytes.len() as u64 > yy_merge_core::MAX_BYTES
        || bytes.contains(&0)
        || std::str::from_utf8(&bytes).is_err()
    {
        return Err("置換結果のサイズまたは文字列が不正なため反映しません".into());
    }
    let history = if !a.manual { capture(a) } else { None };
    let length = snap.len();
    if !a.editors[3]
        .document
        .replace_ranges(std::slice::from_ref(&(0..length)), |_| bytes.clone())
    {
        return Err("RESULTを編集できないため反映しません".into());
    }
    let e = &mut a.editors[3];
    e.document
        .set_selections(SelectionSet::single(Selection::caret(caret)));
    e.clear_alignment();
    e.marks.clear();
    e.follow_caret = true;
    e.reveal_caret();
    if let Some(history) = history {
        a.history.push(history);
        a.redo.clear();
    }
    a.manual = true;
    a.saved = false;
    a.edited = true;
    request_projection(a, false);
    refresh(a);
    trim_history(a);
    invalidate(a.panes[3]);
    Ok(count)
}

pub(super) fn smoke(a: &mut App, dir: &std::path::Path) -> Result<()> {
    use windows::Win32::UI::Controls::{BST_CHECKED, BST_UNCHECKED};
    let wait = |a: &mut App| -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while a.replace.job.is_some() {
            tick(a);
            if Instant::now() > deadline {
                return Err("replacement smoke timeout".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    };
    let query = |a: &mut App, pattern: &str, regex: bool, value: &str| {
        search_view::cancel(a);
        text(a.search_controls.query_handle(), pattern);
        for (h, checked) in a
            .search_controls
            .option_handles()
            .into_iter()
            .zip([regex, true, false])
        {
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
        text(a.replace_controls.value, value);
    };
    let select = |a: &mut App, range: Range<u64>| {
        a.editors[3]
            .document
            .set_selections(SelectionSet::single(Selection::new(range.start, range.end)))
    };
    let unresolved = a
        .loaded
        .as_ref()
        .unwrap()
        .merged
        .as_ref()
        .unwrap()
        .unresolved();
    let source: Vec<_> = a.editors[..3].iter().map(|e| e.text()).collect();
    let original_options = a.options.clone();
    let dpi = unsafe { GetDpiForWindow(a.hwnd) };
    command(a, REPLACE_OPEN)?;
    for (width, height) in [(680, 420), (1280, 820)] {
        unsafe {
            win(SetWindowPos(
                a.hwnd,
                None,
                0,
                0,
                width,
                height,
                SWP_NOMOVE | SWP_NOZORDER,
            ))?;
        }
        layout(a);
        let mut client = RECT::default();
        unsafe {
            win(GetClientRect(a.hwnd, &mut client))?;
        }
        for h in a
            .replace_controls
            .handles()
            .into_iter()
            .chain(a.panes.iter().copied())
        {
            let mut rect = RECT::default();
            unsafe {
                win(GetWindowRect(h, &mut rect))?;
                MapWindowPoints(
                    None,
                    Some(a.hwnd),
                    std::slice::from_raw_parts_mut((&raw mut rect).cast::<POINT>(), 2),
                );
            }
            if rect.left < 0
                || rect.top < 0
                || rect.right > client.right
                || rect.bottom > client.bottom
                || rect.right <= rect.left
                || rect.bottom <= rect.top
            {
                return Err(format!(
                    "replace controls/pane outside resized client: {rect:?}"
                ));
            }
        }
    }
    a.editors[3] = editor("cat\r\n猫 cat", dpi)?;
    a.saved = true;
    query(a, "cat", false, "cat");
    select(a, 0..3);
    let history = a.history.len();
    let version = a.editors[3].document.version();
    command(a, REPLACE_ONE)?;
    wait(a)?;
    if !a.saved
        || a.manual
        || a.history.len() != history
        || a.editors[3].document.version() != version
    {
        return Err("unchanged replacement dirtied RESULT".into());
    }
    query(a, "cat", false, "犬");
    command(a, REPLACE_ONE)?;
    wait(a)?;
    if a.editors[3].text() != "犬\r\n猫 cat"
        || !a.manual
        || a.saved
        || !a.edited
        || a.history.len() != history + 1
    {
        return Err("single replacement lost manual/unsaved/undo state".into());
    }
    if !a.editors[3].document.undo() || a.editors[3].text() != "cat\r\n猫 cat" {
        return Err("single replacement Undo failed".into());
    }
    if !a.editors[3].document.redo() || a.editors[3].text() != "犬\r\n猫 cat" {
        return Err("single replacement Redo failed".into());
    }
    query(a, "犬|cat", true, "$1\\n");
    command(a, REPLACE_ALL)?;
    wait(a)?;
    if a.editors[3].text() != "$1\\n\r\n猫 $1\\n" {
        return Err("regex search did not replace with literal text".into());
    }
    if !a.editors[3].document.undo() || a.editors[3].text() != "犬\r\n猫 cat" {
        return Err("replace all was not one Undo".into());
    }
    query(a, "(?<animal>犬)|(?<word>cat)", true, "${word}${animal}\\n");
    unsafe {
        SendMessageW(
            a.replace_controls.expand,
            BM_SETCHECK,
            Some(WPARAM(BST_CHECKED.0 as usize)),
            None,
        );
    }
    select(a, 0..3);
    command(a, REPLACE_ONE)?;
    wait(a)?;
    if a.editors[3].text() != "犬\n\r\n猫 cat"
        || a.editors[3].document.selections().primary().head != 4
    {
        return Err("capture single replacement/caret failed".into());
    }
    if !a.editors[3].document.undo() {
        return Err("capture replacement Undo failed".into());
    }
    command(a, REPLACE_ALL)?;
    wait(a)?;
    if a.editors[3].text() != "犬\n\r\n猫 cat\n" {
        return Err("capture all replacement failed".into());
    }
    if !a.editors[3].document.undo() || a.editors[3].text() != "犬\r\n猫 cat" {
        return Err("capture all Undo failed".into());
    }
    text(a.replace_controls.value, "$99");
    command(a, REPLACE_ALL)?;
    wait(a)?;
    if a.editors[3].text() != "犬\r\n猫 cat" {
        return Err("invalid capture changed RESULT".into());
    }
    text(a.replace_controls.value, "x");
    command(a, REPLACE_ALL)?;
    unsafe {
        SendMessageW(
            a.replace_controls.expand,
            BM_SETCHECK,
            Some(WPARAM(BST_UNCHECKED.0 as usize)),
            None,
        );
    }
    wait(a)?;
    if a.editors[3].text() != "犬\r\n猫 cat" {
        return Err("stale expansion mode applied".into());
    }
    let before = a.editors[3].text();
    query(a, "cat", false, "dog");
    select(a, 0..0);
    command(a, REPLACE_ALL)?;
    select(a, 1..1);
    wait(a)?;
    if a.editors[3].text() != before {
        return Err("stale selection replacement applied".into());
    }
    select(a, 0..0);
    command(a, REPLACE_ALL)?;
    a.editors[3].document.insert_text("edit", false);
    let edited = a.editors[3].text();
    wait(a)?;
    if a.editors[3].text() != edited {
        return Err("stale document replacement applied".into());
    }
    command(a, REPLACE_ALL)?;
    text(a.replace_controls.value, "new template");
    wait(a)?;
    if a.editors[3].text() != edited {
        return Err("stale template replacement applied".into());
    }
    command(a, REPLACE_ALL)?;
    text(a.search_controls.query_handle(), "new query");
    wait(a)?;
    if a.editors[3].text() != edited {
        return Err("stale query replacement applied".into());
    }
    query(a, "cat", false, "dog");
    command(a, REPLACE_ALL)?;
    a.generation += 1;
    wait(a)?;
    if a.editors[3].text() != edited {
        return Err("stale generation replacement applied".into());
    }
    command(a, REPLACE_ALL)?;
    search_view::focus(a, 0);
    wait(a)?;
    if a.replace.job.is_some() || command(a, REPLACE_ALL).is_ok() || a.editors[3].text() != edited {
        return Err("read-only pane allowed replacement".into());
    }
    search_view::focus(a, 3);
    command(a, REPLACE_ALL)?;
    cancel(a);
    tick(a);
    if a.editors[3].text() != edited {
        return Err("cancelled replacement applied".into());
    }
    // Reject an oversized candidate at application as well as in the worker.
    let huge = "猫".repeat((yy_merge_core::MAX_BYTES as usize / 3) + 1);
    let failed = Prepared {
        bytes: huge.into_bytes(),
        count: 1,
        caret: 0,
    };
    if apply(a, failed).is_ok() || a.editors[3].text() != edited {
        return Err("oversized replacement applied".into());
    }
    key(a, a.replace_controls.value, 9, false).unwrap()?;
    if unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() }
        != a.replace_controls.expand
    {
        return Err("replacement syntax Tab navigation failed".into());
    }
    key(a, a.replace_controls.expand, 9, false).unwrap()?;
    if unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() } != a.replace_controls.one
    {
        return Err("replacement Tab navigation failed".into());
    }
    command(a, REPLACE_ALL)?;
    key(a, a.replace_controls.value, 27, false).unwrap()?;
    if a.replace.visible || a.replace.job.is_some() || a.editors[3].text() != edited {
        return Err("replacement close did not cancel".into());
    }
    if unresolved
        != a.loaded
            .as_ref()
            .unwrap()
            .merged
            .as_ref()
            .unwrap()
            .unresolved()
        || source != a.editors[..3].iter().map(|e| e.text()).collect::<Vec<_>>()
    {
        return Err("replacement altered sources or resolved conflicts".into());
    }
    begin_options(a, original_options);
    await_rule_smoke(a)?;
    query(a, "", false, "");
    layout(a);
    std::fs::write(dir.join("replace-smoke.txt"),"PASS: RESULT-only single/all replacement, opt-in capture/escape templates and literal default, invalid capture rejected, expanded caret, CRLF, one-step Undo/Redo, no-op preserves saved/version/history, manual/unsaved transition, unresolved and source bytes retained, stale document/selection/query/template/expansion/generation rejected, pane/cancel/close, output budget, Tab navigation, resize 680x420/1280x820\n").map_err(|e|e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn query(pattern: &str, regex: bool) -> Query {
        Query {
            pattern: pattern.into(),
            regex,
            case_sensitive: true,
            whole_word: false,
        }
    }
    fn run(
        text: &str,
        q: &Query,
        value: &str,
        selected: Range<u64>,
        all: bool,
        limit: usize,
    ) -> Result<Prepared> {
        let doc = yy_core::Document::from_text(text);
        prepare(
            q,
            value,
            doc.snapshot(),
            selected,
            Options {
                all,
                ..Default::default()
            },
            &Cancellation::default(),
            limit,
        )
    }
    #[test]
    fn capture_templates_are_explicit_validated_and_bounded() {
        let c = Cancellation::default();
        let q = query(r"(?<name>猫|dog)(?:=(\d+))?", true);
        let doc = yy_core::Document::from_text("猫=12\r\ndog");
        let options = Options {
            all: true,
            expand: true,
        };
        let result = prepare(
            &q,
            r"\U${name}\E:$2\n$$",
            doc.snapshot(),
            0..0,
            options,
            &c,
            1024,
        )
        .unwrap();
        assert_eq!(result.count, 2);
        assert_eq!(
            String::from_utf8(result.bytes).unwrap(),
            "猫:12\n$\r\nDOG:\n$"
        );
        let result = prepare(
            &q,
            r"$2/${name}",
            doc.snapshot(),
            0..6,
            Options {
                all: false,
                expand: true,
            },
            &c,
            1024,
        )
        .unwrap();
        assert_eq!(result.caret, 6);
        assert_eq!(String::from_utf8(result.bytes).unwrap(), "12/猫\r\ndog");
        for template in ["$99", "${unknown}", "${", &format!("${}", "9".repeat(512))] {
            assert!(prepare(&q, template, doc.snapshot(), 0..0, options, &c, 1024).is_err());
        }
        assert!(
            prepare(
                &query("猫", false),
                "$0",
                doc.snapshot(),
                0..0,
                options,
                &c,
                1024
            )
            .is_err()
        );
        let doc = yy_core::Document::from_text("a".repeat(65536).as_str());
        assert!(
            prepare(
                &query("(a+)", true),
                &"$1".repeat(256),
                doc.snapshot(),
                0..0,
                options,
                &c,
                10000
            )
            .is_err()
        );
        assert_eq!(doc.snapshot().len(), 65536);
        let doc = yy_core::Document::from_text("猫");
        let result = prepare(
            &query("^|$", true),
            r"<$0>\n",
            doc.snapshot(),
            0..0,
            options,
            &c,
            11,
        )
        .unwrap();
        assert_eq!(String::from_utf8(result.bytes).unwrap(), "<>\n猫<>\n");
    }
    #[test]
    fn literal_unicode_case_word_and_single_keep_other_source_bytes() {
        let text = "猫 cat scatter CAT\r\ncat";
        let mut q = query("cat", false);
        q.case_sensitive = false;
        q.whole_word = true;
        let output = run(text, &q, "犬", 0..0, true, 1024).unwrap();
        assert_eq!(output.count, 3);
        assert_eq!(
            String::from_utf8(output.bytes).unwrap(),
            "猫 犬 scatter 犬\r\n犬"
        );
        let output = run(text, &q, "$1\\n", 4..7, false, 1024).unwrap();
        assert_eq!(output.count, 1);
        assert_eq!(output.caret, 8);
        assert_eq!(
            String::from_utf8(output.bytes).unwrap(),
            "猫 $1\\n scatter CAT\r\ncat"
        );
        assert!(run(text, &q, "x", 4..6, false, 1024).is_err());
        assert!(run("scatter", &q, "x", 1..4, false, 1024).is_err());
    }
    #[test]
    fn regex_uses_original_context_and_literal_templates_preserving_crlf() {
        let q = query("^cat$", true);
        let output = run("cat\r\ncat\nscatter", &q, "${name}\\n", 0..3, false, 1024).unwrap();
        assert_eq!(
            String::from_utf8(output.bytes).unwrap(),
            "${name}\\n\r\ncat\nscatter"
        );
        assert!(run("cats", &q, "x", 0..3, false, 1024).is_err());
        assert!(run("aaaa", &query("a+", true), "x", 0..2, false, 1024).is_err());
        let output = run("猫x", &query("^|$", true), "前", 0..0, true, 1024).unwrap();
        assert_eq!(output.count, 2);
        assert_eq!(String::from_utf8(output.bytes).unwrap(), "前猫x前");
        let output = run("", &query("^$", true), "猫", 0..0, false, 3).unwrap();
        assert_eq!(output.count, 1);
        assert_eq!(output.bytes, "猫".as_bytes());
    }
    #[test]
    fn deletion_no_matches_budget_invalid_query_nul_and_cancel_are_atomic() {
        let q = query("a", false);
        assert!(run("aaa", &q, "zz", 0..0, true, 5).is_err());
        assert_eq!(
            run("aaa", &q, "zz", 0..0, true, 6).unwrap().bytes,
            b"zzzzzz"
        );
        assert!(
            run("aaaaaaaa", &q, "", 0..0, true, 0)
                .unwrap()
                .bytes
                .is_empty()
        );
        assert_eq!(run("none", &q, "x", 0..0, true, 10).unwrap().count, 0);
        for q in [query("[", true), query("", false)] {
            assert!(run("a", &q, "x", 0..0, true, 10).is_err());
        }
        assert!(run("a", &q, "\0", 0..0, true, 10).is_err());
        let doc = yy_core::Document::from_text("a");
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(
            prepare(
                &q,
                "x",
                doc.snapshot(),
                0..0,
                Options {
                    all: true,
                    ..Default::default()
                },
                &cancel,
                10
            )
            .is_err()
        );
        assert_eq!(doc.snapshot().read(0..1), b"a");
    }
}

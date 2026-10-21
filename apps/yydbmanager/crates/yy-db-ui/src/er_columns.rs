//! Table-scoped spreadsheet editor, sharing yysheet's renderer and clipboard.
use super::*;
use std::cell::Cell as Flag;
use yy_db_core::schema::Model;
#[path = "er_column_data.rs"]
mod data;
use data::{HEADERS, Row};
const WIDTHS: [f32; 6] = [170., 180., 170., 115., 220., 115.];
struct Snapshot {
    rows: Vec<Row>,
    active: (usize, usize),
    anchor: (usize, usize),
}
struct State {
    rows: Vec<Row>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    active: (usize, usize),
    anchor: (usize, usize),
    top: usize,
    left: f32,
    dragging: bool,
    drag_mode: u8,
    error: Option<String>,
    editing: bool,
    done: Flag<bool>,
    accepted: bool,
    model: Model,
    entity: usize,
    scale: f32,
    painter: GridPainter,
    window: HWND,
    grid: HWND,
    input: HWND,
    type_input: HWND,
    type_edit: HWND,
    status: HWND,
}
impl State {
    fn count(&self) -> usize {
        (self.rows.len() + 1).clamp(1, 4096)
    }
    fn rect(&self) -> (usize, usize, usize, usize) {
        (
            self.active.0.min(self.anchor.0),
            self.active.1.min(self.anchor.1),
            self.active.0.max(self.anchor.0),
            self.active.1.max(self.anchor.1),
        )
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            rows: self.rows.clone(),
            active: self.active,
            anchor: self.anchor,
        }
    }
    fn restore(&mut self, snapshot: Snapshot) {
        self.rows = snapshot.rows;
        self.active = snapshot.active;
        self.anchor = snapshot.anchor;
    }
    fn save_undo(&mut self) {
        if self.undo.len() == 32 {
            self.undo.remove(0);
        }
        self.undo.push(self.snapshot());
        self.redo.clear();
    }
    fn row(&mut self, i: usize) {
        self.rows
            .resize_with(self.rows.len().max(i + 1), Row::blank);
    }
    fn input_control(&self) -> HWND {
        if self.active.1 == 2 {
            self.type_input
        } else {
            self.input
        }
    }
    fn input_edit(&self) -> HWND {
        if self.active.1 == 2 {
            self.type_edit
        } else {
            self.input
        }
    }
    fn finish(&mut self, cancel: bool) {
        if self.editing {
            if !cancel {
                let value = text(self.input_control());
                let (r, c) = self.active;
                if self.rows.get(r).map(|r| r.cells[c].as_str()).unwrap_or("") != value {
                    self.save_undo();
                    self.row(r);
                    self.rows[r].cells[c] = value;
                }
            }
            self.editing = false;
            unsafe {
                send_message(self.type_input, CB_SHOWDROPDOWN, WPARAM(0), LPARAM(0));
                let _ = ShowWindow(self.input_control(), SW_HIDE);
            }
        }
    }
    fn begin(&mut self, replace: bool) {
        self.finish(false);
        self.anchor = self.active;
        let (r, c) = self.active;
        let x = 48. + WIDTHS[..c].iter().sum::<f32>() - self.left;
        let y = (r.saturating_sub(self.top) + 1) as f32 * self.painter.row_h;
        let value = if replace {
            ""
        } else {
            self.rows.get(r).map(|r| r.cells[c].as_str()).unwrap_or("")
        };
        let control = self.input_control();
        set_text(control, value);
        unsafe {
            let _ = MoveWindow(
                control,
                (x * self.scale) as i32,
                (y * self.scale) as i32,
                (WIDTHS[c] * self.scale) as i32,
                (if c == 2 {
                    300. * self.scale
                } else {
                    self.painter.row_h * self.scale
                })
                .ceil() as i32,
                true,
            );
            let _ = ShowWindow(control, SW_SHOW);
            let _ = SetFocus(Some(self.input_edit()));
            send_message(self.input_edit(), EM_SETSEL, WPARAM(0), LPARAM(-1));
        }
        self.editing = true;
        invalidate(self.grid);
    }
    fn visible_rows(&self) -> usize {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.grid, &mut r);
        }
        ((r.bottom as f32 / self.scale / self.painter.row_h) as usize)
            .saturating_sub(1)
            .max(1)
    }
    fn refresh(&mut self, reveal: bool) {
        self.active.0 = self.active.0.min(self.count() - 1);
        self.anchor.0 = self.anchor.0.min(self.count() - 1);
        let visible = self.visible_rows();
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.grid, &mut r);
        }
        if reveal {
            if self.active.0 < self.top {
                self.top = self.active.0;
            }
            if self.active.0 >= self.top + visible {
                self.top = self.active.0 + 1 - visible;
            }
            let x = WIDTHS[..self.active.1].iter().sum::<f32>();
            let width = (r.right as f32 / self.scale - 48.).max(1.);
            if x < self.left {
                self.left = x;
            }
            if x + WIDTHS[self.active.1] > self.left + width {
                self.left = x + WIDTHS[self.active.1] - width;
            }
        }
        self.top = self.top.min(self.count().saturating_sub(visible));
        let page = (r.right as f32 / self.scale - 48.).max(1.);
        self.left = self
            .left
            .clamp(0., (WIDTHS.iter().sum::<f32>() - page).max(0.));
        unsafe {
            SetScrollInfo(
                self.grid,
                SB_VERT,
                &SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_ALL,
                    nMin: 0,
                    nMax: (self.count() - 1) as i32,
                    nPage: visible as u32,
                    nPos: self.top as i32,
                    ..Default::default()
                },
                true,
            );
            SetScrollInfo(
                self.grid,
                SB_HORZ,
                &SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_ALL,
                    nMin: 0,
                    nMax: WIDTHS.iter().sum::<f32>() as i32 - 1,
                    nPage: page as u32,
                    nPos: self.left as i32,
                    ..Default::default()
                },
                true,
            );
        }
        let (a, b, c, d) = self.rect();
        let key = self
            .rows
            .get(self.active.0)
            .and_then(|r| r.original)
            .map(|i| {
                let e = &self.model.entities[self.entity];
                let name = &e.fields[i].name;
                let mut tags = vec![];
                if e.keys.iter().any(|k| k.primary && k.columns.contains(name)) {
                    tags.push("主キー");
                }
                if self
                    .model
                    .relations
                    .iter()
                    .any(|r| r.child == e.id && r.columns.contains(name))
                {
                    tags.push("外部キー / 論理関係");
                }
                tags.join("・")
            })
            .unwrap_or_default();
        set_text(
            self.status,
            &format!(
                "{}行目・{}  選択: {}行 × {}列  {}",
                self.active.0 + 1,
                HEADERS[self.active.1],
                c - a + 1,
                d - b + 1,
                key
            ),
        );
        invalidate(self.grid);
    }
    fn hit(&self, l: LPARAM) -> (usize, usize, bool, bool) {
        let x = (l.0 as i16 as f32) / self.scale;
        let y = ((l.0 >> 16) as i16 as f32) / self.scale;
        let r = self.top + ((y / self.painter.row_h) as usize).saturating_sub(1);
        let mut c = 0;
        let mut edge = 48. - self.left + WIDTHS[0];
        while c < 5 && x >= edge {
            c += 1;
            edge += WIDTHS[c];
        }
        (r.min(self.count() - 1), c, x < 48., y < self.painter.row_h)
    }
    fn paint(&mut self) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.grid, &mut r);
        }
        let mut x = -self.left;
        let cols = WIDTHS
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let v = (i as u32, x, *w);
                x += w;
                v
            })
            .collect::<Vec<_>>();
        let count = (self.visible_rows() + 2).min(self.count() - self.top);
        let rows = (self.top..self.top + count)
            .map(|i| (i as u64, (i - self.top) as f32 * self.painter.row_h))
            .collect::<Vec<_>>();
        let cells = (self.top..self.top + count)
            .map(|i| {
                (0..6)
                    .map(|c| Cell {
                        text: self
                            .rows
                            .get(i)
                            .map(|r| r.cells[c].clone())
                            .unwrap_or_default(),
                        ..Default::default()
                    })
                    .collect()
            })
            .collect::<Vec<Vec<Cell>>>();
        let labels = rows.iter().map(|(i, _)| (i + 1, false)).collect::<Vec<_>>();
        let headers = HEADERS
            .iter()
            .enumerate()
            .map(|(i, h)| (i as u32, h.to_string()))
            .collect::<Vec<_>>();
        let (a, b, c, d) = self.rect();
        let scene = Scene {
            cols: &cols,
            rows: &rows,
            header_w: 48.,
            cells: &cells,
            sel: (a as u64, b as u32, c as u64, d as u32),
            active: (self.active.0 as u64, self.active.1 as u32),
            editing: self.editing,
            row_labels: &labels,
            buttons: &[],
            col_types: &headers,
            marks: &[],
            point: None,
            fill: None,
            handle: false,
            fill_button: None,
        };
        let _ = self.painter.paint(
            self.grid,
            r.right.max(1) as u32,
            r.bottom.max(1) as u32,
            &scene,
        );
    }
    fn paste(&mut self) {
        self.finish(false);
        if let Some((text, _)) = yy_ui_win::clipboard::get_text(self.window) {
            let mut rows = self.rows.clone();
            match data::paste(&mut rows, self.active.0, self.active.1, &text) {
                Ok(end) => {
                    self.save_undo();
                    self.rows = rows;
                    self.anchor = end;
                }
                Err(e) => self.error(&e),
            }
        }
        self.refresh(true);
    }
    fn copy(&mut self, cut: bool) {
        self.finish(false);
        let rect = self.rect();
        let mut rows = self.rows.clone();
        rows.resize_with(rows.len().max(rect.2 + 1), Row::blank);
        if yy_ui_win::clipboard::set_text(self.window, &data::copy(&rows, rect), false) && cut {
            self.clear();
        }
    }
    fn clear(&mut self) {
        self.finish(false);
        self.save_undo();
        let (a, b, c, d) = self.rect();
        self.row(c);
        for r in &mut self.rows[a..=c] {
            for v in &mut r.cells[b..=d] {
                v.clear();
            }
        }
        self.refresh(false);
    }
    fn error(&mut self, e: &str) {
        self.error = Some(e.to_owned());
        // Show the modal error only after this event releases the mutable state.
        unsafe {
            let _ = PostMessageW(Some(self.window), WM_APP + 1, WPARAM(0), LPARAM(0));
        }
    }
    fn action(&mut self, id: usize) {
        self.finish(false);
        match id {
            1 => {
                let result = data::fields(&self.rows)
                    .and_then(|rows| self.model.replace_fields(self.entity, &rows));
                match result {
                    Ok(()) => {
                        self.accepted = true;
                        self.done.set(true);
                    }
                    Err(e) => self.error(&e),
                }
            }
            2 => self.done.set(true),
            10 => {
                if self.rows.len() < 4096 {
                    self.save_undo();
                    let i = self.active.0.min(self.rows.len());
                    self.rows.insert(i, Row::blank());
                    self.active = (i, 0);
                    self.anchor = self.active;
                }
            }
            11 => {
                let (a, _, c, _) = self.rect();
                if a < self.rows.len() {
                    self.save_undo();
                    let end = (c + 1).min(self.rows.len());
                    self.rows.drain(a..end);
                    self.active = (a.min(self.count() - 1), 0);
                    self.anchor = self.active;
                }
            }
            12 | 13 => {
                let (a, _, c, _) = self.rect();
                if c < self.rows.len() && (id == 12 && a > 0 || id == 13 && c + 1 < self.rows.len())
                {
                    self.save_undo();
                    if id == 12 {
                        self.rows[a - 1..=c].rotate_left(1);
                        self.active.0 -= 1;
                        self.anchor.0 -= 1;
                    } else {
                        self.rows[a..=c + 1].rotate_right(1);
                        self.active.0 += 1;
                        self.anchor.0 += 1;
                    }
                }
            }
            14 => self.copy(false),
            15 => self.paste(),
            16 => {
                if let Some(snapshot) = self.undo.pop() {
                    self.redo.push(self.snapshot());
                    self.restore(snapshot);
                }
            }
            17 => {
                if let Some(snapshot) = self.redo.pop() {
                    self.undo.push(self.snapshot());
                    self.restore(snapshot);
                }
            }
            _ => {}
        }
        self.refresh(true);
        unsafe {
            let _ = SetFocus(Some(self.grid));
            if self.done.get() {
                let _ = PostMessageW(Some(self.window), WM_NULL, WPARAM(0), LPARAM(0));
            }
        }
    }
    fn key(&mut self, key: u16, ctrl: bool, shift: bool) -> bool {
        if key == VK_F4.0 && self.active.1 == 2 {
            if !self.editing {
                self.begin(false);
            }
            unsafe {
                let open =
                    send_message(self.type_input, CB_GETDROPPEDSTATE, WPARAM(0), LPARAM(0)).0 != 0;
                send_message(
                    self.type_input,
                    CB_SHOWDROPDOWN,
                    WPARAM(usize::from(!open)),
                    LPARAM(0),
                );
            }
            return true;
        }
        if self.editing {
            if key == VK_ESCAPE.0 {
                self.finish(true);
                unsafe {
                    let _ = SetFocus(Some(self.grid));
                }
                self.refresh(false);
                return true;
            }
            if !matches!(key, 9 | 13) {
                return false;
            }
            self.finish(false);
        }
        if ctrl {
            match key {
                65 => {
                    self.anchor = (0, 0);
                    self.active = (self.rows.len().saturating_sub(1), 5);
                }
                67 => self.copy(false),
                88 => self.copy(true),
                86 => self.paste(),
                36 | 35 => {
                    if !shift {
                        self.anchor = if key == 36 {
                            (0, 0)
                        } else {
                            (self.rows.len().saturating_sub(1), 5)
                        };
                    }
                    self.active = if key == 36 {
                        (0, 0)
                    } else {
                        (self.rows.len().saturating_sub(1), 5)
                    };
                }
                90 => self.action(16),
                89 => self.action(17),
                _ => return false,
            }
        } else {
            let (mut r, mut c) = self.active;
            match key {
                9 => {
                    if shift {
                        if c > 0 {
                            c -= 1;
                        } else if r > 0 {
                            r -= 1;
                            c = 5;
                        }
                    } else if c < 5 {
                        c += 1;
                    } else if r < 4095 {
                        r += 1;
                        c = 0;
                    }
                }
                13 => {
                    r = if shift {
                        r.saturating_sub(1)
                    } else {
                        (r + 1).min(4095)
                    }
                }
                37 => c = c.saturating_sub(1),
                39 => c = (c + 1).min(5),
                38 => r = r.saturating_sub(1),
                40 => r = (r + 1).min(4095),
                33 => r = r.saturating_sub(self.visible_rows()),
                34 => r = (r + self.visible_rows()).min(self.count() - 1),
                36 => c = 0,
                35 => c = 5,
                46 => {
                    self.clear();
                    return true;
                }
                113 => {
                    self.begin(false);
                    return true;
                }
                32 if matches!(c, 3 | 5) => {
                    self.save_undo();
                    self.row(r);
                    self.rows[r].cells[c] =
                        if matches!(self.rows[r].cells[c].as_str(), "true" | "1") {
                            "false"
                        } else {
                            "true"
                        }
                        .into();
                    self.refresh(false);
                    return true;
                }
                27 => {
                    self.done.set(true);
                    return true;
                }
                _ => return false,
            }
            if r >= self.count() {
                self.row(r - 1);
            }
            self.active = (r, c);
            if !shift || matches!(key, 9 | 13) {
                self.anchor = self.active;
            }
        }
        unsafe {
            let _ = SetFocus(Some(self.grid));
        }
        self.refresh(true);
        true
    }
    fn layout(&mut self) {
        self.finish(false);
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.window, &mut r);
        }
        let px = |v: i32| (v as f32 * self.scale) as i32;
        let width = r.right;
        let height = r.bottom;
        unsafe {
            let _ = MoveWindow(
                self.grid,
                px(12),
                px(82),
                (width - px(24)).max(1),
                (height - px(166)).max(1),
                true,
            );
            for i in 0..8 {
                if let Ok(b) = GetDlgItem(Some(self.window), 10 + i) {
                    let _ = MoveWindow(b, px(12 + i * 95), px(12), px(90), px(28), true);
                }
            }
            if let Ok(note) = GetDlgItem(Some(self.window), 20) {
                let _ = MoveWindow(note, px(12), px(46), (width - px(24)).max(1), px(32), true);
            }
            let _ = MoveWindow(
                self.status,
                px(12),
                height - px(78),
                (width - px(24)).max(1),
                px(26),
                true,
            );
            for (id, x) in [(1, width - px(232)), (2, width - px(122))] {
                if let Ok(b) = GetDlgItem(Some(self.window), id) {
                    let _ = MoveWindow(b, x, height - px(44), px(104), px(30), true);
                }
            }
        }
        self.refresh(false);
    }
}
unsafe extern "system" fn proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let ptr = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut State;
        if ptr.is_null() {
            return DefWindowProcW(h, msg, w, l);
        }
        if msg == WM_APP + 1 {
            let error = (*ptr).error.take();
            if let Some(error) = error {
                MessageBoxW(
                    Some(h),
                    &HSTRING::from(error),
                    w!("列定義を確認してください"),
                    MB_OK | MB_ICONERROR,
                );
            }
            return LRESULT(0);
        }
        let grid = GetDlgCtrlID(h) == 100;
        // Ignore creation/focus/nonclient notifications before borrowing state:
        // MoveWindow, SetFocus and scrollbar changes can dispatch them synchronously.
        let handled = if grid {
            matches!(
                msg,
                WM_ERASEBKGND
                    | WM_PAINT
                    | WM_LBUTTONDOWN
                    | WM_LBUTTONDBLCLK
                    | WM_MOUSEMOVE
                    | WM_LBUTTONUP
                    | WM_MOUSEWHEEL
                    | WM_VSCROLL
                    | WM_HSCROLL
                    | WM_COPY
                    | WM_CUT
                    | WM_PASTE
                    | WM_CLEAR
                    | WM_KEYDOWN
                    | WM_IME_STARTCOMPOSITION
                    | WM_CHAR
                    | WM_GETDLGCODE
            )
        } else {
            matches!(msg, WM_CLOSE | WM_SIZE | WM_GETMINMAXINFO)
                || msg == WM_COMMAND && w.0 >> 16 == 0
        };
        if !handled {
            return DefWindowProcW(h, msg, w, l);
        }
        let s = &mut *ptr;
        if !grid {
            match msg {
                WM_CLOSE => {
                    s.done.set(true);
                    let _ = PostMessageW(Some(h), WM_NULL, WPARAM(0), LPARAM(0));
                    return LRESULT(0);
                }
                WM_COMMAND if w.0 >> 16 == 0 => {
                    s.action(w.0 & 0xffff);
                    return LRESULT(0);
                }
                WM_SIZE => {
                    s.layout();
                    return LRESULT(0);
                }
                WM_GETMINMAXINFO => {
                    let m = &mut *(l.0 as *mut MINMAXINFO);
                    m.ptMinTrackSize = POINT {
                        x: (800. * s.scale) as i32,
                        y: (360. * s.scale) as i32,
                    };
                    return LRESULT(0);
                }
                _ => {}
            }
        } else {
            match msg {
                WM_ERASEBKGND => return LRESULT(1),
                WM_PAINT => {
                    let mut ps = PAINTSTRUCT::default();
                    let _ = BeginPaint(h, &mut ps);
                    s.paint();
                    let _ = EndPaint(h, &ps);
                    return LRESULT(0);
                }
                WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                    s.finish(false);
                    let _ = SetFocus(Some(h));
                    let (r, c, row_header, col_header) = s.hit(l);
                    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                    s.drag_mode = if row_header && col_header {
                        3
                    } else if row_header {
                        1
                    } else if col_header {
                        2
                    } else {
                        0
                    };
                    if row_header && col_header {
                        s.anchor = (0, 0);
                        s.active = (s.rows.len().saturating_sub(1), 5);
                    } else if row_header {
                        s.anchor = (if shift { s.anchor.0 } else { r }, 0);
                        s.active = (r, 5);
                    } else if col_header {
                        s.anchor = (0, if shift { s.anchor.1 } else { c });
                        s.active = (s.rows.len().saturating_sub(1), c);
                    } else {
                        s.active = (r, c);
                        if GetKeyState(VK_SHIFT.0 as i32) >= 0 {
                            s.anchor = s.active;
                        }
                    }
                    s.dragging = true;
                    SetCapture(h);
                    s.refresh(false);
                    if msg == WM_LBUTTONDBLCLK && !row_header && !col_header {
                        s.dragging = false;
                        let _ = ReleaseCapture();
                        s.begin(false);
                    }
                    return LRESULT(0);
                }
                WM_MOUSEMOVE if s.dragging && GetCapture() == h => {
                    let (r, c, _, _) = s.hit(l);
                    s.active = match s.drag_mode {
                        1 => (r, 5),
                        2 => (s.rows.len().saturating_sub(1), c),
                        3 => s.active,
                        _ => (r, c),
                    };
                    s.refresh(true);
                    return LRESULT(0);
                }
                WM_LBUTTONUP => {
                    s.dragging = false;
                    let _ = ReleaseCapture();
                    return LRESULT(0);
                }
                WM_MOUSEWHEEL => {
                    s.finish(false);
                    let delta = (w.0 >> 16) as i16 as i32 / 120;
                    if GetKeyState(VK_SHIFT.0 as i32) < 0 {
                        s.left -= delta as f32 * 80.;
                    } else {
                        s.top = s.top.saturating_add_signed(-delta as isize * 3);
                    }
                    s.refresh(false);
                    return LRESULT(0);
                }
                WM_VSCROLL | WM_HSCROLL => {
                    s.finish(false);
                    let vert = msg == WM_VSCROLL;
                    let bar = if vert { SB_VERT } else { SB_HORZ };
                    let mut info = SCROLLINFO {
                        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                        fMask: SIF_ALL,
                        ..Default::default()
                    };
                    let _ = GetScrollInfo(h, bar, &mut info);
                    let pos = if vert { s.top as i32 } else { s.left as i32 };
                    let step = if vert { 1 } else { 30 };
                    let next = match w.0 & 0xffff {
                        0 => pos - step,
                        1 => pos + step,
                        2 => pos - info.nPage as i32,
                        3 => pos + info.nPage as i32,
                        4 | 5 => info.nTrackPos,
                        6 => 0,
                        7 => info.nMax,
                        _ => pos,
                    };
                    if vert {
                        s.top = next.max(0) as usize;
                    } else {
                        s.left = next.max(0) as f32;
                    }
                    s.refresh(false);
                    return LRESULT(0);
                }
                WM_COPY => {
                    s.copy(false);
                    return LRESULT(0);
                }
                WM_CUT => {
                    s.copy(true);
                    return LRESULT(0);
                }
                WM_PASTE => {
                    s.paste();
                    return LRESULT(0);
                }
                WM_CLEAR => {
                    s.clear();
                    return LRESULT(0);
                }
                WM_KEYDOWN => {
                    if s.key(
                        w.0 as u16,
                        GetKeyState(VK_CONTROL.0 as i32) < 0,
                        GetKeyState(VK_SHIFT.0 as i32) < 0,
                    ) {
                        return LRESULT(0);
                    }
                }
                WM_IME_STARTCOMPOSITION => {
                    s.begin(true);
                    return LRESULT(0);
                }
                WM_CHAR if w.0 >= 32 => {
                    if !s.editing {
                        s.begin(true);
                    }
                    send_message(s.input_edit(), WM_CHAR, w, l);
                    return LRESULT(0);
                }
                WM_GETDLGCODE => {
                    return LRESULT((DLGC_WANTARROWS | DLGC_WANTCHARS | DLGC_WANTTAB) as isize);
                }
                _ => {}
            }
        }
        DefWindowProcW(h, msg, w, l)
    }
}
pub(super) fn edit(
    parent: HWND,
    font: HFONT,
    dpi: u32,
    model: &mut Model,
    entity: usize,
) -> yy_db_core::Result<bool> {
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        for class in [w!("YYDbERColumns"), w!("YYDbERColumnGrid")] {
            RegisterClassW(&WNDCLASSW {
                hInstance: instance.into(),
                lpszClassName: class,
                lpfnWndProc: Some(proc),
                style: CS_DBLCLKS,
                hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            });
        }
        let mut state = Box::new(State {
            rows: data::from_entity(&model.entities[entity]),
            undo: vec![],
            redo: vec![],
            active: (0, 0),
            anchor: (0, 0),
            top: 0,
            left: 0.,
            dragging: false,
            drag_mode: 0,
            error: None,
            editing: false,
            done: Flag::new(false),
            accepted: false,
            model: model.clone(),
            entity,
            scale: dpi as f32 / 96.,
            painter: GridPainter::new(font::BUNDLED_FAMILY, 11., dpi).map_err(|e| e.to_string())?,
            window: HWND::default(),
            grid: HWND::default(),
            input: HWND::default(),
            type_input: HWND::default(),
            type_edit: HWND::default(),
            status: HWND::default(),
        });
        let px = |v: i32| v * dpi as i32 / 96;
        let mut r = RECT::default();
        let _ = GetWindowRect(parent, &mut r);
        let window = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            w!("YYDbERColumns"),
            &HSTRING::from(format!(
                "列定義をまとめて編集 — {}",
                model.entities[entity].id.name
            )),
            WS_OVERLAPPEDWINDOW,
            r.left + px(24),
            r.top + px(24),
            px(1100),
            px(650),
            Some(parent),
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| e.to_string())?;
        state.window = window;
        // Create all controls before installing state, so creation notifications cannot reenter it.
        let create = || -> windows::core::Result<(HWND, HWND, HWND, HWND, HWND)> {
            let grid = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("YYDbERColumnGrid"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_VSCROLL | WS_HSCROLL,
                0,
                0,
                1,
                1,
                Some(window),
                Some(HMENU(100 as *mut _)),
                Some(instance.into()),
                None,
            )?;
            let input = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                0,
                0,
                1,
                1,
                Some(grid),
                Some(HMENU(101 as *mut _)),
                Some(instance.into()),
                None,
            )?;
            send_message(input, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
            send_message(input, EM_SETLIMITTEXT, WPARAM(32768), LPARAM(0));
            let type_input = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("COMBOBOX"),
                w!(""),
                WS_CHILD | WS_VSCROLL | WINDOW_STYLE((CBS_DROPDOWN | CBS_AUTOHSCROLL) as u32),
                0,
                0,
                px(170),
                px(300),
                Some(grid),
                Some(HMENU(102 as *mut _)),
                Some(instance.into()),
                None,
            )?;
            send_message(type_input, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
            super::types::populate(type_input, &super::types::choices(model));
            let mut info = COMBOBOXINFO {
                cbSize: std::mem::size_of::<COMBOBOXINFO>() as u32,
                ..Default::default()
            };
            GetComboBoxInfo(type_input, &mut info)?;
            let type_edit = info.hwndItem;
            send_message(type_edit, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
            let status = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!(""),
                WS_CHILD | WS_VISIBLE,
                0,
                0,
                1,
                1,
                Some(window),
                Some(HMENU(21 as *mut _)),
                Some(instance.into()),
                None,
            )?;
            send_message(status, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
            for (id, label) in [
                (10, "行を挿入"),
                (11, "行を削除"),
                (12, "上へ移動"),
                (13, "下へ移動"),
                (14, "コピー"),
                (15, "貼り付け"),
                (16, "元に戻す"),
                (17, "やり直し"),
                (1, "適用"),
                (2, "キャンセル"),
                (
                    20,
                    "Tab / Enter: 次のセル  Shift: 逆方向  F2 / ダブルクリック: 編集  Ctrl+C / V: コピー・貼り付け  型の候補: F2 / F4",
                ),
            ] {
                let b = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    if id == 20 { w!("STATIC") } else { w!("BUTTON") },
                    &HSTRING::from(label),
                    WS_CHILD
                        | WS_VISIBLE
                        | if id == 20 {
                            WINDOW_STYLE::default()
                        } else {
                            WS_TABSTOP
                        },
                    0,
                    0,
                    1,
                    1,
                    Some(window),
                    Some(HMENU(id as *mut _)),
                    Some(instance.into()),
                    None,
                )?;
                send_message(b, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(0));
            }
            Ok((grid, input, status, type_input, type_edit))
        };
        let controls = create();
        let (grid, input, status, type_input, type_edit) = match controls {
            Ok(v) => v,
            Err(e) => {
                let _ = DestroyWindow(window);
                return Err(e.to_string());
            }
        };
        state.grid = grid;
        state.input = input;
        state.type_input = type_input;
        state.type_edit = type_edit;
        state.status = status;
        let ptr = &mut *state as *mut State as isize;
        SetWindowLongPtrW(window, GWLP_USERDATA, ptr);
        SetWindowLongPtrW(grid, GWLP_USERDATA, ptr);
        state.layout();
        let _ = EnableWindow(parent, false);
        let _ = ShowWindow(window, SW_SHOW);
        let _ = SetFocus(Some(grid));
        let mut msg = MSG::default();
        while !state.done.get() {
            let code = GetMessageW(&mut msg, None, 0, 0).0;
            if code <= 0 {
                if code == 0 {
                    PostQuitMessage(msg.wParam.0 as i32);
                }
                break;
            }
            let focus = GetFocus();
            let in_grid =
                focus == grid || focus == input || focus == type_input || focus == type_edit;
            if msg.message == WM_SYSKEYDOWN
                && msg.wParam.0 == VK_DOWN.0 as usize
                && in_grid
                && state.active.1 == 2
            {
                if !state.editing {
                    state.begin(false);
                }
                send_message(type_input, CB_SHOWDROPDOWN, WPARAM(1), LPARAM(0));
                continue;
            }
            if msg.message == WM_KEYDOWN
                && in_grid
                && state.key(
                    msg.wParam.0 as u16,
                    GetKeyState(VK_CONTROL.0 as i32) < 0,
                    GetKeyState(VK_SHIFT.0 as i32) < 0,
                )
            {
                continue;
            }
            // Let EDIT handle ordinary typing/IME; grid owns navigation and text entry.
            if in_grid || !IsDialogMessageW(window, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let accepted = state.accepted;
        if accepted {
            *model = state.model.clone();
        }
        SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        SetWindowLongPtrW(grid, GWLP_USERDATA, 0);
        let _ = EnableWindow(parent, true);
        let _ = DestroyWindow(window);
        let _ = SetForegroundWindow(parent);
        Ok(accepted)
    }
}

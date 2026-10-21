use super::*;
use yy_db_core::plan_visual::{self as visual, Hit};
use yy_draw::Point;

const ZOOM_IN: u16 = 9001;
const ZOOM_OUT: u16 = 9002;
const ZOOM_100: u16 = 9003;
const ZOOM_FIT: u16 = 9004;
const ZOOM_CUSTOM: u16 = 9005;
const PROPERTIES: u16 = 9006;
const FIND: u16 = 9007;
const RAW_SAVE: u16 = 9008;
const RAW_VIEW: u16 = 9009;

pub(super) struct View {
    pub selected: Option<Hit>,
    hover: Option<Hit>,
    hover_since: Instant,
    cursor: POINT,
    tip: HWND,
    tip_text: Vec<u16>,
    tip_visible: bool,
    drag: Option<Drag>,
    pub needs_fit: bool,
    search: String,
}
enum Drag {
    Pan(POINT),
    Navigator,
}
impl View {
    pub fn new(grid: HWND, properties: HWND, dpi: u32) -> Result<Self> {
        unsafe {
            let tip = CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                None,
                WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                Some(grid),
                None,
                None,
                None,
            )?;
            let mut tip_text = wide("");
            let info = TTTOOLINFOW {
                cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
                uFlags: TTF_TRACK | TTF_ABSOLUTE,
                hwnd: grid,
                uId: 1,
                lpszText: windows::core::PWSTR(tip_text.as_mut_ptr()),
                ..Default::default()
            };
            send_message(
                tip,
                TTM_ADDTOOLW,
                WPARAM(0),
                LPARAM(&info as *const _ as isize),
            );
            send_message(
                tip,
                TTM_SETMAXTIPWIDTH,
                WPARAM(0),
                LPARAM((480 * dpi / 96) as isize),
            );
            send_message(
                properties,
                LVM_SETEXTENDEDLISTVIEWSTYLE,
                WPARAM(0),
                LPARAM((LVS_EX_FULLROWSELECT | LVS_EX_GRIDLINES | LVS_EX_DOUBLEBUFFER) as isize),
            );
            for (i, (label, width)) in [("プロパティ", 145), ("値", 280)].into_iter().enumerate()
            {
                let mut label = wide(label);
                let column = LVCOLUMNW {
                    mask: LVCF_TEXT | LVCF_WIDTH,
                    pszText: windows::core::PWSTR(label.as_mut_ptr()),
                    cx: width * dpi as i32 / 96,
                    ..Default::default()
                };
                send_message(
                    properties,
                    LVM_INSERTCOLUMNW,
                    WPARAM(i),
                    LPARAM(&column as *const _ as isize),
                );
            }
            Ok(Self {
                selected: None,
                hover: None,
                hover_since: Instant::now(),
                cursor: POINT::default(),
                tip,
                tip_text,
                tip_visible: false,
                drag: None,
                needs_fit: false,
                search: String::new(),
            })
        }
    }
    fn hide_tip(&mut self, grid: HWND) {
        if self.tip_visible {
            let info = TTTOOLINFOW {
                cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
                hwnd: grid,
                uId: 1,
                ..Default::default()
            };
            unsafe {
                send_message(
                    self.tip,
                    TTM_TRACKACTIVATE,
                    WPARAM(0),
                    LPARAM(&info as *const _ as isize),
                );
            }
            self.tip_visible = false;
        }
    }
}

fn active(a: &App) -> bool {
    a.plan.is_some() && a.drawing.is_some()
}
fn client(hwnd: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    r
}
fn screen_point(l: LPARAM) -> POINT {
    POINT {
        x: l.0 as i16 as i32,
        y: (l.0 >> 16) as i16 as i32,
    }
}
fn at(a: &App, p: POINT) -> Option<Hit> {
    visual::hit(
        a.plan.as_ref()?,
        a.drawing.as_ref()?,
        Point::new(
            a.origin.x + p.x as f64 / a.zoom,
            a.origin.y + p.y as f64 / a.zoom,
        ),
        6. / a.zoom,
    )
}

pub(super) fn present(a: &mut App, plan: Plan) -> yy_db_core::Result<()> {
    visual::validate(&plan)?;
    a.plan_view.hide_tip(a.grid_hwnd);
    a.plan_view.hover = None;
    a.plan_view.selected = Some(Hit::Node(0));
    a.plan_view.drag = None;
    a.drawing = Some(plan.drawing());
    a.plan = Some(plan);
    a.zoom = a.dpi as f64 / 96.;
    let bounds = a.drawing.as_ref().unwrap().bounds();
    a.origin = Point::new(bounds.x - 16., bounds.y - 16.);
    a.plan_view.needs_fit = false;
    properties(a);
    invalidate(a.grid_hwnd);
    set_text(
        a.status,
        "実行計画: 右→左がデータの流れ。Ctrl+ホイールでズーム、空白をドラッグして移動、右クリックでズーム／検索／保存。コストは推定値です。",
    );
    Ok(())
}
pub(super) fn properties(a: &mut App) {
    unsafe {
        send_message(a.properties, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    }
    let Some((plan, hit)) = a.plan.as_ref().zip(a.plan_view.selected) else {
        return;
    };
    let (index, edge) = match hit {
        Hit::Node(i) => (i, false),
        Hit::Edge(i) => (i, true),
    };
    property_rows(a, visual::details(plan, index, edge));
}
pub(super) fn property_rows(a: &mut App, rows: Vec<(String, String)>) {
    unsafe {
        send_message(a.properties, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    }
    for (i, (name, value)) in rows.into_iter().enumerate() {
        let mut name = wide(&name);
        let item = LVITEMW {
            mask: LVIF_TEXT,
            iItem: i as i32,
            pszText: windows::core::PWSTR(name.as_mut_ptr()),
            ..Default::default()
        };
        let mut value = wide(&value);
        let sub = LVITEMW {
            iSubItem: 1,
            pszText: windows::core::PWSTR(value.as_mut_ptr()),
            ..Default::default()
        };
        unsafe {
            send_message(
                a.properties,
                LVM_INSERTITEMW,
                WPARAM(0),
                LPARAM(&item as *const _ as isize),
            );
            send_message(
                a.properties,
                LVM_SETITEMTEXTW,
                WPARAM(i),
                LPARAM(&sub as *const _ as isize),
            );
        }
    }
}

fn fit(a: &mut App, allow_grow: bool) {
    let Some(doc) = &a.drawing else {
        return;
    };
    let r = client(a.grid_hwnd);
    let bounds = doc.bounds();
    let scale =
        ((r.right - 32).max(1) as f64 / bounds.w).min((r.bottom - 32).max(1) as f64 / bounds.h);
    let dpi = a.dpi as f64 / 96.;
    a.zoom = scale.clamp(0.1 * dpi, if allow_grow { 4. * dpi } else { dpi });
    a.origin = Point::new(
        bounds.x - (r.right as f64 / a.zoom - bounds.w) / 2.,
        bounds.y - (r.bottom as f64 / a.zoom - bounds.h) / 2.,
    );
    invalidate(a.grid_hwnd);
}
fn zoom_at(a: &mut App, factor: f64, p: POINT) {
    let old = a.zoom;
    let dpi = a.dpi as f64 / 96.;
    a.zoom = factor.clamp(0.1 * dpi, 4. * dpi);
    a.origin.x += p.x as f64 / old - p.x as f64 / a.zoom;
    a.origin.y += p.y as f64 / old - p.y as f64 / a.zoom;
    a.plan_view.hide_tip(a.grid_hwnd);
    invalidate(a.grid_hwnd);
}

/// Win32 calls which can send size/paint notifications run after APP is released.
pub(super) fn sync(hwnd: HWND) {
    let state = with_app(|a| {
        if !active(a) {
            a.plan_view.hide_tip(a.grid_hwnd);
            return None;
        }
        if a.plan_view.needs_fit {
            a.plan_view.needs_fit = false;
            fit(a, false);
        }
        let bounds = a.drawing.as_ref().unwrap().bounds().expand(24.);
        Some((bounds, a.origin, a.zoom, a.properties, a.dpi))
    })
    .flatten();
    unsafe {
        let _ = ShowScrollBar(hwnd, SB_BOTH, state.is_some());
        if let Some((bounds, origin, zoom, properties, dpi)) = state {
            let r = client(hwnd);
            for (bar, size, page, pos) in [
                (SB_HORZ, bounds.w, r.right, (origin.x - bounds.x) * zoom),
                (SB_VERT, bounds.h, r.bottom, (origin.y - bounds.y) * zoom),
            ] {
                let info = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
                    nMin: 0,
                    nMax: (size * zoom).ceil() as i32,
                    nPage: page.max(1) as u32,
                    nPos: pos as i32,
                    ..Default::default()
                };
                SetScrollInfo(hwnd, bar, &info, true);
            }
            send_message(
                properties,
                LVM_SETCOLUMNWIDTH,
                WPARAM(0),
                LPARAM((145 * dpi / 96) as isize),
            );
        }
    }
}

pub(super) fn tick() {
    let grid = with_app(|a| {
        if !active(a) {
            a.plan_view.hide_tip(a.grid_hwnd);
            return None;
        }
        if a.plan_view.needs_fit {
            return Some(a.grid_hwnd);
        }
        if a.plan_view.drag.is_none()
            && !a.plan_view.tip_visible
            && a.plan_view.hover_since.elapsed().as_millis() >= 450
            && let Some(hit) = a.plan_view.hover
        {
            a.plan_view.tip_text = wide(&visual::tooltip(a.plan.as_ref().unwrap(), hit));
            let info = TTTOOLINFOW {
                cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
                hwnd: a.grid_hwnd,
                uId: 1,
                lpszText: windows::core::PWSTR(a.plan_view.tip_text.as_mut_ptr()),
                ..Default::default()
            };
            let mut p = a.plan_view.cursor;
            unsafe {
                let _ = ClientToScreen(a.grid_hwnd, &mut p);
                send_message(
                    a.plan_view.tip,
                    TTM_UPDATETIPTEXTW,
                    WPARAM(0),
                    LPARAM(&info as *const _ as isize),
                );
                send_message(
                    a.plan_view.tip,
                    TTM_TRACKPOSITION,
                    WPARAM(0),
                    LPARAM(((p.x + 18) as u16 as u32 | ((p.y + 22) as u16 as u32) << 16) as isize),
                );
                send_message(
                    a.plan_view.tip,
                    TTM_TRACKACTIVATE,
                    WPARAM(1),
                    LPARAM(&info as *const _ as isize),
                );
            }
            a.plan_view.tip_visible = true;
        }
        None
    })
    .flatten();
    if let Some(grid) = grid {
        sync(grid);
    }
}

fn find(a: &mut App, next: bool) {
    let Some(plan) = &a.plan else {
        return;
    };
    let needle = a.plan_view.search.to_lowercase();
    if needle.is_empty() {
        return;
    }
    let start = if next {
        a.plan_view
            .selected
            .map(|h| match h {
                Hit::Node(i) | Hit::Edge(i) => i + 1,
            })
            .unwrap_or(0)
    } else {
        0
    };
    if let Some(index) = (0..plan.nodes.len())
        .map(|n| (n + start) % plan.nodes.len())
        .find(|&i| {
            let node = &plan.nodes[i];
            format!(
                "{} {} {}",
                node.operator,
                node.object,
                node.properties
                    .get("Expressions")
                    .map(String::as_str)
                    .unwrap_or("")
            )
            .to_lowercase()
            .contains(&needle)
        })
    {
        a.plan_view.selected = Some(Hit::Node(index));
        let rect = a.drawing.as_ref().unwrap().nodes[index].rect;
        let r = client(a.grid_hwnd);
        a.origin = Point::new(
            rect.center().x - r.right as f64 / a.zoom / 2.,
            rect.center().y - r.bottom as f64 / a.zoom / 2.,
        );
        properties(a);
        invalidate(a.grid_hwnd);
    } else {
        set_text(a.status, "一致する計画ノードがありません。");
    }
}
pub(super) fn action(hwnd: HWND, id: u16) {
    let context = with_app(|a| {
        (
            unsafe { GetParent(hwnd).unwrap_or_default() },
            a.font,
            a.dpi,
            a.zoom,
            a.plan_view.search.clone(),
        )
    });
    let Some((parent, font, dpi, zoom, search)) = context else {
        return;
    };
    let answer = match id {
        ZOOM_CUSTOM => input_text(
            parent,
            "ズーム倍率（10〜400%）",
            &format!("{:.0}", zoom * 96. / dpi as f64 * 100.),
            font,
            dpi,
        ),
        FIND => input_text(parent, "計画ノードを検索", &search, font, dpi),
        _ => None,
    };
    if matches!(id, ZOOM_CUSTOM | FIND) && answer.is_none() {
        return;
    }
    if id == RAW_VIEW {
        if let Some(raw) = with_app(|a| a.plan.as_ref().map(|p| p.raw.clone())).flatten() {
            detail_dialog(parent, "実行計画の原文（XML / JSON）", &raw, font, dpi);
        }
        return;
    }
    if id == RAW_SAVE {
        let source = with_app(|a| {
            a.plan
                .as_ref()
                .map(|p| (p.raw.clone(), p.kind == DbKind::SqlServer))
        })
        .flatten();
        if let Some((raw, xml)) = source
            && let Some(path) = file_dialog(true, if xml { "sqlplan" } else { "json" })
            && let Err(e) = std::fs::write(path, raw)
        {
            message(&e.to_string());
        }
        return;
    }
    with_app(|a| {
        let r = client(hwnd);
        let center = POINT {
            x: r.right / 2,
            y: r.bottom / 2,
        };
        match id {
            ZOOM_IN => zoom_at(a, a.zoom * 1.25, center),
            ZOOM_OUT => zoom_at(a, a.zoom / 1.25, center),
            ZOOM_100 => zoom_at(a, a.dpi as f64 / 96., center),
            ZOOM_FIT => fit(a, true),
            ZOOM_CUSTOM => {
                if let Some(factor) = answer
                    .as_ref()
                    .and_then(|s| s.trim().trim_end_matches('%').parse::<f64>().ok())
                    .filter(|v| v.is_finite() && (10. ..=400.).contains(v))
                {
                    zoom_at(a, factor / 100. * a.dpi as f64 / 96., center);
                } else {
                    set_text(a.status, "ズーム倍率は10〜400の数値で指定してください。");
                }
            }
            FIND => {
                a.plan_view.search = answer.unwrap();
                find(a, false);
            }
            PROPERTIES => {
                properties(a);
                unsafe {
                    let _ = SetFocus(Some(a.properties));
                }
            }
            _ => {}
        }
    });
    sync(hwnd);
}
fn context_menu(hwnd: HWND, mut point: POINT) {
    if point.x == -1 && point.y == -1 {
        let r = client(hwnd);
        point = POINT {
            x: r.right / 2,
            y: r.bottom / 2,
        };
        unsafe {
            let _ = ClientToScreen(hwnd, &mut point);
        }
    }
    let mut local = point;
    unsafe {
        let _ = ScreenToClient(hwnd, &mut local);
    }
    with_app(|a| {
        a.plan_view.hide_tip(hwnd);
        if let Some(hit) = at(a, local) {
            a.plan_view.selected = Some(hit);
            properties(a);
        }
        invalidate(hwnd);
    });
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        for (id, label) in [
            (SAVE_PLAN, "実行計画を保存…"),
            (RAW_VIEW, "実行計画の原文を表示（XML / JSON）…"),
            (RAW_SAVE, "原文を保存（SQL Server: .sqlplan）…"),
            (0, ""),
            (FIND, "ノードを検索…\tCtrl+F"),
            (0, ""),
            (ZOOM_IN, "拡大"),
            (ZOOM_OUT, "縮小"),
            (ZOOM_100, "100%"),
            (ZOOM_CUSTOM, "ズーム倍率を指定…"),
            (ZOOM_FIT, "全体を表示\tCtrl+0"),
            (0, ""),
            (PROPERTIES, "プロパティ\tF4"),
        ] {
            let _ = AppendMenuW(
                menu,
                if id == 0 { MF_SEPARATOR } else { MF_STRING },
                id as usize,
                &HSTRING::from(label),
            );
        }
        let id = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            None,
            hwnd,
            None,
        )
        .0 as u16;
        let _ = DestroyMenu(menu);
        if id == SAVE_PLAN {
            command(SAVE_PLAN);
        } else if id != 0 {
            action(hwnd, id);
        }
    }
}

fn map_rect(r: &RECT, dpi: u32) -> RECT {
    let px = |v: i32| v * dpi as i32 / 96;
    RECT {
        left: r.right - px(230),
        top: r.bottom - px(164),
        right: r.right - px(24),
        bottom: r.bottom - px(24),
    }
}
fn navigator_move(a: &mut App, p: POINT) {
    let r = map_rect(&client(a.grid_hwnd), a.dpi);
    let bounds = a.drawing.as_ref().unwrap().bounds().expand(20.);
    let view = client(a.grid_hwnd);
    let x = ((p.x - r.left) as f64 / (r.right - r.left) as f64).clamp(0., 1.);
    let y = ((p.y - r.top) as f64 / (r.bottom - r.top) as f64).clamp(0., 1.);
    a.origin = Point::new(
        bounds.x + x * bounds.w - view.right as f64 / a.zoom / 2.,
        bounds.y + y * bounds.h - view.bottom as f64 / a.zoom / 2.,
    );
    invalidate(a.grid_hwnd);
}

pub(super) fn handle(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    if !with_app(|a| active(a)).unwrap_or(false) {
        return None;
    }
    match msg {
        WM_CONTEXTMENU => context_menu(hwnd, screen_point(l)),
        WM_SIZE => {
            invalidate(hwnd);
        }
        WM_LBUTTONDOWN | WM_MBUTTONDOWN => {
            let p = screen_point(l);
            with_app(|a| {
                a.plan_view.hide_tip(hwnd);
                a.plan_view.hover = None;
                let r = client(hwnd);
                let px = 24 * a.dpi as i32 / 96;
                if p.x > r.right - px && p.y > r.bottom - px {
                    a.plan_view.drag = Some(Drag::Navigator);
                } else if let Some(hit) = at(a, p).filter(|_| msg == WM_LBUTTONDOWN) {
                    a.plan_view.selected = Some(hit);
                    properties(a);
                } else {
                    a.plan_view.drag = Some(Drag::Pan(p));
                }
                invalidate(hwnd);
            });
            unsafe {
                let _ = SetFocus(Some(hwnd));
                SetCapture(hwnd);
            }
        }
        WM_LBUTTONUP | WM_MBUTTONUP | WM_CAPTURECHANGED => {
            with_app(|a| {
                a.plan_view.drag = None;
                invalidate(hwnd);
            });
            if msg != WM_CAPTURECHANGED {
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            sync(hwnd);
        }
        WM_MOUSEMOVE => {
            let dragging = with_app(|a| a.plan_view.drag.is_some()).unwrap_or(false);
            let p = screen_point(l);
            with_app(|a| {
                if let Some(drag) = a.plan_view.drag.take() {
                    match drag {
                        Drag::Pan(previous) => {
                            a.origin.x -= (p.x - previous.x) as f64 / a.zoom;
                            a.origin.y -= (p.y - previous.y) as f64 / a.zoom;
                            a.plan_view.drag = Some(Drag::Pan(p));
                            invalidate(hwnd);
                        }
                        Drag::Navigator => {
                            navigator_move(a, p);
                            a.plan_view.drag = Some(Drag::Navigator);
                        }
                    }
                } else {
                    let hover = at(a, p);
                    if hover != a.plan_view.hover {
                        a.plan_view.hide_tip(hwnd);
                        a.plan_view.hover = hover;
                        a.plan_view.hover_since = Instant::now();
                    }
                    a.plan_view.cursor = p;
                }
            });
            unsafe {
                let mut event = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    ..Default::default()
                };
                let _ = TrackMouseEvent(&mut event);
            }
            if dragging {
                sync(hwnd);
            }
        }
        WM_MOUSELEAVE => {
            with_app(|a| {
                a.plan_view.hover = None;
                a.plan_view.hide_tip(hwnd);
            });
        }
        WM_MOUSEWHEEL => {
            with_app(|a| {
                let delta = (w.0 >> 16) as i16 as f64 / 120.;
                a.plan_view.hide_tip(hwnd);
                if unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0 {
                    let mut p = screen_point(l);
                    unsafe {
                        let _ = ScreenToClient(hwnd, &mut p);
                    }
                    zoom_at(a, a.zoom * 1.15_f64.powf(delta), p);
                } else if unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0 {
                    a.origin.x -= delta * 80. / a.zoom;
                    invalidate(hwnd);
                } else {
                    a.origin.y -= delta * 80. / a.zoom;
                    invalidate(hwnd);
                }
            });
            sync(hwnd);
        }
        WM_HSCROLL | WM_VSCROLL => {
            let bar = if msg == WM_HSCROLL { SB_HORZ } else { SB_VERT };
            let mut info = SCROLLINFO {
                cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                fMask: SIF_ALL,
                ..Default::default()
            };
            unsafe {
                let _ = GetScrollInfo(hwnd, bar, &mut info);
            }
            let pos = match (w.0 & 0xffff) as i32 {
                0 => info.nPos - 30,
                1 => info.nPos + 30,
                2 => info.nPos - info.nPage as i32,
                3 => info.nPos + info.nPage as i32,
                4 | 5 => info.nTrackPos,
                6 => 0,
                7 => info.nMax,
                _ => info.nPos,
            };
            with_app(|a| {
                let bounds = a.drawing.as_ref().unwrap().bounds().expand(24.);
                if msg == WM_HSCROLL {
                    a.origin.x = bounds.x + pos.max(0) as f64 / a.zoom;
                } else {
                    a.origin.y = bounds.y + pos.max(0) as f64 / a.zoom;
                }
                a.plan_view.hide_tip(hwnd);
                invalidate(hwnd);
            });
            sync(hwnd);
        }
        WM_KEYDOWN => {
            let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
            match w.0 as u16 {
                k if k == VK_F4.0 => action(hwnd, PROPERTIES),
                k if ctrl && k == b'F' as u16 => action(hwnd, FIND),
                k if ctrl && k == b'0' as u16 => action(hwnd, ZOOM_FIT),
                k if ctrl && matches!(k, 0xbb | 0x6b) => action(hwnd, ZOOM_IN),
                k if ctrl && matches!(k, 0xbd | 0x6d) => action(hwnd, ZOOM_OUT),
                k if k == VK_F3.0 => {
                    with_app(|a| find(a, true));
                    sync(hwnd);
                }
                k if k == VK_TAB.0 => {
                    with_app(|a| {
                        let count = a.plan.as_ref().unwrap().nodes.len();
                        let next = a
                            .plan_view
                            .selected
                            .map(|h| match h {
                                Hit::Node(i) | Hit::Edge(i) => (i + 1) % count,
                            })
                            .unwrap_or(0);
                        a.plan_view.selected = Some(Hit::Node(next));
                        properties(a);
                        invalidate(hwnd);
                    });
                }
                k if [VK_LEFT.0, VK_RIGHT.0, VK_UP.0, VK_DOWN.0].contains(&k) => {
                    with_app(|a| {
                        let step = 50. / a.zoom;
                        match k {
                            0x25 => a.origin.x -= step,
                            0x27 => a.origin.x += step,
                            0x26 => a.origin.y -= step,
                            _ => a.origin.y += step,
                        }
                        invalidate(hwnd);
                    });
                    sync(hwnd);
                }
                _ => return None,
            }
        }
        WM_GETDLGCODE => {
            return Some(LRESULT(
                (DLGC_WANTARROWS | DLGC_WANTTAB | DLGC_WANTCHARS) as isize,
            ));
        }
        _ => return None,
    }
    Some(LRESULT(0))
}

pub(super) fn overlay(a: &App, dc: HDC, r: &RECT) {
    unsafe {
        let Some(doc) = a.drawing.as_ref() else {
            return;
        };
        let transform = |p: Point| POINT {
            x: ((p.x - a.origin.x) * a.zoom) as i32,
            y: ((p.y - a.origin.y) * a.zoom) as i32,
        };
        if let Some(hit) = a.plan_view.selected {
            let pen = CreatePen(PS_DOT, 1, COLORREF(0xc87920));
            let old = SelectObject(dc, pen.into());
            let old_brush = SelectObject(dc, GetStockObject(HOLLOW_BRUSH));
            match hit {
                Hit::Node(i) => {
                    if let Some(n) = doc.nodes.get(i) {
                        let p = transform(Point::new(n.rect.x, n.rect.y));
                        let q =
                            transform(Point::new(n.rect.x + n.rect.w, n.rect.y + n.rect.h - 24.));
                        let labels = visual::labels(a.plan.as_ref().unwrap(), i);
                        let label = RECT {
                            left: p.x,
                            top: p.y + (40. * a.zoom) as i32,
                            right: q.x,
                            bottom: p.y + ((44. + labels.len() as f64 * 14.) * a.zoom) as i32,
                        };
                        let blue = CreateSolidBrush(COLORREF(0xc56d00));
                        FillRect(dc, &label, blue);
                        let _ = DeleteObject(blue.into());
                        let font = CreateFontW(
                            -(11.5 * a.zoom).round().max(1.) as i32,
                            0,
                            0,
                            0,
                            400,
                            0,
                            0,
                            0,
                            DEFAULT_CHARSET,
                            OUT_DEFAULT_PRECIS,
                            CLIP_DEFAULT_PRECIS,
                            CLEARTYPE_QUALITY,
                            FF_MODERN.0 as u32,
                            w!("UDEV Gothic"),
                        );
                        let old_font = SelectObject(dc, font.into());
                        let old_color = SetTextColor(dc, COLORREF(0xffffff));
                        let old_mode = SetBkMode(dc, TRANSPARENT);
                        for (line, text) in labels.into_iter().enumerate() {
                            let mut text = wide(&text);
                            text.pop();
                            let top = p.y + ((41. + line as f64 * 14.) * a.zoom) as i32;
                            let mut rect = RECT {
                                left: p.x,
                                top,
                                right: q.x,
                                bottom: top + (14. * a.zoom).ceil() as i32,
                            };
                            DrawTextW(
                                dc,
                                &mut text,
                                &mut rect,
                                DT_SINGLELINE | DT_CENTER | DT_VCENTER | DT_NOPREFIX,
                            );
                        }
                        SetTextColor(dc, old_color);
                        let _ = SetBkMode(dc, BACKGROUND_MODE(old_mode as u32));
                        SelectObject(dc, old_font);
                        let _ = DeleteObject(font.into());
                    }
                }
                Hit::Edge(i) => {
                    let edge_index = a
                        .plan
                        .as_ref()
                        .unwrap()
                        .nodes
                        .iter()
                        .enumerate()
                        .filter(|(_, n)| n.parent.is_some())
                        .position(|(n, _)| n == i);
                    if let Some(c) = edge_index.and_then(|i| doc.connectors.get(i)) {
                        let mut points = vec![transform(doc.endpoint(c.start))];
                        points.extend(c.waypoints.iter().map(|p| transform(*p)));
                        points.push(transform(doc.endpoint(c.end)));
                        let _ = Polyline(dc, &points);
                    }
                }
            }
            SelectObject(dc, old);
            SelectObject(dc, old_brush);
            let _ = DeleteObject(pen.into());
        }
        let px = |v: i32| v * a.dpi as i32 / 96;
        let plus = RECT {
            left: r.right - px(22),
            top: r.bottom - px(22),
            right: r.right,
            bottom: r.bottom,
        };
        FillRect(dc, &plus, GetSysColorBrush(COLOR_BTNFACE));
        let _ = FrameRect(dc, &plus, GetSysColorBrush(COLOR_3DSHADOW));
        let cx = (plus.left + plus.right) / 2;
        let cy = (plus.top + plus.bottom) / 2;
        let pen = CreatePen(PS_SOLID, px(1).max(1), COLORREF(0x606060));
        let old = SelectObject(dc, pen.into());
        let _ = MoveToEx(dc, cx - px(5), cy, None);
        let _ = LineTo(dc, cx + px(5), cy);
        let _ = MoveToEx(dc, cx, cy - px(5), None);
        let _ = LineTo(dc, cx, cy + px(5));
        if matches!(a.plan_view.drag, Some(Drag::Navigator)) {
            let map = map_rect(r, a.dpi);
            FillRect(dc, &map, GetSysColorBrush(COLOR_WINDOW));
            let _ = FrameRect(dc, &map, GetSysColorBrush(COLOR_3DSHADOW));
            let bounds = doc.bounds().expand(20.);
            let project = |p: Point| POINT {
                x: map.left + ((p.x - bounds.x) / bounds.w * (map.right - map.left) as f64) as i32,
                y: map.top + ((p.y - bounds.y) / bounds.h * (map.bottom - map.top) as f64) as i32,
            };
            let old_brush = SelectObject(dc, GetStockObject(HOLLOW_BRUSH));
            for n in doc.nodes.iter().take(a.plan.as_ref().unwrap().nodes.len()) {
                let p = project(Point::new(n.rect.x, n.rect.y));
                let q = project(Point::new(n.rect.x + n.rect.w, n.rect.y + n.rect.h));
                let _ = Rectangle(dc, p.x, p.y, q.x.max(p.x + 2), q.y.max(p.y + 2));
            }
            let p = project(a.origin);
            let q = project(Point::new(
                a.origin.x + r.right as f64 / a.zoom,
                a.origin.y + r.bottom as f64 / a.zoom,
            ));
            let viewport = RECT {
                left: p.x.max(map.left),
                top: p.y.max(map.top),
                right: q.x.min(map.right),
                bottom: q.y.min(map.bottom),
            };
            let brush = CreateSolidBrush(COLORREF(0xd89b32));
            let _ = FrameRect(dc, &viewport, brush);
            let _ = DeleteObject(brush.into());
            SelectObject(dc, old_brush);
        }
        SelectObject(dc, old);
        let _ = DeleteObject(pen.into());
    }
}

pub(super) fn smoke(directory: &std::path::Path) -> yy_db_core::Result<()> {
    let Some(grid) = with_app(|a| active(a).then_some(a.grid_hwnd)).flatten() else {
        return Ok(());
    };
    sync(grid);
    action(grid, ZOOM_100);
    let before = with_app(|a| a.zoom).unwrap();
    action(grid, ZOOM_IN);
    if !with_app(|a| a.zoom > before).unwrap_or(false) {
        return Err("計画のズーム操作が反映されません".into());
    }
    action(grid, ZOOM_FIT);
    let point = with_app(|a| {
        let node = &a.drawing.as_ref().unwrap().nodes[0];
        POINT {
            x: ((node.rect.center().x - a.origin.x) * a.zoom) as i32,
            y: ((node.rect.y + 18. - a.origin.y) * a.zoom) as i32,
        }
    })
    .unwrap();
    let pack = |p: POINT| LPARAM((p.x as u16 as u32 | ((p.y as u16 as u32) << 16)) as isize);
    handle(grid, WM_LBUTTONDOWN, WPARAM(0), pack(point));
    handle(grid, WM_LBUTTONUP, WPARAM(0), pack(point));
    let props = with_app(|a| unsafe {
        send_message(a.properties, LVM_GETITEMCOUNT, WPARAM(0), LPARAM(0)).0
    })
    .unwrap_or(0);
    if props < 5 {
        return Err("計画のプロパティ一覧が空です".into());
    }
    handle(grid, WM_MOUSEMOVE, WPARAM(0), pack(point));
    with_app(|a| {
        a.plan_view.hover_since = Instant::now() - std::time::Duration::from_millis(500);
    });
    tick();
    if !with_app(|a| a.plan_view.tip_visible).unwrap_or(false) {
        return Err("計画ノードのツールチップが表示されません".into());
    }
    with_app(|a| {
        a.plan_view.hide_tip(grid);
        a.plan_view.hover = None;
    });
    let old = with_app(|a| a.origin).unwrap();
    handle(grid, WM_LBUTTONDOWN, WPARAM(0), LPARAM(0));
    handle(grid, WM_MOUSEMOVE, WPARAM(1), pack(POINT { x: 40, y: 30 }));
    handle(grid, WM_LBUTTONUP, WPARAM(0), pack(POINT { x: 40, y: 30 }));
    if with_app(|a| a.origin == old).unwrap_or(true) {
        return Err("計画のドラッグ移動が反映されません".into());
    }
    action(grid, ZOOM_FIT);
    let rect = client(grid);
    handle(
        grid,
        WM_LBUTTONDOWN,
        WPARAM(0),
        pack(POINT {
            x: rect.right - 3,
            y: rect.bottom - 3,
        }),
    );
    if !with_app(|a| matches!(a.plan_view.drag, Some(Drag::Navigator))).unwrap_or(false) {
        return Err("計画のミニマップが開きません".into());
    }
    unsafe {
        send_message(grid, WM_PAINT, WPARAM(0), LPARAM(0));
    }
    handle(grid, WM_LBUTTONUP, WPARAM(0), LPARAM(0));
    with_app(|a| {
        a.plan_view.search = "sort".into();
        find(a, false);
    });

    with_app(|a|{
        let doc=a.drawing.as_ref().unwrap();let options=yy_db_core::plan::drawing_export(doc);
        std::fs::write(directory.join("ssms-plan.png"),a.renderer.png(doc,&options,1.)?).map_err(|e|e.to_string())?;
        std::fs::write(directory.join("plan-view-smoke.txt"),"Plan view: zoom, fit, selection, property grid, tooltip, drag pan, navigator and search OK\n").map_err(|e|e.to_string())
    }).unwrap_or_else(||Err("計画の画面状態がありません".into()))
}

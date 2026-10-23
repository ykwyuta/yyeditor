use std::{
    cell::RefCell,
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Gdi::*, Imaging::*},
        System::{Com::*, LibraryLoader::*},
        UI::{
            Controls::Dialogs::*,
            Controls::*,
            HiDpi::*,
            Input::KeyboardAndMouse::*,
            Shell::{
                Common::COMDLG_FILTERSPEC, FileOpenDialog, FileSaveDialog, IFileDialog,
                SIGDN_FILESYSPATH,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, PCWSTR, w},
};
use yy3_core::{
    image::{self, Convert, Raster, Sampling},
    mesh, *,
};
#[path = "camera.rs"]
mod camera;
#[path = "drawing.rs"]
mod drawing;
#[path = "preview.rs"]
mod preview;
#[path = "smoke.rs"]
mod smoke;

const FRAME: PCWSTR = w!("YY3DesignerFrame");
const PANE: PCWSTR = w!("YY3DesignerPane");
const HELP_MD: &str = include_str!("../../../help/help.md");
const NEW: u16 = 100;
const OPEN: u16 = 101;
const SAVE: u16 = 102;
const SAVE_AS: u16 = 103;
const IMPORT: u16 = 104;
const EXPORT: u16 = 105;
const EXPORT_ROUND: u16 = 106;
const EXPORT_OBJ: u16 = 107;
const EXPORT_PREVIEW: u16 = 108;
const EXIT: u16 = 109;
const UNDO: u16 = 120;
const REDO: u16 = 121;
const COPY: u16 = 122;
const CUT: u16 = 123;
const PASTE: u16 = 124;
const DELETE: u16 = 125;
const ALL: u16 = 126;
const NONE: u16 = 127;
const FLIP_H: u16 = 128;
const FLIP_V: u16 = 129;
const ROTATE: u16 = 130;
const OBJECT_NEW: u16 = 140;
const OBJECT_DUP: u16 = 141;
const OBJECT_DELETE: u16 = 142;
const OBJECT_RENAME: u16 = 143;
const APPLY_SIZE: u16 = 144;
const SCALE_SIZE: u16 = 145;
const REMOVE_VIEW: u16 = 146;
const RESET_DEPTH: u16 = 147;
const LAYER_ADD: u16 = 150;
const LAYER_DUP: u16 = 151;
const LAYER_DELETE: u16 = 152;
const LAYER_UP: u16 = 153;
const LAYER_DOWN: u16 = 154;
const LAYER_MERGE: u16 = 155;
const LAYER_APPLY: u16 = 156;
const GRID: u16 = 160;
const FIT: u16 = 161;
const ZOOM_IN: u16 = 162;
const ZOOM_OUT: u16 = 163;
const COLOR: u16 = 164;
const APPLY_COLOR: u16 = 165;
const APPLY_MODEL: u16 = 166;
const HELP: u16 = 170;
const PALETTE_NEXT: u16 = 171;
const CAMERA_FIRST: u16 = 240;
const CAMERA_LAST: u16 = 246;
const OBJECT: u16 = 200;
const NAME: u16 = 201;
const WIDTH: u16 = 202;
const HEIGHT: u16 = 203;
const DEPTH: u16 = 204;
const VIEW: u16 = 205;
const LAYER: u16 = 206;
const LAYER_NAME: u16 = 207;
const OPACITY: u16 = 208;
const VISIBLE: u16 = 209;
const TOOL: u16 = 210;
const BRUSH: u16 = 211;
const FILLED: u16 = 212;
const COLOR_TEXT: u16 = 213;
const COLORS: u16 = 214;
const NEAREST: u16 = 215;
const DITHER: u16 = 216;
const ROUND: u16 = 217;
const CUTOFF: u16 = 218;
thread_local! {static APP:RefCell<Option<App>>=const{RefCell::new(None)};}
pub(super) fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}
struct Stroke {
    before: Project,
    start: [i32; 2],
    last: [i32; 2],
    sculpt: Option<sculpt::Stroke>,
}
pub(super) struct App {
    pub hwnd: HWND,
    pub canvas: HWND,
    pub preview: HWND,
    pub panel: HWND,
    pub font: HFONT,
    controls: BTreeMap<u16, HWND>,
    positions: Vec<(u16, i32, i32, i32, i32)>,
    pub editor: Editor,
    path: Option<PathBuf>,
    saved: Project,
    pub direction: Direction,
    pub tool: Tool,
    pub color: Color,
    pub palette_page: usize,
    pub selection: Option<Rect>,
    pub zoom: f32,
    pub pan: [f32; 2],
    pub grid: bool,
    pub depths: Option<Vec<u32>>,
    dpi: i32,
    pub scroll: i32,
    stroke: Option<Stroke>,
    panning: Option<[i32; 2]>,
    orbit: Option<[i32; 2]>,
    yaw: f32,
    pitch: f32,
    model_zoom: f32,
    viewpoint: Option<camera::Preset>,
    worker: preview::Worker,
    snapshot: Arc<Object>,
    pending: Option<preview::Request>,
    generation: u64,
    pub preview_image: Option<Raster>,
    mesh: Option<Arc<mesh::Mesh>>,
    preview_error: Option<String>,
    smoke: Option<PathBuf>,
    smoke_phase: u8,
    smoke_started: std::time::Instant,
    rendered_generation: u64,
}
impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.font.into());
        }
    }
}
impl App {
    pub fn s(&self, n: i32) -> i32 {
        n * self.dpi / 96
    }
    pub fn view(&self) -> Option<&View> {
        self.editor.project.object().views[self.direction.index()].as_ref()
    }
    fn view_mut(&mut self) -> &mut View {
        self.editor.project.object_mut().views[self.direction.index()]
            .as_mut()
            .unwrap()
    }
    fn rect(&self, hwnd: HWND) -> RECT {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut r);
        }
        r
    }
    pub fn origin(&self) -> [f32; 2] {
        let r = self.rect(self.canvas);
        let [w, h] = self.direction.size(self.editor.project.object().size);
        [
            (r.right as f32 - w as f32 * self.zoom) / 2. + self.pan[0],
            (r.bottom as f32 - h as f32 * self.zoom) / 2. + self.pan[1],
        ]
    }
    fn pixel(&self, p: [i32; 2]) -> [i32; 2] {
        let origin = self.origin();
        [
            (p[0] as f32 - origin[0]).div_euclid(self.zoom) as i32,
            (p[1] as f32 - origin[1]).div_euclid(self.zoom) as i32,
        ]
    }
    fn bounded_pixel(&self, p: [i32; 2]) -> [i32; 2] {
        let [w, h] = self.direction.size(self.editor.project.object().size);
        let [x, y] = self.pixel(p);
        [x.clamp(0, w as i32 - 1), y.clamp(0, h as i32 - 1)]
    }
    fn fit(&mut self) {
        let r = self.rect(self.canvas);
        let [w, h] = self.direction.size(self.editor.project.object().size);
        self.zoom = ((r.right - self.s(36)).max(1) as f32 / w as f32)
            .min((r.bottom - self.s(56)).max(1) as f32 / h as f32)
            .clamp(0.25, 64.);
        self.pan = [0.; 2];
        self.invalidate();
    }
    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
            let _ = InvalidateRect(Some(self.canvas), None, false);
            let _ = InvalidateRect(Some(self.preview), None, false);
            let _ = InvalidateRect(Some(self.panel), None, false);
        }
    }
    fn text(&self, id: u16) -> String {
        let hwnd = self.controls[&id];
        unsafe {
            let len = GetWindowTextLengthW(hwnd);
            let mut s = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, &mut s);
            String::from_utf16_lossy(&s[..n as usize])
        }
    }
    fn number(&self, id: u16, min: u32, max: u32) -> io::Result<u32> {
        let n = self
            .text(id)
            .trim()
            .parse::<u32>()
            .map_err(|_| io::Error::other("数値を入力してください"))?;
        if n < min || n > max {
            return Err(io::Error::other(format!("値は{min}〜{max}にしてください")));
        }
        Ok(n)
    }
    fn set_text(&self, id: u16, text: &str) {
        unsafe {
            let _ = SetWindowTextW(self.controls[&id], &HSTRING::from(text));
        }
    }
    fn selected(&self, id: u16) -> usize {
        unsafe {
            SendMessageW(self.controls[&id], CB_GETCURSEL, None, None)
                .0
                .max(0) as usize
        }
    }
    fn checked(&self, id: u16) -> bool {
        unsafe {
            SendMessageW(self.controls[&id], BM_GETCHECK, None, None).0 == BST_CHECKED.0 as isize
        }
    }
    fn check(&self, id: u16, value: bool) {
        unsafe {
            SendMessageW(
                self.controls[&id],
                BM_SETCHECK,
                Some(WPARAM(if value {
                    BST_CHECKED.0
                } else {
                    BST_UNCHECKED.0
                } as usize)),
                None,
            );
        }
    }
    fn status(&self, message: &str) {
        unsafe {
            let _ = SetWindowTextW(self.controls[&900], &HSTRING::from(message));
        }
    }
    fn title(&self) {
        let filename = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "新規プロジェクト".into());
        let mark = if self.editor.project != self.saved {
            " *"
        } else {
            ""
        };
        unsafe {
            let _ = SetWindowTextW(
                self.hwnd,
                &HSTRING::from(format!("{filename}{mark} — YY3Designer")),
            );
        }
    }
    fn selection_or_all(&self) -> Rect {
        self.selection.unwrap_or_else(|| {
            let [w, h] = self.direction.size(self.editor.project.object().size);
            Rect { x: 0, y: 0, w, h }
        })
    }
    fn edit(&mut self, f: impl FnOnce(&mut Project) -> io::Result<()>) -> io::Result<()> {
        self.finish_stroke();
        if self.editor.change(f)? {
            self.changed();
        }
        Ok(())
    }
    fn changed(&mut self) {
        self.refresh_depths();
        self.palette_page = self
            .palette_page
            .min(self.editor.project.palette.len().saturating_sub(1) / 32);
        self.mesh = None;
        self.preview_error = None;
        self.snapshot = Arc::new(self.editor.project.object().clone());
        self.refresh();
        self.request_preview();
        self.invalidate();
        self.title();
    }
    fn refresh(&self) {
        self.set_text(
            PALETTE_NEXT,
            &format!(
                "色 ▶ {}/{}",
                self.palette_page + 1,
                self.editor.project.palette.len().div_ceil(32).max(1)
            ),
        );
        let object = self.editor.project.object();
        self.combo(
            OBJECT,
            self.editor.project.objects.iter().map(|o| o.name.clone()),
            self.editor.project.active,
        );
        self.set_text(NAME, &object.name);
        for (id, n) in [
            (WIDTH, object.size[0]),
            (HEIGHT, object.size[1]),
            (DEPTH, object.size[2]),
            (ROUND, (object.roundness * 100.).round() as u32),
            (CUTOFF, u32::from(object.alpha_cutoff)),
        ] {
            self.set_text(id, &n.to_string());
        }
        self.combo(
            VIEW,
            Direction::ALL.iter().map(|d| {
                format!(
                    "{}{}",
                    d.label(),
                    if object.views[d.index()].is_some() {
                        " ●"
                    } else {
                        "（未使用）"
                    }
                )
            }),
            self.direction.index(),
        );
        if let Some(view) = self.view() {
            self.combo(
                LAYER,
                view.layers
                    .iter()
                    .rev()
                    .map(|l| format!("{}{}", if l.visible { "● " } else { "○ " }, l.name)),
                view.layers.len() - 1 - view.active,
            );
            let layer = &view.layers[view.active];
            self.set_text(LAYER_NAME, &layer.name);
            self.set_text(OPACITY, &layer.opacity.to_string());
            self.check(VISIBLE, layer.visible);
        }
    }
    fn combo(&self, id: u16, items: impl IntoIterator<Item = String>, selected: usize) {
        unsafe {
            let hwnd = self.controls[&id];
            SendMessageW(hwnd, CB_RESETCONTENT, None, None);
            for item in items {
                let text = HSTRING::from(item);
                SendMessageW(
                    hwnd,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(text.as_ptr() as isize)),
                );
            }
            SendMessageW(hwnd, CB_SETCURSEL, Some(WPARAM(selected)), None);
        }
    }
    fn request_preview(&mut self) {
        let r = self.preview_content();
        let height = (r.bottom - r.top).max(1);
        let scale = 640. / r.right.max(height).max(640) as f32;
        self.generation += 1;
        let request = preview::Request {
            id: self.generation,
            object: self.snapshot.clone(),
            width: (r.right as f32 * scale).max(1.) as u32,
            height: (height as f32 * scale).max(1.) as u32,
            yaw: self.yaw,
            pitch: self.pitch,
            zoom: self.model_zoom,
        };
        self.pending = self.worker.submit(request);
    }
    fn camera_buttons(&self) -> Vec<RECT> {
        let width = self.rect(self.preview).right;
        let margin = self.s(6);
        let gap = self.s(4);
        let w = self.s(52);
        let h = self.s(26);
        let columns = ((width - 2 * margin + gap) / (w + gap)).clamp(1, 7) as usize;
        (0..camera::Preset::ALL.len())
            .map(|i| {
                let left = margin + (i % columns) as i32 * (w + gap);
                let top = self.s(30) + (i / columns) as i32 * (h + gap);
                RECT {
                    left,
                    top,
                    right: left + w,
                    bottom: top + h,
                }
            })
            .collect()
    }
    fn preview_content(&self) -> RECT {
        let mut r = self.rect(self.preview);
        r.top =
            (self.camera_buttons().last().unwrap().bottom + self.s(6)).min((r.bottom - 1).max(0));
        r
    }
    fn set_camera(&mut self, preset: camera::Preset) {
        (self.yaw, self.pitch) = preset.angles();
        self.model_zoom = 1.;
        self.orbit = None;
        self.viewpoint = Some(preset);
        self.request_preview();
        self.invalidate();
    }
    fn poll(&mut self) {
        while let Ok(response) = self.worker.receiver.try_recv() {
            if !Arc::ptr_eq(&response.object, &self.snapshot) {
                continue;
            }
            self.rendered_generation = response.id;
            match response.result {
                Ok((mesh, image)) => {
                    self.status(&format!(
                        "{} · {}ボクセル / {}三角形 · 中ボタンで移動 / ホイールでズーム",
                        self.direction.label(),
                        mesh.voxels,
                        mesh.triangles.len()
                    ));
                    self.mesh = Some(mesh);
                    self.preview_image = Some(image);
                    self.preview_error = None;
                }
                Err(e) => {
                    self.preview_image = None;
                    self.status(&e);
                    self.preview_error = Some(e);
                }
            }
            self.invalidate();
        }
        if let Some(request) = self.pending.take() {
            self.pending = self.worker.submit(request);
        }
    }
    fn finish_stroke(&mut self) {
        if let Some(stroke) = self.stroke.take() {
            if let Err(e) = self.editor.project.validate() {
                self.editor.project = stroke.before;
                self.refresh_depths();
                self.invalidate();
                self.status(&e.to_string());
            } else if self.editor.commit(stroke.before) {
                self.changed();
            }
        }
    }
    fn refresh_depths(&mut self) {
        self.depths = if matches!(self.tool, Tool::Thin | Tool::RestoreDepth) {
            match sculpt::depths(self.editor.project.object(), self.direction) {
                Ok(d) => Some(d),
                Err(e) => {
                    self.status(&e.to_string());
                    None
                }
            }
        } else {
            None
        };
    }
    fn set_tool(&mut self, tool: Tool) {
        self.finish_stroke();
        self.tool = tool;
        self.refresh_depths();
        self.invalidate();
    }
    fn depth_status(&self, point: [i32; 2]) {
        if let Some(depths) = &self.depths {
            let [x, y] = self.pixel(point);
            let [w, h] = self.direction.size(self.editor.project.object().size);
            if x >= 0 && y >= 0 && x < w as i32 && y < h as i32 {
                self.status(&format!(
                    "{} ({x},{y}) 残りの厚み {} / {} · ひと筆で1段削る / Uで元に戻す",
                    self.direction.label(),
                    depths[(y as u32 * w + x as u32) as usize],
                    self.direction.ray_length(self.editor.project.object().size)
                ));
            }
        }
    }
    fn switch_direction(&mut self, direction: Direction) -> io::Result<()> {
        self.finish_stroke();
        if self.editor.project.object().views[direction.index()].is_none() {
            let mut source = self.editor.project.object().clone();
            source.carved.clear();
            let view = mesh::projected_view(&source, direction)?;
            self.edit(|p| {
                p.object_mut().views[direction.index()] = Some(view);
                Ok(())
            })?;
        }
        self.direction = direction;
        self.refresh_depths();
        self.selection = None;
        self.refresh();
        self.fit();
        Ok(())
    }
    fn choose_object(&mut self, index: usize) {
        self.finish_stroke();
        self.editor.project.active = index.min(self.editor.project.objects.len() - 1);
        self.direction = Direction::ALL
            .into_iter()
            .find(|d| self.editor.project.object().views[d.index()].is_some())
            .unwrap();
        self.selection = None;
        self.changed();
        self.fit();
    }
    fn layout(&mut self) {
        unsafe {
            let r = self.rect(self.hwnd);
            let header_h = self.s(53);
            let status_h = self.s(25);
            let sidebar = self.s(224);
            let gap = self.s(8);
            let content_top = header_h;
            let available = (r.right - sidebar - gap * 3).max(2);
            let height = (r.bottom - content_top - status_h - gap).max(2);
            let stacked = available < self.s(650);
            let left = if stacked {
                available
            } else {
                available * 55 / 100
            };
            let edit_h = if stacked { height * 58 / 100 } else { height };
            let _ = MoveWindow(
                self.panel,
                0,
                0,
                sidebar,
                (r.bottom - status_h).max(1),
                true,
            );
            let _ = MoveWindow(self.canvas, sidebar + gap, content_top, left, edit_h, true);
            let (px, py, pw, ph) = if stacked {
                (
                    sidebar + gap,
                    content_top + edit_h + gap,
                    available,
                    (height - edit_h - gap).max(1),
                )
            } else {
                (
                    sidebar + gap * 2 + left,
                    content_top,
                    (available - left - gap).max(1),
                    height,
                )
            };
            let _ = MoveWindow(self.preview, px, py, pw, ph, true);
            for (i, r) in self.camera_buttons().iter().enumerate() {
                let _ = MoveWindow(
                    self.controls[&(CAMERA_FIRST + i as u16)],
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    true,
                );
            }
            let _ = MoveWindow(
                self.controls[&900],
                0,
                r.bottom - status_h,
                r.right,
                status_h,
                true,
            );
            let scroll_max = (self.s(904) - (r.bottom - status_h)).max(0);
            self.scroll = self.scroll.clamp(0, scroll_max);
            let info = SCROLLINFO {
                cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
                nMin: 0,
                nMax: self.s(904),
                nPage: (r.bottom - status_h).max(1) as u32,
                nPos: self.scroll,
                ..Default::default()
            };
            SetScrollInfo(self.panel, SB_VERT, &info, true);
            for &(id, x, y, w, h) in &self.positions {
                let hwnd = self.controls[&id];
                let _ = MoveWindow(
                    hwnd,
                    self.s(x),
                    self.s(y - 40) - self.scroll,
                    self.s(w),
                    self.s(h),
                    true,
                );
            }
            let _ = MoveWindow(
                self.controls[&PALETTE_NEXT],
                self.s(558),
                self.s(10),
                self.s(92),
                self.s(28),
                true,
            );
            self.request_preview();
            self.invalidate();
        }
    }
    fn scroll_panel(&mut self, delta: i32) {
        self.scroll = (self.scroll + delta).max(0);
        self.layout();
    }
    fn color_text(&self) {
        self.set_text(
            COLOR_TEXT,
            &self
                .color
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<String>(),
        );
    }
}

unsafe fn control(
    parent: HWND,
    font: HFONT,
    class: PCWSTR,
    label: &str,
    id: u16,
    style: WINDOW_STYLE,
) -> windows::core::Result<HWND> {
    unsafe {
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            &HSTRING::from(label),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | style,
            0,
            0,
            1,
            1,
            Some(parent),
            Some(HMENU(std::ptr::without_provenance_mut(id as usize))),
            None,
            None,
        )?;
        SendMessageW(
            hwnd,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(0)),
        );
        Ok(hwnd)
    }
}
fn make_font(dpi: i32) -> HFONT {
    yy_ui_win::font::register_gdi();
    let (config, _) = yy_config::Config::load();
    unsafe {
        CreateFontW(
            -(config.editor.font_size.max(8.) * dpi as f32 / 72.).round() as i32,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32,
            &HSTRING::from(config.editor.font_family),
        )
    }
}
unsafe fn menu() -> windows::core::Result<HMENU> {
    unsafe {
        let bar = CreateMenu()?;
        for (name, items) in [
            (
                "ファイル(&F)",
                vec![
                    (NEW, "新規\tCtrl+N"),
                    (OPEN, "開く…\tCtrl+O"),
                    (SAVE, "保存\tCtrl+S"),
                    (SAVE_AS, "名前を付けて保存…"),
                    (IMPORT, "写真・イラストをドット絵へ…"),
                    (EXPORT, "現在の方向をPNGへ…"),
                    (EXPORT_ROUND, "丸み付きドットをPNGへ…"),
                    (EXPORT_OBJ, "立体をOBJ + MTLへ…"),
                    (EXPORT_PREVIEW, "3DプレビューをPNGへ…"),
                    (EXIT, "終了"),
                ],
            ),
            (
                "編集(&E)",
                vec![
                    (UNDO, "元に戻す\tCtrl+Z"),
                    (REDO, "やり直す\tCtrl+Y"),
                    (CUT, "切り取り\tCtrl+X"),
                    (COPY, "コピー\tCtrl+C"),
                    (PASTE, "貼り付け\tCtrl+V"),
                    (DELETE, "選択範囲を消去\tDelete"),
                    (ALL, "すべて選択\tCtrl+A"),
                    (NONE, "選択を解除\tEsc"),
                    (FLIP_H, "左右反転"),
                    (FLIP_V, "上下反転"),
                    (ROTATE, "右へ90度回転"),
                ],
            ),
            (
                "オブジェクト(&O)",
                vec![
                    (OBJECT_NEW, "追加"),
                    (OBJECT_DUP, "複製"),
                    (OBJECT_DELETE, "削除"),
                    (OBJECT_RENAME, "名前を適用"),
                    (REMOVE_VIEW, "現在の方向を立体から外す"),
                    (RESET_DEPTH, "削った厚みをすべて戻す"),
                ],
            ),
            (
                "レイヤー(&L)",
                vec![
                    (LAYER_ADD, "追加"),
                    (LAYER_DUP, "複製"),
                    (LAYER_DELETE, "削除"),
                    (LAYER_UP, "上へ移動"),
                    (LAYER_DOWN, "下へ移動"),
                    (LAYER_MERGE, "下のレイヤーと結合"),
                    (LAYER_APPLY, "名前・不透明度・表示を適用"),
                ],
            ),
            (
                "表示(&V)",
                vec![
                    (GRID, "グリッド表示切替\tG"),
                    (FIT, "全体を表示\tF"),
                    (ZOOM_IN, "拡大\t+"),
                    (ZOOM_OUT, "縮小\t-"),
                    (COLOR, "色を選択…"),
                ],
            ),
            ("ヘルプ(&H)", vec![(HELP, "YY3Designer ヘルプ\tF1")]),
        ] {
            let sub = CreatePopupMenu()?;
            for (id, label) in items {
                AppendMenuW(sub, MF_STRING, id as usize, &HSTRING::from(label))?;
            }
            AppendMenuW(bar, MF_POPUP, sub.0 as usize, &HSTRING::from(name))?;
        }
        Ok(bar)
    }
}

pub fn run(args: &[std::ffi::OsString]) -> Result<(), String> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        let result = start(args);
        CoUninitialize();
        result
    }
}
unsafe fn start(args: &[std::ffi::OsString]) -> Result<(), String> {
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let icon = LoadIconW(
            Some(instance.into()),
            PCWSTR(std::ptr::without_provenance(1)),
        )
        .unwrap_or_default();
        for (class, proc) in [
            (
                FRAME,
                frame_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
            ),
            (PANE, pane_proc),
        ] {
            RegisterClassW(&WNDCLASSW {
                hInstance: instance.into(),
                lpszClassName: class,
                lpfnWndProc: Some(proc),
                hIcon: icon,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            });
        }
        let dpi = GetDpiForSystem().max(96) as i32;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            FRAME,
            w!("YY3Designer"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1280 * dpi / 96,
            860 * dpi / 96,
            None,
            Some(menu().map_err(|e| e.to_string())?),
            Some(instance.into()),
            None,
        )
        .map_err(|e| e.to_string())?;
        let font = make_font(dpi);
        let mut controls = BTreeMap::new();
        let mut positions = vec![];
        let panel = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PANE,
            w!("設定"),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_CLIPCHILDREN,
            0,
            0,
            1,
            1,
            Some(hwnd),
            None,
            None,
            None,
        )
        .map_err(|e| e.to_string())?;
        SetWindowLongPtrW(panel, GWLP_USERDATA, 3);
        let canvas = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            PANE,
            w!("ドット絵"),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            1,
            1,
            Some(hwnd),
            None,
            None,
            None,
        )
        .map_err(|e| e.to_string())?;
        SetWindowLongPtrW(canvas, GWLP_USERDATA, 1);
        let preview = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            PANE,
            w!("3D"),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
            0,
            0,
            1,
            1,
            Some(hwnd),
            None,
            None,
            None,
        )
        .map_err(|e| e.to_string())?;
        SetWindowLongPtrW(preview, GWLP_USERDATA, 2);
        let edit = WINDOW_STYLE(ES_AUTOHSCROLL as u32) | WS_BORDER;
        let combo = WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_HASSTRINGS) as u32) | WS_VSCROLL;
        let button = WINDOW_STYLE(BS_PUSHBUTTON as u32);
        let check = WINDOW_STYLE(BS_AUTOCHECKBOX as u32);
        for (i, preset) in camera::Preset::ALL.iter().enumerate() {
            let id = CAMERA_FIRST + i as u16;
            controls.insert(
                id,
                control(preview, font, w!("BUTTON"), preset.label(), id, button)
                    .map_err(|e| e.to_string())?,
            );
        }
        let specs = [
            (OBJECT, "", w!("COMBOBOX"), combo, 12, 66, 190, 240),
            (NAME, "", w!("EDIT"), edit, 12, 122, 132, 26),
            (
                OBJECT_RENAME,
                "適用",
                w!("BUTTON"),
                button,
                148,
                122,
                54,
                26,
            ),
            (WIDTH, "32", w!("EDIT"), edit, 12, 175, 59, 26),
            (HEIGHT, "32", w!("EDIT"), edit, 77, 175, 59, 26),
            (DEPTH, "8", w!("EDIT"), edit, 142, 175, 60, 26),
            (
                APPLY_SIZE,
                "寸法変更",
                w!("BUTTON"),
                button,
                12,
                204,
                94,
                26,
            ),
            (
                SCALE_SIZE,
                "画像も拡縮",
                w!("BUTTON"),
                button,
                110,
                204,
                92,
                26,
            ),
            (VIEW, "", w!("COMBOBOX"), combo, 12, 254, 190, 200),
            (
                REMOVE_VIEW,
                "この方向を外す",
                w!("BUTTON"),
                button,
                12,
                282,
                190,
                26,
            ),
            (LAYER, "", w!("COMBOBOX"), combo, 12, 334, 190, 240),
            (LAYER_ADD, "＋", w!("BUTTON"), button, 12, 364, 42, 26),
            (LAYER_DUP, "複製", w!("BUTTON"), button, 58, 364, 66, 26),
            (LAYER_DELETE, "削除", w!("BUTTON"), button, 128, 364, 74, 26),
            (LAYER_NAME, "", w!("EDIT"), edit, 12, 394, 190, 24),
            (OPACITY, "255", w!("EDIT"), edit, 12, 440, 48, 26),
            (VISIBLE, "表示", w!("BUTTON"), check, 66, 440, 62, 26),
            (LAYER_APPLY, "適用", w!("BUTTON"), button, 132, 440, 70, 26),
            (TOOL, "", w!("COMBOBOX"), combo, 12, 496, 190, 260),
            (BRUSH, "1", w!("EDIT"), edit, 12, 548, 48, 26),
            (FILLED, "図形を塗る", w!("BUTTON"), check, 68, 548, 134, 26),
            (COLOR_TEXT, "172033FF", w!("EDIT"), edit, 12, 600, 125, 26),
            (APPLY_COLOR, "適用", w!("BUTTON"), button, 142, 600, 60, 26),
            (
                COLOR,
                "カラーパレット…",
                w!("BUTTON"),
                button,
                12,
                628,
                190,
                26,
            ),
            (COLORS, "", w!("COMBOBOX"), combo, 12, 682, 190, 220),
            (
                NEAREST,
                "最近傍（ドット保持）",
                w!("BUTTON"),
                check,
                12,
                710,
                190,
                23,
            ),
            (
                DITHER,
                "ディザリング",
                w!("BUTTON"),
                check,
                12,
                734,
                190,
                23,
            ),
            (ROUND, "0", w!("EDIT"), edit, 12, 788, 82, 26),
            (CUTOFF, "128", w!("EDIT"), edit, 100, 788, 102, 26),
            (
                APPLY_MODEL,
                "丸み・αを適用",
                w!("BUTTON"),
                button,
                12,
                818,
                190,
                26,
            ),
        ];
        for (id, label, class, style, x, y, w, h) in specs {
            let child = control(panel, font, class, label, id, style).map_err(|e| e.to_string())?;
            controls.insert(id, child);
            positions.push((id, x, y, w, h));
        }
        controls.insert(
            900,
            control(
                hwnd,
                font,
                w!("STATIC"),
                "鉛筆で描画 · 中ボタンで移動 · ホイールでズーム",
                900,
                WINDOW_STYLE(0),
            )
            .map_err(|e| e.to_string())?,
        );
        controls.insert(
            PALETTE_NEXT,
            control(hwnd, font, w!("BUTTON"), "色 ▶", PALETTE_NEXT, button)
                .map_err(|e| e.to_string())?,
        );
        let project = Project::default();
        let snapshot = Arc::new(project.object().clone());
        let smoke = args
            .first()
            .is_some_and(|s| s == "--ui-smoke-test")
            .then(|| {
                args.get(1)
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("yy3-smoke"))
            });
        let app = App {
            hwnd,
            canvas,
            preview,
            panel,
            font,
            controls,
            positions,
            editor: Editor::new(project.clone()),
            path: None,
            saved: project,
            direction: Direction::Front,
            tool: Tool::Pencil,
            color: [23, 32, 51, 255],
            palette_page: 0,
            selection: None,
            zoom: 12.,
            pan: [0.; 2],
            grid: true,
            depths: None,
            dpi,
            scroll: 0,
            stroke: None,
            panning: None,
            orbit: None,
            yaw: camera::Preset::Isometric.angles().0,
            pitch: camera::Preset::Isometric.angles().1,
            model_zoom: 1.,
            viewpoint: Some(camera::Preset::Isometric),
            worker: preview::Worker::new(),
            snapshot,
            pending: None,
            generation: 0,
            preview_image: None,
            mesh: None,
            preview_error: None,
            smoke,
            smoke_phase: 0,
            smoke_started: std::time::Instant::now(),
            rendered_generation: 0,
        };
        APP.with(|a| *a.borrow_mut() = Some(app));
        with(|a| {
            a.combo(TOOL, Tool::ALL.iter().map(|t| t.label().to_owned()), 0);
            a.combo(
                COLORS,
                [
                    "元の色（減色なし）",
                    "8色",
                    "16色",
                    "32色",
                    "64色",
                    "128色",
                    "256色",
                ]
                .map(str::to_owned),
                3,
            );
            a.refresh();
            a.layout();
            a.fit();
            a.title();
        });
        SetTimer(Some(hwnd), 1, 30, None);
        let _ = ShowWindow(hwnd, SW_SHOW);
        if args.first().is_some_and(|s| s != "--ui-smoke-test") {
            let path = PathBuf::from(&args[0]);
            match yy3_core::load(&path) {
                Ok(p) => {
                    with(|a| {
                        a.editor = Editor::new(p.clone());
                        a.saved = p;
                        a.path = Some(path);
                        a.choose_object(a.editor.project.active);
                    });
                }
                Err(e) => error(hwnd, &e.to_string()),
            }
        }
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if handle_key(&message) {
                continue;
            }
            if IsDialogMessageW(hwnd, &message).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        APP.with(|a| a.borrow_mut().take());
        if message.wParam.0 != 0 {
            return Err("YY3Designer UI smoke failed; see ui-smoke.txt".into());
        }
        Ok(())
    }
}
fn error(hwnd: HWND, message: &str) {
    unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(message),
            w!("YY3Designer"),
            MB_OK | MB_ICONERROR,
        );
    }
}
fn io_error(error: windows::core::Error) -> io::Error {
    io::Error::other(error.to_string())
}
unsafe fn dialog(
    hwnd: HWND,
    save: bool,
    extension: &str,
    description: &str,
) -> io::Result<Option<PathBuf>> {
    unsafe {
        let dialog: IFileDialog = CoCreateInstance(
            if save {
                &FileSaveDialog
            } else {
                &FileOpenDialog
            },
            None,
            CLSCTX_INPROC_SERVER,
        )
        .map_err(io_error)?;
        let pattern = HSTRING::from(if extension == "image" {
            "*.png;*.jpg;*.jpeg;*.bmp;*.gif;*.tif;*.tiff".into()
        } else {
            format!("*.{extension}")
        });
        let desc = HSTRING::from(description);
        let any = HSTRING::from("すべてのファイル");
        let star = HSTRING::from("*.*");
        dialog
            .SetFileTypes(&[
                COMDLG_FILTERSPEC {
                    pszName: PCWSTR(desc.as_ptr()),
                    pszSpec: PCWSTR(pattern.as_ptr()),
                },
                COMDLG_FILTERSPEC {
                    pszName: PCWSTR(any.as_ptr()),
                    pszSpec: PCWSTR(star.as_ptr()),
                },
            ])
            .map_err(io_error)?;
        if save {
            dialog
                .SetDefaultExtension(&HSTRING::from(extension))
                .map_err(io_error)?;
        }
        if let Err(e) = dialog.Show(Some(hwnd)) {
            if e.code().0 as u32 == 0x800704c7 {
                return Ok(None);
            }
            return Err(io_error(e));
        }
        let item = dialog.GetResult().map_err(io_error)?;
        let path = item.GetDisplayName(SIGDN_FILESYSPATH).map_err(io_error)?;
        let result = PathBuf::from(path.to_string().map_err(io::Error::other)?);
        CoTaskMemFree(Some(path.0.cast()));
        Ok(Some(result))
    }
}
fn confirm_discard() -> bool {
    let Some((hwnd, dirty)) = with(|a| {
        a.finish_stroke();
        (a.hwnd, a.editor.project != a.saved)
    }) else {
        return false;
    };
    if !dirty {
        return true;
    }
    let response = unsafe {
        MessageBoxW(
            Some(hwnd),
            w!("変更を保存しますか？"),
            w!("YY3Designer"),
            MB_YESNOCANCEL | MB_ICONQUESTION,
        )
    };
    if response == IDNO {
        return true;
    }
    if response == IDYES {
        return save_project(false).unwrap_or(false);
    }
    false
}
fn save_project(save_as: bool) -> io::Result<bool> {
    let (hwnd, path) = with(|a| {
        a.finish_stroke();
        (a.hwnd, if save_as { None } else { a.path.clone() })
    })
    .unwrap();
    let path = if let Some(p) = path {
        Some(p)
    } else {
        unsafe { dialog(hwnd, true, "yy3d", "YY3Designer プロジェクト")? }
    };
    let Some(path) = path else { return Ok(false) };
    with(|a| {
        yy3_core::save(&path, &a.editor.project)?;
        a.path = Some(path);
        a.saved = a.editor.project.clone();
        a.title();
        a.status("プロジェクトを保存しました");
        Ok(true)
    })
    .unwrap()
}
fn read_picture(path: &Path) -> io::Result<Raster> {
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(io_error)?;
        let decoder = factory
            .CreateDecoderFromFilename(
                &HSTRING::from(path.as_os_str()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(io_error)?;
        let frame = decoder.GetFrame(0).map_err(io_error)?;
        let (mut w, mut h) = (0, 0);
        frame.GetSize(&mut w, &mut h).map_err(io_error)?;
        if w == 0 || h == 0 || w > 16384 || h > 16384 || u64::from(w) * u64::from(h) > 40_000_000 {
            return Err(io::Error::other(
                "画像は4,000万画素・各辺16,384以下にしてください",
            ));
        }
        let converter = factory.CreateFormatConverter().map_err(io_error)?;
        converter
            .Initialize(
                &frame,
                &GUID_WICPixelFormat32bppRGBA,
                WICBitmapDitherTypeNone,
                None,
                0.,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(io_error)?;
        let mut bytes = vec![0; (w * h * 4) as usize];
        converter
            .CopyPixels(std::ptr::null(), w * 4, &mut bytes)
            .map_err(io_error)?;
        Ok(Raster {
            width: w,
            height: h,
            pixels: bytes.as_chunks::<4>().0.to_vec(),
        })
    }
}
fn choose_color() -> io::Result<()> {
    let (hwnd, color) = with(|a| (a.hwnd, a.color)).unwrap();
    let mut custom = [COLORREF(0); 16];
    let mut config = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: hwnd,
        rgbResult: drawing::rgb(color),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_FULLOPEN | CC_RGBINIT,
        ..Default::default()
    };
    unsafe {
        if ChooseColorW(&mut config).as_bool() {
            with(|a| {
                let n = config.rgbResult.0;
                a.color = [n as u8, (n >> 8) as u8, (n >> 16) as u8, color[3]];
                a.color_text();
                a.invalidate();
            });
        }
    }
    Ok(())
}

fn dispatch(id: u16, notification: u16) -> io::Result<()> {
    let hwnd = with(|a| a.hwnd).unwrap();
    match id {
        NEW => {
            if confirm_discard() {
                with(|a| {
                    let p = Project::default();
                    a.editor = Editor::new(p.clone());
                    a.saved = p;
                    a.path = None;
                    a.choose_object(0);
                });
            }
        }
        OPEN => {
            if confirm_discard()
                && let Some(path) =
                    unsafe { dialog(hwnd, false, "yy3d", "YY3Designer プロジェクト")? }
            {
                let p = yy3_core::load(&path)?;
                with(|a| {
                    a.editor = Editor::new(p.clone());
                    a.saved = p;
                    a.path = Some(path);
                    a.choose_object(a.editor.project.active);
                });
            }
        }
        SAVE | SAVE_AS => {
            save_project(id == SAVE_AS)?;
        }
        EXIT => unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        HELP => {
            yy_ui_win::help::show(HELP_MD, "YY3Designer ヘルプ", None, "yy3designer-help")
                .map_err(io_error)?;
        }
        COLOR => {
            choose_color()?;
        }
        IMPORT => {
            if let Some(path) = unsafe { dialog(hwnd, false, "image", "写真・イラスト")? } {
                let source = read_picture(&path)?;
                let (direction, size, options) = with(|a| {
                    (
                        a.direction,
                        a.direction.size(a.editor.project.object().size),
                        Convert {
                            sampling: if a.checked(NEAREST) {
                                Sampling::Nearest
                            } else {
                                Sampling::Area
                            },
                            colors: [0, 8, 16, 32, 64, 128, 256][a.selected(COLORS).min(6)],
                            dither: a.checked(DITHER),
                        },
                    )
                })
                .unwrap();
                let (pixels, palette) = image::convert(&source, size[0], size[1], &options)?;
                with(|a| {
                    a.edit(|p| {
                        let v = p.object_mut().enable(direction);
                        if v.layers.len() >= 32 {
                            return Err(io::Error::other("レイヤーは最大32枚です"));
                        }
                        v.layers.push(Layer {
                            name: path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                            visible: true,
                            opacity: 255,
                            pixels,
                        });
                        v.active = v.layers.len() - 1;
                        if !palette.is_empty() {
                            p.palette = palette;
                        }
                        Ok(())
                    })
                })
                .unwrap()?;
            }
        }
        EXPORT | EXPORT_ROUND | EXPORT_OBJ | EXPORT_PREVIEW => {
            let ext = if id == EXPORT_OBJ { "obj" } else { "png" };
            if let Some(path) = unsafe {
                dialog(
                    hwnd,
                    true,
                    ext,
                    if id == EXPORT_OBJ {
                        "3Dモデル (OBJ + MTL)"
                    } else {
                        "PNG画像"
                    },
                )?
            } {
                let result = with(|a| {
                    a.finish_stroke();
                    if id == EXPORT_OBJ {
                        let mesh = mesh::build(a.editor.project.object())?;
                        mesh::export_obj(&path, &mesh)
                    } else {
                        let image = if id == EXPORT_PREVIEW {
                            let mesh = mesh::build(a.editor.project.object())?;
                            mesh::render(&mesh, 1200, 1000, a.yaw, a.pitch, a.model_zoom)?
                        } else {
                            image::export_view(
                                a.view().unwrap(),
                                if id == EXPORT_ROUND { 8 } else { 1 },
                                if id == EXPORT_ROUND {
                                    a.editor.project.object().roundness
                                } else {
                                    0.
                                },
                            )?
                        };
                        image::write_png(&path, &image)
                    }
                })
                .unwrap();
                result?;
                with(|a| a.status("エクスポートしました"));
            }
        }
        COPY | CUT => {
            let clip = with(|a| Clip::capture(a.view().unwrap(), a.selection_or_all())).unwrap();
            let bytes = clip_bytes(&clip)?;
            let format = yy_ui_win::clipboard::named_format("YY3Designer.Pixels.v1");
            if !yy_ui_win::clipboard::set_formats(hwnd, &[(format, bytes)]) {
                return Err(io::Error::other("クリップボードにコピーできませんでした"));
            }
            if id == CUT {
                return dispatch(DELETE, 0);
            }
        }
        PASTE => {
            let format = yy_ui_win::clipboard::named_format("YY3Designer.Pixels.v1");
            let bytes = yy_ui_win::clipboard::get_data(
                hwnd,
                format,
                4 * MAX_SIDE as usize * MAX_SIDE as usize + 8,
            )
            .ok_or_else(|| {
                io::Error::other("YY3Designerの範囲をコピーしてから貼り付けてください")
            })?;
            let clip = parse_clip(&bytes)?;
            with(|a| {
                let d = a.direction;
                let at = a.selection.map(|r| [r.x, r.y]).unwrap_or([0, 0]);
                a.edit(|p| {
                    clip.paste(p.object_mut().enable(d), at);
                    Ok(())
                })
            })
            .unwrap()?;
        }
        _ => {
            with(|a| local_command(a, id, notification)).unwrap()?;
        }
    }
    Ok(())
}
fn clip_bytes(clip: &Clip) -> io::Result<Vec<u8>> {
    clip.validate()?;
    let mut b = clip.width.to_le_bytes().to_vec();
    b.extend(clip.height.to_le_bytes());
    b.extend(clip.pixels.iter().flatten().copied());
    Ok(b)
}
fn parse_clip(b: &[u8]) -> io::Result<Clip> {
    if b.len() < 8 {
        return Err(io::Error::other("クリップボードが不正です"));
    }
    let width = u32::from_le_bytes(b[..4].try_into().unwrap());
    let height = u32::from_le_bytes(b[4..8].try_into().unwrap());
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(io::Error::other("クリップボードの寸法が不正です"));
    }
    let expected = 8u64 + u64::from(width) * u64::from(height) * 4;
    if expected > b.len() as u64 || expected > 4 * 256 * 256 + 8 {
        return Err(io::Error::other("クリップボードが不正です"));
    }
    let clip = Clip {
        width,
        height,
        pixels: b[8..expected as usize].as_chunks::<4>().0.to_vec(),
    };
    clip.validate()?;
    Ok(clip)
}
fn local_command(a: &mut App, id: u16, notification: u16) -> io::Result<()> {
    let d = a.direction;
    let r = a.selection_or_all();
    match id {
        CAMERA_FIRST..=CAMERA_LAST => {
            a.set_camera(camera::Preset::ALL[(id - CAMERA_FIRST) as usize]);
        }
        PALETTE_NEXT => {
            a.palette_page =
                (a.palette_page + 1) % a.editor.project.palette.len().div_ceil(32).max(1);
            a.refresh();
            a.invalidate();
        }
        OBJECT if notification == CBN_SELCHANGE as u16 => {
            a.choose_object(a.selected(OBJECT));
        }
        VIEW if notification == CBN_SELCHANGE as u16 => {
            a.switch_direction(Direction::ALL[a.selected(VIEW).min(5)])?;
        }
        LAYER if notification == CBN_SELCHANGE as u16 => {
            let selected = a.selected(LAYER);
            a.finish_stroke();
            let v = a.view_mut();
            v.active = v.layers.len() - 1 - selected.min(v.layers.len() - 1);
            a.refresh();
        }
        TOOL if notification == CBN_SELCHANGE as u16 => {
            a.set_tool(Tool::ALL[a.selected(TOOL).min(Tool::ALL.len() - 1)]);
        }
        UNDO | REDO => {
            a.finish_stroke();
            let changed = if id == UNDO {
                a.editor.undo()
            } else {
                a.editor.redo()
            };
            if changed {
                if a.view().is_none() {
                    a.direction = Direction::ALL
                        .into_iter()
                        .find(|d| a.editor.project.object().views[d.index()].is_some())
                        .unwrap();
                }
                a.selection = None;
                a.changed();
            }
        }
        OBJECT_NEW | OBJECT_DUP => {
            a.edit(|p| {
                let mut object = if id == OBJECT_NEW {
                    Object::new("", p.object().size)?
                } else {
                    p.object().clone()
                };
                object.name = format!("オブジェクト {}", p.objects.len() + 1);
                p.objects.push(object);
                p.active = p.objects.len() - 1;
                Ok(())
            })?;
            a.choose_object(a.editor.project.active);
        }
        OBJECT_DELETE => {
            a.edit(|p| {
                if p.objects.len() <= 1 {
                    return Err(io::Error::other("最後のオブジェクトは削除できません"));
                }
                p.objects.remove(p.active);
                p.active = p.active.min(p.objects.len() - 1);
                Ok(())
            })?;
            a.choose_object(a.editor.project.active);
        }
        OBJECT_RENAME => {
            let name = a.text(NAME);
            a.edit(|p| {
                p.object_mut().name = name;
                Ok(())
            })?;
        }
        APPLY_SIZE | SCALE_SIZE => {
            let size = [
                a.number(WIDTH, 1, 256)?,
                a.number(HEIGHT, 1, 256)?,
                a.number(DEPTH, 1, 256)?,
            ];
            a.edit(|p| p.object_mut().resize(size, id == SCALE_SIZE))?;
            a.selection = None;
            a.fit();
        }
        APPLY_MODEL => {
            let round = a.number(ROUND, 0, 100)? as f32 / 100.;
            let cutoff = a.number(CUTOFF, 1, 255)? as u8;
            a.edit(|p| {
                p.object_mut().roundness = round;
                p.object_mut().alpha_cutoff = cutoff;
                Ok(())
            })?;
        }
        REMOVE_VIEW => {
            a.edit(|p| {
                if p.object().views.iter().flatten().count() <= 1 {
                    return Err(io::Error::other("少なくとも1方向は残してください"));
                }
                p.object_mut().views[d.index()] = None;
                Ok(())
            })?;
            a.direction = Direction::ALL
                .into_iter()
                .find(|d| a.editor.project.object().views[d.index()].is_some())
                .unwrap();
            a.selection = None;
            a.refresh();
            a.fit();
        }
        RESET_DEPTH => {
            a.edit(|p| {
                p.object_mut().carved.clear();
                Ok(())
            })?;
        }
        LAYER_ADD | LAYER_DUP => {
            a.edit(|p| {
                let v = p.object_mut().enable(d);
                if v.layers.len() >= 32 {
                    return Err(io::Error::other("レイヤーは最大32枚です"));
                }
                let layer = if id == LAYER_DUP {
                    v.layers[v.active].clone()
                } else {
                    Layer {
                        name: format!("レイヤー {}", v.layers.len() + 1),
                        visible: true,
                        opacity: 255,
                        pixels: vec![CLEAR; (v.width * v.height) as usize],
                    }
                };
                v.active += 1;
                v.layers.insert(v.active, layer);
                Ok(())
            })?;
        }
        LAYER_DELETE => {
            a.edit(|p| {
                let v = p.object_mut().enable(d);
                if v.layers.len() <= 1 {
                    return Err(io::Error::other("最後のレイヤーは削除できません"));
                }
                v.layers.remove(v.active);
                v.active = v.active.min(v.layers.len() - 1);
                Ok(())
            })?;
        }
        LAYER_UP | LAYER_DOWN => {
            a.edit(|p| {
                let v = p.object_mut().enable(d);
                let next = if id == LAYER_UP {
                    (v.active + 1).min(v.layers.len() - 1)
                } else {
                    v.active.saturating_sub(1)
                };
                v.layers.swap(v.active, next);
                v.active = next;
                Ok(())
            })?;
        }
        LAYER_MERGE => {
            a.edit(|p| {
                let v = p.object_mut().enable(d);
                if v.active == 0 {
                    return Err(io::Error::other("下のレイヤーがありません"));
                }
                let top = v.layers.remove(v.active);
                v.active -= 1;
                let below = &mut v.layers[v.active];
                for (dst, src) in below.pixels.iter_mut().zip(top.pixels) {
                    let base = if below.visible {
                        over(CLEAR, *dst, below.opacity)
                    } else {
                        CLEAR
                    };
                    *dst = if top.visible {
                        over(base, src, top.opacity)
                    } else {
                        base
                    };
                }
                below.visible = true;
                below.opacity = 255;
                Ok(())
            })?;
        }
        LAYER_APPLY => {
            let name = a.text(LAYER_NAME);
            let opacity = a.number(OPACITY, 0, 255)? as u8;
            let visible = a.checked(VISIBLE);
            a.edit(|p| {
                let v = p.object_mut().enable(d);
                let layer = &mut v.layers[v.active];
                layer.name = name;
                layer.opacity = opacity;
                layer.visible = visible;
                Ok(())
            })?;
        }
        DELETE | FLIP_H | FLIP_V | ROTATE => {
            a.edit(|p| {
                let v = p.object_mut().enable(d);
                match id {
                    DELETE => {
                        for y in r.y..r.y + r.h as i32 {
                            for x in r.x..r.x + r.w as i32 {
                                put(v, x, y, CLEAR, None);
                            }
                        }
                    }
                    FLIP_H | FLIP_V => flip(v, id == FLIP_H, r),
                    _ => rotate(v, r),
                }
                Ok(())
            })?;
        }
        ALL => {
            let [w, h] = a.direction.size(a.editor.project.object().size);
            a.selection = Some(Rect { x: 0, y: 0, w, h });
            a.invalidate();
        }
        NONE => {
            if let Some(stroke) = a.stroke.take() {
                a.editor.project = stroke.before;
            }
            a.selection = None;
            a.refresh_depths();
            a.invalidate();
        }
        GRID => {
            a.grid = !a.grid;
            a.invalidate();
        }
        FIT => a.fit(),
        ZOOM_IN | ZOOM_OUT => {
            let bounds = a.rect(a.canvas);
            zoom_canvas(
                a,
                [bounds.right / 2, bounds.bottom / 2],
                if id == ZOOM_IN { 1.25 } else { 0.8 },
            );
        }
        APPLY_COLOR => {
            let text = a.text(COLOR_TEXT);
            let text = text.trim().trim_start_matches('#');
            if text.len() != 6 && text.len() != 8 {
                return Err(io::Error::other(
                    "色はRRGGBBまたはRRGGBBAAで入力してください",
                ));
            }
            if !text.is_ascii() {
                return Err(io::Error::other("色は16進数で入力してください"));
            }
            let mut c = [255; 4];
            for (i, b) in c.iter_mut().take(text.len() / 2).enumerate() {
                *b = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)
                    .map_err(|_| io::Error::other("色は16進数で入力してください"))?;
            }
            a.color = c;
            a.color_text();
            a.invalidate();
        }
        _ => {}
    }
    Ok(())
}
fn zoom_canvas(a: &mut App, mouse: [i32; 2], factor: f32) {
    let old = a.origin();
    let logical = [
        (mouse[0] as f32 - old[0]) / a.zoom,
        (mouse[1] as f32 - old[1]) / a.zoom,
    ];
    a.zoom = (a.zoom * factor).clamp(0.25, 96.);
    let new = a.origin();
    for i in 0..2 {
        a.pan[i] += mouse[i] as f32 - new[i] - logical[i] * a.zoom;
    }
    a.invalidate();
}
fn paint_to(a: &mut App, point: [i32; 2], first: bool) {
    let p = a.bounded_pixel(point);
    let color = if a.tool == Tool::Eraser {
        CLEAR
    } else {
        a.color
    };
    let size = match a.number(BRUSH, 1, 16) {
        Ok(n) => n,
        Err(e) => {
            a.status(&e.to_string());
            return;
        }
    };
    let clip = a.selection;
    if a.tool == Tool::Picker {
        if let Some(v) = a.view() {
            a.color = v.composite()[(p[1] as u32 * v.width + p[0] as u32) as usize];
            a.color_text();
            a.invalidate();
        }
        return;
    }
    if first {
        a.finish_stroke();
        let sculpt = if matches!(a.tool, Tool::Thin | Tool::RestoreDepth) {
            match sculpt::Stroke::new(
                a.editor.project.object(),
                a.direction,
                a.tool == Tool::RestoreDepth,
            ) {
                Ok(s) => Some(s),
                Err(e) => {
                    a.status(&e.to_string());
                    return;
                }
            }
        } else {
            None
        };
        a.stroke = Some(Stroke {
            before: a.editor.project.clone(),
            start: p,
            last: p,
            sculpt,
        });
    }
    let Some(stroke) = a.stroke.as_ref() else {
        return;
    };
    let start = stroke.start;
    let last = stroke.last;
    match a.tool {
        Tool::Thin | Tool::RestoreDepth => {
            if let Some(sculpt) = a.stroke.as_mut().and_then(|s| s.sculpt.as_mut()) {
                sculpt.apply(a.editor.project.object_mut(), last, p, size, clip);
                a.depths = Some(sculpt.depths.clone());
            }
        }
        Tool::Pencil | Tool::Eraser => line(a.view_mut(), last, p, color, size, clip),
        Tool::Fill if first => fill(a.view_mut(), p, color, clip),
        Tool::Selection => {
            a.selection = Some(Rect::between(start, p));
        }
        Tool::Line | Tool::Rectangle | Tool::Ellipse => {
            let tool = a.tool;
            let filled = a.checked(FILLED);
            let old = stroke.before.object().views[a.direction.index()]
                .as_ref()
                .unwrap();
            let active = old.active;
            let pixels = old.layers[active].pixels.clone();
            let v = a.view_mut();
            v.layers[active].pixels = pixels;
            shape(v, tool, [start, p], color, size, filled, clip);
        }
        _ => {}
    }
    if let Some(stroke) = &mut a.stroke {
        stroke.last = p;
    }
    unsafe {
        let _ = InvalidateRect(Some(a.canvas), None, false);
    }
}
fn handle_key(message: &MSG) -> bool {
    if message.message != WM_KEYDOWN {
        return false;
    }
    let key = message.wParam.0 as u16;
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let mut class = [0u16; 32];
    unsafe {
        GetClassNameW(message.hwnd, &mut class);
    };
    let text = String::from_utf16_lossy(&class);
    let editing = text.starts_with("Edit") || text.starts_with("EDIT") || text.starts_with("Combo");
    let id = if key == VK_F1.0 {
        Some(HELP)
    } else if ctrl && key == b'S' as u16 {
        Some(SAVE)
    } else if editing {
        None
    } else if ctrl {
        match key {
            78 => Some(NEW),
            79 => Some(OPEN),
            90 => Some(UNDO),
            89 => Some(REDO),
            67 => Some(COPY),
            88 => Some(CUT),
            86 => Some(PASTE),
            65 => Some(ALL),
            _ => None,
        }
    } else {
        match key {
            46 => Some(DELETE),
            27 => Some(NONE),
            71 => Some(GRID),
            70 => Some(FIT),
            187 | 107 => Some(ZOOM_IN),
            189 | 109 => Some(ZOOM_OUT),
            _ => None,
        }
    };
    if let Some(id) = id {
        if let Err(e) = dispatch(id, 0) {
            let hwnd = with(|a| a.hwnd).unwrap();
            error(hwnd, &e.to_string());
        }
        return true;
    }
    if !editing && !ctrl {
        let tool = match key {
            66 => Some(Tool::Pencil),
            69 => Some(Tool::Eraser),
            75 => Some(Tool::Fill),
            73 => Some(Tool::Picker),
            76 => Some(Tool::Line),
            82 => Some(Tool::Rectangle),
            79 => Some(Tool::Ellipse),
            77 => Some(Tool::Selection),
            84 => Some(Tool::Thin),
            85 => Some(Tool::RestoreDepth),
            _ => None,
        };
        if let Some(tool) = tool {
            with(|a| {
                a.set_tool(tool);
                unsafe {
                    SendMessageW(
                        a.controls[&TOOL],
                        CB_SETCURSEL,
                        Some(WPARAM(Tool::ALL.iter().position(|t| *t == tool).unwrap())),
                        None,
                    );
                }
            });
            return true;
        }
    }
    false
}
fn point(l: LPARAM) -> [i32; 2] {
    [
        (l.0 as u16 as i16) as i32,
        ((l.0 >> 16) as u16 as i16) as i32,
    ]
}
unsafe extern "system" fn frame_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_COMMAND => {
                let id = (w.0 & 0xffff) as u16;
                let notification = (w.0 >> 16) as u16;
                if APP.with(|a| a.try_borrow_mut().is_ok())
                    && with(|_| ()).is_some()
                    && let Err(e) = dispatch(id, notification)
                {
                    error(hwnd, &e.to_string());
                }
                return LRESULT(0);
            }
            WM_SIZE => {
                with(|a| a.layout());
                return LRESULT(0);
            }
            WM_PAINT => {
                drawing::paint(hwnd, 0);
                return LRESULT(0);
            }
            WM_PRINTCLIENT => {
                drawing::print(hwnd, 0, HDC(w.0 as *mut _));
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_TIMER => {
                with(|a| a.poll());
                if with(|a| a.smoke.is_some()).unwrap_or(false)
                    && let Err(e) = smoke::advance()
                {
                    with(|a| {
                        if let Some(dir) = &a.smoke {
                            let _ = std::fs::write(dir.join("ui-smoke.txt"), e.to_string());
                        }
                        a.smoke = None;
                    });
                    let _ = DestroyWindow(hwnd);
                    PostQuitMessage(1);
                }
                return LRESULT(0);
            }
            WM_CLOSE => {
                if confirm_discard() {
                    let _ = DestroyWindow(hwnd);
                }
                return LRESULT(0);
            }
            WM_DESTROY => {
                let _ = KillTimer(Some(hwnd), 1);
                PostQuitMessage(0);
                return LRESULT(0);
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
                let p = point(l);
                with(|a| {
                    let x = p[0] - a.s(234);
                    let y = p[1] - a.s(4);
                    let cell = a.s(20);
                    if x >= 0 && y >= 0 && x < 16 * cell && y < 2 * cell {
                        let i = a.palette_page * 32 + (y / cell * 16 + x / cell) as usize;
                        if let Some(&c) = a.editor.project.palette.get(i) {
                            if message == WM_LBUTTONDOWN {
                                a.color = c;
                                a.color_text();
                                a.invalidate();
                            } else {
                                let color = a.color;
                                let _ = a.edit(|p| {
                                    p.palette[i] = color;
                                    Ok(())
                                });
                            }
                        }
                    }
                });
                return LRESULT(0);
            }
            WM_GETMINMAXINFO => {
                let dpi = GetDpiForWindow(hwnd).max(96) as i32;
                let info = &mut *(l.0 as *mut MINMAXINFO);
                info.ptMinTrackSize = POINT {
                    x: 680 * dpi / 96,
                    y: 520 * dpi / 96,
                };
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                with(|a| {
                    a.dpi = (w.0 & 0xffff) as i32;
                    let font = make_font(a.dpi);
                    for child in a.controls.values() {
                        SendMessageW(
                            *child,
                            WM_SETFONT,
                            Some(WPARAM(font.0 as usize)),
                            Some(LPARAM(1)),
                        );
                    }
                    let old = std::mem::replace(&mut a.font, font);
                    let _ = DeleteObject(old.into());
                    let rect = &*(l.0 as *const RECT);
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    a.layout();
                });
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, message, w, l)
    }
}
unsafe extern "system" fn pane_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let kind = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as u32;
        if message == WM_COMMAND {
            return SendMessageW(
                GetParent(hwnd).unwrap_or_default(),
                message,
                Some(w),
                Some(l),
            );
        }
        match message {
            WM_PAINT => {
                drawing::paint(hwnd, kind);
                return LRESULT(0);
            }
            WM_PRINTCLIENT => {
                drawing::print(hwnd, kind, HDC(w.0 as *mut _));
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_VSCROLL if kind == 3 => {
                with(|a| {
                    let code = (w.0 & 0xffff) as u32;
                    let delta = match code {
                        0 => -a.s(24),
                        1 => a.s(24),
                        2 => -a.s(200),
                        3 => a.s(200),
                        5 | 4 => ((w.0 >> 16) & 0xffff) as i32 - a.scroll,
                        _ => 0,
                    };
                    a.scroll_panel(delta);
                });
                return LRESULT(0);
            }
            WM_MOUSEWHEEL => {
                let delta = ((w.0 >> 16) as u16 as i16) as f32 / 120.;
                let mut p = POINT {
                    x: point(l)[0],
                    y: point(l)[1],
                };
                let _ = ScreenToClient(hwnd, &mut p);
                with(|a| {
                    if kind == 1 {
                        zoom_canvas(a, [p.x, p.y], 1.2f32.powf(delta));
                    } else if kind == 2 {
                        a.model_zoom = (a.model_zoom * 1.15f32.powf(delta)).clamp(0.2, 8.);
                        a.request_preview();
                    } else if kind == 3 {
                        a.scroll_panel(-(delta * 72. * a.dpi as f32 / 96.) as i32);
                    }
                });
                return LRESULT(0);
            }
            WM_LBUTTONDOWN => {
                let p = point(l);
                let _ = SetFocus(Some(hwnd));
                with(|a| {
                    if kind == 1 {
                        let [x, y] = a.pixel(p);
                        let [vw, vh] = a.direction.size(a.editor.project.object().size);
                        if x >= 0 && y >= 0 && x < vw as i32 && y < vh as i32 {
                            SetCapture(hwnd);
                            paint_to(a, p, true);
                        }
                    } else if kind == 2 {
                        SetCapture(hwnd);
                        a.orbit = Some(p);
                    }
                });
                return LRESULT(0);
            }
            WM_RBUTTONDOWN if kind == 1 => {
                let p = point(l);
                with(|a| {
                    let tool = a.tool;
                    a.tool = Tool::Picker;
                    paint_to(a, p, false);
                    a.tool = tool;
                });
                return LRESULT(0);
            }
            WM_MBUTTONDOWN if kind == 1 => {
                SetCapture(hwnd);
                with(|a| a.panning = Some(point(l)));
                return LRESULT(0);
            }
            WM_MOUSEMOVE => {
                let p = point(l);
                with(|a| {
                    if kind == 1 {
                        if let Some(last) = a.panning {
                            for i in 0..2 {
                                a.pan[i] += (p[i] - last[i]) as f32;
                            }
                            a.panning = Some(p);
                            a.invalidate();
                        } else if a.stroke.is_some() {
                            paint_to(a, p, false);
                        }
                        a.depth_status(p);
                    } else if kind == 2
                        && let Some(last) = a.orbit
                    {
                        a.viewpoint = None;
                        a.yaw += (p[0] - last[0]) as f32 * 0.01;
                        a.pitch = (a.pitch + (p[1] - last[1]) as f32 * 0.01)
                            .clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
                        a.orbit = Some(p);
                        a.request_preview();
                    }
                });
                return LRESULT(0);
            }
            WM_LBUTTONUP | WM_MBUTTONUP => {
                with(|a| {
                    if kind == 1 {
                        a.finish_stroke();
                        a.panning = None;
                    } else {
                        a.orbit = None;
                    }
                });
                let _ = ReleaseCapture();
                return LRESULT(0);
            }
            WM_CAPTURECHANGED => {
                with(|a| {
                    a.finish_stroke();
                    a.panning = None;
                    a.orbit = None;
                });
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, message, w, l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipboard_checks_dimensions_before_length_arithmetic() {
        let original = Clip {
            width: 2,
            height: 1,
            pixels: vec![[10, 20, 30, 40], CLEAR],
        };
        let encoded = clip_bytes(&original).unwrap();
        assert_eq!(parse_clip(&encoded).unwrap().pixels, original.pixels);
        assert!(parse_clip(&encoded[..encoded.len() - 1]).is_err());
        let mut malicious = u32::MAX.to_le_bytes().to_vec();
        malicious.extend(u32::MAX.to_le_bytes());
        assert!(parse_clip(&malicious).is_err());
    }
}

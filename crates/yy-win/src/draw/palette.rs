//! Owner-drawn, categorized palette. Thumbnails are rendered lazily off the UI thread.
use super::*;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use windows::Win32::{
    Foundation::COLORREF,
    Graphics::Gdi::*,
    UI::Shell::{DefSubclassProc, SetWindowSubclass},
};

pub const CATEGORY: u16 = 104;
const LEFT: i32 = 260;
pub const WIDTH: i32 = LEFT;
pub struct Item {
    pub tool: Tool,
    pub label: String,
    pub category: String,
    pub key: String,
}
pub struct Thumb {
    pub pixels: Vec<u8>,
    pub white: Vec<u8>,
    pub size: u32,
}
pub struct PaletteDrag {
    pub tool: Tool,
    pub key: String,
    pub start: Point,
    pub current: Point,
    pub active: bool,
}
type Thumbnails = Vec<(String, std::result::Result<Thumb, String>)>;
#[derive(Default)]
pub struct State {
    pub category: HWND,
    pub categories: Vec<String>,
    pub items: Vec<Item>,
    pub thumbs: HashMap<String, Thumb>,
    order: VecDeque<String>,
    requested: HashSet<String>,
    queue: Vec<(String, String)>,
    task: Option<yy_draw::jobs::Task<Thumbnails>>,
    pub drag: Option<PaletteDrag>,
}
pub fn install(list: HWND, parent: HWND) {
    unsafe {
        let _ = SetWindowSubclass(list, Some(subclass), 8901, parent.0 as usize);
    }
}
impl State {
    pub fn dpi_changed(&mut self) {
        self.task = None;
        self.thumbs.clear();
        self.order.clear();
        self.requested.clear();
        self.queue.clear();
    }
}
unsafe extern "system" fn subclass(
    list: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    let result = unsafe { DefSubclassProc(list, msg, wp, lp) };
    if msg == WM_LBUTTONDOWN {
        let parent = HWND(data as *mut _);
        let hit = unsafe { SendMessageW(list, LB_ITEMFROMPOINT, None, Some(lp)).0 as u32 };
        if hit >> 16 == 0 {
            with(|a| {
                if !a.apply_properties() {
                    return;
                }
                if let Some(item) = a.palette_state.items.get((hit & 0xffff) as usize) {
                    let mut p = windows::Win32::Foundation::POINT {
                        x: point(lp).x as i32,
                        y: point(lp).y as i32,
                    };
                    unsafe {
                        let _ =
                            MapWindowPoints(Some(list), Some(parent), std::slice::from_mut(&mut p));
                        SetCapture(parent);
                    }
                    let p = Point::new(p.x as f64, p.y as f64);
                    a.palette_state.drag = Some(PaletteDrag {
                        tool: item.tool,
                        key: item.key.clone(),
                        start: p,
                        current: p,
                        active: false,
                    });
                }
            });
        }
    }
    result
}
impl App {
    pub(super) fn rebuild_palette(&mut self) {
        let filter = text(self.filter).to_lowercase();
        let selected_category = text(self.palette_state.category);
        let mut categories = BTreeSet::new();
        for e in yy_draw::catalog::all() {
            categories.insert(e.category_path());
        }
        let categories: Vec<_> = ["基本図形".to_owned(), "文書・追加素材".to_owned()]
            .into_iter()
            .chain(categories)
            .collect();
        unsafe {
            if self.palette_state.categories != categories {
                SendMessageW(self.palette_state.category, CB_RESETCONTENT, None, None);
                for s in &categories {
                    let s = HSTRING::from(s);
                    SendMessageW(
                        self.palette_state.category,
                        CB_ADDSTRING,
                        None,
                        Some(LPARAM(s.as_ptr() as isize)),
                    );
                }
                let index = categories
                    .iter()
                    .position(|s| *s == selected_category)
                    .unwrap_or(0);
                SendMessageW(
                    self.palette_state.category,
                    CB_SETCURSEL,
                    Some(WPARAM(index)),
                    None,
                );
                self.palette_state.categories = categories;
            }
        }
        let category = text(self.palette_state.category);
        let matches = |label: &str, path: &str| {
            path == category && format!("{path} {label}").to_lowercase().contains(&filter)
        };
        let mut items = vec![];
        for (shape, name) in SHAPES {
            if matches(name, "基本図形") {
                items.push(Item {
                    tool: Tool::Shape(*shape),
                    label: (*name).into(),
                    category: "基本図形".into(),
                    key: format!("shape:{shape:?}"),
                });
            }
        }
        let originals: HashSet<_> = yy_draw::catalog::all()
            .iter()
            .map(|e| e.svg.as_str())
            .collect();
        let mut seen = HashSet::new();
        for (doc, library) in [(&self.tabs[self.tab].doc, false), (&self.library, true)] {
            for asset in &doc.assets {
                if !yy_draw::library::is_visible(doc, asset.id)
                    || originals.contains(asset.svg.as_str())
                    || !seen.insert(asset.svg.as_str())
                    || !matches(&asset.name, "文書・追加素材")
                {
                    continue;
                }
                items.push(Item {
                    tool: if library {
                        Tool::LibraryIcon(asset.id)
                    } else {
                        Tool::Icon(asset.id)
                    },
                    label: asset.name.clone(),
                    category: "文書・追加素材".into(),
                    key: blake3::hash(asset.svg.as_bytes()).to_hex().to_string(),
                });
            }
        }
        for (index, entry) in yy_draw::catalog::all().iter().enumerate() {
            let path = entry.category_path();
            if matches(&entry.name, &path) {
                items.push(Item {
                    tool: Tool::CatalogIcon(index),
                    label: entry.name.clone(),
                    category: path,
                    key: format!("catalog:{index}"),
                });
            }
        }
        self.palette_tools = items.iter().map(|i| i.tool).collect();
        self.palette_state.items = items;
        unsafe {
            SendMessageW(self.palette, WM_SETREDRAW, Some(WPARAM(0)), None);
            SendMessageW(self.palette, LB_RESETCONTENT, None, None);
            for item in &self.palette_state.items {
                let label = HSTRING::from(&item.label);
                SendMessageW(
                    self.palette,
                    LB_ADDSTRING,
                    None,
                    Some(LPARAM(label.as_ptr() as isize)),
                );
            }
            SendMessageW(
                self.palette,
                LB_SETITEMHEIGHT,
                Some(WPARAM(0)),
                Some(LPARAM((64. * self.dpi) as isize)),
            );
            SendMessageW(self.palette, WM_SETREDRAW, Some(WPARAM(1)), None);
            let _ = InvalidateRect(Some(self.palette), None, false);
        }
    }
    fn palette_svg(&self, tool: Tool) -> Option<String> {
        match tool {
            Tool::CatalogIcon(i) => yy_draw::catalog::all().get(i).map(|e| e.svg.clone()),
            Tool::Icon(i) => self.tabs[self.tab]
                .doc
                .assets
                .iter()
                .find(|a| a.id == i)
                .map(|a| a.svg.clone()),
            Tool::LibraryIcon(i) => self
                .library
                .assets
                .iter()
                .find(|a| a.id == i)
                .map(|a| a.svg.clone()),
            Tool::Shape(shape) => {
                let mut d = Document::default();
                d.add_node(shape, Point::new(4., 4.), None);
                let n = &mut d.nodes[0];
                n.rect = Rect {
                    x: 6.,
                    y: 10.,
                    w: 36.,
                    h: 28.,
                };
                n.label = if shape == Shape::Text {
                    "T".into()
                } else {
                    String::new()
                };
                Some(yy_draw::render::scene(
                    &d,
                    &Export {
                        bounds: Rect {
                            x: 0.,
                            y: 0.,
                            w: 48.,
                            h: 48.,
                        },
                        background: None,
                        grid: false,
                        text: true,
                        selected: vec![],
                    },
                ))
            }
            _ => None,
        }
    }
    pub(super) fn draw_palette_item(&mut self, draw: &DRAWITEMSTRUCT) {
        if draw.itemID == u32::MAX {
            return;
        }
        let Some(item) = self.palette_state.items.get(draw.itemID as usize) else {
            return;
        };
        let key = item.key.clone();
        let tool = item.tool;
        let label = item.label.clone();
        let category = item.category.clone();
        if !self.palette_state.requested.contains(&key) {
            if let Some(svg) = self.palette_svg(tool) {
                self.palette_state.queue.push((key.clone(), svg));
                self.palette_state.requested.insert(key.clone());
            }
        }
        unsafe {
            let selected = draw.itemState.0 & ODS_SELECTED.0 != 0;
            let color = if selected { 0x00f5e8dd } else { 0x00ffffff };
            let brush = CreateSolidBrush(COLORREF(color));
            FillRect(draw.hDC, &draw.rcItem, brush);
            let _ = DeleteObject(brush.into());
            let left = draw.rcItem.left + (6. * self.dpi) as i32;
            let top = draw.rcItem.top + (8. * self.dpi) as i32;
            let size = (44. * self.dpi) as i32;
            if let Some(thumb) = self.palette_state.thumbs.get(&key) {
                self.palette_state.order.retain(|v| v != &key);
                self.palette_state.order.push_back(key.clone());
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: thumb.size as i32,
                        biHeight: -(thumb.size as i32),
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                StretchDIBits(
                    draw.hDC,
                    left,
                    top,
                    size,
                    size,
                    0,
                    0,
                    thumb.size as i32,
                    thumb.size as i32,
                    Some(thumb.white.as_ptr().cast()),
                    &info,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
            }
            SetBkMode(draw.hDC, TRANSPARENT);
            let old = SelectObject(draw.hDC, self.font.into());
            SetTextColor(draw.hDC, COLORREF(0x003b3024));
            let mut r = draw.rcItem;
            r.left = left + size + (8. * self.dpi) as i32;
            r.top += (5. * self.dpi) as i32;
            r.bottom = r.top + (34. * self.dpi) as i32;
            let mut s: Vec<_> = label.encode_utf16().collect();
            DrawTextW(
                draw.hDC,
                &mut s,
                &mut r,
                DT_WORDBREAK | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            r.top = r.bottom;
            r.bottom = draw.rcItem.bottom;
            SetTextColor(draw.hDC, COLORREF(0x007d7168));
            let mut s: Vec<_> = category.encode_utf16().collect();
            DrawTextW(
                draw.hDC,
                &mut s,
                &mut r,
                DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            SelectObject(draw.hDC, old);
            if draw.itemState.0 & ODS_FOCUS.0 != 0 {
                let _ = DrawFocusRect(draw.hDC, &draw.rcItem);
            }
        }
    }
    pub(super) fn poll_thumbnails(&mut self) {
        if let Some(task) = &self.palette_state.task
            && let Some(result) = task.poll()
        {
            self.palette_state.task = None;
            if let Ok(images) = result {
                for (key, image) in images {
                    if let Ok(image) = image {
                        self.palette_state.order.push_back(key.clone());
                        self.palette_state.thumbs.insert(key, image);
                        while self.palette_state.thumbs.len() > 256 {
                            if let Some(old) = self.palette_state.order.pop_front() {
                                self.palette_state.thumbs.remove(&old);
                                self.palette_state.requested.remove(&old);
                            } else {
                                break;
                            }
                        }
                    }
                }
            }
            unsafe {
                let _ = InvalidateRect(Some(self.palette), None, false);
            }
            self.invalidate_overlay();
        }
        if self.palette_state.task.is_none() && !self.palette_state.queue.is_empty() {
            let count = self.palette_state.queue.len().min(8);
            let batch = self.palette_state.queue.drain(..count).collect::<Vec<_>>();
            let renderer = self.renderer.clone();
            let size = (48. * self.dpi).ceil() as u32;
            self.palette_state.task = Some(yy_draw::jobs::Task::spawn(move |c| {
                let mut out = vec![];
                for (key, svg) in batch {
                    c.checkpoint(0, "一覧のアイコンを描画")?;
                    let image = renderer.thumbnail(&svg, size).map(|p| {
                        let mut pixels = p.data().to_vec();
                        let mut white = pixels.clone();
                        for (a, b) in pixels.chunks_exact_mut(4).zip(white.chunks_exact_mut(4)) {
                            a.swap(0, 2);
                            b[0] = a[0].saturating_add(255 - a[3]);
                            b[1] = a[1].saturating_add(255 - a[3]);
                            b[2] = a[2].saturating_add(255 - a[3]);
                            b[3] = 255;
                        }
                        Thumb {
                            pixels,
                            white,
                            size,
                        }
                    });
                    out.push((key, image));
                }
                Ok(out)
            }));
        }
    }
    pub(super) fn finish_palette_drag(&mut self, p: Point) -> bool {
        let Some(drag) = self.palette_state.drag.take() else {
            return false;
        };
        let (l, t, w, h) = self.viewport();
        if drag.active
            && p.x >= l as f64
            && p.y >= t as f64
            && p.x < (l as f64 + w as f64)
            && p.y < (t as f64 + h as f64)
        {
            self.tool = drag.tool;
            let q = self.document_point(p);
            let count = self.tabs[self.tab].doc.nodes.len();
            self.down(p, false);
            let doc = &mut self.tabs[self.tab].doc;
            if doc.nodes.len() > count {
                let n = doc.nodes.last().unwrap();
                if n.shape != Shape::Junction {
                    let position = doc.snap_point(Point::new(
                        q.x - n.rect.w / 2.,
                        q.y - if n.shape == Shape::Icon {
                            (n.rect.h - 24.) / 2.
                        } else {
                            n.rect.h / 2.
                        },
                    ));
                    let n = doc.nodes.last_mut().unwrap();
                    n.rect.x = position.x;
                    n.rect.y = position.y;
                }
            }
        }
        self.up(p);
        true
    }
    #[cfg(test)]
    pub(super) fn test_palette_image(&mut self) -> Vec<u8> {
        unsafe {
            let width = (248. * self.dpi) as i32;
            let row = (64. * self.dpi) as i32;
            let height = row * 3;
            let dc = CreateCompatibleDC(None);
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
            let mut bits = std::ptr::null_mut();
            let bitmap =
                CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            let old = SelectObject(dc, bitmap.into());
            for i in 0..3 {
                self.draw_palette_item(&DRAWITEMSTRUCT {
                    CtlID: u32::from(PALETTE),
                    itemID: i,
                    hDC: dc,
                    rcItem: RECT {
                        left: 0,
                        top: i as i32 * row,
                        right: width,
                        bottom: (i as i32 + 1) * row,
                    },
                    ..Default::default()
                });
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                self.poll_thumbnails();
                if (0..3).all(|i| {
                    self.palette_state
                        .thumbs
                        .contains_key(&self.palette_state.items[i].key)
                }) {
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            for i in 0..3 {
                self.draw_palette_item(&DRAWITEMSTRUCT {
                    CtlID: u32::from(PALETTE),
                    itemID: i,
                    hDC: dc,
                    rcItem: RECT {
                        left: 0,
                        top: i as i32 * row,
                        right: width,
                        bottom: (i as i32 + 1) * row,
                    },
                    ..Default::default()
                });
            }
            let _ = GdiFlush();
            let pixels =
                std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize)
                    .to_vec();
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            for i in 0..3 {
                let colored = (10..row - 12).any(|y| {
                    (6..(48. * self.dpi) as i32).any(|x| {
                        let p = ((i * row + y) * width + x) as usize * 4;
                        pixels[p..p + 3] != [255, 255, 255]
                    })
                });
                assert!(colored, "palette row {i} has no icon");
            }
            let mut bmp = vec![0u8; 54];
            bmp[..2].copy_from_slice(b"BM");
            bmp[2..6].copy_from_slice(&(54 + pixels.len() as u32).to_le_bytes());
            bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
            bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
            bmp[18..22].copy_from_slice(&width.to_le_bytes());
            bmp[22..26].copy_from_slice(&(-height).to_le_bytes());
            bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
            bmp[28..30].copy_from_slice(&32u16.to_le_bytes());
            bmp.extend(pixels);
            bmp
        }
    }
}

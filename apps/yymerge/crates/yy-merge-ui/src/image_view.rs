//! Dedicated native raster review; immutable pixels are prepared off the message thread.
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{Com::*, LibraryLoader::*},
        UI::{
            HiDpi::*,
            Input::KeyboardAndMouse::*,
            Shell::{FileOpenDialog, FileSaveDialog, IFileDialog, SIGDN_FILESYSPATH},
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, PCWSTR, w},
};
use yy_merge_cli::Gui;
use yy_merge_core::{
    Cancellation, Result,
    images::{self, Input, Raster, Settings, Summary},
};
const CLASS: PCWSTR = w!("YYMergeImageReview");
const OPEN: usize = 400;
const SAVE: usize = 401;
const HTML: usize = 402;
const JSON: usize = 403;
const EXIT: usize = 404;
const FIT: usize = 410;
const ACTUAL: usize = 411;
const PLUS: usize = 412;
const MINUS: usize = 413;
const DIFF: usize = 420;
const OVERLAY: usize = 421;
const BLINK: usize = 422;
const HELP: usize = 423;
const APPLY: usize = 430;
const RESET: usize = 431;
const ALIGN_LEFT: usize = 432;
const ALIGN_RIGHT: usize = 433;
const ALIGN_UP: usize = 434;
const ALIGN_DOWN: usize = 435;
const PREVIOUS_PAGE: usize = 436;
const NEXT_PAGE: usize = 437;
const HELP_MD: &str = include_str!("../../../help/help.md");
thread_local! { static STATE: RefCell<Option<State>> = const { RefCell::new(None) }; }
fn state<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    STATE.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}
fn win<T>(r: windows::core::Result<T>) -> Result<T> {
    r.map_err(|e| e.to_string())
}
fn text(h: HWND, s: &str) {
    unsafe {
        let _ = SetWindowTextW(h, &HSTRING::from(s));
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Difference,
    Overlay,
    Blink,
}
struct Bitmap {
    width: u32,
    height: u32,
    pixels: Arc<[u8]>,
}
impl Bitmap {
    fn new(r: &Raster, c: &Cancellation) -> Result<Self> {
        Ok(Self {
            width: r.width,
            height: r.height,
            pixels: images::bgrx(r, c)?,
        })
    }
}
struct Pair {
    left: Input,
    right: Input,
    bitmaps: [Bitmap; 2],
    documents: Option<Arc<yy_merge_core::office_render::Pair>>,
    page: usize,
    missing: [bool; 2],
}
impl Pair {
    fn page(
        documents: Arc<yy_merge_core::office_render::Pair>,
        page: usize,
        c: &Cancellation,
    ) -> Result<Arc<Self>> {
        let p = documents.page(page, c)?;
        Self::new(p.left, p.right, Some(documents), page, p.missing, c)
    }
    fn new(
        left: Input,
        right: Input,
        documents: Option<Arc<yy_merge_core::office_render::Pair>>,
        page: usize,
        missing: [bool; 2],
        c: &Cancellation,
    ) -> Result<Arc<Self>> {
        let bitmaps = [
            Bitmap::new(&left.raster, c)?,
            Bitmap::new(&right.raster, c)?,
        ];
        Ok(Arc::new(Self {
            left,
            right,
            bitmaps,
            documents,
            page,
            missing,
        }))
    }
    fn open(paths: &[PathBuf; 2], c: &Cancellation) -> Result<Arc<Self>> {
        if paths
            .iter()
            .any(|p| yy_merge_core::office_render::is_document(p))
        {
            Self::page(
                Arc::new(yy_merge_core::office_render::Pair::open(
                    &paths[0], &paths[1], c,
                )?),
                0,
                c,
            )
        } else {
            Self::new(
                Input::open(&paths[0], c)?,
                Input::open(&paths[1], c)?,
                None,
                0,
                [false; 2],
                c,
            )
        }
    }
}
struct Display {
    mode: Mode,
    pair: Arc<Pair>,
    summary: Summary,
    third: Bitmap,
}
enum Completed {
    Display(Display),
    Export(PathBuf),
}
struct Job {
    thread: Option<std::thread::JoinHandle<()>>,
    rx: mpsc::Receiver<Result<Completed>>,
    cancel: Arc<Cancellation>,
    generation: u64,
    fit: bool,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
struct State {
    hwnd: HWND,
    font: HFONT,
    controls: Vec<HWND>,
    edits: [HWND; 3],
    top: i32,
    paths: [PathBuf; 2],
    labels: [Option<String>; 2],
    settings: Settings,
    mode: Mode,
    display: Option<Display>,
    job: Option<Job>,
    generation: u64,
    scale: f64,
    pan: [f64; 2],
    fit: bool,
    drag: Option<POINT>,
    blink: bool,
    last_blink: Instant,
    status: String,
    exit: i32,
    smoke: Option<PathBuf>,
    smoked: bool,
}
fn redraw(s: &State) {
    unsafe {
        let _ = InvalidateRect(Some(s.hwnd), None, false);
    }
}
fn status(s: &mut State, value: impl Into<String>) {
    s.status = value.into();
    redraw(s);
}
fn value(h: HWND) -> String {
    unsafe {
        let mut b = vec![0u16; GetWindowTextLengthW(h).max(0) as usize + 1];
        let n = GetWindowTextW(h, &mut b);
        String::from_utf16_lossy(&b[..n.max(0) as usize])
    }
}
fn sync_edits(s: &State) {
    for (h, v) in s.edits.iter().zip([
        s.settings.threshold.to_string(),
        s.settings.offset_x.to_string(),
        s.settings.offset_y.to_string(),
    ]) {
        text(*h, &v);
    }
}
fn calculate(
    pair: Arc<Pair>,
    settings: Settings,
    mode: Mode,
    c: &Cancellation,
) -> Result<Completed> {
    let result =
        yy_merge_core::office_render::compare(&pair.left, &pair.right, pair.missing, settings, c)?;
    let raster = if mode == Mode::Overlay {
        images::overlay(&pair.left.raster, &pair.right.raster, settings, c)?
    } else {
        result.difference
    };
    let third = Bitmap::new(&raster, c)?;
    Ok(Completed::Display(Display {
        mode,
        pair,
        summary: result.summary,
        third,
    }))
}
fn start(s: &mut State, reload: bool) {
    s.job = None;
    s.generation = s.generation.wrapping_add(1);
    let generation = s.generation;
    let settings = s.settings;
    let mode = s.mode;
    let paths = s.paths.clone();
    let pair = if reload {
        None
    } else {
        s.display.as_ref().map(|d| d.pair.clone())
    };
    let cancel = Arc::new(Cancellation::new());
    let worker = cancel.clone();
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let result = (|| {
            let pair = if let Some(pair) = pair {
                pair
            } else {
                Pair::open(&paths, &worker)?
            };
            pair.left.source.verify(&worker)?;
            pair.right.source.verify(&worker)?;
            calculate(pair, settings, mode, &worker)
        })();
        let _ = tx.send(result);
    });
    s.job = Some(Job {
        thread: Some(thread),
        rx,
        cancel,
        generation,
        fit: reload,
    });
    s.exit = 130;
    status(s, "画像を比較中… 操作は継続できます。Escで中止");
}
fn poll(s: &mut State) {
    let Some(job) = &s.job else { return };
    let result = match job.rx.try_recv() {
        Ok(r) => r,
        Err(mpsc::TryRecvError::Empty) => return,
        Err(_) => Err("画像ワーカーが終了しました".into()),
    };
    let job = s.job.take().unwrap();
    if job.generation != s.generation {
        return;
    }
    match result {
        Ok(Completed::Display(d)) => {
            s.settings = d.summary.settings;
            s.mode = d.mode;
            s.labels = [None, None];
            s.paths = [
                d.pair.left.source.path.clone(),
                d.pair.right.source.path.clone(),
            ];
            s.exit = i32::from(!d.summary.equivalent);
            let mut msg = format!(
                "{} | 変更 {} / {} 画素 | 閾値 {} | 右位置 ({},{}) | {}×{} / {}×{} | ドラッグ移動・ホイールズーム",
                if d.summary.equivalent {
                    "画素は同等"
                } else {
                    "相違あり"
                },
                d.summary.changed_pixels,
                d.summary.compared_pixels,
                s.settings.threshold,
                s.settings.offset_x,
                s.settings.offset_y,
                d.summary.left_size[0],
                d.summary.left_size[1],
                d.summary.right_size[0],
                d.summary.right_size[1]
            );
            if let Some(docs) = &d.pair.documents {
                let presence = match d.pair.missing {
                    [true, false] => "左ページなし",
                    [false, true] => "右ページなし",
                    _ => "両側あり",
                };
                msg = format!(
                    "ページ {}/{}（左{} / 右{}・{}）| {}",
                    d.pair.page + 1,
                    docs.pages(),
                    docs.left.pages,
                    docs.right.pages,
                    presence,
                    msg
                );
            }
            if !job.fit
                && !s.fit
                && let Some(previous) = &s.display
            {
                s.pan[0] += f64::from(d.summary.canvas.x - previous.summary.canvas.x) * s.scale;
                s.pan[1] += f64::from(d.summary.canvas.y - previous.summary.canvas.y) * s.scale;
            }
            s.display = Some(d);
            if job.fit || s.fit {
                fit(s);
            }
            sync_edits(s);
            status(s, msg);
        }
        Ok(Completed::Export(path)) => {
            s.exit = s
                .display
                .as_ref()
                .map_or(2, |d| i32::from(!d.summary.equivalent));
            status(s, format!("保存しました: {}", path.display()));
        }
        Err(e) => {
            s.exit = 2;
            if let Some(d) = &s.display {
                s.settings = d.summary.settings;
                s.mode = d.mode;
                s.labels = [None, None];
                s.paths = [
                    d.pair.left.source.path.clone(),
                    d.pair.right.source.path.clone(),
                ];
                sync_edits(s);
            }
            status(s, e);
        }
    }
}
fn client(s: &State) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(s.hwnd, &mut r);
    }
    r
}
fn unit(s: &State, n: i32) -> i32 {
    n * unsafe { GetDpiForWindow(s.hwnd) }.max(96) as i32 / 96
}
fn panels(s: &State) -> [RECT; 3] {
    let r = client(s);
    let gap = unit(s, 6);
    let width = ((r.right - gap * 4) / 3).max(1);
    let top = s.top + unit(s, 30);
    let bottom = (r.bottom - unit(s, 58)).max(top + 1);
    std::array::from_fn(|i| {
        let left = gap + i as i32 * (width + gap);
        RECT {
            left,
            top,
            right: left + width,
            bottom,
        }
    })
}
fn fit(s: &mut State) {
    if let Some(d) = &s.display {
        let p = panels(s)[0];
        s.scale = ((p.right - p.left - 4) as f64 / d.summary.canvas.width as f64)
            .min((p.bottom - p.top - 4) as f64 / d.summary.canvas.height as f64)
            .clamp(0.001, 32.);
        s.pan = [
            ((p.right - p.left) as f64 - d.summary.canvas.width as f64 * s.scale) / 2.,
            ((p.bottom - p.top) as f64 - d.summary.canvas.height as f64 * s.scale) / 2.,
        ];
        s.fit = true;
    }
}
fn zoom(s: &mut State, factor: f64, anchor: Option<POINT>) {
    let ps = panels(s);
    let p = anchor
        .and_then(|p| {
            ps.iter()
                .find(|r| p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom)
                .map(|r| [f64::from(p.x - r.left), f64::from(p.y - r.top)])
        })
        .unwrap_or([
            f64::from(ps[0].right - ps[0].left) / 2.,
            f64::from(ps[0].bottom - ps[0].top) / 2.,
        ]);
    let next = (s.scale * factor).clamp(0.001, 32.);
    let ratio = next / s.scale;
    for (i, anchor) in p.into_iter().enumerate() {
        s.pan[i] = anchor + (s.pan[i] - anchor) * ratio;
    }
    s.scale = next;
    s.fit = false;
    redraw(s);
}
unsafe fn create_font(hwnd: HWND) -> HFONT {
    unsafe {
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        CreateFontW(
            -14 * dpi / 96,
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
            FF_MODERN.0 as u32,
            &HSTRING::from(yy_ui_win::font::BUNDLED_FAMILY),
        )
    }
}
fn layout(s: &mut State) {
    let width = client(s).right;
    let mut x = unit(s, 8);
    let mut y = unit(s, 6);
    let row = unit(s, 30);
    for (i, h) in s.controls.iter().enumerate() {
        let w = unit(s, [48, 48, 36, 60, 36, 60, 64, 64, 60][i]);
        if x + w > width - unit(s, 6) {
            x = unit(s, 8);
            y += row;
        }
        unsafe {
            let _ = MoveWindow(*h, x, y, w, unit(s, 25), true);
        }
        x += w + unit(s, 5);
    }
    s.top = y + row;
    if s.fit {
        fit(s);
    }
    redraw(s);
}
unsafe fn fill(dc: HDC, r: &RECT, color: u32) {
    unsafe {
        let b = CreateSolidBrush(COLORREF(color));
        FillRect(dc, r, b);
        let _ = DeleteObject(b.into());
    }
}
unsafe fn draw_bitmap(dc: HDC, b: &Bitmap, s: &State, p: RECT, x: i32, y: i32) {
    unsafe {
        let Some(d) = &s.display else { return };
        let saved = SaveDC(dc);
        IntersectClipRect(dc, p.left, p.top, p.right, p.bottom);
        let x = p.left as f64 + s.pan[0] + f64::from(x - d.summary.canvas.x) * s.scale;
        let y = p.top as f64 + s.pan[1] + f64::from(y - d.summary.canvas.y) * s.scale;
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: b.width as i32,
                biHeight: -(b.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        SetStretchBltMode(dc, COLORONCOLOR);
        StretchDIBits(
            dc,
            x.round() as i32,
            y.round() as i32,
            (b.width as f64 * s.scale).round().max(1.) as i32,
            (b.height as f64 * s.scale).round().max(1.) as i32,
            0,
            0,
            b.width as i32,
            b.height as i32,
            Some(b.pixels.as_ptr().cast()),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
        let _ = RestoreDC(dc, saved);
    }
}
fn paint(s: &State, dc: HDC) {
    unsafe {
        let r = client(s);
        fill(dc, &r, 0x00f5f5f5);
        let old = SelectObject(dc, s.font.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x00252525));
        let mode = s.display.as_ref().map_or(s.mode, |d| d.mode);
        let labels = [
            format!(
                "LEFT: {} — {}",
                s.paths[0].file_name().unwrap_or_default().to_string_lossy(),
                s.labels[0]
                    .clone()
                    .unwrap_or_else(|| s.paths[0].display().to_string())
            ),
            format!(
                "RIGHT: {} — {}",
                s.paths[1].file_name().unwrap_or_default().to_string_lossy(),
                s.labels[1]
                    .clone()
                    .unwrap_or_else(|| s.paths[1].display().to_string())
            ),
            String::from(match mode {
                Mode::Difference => "差分（桃:変更／橙:欠落）",
                Mode::Overlay => "OVERLAY（50%ずつ）",
                Mode::Blink => {
                    if s.blink {
                        "BLINK: RIGHT"
                    } else {
                        "BLINK: LEFT"
                    }
                }
            }),
        ];
        for (i, p) in panels(s).into_iter().enumerate() {
            let mut label = RECT {
                left: p.left,
                top: s.top,
                right: p.right,
                bottom: p.top,
            };
            let mut utf16: Vec<u16> = labels[i].encode_utf16().collect();
            DrawTextW(
                dc,
                &mut utf16,
                &mut label,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
            fill(dc, &p, 0x00c8c8c8);
            if let Some(d) = &s.display {
                let image = match i {
                    0 => (&d.pair.bitmaps[0], 0, 0),
                    1 => (
                        &d.pair.bitmaps[1],
                        d.summary.settings.offset_x,
                        d.summary.settings.offset_y,
                    ),
                    _ if mode == Mode::Blink => {
                        if s.blink {
                            (
                                &d.pair.bitmaps[1],
                                d.summary.settings.offset_x,
                                d.summary.settings.offset_y,
                            )
                        } else {
                            (&d.pair.bitmaps[0], 0, 0)
                        }
                    }
                    _ => (&d.third, d.summary.canvas.x, d.summary.canvas.y),
                };
                draw_bitmap(dc, image.0, s, p, image.1, image.2);
            }
        }
        let mut r = RECT {
            left: unit(s, 8),
            top: r.bottom - unit(s, 54),
            right: r.right - unit(s, 8),
            bottom: r.bottom,
        };
        let mut text: Vec<u16> = format!("{} | {:.1}%", s.status, s.scale * 100.)
            .encode_utf16()
            .collect();
        DrawTextW(dc, &mut text, &mut r, DT_WORDBREAK | DT_END_ELLIPSIS);
        SelectObject(dc, old);
    }
}
fn choose(
    parent: HWND,
    save: bool,
    title: &str,
    extension: Option<&str>,
) -> Result<Option<PathBuf>> {
    unsafe {
        let dialog: IFileDialog = win(CoCreateInstance(
            if save {
                &FileSaveDialog
            } else {
                &FileOpenDialog
            },
            None,
            CLSCTX_INPROC_SERVER,
        ))?;
        win(dialog.SetTitle(&HSTRING::from(title)))?;
        if let Some(ext) = extension {
            win(dialog.SetDefaultExtension(&HSTRING::from(ext)))?;
        }
        if dialog.Show(Some(parent)).is_err() {
            return Ok(None);
        }
        let p = win(dialog.GetResult())?;
        let name = win(p.GetDisplayName(SIGDN_FILESYSPATH))?;
        let result = name.to_string().map_err(|e| e.to_string());
        CoTaskMemFree(Some(name.as_ptr().cast()));
        Ok(Some(PathBuf::from(result?)))
    }
}
fn export(s: &mut State, path: PathBuf, kind: usize) -> Result<()> {
    if s.job.is_some() {
        return Err("処理の完了後に保存してください".into());
    }
    let d = s.display.as_ref().ok_or("画像を開いてください")?;
    let pair = d.pair.clone();
    let settings = d.summary.settings;
    let cancel = Arc::new(Cancellation::new());
    let worker = cancel.clone();
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let result = (|| {
            let result = yy_merge_core::office_render::compare(
                &pair.left,
                &pair.right,
                pair.missing,
                settings,
                &worker,
            )?;
            let bytes = match kind {
                SAVE => images::png(&result.difference, &worker)?,
                HTML => {
                    let html = images::html(&pair.left, &pair.right, &result, &worker)?;
                    let note = format!("<p>Page {} / {} | missing LEFT={} RIGHT={}</p><main>", pair.page + 1, pair.documents.as_ref().map_or(1, |d| d.pages()), pair.missing[0], pair.missing[1]);
                    html.replacen("<main>", &note, 1).into_bytes()
                },
                _ => serde_json::to_vec_pretty(&serde_json::json!({"mode":"image","page":pair.page+1,"pages":pair.documents.as_ref().map_or(1, |d|d.pages()),"missing":pair.missing,"left":pair.left.source.path,"right":pair.right.source.path,"original_identical":pair.left.source.original_identical(&pair.right.source),"comparison":result.summary})).map_err(|e| e.to_string())?,
            };
            yy_merge_core::files::save(
                &path,
                &bytes,
                &[&pair.left.source, &pair.right.source],
                true,
                &worker,
            )?;
            Ok(Completed::Export(path))
        })();
        let _ = tx.send(result);
    });
    s.job = Some(Job {
        thread: Some(thread),
        rx,
        cancel,
        generation: s.generation,
        fit: false,
    });
    status(s, "画像レポートを保存中… Escで中止");
    Ok(())
}
fn command(s: &mut State, id: usize) -> Result<()> {
    match id {
        PREVIOUS_PAGE | NEXT_PAGE => {
            if s.job.is_some() {
                return Err("ページの読み込み完了を待ってください".into());
            }
            let pair = s
                .display
                .as_ref()
                .ok_or("PDF・Officeファイルを開いてください")?
                .pair
                .clone();
            let docs = pair.documents.clone().ok_or("通常の画像は1ページです")?;
            let page = if id == NEXT_PAGE {
                (pair.page + 1).min(docs.pages() - 1)
            } else {
                pair.page.saturating_sub(1)
            };
            if page == pair.page {
                return Ok(());
            }
            s.generation = s.generation.wrapping_add(1);
            let cancel = Arc::new(Cancellation::new());
            let worker = cancel.clone();
            let (tx, rx) = mpsc::channel();
            let mode = s.mode;
            let thread = std::thread::spawn(move || {
                let result = Pair::page(docs, page, &worker)
                    .and_then(|p| calculate(p, Settings::default(), mode, &worker));
                let _ = tx.send(result);
            });
            s.job = Some(Job {
                thread: Some(thread),
                rx,
                cancel,
                generation: s.generation,
                fit: true,
            });
            status(s, format!("ページ {} を描画中…", page + 1));
        }
        OPEN => {
            let Some(a) = choose(s.hwnd, false, "LEFT: 比較する画像", None)? else {
                return Ok(());
            };
            let Some(b) = choose(s.hwnd, false, "RIGHT: 比較する画像", None)? else {
                return Ok(());
            };
            s.labels = [None, None];
            s.paths = [a, b];
            s.settings = Default::default();
            sync_edits(s);
            start(s, true);
        }
        SAVE | HTML | JSON => {
            if s.job.is_some() {
                return Err("比較が完了してから保存してください".into());
            }
            let ext = match id {
                SAVE => "png",
                HTML => "html",
                _ => "json",
            };
            if let Some(path) = choose(
                s.hwnd,
                true,
                "画像比較の出力（原本は変更しません）",
                Some(ext),
            )? {
                export(s, path, id)?;
            }
        }
        FIT => {
            fit(s);
            redraw(s);
        }
        ACTUAL => zoom(s, 1. / s.scale, None),
        PLUS => zoom(s, 1.25, None),
        MINUS => zoom(s, 0.8, None),
        DIFF | OVERLAY | BLINK => {
            s.mode = match id {
                DIFF => Mode::Difference,
                OVERLAY => Mode::Overlay,
                _ => Mode::Blink,
            };
            start(s, false);
        }
        APPLY => {
            let threshold = value(s.edits[0])
                .trim()
                .parse::<u8>()
                .map_err(|_| "閾値は0～255の整数です")?;
            let x = value(s.edits[1])
                .trim()
                .parse::<i32>()
                .map_err(|_| "X位置は整数です")?;
            let y = value(s.edits[2])
                .trim()
                .parse::<i32>()
                .map_err(|_| "Y位置は整数です")?;
            s.settings = Settings {
                threshold,
                offset_x: x,
                offset_y: y,
            };
            start(s, false);
        }
        RESET | ALIGN_LEFT | ALIGN_RIGHT | ALIGN_UP | ALIGN_DOWN => {
            if id == RESET {
                s.settings.offset_x = 0;
                s.settings.offset_y = 0;
            } else {
                let n = if unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0 {
                    10
                } else {
                    1
                };
                match id {
                    ALIGN_LEFT => s.settings.offset_x = s.settings.offset_x.saturating_sub(n),
                    ALIGN_RIGHT => s.settings.offset_x = s.settings.offset_x.saturating_add(n),
                    ALIGN_UP => s.settings.offset_y = s.settings.offset_y.saturating_sub(n),
                    _ => s.settings.offset_y = s.settings.offset_y.saturating_add(n),
                }
            }
            sync_edits(s);
            start(s, false);
        }
        HELP => {
            win(yy_ui_win::help::show(
                HELP_MD,
                "yymerge ヘルプ",
                None,
                "yymerge-image-help",
            ))?;
        }
        EXIT => unsafe {
            win(PostMessageW(Some(s.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)))?;
        },
        _ => {}
    }
    Ok(())
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // DestroyWindow dispatches WM_DESTROY synchronously while State may be borrowed.
    if msg == WM_DESTROY {
        unsafe {
            PostQuitMessage(0);
        }
        return LRESULT(0);
    }
    let result = state(|s| match msg {
        WM_DPICHANGED => {
            unsafe {
                let next = create_font(hwnd);
                let old = std::mem::replace(&mut s.font, next);
                for h in &s.controls {
                    SendMessageW(
                        *h,
                        WM_SETFONT,
                        Some(WPARAM(next.0 as usize)),
                        Some(LPARAM(1)),
                    );
                }
                let _ = DeleteObject(old.into());
                if l.0 != 0 {
                    let r = &*(l.0 as *const RECT);
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER,
                    );
                }
            }
            layout(s);
            Some(LRESULT(0))
        }
        WM_SIZE => {
            layout(s);
            Some(LRESULT(0))
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe {
                let dc = BeginPaint(hwnd, &mut ps);
                let r = client(s);
                let memory = CreateCompatibleDC(Some(dc));
                let bitmap = CreateCompatibleBitmap(dc, r.right.max(1), r.bottom.max(1));
                if !memory.is_invalid() && !bitmap.is_invalid() {
                    let old = SelectObject(memory, bitmap.into());
                    paint(s, memory);
                    let _ = BitBlt(dc, 0, 0, r.right, r.bottom, Some(memory), 0, 0, SRCCOPY);
                    SelectObject(memory, old);
                } else {
                    paint(s, dc);
                }
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(memory);
                let _ = EndPaint(hwnd, &ps);
            }
            Some(LRESULT(0))
        }
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_COMMAND => {
            if let Err(e) = command(s, w.0 & 0xffff) {
                status(s, e);
            }
            Some(LRESULT(0))
        }
        WM_TIMER => {
            poll(s);
            if s.mode == Mode::Blink && s.last_blink.elapsed() >= Duration::from_millis(500) {
                s.last_blink = Instant::now();
                s.blink = !s.blink;
                redraw(s);
            }
            if s.smoke.is_some() && s.job.is_none() && !s.smoked {
                s.smoked = true;
                let result = smoke(s);
                if let Err(e) = result {
                    s.exit = 2;
                    let _ = std::fs::write(s.smoke.as_ref().unwrap().join("image-error.txt"), e);
                } else {
                    s.exit = 0;
                }
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            Some(LRESULT(0))
        }
        WM_MOUSEWHEEL => {
            let mut p = POINT {
                x: l.0 as i16 as i32,
                y: (l.0 >> 16) as i16 as i32,
            };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut p);
            }
            zoom(s, 1.15f64.powf((w.0 >> 16) as i16 as f64 / 120.), Some(p));
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN => {
            let p = POINT {
                x: l.0 as i16 as i32,
                y: (l.0 >> 16) as i16 as i32,
            };
            if panels(s)
                .iter()
                .any(|r| p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom)
            {
                s.drag = Some(p);
                unsafe {
                    SetCapture(hwnd);
                    let _ = SetFocus(Some(hwnd));
                }
            }
            Some(LRESULT(0))
        }
        WM_MOUSEMOVE => {
            if let Some(old) = s.drag {
                let p = POINT {
                    x: l.0 as i16 as i32,
                    y: (l.0 >> 16) as i16 as i32,
                };
                s.pan[0] += f64::from(p.x - old.x);
                s.pan[1] += f64::from(p.y - old.y);
                s.drag = Some(p);
                s.fit = false;
                redraw(s);
            }
            Some(LRESULT(0))
        }
        WM_LBUTTONUP | WM_CAPTURECHANGED => {
            s.drag = None;
            if msg == WM_LBUTTONUP {
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            Some(LRESULT(0))
        }
        WM_CLOSE => {
            if let Some(job) = &s.job {
                job.cancel.cancel();
                s.exit = 130;
            }
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            Some(LRESULT(0))
        }
        _ => None,
    })
    .flatten();
    result.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, w, l) })
}
pub(super) fn run(g: Gui) -> Result<i32> {
    if g.inputs.active() {
        return Err("画像比較では文字コード指定を使用できません".into());
    }
    if g.base.is_some()
        || g.output.is_some()
        || g.encoding.is_some()
        || g.rules.core().active()
        || g.rules.rules_profile.is_some()
        || g.git_merge
        || g.git_left_missing
        || g.git_right_missing
    {
        return Err(
            "画像比較ではマージ・文字コード・テキスト比較規則・欠落したGit入力は使えません".into(),
        );
    }
    let paths = [
        g.left.ok_or("LEFT画像が必要です")?,
        g.right.ok_or("RIGHT画像が必要です")?,
    ];
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        win(CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok())?;
        yy_ui_win::font::register_gdi();
        let instance = win(GetModuleHandleW(None))?;
        let wc = WNDCLASSW {
            lpszClassName: CLASS,
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            hIcon: LoadIconW(
                Some(instance.into()),
                PCWSTR(std::ptr::without_provenance(1)),
            )
            .unwrap_or_default(),
            hCursor: win(LoadCursorW(None, IDC_ARROW))?,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err("image window registration failed".into());
        }
        let hwnd = win(CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            w!("yymerge — 画像・PDF比較（読み取り専用）"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1180,
            760,
            None,
            None,
            Some(instance.into()),
            None,
        ))?;
        let font = create_font(hwnd);
        let menu = win(CreateMenu())?;
        for (title, items) in [
            (
                "ファイル(&F)",
                vec![
                    (OPEN, "画像・PDF・Officeを開く…"),
                    (SAVE, "差分PNGを保存…"),
                    (HTML, "HTMLレポートを保存…"),
                    (JSON, "JSONレポートを保存…"),
                    (EXIT, "閉じる"),
                ],
            ),
            (
                "表示(&V)",
                vec![
                    (FIT, "全体に合わせる\t0"),
                    (ACTUAL, "100%\t1"),
                    (PLUS, "拡大\t+"),
                    (MINUS, "縮小\t-"),
                    (DIFF, "差分"),
                    (OVERLAY, "重ね合わせ（50%）"),
                    (BLINK, "点滅切り替え"),
                    (PREVIOUS_PAGE, "前のページ／スライド\tPageUp"),
                    (NEXT_PAGE, "次のページ／スライド\tPageDown"),
                ],
            ),
            (
                "位置合わせ(&A)",
                vec![
                    (ALIGN_LEFT, "右画像を左へ\tAlt+←"),
                    (ALIGN_RIGHT, "右画像を右へ\tAlt+→"),
                    (ALIGN_UP, "右画像を上へ\tAlt+↑"),
                    (ALIGN_DOWN, "右画像を下へ\tAlt+↓"),
                    (RESET, "右画像の位置を戻す"),
                ],
            ),
            ("ヘルプ(&H)", vec![(HELP, "yymergeの使い方\tF1")]),
        ] {
            let popup = win(CreatePopupMenu())?;
            for (id, label) in items {
                win(AppendMenuW(popup, MF_STRING, id, &HSTRING::from(label)))?;
            }
            win(AppendMenuW(
                menu,
                MF_POPUP,
                popup.0 as usize,
                &HSTRING::from(title),
            ))?;
        }
        win(SetMenu(hwnd, Some(menu)))?;
        let mut controls = Vec::new();
        let mut edits = Vec::new();
        for (i, (class, label, width)) in [
            (w!("STATIC"), "閾値", 0),
            (w!("EDIT"), "0", 1),
            (w!("STATIC"), "右X", 0),
            (w!("EDIT"), "0", 1),
            (w!("STATIC"), "右Y", 0),
            (w!("EDIT"), "0", 1),
            (w!("BUTTON"), "適用", 2),
            (w!("BUTTON"), "全体", 2),
            (w!("BUTTON"), "100%", 2),
        ]
        .into_iter()
        .enumerate()
        {
            let id = match i {
                6 => APPLY,
                7 => FIT,
                8 => ACTUAL,
                _ => 500 + i,
            };
            let style = WS_CHILD
                | WS_VISIBLE
                | if width == 1 {
                    WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32)
                } else if width == 2 {
                    WS_TABSTOP
                } else {
                    WINDOW_STYLE::default()
                };
            let h = win(CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                &HSTRING::from(label),
                style,
                0,
                0,
                1,
                1,
                Some(hwnd),
                Some(HMENU(std::ptr::without_provenance_mut(id))),
                Some(instance.into()),
                None,
            ))?;
            SendMessageW(
                h,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
            if width == 1 {
                SendMessageW(
                    h,
                    windows::Win32::UI::Controls::EM_SETLIMITTEXT,
                    Some(WPARAM(8)),
                    None,
                );
                edits.push(h);
            }
            controls.push(h);
        }
        let s = State {
            hwnd,
            font,
            controls,
            edits: edits.try_into().map_err(|_| "image controls missing")?,
            top: 0,
            paths,
            labels: [g.left_label, g.right_label],
            settings: Default::default(),
            mode: Mode::Difference,
            display: None,
            job: None,
            generation: 0,
            scale: 1.,
            pan: [0.; 2],
            fit: true,
            drag: None,
            blink: false,
            last_blink: Instant::now(),
            status: String::new(),
            exit: 130,
            smoke: g.ui_smoke_test,
            smoked: false,
        };
        STATE.with(|slot| *slot.borrow_mut() = Some(s));
        state(|s| {
            layout(s);
            start(s, true);
        });
        let _ = ShowWindow(hwnd, SW_SHOW);
        SetTimer(Some(hwnd), 1, 25, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if yy_ui_win::help::translate(&msg) {
                continue;
            }
            if matches!(msg.message, WM_KEYDOWN | WM_SYSKEYDOWN) {
                let editing = state(|s| s.edits.contains(&GetFocus())).unwrap_or(false);
                let key = msg.wParam.0;
                let alt = GetKeyState(VK_MENU.0 as i32) < 0;
                let id = if key == VK_F1.0 as usize {
                    Some(HELP)
                } else if editing {
                    (key == 13).then_some(APPLY)
                } else {
                    match key {
                        48 | 96 => Some(FIT),
                        49 | 97 => Some(ACTUAL),
                        187 | 107 => Some(PLUS),
                        189 | 109 => Some(MINUS),
                        33 => Some(PREVIOUS_PAGE),
                        34 => Some(NEXT_PAGE),
                        37 if alt => Some(ALIGN_LEFT),
                        39 if alt => Some(ALIGN_RIGHT),
                        38 if alt => Some(ALIGN_UP),
                        40 if alt => Some(ALIGN_DOWN),
                        _ => None,
                    }
                };
                if key == 27 {
                    state(|s| {
                        if s.job.take().is_none() {
                            return;
                        }
                        if let Some(d) = &s.display {
                            s.settings = d.summary.settings;
                            s.mode = d.mode;
                            sync_edits(s);
                        }
                        s.exit = 130;
                        status(s, "処理を中止しました。表示は最後に完了した結果です");
                    });
                    continue;
                }
                if let Some(id) = id {
                    state(|s| {
                        if let Err(e) = command(s, id) {
                            status(s, e);
                        }
                    });
                    continue;
                }
            }
            if IsDialogMessageW(hwnd, &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        yy_ui_win::help::close();
        let s = STATE
            .with(|slot| slot.borrow_mut().take())
            .ok_or("image state missing")?;
        let exit = s.exit;
        let _ = DeleteObject(s.font.into());
        drop(s);
        let _ = UnregisterClassW(CLASS, Some(instance.into()));
        CoUninitialize();
        Ok(exit)
    }
}
pub(super) fn spawn(left: &Path, right: &Path) -> Result<()> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command = std::process::Command::new(&exe);
    if exe
        .file_stem()
        .is_none_or(|s| !s.to_string_lossy().eq_ignore_ascii_case("yymergew"))
    {
        command.arg("gui");
    }
    let mut child = command
        .arg("--image")
        .arg("--left")
        .arg(left)
        .arg("--right")
        .arg(right)
        .spawn()
        .map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
fn smoke(s: &mut State) -> Result<()> {
    let dir = s.smoke.clone().ok_or("smoke directory missing")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if s.display.as_ref().is_none_or(|d| d.summary.equivalent) {
        return Err(format!(
            "image review did not finish with differences: {}",
            s.status
        ));
    }
    let c = Cancellation::new();
    let pair = s.display.as_ref().unwrap().pair.clone();
    if let Some(docs) = &pair.documents {
        if docs.pages() > 1 {
            for (id, expected) in [(NEXT_PAGE, 1), (PREVIOUS_PAGE, 0)] {
                command(s, id)?;
                let deadline = Instant::now() + Duration::from_secs(60);
                while s.job.is_some() {
                    poll(s);
                    if Instant::now() > deadline {
                        return Err("Office page navigation timed out".into());
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                if s.display.as_ref().is_none_or(|d| d.pair.page != expected) {
                    return Err(format!("Office page navigation failed: {}", s.status));
                }
            }
        }
        let pdf = yy_merge_core::office_render::is_pdf(&pair.left.source.path);
        std::fs::write(dir.join(if pdf { "pdf-smoke.txt" } else { "office-image-smoke.txt" }), format!("PASS: {} input, cached PDF page navigation, missing-page-aware comparison, original-source verification\n", if pdf { "direct PDF" } else { "real Office rendering" })).map_err(|e| e.to_string())?;
    }
    for (w, h) in [(680, 420), (1180, 760)] {
        unsafe {
            win(SetWindowPos(
                s.hwnd,
                None,
                0,
                0,
                w,
                h,
                SWP_NOMOVE | SWP_NOZORDER,
            ))?;
        }
        layout(s);
        let area = client(s);
        for panel in panels(s) {
            if panel.left < 0
                || panel.right > area.right
                || panel.bottom > area.bottom
                || panel.bottom <= panel.top
            {
                return Err("image panels outside resized client".into());
            }
        }
    }
    let p = panels(s)[1];
    let anchor = POINT {
        x: p.left + 30,
        y: p.top + 30,
    };
    let before = [(30. - s.pan[0]) / s.scale, (30. - s.pan[1]) / s.scale];
    zoom(s, 1.5, Some(anchor));
    if ((30. - s.pan[0]) / s.scale - before[0]).abs() > 1e-8
        || ((30. - s.pan[1]) / s.scale - before[1]).abs() > 1e-8
    {
        return Err("image mouse zoom moved its anchor".into());
    }
    for mode in [Mode::Difference, Mode::Overlay, Mode::Blink] {
        let Completed::Display(d) = calculate(pair.clone(), s.settings, mode, &c)? else {
            return Err("image display not produced".into());
        };
        s.mode = mode;
        s.display = Some(d);
    }
    s.settings = Settings::default();
    s.mode = Mode::Difference;
    start(s, false);
    let previous = s.job.as_ref().unwrap().cancel.clone();
    s.settings.threshold = 255;
    start(s, false);
    if previous.check().is_ok() {
        return Err("superseded image job was not cancelled".into());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while s.job.is_some() {
        poll(s);
        if Instant::now() > deadline {
            return Err("image threshold worker timeout".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    if s.display.as_ref().unwrap().summary.settings.threshold != 255
        || !s.display.as_ref().unwrap().summary.equivalent
    {
        return Err("stale image threshold result applied".into());
    }
    text(s.edits[0], "invalid");
    if command(s, APPLY).is_ok() {
        return Err("invalid image threshold was accepted".into());
    }
    s.settings = Default::default();
    sync_edits(s);
    s.mode = Mode::Difference;
    start(s, false);
    let deadline = Instant::now() + Duration::from_secs(5);
    while s.job.is_some() {
        poll(s);
        if Instant::now() > deadline {
            return Err("image restore worker timeout".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    fit(s);
    let dc = unsafe { GetDC(Some(s.hwnd)) };
    let mem = unsafe { CreateCompatibleDC(Some(dc)) };
    let area = client(s);
    let bitmap = unsafe { CreateCompatibleBitmap(dc, area.right, area.bottom) };
    let old = unsafe { SelectObject(mem, bitmap.into()) };
    paint(s, mem);
    let panel = panels(s)[2];
    let pixel = unsafe {
        GetPixel(
            mem,
            (panel.left as f64 + s.pan[0] + 40. * s.scale).round() as i32,
            (panel.top as f64 + s.pan[1] + 50. * s.scale).round() as i32,
        )
    };
    let mut capture = vec![0u8; area.right as usize * area.bottom as usize * 4];
    unsafe {
        SelectObject(mem, old);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: area.right,
                biHeight: -area.bottom,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        if GetDIBits(
            dc,
            bitmap,
            0,
            area.bottom as u32,
            Some(capture.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        ) != area.bottom
        {
            return Err("image render capture failed".into());
        }
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(mem);
        ReleaseDC(Some(s.hwnd), dc);
    }
    for p in capture.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
        p[3] = 255;
    }
    std::fs::write(
        dir.join("image-render.png"),
        images::png(
            &Raster::new(area.right as u32, area.bottom as u32, capture)?,
            &c,
        )?,
    )
    .map_err(|e| e.to_string())?;
    if pair.documents.is_none() && pixel != COLORREF(0x00a000ff) {
        return Err(format!(
            "image difference was not drawn magenta at expected coordinates: {pixel:?}"
        ));
    }
    pair.left.source.verify(&c)?;
    pair.right.source.verify(&c)?;
    let d = s.display.as_ref().unwrap();
    let bytes = images::png(
        &images::compare(
            &pair.left.raster,
            &pair.right.raster,
            d.summary.settings,
            &c,
        )?
        .difference,
        &c,
    )?;
    yy_merge_core::files::save(
        &dir.join("difference.png"),
        &bytes,
        &[&pair.left.source, &pair.right.source],
        true,
        &c,
    )?;
    win(yy_ui_win::help::show(
        HELP_MD,
        "yymerge 画像比較ヘルプ",
        None,
        "yymerge-image-smoke-help",
    ))?;
    yy_ui_win::help::close();
    std::fs::write(dir.join("image-smoke.txt"),"PASS: worker image load and threshold recompute, superseded generation cancelled, invalid threshold rejected, three native image panels, resize 680x420/1180x760, mouse-anchored synchronized zoom, difference/overlay/blink preparation, GDI magenta pixel and render capture, PNG output, source verification, help\n").map_err(|e|e.to_string())
}

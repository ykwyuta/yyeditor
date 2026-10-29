//! Background totals, short-lived cache and native occupancy cells.
use super::*;
use std::{
    sync::mpsc,
    thread::JoinHandle,
    time::{Duration, Instant},
};
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::*;
use yy_files::folder_size::Size;

struct Cached {
    value: Size,
    generation: u64,
    at: Instant,
}
struct Update {
    generation: u64,
    values: HashMap<PathBuf, Size>,
    done: bool,
    failed: bool,
}
pub(super) struct Model {
    cache: HashMap<PathBuf, Cached>,
    tx: mpsc::SyncSender<Update>,
    rx: mpsc::Receiver<Update>,
    cancel: Option<Arc<AtomicBool>>,
    threads: Vec<JoinHandle<()>>,
    pub(super) running: bool,
    failed: bool,
    stopped: bool,
    root: Option<PathBuf>,
}
impl Default for Model {
    fn default() -> Self {
        let (tx, rx) = mpsc::sync_channel(8);
        Self {
            cache: HashMap::new(),
            tx,
            rx,
            cancel: None,
            threads: Vec::new(),
            running: false,
            failed: false,
            stopped: false,
            root: None,
        }
    }
}
impl Model {
    pub(super) fn status_note(&self) -> Option<String> {
        self.root.as_ref()?;
        if self.stopped {
            return Some("サイズ計算を中止（F5で再計算）".into());
        }
        if self.failed {
            return Some("サイズ未取得（F5で再計算）".into());
        }
        let value = &self.cache.get(self.root.as_ref()?)?.value;
        if value.errors > 0 {
            return Some(format!("サイズは一部未集計（{}件）", value.errors));
        }
        if value.skipped_mounts > 0 {
            return Some("WSLの仮想領域を集計から除外".into());
        }
        None
    }
    pub(super) fn cancel(&mut self) {
        self.stopped |= self.running;
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.running = false;
    }
    pub(super) fn deactivate(&mut self) {
        self.root = None;
        self.failed = false;
        self.stopped = false;
    }
    pub(super) fn stop(&mut self) {
        self.cancel();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
    pub(super) fn start(&mut self, hwnd: isize, generation: u64, root: PathBuf) {
        self.cancel();
        self.cache
            .retain(|_, c| c.at.elapsed() < Duration::from_secs(30));
        let mut active = Vec::new();
        for thread in self.threads.drain(..) {
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                active.push(thread);
            }
        }
        self.threads = active;
        self.root = Some(root.clone());
        self.running = true;
        self.failed = false;
        self.stopped = false;
        let flag = Arc::new(AtomicBool::new(false));
        self.cancel = Some(flag.clone());
        let tx = self.tx.clone();
        self.threads.push(std::thread::spawn(move || {
            let mut values = HashMap::new();
            let mut last = Instant::now();
            let send = |values, done, failed| {
                let mut pending = Update {
                    generation,
                    values,
                    done,
                    failed,
                };
                loop {
                    if flag.load(Ordering::Relaxed) {
                        return;
                    }
                    match tx.try_send(pending) {
                        Ok(()) => break,
                        Err(mpsc::TrySendError::Full(value)) => {
                            pending = value;
                            std::thread::park_timeout(Duration::from_millis(2));
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => return,
                    }
                }
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(hwnd as *mut _)),
                        WM_APP_SIZES,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            };
            let result = yy_files::folder_size::scan_with(
                &yy_files::Local,
                &root,
                &|path| super::super::wsl::exclude_size_mount(&root, path),
                &|| flag.load(Ordering::Relaxed),
                |path, value| {
                    values.insert(path.to_owned(), value);
                    if values.len() >= 128 || last.elapsed() >= Duration::from_millis(100) {
                        send(std::mem::take(&mut values), false, false);
                        last = Instant::now();
                    }
                },
            );
            send(
                values,
                true,
                result.as_ref().is_err_and(|e| !yy_files::is_cancelled(e)),
            );
        }));
    }
    pub(super) fn poll(&mut self, generation: u64) -> bool {
        let mut changed = false;
        for update in self.rx.try_iter() {
            if update.generation != generation {
                continue;
            }
            changed = true;
            for (path, value) in update.values {
                let direct = self
                    .root
                    .as_ref()
                    .is_some_and(|root| path == *root || path.parent() == Some(root));
                if self.cache.len() < 50_000 || direct || self.cache.contains_key(&path) {
                    self.cache.insert(
                        path,
                        Cached {
                            value,
                            generation,
                            at: Instant::now(),
                        },
                    );
                }
            }
            if update.done {
                self.running = false;
                self.failed = update.failed;
            }
        }
        changed
    }
}
impl Drop for Model {
    fn drop(&mut self) {
        self.stop();
    }
}

impl State {
    pub(super) fn share_text(&self, row: &Row) -> String {
        self.share(row)
            .map(|(percent, approximate)| {
                format!("{}{percent:.1}%", if approximate { "≈" } else { "" })
            })
            .unwrap_or_else(|| {
                if self.sizes.running
                    && row.attrs & ATTR_REPARSE == 0
                    && !self
                        .sizes
                        .cache
                        .get(&row.path)
                        .is_some_and(|c| c.value.excluded)
                {
                    "計算中…".into()
                } else {
                    "—".into()
                }
            })
    }
    pub(super) fn fit_size_columns(&self) {
        if !matches!(self.loc, Loc::Folder(_)) || !self.cols.contains(&Col::Share) {
            return;
        }
        unsafe {
            let mut rect = RECT::default();
            let _ = GetClientRect(self.list, &mut rect);
            let available = (rect.right - self.d(20)).max(self.d(280));
            let total = self.d(104);
            let share = self.d(104);
            let name = (available - total - share).clamp(self.d(64), self.d(280));
            for (column, width) in [
                (Col::Name, name),
                (Col::FolderTotal, total),
                (Col::Share, share),
            ] {
                if let Some(index) = self.cols.iter().position(|c| *c == column) {
                    SendMessageW(
                        self.list,
                        LVM_SETCOLUMNWIDTH,
                        Some(WPARAM(index)),
                        Some(LPARAM(width as isize)),
                    );
                }
            }
        }
    }
    pub(super) fn effective_size(&self, row: &Row) -> Option<u64> {
        if row.dir && row.entry.is_none() && row.drive.is_none() {
            self.sizes
                .cache
                .get(&row.path)
                .map(|cached| cached.value.bytes)
        } else {
            Some(row.size)
        }
    }
    pub(super) fn size_text(&self, row: &Row) -> String {
        if row.attrs & ATTR_REPARSE != 0 {
            return "リンク".into();
        }
        if !row.dir {
            return crate::util::human_size(row.size);
        }
        match self.sizes.cache.get(&row.path) {
            Some(c) if c.value.excluded => "集計外".into(),
            Some(c) => format!(
                "{}{}",
                if c.value.errors > 0 {
                    "≥"
                } else if !c.value.complete || c.generation != self.generation {
                    "≈"
                } else {
                    ""
                },
                crate::util::human_size(c.value.bytes)
            ),
            None if self.sizes.running => "計算中…".into(),
            None => "集計できません".into(),
        }
    }
    pub(super) fn share(&self, row: &Row) -> Option<(f64, bool)> {
        if row.attrs & ATTR_REPARSE != 0 {
            return None;
        }
        if self
            .sizes
            .cache
            .get(&row.path)
            .is_some_and(|c| c.value.excluded)
        {
            return None;
        }
        let root = self.sizes.root.as_ref()?;
        let root = self.sizes.cache.get(root)?;
        let bytes = self.effective_size(row)?;
        let percent = if root.value.bytes == 0 {
            0.0
        } else {
            (bytes as f64 * 100.0 / root.value.bytes as f64).clamp(0.0, 100.0)
        };
        Some((
            percent,
            !root.value.complete
                || root.generation != self.generation
                || root.value.errors > 0
                || self.sizes.failed,
        ))
    }
}

pub(super) fn draw(lparam: LPARAM) -> LRESULT {
    unsafe {
        let nm = &*(lparam.0 as *const NMLVCUSTOMDRAW);
        let stage = nm.nmcd.dwDrawStage;
        if stage == CDDS_PREPAINT {
            return LRESULT(CDRF_NOTIFYITEMDRAW as isize);
        }
        if stage == CDDS_ITEMPREPAINT {
            return LRESULT(CDRF_NOTIFYSUBITEMDRAW as isize);
        }
        if stage.0 != (CDDS_ITEMPREPAINT.0 | CDDS_SUBITEM.0) {
            return LRESULT(CDRF_DODEFAULT as isize);
        }
        let cell = with(|s| {
            if s.cols.get(nm.iSubItem as usize) != Some(&Col::Share) {
                return None;
            }
            let row = s.rows.get(*s.view.get(nm.nmcd.dwItemSpec)?)?;
            Some((
                s.list,
                s.share(row),
                s.cell(nm.nmcd.dwItemSpec, nm.iSubItem as usize),
                s.d(4),
            ))
        })
        .flatten();
        let Some((list, share, text, inset)) = cell else {
            return LRESULT(CDRF_DODEFAULT as isize);
        };
        let mut rect = RECT {
            top: nm.iSubItem,
            left: LVIR_BOUNDS as i32,
            ..Default::default()
        };
        SendMessageW(
            list,
            LVM_GETSUBITEMRECT,
            Some(WPARAM(nm.nmcd.dwItemSpec)),
            Some(LPARAM(&mut rect as *mut _ as isize)),
        );
        let selected = nm.nmcd.uItemState.0 & CDIS_SELECTED.0 != 0;
        let bg = if selected {
            COLOR_HIGHLIGHT
        } else {
            COLOR_WINDOW
        };
        FillRect(nm.nmcd.hdc, &rect, GetSysColorBrush(bg));
        rect.left += inset;
        rect.right -= inset;
        rect.top += inset;
        rect.bottom -= inset;
        if let Some((percent, _)) = share {
            let mut bar = rect;
            bar.right = bar.left + ((bar.right - bar.left) as f64 * percent / 100.0) as i32;
            let brush = CreateSolidBrush(if selected {
                COLORREF(0x00a66d38)
            } else {
                COLORREF(0x00e9d9bd)
            });
            FillRect(nm.nmcd.hdc, &bar, brush);
            let _ = DeleteObject(brush.into());
        }
        let old_mode = SetBkMode(nm.nmcd.hdc, TRANSPARENT);
        let old_color = SetTextColor(
            nm.nmcd.hdc,
            COLORREF(GetSysColor(if selected {
                COLOR_HIGHLIGHTTEXT
            } else {
                COLOR_WINDOWTEXT
            })),
        );
        let mut chars: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(
            nm.nmcd.hdc,
            &mut chars,
            &mut rect,
            DT_SINGLELINE | DT_VCENTER | DT_RIGHT | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        SetTextColor(nm.nmcd.hdc, old_color);
        SetBkMode(nm.nmcd.hdc, BACKGROUND_MODE(old_mode as u32));
        LRESULT(CDRF_SKIPDEFAULT as isize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::{
        Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
        LibraryLoader::GetModuleHandleW,
    };
    fn pump_until(predicate: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if predicate() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "browse timeout: {:?}",
                with(|s| (
                    s.loading,
                    s.sizes.running,
                    s.generation,
                    s.rows.len(),
                    s.sizes.status_note()
                ))
            );
            unsafe {
                MsgWaitForMultipleObjectsEx(None, 20, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            }
        }
    }
    #[test]
    fn native_totals_bars_sorting_navigation_and_small_window_are_async() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("files");
        let config = temp.path().join("config");
        std::fs::create_dir_all(root.join("a/nested")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        std::fs::create_dir(&config).unwrap();
        std::fs::write(root.join("a/file"), [0; 192]).unwrap();
        std::fs::write(root.join("a/nested/.hidden"), [0; 64]).unwrap();
        std::fs::write(root.join("b/file"), [0; 64]).unwrap();
        std::fs::write(root.join("direct"), [0; 64]).unwrap();
        let dirs = Dirs::new(config);
        let state = BrowseState {
            last: root.to_string_lossy().into_owned(),
            ..BrowseState::default()
        };
        state.save(&dirs.browse_file()).unwrap();
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_LISTVIEW_CLASSES | ICC_TREEVIEW_CLASSES,
            })
            .unwrap();
            let parent = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("folder size fixture"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                660,
                480,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let font = HFONT(GetStockObject(DEFAULT_GUI_FONT).0);
            let hwnd = create(
                parent,
                GetModuleHandleW(None).unwrap().into(),
                font,
                96,
                dirs,
                |_| {},
            );
            struct Frame(HWND);
            impl Drop for Frame {
                fn drop(&mut self) {
                    shutdown();
                    unsafe {
                        let mut msg = MSG::default();
                        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                            DispatchMessageW(&msg);
                        }
                        let _ = DestroyWindow(self.0);
                        CoUninitialize();
                    }
                    STATE.with(|s| s.borrow_mut().take());
                }
            }
            let _frame = Frame(parent);
            let _ = ShowWindow(hwnd, SW_SHOW);
            MoveWindow(hwnd, 0, 0, 640, 420, true).unwrap();
            pump_until(|| {
                with(|s| !s.loading && !s.sizes.running && s.rows.len() == 3).unwrap_or(false)
            });
            with(|s| {
                let a = s.rows.iter().find(|r| r.name == "a").unwrap();
                assert_eq!(s.effective_size(a), Some(256));
                assert!((s.share(a).unwrap().0 - 66.666666).abs() < 0.001);
                assert!(!s.share(a).unwrap().1);
                assert!(s.cols.contains(&Col::FolderTotal) && s.cols.contains(&Col::Share));
                let generation = s.generation;
                s.sizes
                    .tx
                    .send(Update {
                        generation: generation - 1,
                        values: HashMap::from([(
                            a.path.clone(),
                            Size {
                                bytes: 999,
                                ..Size::default()
                            },
                        )]),
                        done: true,
                        failed: false,
                    })
                    .unwrap();
                assert!(!s.sizes.poll(generation));
                s.sort = (Col::FolderTotal, false);
                s.rebuild_view();
                assert_eq!(s.rows[s.view[0]].name, "a");
                for width in [640, 1100] {
                    MoveWindow(hwnd, 0, 0, width, 420, true).unwrap();
                    s.layout();
                    let mut list = RECT::default();
                    GetClientRect(s.list, &mut list).unwrap();
                    let used: i32 = (0..3)
                        .map(|i| {
                            SendMessageW(s.list, LVM_GETCOLUMNWIDTH, Some(WPARAM(i)), None).0 as i32
                        })
                        .sum();
                    assert!(
                        used <= list.right,
                        "size/share columns must fit at {width}px: {used} > {}",
                        list.right
                    );
                }
            });
            // Paint the actual occupancy subitem into a memory DC. Verify both
            // the filled part and its unfilled background, beyond text checks.
            let list = with(|s| s.list).unwrap();
            let screen = GetDC(Some(list));
            let dc = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, 1200, 500);
            ReleaseDC(Some(list), screen);
            let previous = SelectObject(dc, bitmap.into());
            struct Surface(HDC, HBITMAP, HGDIOBJ);
            impl Drop for Surface {
                fn drop(&mut self) {
                    unsafe {
                        SelectObject(self.0, self.2);
                        let _ = DeleteObject(self.1.into());
                        let _ = DeleteDC(self.0);
                    }
                }
            }
            let _surface = Surface(dc, bitmap, previous);
            let mut custom = NMLVCUSTOMDRAW::default();
            custom.nmcd.hdc = dc;
            custom.nmcd.dwDrawStage = NMCUSTOMDRAW_DRAW_STAGE(CDDS_ITEMPREPAINT.0 | CDDS_SUBITEM.0);
            custom.nmcd.dwItemSpec = 0;
            custom.iSubItem = 2;
            assert_eq!(
                draw(LPARAM(&custom as *const _ as isize)),
                LRESULT(CDRF_SKIPDEFAULT as isize)
            );
            let mut cell = RECT {
                left: LVIR_BOUNDS as i32,
                top: 2,
                ..Default::default()
            };
            SendMessageW(
                list,
                LVM_GETSUBITEMRECT,
                Some(WPARAM(0)),
                Some(LPARAM(&mut cell as *mut _ as isize)),
            );
            assert_ne!(
                GetPixel(dc, cell.left + 6, cell.top + 5),
                COLORREF(GetSysColor(COLOR_WINDOW))
            );
            assert_eq!(
                GetPixel(dc, cell.right - 2, cell.top + 5),
                COLORREF(GetSysColor(COLOR_WINDOW))
            );
            go(Loc::Folder(root.join("a")), true);
            pump_until(|| {
                with(|s| {
                    !s.loading
                        && !s.sizes.running
                        && s.sizes
                            .cache
                            .get(&root.join("a"))
                            .is_some_and(|c| c.value.complete && c.value.bytes == 256)
                })
                .unwrap_or(false)
            });
            back();
            pump_until(|| {
                with(|s| {
                    s.loc == Loc::Folder(root.clone())
                        && !s.loading
                        && !s.sizes.running
                        && s.watch_ready.load(Ordering::Acquire)
                })
                .unwrap_or(false)
            });
            std::fs::write(root.join("a/nested/.hidden"), [0; 128]).unwrap();
            pump_until(|| {
                with(|s| {
                    !s.loading
                        && !s.sizes.running
                        && s.sizes
                            .cache
                            .get(&root.join("a"))
                            .is_some_and(|c| c.value.bytes == 320)
                })
                .unwrap_or(false)
            });
        }
    }
}

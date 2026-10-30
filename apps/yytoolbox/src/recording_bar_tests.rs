use super::*;
use windows::Win32::System::SystemServices::SS_WHITERECT;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};

struct Window(HWND);
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}
fn owner() -> Window {
    Window(unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("recording test owner"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
        .unwrap()
    })
}

#[test]
fn bar_fits_dpi_and_hotkeys_and_stop_notifies_owner_once() {
    let owner = owner();
    for key in [
        "なし（ボタンのみ）",
        "Ctrl+Shift+F9",
        "Ctrl+Shift+F10",
        "Ctrl+Shift+F11",
        "Ctrl+Alt+F9",
        "Ctrl+Alt+F10",
    ] {
        for audio_only in [false, true] {
            let bar = ControlBar::new(owner.0, 3600, key, 103, audio_only).unwrap();
            unsafe {
                let _ = ShowWindow(bar.0, SW_HIDE);
                let s = data(bar.0).unwrap();
                for dpi in [96, 120, 144, 192] {
                    s.dpi.set(dpi);
                    MoveWindow(
                        bar.0,
                        0,
                        0,
                        240 * dpi as i32 / 96,
                        104 * dpi as i32 / 96,
                        false,
                    )
                    .unwrap();
                    layout(bar.0, true);
                    SendMessageW(bar.0, WM_TIMER, None, None);
                    for h in [s.status, s.hint, s.stop] {
                        let mut client = RECT::default();
                        GetClientRect(h, &mut client).unwrap();
                        let length = GetWindowTextLengthW(h) as usize;
                        let mut text = vec![0u16; length + 1];
                        GetWindowTextW(h, &mut text);
                        let dc = GetDC(Some(h));
                        let old = SelectObject(dc, s.font.get().into());
                        let mut extent = SIZE::default();
                        GetTextExtentPoint32W(dc, &text[..length], &mut extent).unwrap();
                        SelectObject(dc, old);
                        ReleaseDC(Some(h), dc);
                        assert!(
                            extent.cx <= client.right && extent.cy <= client.bottom,
                            "{key} DPI={dpi}: {extent:?} {client:?}"
                        );
                    }
                }
                SendMessageW(s.stop, BM_CLICK, None, None);
                SendMessageW(s.stop, BM_CLICK, None, None);
                let mut message = MSG::default();
                assert!(
                    PeekMessageW(
                        &mut message,
                        Some(owner.0),
                        WM_COMMAND,
                        WM_COMMAND,
                        PM_REMOVE
                    )
                    .as_bool()
                );
                assert_eq!(message.wParam.0, 103);
                assert!(
                    !PeekMessageW(
                        &mut message,
                        Some(owner.0),
                        WM_COMMAND,
                        WM_COMMAND,
                        PM_REMOVE
                    )
                    .as_bool()
                );
                assert!(!IsWindowEnabled(s.stop).as_bool());
                let handle = bar.0;
                drop(bar);
                assert!(!IsWindow(Some(handle)).as_bool());
            }
        }
    }
}

#[test]
#[ignore = "shows a visible capture-excluded bar and records a test background, then clicks Stop"]
fn real_visible_bar_is_excluded_and_button_saves_short_mp4() {
    struct Dpi(DPI_AWARENESS_CONTEXT, HWND);
    impl Drop for Dpi {
        fn drop(&mut self) {
            unsafe {
                SetThreadDpiAwarenessContext(self.0);
                let _ = SetForegroundWindow(self.1);
            }
        }
    }
    let _dpi = Dpi(
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) },
        unsafe { GetForegroundWindow() },
    );
    let _com = Com::new().unwrap();
    let _media = Media::new().unwrap();
    let owner = owner();
    unsafe {
        let mut monitor = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        assert!(
            GetMonitorInfoW(
                MonitorFromWindow(owner.0, MONITOR_DEFAULTTONEAREST),
                &mut monitor
            )
            .as_bool()
        );
        let work = monitor.rcWork;
        let _canvas = Window(
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("capture exclusion test background"),
                WS_POPUP | WS_VISIBLE | WINDOW_STYLE(SS_WHITERECT.0),
                work.left,
                work.top,
                work.right - work.left,
                work.bottom - work.top,
                None,
                None,
                None,
                None,
            )
            .unwrap(),
        );
        let _ = UpdateWindow(_canvas.0);
        let bar = ControlBar::new(owner.0, 10, "なし（ボタンのみ）", 103, false).unwrap();
        assert!(IsWindowVisible(bar.0).as_bool());
        let mut rect = RECT::default();
        GetWindowRect(bar.0, &mut rect).unwrap();
        let desktop = Desktop::new(
            (rect.right - rect.left) as u32,
            (rect.bottom - rect.top) as u32,
        )
        .unwrap();
        // Positive control: the same visible bar appears when exclusion is off.
        SetWindowDisplayAffinity(bar.0, WDA_NONE).unwrap();
        windows::Win32::Graphics::Dwm::DwmFlush().unwrap();
        let shown = desktop.frame(rect).unwrap().to_vec();
        SetWindowDisplayAffinity(bar.0, WDA_EXCLUDEFROMCAPTURE).unwrap();
        windows::Win32::Graphics::Dwm::DwmFlush().unwrap();
        let excluded = desktop.frame(rect).unwrap().to_vec();
        assert_ne!(
            shown, excluded,
            "exclusion must remove a visibly rendered bar"
        );
        let center = ((desktop.height / 2 * desktop.width + desktop.width / 2) * 4) as usize;
        assert!(
            shown[center..center + 3].iter().any(|c| *c < 180),
            "positive control must sample the bar background"
        );
        assert!(excluded[center..center + 3].iter().all(|c| *c > 235));
        let files = tempfile::tempdir().unwrap();
        let path = files.path().join("button-stop.mp4");
        let mut params = yytoolbox::Tool::Recording.defaults();
        params.insert("seconds".into(), "10".into());
        let plan = Plan::new(path.clone(), rect, &params).unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        let stop_flag = flag.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        struct Source {
            desktop: Desktop,
            count: Cell<u32>,
            ready: mpsc::SyncSender<()>,
        }
        impl FrameSource for Source {
            fn frame(&self, rect: RECT) -> Result<&[u8]> {
                let frame = self.desktop.frame(rect)?;
                self.count.set(self.count.get() + 1);
                if self.count.get() == 20 {
                    self.ready.send(()).map_err(|e| e.to_string())?;
                }
                Ok(frame)
            }
        }
        let worker = std::thread::spawn(move || {
            let _ = done_tx.send(record_with_source(plan, &stop_flag, |w, h| {
                Ok(Source {
                    desktop: Desktop::new(w, h)?,
                    count: Cell::new(0),
                    ready: ready_tx,
                })
            }));
        });
        struct Worker(Arc<AtomicBool>, Option<JoinHandle<()>>);
        impl Drop for Worker {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
                if let Some(thread) = self.1.take() {
                    thread.join().unwrap();
                }
            }
        }
        let worker = Worker(flag.clone(), Some(worker));
        ready_rx
            .recv_timeout(Duration::from_secs(8))
            .expect("20 captured frames were not produced");
        SendMessageW(data(bar.0).unwrap().stop, BM_CLICK, None, None);
        let mut message = MSG::default();
        assert!(
            PeekMessageW(
                &mut message,
                Some(owner.0),
                WM_COMMAND,
                WM_COMMAND,
                PM_REMOVE
            )
            .as_bool()
        );
        assert_eq!(message.wParam.0, 103);
        flag.store(true, Ordering::Relaxed);
        let result = done_rx
            .recv_timeout(Duration::from_secs(8))
            .unwrap()
            .unwrap();
        drop(worker);
        assert!(result.notice.contains("保存"));
        assert!(IsWindowVisible(bar.0).as_bool());
        assert_eq!(
            std::fs::read_dir(files.path()).unwrap().count(),
            1,
            "only the MP4 is saved"
        );
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 1).unwrap();
        let attributes = attributes.unwrap();
        attributes
            .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
            .unwrap();
        let reader =
            MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attributes).unwrap();
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let media = MFCreateMediaType().unwrap();
        media
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .unwrap();
        media.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).unwrap();
        reader.SetCurrentMediaType(stream, None, &media).unwrap();
        let media = reader.GetCurrentMediaType(stream).unwrap();
        let size = media.GetUINT64(&MF_MT_FRAME_SIZE).unwrap();
        let width = (size >> 32) as usize;
        let height = size as u32 as usize;
        let stride = media
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .map(|n| n as i32)
            .unwrap_or_else(|_| {
                MFGetStrideForBitmapInfoHeader(MFVideoFormat_RGB32.data1, width as u32).unwrap()
            });
        let mut frames = 0;
        loop {
            let mut sample = None;
            let mut bits = 0;
            let mut time = 0;
            reader
                .ReadSample(
                    stream,
                    0,
                    None,
                    Some(&mut bits),
                    Some(&mut time),
                    Some(&mut sample),
                )
                .unwrap();
            if let Some(sample) = sample {
                let buffer = sample.ConvertToContiguousBuffer().unwrap();
                let mut ptr = std::ptr::null_mut();
                let mut length = 0;
                buffer.Lock(&mut ptr, None, Some(&mut length)).unwrap();
                let bytes = std::slice::from_raw_parts(ptr, length as usize);
                let y = height / 2;
                let row = if stride < 0 { height - 1 - y } else { y };
                let index = row * stride.unsigned_abs() as usize + width / 2 * 4;
                let pixel = bytes[index..index + 3].to_vec();
                buffer.Unlock().unwrap();
                assert!(
                    pixel.iter().all(|c| *c > 235),
                    "recorded bar is visible: {pixel:?}"
                );
                assert!(time < 30_000_000);
                frames += 1;
            }
            if bits & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            assert!(frames < 100);
        }
        assert!(frames >= 20);
        eprintln!("visible bar excluded from all {frames} decoded frames; button saved early MP4");
    }
}

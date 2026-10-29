//! Explicitly invoked tests: these capture the interactive desktop and real audio devices.
use super::*;
use windows::Win32::System::SystemServices::SS_WHITERECT;

#[test]
#[ignore = "shows a two-tone window and captures the real desktop to verify MP4 orientation"]
fn real_desktop_video_keeps_top_and_bottom() {
    let _com = Com::new().unwrap();
    let _media = Media::new().unwrap();
    unsafe {
        use windows::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            SetThreadDpiAwarenessContext,
        };
        struct Dpi(DPI_AWARENESS_CONTEXT);
        impl Drop for Dpi {
            fn drop(&mut self) {
                unsafe {
                    SetThreadDpiAwarenessContext(self.0);
                }
            }
        }
        let _dpi = Dpi(SetThreadDpiAwarenessContext(
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        ));
        struct Window(HWND, HWND);
        impl Drop for Window {
            fn drop(&mut self) {
                unsafe {
                    let _ = DestroyWindow(self.0);
                    let _ = SetForegroundWindow(self.1);
                }
            }
        }
        let foreground = GetForegroundWindow();
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("STATIC"),
            w!("yytoolbox orientation verification"),
            WS_POPUP | WS_VISIBLE | WINDOW_STYLE(SS_WHITERECT.0),
            100,
            100,
            160,
            120,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let _window = Window(hwnd, foreground);
        let child = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            None,
            WS_CHILD
                | WS_VISIBLE
                | WINDOW_STYLE(windows::Win32::System::SystemServices::SS_BLACKRECT.0),
            0,
            60,
            160,
            60,
            Some(hwnd),
            None,
            None,
            None,
        )
        .unwrap();
        let _ = UpdateWindow(hwnd);
        let _ = UpdateWindow(child);
        windows::Win32::Graphics::Dwm::DwmFlush().unwrap();
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client).unwrap();
        assert_eq!((client.right, client.bottom), (160, 120));
        let mut origin = POINT::default();
        assert!(ClientToScreen(hwnd, &mut origin).as_bool());
        let rect = RECT {
            left: origin.x,
            top: origin.y,
            right: origin.x + client.right,
            bottom: origin.y + client.bottom,
        };
        let desktop = Desktop::new(160, 120).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let frame = desktop.frame(rect).unwrap();
            let top = frame[3 * 160 * 4 + 80 * 4];
            let bottom = frame[116 * 160 * 4 + 80 * 4];
            if top > 220 && bottom < 180 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "test pattern not visible: top={top}, bottom={bottom}"
            );
            std::thread::park_timeout(Duration::from_millis(10));
        }
        let path = output_dir().join("orientation.mp4");
        let mut params = yytoolbox::Tool::Recording.defaults();
        params.insert("seconds".into(), "2".into());
        record(
            Plan::new(path.clone(), rect, &params).unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap();
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
        assert_eq!(
            media.GetUINT64(&MF_MT_FRAME_SIZE).unwrap(),
            (160u64 << 32) | 120
        );
        let stride = media
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .map(|value| value as i32)
            .unwrap_or_else(|_| {
                MFGetStrideForBitmapInfoHeader(MFVideoFormat_RGB32.data1, 160).unwrap()
            });
        let mut frames = 0;
        loop {
            let mut flags = 0;
            let mut sample = None;
            reader
                .ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))
                .unwrap();
            if let Some(sample) = sample {
                let buffer = sample.ConvertToContiguousBuffer().unwrap();
                let mut ptr = std::ptr::null_mut();
                let mut length = 0;
                buffer.Lock(&mut ptr, None, Some(&mut length)).unwrap();
                let bytes = std::slice::from_raw_parts(ptr, length as usize);
                assert!(bytes.len() >= 120 * stride.unsigned_abs() as usize);
                let row = |y: usize| if stride < 0 { 119 - y } else { y } * stride.unsigned_abs() as usize + 80 * 4;
                let top = bytes[row(3)];
                let bottom = bytes[row(116)];
                buffer.Unlock().unwrap();
                assert!(
                    top > 220 && bottom < 180,
                    "MP4 is inverted: top={top}, bottom={bottom}, stride={stride}"
                );
                frames += 1;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            assert!(frames < 200, "unexpected video duration");
        }
        assert!(frames > 0);
        eprintln!(
            "{}: {frames} real desktop frames retain white top / dark bottom",
            path.display()
        );
    }
}

fn output_dir() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/recording-hardware");
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn decode(path: &std::path::Path, audio: bool) -> (usize, i64, f64) {
    unsafe {
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 1).unwrap();
        let attributes = attributes.unwrap();
        attributes
            .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
            .unwrap();
        let reader =
            MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attributes).unwrap();
        let stream = if audio {
            MF_SOURCE_READER_FIRST_AUDIO_STREAM
        } else {
            MF_SOURCE_READER_FIRST_VIDEO_STREAM
        }
        .0 as u32;
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .unwrap();
        reader.SetStreamSelection(stream, true).unwrap();
        let ty = MFCreateMediaType().unwrap();
        ty.SetGUID(
            &MF_MT_MAJOR_TYPE,
            if audio {
                &MFMediaType_Audio
            } else {
                &MFMediaType_Video
            },
        )
        .unwrap();
        ty.SetGUID(
            &MF_MT_SUBTYPE,
            if audio {
                &MFAudioFormat_PCM
            } else {
                &MFVideoFormat_RGB32
            },
        )
        .unwrap();
        reader.SetCurrentMediaType(stream, None, &ty).unwrap();
        let mut count = 0;
        let mut last = 0;
        let mut power = 0.0;
        let mut values = 0;
        for _ in 0..1000 {
            let mut flags = 0;
            let mut time = 0;
            let mut sample = None;
            reader
                .ReadSample(
                    stream,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )
                .unwrap();
            if let Some(sample) = sample {
                count += 1;
                last = time;
                let buffer = sample.ConvertToContiguousBuffer().unwrap();
                let mut ptr = std::ptr::null_mut();
                let mut len = 0;
                buffer.Lock(&mut ptr, None, Some(&mut len)).unwrap();
                let bytes = std::slice::from_raw_parts(ptr, len as usize);
                if audio {
                    for pair in bytes.chunks_exact(2) {
                        let value = f64::from(i16::from_le_bytes([pair[0], pair[1]]));
                        power += value * value;
                        values += 1;
                    }
                } else {
                    // Actual desktop frames should contain image data, not an empty buffer.
                    assert!(bytes.iter().any(|b| *b != 0));
                }
                buffer.Unlock().unwrap();
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        assert!(count > 0 && last > 10_000_000, "count={count} last={last}");
        (count, last, (power / values.max(1) as f64).sqrt())
    }
}

#[test]
#[ignore = "explicit real desktop capture; run with --ignored --test-threads=1"]
fn real_desktop_fullscreen_and_rectangle() {
    let _com = Com::new().unwrap();
    let _media = Media::new().unwrap();
    let full = area("全画面（メイン）").unwrap().unwrap();
    for (name, rect) in [
        ("fullscreen", full),
        (
            "rectangle",
            RECT {
                left: 40,
                top: 40,
                right: 680.min(full.right),
                bottom: 400.min(full.bottom),
            },
        ),
    ] {
        let path = output_dir().join(format!("{name}.mp4"));
        let mut params = yytoolbox::Tool::Recording.defaults();
        params.insert("seconds".into(), "2".into());
        params.insert("max_width".into(), "640".into());
        let result = record(
            Plan::new(path.clone(), rect, &params).unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let stats = decode(&path, false);
        eprintln!("{}; decoded={stats:?}", result.text);
    }
}

struct Tone(Vec<u8>);
impl Tone {
    fn start() -> Self {
        let samples: Vec<u8> = (0..48000u32)
            .flat_map(|n| {
                let value = ((f64::from(n) * 523.25 * std::f64::consts::TAU / 48000.0).sin()
                    * 2000.0) as i16;
                [value.to_le_bytes(), value.to_le_bytes()].concat()
            })
            .collect();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&48000u32.to_le_bytes());
        wav.extend_from_slice(&192000u32.to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(&samples);
        let tone = Self(wav);
        assert!(
            unsafe {
                PlaySoundW(
                    windows::core::PCWSTR(tone.0.as_ptr().cast()),
                    None,
                    SND_MEMORY | SND_ASYNC | SND_LOOP | SND_NODEFAULT,
                )
            }
            .as_bool()
        );
        tone
    }
}
impl Drop for Tone {
    fn drop(&mut self) {
        unsafe {
            let _ = PlaySoundW(windows::core::PCWSTR::null(), None, SND_FLAGS(0));
        }
    }
}

fn real_audio(loopback: bool) {
    let _com = Com::new().unwrap();
    let _media = Media::new().unwrap();
    unsafe {
        let devices: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
        let endpoints = devices
            .EnumAudioEndpoints(
                if loopback { eRender } else { eCapture },
                DEVICE_STATE(DEVICE_STATEMASK_ALL),
            )
            .unwrap();
        eprintln!(
            "endpoint count (including disabled/disconnected): {}",
            endpoints.GetCount().unwrap()
        );
        for i in 0..endpoints.GetCount().unwrap() {
            eprintln!(
                "endpoint {i}: state={:?}",
                endpoints.Item(i).unwrap().GetState().unwrap()
            );
        }
    }
    let _tone = loopback.then(Tone::start);
    let begin = Arc::new(AtomicBool::new(true));
    let audio = Audio::start(loopback, begin).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let samples = loop {
        if let Some(error) = audio.error.lock().unwrap().clone() {
            panic!("{error}");
        }
        let count = audio.queue.lock().unwrap().len();
        if count >= 48000 {
            break audio.take(count).unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "no actual audio packets: {count} samples"
        );
        std::thread::park_timeout(Duration::from_millis(5));
    };
    let rms =
        (samples.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / samples.len() as f64).sqrt();
    eprintln!(
        "actual {} packets: {} samples, RMS={rms:.2}",
        if loopback { "loopback" } else { "microphone" },
        samples.len()
    );
    if loopback {
        assert!(rms > 10.0, "played tone was not captured");
    }
    drop(audio);
    let path = output_dir().join(if loopback {
        "system-audio.mp4"
    } else {
        "microphone.mp4"
    });
    let mut params = yytoolbox::Tool::Recording.defaults();
    params.insert("seconds".into(), "10".into());
    params.insert("max_width".into(), "640".into());
    params.insert(
        if loopback {
            "system_audio"
        } else {
            "microphone"
        }
        .into(),
        "あり".into(),
    );
    let rect = area("全画面（メイン）").unwrap().unwrap();
    let stop = AtomicBool::new(false);
    struct StopDesktop<'a> {
        desktop: Desktop,
        count: std::cell::Cell<u32>,
        stop: &'a AtomicBool,
    }
    impl FrameSource for StopDesktop<'_> {
        fn frame(&self, rect: RECT) -> Result<&[u8]> {
            let bytes = self.desktop.frame(rect)?;
            self.count.set(self.count.get() + 1);
            if self.count.get() == 20 {
                self.stop.store(true, Ordering::Relaxed);
            }
            Ok(bytes)
        }
    }
    record_with_source(
        Plan::new(path.clone(), rect, &params).unwrap(),
        &stop,
        |w, h| {
            Ok(StopDesktop {
                desktop: Desktop::new(w, h)?,
                count: std::cell::Cell::new(0),
                stop: &stop,
            })
        },
    )
    .unwrap();
    let stats = decode(&path, true);
    eprintln!("{} AAC decoded={stats:?}", path.display());
    assert!(
        stats.1 < 30_000_000,
        "early stop must not pad audio to 10 seconds: {stats:?}"
    );
    if loopback {
        assert!(stats.2 > 10.0);
    }
}

#[test]
#[ignore = "plays a quiet tone and records the real default output device"]
fn real_system_audio() {
    real_audio(true);
}

#[test]
#[ignore = "records the real default microphone"]
fn real_microphone() {
    real_audio(false);
}

#[test]
#[ignore = "creates a visible test window, hides it and stops recording with the control bar"]
fn real_window_exclusion_and_stop_button() {
    unsafe {
        let foreground = GetForegroundWindow();
        struct Window(HWND, HWND);
        impl Drop for Window {
            fn drop(&mut self) {
                unsafe {
                    let _ = DestroyWindow(self.0);
                    let _ = SetForegroundWindow(self.1);
                }
            }
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("STATIC"),
            w!("yytoolbox capture verification"),
            WS_POPUP | WS_VISIBLE | WINDOW_STYLE(SS_WHITERECT.0),
            100,
            100,
            320,
            240,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let window = Window(hwnd, foreground);
        let _ = UpdateWindow(hwnd);
        windows::Win32::Graphics::Dwm::DwmFlush().unwrap();
        let desktop = Desktop::new(64, 48).unwrap();
        let rect = RECT {
            left: 100,
            top: 100,
            right: 420,
            bottom: 340,
        };
        let before = desktop.frame(rect).unwrap().to_vec();
        let mut guard = UiGuard::new(hwnd).unwrap();
        assert!(!IsWindowVisible(hwnd).as_bool());
        let after = desktop.frame(rect).unwrap().to_vec();
        assert_ne!(
            before, after,
            "hidden yytoolbox window must disappear from real capture"
        );
        let files = tempfile::tempdir().unwrap();
        let path = files.path().join("early-stop.mp4");
        std::fs::write(&path, b"preserve-existing-file").unwrap();
        let mut params = yytoolbox::Tool::Recording.defaults();
        params.insert("seconds".into(), "10".into());
        params.insert("max_width".into(), "640".into());
        let plan = Plan::new(path.clone(), rect, &params).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        struct ReadyDesktop {
            desktop: Desktop,
            count: std::cell::Cell<u32>,
            ready: mpsc::SyncSender<()>,
        }
        impl FrameSource for ReadyDesktop {
            fn frame(&self, rect: RECT) -> Result<&[u8]> {
                let bytes = self.desktop.frame(rect)?;
                self.count.set(self.count.get() + 1);
                if self.count.get() == 20 {
                    let _ = self.ready.send(());
                }
                Ok(bytes)
            }
        }
        let worker = std::thread::spawn(move || {
            let _ = tx.send(record_with_source(plan, &flag, |w, h| {
                Ok(ReadyDesktop {
                    desktop: Desktop::new(w, h)?,
                    count: std::cell::Cell::new(0),
                    ready: ready_tx,
                })
            }));
        });
        struct Worker(Arc<AtomicBool>, Option<JoinHandle<()>>);
        impl Drop for Worker {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
                if let Some(thread) = self.1.take() {
                    let _ = thread.join();
                }
            }
        }
        let worker = Worker(cancel.clone(), Some(worker));
        // Wait for actual captured frames, not just an open temporary file.
        ready_rx
            .recv_timeout(Duration::from_secs(8))
            .unwrap_or_else(|e| {
                panic!(
                    "recorder did not capture 20 frames: {e}; result={:?}",
                    rx.try_recv()
                )
            });
        guard.show_controls(10, "なし（ボタンのみ）", 103).unwrap();
        guard.controls.as_ref().unwrap().click_stop();
        let mut msg = MSG::default();
        assert!(PeekMessageW(&mut msg, Some(hwnd), WM_COMMAND, WM_COMMAND, PM_REMOVE).as_bool());
        assert_eq!(msg.wParam.0, 103);
        cancel.store(true, Ordering::Relaxed);
        let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(worker);
        assert!(result.unwrap().text.contains("録画を保存しました"));
        let _com = Com::new().unwrap();
        let _media = Media::new().unwrap();
        let stats = decode(&path, false);
        assert!(
            stats.1 < 90_000_000,
            "Stop button must end before the requested 10 seconds: {stats:?}"
        );
        assert_eq!(std::fs::read_dir(files.path()).unwrap().count(), 1);
        drop(guard);
        assert!(IsWindowVisible(hwnd).as_bool());
        drop(window);
        eprintln!("real GDI exclusion, window restoration, stop button and early MP4 save passed");
    }
}

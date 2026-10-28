//! Explicitly invoked tests: these capture the interactive desktop and real audio devices.
use super::*;
use windows::Win32::System::SystemServices::SS_WHITERECT;

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
    params.insert("seconds".into(), "2".into());
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
    record(
        Plan::new(path.clone(), rect, &params).unwrap(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let stats = decode(&path, true);
    eprintln!("{} AAC decoded={stats:?}", path.display());
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
#[ignore = "creates a visible test window, hides it and sends a real global Escape key"]
fn real_window_exclusion_and_escape() {
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
        let guard = UiGuard::new(hwnd).unwrap();
        assert!(!IsWindowVisible(hwnd).as_bool());
        let after = desktop.frame(rect).unwrap().to_vec();
        assert_ne!(
            before, after,
            "hidden yytoolbox window must disappear from real capture"
        );
        let path = output_dir().join("cancelled.mp4");
        std::fs::write(&path, b"preserve-existing-file").unwrap();
        let mut params = yytoolbox::Tool::Recording.defaults();
        params.insert("seconds".into(), "10".into());
        params.insert("max_width".into(), "640".into());
        let plan = Plan::new(path.clone(), rect, &params).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let _ = tx.send(record(plan, &flag));
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
        // Wait for the real recorder's temporary MP4, rather than a fixed delay.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !std::fs::read_dir(output_dir())
            .unwrap()
            .any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".tmp"))
        {
            assert!(
                Instant::now() < deadline,
                "recorder did not open its temporary MP4: {:?}",
                rx.try_recv()
            );
            std::thread::park_timeout(Duration::from_millis(5));
        }
        let inputs = [
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_ESCAPE,
                        ..Default::default()
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_ESCAPE,
                        dwFlags: KEYEVENTF_KEYUP,
                        ..Default::default()
                    },
                },
            },
        ];
        assert_eq!(SendInput(&inputs, std::mem::size_of::<INPUT>() as i32), 2);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mut msg = MSG::default();
            if PeekMessageW(&mut msg, Some(hwnd), WM_HOTKEY, WM_HOTKEY, PM_REMOVE).as_bool() {
                assert_eq!(msg.wParam.0, CANCEL_HOTKEY as usize);
                cancel.store(true, Ordering::Relaxed);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "real Escape did not produce WM_HOTKEY"
            );
            std::thread::park_timeout(Duration::from_millis(5));
        }
        let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(worker);
        assert!(result.unwrap_err().contains("中止"));
        assert_eq!(std::fs::read(path).unwrap(), b"preserve-existing-file");
        drop(guard);
        assert!(IsWindowVisible(hwnd).as_bool());
        drop(window);
        eprintln!(
            "real GDI exclusion, window restoration, global Escape and MP4 preservation passed"
        );
    }
}

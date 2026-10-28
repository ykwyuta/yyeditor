//! Native GDI → Media Foundation H.264/AAC. WASAPI supplies optional PCM sources.
#![allow(unsafe_code)]
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use windows::core::BOOL;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        Media::{Audio::*, MediaFoundation::*},
        System::{Com::*, Threading::GetCurrentThreadId},
        UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
    core::{HSTRING, w},
};
use yytoolbox::{Output, Params, Result, integer, value};

pub const CANCEL_HOTKEY: i32 = 0x5954;
pub struct Plan {
    pub destination: PathBuf,
    pub rect: RECT,
    seconds: u32,
    fps: u32,
    max_width: u32,
    system: bool,
    microphone: bool,
}
impl Plan {
    pub fn new(destination: PathBuf, rect: RECT, params: &Params) -> Result<Self> {
        if rect.right - rect.left < 2 || rect.bottom - rect.top < 2 {
            return Err("録画範囲は2×2ピクセル以上です".into());
        }
        Ok(Self {
            destination,
            rect,
            seconds: integer(params, "seconds", 1, 3600)?,
            fps: integer(params, "fps", 1, 30)?,
            max_width: integer(params, "max_width", 320, 7680)?,
            system: value(params, "system_audio", "なし") == "あり",
            microphone: value(params, "microphone", "なし") == "あり",
        })
    }
}

/// The app stays entirely hidden while recording. Restore only our own visible windows.
pub struct UiGuard {
    hwnd: HWND,
    windows: Vec<(HWND, u32)>,
}
impl UiGuard {
    pub fn new(hwnd: HWND) -> Result<Self> {
        unsafe {
            RegisterHotKey(
                Some(hwnd),
                CANCEL_HOTKEY,
                MOD_NOREPEAT,
                u32::from(VK_ESCAPE.0),
            )
            .map_err(|e| format!("Escを録画中止キーに登録できません: {e}"))?;
        }
        let mut guard = Self {
            hwnd,
            windows: Vec::new(),
        };
        unsafe extern "system" fn collect(hwnd: HWND, param: LPARAM) -> BOOL {
            if unsafe { IsWindowVisible(hwnd) }.as_bool() {
                let windows = unsafe { &mut *(param.0 as *mut Vec<(HWND, u32)>) };
                let mut affinity = 0;
                unsafe {
                    let _ = GetWindowDisplayAffinity(hwnd, &mut affinity);
                }
                windows.push((hwnd, affinity));
            }
            TRUE
        }
        unsafe {
            let _ = EnumThreadWindows(
                GetCurrentThreadId(),
                Some(collect),
                LPARAM(&mut guard.windows as *mut _ as isize),
            );
            for (window, _) in &guard.windows {
                let _ = SetWindowDisplayAffinity(*window, WDA_EXCLUDEFROMCAPTURE);
                let _ = ShowWindow(*window, SW_HIDE);
            }
            windows::Win32::Graphics::Dwm::DwmFlush()
                .map_err(|e| format!("録画前に画面を隠せませんでした: {e}"))?;
        }
        Ok(guard)
    }
}
impl Drop for UiGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = UnregisterHotKey(Some(self.hwnd), CANCEL_HOTKEY);
            for (window, affinity) in &self.windows {
                if IsWindow(Some(*window)).as_bool() {
                    let _ = SetWindowDisplayAffinity(*window, WINDOW_DISPLAY_AFFINITY(*affinity));
                    let _ = ShowWindow(*window, SW_SHOWNA);
                }
            }
            let _ = SetForegroundWindow(self.hwnd);
        }
    }
}

pub fn area(mode: &str) -> Result<Option<RECT>> {
    unsafe {
        match mode {
            "全画面（メイン）" => Ok(Some(RECT {
                left: 0,
                top: 0,
                right: GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            })),
            "全画面（すべて）" => {
                let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
                let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
                Ok(Some(RECT {
                    left: x,
                    top: y,
                    right: x + GetSystemMetrics(SM_CXVIRTUALSCREEN),
                    bottom: y + GetSystemMetrics(SM_CYVIRTUALSCREEN),
                }))
            }
            "矩形選択" => pick_rectangle(),
            _ => Err("録画する範囲が不正です".into()),
        }
    }
}
struct Picker {
    start: Option<POINT>,
    end: POINT,
    result: Option<RECT>,
    done: bool,
}
extern "system" fn picker_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Picker;
        if !ptr.is_null() {
            let picker = &mut *ptr;
            match message {
                WM_LBUTTONDOWN => {
                    let mut point = POINT::default();
                    let _ = GetCursorPos(&mut point);
                    picker.start = Some(point);
                    picker.end = point;
                    let _ = SetCapture(hwnd);
                    return LRESULT(0);
                }
                WM_MOUSEMOVE => {
                    if picker.start.is_some() {
                        let _ = GetCursorPos(&mut picker.end);
                        let _ = InvalidateRect(Some(hwnd), None, true);
                    }
                    return LRESULT(0);
                }
                WM_LBUTTONUP => {
                    let _ = ReleaseCapture();
                    let _ = GetCursorPos(&mut picker.end);
                    if let Some(start) = picker.start {
                        picker.result = Some(RECT {
                            left: start.x.min(picker.end.x),
                            top: start.y.min(picker.end.y),
                            right: start.x.max(picker.end.x),
                            bottom: start.y.max(picker.end.y),
                        });
                    }
                    picker.done = true;
                    return LRESULT(0);
                }
                WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => {
                    picker.done = true;
                    return LRESULT(0);
                }
                WM_CLOSE => {
                    picker.done = true;
                    return LRESULT(0);
                }
                WM_PAINT => {
                    let mut paint = PAINTSTRUCT::default();
                    let dc = BeginPaint(hwnd, &mut paint);
                    let mut client = RECT::default();
                    let _ = GetClientRect(hwnd, &mut client);
                    FillRect(dc, &client, HBRUSH(GetStockObject(BLACK_BRUSH).0));
                    if let Some(start) = picker.start {
                        let mut a = start;
                        let mut b = picker.end;
                        let _ = ScreenToClient(hwnd, &mut a);
                        let _ = ScreenToClient(hwnd, &mut b);
                        let rect = RECT {
                            left: a.x.min(b.x),
                            top: a.y.min(b.y),
                            right: a.x.max(b.x),
                            bottom: a.y.max(b.y),
                        };
                        FillRect(dc, &rect, HBRUSH(GetStockObject(WHITE_BRUSH).0));
                    }
                    let _ = EndPaint(hwnd, &paint);
                    return LRESULT(0);
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, message, wp, lp)
    }
}
unsafe fn pick_rectangle() -> Result<Option<RECT>> {
    unsafe {
        let class = w!("YYToolboxRecordingRectangle");
        let _ = RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(picker_proc),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_CROSS).map_err(|e| e.to_string())?,
            ..Default::default()
        });
        let mut picker = Picker {
            start: None,
            end: POINT::default(),
            result: None,
            done: false,
        };
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            class,
            w!("範囲をドラッグ — Escで中止"),
            WS_POPUP,
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
            None,
            None,
            None,
            Some(&mut picker as *mut _ as *const _),
        )
        .map_err(|e| e.to_string())?;
        struct Window(HWND);
        impl Drop for Window {
            fn drop(&mut self) {
                unsafe {
                    let _ = DestroyWindow(self.0);
                }
            }
        }
        let _window = Window(hwnd);
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 100, LWA_ALPHA).map_err(|e| e.to_string())?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        while !picker.done {
            let mut msg = MSG::default();
            if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                break;
            }
            if msg.message == WM_HOTKEY && msg.wParam.0 == CANCEL_HOTKEY as usize {
                picker.done = true;
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        Ok(picker.result)
    }
}

struct Com;
impl Com {
    fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|e| e.to_string())?;
        }
        Ok(Self)
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
struct Media;
impl Media {
    fn new() -> Result<Self> {
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| {
                format!(
                    "Media Foundationを開始できません。Windowsのメディア機能を確認してください: {e}"
                )
            })?;
        }
        Ok(Self)
    }
}
impl Drop for Media {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
        }
    }
}

struct Encoder {
    writer: IMFSinkWriter,
    video: u32,
    audio: Option<u32>,
}
impl Encoder {
    fn new(path: &std::path::Path, width: u32, height: u32, fps: u32, audio: bool) -> Result<Self> {
        unsafe {
            let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, None)
                .map_err(|e| e.to_string())?;
            let output = MFCreateMediaType().map_err(|e| e.to_string())?;
            output
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| e.to_string())?;
            output
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(|e| e.to_string())?;
            output
                .SetUINT32(&MF_MT_AVG_BITRATE, 6_000_000)
                .map_err(|e| e.to_string())?;
            output
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| e.to_string())?;
            output
                .SetUINT64(
                    &MF_MT_FRAME_SIZE,
                    (u64::from(width) << 32) | u64::from(height),
                )
                .map_err(|e| e.to_string())?;
            output
                .SetUINT64(&MF_MT_FRAME_RATE, (u64::from(fps) << 32) | 1)
                .map_err(|e| e.to_string())?;
            output
                .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)
                .map_err(|e| e.to_string())?;
            let video = writer.AddStream(&output).map_err(|e| e.to_string())?;
            let input = MFCreateMediaType().map_err(|e| e.to_string())?;
            output.CopyAllItems(&input).map_err(|e| e.to_string())?;
            input
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(|e| e.to_string())?;
            input
                .SetUINT32(&MF_MT_DEFAULT_STRIDE, width * 4)
                .map_err(|e| e.to_string())?;
            writer
                .SetInputMediaType(video, &input, None)
                .map_err(|e| format!("H.264エンコーダーを初期化できません: {e}"))?;
            let audio = if audio {
                let output = MFCreateMediaType().map_err(|e| e.to_string())?;
                output
                    .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                    .map_err(|e| e.to_string())?;
                output
                    .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)
                    .map_err(|e| e.to_string())?;
                output
                    .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)
                    .map_err(|e| e.to_string())?;
                output
                    .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48000)
                    .map_err(|e| e.to_string())?;
                output
                    .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
                    .map_err(|e| e.to_string())?;
                output
                    .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24000)
                    .map_err(|e| e.to_string())?;
                let stream = writer.AddStream(&output).map_err(|e| e.to_string())?;
                let input = MFCreateMediaType().map_err(|e| e.to_string())?;
                input
                    .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                    .map_err(|e| e.to_string())?;
                input
                    .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                    .map_err(|e| e.to_string())?;
                input
                    .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)
                    .map_err(|e| e.to_string())?;
                input
                    .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48000)
                    .map_err(|e| e.to_string())?;
                input
                    .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
                    .map_err(|e| e.to_string())?;
                input
                    .SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)
                    .map_err(|e| e.to_string())?;
                input
                    .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 192000)
                    .map_err(|e| e.to_string())?;
                writer
                    .SetInputMediaType(stream, &input, None)
                    .map_err(|e| format!("AACエンコーダーを初期化できません: {e}"))?;
                Some(stream)
            } else {
                None
            };
            writer.BeginWriting().map_err(|e| e.to_string())?;
            Ok(Self {
                writer,
                video,
                audio,
            })
        }
    }
    fn sample(&self, stream: u32, data: &[u8], time: i64, duration: i64) -> Result<()> {
        unsafe {
            let buffer = MFCreateMemoryBuffer(data.len() as u32).map_err(|e| e.to_string())?;
            let mut ptr = std::ptr::null_mut();
            buffer
                .Lock(&mut ptr, None, None)
                .map_err(|e| e.to_string())?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            buffer.Unlock().map_err(|e| e.to_string())?;
            buffer
                .SetCurrentLength(data.len() as u32)
                .map_err(|e| e.to_string())?;
            let sample = MFCreateSample().map_err(|e| e.to_string())?;
            sample.AddBuffer(&buffer).map_err(|e| e.to_string())?;
            sample.SetSampleTime(time).map_err(|e| e.to_string())?;
            sample
                .SetSampleDuration(duration)
                .map_err(|e| e.to_string())?;
            self.writer
                .WriteSample(stream, &sample)
                .map_err(|e| format!("録画データを書き込めません: {e}"))?;
            Ok(())
        }
    }
    fn finish(&self) -> Result<()> {
        unsafe {
            self.writer
                .Finalize()
                .map_err(|e| format!("MP4を確定できません: {e}"))
        }
    }
}

struct Desktop {
    screen: HDC,
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    data: *mut u8,
    width: i32,
    height: i32,
}
impl Desktop {
    fn new(width: u32, height: u32) -> Result<Self> {
        unsafe {
            let screen = GetDC(None);
            if screen.is_invalid() {
                return Err("画面を取得できません".into());
            }
            let dc = CreateCompatibleDC(Some(screen));
            if dc.is_invalid() {
                ReleaseDC(None, screen);
                return Err("録画用DCを作成できません".into());
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: height as i32,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut data = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut data, None, 0)
            {
                Ok(v) => v,
                Err(e) => {
                    let _ = DeleteDC(dc);
                    ReleaseDC(None, screen);
                    return Err(e.to_string());
                }
            };
            let previous = SelectObject(dc, bitmap.into());
            let _ = SetStretchBltMode(dc, HALFTONE);
            Ok(Self {
                screen,
                dc,
                bitmap,
                previous,
                data: data.cast(),
                width: width as i32,
                height: height as i32,
            })
        }
    }
    fn frame(&self, rect: RECT) -> Result<&[u8]> {
        unsafe {
            StretchBlt(
                self.dc,
                0,
                0,
                self.width,
                self.height,
                Some(self.screen),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SRCCOPY | CAPTUREBLT,
            )
            .ok()
            .map_err(|e| format!("画面を取得できません: {e}"))?;
            let _ = GdiFlush();
            Ok(std::slice::from_raw_parts(
                self.data,
                self.width as usize * self.height as usize * 4,
            ))
        }
    }
}
impl Drop for Desktop {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
            ReleaseDC(None, self.screen);
        }
    }
}

struct Audio {
    queue: Arc<Mutex<VecDeque<i16>>>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
}
impl Audio {
    fn start(loopback: bool, begin: Arc<AtomicBool>) -> Result<Self> {
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let (q, s, err) = (queue.clone(), stop.clone(), error.clone());
        let (tx, rx) = mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            let result = capture_audio(loopback, &q, &s, &begin, &tx);
            if let Err(e) = result {
                let _ = tx.try_send(Err(e.clone()));
                *err.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
            }
        });
        let result = Self {
            queue,
            stop,
            error,
            thread: Some(thread),
        };
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|_| "音声デバイスの初期化が時間切れです".to_owned())??;
        Ok(result)
    }
    fn take(&self, samples: usize) -> Result<Vec<i16>> {
        if let Some(error) = self.error.lock().map_err(|e| e.to_string())?.clone() {
            return Err(error);
        }
        let mut queue = self.queue.lock().map_err(|e| e.to_string())?;
        Ok((0..samples)
            .map(|_| queue.pop_front().unwrap_or(0))
            .collect())
    }
}
impl Drop for Audio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn capture_audio(
    loopback: bool,
    queue: &Mutex<VecDeque<i16>>,
    stop: &AtomicBool,
    begin: &AtomicBool,
    ready: &mpsc::SyncSender<Result<()>>,
) -> Result<()> {
    let _com = Com::new()?;
    unsafe {
        let devices: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let device = devices
            .GetDefaultAudioEndpoint(if loopback { eRender } else { eCapture }, eConsole)
            .map_err(|e| {
                format!(
                    "{}を取得できません: {e}",
                    if loopback {
                        "PC再生音デバイス"
                    } else {
                        "マイク"
                    }
                )
            })?;
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| e.to_string())?;
        let format = WAVEFORMATEX {
            wFormatTag: 1,
            nChannels: 2,
            nSamplesPerSec: 48000,
            nAvgBytesPerSec: 192000,
            nBlockAlign: 4,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
            | if loopback {
                AUDCLNT_STREAMFLAGS_LOOPBACK
            } else {
                0
            };
        client
            .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 1_000_000, 0, &format, None)
            .map_err(|e| {
                format!("音声を初期化できません（マイクの許可・既定デバイスを確認）: {e}")
            })?;
        let capture: IAudioCaptureClient = client.GetService().map_err(|e| e.to_string())?;
        ready.send(Ok(())).map_err(|e| e.to_string())?;
        while !begin.load(Ordering::Acquire) && !stop.load(Ordering::Relaxed) {
            std::thread::park_timeout(Duration::from_millis(2));
        }
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        client.Start().map_err(|e| e.to_string())?;
        struct Started(IAudioClient);
        impl Drop for Started {
            fn drop(&mut self) {
                unsafe {
                    let _ = self.0.Stop();
                }
            }
        }
        let _started = Started(client);
        while !stop.load(Ordering::Relaxed) {
            while capture.GetNextPacketSize().map_err(|e| e.to_string())? > 0 {
                let mut data = std::ptr::null_mut();
                let mut frames = 0;
                let mut flags = 0;
                capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .map_err(|e| e.to_string())?;
                struct Packet<'a>(&'a IAudioCaptureClient, u32);
                impl Drop for Packet<'_> {
                    fn drop(&mut self) {
                        unsafe {
                            let _ = self.0.ReleaseBuffer(self.1);
                        }
                    }
                }
                let _packet = Packet(&capture, frames);
                if frames > 48000 {
                    return Err("音声パケットが大きすぎます".into());
                }
                let mut q = queue.lock().map_err(|e| e.to_string())?;
                if q.len() + frames as usize * 2 > 48000 * 4 {
                    return Err("音声処理が録画速度に追いつきません".into());
                }
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    q.extend(std::iter::repeat_n(0, frames as usize * 2));
                } else {
                    if data.is_null() {
                        return Err("音声バッファが空です".into());
                    }
                    for pair in
                        std::slice::from_raw_parts(data, frames as usize * 4).chunks_exact(2)
                    {
                        q.push_back(i16::from_le_bytes([pair[0], pair[1]]));
                    }
                }
            }
            std::thread::park_timeout(Duration::from_millis(3));
        }
    }
    Ok(())
}

pub fn record(plan: Plan, cancelled: &AtomicBool) -> Result<Output> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("録画を中止しました。録画ファイルは保存していません".into());
    }
    let _com = Com::new()?;
    let _media = Media::new()?;
    let sw = (plan.rect.right - plan.rect.left) as u32;
    let sh = (plan.rect.bottom - plan.rect.top) as u32;
    let width = sw.min(plan.max_width).max(2) & !1;
    let height = ((u64::from(sh) * u64::from(width) / u64::from(sw)) as u32).max(2) & !1;
    if u64::from(width) * u64::from(height) > 32_000_000 {
        return Err("録画画像は3200万画素までです。最大録画幅を小さくしてください".into());
    }
    let temp = tempfile::Builder::new()
        .suffix(".mp4")
        .tempfile_in(
            plan.destination
                .parent()
                .unwrap_or(std::path::Path::new(".")),
        )
        .map_err(|e| e.to_string())?
        .into_temp_path();
    let encoder = Encoder::new(
        &temp,
        width,
        height,
        plan.fps,
        plan.system || plan.microphone,
    )?;
    let desktop = Desktop::new(width, height)?;
    let begin = Arc::new(AtomicBool::new(false));
    let mut audio = Vec::new();
    if plan.system {
        audio.push(Audio::start(true, begin.clone())?);
    }
    if plan.microphone {
        audio.push(Audio::start(false, begin.clone())?);
    }
    let start = Instant::now();
    begin.store(true, Ordering::Release);
    let mut sent_audio = 0u64;
    let total_audio = u64::from(plan.seconds) * 48000;
    let frame_duration = 10_000_000 / i64::from(plan.fps);
    let mut frames = 0;
    while start.elapsed() < Duration::from_secs(u64::from(plan.seconds)) {
        if cancelled.load(Ordering::Relaxed) {
            return Err("録画を中止しました。録画ファイルは保存していません".into());
        }
        let elapsed = start.elapsed();
        let time = (elapsed.as_nanos() / 100) as i64;
        encoder.sample(
            encoder.video,
            desktop.frame(plan.rect)?,
            time,
            frame_duration.min(i64::from(plan.seconds) * 10_000_000 - time),
        )?;
        frames += 1;
        if let Some(stream) = encoder.audio {
            let desired =
                ((start.elapsed().as_nanos() * 48000 / 1_000_000_000) as u64).min(total_audio);
            if desired > sent_audio {
                let count = (desired - sent_audio) as usize;
                let sources = audio
                    .iter()
                    .map(|a| a.take(count * 2))
                    .collect::<Result<Vec<_>>>()?;
                let mixed: Vec<u8> = (0..count * 2)
                    .flat_map(|i| {
                        let sum: i32 = sources.iter().map(|v| i32::from(v[i])).sum();
                        let v = (sum / sources.len().max(1) as i32) as i16;
                        v.to_le_bytes()
                    })
                    .collect();
                encoder.sample(
                    stream,
                    &mixed,
                    (sent_audio * 10_000_000 / 48000) as i64,
                    (count as u64 * 10_000_000 / 48000) as i64,
                )?;
                sent_audio = desired;
            }
        }
        let next = Duration::from_secs_f64(f64::from(frames) / f64::from(plan.fps));
        while next > start.elapsed() {
            if cancelled.load(Ordering::Relaxed) {
                return Err("録画を中止しました。録画ファイルは保存していません".into());
            }
            std::thread::park_timeout(
                next.saturating_sub(start.elapsed())
                    .min(Duration::from_millis(20)),
            );
        }
    }
    if let Some(stream) = encoder.audio {
        let count = total_audio.saturating_sub(sent_audio) as usize;
        if count > 0 {
            let sources = audio
                .iter()
                .map(|a| a.take(count * 2))
                .collect::<Result<Vec<_>>>()?;
            let mixed: Vec<u8> = (0..count * 2)
                .flat_map(|i| {
                    let sum: i32 = sources.iter().map(|v| i32::from(v[i])).sum();
                    ((sum / sources.len().max(1) as i32) as i16).to_le_bytes()
                })
                .collect();
            encoder.sample(
                stream,
                &mixed,
                (sent_audio * 10_000_000 / 48000) as i64,
                (count as u64 * 10_000_000 / 48000) as i64,
            )?;
        }
    }
    drop(audio);
    if cancelled.load(Ordering::Relaxed) {
        return Err("録画を中止しました。録画ファイルは保存していません".into());
    }
    encoder.finish()?;
    drop(encoder);
    temp.persist(&plan.destination)
        .map_err(|e| e.error.to_string())?;
    let text = format!(
        "録画を保存しました: {}\n{width}×{height} / {}秒 / {frames}フレーム\nPC再生音: {} / マイク: {}",
        plan.destination.display(),
        plan.seconds,
        plan.system,
        plan.microphone
    );
    Ok(Output {
        bytes: text.as_bytes().to_vec(),
        text,
        filename: "recording-info.txt",
        notice: format!(
            "MP4保存完了: {}。結果欄は録画情報です",
            plan.destination.display()
        ),
        qr: None,
    })
}

#[cfg(test)]
#[path = "recording_hardware_tests.rs"]
mod hardware_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_mp4_h264_aac_encodes_and_decodes_synthetic_frames_and_audio() {
        // No desktop or physical microphone is captured by this regression test.
        let _com = Com::new().unwrap();
        let _media = Media::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.mp4");
        let encoder = Encoder::new(&path, 64, 64, 10, true).unwrap();
        let mut frame = vec![0u8; 64 * 64 * 4];
        for y in 0..64 {
            for x in 0..64 {
                let offset = (y * 64 + x) * 4;
                frame[offset..offset + 4].copy_from_slice(if y < 32 {
                    &[255, 0, 0, 255]
                } else {
                    &[0, 0, 255, 255]
                });
            }
        }
        for index in 0..20 {
            encoder
                .sample(encoder.video, &frame, index * 1_000_000, 1_000_000)
                .unwrap();
            let audio: Vec<u8> = (0..4800)
                .flat_map(|n| {
                    let sample = ((f64::from(n) * 440.0 * std::f64::consts::TAU / 48000.0).sin()
                        * 4000.0) as i16;
                    [sample.to_le_bytes(), sample.to_le_bytes()].concat()
                })
                .collect();
            encoder
                .sample(encoder.audio.unwrap(), &audio, index * 1_000_000, 1_000_000)
                .unwrap();
        }
        encoder.finish().unwrap();
        drop(encoder);
        assert!(std::fs::metadata(&path).unwrap().len() > 1000);
        unsafe {
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 1).unwrap();
            let attributes = attributes.unwrap();
            attributes
                .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .unwrap();
            let reader =
                MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attributes).unwrap();
            let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            let output = MFCreateMediaType().unwrap();
            output
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .unwrap();
            output
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .unwrap();
            reader.SetCurrentMediaType(stream, None, &output).unwrap();
            let media = reader.GetCurrentMediaType(stream).unwrap();
            assert_eq!(
                media.GetUINT64(&MF_MT_FRAME_SIZE).unwrap(),
                (64u64 << 32) | 64
            );
            let mut decoded = 0;
            let mut last = 0;
            for _ in 0..100 {
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
                    decoded += 1;
                    last = time;
                    if decoded == 1 {
                        let buffer = sample.ConvertToContiguousBuffer().unwrap();
                        let mut ptr = std::ptr::null_mut();
                        let mut length = 0;
                        buffer.Lock(&mut ptr, None, Some(&mut length)).unwrap();
                        let bytes = std::slice::from_raw_parts(ptr, length as usize);
                        assert!(bytes[0] > bytes[2] + 100, "bottom row should be blue");
                        let offset = 63 * 64 * 4;
                        assert!(
                            bytes[offset + 2] > bytes[offset] + 100,
                            "top row should be red"
                        );
                        buffer.Unlock().unwrap();
                    }
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
            }
            assert_eq!(decoded, 20);
            assert!((18_000_000..=20_000_000).contains(&last));
            let audio = reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32)
                .unwrap();
            assert_eq!(audio.GetGUID(&MF_MT_SUBTYPE).unwrap(), MFAudioFormat_AAC);
            assert_eq!(audio.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap(), 2);
            let audio_reader =
                MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None).unwrap();
            let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
            audio_reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .unwrap();
            audio_reader.SetStreamSelection(stream, true).unwrap();
            let output = MFCreateMediaType().unwrap();
            output
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .unwrap();
            output.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).unwrap();
            audio_reader
                .SetCurrentMediaType(stream, None, &output)
                .unwrap();
            let mut decoded_bytes = 0;
            let mut nonzero = false;
            for _ in 0..200 {
                let mut flags = 0;
                let mut sample = None;
                audio_reader
                    .ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))
                    .unwrap();
                if let Some(sample) = sample {
                    let buffer = sample.ConvertToContiguousBuffer().unwrap();
                    let mut ptr = std::ptr::null_mut();
                    let mut length = 0;
                    buffer.Lock(&mut ptr, None, Some(&mut length)).unwrap();
                    decoded_bytes += length;
                    nonzero |= std::slice::from_raw_parts(ptr, length as usize)
                        .iter()
                        .any(|b| *b != 0);
                    buffer.Unlock().unwrap();
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
            }
            assert!(
                decoded_bytes > 300000 && nonzero,
                "encoded AAC must decode to non-silent PCM"
            );
        }
    }
    #[test]
    fn invalid_settings_and_cancellation_preserve_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keep.mp4");
        std::fs::write(&path, b"keep").unwrap();
        let params = yytoolbox::Tool::Recording.defaults();
        let rect = RECT {
            left: -50,
            top: 0,
            right: 50,
            bottom: 50,
        };
        let plan = Plan::new(path.clone(), rect, &params).unwrap();
        assert!(record(plan, &AtomicBool::new(true)).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"keep");
        let mut bad = params;
        bad.insert("seconds".into(), "0".into());
        assert!(Plan::new(dir.path().join("x.mp4"), rect, &bad).is_err());
    }
}

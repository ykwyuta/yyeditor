use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW,
};
use windows::core::HSTRING;

/// 3 桁ごとにカンマを入れる。
pub(crate) fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// バイト数を読みやすい単位で表す。
pub(crate) fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.2} {}", UNITS[i])
}

pub(crate) fn error_box(owner: HWND, text: &str) {
    unsafe {
        MessageBoxW(
            Some(owner),
            &HSTRING::from(text),
            &HSTRING::from("yyeditor"),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub(crate) fn info_box(owner: HWND, text: &str) {
    unsafe {
        MessageBoxW(
            Some(owner),
            &HSTRING::from(text),
            &HSTRING::from("yyeditor"),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

/// NUL 終端付きの UTF-16 文字列。
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

thread_local! {
    static FAILED_STEP: std::cell::Cell<&'static str> = const { std::cell::Cell::new("") };
}

/// Win32 / COM のエラーに「どの処理で失敗したか」を記録する。
///
/// `windows::core::Error::new` はメッセージ付きのエラーを作る際に WinRT のエラー情報
/// （`RoOriginateErrorW`）を生成するため使わず、失敗した処理名を別に保持する。
pub(crate) trait Context<T> {
    fn context(self, what: &'static str) -> windows::core::Result<T>;
}

impl<T> Context<T> for windows::core::Result<T> {
    fn context(self, what: &'static str) -> windows::core::Result<T> {
        if self.is_err() {
            FAILED_STEP.with(|s| s.set(what));
        }
        self
    }
}

/// エラーを「処理名: メッセージ (HRESULT)」の形式で表す。
pub(crate) fn describe_error(e: &windows::core::Error) -> String {
    let step = FAILED_STEP.with(|s| s.get());
    let msg = e.message();
    let code = e.code().0 as u32;
    if step.is_empty() {
        format!("{msg} (0x{code:08X})")
    } else {
        format!("{step}: {msg} (0x{code:08X})")
    }
}

/// BGRA 画素列を 32bpp のトップダウン BMP としてエンコードする。
pub(crate) fn encode_bmp(width: u32, height: u32, bgra: &[u8]) -> Vec<u8> {
    let data_len = width * height * 4;
    let file_len = 14 + 40 + data_len;
    let mut v = Vec::with_capacity(file_len as usize);
    v.extend_from_slice(b"BM");
    v.extend_from_slice(&file_len.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&(14u32 + 40).to_le_bytes());
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&(width as i32).to_le_bytes());
    v.extend_from_slice(&(-(height as i32)).to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&32u16.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    v.extend_from_slice(&data_len.to_le_bytes());
    v.extend_from_slice(&2835u32.to_le_bytes());
    v.extend_from_slice(&2835u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&bgra[..data_len as usize]);
    v
}

/// 画面の部品（タブ・検索バーなど）の文字のフォント。メニュー・ステータスバーと同じ
/// Windows のメッセージのフォント（日本語の Windows では Yu Gothic UI）を `dpi` に合わせて作る。
pub(crate) fn ui_font(dpi: u32) -> windows::Win32::Graphics::Gdi::HFONT {
    use windows::Win32::Graphics::Gdi::{CreateFontIndirectW, LOGFONTW};
    use windows::Win32::UI::HiDpi::SystemParametersInfoForDpi;
    use windows::Win32::UI::WindowsAndMessaging::{NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS};
    unsafe {
        let mut ncm = NONCLIENTMETRICSW {
            cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        let lf = if SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            ncm.cbSize,
            Some(&mut ncm as *mut _ as *mut _),
            0,
            dpi,
        )
        .is_ok()
        {
            ncm.lfMessageFont
        } else {
            // 取得できなければ 9 ポイントの Yu Gothic UI
            let mut lf = LOGFONTW {
                lfHeight: -(12 * dpi as i32 / 96),
                ..Default::default()
            };
            for (d, s) in lf.lfFaceName.iter_mut().zip("Yu Gothic UI".encode_utf16()) {
                *d = s;
            }
            lf
        };
        CreateFontIndirectW(&lf)
    }
}

thread_local! {
    static FAILED_STEP: std::cell::Cell<&'static str> = const { std::cell::Cell::new("") };
}

/// Win32 / COM のエラーに「どの処理で失敗したか」を記録する。
///
/// `windows::core::Error::new` はメッセージ付きのエラーを作る際に WinRT のエラー情報
/// （`RoOriginateErrorW`）を生成するため使わず、失敗した処理名を別に保持する。
pub trait Context<T> {
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
pub fn describe_error(e: &windows::core::Error) -> String {
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
pub fn encode_bmp(width: u32, height: u32, bgra: &[u8]) -> Vec<u8> {
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

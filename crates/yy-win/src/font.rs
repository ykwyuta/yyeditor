//! 同梱フォント（UDEV Gothic）。
//!
//! 実行ファイルに埋め込み、インストールせずにこのプロセスの中だけで使えるよう登録する。
//! ライセンスは `fonts/LICENSE-UDEVGothic.txt`（SIL Open Font License 1.1）。

use std::sync::Once;

use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::AddFontMemResourceEx;
use windows::core::{Interface, Result};

/// 同梱フォントのファミリー名（既定のフォント）。
pub(crate) const BUNDLED_FAMILY: &str = "UDEV Gothic";

static BUNDLED_FONT: &[u8] = include_bytes!("../fonts/UDEVGothic-Regular.ttf");

/// 同梱フォントだけを含むコレクションを作る（DirectWrite 用）。
///
/// `IDWriteFactory5` のない環境（Windows 10 より前）ではエラーを返す。
pub(crate) fn collection(dwrite: &IDWriteFactory) -> Result<IDWriteFontCollection> {
    unsafe {
        let factory: IDWriteFactory5 = dwrite.cast()?;
        let loader = factory.CreateInMemoryFontFileLoader()?;
        factory.RegisterFontFileLoader(&loader)?;
        // 所有者を渡さないとローダーがデータを複製する
        let file = loader.CreateInMemoryFontFileReference(
            &factory,
            BUNDLED_FONT.as_ptr().cast(),
            BUNDLED_FONT.len() as u32,
            None,
        )?;
        let builder = factory.CreateFontSetBuilder()?;
        builder.AddFontFile(&file)?;
        let set = builder.CreateFontSet()?;
        Ok(factory.CreateFontCollectionFromFontSet(&set)?.into())
    }
}

/// 同梱フォントを GDI から使えるようにする（このプロセスだけ。何度呼んでもよい）。
pub(crate) fn register_gdi() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        // 登録したフォントの数が書き込まれる（windows クレートの宣言は *const）
        let mut count = 0u32;
        let _ = AddFontMemResourceEx(
            BUNDLED_FONT.as_ptr().cast(),
            BUNDLED_FONT.len() as u32,
            None,
            (&raw mut count).cast_const(),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{
        CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateFontW, DEFAULT_CHARSET,
        DeleteDC, DeleteObject, FF_MODERN, FW_NORMAL, GetTextFaceW, OUT_DEFAULT_PRECIS,
        SelectObject,
    };
    use windows::core::HSTRING;

    #[test]
    fn directwrite_finds_bundled_family() {
        unsafe {
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap();
            let fonts = collection(&dwrite).unwrap();
            let mut index = 0u32;
            let mut found = windows::core::BOOL(0);
            fonts
                .FindFamilyName(&HSTRING::from(BUNDLED_FAMILY), &mut index, &mut found)
                .unwrap();
            assert!(found.as_bool());
        }
    }

    #[test]
    fn gdi_selects_bundled_family() {
        register_gdi();
        unsafe {
            let font = CreateFontW(
                -16,
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
                &HSTRING::from(BUNDLED_FAMILY),
            );
            let dc = CreateCompatibleDC(None);
            let old = SelectObject(dc, font.into());
            let mut buf = [0u16; 64];
            let n = GetTextFaceW(dc, Some(&mut buf)).max(0) as usize;
            let face = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
            SelectObject(dc, old);
            let _ = DeleteDC(dc);
            let _ = DeleteObject(font.into());
            assert_eq!(face.trim_end_matches('\0'), BUNDLED_FAMILY);
        }
    }
}

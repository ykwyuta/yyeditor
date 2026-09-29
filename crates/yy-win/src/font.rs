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

/// システムのフォントに同梱フォントを加えたコレクションを作る（DirectWrite 用）。
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
        builder.AddFontSet(&factory.GetSystemFontSet()?)?;
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

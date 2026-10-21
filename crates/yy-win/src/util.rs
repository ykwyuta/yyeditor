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

static APP_NAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// アプリの名前（メッセージボックスの見出し。既定は yyeditor）。
pub(crate) fn app_name() -> &'static str {
    APP_NAME.get().copied().unwrap_or("yyeditor")
}

/// アプリの名前を決める（ターミナルは yyterm）。
pub(crate) fn set_app_name(name: &'static str) {
    let _ = APP_NAME.set(name);
}

pub(crate) fn error_box(owner: HWND, text: &str) {
    unsafe {
        MessageBoxW(
            Some(owner),
            &HSTRING::from(text),
            &HSTRING::from(crate::util::app_name()),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub(crate) fn info_box(owner: HWND, text: &str) {
    unsafe {
        MessageBoxW(
            Some(owner),
            &HSTRING::from(text),
            &HSTRING::from(crate::util::app_name()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

/// NUL 終端付きの UTF-16 文字列。
/// 上限付きの数を 1 つ増やす（上限に達していれば増やさず `false`）。同時に動かすものの数を抑えるのに使う。
/// Drop で数を減らす型を作るときは、`true` のときだけ作ること（`then_some(Permit)` は `false` でも作って
/// すぐ捨てるので、数がずれる）。
pub(crate) fn try_increment(n: &std::sync::atomic::AtomicUsize, max: usize) -> bool {
    use std::sync::atomic::Ordering;
    let mut cur = n.load(Ordering::Acquire);
    while cur < max {
        match n.compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(now) => cur = now,
        }
    }
    false
}

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) use yy_ui_win::util::{Context, describe_error, encode_bmp};

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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn increments_up_to_the_limit() {
        let n = AtomicUsize::new(0);
        assert!(super::try_increment(&n, 2));
        assert!(super::try_increment(&n, 2));
        assert!(!super::try_increment(&n, 2));
        assert_eq!(n.load(Ordering::Acquire), 2);
        n.fetch_sub(1, Ordering::AcqRel);
        assert!(super::try_increment(&n, 2));
    }
}

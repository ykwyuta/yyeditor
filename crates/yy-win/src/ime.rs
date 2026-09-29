//! 日本語入力（IMM32）。07 章 3 参照。
//!
//! 変換中の文字列はシステムの変換ウィンドウではなくエディタ内にインライン表示する。
//! そのため `WM_IME_SETCONTEXT` で既定の変換ウィンドウを抑止し、
//! `WM_IME_COMPOSITION` で変換中・確定文字列を取り出す。

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::Input::Ime::*;

/// 変換中・確定した文字列。
pub(crate) struct CompositionUpdate {
    /// 確定した文字列（あれば文書に挿入する）
    pub result: Option<String>,
    /// 変換中の文字列（`Some("")` なら変換中の文字列が消えた）
    pub composing: Option<(String, usize)>,
}

struct Context {
    hwnd: HWND,
    himc: HIMC,
}

impl Context {
    fn get(hwnd: HWND) -> Option<Context> {
        let himc = unsafe { ImmGetContext(hwnd) };
        (!himc.is_invalid()).then_some(Context { hwnd, himc })
    }

    fn string(&self, kind: IME_COMPOSITION_STRING) -> String {
        unsafe {
            let bytes = ImmGetCompositionStringW(self.himc, kind, None, 0);
            if bytes <= 0 {
                return String::new();
            }
            let mut buf = vec![0u16; bytes as usize / 2];
            ImmGetCompositionStringW(
                self.himc,
                kind,
                Some(buf.as_mut_ptr() as *mut _),
                bytes as u32,
            );
            String::from_utf16_lossy(&buf)
        }
    }

    fn cursor(&self) -> usize {
        unsafe { ImmGetCompositionStringW(self.himc, GCS_CURSORPOS, None, 0).max(0) as usize }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            let _ = ImmReleaseContext(self.hwnd, self.himc);
        }
    }
}

/// `WM_IME_COMPOSITION` の `lParam` から、変換中・確定文字列を読み取る。
pub(crate) fn read_composition(hwnd: HWND, flags: u32) -> CompositionUpdate {
    let mut update = CompositionUpdate {
        result: None,
        composing: None,
    };
    let Some(ctx) = Context::get(hwnd) else {
        return update;
    };
    if flags & GCS_RESULTSTR.0 != 0 {
        let s = ctx.string(GCS_RESULTSTR);
        if !s.is_empty() {
            update.result = Some(s);
        }
    }
    if flags & GCS_COMPSTR.0 != 0 {
        let s = ctx.string(GCS_COMPSTR);
        let cursor = if flags & GCS_CURSORPOS.0 != 0 {
            ctx.cursor()
        } else {
            s.encode_utf16().count()
        };
        update.composing = Some((s, cursor));
    } else if update.result.is_some() {
        update.composing = Some((String::new(), 0));
    }
    update
}

/// 変換ウィンドウ・候補ウィンドウをキャレット位置（ビューのクライアント座標、ピクセル）に合わせる。
pub(crate) fn set_position(hwnd: HWND, x: i32, y: i32, line_height: i32) {
    let Some(ctx) = Context::get(hwnd) else {
        return;
    };
    unsafe {
        let cf = COMPOSITIONFORM {
            dwStyle: CFS_POINT,
            ptCurrentPos: POINT { x, y },
            rcArea: RECT::default(),
        };
        let _ = ImmSetCompositionWindow(ctx.himc, &cf);
        let cand = CANDIDATEFORM {
            dwIndex: 0,
            dwStyle: CFS_EXCLUDE,
            ptCurrentPos: POINT {
                x,
                y: y + line_height,
            },
            rcArea: RECT {
                left: x,
                top: y,
                right: x + 1,
                bottom: y + line_height,
            },
        };
        let _ = ImmSetCandidateWindow(ctx.himc, &cand);
    }
}

/// 変換中の文字列を取り消す（ファイルを開き直す場合など）。
pub(crate) fn cancel(hwnd: HWND) {
    if let Some(ctx) = Context::get(hwnd) {
        unsafe {
            let _ = ImmNotifyIME(ctx.himc, NI_COMPOSITIONSTR, CPS_CANCEL, 0);
        }
    }
}

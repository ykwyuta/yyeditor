//! yyeditor の Windows UI 層（Win32 + Direct2D / DirectWrite）。
//!
//! ウィンドウ構成（07 章 1）:
//!
//! * フレームウィンドウ … メニュー、ステータスバー、ファイルのドロップを受け付ける
//! * エディタビュー（子ウィンドウ）… 独自描画のテキストビュー。スクロールバーを持つ
//!
//! アプリケーションの状態 [`app::App`] は UI スレッドのスレッドローカルに 1 つだけ置く。
//! ダイアログ等のモーダルループ中にウィンドウプロシージャが再入するため、
//! 状態を借用したままモーダル処理を呼ばないこと（[`app::with_app`] 参照）。

#![cfg(windows)]
// Win32 API の呼び出しには unsafe が必要
#![allow(unsafe_code)]

mod app;
mod clipboard;
mod goto;
mod ime;
mod render;
mod util;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{ICC_BAR_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};

use crate::util::Context;

pub(crate) const FRAME_CLASS: PCWSTR = w!("YYEditorFrame");
pub(crate) const VIEW_CLASS: PCWSTR = w!("YYEditorView");

/// エディタを起動し、ウィンドウが閉じられるまでメッセージループを回す。
///
/// 起動に失敗した場合は、その内容をメッセージボックスで表示してからエラーを返す。
pub fn run(initial_file: Option<std::path::PathBuf>) -> Result<()> {
    let r = run_inner(initial_file);
    if let Err(e) = &r {
        util::error_box(
            HWND::default(),
            &format!("起動できませんでした。\n{}", util::describe_error(e)),
        );
    }
    r
}

fn run_inner(initial_file: Option<std::path::PathBuf>) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .context("CoInitializeEx")?;
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);

        let hinstance = GetModuleHandleW(None)?.into();
        let cursor = LoadCursorW(None, IDC_ARROW)?;
        let icon = LoadIconW(None, IDI_APPLICATION)?;

        let frame_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(app::frame_proc),
            hInstance: hinstance,
            hCursor: cursor,
            hIcon: icon,
            lpszClassName: FRAME_CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&frame_class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let view_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(app::view_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_IBEAM)?,
            lpszClassName: VIEW_CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&view_class) == 0 {
            return Err(windows::core::Error::from_thread());
        }

        let accel = app::create_accelerators().context("CreateAcceleratorTableW")?;
        let frame = app::App::create(hinstance, initial_file)?;

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if TranslateAcceleratorW(frame, accel, &msg) != 0 {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = DestroyAcceleratorTable(accel);
        app::shutdown();
    }
    Ok(())
}

/// ファイルの先頭 1 画面分を画面外に描画して BMP で保存する（描画の確認・不具合調査用）。
///
/// `yyeditor.exe --render-bmp <入力> <出力.bmp>` から呼ばれる。
pub fn render_to_bmp(
    input: &std::path::Path,
    output: &std::path::Path,
    width: u32,
    height: u32,
) -> std::result::Result<(), String> {
    let describe = |e: windows::core::Error| util::describe_error(&e);
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(describe)?;
    }
    let (config, _) = yy_config::Config::load();
    let mut doc =
        yy_core::Document::open(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let pool = yy_jobs::JobPool::new(2);
    doc.start_indexing(&pool, std::sync::Arc::new(|| {}));
    doc.wait_indexing();
    let mut renderer = render::Renderer::new(
        &config.editor.font_family,
        config.editor.font_size,
        config.editor.tab_width,
        config.colors.clone(),
        96,
    )
    .map_err(describe)?;
    let snap = doc.snapshot();
    let rows_cfg = yy_layout::RowConfig::new(config.view.max_row_bytes.max(256) as u64);
    let page = (height as f32 / renderer.metrics().line_height).ceil() as usize;
    let rows = yy_layout::rows_from(snap, rows_cfg, 0, page);
    let first = snap.line_of_offset(0);
    let frame = render::Frame {
        version: 0,
        rows: &rows,
        first_line: first.line,
        line_exact: first.exact,
        line_digits: snap.estimated_line_count().to_string().len(),
        show_line_numbers: config.view.line_numbers,
        scroll_x: 0.0,
        selections: &[],
        carets: &[],
        caret_visible: false,
        overwrite: false,
        composition: None,
        rect: &[],
    };
    let pixels = renderer
        .render_offscreen(width, height, &frame)
        .map_err(describe)?;
    std::fs::write(output, util::encode_bmp(width, height, &pixels))
        .map_err(|e| format!("{}: {e}", output.display()))
}

#[inline]
pub(crate) fn loword(v: usize) -> u32 {
    (v & 0xFFFF) as u32
}

#[inline]
pub(crate) fn hiword(v: usize) -> u32 {
    ((v >> 16) & 0xFFFF) as u32
}

#[inline]
pub(crate) fn default_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

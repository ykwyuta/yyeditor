//! Editor context menu and HTML-to-Markdown paste command.
use super::*;

pub(super) fn can_paste(app: &App) -> bool {
    app.hex.is_none() && app.code.is_none() && !app.doc.is_read_only() && !app.doc.is_loading()
}

pub(super) fn paste(hwnd: HWND) {
    if with_app(|a| can_paste(a)) != Some(true) {
        return;
    }
    match crate::htmlmarkdown::clipboard_markdown(hwnd) {
        Ok(text) if !text.is_empty() => {
            with_app(|a| a.paste(&text, false));
        }
        Ok(_) => info_box(hwnd, "HTMLにMarkdownへ変換できる本文がありません。"),
        Err(message) => error_box(hwnd, &message),
    }
}

pub(super) fn context_menu(view: HWND, lparam: LPARAM) {
    let Some((frame, markdown, undo, redo, selection, writable)) = with_app(|a| {
        (
            a.frame,
            can_paste(a),
            a.doc.can_undo(),
            a.doc.can_redo(),
            !a.doc.selections().all_empty() || a.rect.is_some_and(|r| !r.is_zero_width()),
            !a.doc.is_read_only() && !a.doc.is_loading(),
        )
    }) else {
        return;
    };
    // Keep the current selection/caret so a right click can replace selected text.
    unsafe {
        let _ = SetFocus(Some(view));
        let Ok(menu) = CreatePopupMenu() else { return };
        for (id, label, enabled) in [
            (ID_UNDO, "元に戻す", undo),
            (ID_REDO, "やり直し", redo),
            (0, "", true),
            (ID_CUT, "切り取り", selection && writable),
            (ID_COPY, "コピー", selection),
            (ID_PASTE, "貼り付け", writable),
            (
                ID_PASTE_MARKDOWN,
                "HTMLをMarkdown化して貼り付ける",
                markdown,
            ),
            (0, "", true),
            (ID_SELECT_ALL, "すべて選択", true),
        ] {
            let flags = if id == 0 {
                MF_SEPARATOR
            } else if enabled {
                MF_STRING
            } else {
                MF_STRING | MF_GRAYED
            };
            let _ = AppendMenuW(menu, flags, id as usize, &HSTRING::from(label));
        }
        let (x, y) = point_of(lparam);
        let mut point = windows::Win32::Foundation::POINT { x, y };
        if (x, y) == (-1, -1) {
            point.x = 16;
            point.y = 16;
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(view, &mut point);
        }
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            Some(0),
            view,
            None,
        );
        let _ = DestroyMenu(menu);
        if command.0 > 0 {
            on_command(frame, command.0 as u16);
        }
    }
}

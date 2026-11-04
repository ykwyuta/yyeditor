//! Native clipboard access for the outline's two context menu paste commands.
use super::*;

pub(super) fn menu_items(
    environment: &ICoreWebView2Environment9,
    shared: &Rc<Shared>,
) -> Result<Vec<ICoreWebView2ContextMenuItem>> {
    let epoch = shared.page.borrow().outline_epoch;
    let mut items = Vec::new();
    for (mode, label) in [
        ("lines", "改行ごとに行を分けて貼り付け"),
        ("single", "一行にまとめて貼り付け"),
    ] {
        unsafe {
            let item = environment.CreateContextMenuItem(
                &HSTRING::from(label),
                None::<&IStream>,
                COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
            )?;
            item.SetIsEnabled(
                windows::Win32::System::DataExchange::IsClipboardFormatAvailable(
                    windows::Win32::System::Ole::CF_UNICODETEXT.0 as u32,
                )
                .is_ok(),
            )?;
            let weak = Rc::downgrade(shared);
            let mut token = 0;
            item.add_CustomItemSelected(
                &webview2_com::CustomItemSelectedEventHandler::create(Box::new(move |_, _| {
                    let Some(shared) = weak.upgrade() else {
                        return Ok(());
                    };
                    if !shared.visible.get() || shared.page.borrow().outline_epoch != epoch {
                        return Ok(());
                    }
                    match clipboard_text(shared.frame) {
                        Ok(text) => post_text(&shared, mode, &text),
                        Err(message) => crate::util::error_box(shared.frame, message),
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            items.push(item);
        }
    }
    Ok(items)
}

fn clipboard_text(owner: HWND) -> std::result::Result<String, &'static str> {
    let bytes = crate::clipboard::get_data(
        owner,
        windows::Win32::System::Ole::CF_UNICODETEXT.0 as u32,
        2 * yy_preview::live::MAX_BYTES + 2,
    )
    .ok_or("クリップボードのテキストを読み取れませんでした。貼り付けは1 MiBまでです。")?;
    let words: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .take_while(|c| *c != 0)
        .collect();
    let text = String::from_utf16_lossy(&words);
    if text.len() > yy_preview::live::MAX_BYTES {
        return Err("貼り付けできるテキストは1 MiBまでです。");
    }
    Ok(text)
}

pub(super) fn post_text(shared: &Shared, mode: &str, text: &str) {
    let Some(epoch) = shared.page.borrow().outline_epoch else {
        return;
    };
    let Some(web) = shared.web.borrow().clone() else {
        return;
    };
    let message = serde_json::json!({"epoch":epoch,"outlinePaste":mode,"text":text}).to_string();
    unsafe {
        let _ = web.webview.PostWebMessageAsJson(&HSTRING::from(message));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::testing::{eval, pump_until, webview2_lock};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;

    #[test]
    fn outline_context_paste_modes_preserve_range_metadata_and_readonly_in_webview2() {
        let _serial = webview2_lock();
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            RegisterClassExW(&WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(preview_proc),
                hInstance: GetModuleHandleW(None).unwrap().into(),
                lpszClassName: PREVIEW_CLASS,
                ..Default::default()
            });
        }
        struct Parent(HWND);
        impl Drop for Parent {
            fn drop(&mut self) {
                unsafe {
                    let _ = DestroyWindow(self.0);
                }
            }
        }
        let parent = Parent(unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PREVIEW_CLASS,
                w!("outline paste test"),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                0,
                0,
                640,
                480,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        });
        let source = serde_json::json!({"format":"yyoutline","version":1,"nodes":[
            {"id":"root","depth":0,"text":"前😀選択後","bookmark":true,"collapsed":false,"refs":[]},
            {"id":"child","depth":1,"text":"子","bookmark":false,"collapsed":false,"refs":[]},
            {"id":"next","depth":0,"text":"次","bookmark":false,"collapsed":false,"refs":["root"]}
        ]})
        .to_string();
        let mut pane = Preview::new(parent.0, parent.0).unwrap();
        pane.set_visible(true);
        pane.set_bounds(0, 0, 620, 440);
        pane.show_outline(&source, 301, false);
        assert!(
            pump_until(Duration::from_secs(45), || pane
                .take_edit_messages()
                .iter()
                .any(|m| m.split('\n').nth(2) == Some("ready"))
                || pane.is_unavailable()),
            "outline did not load"
        );
        if pane.is_unavailable() {
            assert!(
                std::env::var_os("YY_REQUIRE_WEBVIEW2").is_none(),
                "WebView2 required: {}",
                notice_of(pane.hwnd)
            );
            return;
        }
        let environment = unsafe {
            pane.web()
                .unwrap()
                .webview
                .cast::<ICoreWebView2_2>()
                .unwrap()
                .Environment()
                .unwrap()
        };
        let menu = menu_items(&environment.cast().unwrap(), &pane.shared).unwrap();
        let labels: Vec<_> = menu
            .iter()
            .map(|item| take_string(|p| unsafe { item.Label(p) }))
            .collect();
        assert_eq!(
            labels,
            ["改行ごとに行を分けて貼り付け", "一行にまとめて貼り付け"]
        );
        assert_eq!(
            eval(
                &pane,
                r#"
            window.pasteAt=(id,start,stop)=>{
              const input=document.querySelector(`.row[data-id="${id}"] input`);
              input.focus();input.setSelectionRange(start,stop);
              input.dispatchEvent(new MouseEvent('contextmenu',{bubbles:true}));
            };
            document.querySelector('.row[data-id="root"] .zoom').click();
            pasteAt('root',3,5);true
        "#
            )
            .as_deref(),
            Some("true")
        );
        post_text(&pane.shared, "lines", "一\r\n二");
        let mut messages = Vec::new();
        assert!(
            pump_until(Duration::from_secs(10), || {
                messages.extend(pane.take_edit_messages());
                messages
                    .iter()
                    .filter(|m| m.split('\n').nth(2) == Some("paste"))
                    .count()
                    == 1
            }),
            "split paste missing: {messages:?}"
        );
        assert_eq!(
            eval(
                &pane,
                r#"JSON.stringify([...document.querySelectorAll('#tree input')].map(i=>i.value))"#
            )
            .as_deref(),
            Some(r#""[\"前😀一\",\"子\",\"二後\",\"次\"]""#)
        );
        assert_eq!(eval(&pane, "document.activeElement.value==='二後' && document.activeElement.selectionStart===1 && document.querySelectorAll('#crumbs button').length===1").as_deref(), Some("true"));
        let split_source = messages
            .iter()
            .find(|m| m.split('\n').nth(2) == Some("paste"))
            .unwrap()
            .splitn(6, '\n')
            .nth(5)
            .unwrap();
        let split: serde_json::Value = serde_json::from_str(split_source).unwrap();
        assert_eq!(split["nodes"][0]["id"], "root");
        assert_eq!(split["nodes"][0]["bookmark"], true);
        assert_eq!(split["nodes"][1]["id"], "child");
        assert_eq!(split["nodes"][3]["refs"][0], "root");
        eval(&pane, "pasteAt('root',3,4)").unwrap();
        post_text(&pane.shared, "single", "A\n\nB");
        assert!(
            pump_until(Duration::from_secs(10), || {
                messages.extend(pane.take_edit_messages());
                messages
                    .iter()
                    .filter(|m| m.split('\n').nth(2) == Some("paste"))
                    .count()
                    == 2
            }),
            "single paste missing: {messages:?}"
        );
        assert_eq!(eval(&pane, "document.querySelector('.row[data-id=root] input').value==='前😀A  B' && document.querySelectorAll('#tree input').length===4 && document.activeElement.selectionStart===7").as_deref(), Some("true"));
        // An intervening edit invalidates the captured range instead of pasting elsewhere.
        eval(&pane, "pasteAt('root',0,0);document.activeElement.value='変更';document.activeElement.dispatchEvent(new Event('input'))").unwrap();
        post_text(&pane.shared, "lines", "無効\n無効");
        pane.editor_post("{\"flush\":301}");
        assert!(
            pump_until(Duration::from_secs(10), || {
                messages.extend(pane.take_edit_messages());
                messages
                    .iter()
                    .any(|m| m.split('\n').nth(2) == Some("flush:301"))
            }),
            "flush missing: {messages:?}"
        );
        let flushed = messages
            .iter()
            .find(|m| m.split('\n').nth(2) == Some("flush:301"))
            .unwrap()
            .splitn(6, '\n')
            .nth(5)
            .unwrap();
        let flushed: serde_json::Value = serde_json::from_str(flushed).unwrap();
        assert_eq!(flushed["nodes"][0]["text"], "変更");
        assert_eq!(flushed["nodes"].as_array().unwrap().len(), 4);
        assert_eq!(
            messages
                .iter()
                .filter(|m| m.split('\n').nth(2) == Some("paste"))
                .count(),
            2
        );
        let latest = messages
            .iter()
            .rev()
            .find(|m| m.split('\n').nth(2) == Some("paste"))
            .unwrap()
            .splitn(6, '\n')
            .nth(5)
            .unwrap()
            .to_owned();
        pane.show_outline(&latest, 302, true);
        assert!(pump_until(Duration::from_secs(20), || pane
            .take_edit_messages()
            .iter()
            .any(
                |m| m.starts_with("302\n") && m.split('\n').nth(2) == Some("ready")
            )));
        assert_eq!(pane.shared.page.borrow().outline_epoch, None);
        eval(&pane, "var input=document.querySelector('.row[data-id=root] input');input.focus();input.setSelectionRange(0,0);input.dispatchEvent(new MouseEvent('contextmenu',{bubbles:true}))").unwrap();
        post_text(&pane.shared, "lines", "禁止\n禁止");
        assert_eq!(eval(&pane, "document.querySelector('.row[data-id=root] input').readOnly && document.querySelector('.row[data-id=root] input').value==='前😀A  B' && document.querySelectorAll('#tree input').length===4").as_deref(), Some("true"));
    }
}

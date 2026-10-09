//! ヘルプ（F1）。実行ファイルに埋め込んだ Markdown（`help/help.md`）を別のウィンドウに表示する。
//!
//! 表示はプレビュー（[`crate::preview`]、WebView2）を使う。WebView2 を使えない環境では、
//! 同じ文章を読み取り専用のテキスト欄に表示する。

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{DeleteObject, HFONT};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Result, w};
use yy_preview::PageOptions;

use crate::HELP_CLASS;
use crate::preview::{Preview, WM_APP_PREVIEW_FAILED};
use crate::util::Context;

/// ヘルプの本文（Markdown）。
pub(crate) const HELP_MD: &str = include_str!("../help/help.md");

/// yysheet のヘルプ（利用ガイド）の本文。
pub(crate) const SHEET_MD: &str = include_str!("../help/sheet.md");

/// yyfilemanager のヘルプの本文。
pub(crate) const FM_MD: &str = include_str!("../help/filemanager.md");

/// yybrowser のヘルプの本文。
pub(crate) const BROWSER_MD: &str = include_str!("../help/browser.md");

thread_local! {
    /// 表示するヘルプ（本文・ウィンドウの題名）。yysheet は [`use_sheet_help`] で切り替える
    static DOC: std::cell::Cell<(&'static str, &'static str)> =
        const { std::cell::Cell::new((HELP_MD, "yyeditor ヘルプ")) };
}

fn doc() -> &'static str {
    DOC.with(|d| d.get().0)
}

/// yysheet のヘルプを表示するようにし、ヘルプのウィンドウのクラス（とプレビュー）を登録する。
pub(crate) fn use_sheet_help(hinstance: windows::Win32::Foundation::HINSTANCE) {
    use_app_help(hinstance, SHEET_MD, "yysheet ヘルプ");
}

/// エディタ以外のアプリのヘルプ（本文・題名）を表示するようにし、ヘルプのウィンドウのクラス
/// （とプレビュー）を登録する。
pub(crate) fn use_app_help(
    hinstance: windows::Win32::Foundation::HINSTANCE,
    md: &'static str,
    title: &'static str,
) {
    DOC.with(|d| d.set((md, title)));
    unsafe {
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        for (class, proc_, bg) in [
            (
                HELP_CLASS,
                help_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
                true,
            ),
            (crate::PREVIEW_CLASS, crate::preview::preview_proc, false),
        ] {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc_),
                hInstance: hinstance,
                hCursor: cursor,
                hbrBackground: if bg {
                    windows::Win32::Graphics::Gdi::HBRUSH(
                        (windows::Win32::Graphics::Gdi::COLOR_WINDOW.0 + 1) as usize as *mut _,
                    )
                } else {
                    Default::default()
                },
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassExW(&wc);
        }
    }
}

/// 開いているヘルプのウィンドウ。
struct HelpWindow {
    hwnd: HWND,
    preview: Preview,
    /// WebView2 を使えないときに本文を表示する欄
    text: Option<(HWND, HFONT)>,
}

thread_local! {
    static HELP: RefCell<Option<HelpWindow>> = const { RefCell::new(None) };
}

/// 見出し `{#section}` の行（0 始まり）。
pub(crate) fn section_line(section: &str) -> Option<usize> {
    section_line_in(doc(), section)
}

fn section_line_in(md: &str, section: &str) -> Option<usize> {
    let tag = format!("{{#{section}}}");
    md.lines()
        .position(|l| l.starts_with('#') && l.trim_end().ends_with(&tag))
}

/// ヘルプを表示する（開いていれば前面に出す）。`section` があればその見出しへ移動する。
pub(crate) fn show(section: Option<&str>) -> Result<()> {
    let line = section.and_then(section_line).unwrap_or(0);
    let existing = HELP.with(|h| h.borrow().as_ref().map(|w| w.hwnd));
    if let Some(hwnd) = existing {
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
        HELP.with(|h| {
            if let Some(w) = h.borrow().as_ref() {
                w.preview.scroll_to_line(line);
                if let Some((edit, _)) = w.text {
                    scroll_text_to_line(edit, line);
                }
            }
        });
        return Ok(());
    }
    let hwnd = unsafe {
        let hinstance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        let dpi = windows::Win32::UI::HiDpi::GetDpiForSystem() as i32;
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            HELP_CLASS,
            &HSTRING::from(DOC.with(|d| d.get().1)),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            860 * dpi / 96,
            760 * dpi / 96,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
        .context("CreateWindowExW(help)")?
    };
    let mut preview = Preview::new(hwnd, hwnd)?;
    let registry = yy_syntax::Registry::builtin();
    let body = yy_preview::markdown_to_html(doc(), Some(&registry));
    let opts = PageOptions {
        has_folder: false,
        token_colors: yy_config::Colors::default()
            .syntax
            .iter()
            .map(|(k, c)| (k.clone(), format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)))
            .collect(),
    };
    preview.set_visible(true);
    preview.show_markdown(&body, &opts, None, Some(line));
    let failed = preview.is_unavailable();
    HELP.with(|h| {
        *h.borrow_mut() = Some(HelpWindow {
            hwnd,
            preview,
            text: None,
        })
    });
    if failed {
        show_text_fallback(hwnd, line);
    }
    unsafe {
        layout(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

/// ヘルプのウィンドウか（その子ウィンドウを含む）。メインのショートカットを働かせないために使う。
pub(crate) fn contains(hwnd: HWND) -> bool {
    HELP.with(|h| {
        h.borrow()
            .as_ref()
            .is_some_and(|w| unsafe { hwnd == w.hwnd || IsChild(w.hwnd, hwnd).as_bool() })
    })
}

/// WebView2 を使えないとき: 本文を読み取り専用のテキスト欄に表示する。
fn show_text_fallback(hwnd: HWND, line: usize) {
    let already = HELP.with(|h| h.borrow().as_ref().is_some_and(|w| w.text.is_some()));
    if already {
        return;
    }
    let text = plain_text(doc()).replace('\n', "\r\n");
    let edit = unsafe {
        let style = WS_CHILD
            | WS_VISIBLE
            | WS_VSCROLL
            | WINDOW_STYLE((ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            &HSTRING::from(text),
            style,
            0,
            0,
            0,
            0,
            Some(hwnd),
            None,
            None,
            None,
        )
    };
    let Ok(edit) = edit else {
        return;
    };
    let font = crate::util::ui_font(unsafe { GetDpiForWindow(hwnd) });
    unsafe {
        SendMessageW(
            edit,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
    }
    HELP.with(|h| {
        if let Some(w) = h.borrow_mut().as_mut() {
            w.text = Some((edit, font));
            w.preview.set_visible(false);
        }
    });
    unsafe {
        layout(hwnd);
    }
    scroll_text_to_line(edit, line);
}

/// テキスト欄に表示するための、Markdown の記号を減らした本文（行の対応は変えない）。
/// `**強調**`・見出しの `{#id}`・リンクの `[文字](#先)` を取り除く。
fn plain_text(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    for line in md.lines() {
        let mut line = line.replace("**", "");
        if line.starts_with('#')
            && let Some(i) = line.rfind(" {#")
        {
            line.truncate(i);
        }
        // [文字](#先) → 文字
        let mut rest = line.as_str();
        let mut plain = String::new();
        while let Some(open) = rest.find('[') {
            let Some(mid) = rest[open..].find("](") else {
                break;
            };
            let Some(close) = rest[open + mid..].find(')') else {
                break;
            };
            plain.push_str(&rest[..open]);
            plain.push_str(&rest[open + 1..open + mid]);
            rest = &rest[open + mid + close + 1..];
        }
        plain.push_str(rest);
        out.push_str(&plain);
        out.push('\n');
    }
    out
}

fn scroll_text_to_line(edit: HWND, line: usize) {
    use windows::Win32::UI::Controls::{EM_GETFIRSTVISIBLELINE, EM_LINESCROLL};
    unsafe {
        let first = SendMessageW(edit, EM_GETFIRSTVISIBLELINE, None, None).0;
        SendMessageW(
            edit,
            EM_LINESCROLL,
            Some(WPARAM(0)),
            Some(LPARAM(line as isize - first)),
        );
    }
}

/// 子ウィンドウをクライアント領域いっぱいに広げる。
unsafe fn layout(hwnd: HWND) {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    HELP.with(|h| {
        if let Some(w) = h.borrow().as_ref() {
            w.preview.set_bounds(0, 0, rc.right, rc.bottom);
            if let Some((edit, _)) = w.text {
                unsafe {
                    let _ = MoveWindow(edit, 0, 0, rc.right, rc.bottom, true);
                }
            }
        }
    });
}

/// ヘルプのウィンドウプロシージャ。
pub(crate) extern "system" fn help_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_SIZE => {
                layout(hwnd);
                LRESULT(0)
            }
            WM_APP_PREVIEW_FAILED => {
                show_text_fallback(hwnd, 0);
                LRESULT(0)
            }
            // プレビューにフォーカスがあるときのショートカット（Ctrl+W で閉じる）
            WM_COMMAND if crate::app::is_close_command(wparam) => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                if let Some(w) = HELP.with(|h| h.borrow_mut().take())
                    && let Some((_, font)) = w.text
                {
                    let _ = DeleteObject(font.into());
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::testing::{eval, pump_until};
    use std::time::Duration;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

    /// メニューに書いたショートカット（`\t` の後ろ）はすべてヘルプに載っている。
    #[test]
    fn help_lists_every_menu_shortcut() {
        let sources = [
            include_str!("app.rs"),
            include_str!("app/csvmode.rs"),
            include_str!("app/hexmode.rs"),
            include_str!("app/syntaxmode.rs"),
        ];
        let mut found = 0;
        for src in sources {
            // メニューの項目の文字列（w!("…")）の `\t` より後ろ
            let labels = src
                .split("w!(\"")
                .skip(1)
                .filter_map(|p| p.find("\")").map(|e| &p[..e]));
            for label in labels {
                let Some((_, key)) = label.split_once("\\t") else {
                    continue;
                };
                assert!(
                    HELP_MD.contains(&format!("| {key} |"))
                        || HELP_MD.contains(&format!("| {key} /"))
                        || HELP_MD.contains(&format!("/ {key} |"))
                        || HELP_MD.contains(&format!("/ {key} /")),
                    "ヘルプのショートカットの表に「{key}」がない"
                );
                found += 1;
            }
        }
        assert!(found > 30, "{found}");
    }

    #[test]
    fn plain_text_drops_markup_but_keeps_lines() {
        let t = plain_text("## 見出し {#id}\n- **太字** と [リンク](#x) と `code`\n- [x] task\n");
        assert_eq!(t, "## 見出し\n- 太字 と リンク と `code`\n- [x] task\n");
        assert_eq!(plain_text(HELP_MD).lines().count(), HELP_MD.lines().count());
    }

    /// 目次のリンク先の見出しがすべてある。
    #[test]
    fn table_of_contents_links_resolve() {
        for md in [HELP_MD, SHEET_MD, FM_MD] {
            let mut n = 0;
            for part in md.split("](#").skip(1) {
                let id = &part[..part.find(')').unwrap()];
                assert!(section_line_in(md, id).is_some(), "見出し {{#{id}}} がない");
                n += 1;
            }
            assert!(n >= 10);
            let html = yy_preview::markdown_to_html(md, None);
            assert!(html.contains("<h2 id=\"shortcuts\""));
            assert_eq!(plain_text(md).lines().count(), md.lines().count());
        }
    }

    /// yysheet のヘルプの COBOL の型の表のバイト数が、yy-cobol で求めた長さと合う。
    #[test]
    fn sheet_help_cobol_types_match() {
        let start = section_line_in(SHEET_MD, "cobol-types").unwrap();
        let mut n = 0;
        for line in SHEET_MD.lines().skip(start) {
            if line.starts_with("## ") && n > 0 {
                break;
            }
            let Some(rest) = line.strip_prefix("| `") else {
                continue;
            };
            let (ty, rest) = rest.split_once("` | ").unwrap();
            let bytes: usize = rest.split(' ').next().unwrap().parse().unwrap();
            let (_, len) = yy_cobol::check_type(ty).unwrap_or_else(|e| panic!("{ty}: {e}"));
            assert_eq!(len, bytes, "{ty}");
            n += 1;
        }
        assert!(n >= 30, "{n}");
    }

    /// yysheet のヘルプのレイアウトカタログの定義ファイルの例が、そのまま読める。
    #[test]
    fn sheet_help_catalog_example_parses() {
        let start = section_line_in(SHEET_MD, "catalog").unwrap();
        let lines: Vec<&str> = SHEET_MD.lines().skip(start).collect();
        let open = lines.iter().position(|l| l.trim() == "```toml").unwrap();
        let close = open
            + 1
            + lines[open + 1..]
                .iter()
                .position(|l| l.trim() == "```")
                .unwrap();
        let text: String = lines[open + 1..close]
            .iter()
            .map(|l| format!("{}\n", l.strip_prefix("  ").unwrap_or(l)))
            .collect();
        let def = yy_sheet::catalog::parse_def(&text).unwrap();
        let spec = def
            .to_spec(
                yy_cobol::Codec::new(yy_cobol::Charset::Ms932),
                yy_sheet::fixed::RecordSep::Crlf,
            )
            .unwrap();
        assert_eq!(spec.layout.record_len, 11);
        assert!(spec.codec.charset.is_ebcdic());
    }

    /// yysheet のメニューのショートカットと関数が、yysheet のヘルプの表にある。
    #[test]
    fn sheet_help_lists_shortcuts_and_functions() {
        let src = include_str!("sheet/mod.rs");
        let mut found = 0;
        // メニューの項目: add(メニュー, ID_…, "文字列")
        for label in src
            .split(" add(")
            .skip(1)
            .filter_map(|p| p.lines().next())
            .filter(|head| head.contains(", ID_"))
            .filter_map(|head| head.split_once('"').map(|(_, r)| r))
            .filter_map(|p| p.find('"').map(|e| &p[..e]))
        {
            let Some((_, key)) = label.split_once("\\t") else {
                continue;
            };
            // 表示形式の候補（{code}）は除く。`Ctrl++（ホイール）` は `Ctrl++`
            if key.contains('{') {
                continue;
            }
            let key = key.split('（').next().unwrap_or(key);
            assert!(
                SHEET_MD.contains(&format!("| {key} |"))
                    || SHEET_MD.contains(&format!("| {key} /"))
                    || SHEET_MD.contains(&format!("/ {key} |"))
                    || SHEET_MD.contains(&format!("/ {key} /")),
                "yysheet のヘルプのショートカットの表に「{key}」がない"
            );
            found += 1;
        }
        assert!(found > 10, "{found}");
        for f in yy_formula::FUNCTIONS {
            assert!(
                SHEET_MD.contains(&format!("| `{}` |", f.name)),
                "yysheet のヘルプの関数の表に {} がない",
                f.name
            );
        }
    }

    /// ヘルプのウィンドウを開いて、ショートカットの節まで移動する。WebView2 を使えない環境
    /// （Wine など）では本文をテキスト欄に表示する。
    #[test]
    fn opens_help_window_at_section() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap();
            for (class, proc_) in [
                (
                    HELP_CLASS,
                    help_proc as unsafe extern "system" fn(_, _, _, _) -> _,
                ),
                (crate::PREVIEW_CLASS, crate::preview::preview_proc),
            ] {
                let wc = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(proc_),
                    hInstance: instance.into(),
                    lpszClassName: class,
                    ..Default::default()
                };
                RegisterClassExW(&wc);
            }
        }
        show(Some("shortcuts")).unwrap();
        let state = || {
            HELP.with(|h| {
                h.borrow()
                    .as_ref()
                    .map(|w| (w.preview.loaded_kind().is_some(), w.text.map(|t| t.0)))
            })
        };
        pump_until(Duration::from_secs(60), || {
            matches!(state(), Some((true, _)) | Some((_, Some(_))))
        });
        let (loaded, text) = state().expect("help window");
        let hwnd = HELP.with(|h| h.borrow().as_ref().unwrap().hwnd);
        assert!(contains(hwnd));
        if let Some(edit) = text {
            // テキスト欄に本文が入っていて、節の近くまでスクロールしている
            assert!(contains(edit));
            let len = unsafe { GetWindowTextLengthW(edit) };
            // 改行を CRLF にしているので、元の文字数（UTF-16）以上
            let expected = plain_text(HELP_MD).encode_utf16().count();
            assert!(len as usize >= expected, "{len} < {expected}");
            let first = unsafe {
                SendMessageW(
                    edit,
                    windows::Win32::UI::Controls::EM_GETFIRSTVISIBLELINE,
                    None,
                    None,
                )
                .0
            };
            assert!(first > 0, "{first}");
        } else {
            assert!(loaded);
            let ok = pump_until(Duration::from_secs(15), || {
                HELP.with(|h| {
                    let h = h.borrow();
                    let w = h.as_ref().unwrap();
                    let r = eval(
                        &w.preview,
                        "document.querySelector('h1').textContent + '|' + \
                         Math.round(document.getElementById('shortcuts').getBoundingClientRect().top)",
                    )
                    .unwrap_or_default();
                    // 見出しがページの上端付近にある
                    r.strip_prefix("\"yyeditor ヘルプ|")
                        .and_then(|t| t.trim_end_matches('"').parse::<i32>().ok())
                        .is_some_and(|top| (-20..120).contains(&top))
                })
            });
            assert!(ok, "did not scroll to the shortcuts section");
        }
        // 開いているときにもう一度開いても 1 つのまま
        show(None).unwrap();
        assert_eq!(HELP.with(|h| h.borrow().as_ref().unwrap().hwnd), hwnd);
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        assert!(HELP.with(|h| h.borrow().is_none()));
    }
}

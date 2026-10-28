//! Markdown / HTML のプレビュー（エディタの右側）: 表示の切り替え、内容の更新、スクロールの同期、
//! 境界の移動。表示そのものは [`crate::preview`]。

use std::path::Path;
use yy_preview::{Kind, PageOptions};

use super::*;
use crate::preview::Preview;

/// 「プレビュー」（表示メニュー）
pub(crate) const ID_PREVIEW: u16 = 209;
pub(crate) const ID_SAVE_EXPANDED: u16 = 118;
pub(crate) const ID_SNIPPETS: u16 = 119;
/// 編集してからプレビューを更新するまでの時間（ミリ秒）
const PREVIEW_DELAY_MS: u32 = 250;
/// これより大きな文書はプレビューしない
const PREVIEW_MAX_BYTES: u64 = 8 << 20;
/// エディタとプレビューの境界の幅（96 DPI でのピクセル）
const SPLITTER: i32 = 6;

/// プレビューの表示状態。
pub(crate) struct PreviewPane {
    pub pane: Option<Preview>,
    pub visible: bool,
    /// フレームの幅に対するプレビューの幅の割合
    pub ratio: f32,
    /// 境界をドラッグ中
    pub dragging: bool,
    /// 境界の範囲（フレームのクライアント座標）
    pub splitter: RECT,
}

impl Default for PreviewPane {
    fn default() -> Self {
        PreviewPane {
            pane: None,
            visible: false,
            ratio: 0.5,
            dragging: false,
            splitter: RECT::default(),
        }
    }
}

impl App {
    /// プレビューの表示を切り替える。
    pub(crate) fn toggle_preview(&mut self) -> std::result::Result<(), String> {
        let visible = !self.preview.visible;
        if visible && self.preview.pane.is_none() {
            let pane = Preview::new(self.frame, self.frame).map_err(|e| e.message())?;
            self.preview.pane = Some(pane);
        }
        self.preview.visible = visible;
        self.layout_children();
        if let Some(p) = &mut self.preview.pane {
            p.set_visible(visible);
        }
        self.update_preview_menu();
        if visible {
            self.refresh_preview();
        }
        self.invalidate();
        Ok(())
    }

    pub(crate) fn update_preview_menu(&self) {
        let flag = if self.preview.visible {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        unsafe {
            CheckMenuItem(self.menu_view, ID_PREVIEW as u32, (MF_BYCOMMAND | flag).0);
        }
    }

    /// エディタの幅を決めてプレビューを置く。プレビューを表示していなければ全幅を返す。
    /// `left` はエディタを置く範囲の左端（フレームのクライアント座標）、`width` はその幅。
    pub(crate) fn layout_preview(&mut self, left: i32, width: i32, top: i32, height: i32) -> i32 {
        let Some(pane) = self.preview.pane.as_ref().filter(|_| self.preview.visible) else {
            self.preview.splitter = RECT::default();
            return width;
        };
        let dpi = unsafe { GetDpiForWindow(self.frame) }.max(96) as i32;
        let split = SPLITTER * dpi / 96;
        let min = 120 * dpi / 96;
        let pw = ((width as f32 * self.preview.ratio) as i32)
            .clamp(min.min(width / 2), (width - min).max(0));
        let editor = (width - pw - split).max(0);
        pane.set_bounds(left + editor + split, top, pw, height);
        self.preview.splitter = RECT {
            left: left + editor,
            top,
            right: left + editor + split,
            bottom: top + height,
        };
        editor
    }

    /// フレームのクライアント座標 `(x, y)` がエディタとプレビューの境界の上か。
    pub(crate) fn on_preview_splitter(&self, x: i32, y: i32) -> bool {
        let r = self.preview.splitter;
        self.preview.visible && x >= r.left && x < r.right && y >= r.top && y < r.bottom
    }

    /// 境界をドラッグして `x` に動かす。
    pub(crate) fn drag_preview_splitter(&mut self, x: i32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.frame, &mut rc);
        }
        let w = rc.right - rc.left;
        // サイドバーの右からフレームの右端までの幅に対する割合
        let area = (w - self.ws.splitter.right).max(1);
        self.preview.ratio = ((w - x) as f32 / area as f32).clamp(0.1, 0.9);
        self.layout_children();
        self.invalidate();
    }

    /// 編集後、少し待ってからプレビューを更新する。
    pub(crate) fn schedule_preview(&mut self) {
        if self.preview.visible {
            unsafe {
                SetTimer(Some(self.view), TIMER_PREVIEW, PREVIEW_DELAY_MS, None);
            }
        }
    }

    /// 作業中の文書でプレビューを作り直す。
    pub(crate) fn refresh_preview(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.view), TIMER_PREVIEW);
        }
        if !self.preview.visible {
            return;
        }
        let Some(pane) = self.preview.pane.as_ref() else {
            return;
        };
        let syntax_id = self.syntax.as_ref().map(|s| s.view.syntax().id.clone());
        let path = self.doc.location();
        let Some(kind) = Kind::detect(syntax_id.as_deref(), path.as_deref()) else {
            pane.show_notice("Markdown・HTML の文書を開くと、ここにプレビューを表示します。");
            return;
        };
        let snap = self.doc.snapshot();
        if snap.len() > PREVIEW_MAX_BYTES {
            pane.show_notice("文書が大きすぎるため、プレビューしません（8 MiB まで）。");
            return;
        }
        let text = String::from_utf8_lossy(&snap.read(0..snap.len())).into_owned();
        // 相対パスの画像などの起点（リモートの文書は接続先のフォルダから取り寄せる）
        let folder = match self
            .doc
            .remote()
            .and_then(|r| yy_remote::RemoteUri::parse(&r.uri))
        {
            Some(u) => crate::remote::connected(&u.target()).map(|session| {
                crate::preview::DocBase::Remote {
                    session,
                    dir: yy_remote::RemoteUri {
                        path: yy_proto::parent_path(&u.path),
                        ..u
                    },
                }
            }),
            None => path
                .as_deref()
                .and_then(|p| p.parent())
                .map(|p| crate::preview::DocBase::Local(p.to_owned())),
        };
        let mut opts = PageOptions {
            has_folder: folder.is_some(),
            token_colors: self
                .config
                .colors
                .syntax
                .iter()
                .map(|(k, c)| (k.clone(), format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)))
                .collect(),
        };
        match kind {
            Kind::Markdown => {
                let snippets = if self.doc.remote().is_none() {
                    yy_config::config_dir().map(|p| p.join("snippets"))
                } else {
                    None
                };
                let local_path = if self.doc.remote().is_none() {
                    path.as_deref()
                } else {
                    None
                };
                let expanded =
                    yy_preview::transclusion::expand(&text, local_path, snippets.as_deref(), false);
                let body = expanded.html(Some(&self.syntaxes));
                let folder = if self.doc.remote().is_none() {
                    Some(crate::preview::DocBase::Embedded {
                        root: local_path.and_then(Path::parent).map(Path::to_path_buf),
                        snippets,
                    })
                } else {
                    folder
                };
                opts.has_folder = folder.is_some();
                let line = self.preview_line();
                pane.show_markdown(&body, &opts, folder, Some(line));
            }
            Kind::Html => pane.show_html(&text, &opts, folder),
        }
    }

    /// 表示中の先頭の行（プレビューのスクロールを合わせる）。
    fn preview_line(&self) -> usize {
        self.doc.snapshot().line_of_offset(self.vp.top).line as usize
    }

    /// エディタのスクロールにプレビューを合わせる。
    pub(crate) fn sync_preview_scroll(&self) {
        if let Some(pane) = self.preview.pane.as_ref().filter(|_| self.preview.visible) {
            pane.scroll_to_line(self.preview_line());
        }
    }
}

/// プレビューにフォーカスがあるときのショートカット（保存・タブの切り替えなど、エディタ全体に
/// 関わるもの）をフレームのコマンドにする。コマンドにしたら `true`。
pub(crate) fn translate_preview_shortcut(frame: HWND, vk: u32) -> bool {
    let (ctrl, shift) = crate::preview::modifiers();
    let vk = VIRTUAL_KEY(vk as u16);
    let id = if ctrl {
        match (vk.0 as u8, shift) {
            _ if vk == VK_TAB => Some(if shift { ID_TAB_PREV } else { ID_TAB_NEXT }),
            (b'S', false) => Some(ID_SAVE),
            (b'S', true) => Some(ID_SAVE_AS),
            (b'O', false) => Some(ID_OPEN),
            (b'N', false) => Some(ID_NEW),
            (b'W', false) => Some(ID_CLOSE),
            (b'G', false) => Some(ID_GOTO),
            (b'F', false) => Some(ID_FIND),
            (b'F', true) => Some(ID_GREP),
            (b'H', false) => Some(ID_REPLACE),
            (b'V', true) => Some(ID_PREVIEW),
            _ => None,
        }
    } else if vk == VK_F3 {
        Some(if shift { ID_FIND_PREV } else { ID_FIND_NEXT })
    } else if vk == VK_F1 {
        Some(ID_HELP)
    } else {
        None
    };
    let Some(id) = id else {
        return false;
    };
    unsafe {
        let _ = PostMessageW(Some(frame), WM_COMMAND, WPARAM(id as usize), LPARAM(0));
    }
    true
}

/// Export a flattened copy without changing the active document, its path or Undo.
pub(super) fn save_expanded(hwnd: HWND) {
    let prepared = with_app(|a| -> std::result::Result<_, String> {
        if a.doc.is_busy() {
            return Err("文書の処理が完了してからインライン展開保存してください。".into());
        }
        if a.doc.remote().is_some() {
            return Err("インライン展開保存はローカルのMarkdown文書で利用できます。".into());
        }
        let path = a.doc.path().map(Path::to_path_buf);
        let syntax = a.syntax.as_ref().map(|s| s.view.syntax().id.as_str());
        if Kind::detect(syntax, path.as_deref()) != Some(Kind::Markdown) {
            return Err("Markdown文書を開いてください。".into());
        }
        let snapshot = a.doc.snapshot();
        if snapshot.len() > PREVIEW_MAX_BYTES {
            return Err("展開保存は8 MiBまでの文書に対応します。".into());
        }
        let source = String::from_utf8(snapshot.read(0..snapshot.len()))
            .map_err(|_| "UTF-8として読み取れません")?;
        let snippets = yy_config::config_dir().map(|p| p.join("snippets"));
        let expanded =
            yy_preview::transclusion::expand(&source, path.as_deref(), snippets.as_deref(), true);
        if !expanded.errors.is_empty() {
            return Err(format!(
                "参照を解決できないため保存しません。\n{}",
                expanded.errors.join("\n")
            ));
        }
        Ok((path, expanded))
    });
    let (current, expanded) = match prepared {
        Some(Ok(value)) => value,
        Some(Err(error)) => {
            error_box(hwnd, &error);
            return;
        }
        None => return,
    };
    let path = unsafe {
        let Ok(dialog) =
            CoCreateInstance::<_, IFileSaveDialog>(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
        else {
            return;
        };
        let _ = dialog.SetTitle(w!("インライン展開保存 — Markdownのコピー"));
        let _ = dialog.SetFileTypes(&[COMDLG_FILTERSPEC {
            pszName: w!("Markdown (*.md)"),
            pszSpec: w!("*.md"),
        }]);
        let _ = dialog.SetDefaultExtension(w!("md"));
        let stem = current
            .as_deref()
            .and_then(Path::file_stem)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ノート".into());
        let _ = dialog.SetFileName(&HSTRING::from(format!("{stem}-expanded.md")));
        if dialog.Show(Some(hwnd)).is_err() {
            return;
        }
        let Some(path) = dialog.cast().ok().and_then(|d| dialog_result(&d)) else {
            return;
        };
        path
    };
    if let Err(error) = write_expanded(&path, &expanded) {
        error_box(hwnd, &format!("展開結果を保存できませんでした: {error}"));
    }
}

fn write_expanded(
    path: &Path,
    expanded: &yy_preview::transclusion::Expansion,
) -> std::io::Result<()> {
    if !expanded.errors.is_empty() {
        return Err(std::io::Error::other("未解決の参照があります"));
    }
    let existing = path.canonicalize().ok();
    if expanded
        .sources
        .iter()
        .any(|source| source == path || existing.as_ref() == Some(source))
    {
        return Err(std::io::Error::other(
            "元のノート・埋め込み元とは別の保存先を指定してください",
        ));
    }
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
    file.write_all(expanded.markdown.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub(super) fn open_snippets(hwnd: HWND) {
    let Some(folder) = yy_config::config_dir().map(|p| p.join("snippets")) else {
        error_box(hwnd, "設定フォルダーを取得できません。");
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&folder) {
        error_box(hwnd, &format!("スニペットフォルダーを作れません: {e}"));
        return;
    }
    unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(
            Some(hwnd),
            w!("open"),
            &HSTRING::from(folder.as_os_str()),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expanded_save_preserves_sources_and_refuses_incomplete_exports() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.md");
        let part = dir.path().join("part.md");
        let target = dir.path().join("copy.md");
        let source = "# 本文\r\n\r\n![[part]]\r\n";
        std::fs::write(&main, source).unwrap();
        std::fs::write(&part, "共通の内容😀\n").unwrap();
        let expanded = yy_preview::transclusion::expand(source, Some(&main), None, true);
        assert!(expanded.errors.is_empty());
        assert!(write_expanded(&main, &expanded).is_err());
        assert!(write_expanded(&part, &expanded).is_err());
        std::fs::write(&target, "以前のコピー").unwrap();
        write_expanded(&target, &expanded).unwrap();
        let saved = std::fs::read_to_string(&target).unwrap();
        assert!(saved.contains("共通の内容😀") && !saved.contains("![["));
        assert_eq!(std::fs::read_to_string(&main).unwrap(), source);
        assert_eq!(std::fs::read_to_string(&part).unwrap(), "共通の内容😀\n");
        let broken = yy_preview::transclusion::expand("![[missing]]", Some(&main), None, true);
        assert!(write_expanded(&target, &broken).is_err());
        assert_eq!(std::fs::read_to_string(target).unwrap(), saved);
    }
}

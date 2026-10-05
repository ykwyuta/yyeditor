//! Markdown / HTML のプレビュー（エディタの右側）: 表示の切り替え、内容の更新、スクロールの同期、
//! 境界の移動。表示そのものは [`crate::preview`]。

use yy_preview::{Kind, PageOptions};

use super::*;
use crate::preview::Preview;

/// 「プレビュー」（表示メニュー）
pub(crate) const ID_PREVIEW: u16 = 209;
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
        // 相対パスの画像などは手元のフォルダから読む（リモートのファイルでは読まない）
        let folder = path
            .as_deref()
            .filter(|_| self.doc.remote().is_none())
            .and_then(|p| p.parent())
            .map(|p| p.to_owned());
        let opts = PageOptions {
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
                let body = yy_preview::markdown_to_html(&text, Some(&self.syntaxes));
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

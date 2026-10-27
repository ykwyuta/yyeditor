//! タブ単位の Markdown / アウトライン編集。文書と保存・Undo は通常編集と共用する。
use super::*;
use crate::preview::{DocBase, Preview};

pub(super) const ID_MARKDOWN: u16 = 212;
pub(super) const ID_OUTLINE: u16 = 213;

#[derive(Clone)]
pub(super) enum Action {
    Command(u16),
    Switch(usize),
    CloseTab(usize),
    CloseWindow,
    TabMenu(usize),
    Open(PathBuf, Option<Encoding>, bool),
    Batch(Vec<Action>),
}

#[derive(Default)]
pub(super) struct LivePane {
    pub pane: Option<Preview>,
    epoch: u64,
    seq: u64,
    pub version: Option<u64>,
    ready: bool,
    receiving: bool,
    pending: Vec<Action>,
    pub dispatching: bool,
}

impl App {
    pub(super) fn toggle_live_editor(&mut self, outline: bool) -> std::result::Result<(), String> {
        if !self.markdown_mode || self.outline_mode != outline {
            if self.hex.is_some() || self.code.is_some() || self.csv.is_some() {
                return Err("通常のテキスト表示に戻してからライブ編集を選んでください。".into());
            }
            self.markdown_source()?;
            if self.markdown.pane.is_none() {
                self.markdown.pane =
                    Some(Preview::new(self.frame, self.frame).map_err(|e| e.message())?);
            }
        }
        self.markdown_mode = !self.markdown_mode || self.outline_mode != outline;
        self.outline_mode = self.markdown_mode && outline;
        self.markdown.version = None;
        self.refresh_markdown();
        self.layout_children();
        Ok(())
    }

    fn markdown_source(&self) -> std::result::Result<String, String> {
        let snap = self.doc.snapshot();
        if snap.len() > yy_preview::live::MAX_BYTES as u64 {
            return Err(
                "ライブ編集は 1 MiB までの文書に対応しています。通常編集を利用してください。"
                    .into(),
            );
        }
        if self.doc.is_loading() {
            return Err("文書の読み込みが終わるまでお待ちください。".into());
        }
        String::from_utf8(snap.read(0..snap.len()))
            .map_err(|_| "テキストとして読み込んだ文書で利用してください。".into())
    }

    pub(super) fn refresh_markdown(&mut self) {
        if self.markdown.receiving {
            return;
        }
        unsafe {
            CheckMenuItem(
                self.menu_view,
                ID_MARKDOWN as u32,
                (MF_BYCOMMAND
                    | if self.markdown_mode && !self.outline_mode {
                        MF_CHECKED
                    } else {
                        MF_UNCHECKED
                    })
                .0,
            );
        }
        unsafe {
            CheckMenuItem(
                self.menu_view,
                ID_OUTLINE as u32,
                (MF_BYCOMMAND
                    | if self.markdown_mode && self.outline_mode {
                        MF_CHECKED
                    } else {
                        MF_UNCHECKED
                    })
                .0,
            );
        }
        if let Some(pane) = &mut self.markdown.pane {
            pane.set_visible(self.markdown_mode);
        }
        if !self.markdown_mode {
            self.markdown.ready = false;
            return;
        }
        if self.markdown.version == Some(self.doc.version()) {
            return;
        }
        let source = match self.markdown_source() {
            Ok(source) => source,
            Err(e) => {
                self.markdown_mode = false;
                self.status_msg = e;
                self.refresh_markdown();
                self.layout_children();
                self.update_status();
                return;
            }
        };
        let location = self.doc.location();
        let folder = match self
            .doc
            .remote()
            .and_then(|r| yy_remote::RemoteUri::parse(&r.uri))
        {
            Some(u) => crate::remote::connected(&u.target()).map(|session| DocBase::Remote {
                session,
                dir: yy_remote::RemoteUri {
                    path: yy_proto::parent_path(&u.path),
                    ..u
                },
            }),
            None => location
                .as_deref()
                .and_then(|p| p.parent())
                .map(|p| DocBase::Local(p.to_owned())),
        };
        self.markdown.epoch += 1;
        self.markdown.seq = 0;
        self.markdown.ready = false;
        self.markdown.pending.clear();
        self.markdown.version = Some(self.doc.version());
        let caret = (self.doc.selections().primary().head as usize).min(source.len());
        let caret = source[..caret].encode_utf16().count();
        if let Some(pane) = &self.markdown.pane {
            if self.outline_mode {
                pane.show_outline(&source, self.markdown.epoch, self.doc.is_busy());
            } else {
                pane.show_editor(
                    &source,
                    self.markdown.epoch,
                    caret,
                    self.doc.is_busy(),
                    folder,
                );
            }
        }
    }

    pub(super) fn layout_markdown(&self, left: i32, top: i32, width: i32, height: i32) {
        if let Some(pane) = &self.markdown.pane
            && self.markdown_mode
        {
            pane.set_bounds(left, top, width, height);
        }
        unsafe {
            let _ = ShowWindow(
                self.view,
                if self.markdown_mode {
                    SW_HIDE
                } else {
                    SW_SHOWNA
                },
            );
        }
    }

    fn accept_markdown(&mut self, message: &str) -> Option<Action> {
        let mut fields = message.splitn(6, '\n');
        let epoch: u64 = fields.next()?.parse().ok()?;
        let seq: u64 = fields.next()?.parse().ok()?;
        let action = fields.next()?;
        let anchor: usize = fields.next()?.parse().ok()?;
        let head: usize = fields.next()?.parse().ok()?;
        let source = fields.next()?;
        if !self.markdown_mode
            || epoch != self.markdown.epoch
            || seq <= self.markdown.seq
            || source.len() > yy_preview::live::MAX_BYTES
            || self.markdown.version != Some(self.doc.version())
        {
            return None;
        }
        self.markdown.seq = seq;
        let before = self.markdown_source().ok()?;
        let kind = if matches!(action, "paragraph" | "newline") {
            EditKind::Newline
        } else if action == "edit" {
            EditKind::Typing
        } else {
            EditKind::Other
        };
        match apply_source(&mut self.doc, &before, source, anchor, head, kind) {
            Err(()) => {
                self.markdown.version = None;
                self.refresh_markdown();
                return None;
            }
            Ok(true) => {
                self.markdown.version = Some(self.doc.version());
                self.markdown.receiving = true;
                self.after_edit();
                self.markdown.receiving = false;
            }
            Ok(false) => self.update_status(),
        }
        if action == "ready" {
            self.markdown.ready = true;
        }
        let caret = yy_preview::live::byte_offset(source, head);
        if let Some(operation) = action.strip_prefix("table:") {
            self.edit_markdown_table(source, caret, operation);
            return None;
        }
        if let Some(pane) = &self.markdown.pane
            && !self.outline_mode
        {
            pane.editor_post(&yy_preview::live::rendered_at(
                epoch,
                seq,
                source,
                Some(caret),
            ));
        }
        if let Some(token) = action.strip_prefix("flush:") {
            if token.parse::<u64>().ok() == Some(epoch) {
                return Some(Action::Batch(std::mem::take(&mut self.markdown.pending)));
            }
            return None;
        }
        let command = match action {
            "save" => ID_SAVE,
            "saveas" => ID_SAVE_AS,
            "undo" => ID_UNDO,
            "redo" => ID_REDO,
            "open" => ID_OPEN,
            "new" => ID_NEW,
            "close" => ID_CLOSE,
            "next" => ID_TAB_NEXT,
            "prev" => ID_TAB_PREV,
            "source" => {
                if self.outline_mode {
                    ID_OUTLINE
                } else {
                    ID_MARKDOWN
                }
            }
            "preview" => ID_PREVIEW,
            "find" => ID_FIND,
            "grep" => ID_GREP,
            "replace" => ID_REPLACE,
            "goto" => ID_GOTO,
            "findnext" => ID_FIND_NEXT,
            "findprev" => ID_FIND_PREV,
            "help" => ID_HELP,
            _ => return None,
        };
        Some(Action::Command(command))
    }

    fn edit_markdown_table(&mut self, source: &str, caret: usize, operation: &str) {
        use yy_preview::live::table::{Operation, edit};
        let result = Operation::parse(operation)
            .ok_or("この表操作には対応していません。")
            .and_then(|op| edit(source, caret, op))
            .and_then(|edited| {
                if self.doc.is_busy() {
                    return Err("読み取り専用または処理中の文書は編集できません。");
                }
                if edited.source.len() > yy_preview::live::MAX_BYTES {
                    return Err(
                        "ライブ編集の上限 1 MiB を超えるため、通常編集で操作してください。",
                    );
                }
                Ok(edited)
            });
        match result {
            Ok(edited) => {
                let caret16 = edited.source[..edited.caret].encode_utf16().count();
                if apply_source(
                    &mut self.doc,
                    source,
                    &edited.source,
                    caret16,
                    caret16,
                    EditKind::Other,
                )
                .is_err()
                {
                    return;
                }
                let previous = self.markdown.epoch;
                self.markdown.epoch += 1;
                self.markdown.seq = 0;
                self.markdown.ready = false;
                self.markdown.version = Some(self.doc.version());
                self.markdown.receiving = true;
                self.after_edit();
                self.markdown.receiving = false;
                if let Some(pane) = &self.markdown.pane {
                    pane.editor_post(&yy_preview::live::replaced(
                        previous,
                        self.markdown.epoch,
                        &edited.source,
                        edited.caret,
                    ));
                    if !self.markdown.pending.is_empty() {
                        pane.editor_post(&format!("{{\"flush\":{}}}", self.markdown.epoch));
                    }
                }
            }
            Err(message) => {
                if let Some(pane) = &self.markdown.pane {
                    pane.editor_post(&yy_preview::live::rendered_at(
                        self.markdown.epoch,
                        self.markdown.seq,
                        source,
                        Some(caret),
                    ));
                    pane.editor_post(&format!(
                        "{{\"epoch\":{},\"tableError\":{}}}",
                        self.markdown.epoch,
                        yy_preview::live::source_json(message)
                    ));
                }
            }
        }
    }
}

fn apply_source(
    doc: &mut Document,
    before: &str,
    source: &str,
    anchor: usize,
    head: usize,
    kind: EditKind,
) -> std::result::Result<bool, ()> {
    let selection = SelectionSet::single(Selection::new(
        yy_preview::live::byte_offset(source, anchor) as u64,
        yy_preview::live::byte_offset(source, head) as u64,
    ));
    if let Some((range, text)) = yy_preview::live::difference(before, source) {
        if doc.is_busy() {
            return Err(());
        }
        Ok(doc.apply_changes(
            vec![Change::replace_bytes(
                range.start as u64..range.end as u64,
                text.as_bytes().to_vec(),
            )],
            kind,
            |_| selection,
        ))
    } else {
        if *doc.selections() != selection {
            doc.set_selections(selection);
        }
        Ok(false)
    }
}

/// ネイティブの保存・タブ切り替え・終了の直前に IME 確定と入力の反映を待つ。
pub(super) fn defer(action: Action) -> bool {
    with_app(|a| {
        if !a.markdown_mode || !a.markdown.ready || a.markdown.dispatching {
            return false;
        }
        let first = a.markdown.pending.is_empty();
        a.markdown.pending.push(action);
        if first {
            if let Some(pane) = &a.markdown.pane {
                pane.editor_post(&format!("{{\"flush\":{}}}", a.markdown.epoch));
            }
        }
        true
    })
    .unwrap_or(false)
}

pub(super) fn on_messages(hwnd: HWND) {
    let messages = with_app(|a| a.markdown.pane.as_ref().map(|p| p.take_edit_messages()))
        .flatten()
        .unwrap_or_default();
    for message in messages {
        let action = with_app(|a| a.accept_markdown(&message)).flatten();
        if let Some(action) = action {
            execute(hwnd, action);
        }
        with_app(|a| a.refresh_markdown());
    }
}

fn execute(hwnd: HWND, action: Action) {
    match action {
        Action::Command(id) => on_command_now(hwnd, id),
        Action::Switch(index) => {
            with_app(|a| a.switch_tab(index));
            focus_editor();
        }
        Action::CloseTab(index) => {
            close_tab_at(hwnd, index);
        }
        Action::CloseWindow => close_window(hwnd),
        Action::TabMenu(index) => tab_menu(hwnd, index),
        Action::Open(path, encoding, readonly) => open_path_now(hwnd, path, encoding, readonly),
        Action::Batch(actions) => {
            for action in actions {
                execute(hwnd, action);
            }
        }
    }
}

/// SetFocus は WM_SETFOCUS を同期送信するため、App の借用を解いてから呼ぶ。
pub(super) fn focus_editor() {
    let action = with_app(|a| {
        if a.markdown_mode {
            a.markdown.pane.as_ref().and_then(|p| p.focus_action())
        } else {
            let view = a.view;
            Some(Box::new(move || unsafe {
                let _ = SetFocus(Some(view));
            }) as Box<dyn FnOnce()>)
        }
    })
    .flatten();
    if let Some(action) = action {
        action();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outline_changes_undo_redo_and_save_keep_ids_and_references() {
        let source = include_str!("../../../../demo/outline/example.yyoutline");
        let mut doc = Document::from_text(source);
        let changed = source.replace("アイデア", "検討事項😀");
        assert!(apply_source(&mut doc, source, &changed, 0, 0, EditKind::Other).unwrap());
        assert!(doc.undo());
        assert_eq!(
            doc.snapshot().read(0..doc.snapshot().len()),
            source.as_bytes()
        );
        assert!(doc.redo());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("日本語.yyoutline");
        doc.save_as(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), changed.as_bytes());
        let reopened = Document::open_shared_read_only(&path, &OpenOptions::default()).unwrap();
        assert_eq!(
            reopened.snapshot().read(0..reopened.snapshot().len()),
            changed.as_bytes()
        );
    }

    #[test]
    fn table_actions_have_separate_undo_steps() {
        use yy_preview::live::table::{Operation, edit};
        let original = "| A | B |\n| --- | --- |\n| 日本語 | 😀 |\n";
        let mut doc = Document::from_text(original);
        let column = edit(
            original,
            original.find("日本語").unwrap(),
            Operation::ColumnAfter,
        )
        .unwrap();
        let caret = column.source[..column.caret].encode_utf16().count();
        assert!(
            apply_source(
                &mut doc,
                original,
                &column.source,
                caret,
                caret,
                EditKind::Other
            )
            .unwrap()
        );
        let row = edit(&column.source, column.caret, Operation::RowAfter).unwrap();
        let caret = row.source[..row.caret].encode_utf16().count();
        assert!(
            apply_source(
                &mut doc,
                &column.source,
                &row.source,
                caret,
                caret,
                EditKind::Other
            )
            .unwrap()
        );
        assert!(doc.undo());
        assert_eq!(
            doc.snapshot().read(0..doc.snapshot().len()),
            column.source.as_bytes()
        );
        assert!(doc.undo());
        assert_eq!(
            doc.snapshot().read(0..doc.snapshot().len()),
            original.as_bytes()
        );
        assert!(doc.redo());
        assert!(doc.redo());
        assert_eq!(
            doc.snapshot().read(0..doc.snapshot().len()),
            row.source.as_bytes()
        );
    }
    #[test]
    fn live_edits_share_document_history_and_save_source_exactly() {
        let original = "# 見出し\r\n\r\n未知の記法 <span>保持</span> 😀\r\n";
        let updated = "# 見出し\r\n\r\n未知の記法 <span>保持</span> **追記😃**\r\n";
        let mut doc = Document::from_text(original);
        assert!(!apply_source(&mut doc, original, original, 0, 0, EditKind::Other).unwrap());
        assert!(!doc.is_modified());
        assert!(
            apply_source(
                &mut doc,
                original,
                updated,
                updated.encode_utf16().count(),
                updated.encode_utf16().count(),
                EditKind::Typing
            )
            .unwrap()
        );
        assert_eq!(doc.selections().primary().head, updated.len() as u64);
        assert!(doc.undo());
        assert_eq!(
            doc.snapshot().read(0..doc.snapshot().len()),
            original.as_bytes()
        );
        assert!(doc.redo());
        assert_eq!(
            doc.snapshot().read(0..doc.snapshot().len()),
            updated.as_bytes()
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.md");
        doc.save_as(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), updated.as_bytes());
        let mut readonly = Document::open_shared_read_only(&path, &OpenOptions::default()).unwrap();
        assert!(apply_source(&mut readonly, updated, "changed", 0, 0, EditKind::Typing).is_err());
        assert_eq!(
            readonly.snapshot().read(0..readonly.snapshot().len()),
            updated.as_bytes()
        );
    }
}

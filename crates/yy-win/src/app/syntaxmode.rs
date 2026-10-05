//! シンタックスハイライト（10 章）: ファイル種類の判定、表示範囲の色付け、括弧の対応、コメント化。

use yy_core::syntax::{SyntaxView, toggle_comment};

use super::*;
use crate::highlight::{row_tokens, token_palette};
use crate::render::RowTokens;

/// 「ハイライト」メニューの項目（`ID_SYNTAX_BASE + 定義の一覧の番号`）
pub(crate) const ID_SYNTAX_NONE: u16 = 800;
pub(crate) const ID_SYNTAX_BASE: u16 = 801;
pub(crate) const ID_TOGGLE_COMMENT: u16 = 414;
pub(crate) const ID_GOTO_BRACKET: u16 = 514;

/// 括弧の対応の結果（（表示の版, キャレット位置）, 括弧の組）。
pub(crate) type BracketCache = ((u64, u64), Option<(u64, u64)>);

/// 文書のハイライトの状態。
pub(crate) struct SyntaxState {
    pub view: SyntaxView,
    /// トークンの種類ごとの色の番号
    pub palette: Vec<Option<u16>>,
    /// 利用者が選んだ（ファイル種類の判定より優先する）
    pub manual: bool,
}

impl App {
    /// 文書のファイル種類からハイライトの定義を選ぶ（10 章 3）。
    pub(crate) fn syntax_for_path(&mut self) {
        if self.syntax_off || self.syntax.as_ref().is_some_and(|s| s.manual) {
            return;
        }
        let id = self.detect_syntax();
        let current = self.syntax.as_ref().map(|s| s.view.syntax().id.clone());
        if id.is_some() && id == current {
            return;
        }
        self.set_syntax(id.as_deref(), false);
    }

    fn detect_syntax(&self) -> Option<String> {
        // リモートのファイルも名前（`ssh://…` の最後）で判定する
        let location = self.doc.location();
        let path = location.as_deref();
        // 設定のファイル種類（拡張子）の指定を優先する
        if let Some(ext) = path.and_then(|p| p.extension()) {
            let ext = ext.to_string_lossy().into_owned();
            if let Some((_, ft)) = self.config.filetype_for_extension(&ext)
                && let Some(s) = &ft.syntax
            {
                return (!s.is_empty() && s != "none").then(|| s.clone());
            }
        }
        let snap = self.doc.snapshot();
        let head = snap.read(0..snap.len().min(4096));
        self.syntaxes.detect(path, &head)
    }

    /// ハイライトの定義を切り替える（`None` ならハイライトしない）。
    pub(crate) fn set_syntax(&mut self, id: Option<&str>, manual: bool) {
        self.syntax = None;
        if let Some(id) = id {
            match self.syntaxes.get(id) {
                Ok(s) => {
                    let palette = token_palette(&s, &self.config.colors);
                    self.syntax = Some(SyntaxState {
                        view: SyntaxView::new(s),
                        palette,
                        manual,
                    });
                }
                Err(e) => self.status_msg = e,
            }
        }
        if manual && self.syntax.is_none() {
            // 「なし」を選んだことを覚えておく
            self.syntax_off = true;
        } else if manual {
            self.syntax_off = false;
        }
        self.sync_syntax();
        self.bracket_cache = None;
        self.renderer.clear_cache();
        self.update_syntax_menu();
        self.update_status();
        self.invalidate();
    }

    /// 内容の変更に合わせて行の開始状態の記録を更新する。
    pub(crate) fn sync_syntax(&mut self) {
        let n = self.notifier();
        let snap = self.doc.snapshot().clone();
        if let Some(s) = &mut self.syntax {
            s.view.sync(&snap, &self.pool, n);
        }
    }

    /// バックグラウンドの読み込みの進み具合を反映する。
    pub(crate) fn poll_syntax(&mut self) {
        if let Some(s) = &mut self.syntax
            && s.view.poll()
        {
            self.invalidate();
        }
    }

    /// 表示の版（文書の版とハイライトの記録の版）。
    pub(crate) fn paint_version(&self) -> u64 {
        let g = self.syntax.as_ref().map_or(0, |s| s.view.generation());
        self.doc.version() ^ g.rotate_left(40)
    }

    /// 表示行のトークンの色（区切り文字モードでは列ごとの表示なので色を付けない）。
    pub(crate) fn visible_tokens(&mut self, rows: &[Row]) -> Vec<RowTokens> {
        if self.csv.is_some() || rows.is_empty() {
            return Vec::new();
        }
        let Some(s) = &mut self.syntax else {
            return Vec::new();
        };
        let snap = self.doc.snapshot();
        let first = rows[0].start;
        let first = if rows[0].line_start {
            first
        } else {
            snap.find_prev(first.saturating_sub(1 << 20)..first, b'\n')
                .map_or(0, |n| n + 1)
        };
        let until = rows.last().map_or(first, |r| r.next).max(first + 1);
        let (lines, _) = s.view.lines(snap, first, until);
        row_tokens(rows, &lines, &s.palette)
    }

    /// キャレットの位置の括弧と、対応する括弧（表示用。同じ位置ならキャッシュを使う）。
    pub(crate) fn caret_brackets(&mut self) -> Vec<std::ops::Range<u64>> {
        if self.csv.is_some() || self.rect.is_some() || self.doc.selections().len() != 1 {
            return Vec::new();
        }
        let Some(s) = &self.syntax else {
            return Vec::new();
        };
        let head = self.doc.selections().primary().head;
        let key = (self.paint_version(), head);
        let pair = match &self.bracket_cache {
            Some((k, p)) if *k == key => *p,
            _ => {
                let p = s.view.matching_bracket(self.doc.snapshot(), head);
                self.bracket_cache = Some((key, p));
                p
            }
        };
        pair.map_or_else(Vec::new, |(a, b)| vec![a..a + 1, b..b + 1])
    }

    /// 対応する括弧へ移動する（Ctrl+]）。
    pub(crate) fn goto_bracket(&mut self) {
        let Some(s) = &self.syntax else {
            self.status_msg = "ハイライトの定義がないため、括弧の対応が分かりません".into();
            self.update_status();
            return;
        };
        let head = self.doc.selections().primary().head;
        match s.view.matching_bracket(self.doc.snapshot(), head) {
            Some((a, b)) => {
                // 開き括弧の上（直前）なら閉じ括弧の後ろへ、閉じ括弧なら開き括弧の前へ
                let target = if head == a || head == a + 1 { b + 1 } else { a };
                self.rect = None;
                self.doc
                    .set_selections(SelectionSet::single(Selection::caret(target)));
                self.scroll_to_offset(target);
                self.after_move();
            }
            None => {
                self.status_msg = "対応する括弧が見つかりません".into();
                self.update_status();
            }
        }
    }

    /// 選択した行のコメントを付け外しする（Ctrl+/）。
    pub(crate) fn toggle_comment(&mut self) {
        let Some(s) = &self.syntax else {
            self.status_msg = "ハイライトの定義がないため、コメントの書き方が分かりません".into();
            self.update_status();
            return;
        };
        let syntax = s.view.syntax().clone();
        let block = syntax
            .block_comment
            .as_ref()
            .map(|(a, b)| (a.as_str(), b.as_str()));
        if syntax.line_comment.is_none() && block.is_none() {
            self.status_msg = format!("{} にはコメントの指定がありません", syntax.name);
            self.update_status();
            return;
        }
        let snap = self.doc.snapshot().clone();
        // 選択範囲の行（選択が行頭で終わる場合、その行は含めない）
        let mut ranges: Vec<std::ops::Range<u64>> = Vec::new();
        for sel in self.doc.selections().iter() {
            let (a, b) = (sel.start(), sel.end());
            let b = if b > a && yy_layout::is_line_start(&snap, b) {
                b - 1
            } else {
                b
            };
            let mut start = snap
                .find_prev(a.saturating_sub(1 << 20)..a, b'\n')
                .map_or(0, |n| n + 1);
            loop {
                let nl = snap.find_next(start..snap.len(), b'\n');
                let mut end = nl.unwrap_or(snap.len());
                if end > start && snap.byte_at(end - 1) == Some(b'\r') {
                    end -= 1;
                }
                if ranges.last().is_none_or(|r| r.start < start) {
                    ranges.push(start..end);
                }
                match nl {
                    Some(n) if n < b => start = n + 1,
                    _ => break,
                }
                if ranges.len() > RECT_EDIT_LIMIT {
                    break;
                }
            }
        }
        ranges.sort_by_key(|r| r.start);
        ranges.dedup_by_key(|r| r.start);
        let lines: Vec<Vec<u8>> = ranges.iter().map(|r| snap.read(r.clone())).collect();
        let Some(new) = toggle_comment(&lines, syntax.line_comment.as_deref(), block) else {
            return;
        };
        let replacements: std::collections::HashMap<Vec<u8>, Vec<u8>> =
            lines.iter().cloned().zip(new.iter().cloned()).collect();
        self.rect = None;
        let changed = self.doc.replace_ranges(&ranges, |b| {
            replacements.get(b).cloned().unwrap_or_else(|| b.to_vec())
        });
        if changed {
            self.after_edit();
        }
    }

    /// 「ハイライト」メニューのチェックを今の定義に合わせる。
    pub(crate) fn update_syntax_menu(&self) {
        let current = self.syntax.as_ref().map(|s| s.view.syntax().id.clone());
        unsafe {
            let check = |id: u16, on: bool| {
                CheckMenuItem(
                    self.menu_syntax,
                    id as u32,
                    (MF_BYCOMMAND | if on { MF_CHECKED } else { MF_UNCHECKED }).0,
                );
            };
            check(ID_SYNTAX_NONE, current.is_none());
            for (i, (id, _)) in self.syntaxes.list().iter().enumerate() {
                check(
                    ID_SYNTAX_BASE + i as u16,
                    current.as_deref() == Some(id.as_str()),
                );
            }
        }
    }

    /// 「ハイライト」メニューの項目を選んだ。
    pub(crate) fn choose_syntax(&mut self, cmd: u16) {
        if cmd == ID_SYNTAX_NONE {
            self.set_syntax(None, true);
            return;
        }
        let list = self.syntaxes.list();
        if let Some((id, _)) = list.get((cmd - ID_SYNTAX_BASE) as usize) {
            let id = id.clone();
            self.set_syntax(Some(&id), true);
        }
    }
}

/// 「表示」メニューに「ハイライト」の子メニューを加える。
pub(crate) fn append_syntax_menu(view: HMENU, list: &[(String, String)]) -> Result<HMENU> {
    unsafe {
        let sub = CreatePopupMenu()?;
        AppendMenuW(sub, MF_STRING, ID_SYNTAX_NONE as usize, w!("なし"))?;
        AppendMenuW(sub, MF_SEPARATOR, 0, None)?;
        for (i, (_, name)) in list.iter().enumerate() {
            AppendMenuW(
                sub,
                MF_STRING,
                (ID_SYNTAX_BASE + i as u16) as usize,
                &HSTRING::from(name.as_str()),
            )?;
        }
        AppendMenuW(view, MF_SEPARATOR, 0, None)?;
        AppendMenuW(view, MF_POPUP, sub.0 as usize, w!("ハイライト(&H)"))?;
        Ok(sub)
    }
}

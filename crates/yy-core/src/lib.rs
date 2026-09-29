//! 文書モデルと編集。
//!
//! 文書の内容は永続ピースツリーのスナップショットで持ち、編集・Undo・保存は
//! すべてスナップショットの差し替えとして行う（01 章 4.3、06 章）。

pub mod edit;
mod history;
mod indexer;
pub mod motion;
mod selection;

use std::collections::HashMap;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use history::{EditKind, Version};
pub use indexer::{IndexBatch, Indexer};
pub use selection::{Selection, SelectionSet};
use yy_buffer::{Snapshot, SourceRef};
use yy_io::{Bom, FileGuard, MmapSource};
use yy_jobs::{JobPool, Notifier};

use edit::Change;
use history::{History, Record};

/// 改行コード。入力・貼り付けではこの改行コードを使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    Lf,
    CrLf,
}

impl Eol {
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            Eol::Lf => b"\n",
            Eol::CrLf => b"\r\n",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::CrLf => "CRLF",
        }
    }

    /// 新規文書の既定値（Windows では CRLF）。
    pub fn platform_default() -> Eol {
        if cfg!(windows) { Eol::CrLf } else { Eol::Lf }
    }

    /// 文書の最初の改行から判定する。改行がなければ `None`。
    pub fn detect(snap: &Snapshot) -> Option<Eol> {
        let n = snap.find_next(0..snap.len().min(1 << 20), b'\n')?;
        Some(if n > 0 && snap.byte_at(n - 1) == Some(b'\r') {
            Eol::CrLf
        } else {
            Eol::Lf
        })
    }
}

/// 連続入力を 1 つのソースにまとめるための状態（ピースの細分化を防ぐ）。
struct TypingRun {
    start: u64,
    bytes: Vec<u8>,
    version: Version,
}

/// 連続入力を 1 つのソースにまとめる上限
const TYPING_RUN_MAX: usize = 64 << 10;

/// 各選択に対する編集内容。
enum Ins {
    /// 削除のみ
    Nothing,
    /// 全カーソル共通のテキスト
    Shared,
    /// このカーソル固有のテキスト
    Own(Vec<u8>),
}

pub struct Document {
    path: Option<PathBuf>,
    file_len: u64,
    bom: Bom,
    eol: Eol,
    snapshot: Snapshot,
    sels: SelectionSet,
    history: History,
    version: Version,
    next_version: Version,
    saved_version: Version,
    /// 行数の情報が増えるたびに増える（表示の行番号の更新判定用）
    index_version: u64,
    indexer: Option<Indexer>,
    /// 現在の内容が参照しているファイル（保存時の退避に使う）
    source: Option<Arc<MmapSource>>,
    _guard: Option<FileGuard>,
    typing: Option<TypingRun>,
}

impl Default for Document {
    fn default() -> Self {
        Document::new_empty()
    }
}

/// 先頭のピース（最初の画面で必ず読む範囲）だけはその場で数えておく。
/// 未確定範囲の行数を推定するときの改行密度として使う。
fn index_first_piece(snap: Snapshot) -> Snapshot {
    let first = snap.unindexed_pieces(1);
    match first.first() {
        Some(p) => {
            let key = p.key();
            let n = yy_buffer::count_lf(p.bytes());
            snap.fill_line_counts(&|p| (p.key() == key).then_some(n))
        }
        None => snap,
    }
}

/// 改行を `eol` に揃える。
fn normalize_eol(text: &str, eol: Eol) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\r' => {
                out.extend_from_slice(eol.as_bytes());
                if b.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
            }
            b'\n' => out.extend_from_slice(eol.as_bytes()),
            c => out.push(c),
        }
        i += 1;
    }
    out
}

impl Document {
    pub fn new_empty() -> Document {
        Document {
            path: None,
            file_len: 0,
            bom: Bom::None,
            eol: Eol::platform_default(),
            snapshot: Snapshot::empty(),
            sels: SelectionSet::default(),
            history: History::default(),
            version: 0,
            next_version: 0,
            saved_version: 0,
            index_version: 0,
            indexer: None,
            source: None,
            _guard: None,
            typing: None,
        }
    }

    /// 内容を指定して作る（テスト用）。
    pub fn from_text(text: &str) -> Document {
        let mut d = Document::new_empty();
        d.snapshot = Snapshot::from_bytes(text.as_bytes().to_vec());
        d.eol = Eol::detect(&d.snapshot).unwrap_or(d.eol);
        d
    }

    /// ファイルを開く。行数のカウントは [`Document::start_indexing`] で別途開始する。
    pub fn open(path: &Path) -> io::Result<Document> {
        let o = yy_io::open_file(path)?;
        let snapshot = index_first_piece(o.snapshot);
        let eol = Eol::detect(&snapshot).unwrap_or_else(Eol::platform_default);
        Ok(Document {
            path: Some(o.path),
            file_len: o.file_len,
            bom: o.bom,
            eol,
            snapshot,
            sels: SelectionSet::default(),
            history: History::default(),
            version: 0,
            next_version: 0,
            saved_version: 0,
            index_version: 0,
            indexer: None,
            source: o.source,
            _guard: o.guard,
            typing: None,
        })
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// タイトルバー等に表示する名前。
    pub fn display_name(&self) -> String {
        self.path
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "無題".to_owned())
    }

    pub fn file_len(&self) -> u64 {
        self.file_len
    }

    pub fn bom(&self) -> Bom {
        self.bom
    }

    pub fn eol(&self) -> Eol {
        self.eol
    }

    /// 内容の版。編集・Undo・Redo で変わる（表示キャッシュの無効化に使う）。
    pub fn version(&self) -> Version {
        self.version
    }

    pub fn index_version(&self) -> u64 {
        self.index_version
    }

    /// 最後に保存（または開いた）時点から変更されているか。
    pub fn is_modified(&self) -> bool {
        self.version != self.saved_version
    }

    pub fn selections(&self) -> &SelectionSet {
        &self.sels
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    // ---- 行数のカウント --------------------------------------------------

    /// 改行数のバックグラウンドカウントを開始する。
    pub fn start_indexing(&mut self, pool: &JobPool, notify: Notifier) {
        if self.snapshot.is_fully_indexed() {
            return;
        }
        self.indexer = Some(Indexer::start(pool, &self.snapshot, notify));
    }

    /// 未確定の範囲が残っていてカウント中でなければ、カウントを開始する。
    ///
    /// 編集で未確定のピースが分割されたり、Undo で古い内容に戻ったりした場合に使う。
    pub fn maintain_indexing(&mut self, pool: &JobPool, notify: Notifier) {
        if self.indexer.is_none() {
            self.start_indexing(pool, notify);
        }
    }

    /// 届いたカウント結果を文書（と Undo 履歴）に反映する。反映したら `true`。
    pub fn poll_indexing(&mut self) -> bool {
        let Some(indexer) = &self.indexer else {
            return false;
        };
        let batches = indexer.drain();
        let finished = indexer.is_finished();
        if finished {
            self.indexer = None;
        }
        if batches.is_empty() {
            return finished;
        }
        let map: HashMap<_, _> = batches.into_iter().flat_map(|b| b.counts).collect();
        let f = |p: &yy_buffer::Piece| map.get(&p.key()).copied();
        self.snapshot = self.snapshot.fill_line_counts(&f);
        self.history.fill_line_counts(&f);
        self.index_version += 1;
        true
    }

    /// 行数カウントの進捗率。カウント中でなければ `None`。
    pub fn indexing_progress(&self) -> Option<f64> {
        self.indexer.as_ref().map(|i| i.progress())
    }

    /// 行数を数え終わるまで待つ（テスト・ベンチマーク用）。
    pub fn wait_indexing(&mut self) {
        while self.indexer.is_some() {
            if !self.poll_indexing() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }

    // ---- 選択・移動 ------------------------------------------------------

    /// 選択を置き換える。連続入力の区切りになる。
    pub fn set_selections(&mut self, mut sels: SelectionSet) {
        sels.clamp(self.snapshot.len());
        self.sels = sels;
        self.typing = None;
        self.history.seal();
    }

    /// 各カーソルを `f` の返す位置へ動かす。`extend` なら選択を広げる。
    pub fn move_carets(&mut self, extend: bool, f: impl Fn(&Snapshot, &Selection) -> u64) {
        let snap = self.snapshot.clone();
        let sels = self.sels.map(|s| {
            let head = f(&snap, s);
            if extend {
                Selection::new(s.anchor, head)
            } else {
                Selection::caret(head)
            }
        });
        self.set_selections(sels);
    }

    pub fn select_all(&mut self) {
        let len = self.snapshot.len();
        self.set_selections(SelectionSet::single(Selection::new(0, len)));
    }

    /// 選択されている文字列（複数ある場合は改行で連結）。何も選択されていなければ `None`。
    /// `max_bytes` を超える場合は `Err(選択の総バイト数)`。
    pub fn selected_text(&self, max_bytes: u64) -> Result<Option<String>, u64> {
        let total: u64 = self.sels.iter().map(|s| s.end() - s.start()).sum();
        if total == 0 {
            return Ok(None);
        }
        if total > max_bytes {
            return Err(total);
        }
        let mut out = Vec::with_capacity(total as usize);
        for (i, s) in self.sels.iter().filter(|s| !s.is_empty()).enumerate() {
            if i > 0 {
                out.extend_from_slice(self.eol.as_bytes());
            }
            out.extend(self.snapshot.read(s.range()));
        }
        Ok(Some(String::from_utf8_lossy(&out).into_owned()))
    }

    // ---- 編集 ------------------------------------------------------------

    /// 新しい内容と選択を確定し、Undo 履歴に記録する。
    fn commit(&mut self, snapshot: Snapshot, sels: SelectionSet, kind: EditKind) {
        let before = std::mem::replace(&mut self.snapshot, snapshot);
        let before_version = self.version;
        self.next_version += 1;
        self.version = self.next_version;
        let sels_before = std::mem::replace(&mut self.sels, sels);
        self.history.record(Record {
            before,
            after: self.snapshot.clone(),
            before_version,
            after_version: self.version,
            sels_before,
            sels_after: self.sels.clone(),
            kind,
        });
    }

    /// すべての選択に対する編集を 1 つの Undo 単位として適用する。
    ///
    /// `f` は各選択について（置き換える範囲, 挿入内容）を返す。`None` なら変更しない。
    /// 変更したカーソルは挿入テキストの末尾に、変更しないカーソルは位置を補正して残す。
    fn edit_selections(
        &mut self,
        kind: EditKind,
        shared: &[u8],
        mut f: impl FnMut(&Snapshot, &Selection) -> Option<(Range<u64>, Ins)>,
    ) -> bool {
        let snap = self.snapshot.clone();
        let shared_src: SourceRef = Arc::new(shared.to_vec());
        let mut changes: Vec<Change> = Vec::new();
        let mut which: Vec<Option<usize>> = Vec::with_capacity(self.sels.len());
        for sel in self.sels.iter() {
            let Some((range, ins)) = f(&snap, sel) else {
                which.push(None);
                continue;
            };
            let change = match ins {
                Ins::Nothing => Change::delete(range),
                Ins::Shared => Change::replace(range, &shared_src, 0..shared.len() as u64),
                Ins::Own(b) => Change::replace_bytes(range, b),
            };
            if let Some(last) = changes.last_mut()
                && change.range.start < last.range.end
            {
                // 重なった変更は範囲をまとめる（挿入は先の変更のものを使う）
                last.range.start = last.range.start.min(change.range.start);
                last.range.end = last.range.end.max(change.range.end);
                which.push(Some(changes.len() - 1));
                continue;
            }
            if change.range.is_empty() && change.insert_len == 0 {
                which.push(None);
                continue;
            }
            changes.push(change);
            which.push(Some(changes.len() - 1));
        }
        if changes.is_empty() {
            return false;
        }
        let applied = edit::apply(&snap, changes.clone());
        let new_sels: Vec<Selection> = self
            .sels
            .iter()
            .zip(&which)
            .map(|(s, w)| match w {
                Some(i) => Selection::caret(applied.new_ends[*i]),
                None => s.map(|p| edit::map_offset(&changes, p)),
            })
            .collect();
        let sels = SelectionSet::from_vec(new_sels, self.sels.primary_index());
        self.commit(applied.snapshot, sels, kind);
        true
    }

    /// 文字列を入力する（選択範囲は置き換える）。`overwrite` なら上書きモード。
    pub fn insert_text(&mut self, text: &str, overwrite: bool) -> bool {
        if text.is_empty() {
            return false;
        }
        let bytes = normalize_eol(text, self.eol);
        let single_caret = self.sels.len() == 1 && self.sels.primary().is_empty();

        // 連続入力: 直前に入力したテキストと 1 つのソースにまとめる
        if single_caret && !overwrite {
            let caret = self.sels.primary().head;
            if let Some(run) = &self.typing
                && run.version == self.version
                && run.start + run.bytes.len() as u64 == caret
                && run.bytes.len() + bytes.len() <= TYPING_RUN_MAX
            {
                let start = run.start;
                let mut combined = run.bytes.clone();
                combined.extend_from_slice(&bytes);
                let applied = edit::apply(
                    &self.snapshot,
                    vec![Change::replace_bytes(start..caret, combined.clone())],
                );
                let end = applied.new_ends[0];
                self.commit(
                    applied.snapshot,
                    SelectionSet::single(Selection::caret(end)),
                    EditKind::Typing,
                );
                self.typing = Some(TypingRun {
                    start,
                    bytes: combined,
                    version: self.version,
                });
                return true;
            }
        }

        let chars = text.chars().count();
        let ok = self.edit_selections(EditKind::Typing, &bytes, |snap, sel| {
            let range = if overwrite && sel.is_empty() {
                // 上書き: 行末を越えない範囲で入力した文字数だけ置き換える
                let line_end = motion::line_end(snap, sel.head);
                let mut end = sel.head;
                for _ in 0..chars {
                    if end >= line_end {
                        break;
                    }
                    end = motion::next_grapheme(snap, end).min(line_end);
                }
                sel.head..end
            } else {
                sel.range()
            };
            Some((range, Ins::Shared))
        });
        self.typing = None;
        if ok && single_caret && !overwrite {
            let caret = self.sels.primary().head;
            self.typing = Some(TypingRun {
                start: caret - bytes.len() as u64,
                bytes,
                version: self.version,
            });
        }
        ok
    }

    /// 改行を入力する。`auto_indent` なら現在行のインデントを引き継ぐ。
    pub fn insert_newline(&mut self, auto_indent: bool) -> bool {
        let eol = self.eol;
        self.typing = None;
        self.edit_selections(EditKind::Newline, b"", |snap, sel| {
            let mut text = eol.as_bytes().to_vec();
            if auto_indent {
                text.extend(motion::indent_of_line(snap, sel.start()));
            }
            Some((sel.range(), Ins::Own(text)))
        })
    }

    /// 貼り付け。改行を文書の改行コードに揃える。カーソルが複数あり、行数がカーソル数と
    /// 一致する場合は 1 行ずつ振り分ける（09 章 3.2）。
    pub fn paste(&mut self, text: &str) -> bool {
        self.typing = None;
        let n = self.sels.len();
        if n > 1 {
            let trimmed = text
                .strip_suffix("\r\n")
                .or(text.strip_suffix('\n'))
                .unwrap_or(text);
            let lines: Vec<&str> = trimmed
                .split('\n')
                .map(|l| l.trim_end_matches('\r'))
                .collect();
            if lines.len() == n {
                let mut it = lines.into_iter();
                return self.edit_selections(EditKind::Paste, b"", |_, sel| {
                    let line = it.next().unwrap_or_default();
                    Some((sel.range(), Ins::Own(line.as_bytes().to_vec())))
                });
            }
        }
        let bytes = normalize_eol(text, self.eol);
        self.edit_selections(EditKind::Paste, &bytes, |_, sel| {
            Some((sel.range(), Ins::Shared))
        })
    }

    /// 選択範囲を削除する。選択がなければ何もしない。
    pub fn delete_selection(&mut self, kind: EditKind) -> bool {
        self.typing = None;
        self.edit_selections(kind, b"", |_, sel| {
            (!sel.is_empty()).then(|| (sel.range(), Ins::Nothing))
        })
    }

    fn delete_with(&mut self, kind: EditKind, f: impl Fn(&Snapshot, u64) -> Range<u64>) -> bool {
        self.typing = None;
        self.edit_selections(kind, b"", |snap, sel| {
            if sel.is_empty() {
                let r = f(snap, sel.head);
                (!r.is_empty()).then_some((r, Ins::Nothing))
            } else {
                Some((sel.range(), Ins::Nothing))
            }
        })
    }

    /// BackSpace
    pub fn delete_backward(&mut self) -> bool {
        self.delete_with(EditKind::Delete, |s, p| motion::prev_grapheme(s, p)..p)
    }

    /// Delete
    pub fn delete_forward(&mut self) -> bool {
        self.delete_with(EditKind::Delete, |s, p| p..motion::next_grapheme(s, p))
    }

    /// Ctrl+BackSpace
    pub fn delete_word_backward(&mut self) -> bool {
        self.delete_with(EditKind::Other, |s, p| motion::prev_word(s, p)..p)
    }

    /// Ctrl+Delete
    pub fn delete_word_forward(&mut self) -> bool {
        self.delete_with(EditKind::Other, |s, p| p..motion::next_word(s, p))
    }

    /// 任意の変更の列を 1 つの Undo 単位として適用する（矩形編集などで使う）。
    ///
    /// `changes` は開始位置の昇順で重ならないこと。`make_sels` は適用結果から新しい選択を作る。
    pub fn apply_changes(
        &mut self,
        changes: Vec<Change>,
        kind: EditKind,
        make_sels: impl FnOnce(&edit::Applied) -> SelectionSet,
    ) -> bool {
        self.typing = None;
        let changes: Vec<Change> = changes
            .into_iter()
            .filter(|c| !c.range.is_empty() || c.insert_len > 0)
            .collect();
        if changes.is_empty() {
            return false;
        }
        let applied = edit::apply(&self.snapshot, changes);
        let mut sels = make_sels(&applied);
        sels.clamp(applied.snapshot.len());
        self.commit(applied.snapshot, sels, kind);
        true
    }

    /// 主選択の文字列（空なら主カーソル位置の単語を選択して `None`）。
    fn occurrence_needle(&mut self) -> Option<Vec<u8>> {
        let p = *self.sels.primary();
        if p.is_empty() {
            let r = motion::word_range(&self.snapshot, p.head);
            if !r.is_empty() {
                self.set_selections(SelectionSet::single(Selection::new(r.start, r.end)));
            }
            return None;
        }
        let len = p.end() - p.start();
        (len <= 64 << 10).then(|| self.snapshot.read(p.range()))
    }

    /// 主選択と同じ文字列の次の出現箇所を選択に加える（Ctrl+D）。
    /// 何も選択していなければカーソル位置の単語を選択する。加えたら `true`。
    pub fn select_next_occurrence(&mut self) -> bool {
        let Some(needle) = self.occurrence_needle() else {
            return !self.sels.primary().is_empty();
        };
        let len = self.snapshot.len();
        let from = self.sels.iter().map(|s| s.end()).max().unwrap_or(0);
        let n = needle.len() as u64;
        let found = self
            .snapshot
            .find_bytes(from..len, &needle)
            .or_else(|| self.snapshot.find_bytes(0..from, &needle))
            .filter(|&pos| {
                !self
                    .sels
                    .iter()
                    .any(|s| s.start() == pos && s.end() == pos + n)
            });
        match found {
            Some(pos) => {
                let mut sels = self.sels.clone();
                sels.add(Selection::new(pos, pos + n));
                self.set_selections(sels);
                true
            }
            None => false,
        }
    }

    /// 主選択と同じ文字列のすべての出現箇所を選択する（Ctrl+Shift+L）。
    /// 選択した数と、`limit` で打ち切ったかを返す。
    pub fn select_all_occurrences(&mut self, limit: usize) -> (usize, bool) {
        let Some(needle) = self.occurrence_needle() else {
            let n = usize::from(!self.sels.primary().is_empty());
            return (n, false);
        };
        let len = self.snapshot.len();
        let n = needle.len() as u64;
        let primary_start = self.sels.primary().start();
        let mut found = Vec::new();
        let mut pos = 0;
        let mut truncated = false;
        while let Some(p) = self.snapshot.find_bytes(pos..len, &needle) {
            if found.len() >= limit {
                truncated = true;
                break;
            }
            found.push(Selection::new(p, p + n));
            pos = p + n;
        }
        if found.is_empty() {
            return (0, false);
        }
        let primary = found
            .iter()
            .position(|s| s.start() == primary_start)
            .unwrap_or(0);
        let count = found.len();
        self.set_selections(SelectionSet::from_vec(found, primary));
        (count, truncated)
    }

    /// 選択範囲に含まれる各行の行末にカーソルを置く（Alt+Shift+I）。
    pub fn carets_at_line_ends(&mut self, limit: usize) -> (usize, bool) {
        let snap = self.snapshot.clone();
        let len = snap.len();
        let mut carets = Vec::new();
        let mut truncated = false;
        'outer: for s in self.sels.iter() {
            let mut ls = motion::line_start(&snap, s.start());
            loop {
                if carets.len() >= limit {
                    truncated = true;
                    break 'outer;
                }
                carets.push(Selection::caret(motion::line_end(&snap, ls)));
                match snap.find_next(ls..len, b'\n') {
                    Some(nl) if !s.is_empty() && nl + 1 < s.end() => {
                        ls = nl + 1;
                    }
                    _ => break,
                }
            }
        }
        let count = carets.len();
        let primary = count - 1;
        self.set_selections(SelectionSet::from_vec(carets, primary));
        (count, truncated)
    }

    // ---- Undo / Redo -----------------------------------------------------

    pub fn undo(&mut self) -> bool {
        let Some(e) = self.history.undo() else {
            return false;
        };
        self.snapshot = e.before.clone();
        self.sels = e.sels_before.clone();
        self.version = e.before_version;
        self.typing = None;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(e) = self.history.redo() else {
            return false;
        };
        self.snapshot = e.after.clone();
        self.sels = e.sels_after.clone();
        self.version = e.after_version;
        self.typing = None;
        true
    }

    // ---- 保存 ------------------------------------------------------------

    /// 上書き保存。ファイル名がなければエラー（UI は「名前を付けて保存」を使う）。
    pub fn save(&mut self) -> io::Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ファイル名がありません"))?;
        self.save_as(&path)
    }

    /// 名前を付けて保存する。保存後は保存したファイルを参照し直す（06 章 3.4）。
    ///
    /// 行数は数え直しになるため、呼び出し側は [`Document::maintain_indexing`] を呼ぶこと。
    pub fn save_as(&mut self, path: &Path) -> io::Result<()> {
        yy_io::save_snapshot(&self.snapshot, path, self.bom, self.source.as_deref())?;
        // 保存したファイルをマップし直す（追記バッファや退避した元ファイルを解放できるように）
        if let Ok(o) = yy_io::open_file(path)
            && o.snapshot.len() == self.snapshot.len()
        {
            self.snapshot = index_first_piece(o.snapshot);
            self.source = o.source;
            self._guard = o.guard;
            self.file_len = o.file_len;
            self.indexer = None;
            self.index_version += 1;
        }
        self.path = Some(path.to_owned());
        self.saved_version = self.version;
        self.typing = None;
        self.history.seal();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn text(d: &Document) -> String {
        String::from_utf8(d.snapshot().read(0..d.snapshot().len())).unwrap()
    }

    fn carets(d: &Document) -> Vec<u64> {
        d.selections().iter().map(|s| s.head).collect()
    }

    #[test]
    fn indexes_file_in_background() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        let mut data = Vec::new();
        let mut i = 0u64;
        while data.len() < 6 << 20 {
            writeln!(data, "line {i} あいうえお").unwrap();
            i += 1;
        }
        // 書き込みハンドルを閉じてから開く（Document::open は書き込み共有を拒否する）
        std::fs::write(&path, &data).unwrap();

        let mut doc = Document::open(&path).unwrap();
        assert_eq!(doc.snapshot().line_count(), None);
        assert_eq!(doc.eol(), Eol::Lf);
        let pool = JobPool::new(3);
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        doc.start_indexing(
            &pool,
            Arc::new(move || {
                c.fetch_add(1, Ordering::Relaxed);
            }),
        );
        assert!(doc.indexing_progress().is_some());
        doc.wait_indexing();
        assert!(calls.load(Ordering::Relaxed) > 0);
        assert_eq!(doc.snapshot().line_count(), Some(i + 1));
        assert!(doc.index_version() > 0);
        assert!(doc.indexing_progress().is_none());
        doc.snapshot().check_invariants();
    }

    #[test]
    fn empty_document() {
        let doc = Document::new_empty();
        assert_eq!(doc.display_name(), "無題");
        assert_eq!(doc.snapshot().line_count(), Some(1));
        assert!(!doc.is_modified());
    }

    #[test]
    fn typing_is_coalesced_into_one_undo_step_and_few_pieces() {
        let mut d = Document::from_text("hello\n");
        d.set_selections(SelectionSet::single(Selection::caret(5)));
        for c in ", world".chars() {
            d.insert_text(&c.to_string(), false);
        }
        assert_eq!(text(&d), "hello, world\n");
        assert_eq!(carets(&d), vec![12]);
        assert!(
            d.snapshot().summary().pieces <= 3,
            "typing must not fragment pieces"
        );
        assert!(d.is_modified());
        assert!(d.undo());
        assert_eq!(text(&d), "hello\n");
        assert_eq!(carets(&d), vec![5]);
        assert!(!d.is_modified());
        assert!(d.redo());
        assert_eq!(text(&d), "hello, world\n");
        assert!(!d.redo());
    }

    #[test]
    fn multi_cursor_typing_and_backspace() {
        let mut d = Document::from_text("a\nb\nc\n");
        d.set_selections(SelectionSet::from_vec(
            vec![
                Selection::caret(1),
                Selection::caret(3),
                Selection::caret(5),
            ],
            0,
        ));
        d.insert_text("xy", false);
        assert_eq!(text(&d), "axy\nbxy\ncxy\n");
        assert_eq!(carets(&d), vec![3, 7, 11]);
        d.delete_backward();
        assert_eq!(text(&d), "ax\nbx\ncx\n");
        assert_eq!(carets(&d), vec![2, 5, 8]);
        // 1 回の Undo で 1 つの操作が全カーソル分戻る
        d.undo();
        assert_eq!(text(&d), "axy\nbxy\ncxy\n");
        d.undo();
        assert_eq!(text(&d), "a\nb\nc\n");
    }

    #[test]
    fn newline_uses_document_eol_and_auto_indent() {
        let mut d = Document::from_text("    indented\r\nnext\r\n");
        assert_eq!(d.eol(), Eol::CrLf);
        d.set_selections(SelectionSet::single(Selection::caret(12)));
        d.insert_newline(true);
        assert_eq!(text(&d), "    indented\r\n    \r\nnext\r\n");
        assert_eq!(carets(&d), vec![18]);
        // 貼り付けの改行も揃える
        d.paste("a\nb");
        assert_eq!(text(&d), "    indented\r\n    a\r\nb\r\nnext\r\n");
    }

    #[test]
    fn paste_distributes_lines_to_cursors() {
        let mut d = Document::from_text("1\n2\n3\n");
        d.set_selections(SelectionSet::from_vec(
            vec![
                Selection::caret(1),
                Selection::caret(3),
                Selection::caret(5),
            ],
            0,
        ));
        d.paste("a\r\nb\r\nc\r\n");
        assert_eq!(text(&d), "1a\n2b\n3c\n");
    }

    #[test]
    fn overwrite_mode_replaces_up_to_line_end() {
        let mut d = Document::from_text("abc\ndef");
        d.set_selections(SelectionSet::single(Selection::caret(1)));
        d.insert_text("XYZW", true);
        assert_eq!(text(&d), "aXYZW\ndef");
    }

    #[test]
    fn selection_replace_cut_and_word_delete() {
        let mut d = Document::from_text("one two three");
        d.set_selections(SelectionSet::single(Selection::new(4, 7)));
        assert_eq!(d.selected_text(100).unwrap().as_deref(), Some("two"));
        assert_eq!(d.selected_text(2), Err(3));
        d.insert_text("2", false);
        assert_eq!(text(&d), "one 2 three");
        d.move_carets(false, |s, _| s.len());
        d.delete_word_backward();
        assert_eq!(text(&d), "one 2 ");
        d.select_all();
        d.delete_selection(EditKind::Cut);
        assert_eq!(text(&d), "");
    }

    #[test]
    fn save_then_undo_and_save_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.txt");
        std::fs::write(&path, "\u{FEFF}first line\n".as_bytes()).unwrap();
        let mut d = Document::open(&path).unwrap();
        assert_eq!(d.bom(), Bom::Utf8);
        d.set_selections(SelectionSet::single(Selection::caret(10)));
        d.insert_text("!", false);
        d.save().unwrap();
        assert!(!d.is_modified());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            "\u{FEFF}first line!\n".as_bytes()
        );
        // 保存後も Undo でき、その状態を保存できる
        assert!(d.undo());
        assert!(d.is_modified());
        assert_eq!(text(&d), "first line\n");
        d.save().unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            "\u{FEFF}first line\n".as_bytes()
        );
        assert!(d.redo());
        assert_eq!(text(&d), "first line!\n");
        drop(d);
        // 退避した元ファイルはすべて削除されている
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn select_next_and_all_occurrences() {
        let mut d = Document::from_text("foo bar foo baz foo");
        d.set_selections(SelectionSet::single(Selection::caret(1)));
        // 何も選択していなければ単語を選択するだけ
        assert!(d.select_next_occurrence());
        assert_eq!(d.selections().primary().range(), 0..3);
        assert!(d.select_next_occurrence());
        assert!(d.select_next_occurrence());
        let ranges: Vec<_> = d.selections().iter().map(Selection::range).collect();
        assert_eq!(ranges, vec![0..3, 8..11, 16..19]);
        // すべて選択済みなら増えない
        assert!(!d.select_next_occurrence());
        d.insert_text("X", false);
        assert_eq!(text(&d), "X bar X baz X");

        let mut d = Document::from_text("ab ab ab ab");
        d.set_selections(SelectionSet::single(Selection::new(3, 5)));
        assert_eq!(d.select_all_occurrences(3), (3, true));
        assert_eq!(d.selections().primary().range(), 3..5);
        assert_eq!(d.select_all_occurrences(10), (4, false));
    }

    #[test]
    fn carets_at_line_ends_of_selection() {
        let mut d = Document::from_text("one\ntwo\r\nthree\nfour");
        d.set_selections(SelectionSet::single(Selection::new(1, 11)));
        assert_eq!(d.carets_at_line_ends(100), (3, false));
        assert_eq!(carets(&d), vec![3, 7, 14]);
        d.insert_text(";", false);
        assert_eq!(text(&d), "one;\ntwo;\r\nthree;\nfour");
    }

    /// 1 万カーソルでの同時入力（一括適用の経路）と、1 回の Undo での復元。
    #[test]
    fn ten_thousand_carets() {
        let text_in: String = (0..10_000).map(|i| format!("row {i}\n")).collect();
        let mut d = Document::from_text(&text_in);
        let sels: Vec<_> = text_in
            .match_indices('\n')
            .map(|(i, _)| Selection::caret(i as u64))
            .collect();
        d.set_selections(SelectionSet::from_vec(sels, 0));
        let t = std::time::Instant::now();
        d.insert_text(";", false);
        let elapsed = t.elapsed();
        eprintln!("10,000 carets: insert took {elapsed:?}");
        assert!(text(&d).starts_with("row 0;\nrow 1;\n"));
        assert_eq!(d.selections().len(), 10_000);
        d.snapshot().check_invariants();
        // 最適化なしのテストビルドでも十分速いこと（リリースビルドでは数 ms）
        assert!(elapsed.as_millis() < 2000, "took {elapsed:?}");
        d.undo();
        assert_eq!(text(&d), text_in);
    }

    #[test]
    fn save_as_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = Document::new_empty();
        d.insert_text("新規", false);
        let path = dir.path().join("new.txt");
        d.save_as(&path).unwrap();
        assert_eq!(d.display_name(), "new.txt");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "新規");
        assert!(!d.is_modified());
    }
}

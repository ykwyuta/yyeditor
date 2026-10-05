//! 文書モデルと編集。
//!
//! 文書の内容は永続ピースツリーのスナップショットで持ち、編集・Undo・保存は
//! すべてスナップショットの差し替えとして行う（01 章 4.3、06 章）。

pub mod codeview;
pub mod csv;
pub mod diff;
pub mod edit;
pub mod grep;
pub mod hex;
mod history;
mod indexer;
pub mod motion;
pub mod record;
mod replace;
mod selection;
pub mod syntax;
mod transcode;
pub mod transform;

use std::collections::HashMap;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use history::{EditKind, Version};
pub use indexer::{IndexBatch, Indexer};
pub use selection::{Selection, SelectionSet};
use yy_buffer::{Snapshot, SourceRef};
pub use yy_encoding::{DecodeStats, Encoding, EscapeMode};
pub use yy_io::SaveError;
use yy_io::{FileGuard, MmapSource, SaveFormat};
use yy_jobs::{JobPool, Notifier};
pub use yy_proto::FileId;
pub use yy_search::{Query, QueryError, ReplaceError, Replacement, Searcher};

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

/// ファイルを開くときの指定。
#[derive(Clone, Copy, Debug)]
pub struct OpenOptions {
    /// 文字コード（`None` なら自動判別）
    pub encoding: Option<Encoding>,
    /// UTF-8 以外のファイルをその場でデコードする大きさの上限。
    /// これより大きいファイルはバックグラウンドで一時ファイルに変換する
    pub sync_limit: u64,
    /// 自動判別に EBCDIC を含める
    pub detect_ebcdic: bool,
    /// ファイルのバイト列をそのまま読む（16 進数編集用。BOM も内容として扱い、保存時もそのまま書く）
    pub raw: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        OpenOptions {
            encoding: None,
            sync_limit: 32 << 20,
            detect_ebcdic: false,
            raw: false,
        }
    }
}

/// 文字コードを自動判別する（先頭の BOM のバイト数も返す）。
fn detect_encoding(bytes: &[u8], opts: &OpenOptions) -> (Encoding, usize) {
    if opts.raw {
        return (Encoding::Utf8, 0);
    }
    let n = bytes.len().min(DETECT_SAMPLE);
    let d = yy_encoding::detect(&bytes[..n], n == bytes.len());
    // UTF-8 として正しければ EBCDIC ではない（EBCDIC の英数字は 0x80 以上）
    if d.bom_len == 0
        && d.encoding != Encoding::Utf8
        && opts.detect_ebcdic
        && let Some(e) = yy_encoding::detect_ebcdic(&bytes[..n], bytes.len() as u64)
    {
        return (e, 0);
    }
    (d.encoding, d.bom_len)
}

/// 自動判別に使う先頭部分の大きさ
const DETECT_SAMPLE: usize = 64 << 10;
/// バックグラウンドで変換する間に表示する先頭部分の大きさ
const PREVIEW_BYTES: usize = 1 << 20;
/// 改行コードの変換をメモリ上で行う大きさの上限
const EOL_IN_MEMORY: u64 = 64 << 20;
/// 書き込み共有が必要なファイルを開くときに UI スレッドで読む先頭部分の大きさ
/// （これより大きなファイルはバックグラウンドでコピーする）
const SHARED_HEAD: usize = 1 << 20;

pub struct Document {
    path: Option<PathBuf>,
    /// 共有中のファイルから独立したスナップショット。編集と保存は禁止。
    read_only: bool,
    file_len: u64,
    encoding: Encoding,
    bom: bool,
    /// 同じ文字コードで保存するときのエスケープ文字の扱い
    escapes: EscapeMode,
    decode_stats: DecodeStats,
    /// バックグラウンドで変換中（その間は読み取り専用）
    loading: Option<transcode::Loader>,
    /// バックグラウンドですべて置換中（その間は読み取り専用）
    replacing: Option<replace::ReplaceJob>,
    /// 終わったすべて置換の結果（置き換えた数かエラー）
    replace_result: Option<Result<u64, String>>,
    /// バックグラウンドで保存中
    saving: Option<SaveJob>,
    /// すべて置換をその場で（メモリ上で）行う文書の大きさの上限
    replace_sync_limit: u64,
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
    load_error: Option<String>,
    /// SSH 接続先のファイル（11 章）。`path` はそれを取り寄せた手元の一時ファイル
    remote: Option<RemoteFile>,
}

/// SSH 接続先のファイルとして開いた文書の出所（11 章 7）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFile {
    /// `ssh://` で始まる場所（履歴・タイトルに使う）
    pub uri: String,
    /// ファイル名（表示用）
    pub name: String,
    /// 開いた・保存した時点のファイルの同一性（外部での変更の検出に使う）
    pub id: Option<FileId>,
}

/// リモートのファイルへの送り出し（11 章 7.1）。保存の内容を書き出した手元のファイルと、
/// 進捗の通知（送った量。`false` が返ったら中止する）を受け取り、保存したファイルの同一性を返す。
/// 保存先が外部で変更されていたら [`SaveError::Conflict`] を返すこと。
pub type Upload =
    Box<dyn FnOnce(&Path, &mut dyn FnMut(u64) -> bool) -> Result<FileId, SaveError> + Send>;

/// バックグラウンドの保存。ドロップしても保存は続く（文書を閉じても書きかけにしない）。
struct SaveJob {
    job: yy_jobs::JobHandle,
    rx: crossbeam_channel::Receiver<Result<Option<FileId>, SaveError>>,
    path: PathBuf,
    encoding: Encoding,
    format: SaveFormat,
    /// 保存している内容の版
    version: Version,
    /// リモートのファイルへの保存なら、保存後の出所
    remote: Option<RemoteFile>,
}

/// 終わった保存（[`Document::poll_save`]）。
#[derive(Debug)]
pub struct SaveDone {
    pub result: Result<(), SaveError>,
    /// 保存中に編集した（変換できない文字の範囲は保存した内容のもの）
    pub edited: bool,
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
            read_only: false,
            file_len: 0,
            encoding: Encoding::Utf8,
            bom: false,
            escapes: EscapeMode::Literal,
            decode_stats: DecodeStats::default(),
            loading: None,
            replacing: None,
            saving: None,
            replace_result: None,
            replace_sync_limit: replace::IN_MEMORY,
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
            load_error: None,
            remote: None,
        }
    }

    /// 内容を指定して作る（テスト用）。
    pub fn from_text(text: &str) -> Document {
        let mut d = Document::new_empty();
        d.snapshot = Snapshot::from_bytes(text.as_bytes().to_vec());
        d.eol = Eol::detect(&d.snapshot).unwrap_or(d.eol);
        d
    }

    /// ファイルを開く（文字コードは自動判別）。行数のカウントは
    /// [`Document::start_indexing`] で別途開始する。
    pub fn open(path: &Path) -> io::Result<Document> {
        Document::open_with(path, &OpenOptions::default())
    }

    /// 文字コードなどを指定してファイルを開く。
    pub fn open_with(path: &Path, opts: &OpenOptions) -> io::Result<Document> {
        let f = yy_io::open_file(path)?;
        let bytes = f.bytes();
        let (encoding, bom_len) = match opts.encoding.filter(|_| !opts.raw) {
            Some(e) => {
                let bom = e.bom();
                let n = if !bom.is_empty() && bytes.starts_with(bom) {
                    bom.len()
                } else {
                    0
                };
                (e, n)
            }
            None => detect_encoding(bytes, opts),
        };
        let body = bom_len..bytes.len();
        let mut doc = Document::new_empty();
        doc.encoding = encoding;
        doc.bom = bom_len > 0;
        let snapshot = if encoding == Encoding::Utf8 {
            f.snapshot(body.start as u64..body.end as u64)
        } else if (body.len() as u64) <= opts.sync_limit {
            let d = transcode::decode_in_memory(encoding, &bytes[body]);
            doc.decode_stats = d.stats;
            doc.escapes = d.escapes;
            d.snapshot
        } else {
            let source = f.source.clone().expect("non-empty file is mapped");
            doc.loading = Some(transcode::Loader::new(source, body.clone(), encoding));
            transcode::preview(encoding, &bytes[body], PREVIEW_BYTES)
        };
        doc.snapshot = index_first_piece(snapshot);
        doc.eol = Eol::detect(&doc.snapshot).unwrap_or_else(Eol::platform_default);
        doc.path = Some(f.path);
        doc.file_len = f.file_len;
        doc.source = f.source;
        doc._guard = f.guard;
        Ok(doc)
    }

    /// SSH 接続先から取り寄せた手元のファイル `cache` を、`remote` のファイルとして開く（11 章 7）。
    ///
    /// `cache` は開いたあとで名前を消す（内容は文書が参照し続け、使われなくなったら消える）。
    /// 文字コードの判別や変換は手元のファイルと同じ。
    pub fn open_remote(
        cache: &Path,
        opts: &OpenOptions,
        remote: RemoteFile,
    ) -> io::Result<Document> {
        let r = Document::open_with(cache, opts);
        let mut doc = match r {
            Ok(d) => d,
            Err(e) => {
                let _ = std::fs::remove_file(cache);
                return Err(e);
            }
        };
        match &doc.source {
            Some(s) => s.unlink(),
            None => {
                let _ = std::fs::remove_file(cache);
            }
        }
        doc.remote = Some(remote);
        Ok(doc)
    }

    /// SSH 接続先のファイルなら、その出所。
    pub fn remote(&self) -> Option<&RemoteFile> {
        self.remote.as_ref()
    }

    /// 利用者に見せるファイルの場所（リモートなら `ssh://…`、名前がなければ `None`）。
    /// 履歴・ブックマーク・ファイル種類の判定に使う（[`Document::path`] は手元の写しの場所）。
    pub fn location(&self) -> Option<PathBuf> {
        match &self.remote {
            Some(r) => Some(PathBuf::from(&r.uri)),
            None => self.path.clone(),
        }
    }

    /// 他のアプリケーションが書き込み用に開いているファイルを読み取り専用で開く。
    /// 書き込み中の mmap は安全ではないため、作業用ファイルへコピーしてからマップする。
    ///
    /// UI スレッドを止めないよう、開くときに読むのは先頭部分だけにする。それより大きなファイルの
    /// コピー（と変換）はバックグラウンドで行い（[`Document::start_indexing`] で始まる）、
    /// その間は先頭部分を表示する。
    pub fn open_shared_read_only(path: &Path, opts: &OpenOptions) -> io::Result<Document> {
        let (head, complete) = yy_io::read_shared_head(path, SHARED_HEAD.max(DETECT_SAMPLE))?;
        let (encoding, bom_len) = match opts.encoding.filter(|_| !opts.raw) {
            Some(e) => {
                let bom = e.bom();
                (
                    e,
                    usize::from(!bom.is_empty() && head.starts_with(bom)) * bom.len(),
                )
            }
            None => detect_encoding(&head, opts),
        };
        let mut doc = Document::new_empty();
        doc.read_only = true;
        doc.encoding = encoding;
        doc.bom = bom_len > 0;
        doc.file_len = std::fs::metadata(path).map_or(head.len() as u64, |m| m.len());
        let body = &head[bom_len..];
        doc.snapshot = if complete && head.len() as u64 <= opts.sync_limit.min(SHARED_HEAD as u64) {
            // 小さなファイルは読んだ内容をそのまま使う
            doc.file_len = head.len() as u64;
            if encoding == Encoding::Utf8 {
                index_first_piece(Snapshot::from_bytes(body.to_vec()))
            } else {
                let decoded = transcode::decode_in_memory(encoding, body);
                doc.decode_stats = decoded.stats;
                doc.escapes = decoded.escapes;
                index_first_piece(decoded.snapshot)
            }
        } else {
            doc.loading = Some(transcode::Loader::shared(
                path.to_owned(),
                bom_len,
                encoding,
            ));
            index_first_piece(transcode::preview(encoding, body, PREVIEW_BYTES))
        };
        doc.eol = Eol::detect(&doc.snapshot).unwrap_or_else(Eol::platform_default);
        doc.path = Some(path.to_owned());
        Ok(doc)
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// タイトルバー等に表示する名前。
    pub fn display_name(&self) -> String {
        if let Some(r) = &self.remote {
            return r.name.clone();
        }
        self.path
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "無題".to_owned())
    }

    pub fn file_len(&self) -> u64 {
        self.file_len
    }

    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// ファイルに BOM が付いているか（保存時に付けるか）。
    pub fn has_bom(&self) -> bool {
        self.bom
    }

    /// 開いたときのデコードの統計（不正なバイト・重複符号の数）。
    pub fn decode_stats(&self) -> DecodeStats {
        self.decode_stats
    }

    /// バックグラウンドで文字コードを変換中か（その間は編集できない）。
    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    /// バックグラウンドの処理（文字コードの変換・すべて置換）中で編集できないか。
    pub fn is_busy(&self) -> bool {
        self.read_only || self.loading.is_some() || self.replacing.is_some()
    }

    /// すべて置換の進捗率。実行中でなければ `None`。
    pub fn replace_progress(&self) -> Option<f64> {
        self.replacing.as_ref().map(|r| r.progress())
    }

    /// 実行中のすべて置換を中止する（結果は「中止しました」になる）。
    pub fn cancel_replace(&mut self) {
        if let Some(r) = &self.replacing {
            r.cancel();
        }
    }

    /// 終わったすべて置換の結果を取り出す（置き換えた数、またはエラーの説明）。
    pub fn take_replace_result(&mut self) -> Option<Result<u64, String>> {
        self.replace_result.take()
    }

    /// 文字コードの変換の進捗率。変換中でなければ `None`。
    pub fn loading_progress(&self) -> Option<f64> {
        self.loading.as_ref().map(|l| l.progress())
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
        if let Some(l) = &mut self.loading {
            // 先に文字コードを変換する（終わったら poll_indexing で差し替える）
            l.start(pool, notify);
            return;
        }
        if self.snapshot.is_fully_indexed() {
            return;
        }
        self.indexer = Some(Indexer::start(pool, &self.snapshot, notify));
    }

    /// 未確定の範囲が残っていてカウント中でなければ、カウントを開始する。
    ///
    /// 編集で未確定のピースが分割されたり、Undo で古い内容に戻ったりした場合に使う。
    pub fn maintain_indexing(&mut self, pool: &JobPool, notify: Notifier) {
        if self.indexer.is_none() && self.loading.is_none() {
            self.start_indexing(pool, notify);
        }
    }

    /// 届いたカウント結果を文書（と Undo 履歴）に反映する。反映したら `true`。
    ///
    /// 文字コードの変換中は、変換が終わったら内容を差し替えて `true` を返す
    /// （呼び出し側は続けて [`Document::maintain_indexing`] で行数のカウントを始めること）。
    /// 変換に失敗した場合は `Err`（先頭部分だけの読み取り専用の表示が残る）。
    pub fn poll_indexing(&mut self) -> bool {
        if let Some(r) = self.replacing.as_ref().and_then(|j| j.poll()) {
            self.replacing = None;
            self.replace_result = Some(match r {
                Ok(o) => Ok(self.apply_replace(o)),
                Err(e) => Err(e.to_string()),
            });
            self.index_version += 1;
            return true;
        }
        if let Some(l) = &mut self.loading {
            let Some(result) = l.poll() else {
                return false;
            };
            self.loading = None;
            match result {
                Ok(d) => {
                    self.snapshot = index_first_piece(d.snapshot);
                    self.decode_stats = d.stats;
                    self.escapes = d.escapes;
                    let mut sels = self.sels.clone();
                    sels.clamp(self.snapshot.len());
                    self.sels = sels;
                }
                Err(e) => {
                    self.load_error = Some(e.to_string());
                    // 保存すると先頭部分だけになってしまうので、別名保存以外はできないようにする
                    self.path = None;
                }
            }
            self.next_version += 1;
            self.version = self.next_version;
            self.saved_version = self.version;
            self.index_version += 1;
            return true;
        }
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

    /// 文字コードの変換に失敗した場合のエラー。
    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// 文字コードの変換が終わるまで待つ（テスト・ベンチマーク用）。
    pub fn wait_loading(&mut self) {
        while self.loading.is_some() {
            if !self.poll_indexing() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
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
        if self.is_busy() {
            return false;
        }
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
        if text.is_empty() || self.is_busy() {
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
        if self.is_busy() {
            return false;
        }
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
        if self.is_busy() {
            return false;
        }
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
        if self.is_busy() {
            return false;
        }
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

    /// 上書き保存（開いたときの文字コード・BOM で）。ファイル名がなければエラー
    /// （UI は「名前を付けて保存」を使う）。
    pub fn save(&mut self) -> Result<(), SaveError> {
        if self.remote.is_some() {
            return Err(io::Error::other(
                "リモートのファイルは start_save_remote で保存してください",
            )
            .into());
        }
        let path = self
            .path
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ファイル名がありません"))?;
        self.save_as(&path)
    }

    /// 名前を付けて保存する（文字コード・BOM は現在のまま）。
    pub fn save_as(&mut self, path: &Path) -> Result<(), SaveError> {
        self.save_as_with(path, self.encoding, self.bom)
    }

    /// 保存するときの形式。開いたときと別の文字コードで保存する場合、読み込み時に
    /// 不正だったバイト（エスケープ文字）は変換できない文字になる。
    pub fn save_format(&self, encoding: Encoding, bom: bool) -> SaveFormat {
        // EBCDIC のレコードの区切り方だけを変える場合も不正だったバイトは元のバイトに戻せる
        let escapes = if encoding.same_charset(&self.encoding) {
            self.escapes
        } else if self.escapes == EscapeMode::Restore {
            EscapeMode::Reject
        } else {
            EscapeMode::Literal
        };
        SaveFormat {
            encoding,
            bom: bom && encoding.supports_bom(),
            escapes,
            // SI なしで終わっていたファイルは、同じ文字コードなら SI を加えずに保存する
            keep_open_shift: encoding.same_charset(&self.encoding)
                && self.decode_stats.open_shift_at_end,
        }
    }

    /// 文字コードと BOM を指定して保存する。保存後は保存したファイルを参照し直す（06 章 3.4）。
    ///
    /// 変換できない文字があれば [`SaveError::Unmappable`] を返し、ファイルは変更しない。
    /// 行数は数え直しになるため、呼び出し側は [`Document::maintain_indexing`] を呼ぶこと。
    pub fn save_as_with(
        &mut self,
        path: &Path,
        encoding: Encoding,
        bom: bool,
    ) -> Result<(), SaveError> {
        let format = self.check_save(encoding, bom)?;
        yy_io::save_snapshot(&self.snapshot, path, &format, self.source.as_deref())?;
        self.apply_saved(path, encoding, &format, self.version);
        self.remote = None;
        Ok(())
    }

    /// 保存を始められるか確かめ、保存する形式を返す。
    fn check_save(&self, encoding: Encoding, bom: bool) -> Result<SaveFormat, SaveError> {
        if self.is_busy() {
            return Err(io::Error::other("文字コードの変換中・置換中は保存できません").into());
        }
        if self.saving.is_some() {
            return Err(io::Error::other("保存中です").into());
        }
        Ok(self.save_format(encoding, bom))
    }

    /// バックグラウンドで保存を始める（06 章 3.3）。保存中も編集できる。
    ///
    /// 保存するのは始めたときの内容で、終わったら `notify` を呼ぶ。結果は
    /// [`Document::poll_save`] で受け取る（そのときに保存したファイルを参照し直す）。
    pub fn start_save(
        &mut self,
        path: &Path,
        encoding: Encoding,
        bom: bool,
        pool: &JobPool,
        notify: Notifier,
    ) -> Result<(), SaveError> {
        self.spawn_save(path.to_owned(), encoding, bom, pool, notify, None)
    }

    /// SSH 接続先のファイル `target` として保存する（11 章 7.1）。保存の内容は手元の一時ファイルに
    /// 書き出してから `upload` で送り出す（どちらもバックグラウンド）。送り出せたら保存できたことに
    /// なり、文書は `target` のファイルになる。結果は [`Document::poll_save`] で受け取る。
    pub fn start_save_remote(
        &mut self,
        encoding: Encoding,
        bom: bool,
        pool: &JobPool,
        notify: Notifier,
        target: RemoteFile,
        upload: Upload,
    ) -> Result<(), SaveError> {
        let staging = yy_io::temp_path("remote");
        self.spawn_save(staging, encoding, bom, pool, notify, Some((target, upload)))
    }

    fn spawn_save(
        &mut self,
        path: PathBuf,
        encoding: Encoding,
        bom: bool,
        pool: &JobPool,
        notify: Notifier,
        remote: Option<(RemoteFile, Upload)>,
    ) -> Result<(), SaveError> {
        let format = self.check_save(encoding, bom)?;
        // 保存する内容を Undo の区切りにする（保存中の入力をまとめない）
        self.typing = None;
        self.history.seal();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let snap = self.snapshot.clone();
        let source = self.source.clone();
        let target = path.clone();
        let fmt = format;
        let (remote, upload) = match remote {
            Some((r, u)) => (Some(r), Some(u)),
            None => (None, None),
        };
        let job = pool.spawn(move |ctx| {
            let len = snap.len();
            // リモートなら、書き出しと送り出しで半分ずつ
            let total = if upload.is_some() { len * 2 } else { len };
            ctx.progress.set_total(total);
            let r = yy_io::save_snapshot_with(&snap, &target, &fmt, source.as_deref(), &mut |p| {
                ctx.progress.set_done(p);
                !ctx.cancel.is_cancelled()
            });
            let r = match (r, upload) {
                (Ok(()), Some(upload)) => {
                    let r = upload(&target, &mut |sent| {
                        ctx.progress.set_done((len + sent).min(total));
                        !ctx.cancel.is_cancelled()
                    });
                    if r.is_err() {
                        let _ = std::fs::remove_file(&target);
                    }
                    r.map(Some)
                }
                (r, _) => r.map(|()| None),
            };
            let _ = tx.send(r);
            notify();
        });
        self.saving = Some(SaveJob {
            job,
            rx,
            path,
            encoding,
            format,
            version: self.version,
            remote,
        });
        Ok(())
    }

    /// バックグラウンドで保存中か。
    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    /// 保存の進捗率。保存中でなければ `None`。
    pub fn save_progress(&self) -> Option<f64> {
        self.saving.as_ref().map(|s| s.job.progress().fraction())
    }

    /// 保存を中止する（書き出し中なら、ファイルは変更しない）。結果は [`Document::poll_save`] で
    /// 受け取る。
    pub fn cancel_save(&self) {
        if let Some(s) = &self.saving {
            s.job.cancel();
        }
    }

    /// バックグラウンドの保存が終わっていれば結果を返す。保存できていれば保存したファイルを
    /// 参照し直す（行数を数え直すことがあるので、呼び出し側は [`Document::maintain_indexing`]
    /// を呼ぶこと）。
    pub fn poll_save(&mut self) -> Option<SaveDone> {
        let s = self.saving.as_ref()?;
        let result = match s.rx.try_recv() {
            Ok(r) => r,
            Err(crossbeam_channel::TryRecvError::Empty) => return None,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                Err(io::Error::other("保存中に内部エラーが発生しました").into())
            }
        };
        let s = self.saving.take().expect("saving");
        let edited = self.version != s.version;
        let result = match result {
            Ok(id) => {
                self.apply_saved(&s.path, s.encoding, &s.format, s.version);
                self.remote = s.remote.map(|mut r| {
                    r.id = id;
                    r
                });
                if self.remote.is_some() {
                    // 保存した内容を書き出した手元の一時ファイルを、次に使う手元の写しにする
                    match &self.source {
                        Some(src) => src.unlink(),
                        None => {
                            let _ = std::fs::remove_file(&s.path);
                        }
                    }
                }
                Ok(())
            }
            Err(e) => Err(e),
        };
        Some(SaveDone { result, edited })
    }

    /// 保存し終えた文書の状態を更新する。`version` は保存した内容の版。
    fn apply_saved(
        &mut self,
        path: &Path,
        encoding: Encoding,
        format: &SaveFormat,
        version: Version,
    ) {
        let edited = self.version != version;
        // 保存したファイルを開き直す（書き込みを拒否するため。UTF-8 なら内容もマップし直して
        // 追記バッファや退避した元ファイルを解放できるようにする。保存中に編集していれば
        // 内容はそのまま）
        if let Ok(o) = yy_io::open_file(path) {
            let bom_len = if format.bom { encoding.bom().len() } else { 0 } as u64;
            if !edited && encoding == Encoding::Utf8 && o.file_len - bom_len == self.snapshot.len()
            {
                self.snapshot = index_first_piece(o.snapshot(bom_len..o.file_len));
                self.indexer = None;
                self.index_version += 1;
            }
            self.source = o.source;
            self._guard = o.guard;
            self.file_len = o.file_len;
        }
        if !encoding.same_charset(&self.encoding) {
            self.escapes = if encoding == Encoding::Utf8 {
                EscapeMode::Literal
            } else {
                EscapeMode::Restore
            };
            self.decode_stats = DecodeStats::default();
        }
        self.encoding = encoding;
        self.bom = format.bom;
        self.path = Some(path.to_owned());
        self.saved_version = version;
        if !edited {
            self.typing = None;
            self.history.seal();
        }
    }

    /// 文書内の範囲 `ranges`（昇順・重なりなし）を `f(元の内容)` で置き換える
    /// （保存できない文字の置き換えなど）。1 つの Undo 単位になる。
    pub fn replace_ranges(&mut self, ranges: &[Range<u64>], f: impl Fn(&[u8]) -> Vec<u8>) -> bool {
        let changes: Vec<Change> = ranges
            .iter()
            .map(|r| Change::replace_bytes(r.clone(), f(&self.snapshot.read(r.clone()))))
            .collect();
        let primary = self.sels.primary().head;
        self.apply_changes(changes.clone(), EditKind::Other, |a| {
            let _ = a;
            SelectionSet::single(Selection::caret(edit::map_offset(&changes, primary)))
        })
    }

    /// 改行コードを `eol` に揃える（CRLF と LF。単独の CR はそのまま）。1 つの Undo 単位になる。
    /// 以後の入力もこの改行コードになる。変更があれば `true`。
    ///
    /// その場で行う（大きな文書は一時ファイルに書き出してマップする）。UI では
    /// [`Document::convert_eol_with`] を使う。
    pub fn convert_eol(&mut self, eol: Eol) -> io::Result<bool> {
        if self.is_busy() {
            return Ok(false);
        }
        self.eol = eol;
        let caret = self.sels.primary().head;
        match convert_eol_snapshot(&self.snapshot, eol, caret, EOL_IN_MEMORY, &mut |_| true) {
            Ok(o) => Ok(self.apply_replace(replace::Outcome::Eol(o)) > 0),
            Err(ReplaceError::Io(e)) => Err(e),
            Err(e) => Err(io::Error::other(e.to_string())),
        }
    }

    /// [`Document::convert_eol`] と同じ。ただし大きな文書（すべて置換をその場で行う上限より
    /// 大きい）はバックグラウンドで始めて `Ok(None)` を返す（終わると
    /// [`Document::poll_indexing`] で反映し、結果は [`Document::take_replace_result`] で
    /// 受け取る。変換中は読み取り専用）。
    pub fn convert_eol_with(
        &mut self,
        eol: Eol,
        pool: &JobPool,
        notify: Notifier,
    ) -> io::Result<Option<bool>> {
        if self.snapshot.len() <= self.replace_sync_limit || self.is_busy() {
            return self.convert_eol(eol).map(Some);
        }
        self.eol = eol;
        let caret = self.sels.primary().head;
        let snap = self.snapshot.clone();
        self.typing = None;
        self.replacing = Some(replace::ReplaceJob::start_task(
            pool,
            notify,
            0..snap.len(),
            Box::new(move |step| {
                convert_eol_snapshot(&snap, eol, caret, EOL_IN_MEMORY, step)
                    .map(replace::Outcome::Eol)
            }),
        ));
        Ok(None)
    }
}

/// `snap` の改行コードを `eol` に揃えた内容と、カーソル `caret` を移した位置（変更がなければ
/// `None`）。`in_memory` より大きな文書は一時ファイルに書き出してマップする。
/// `step(処理した位置)` が `false` を返したら中止する。
fn convert_eol_snapshot(
    snap: &Snapshot,
    eol: Eol,
    caret: u64,
    in_memory: u64,
    step: &mut dyn FnMut(u64) -> bool,
) -> Result<Option<(Snapshot, u64)>, ReplaceError> {
    let mut conv = EolConverter::new(eol, caret);
    // 進捗を知らせ、中止できるように分けて変換する
    let mut feed_all =
        |conv: &mut EolConverter, emit: &mut dyn FnMut(&[u8])| -> Result<(), ReplaceError> {
            let mut pos = 0u64;
            for c in snap.chunks(0..snap.len()) {
                for part in c.chunks(1 << 20) {
                    conv.feed(part, emit);
                    pos += part.len() as u64;
                    if !step(pos) {
                        return Err(ReplaceError::Cancelled);
                    }
                }
            }
            conv.finish(emit);
            Ok(())
        };
    let snapshot = if snap.len() <= in_memory {
        let mut out = Vec::with_capacity(snap.len() as usize);
        feed_all(&mut conv, &mut |b| out.extend_from_slice(b))?;
        if !conv.changed {
            return Ok(None);
        }
        Snapshot::from_bytes(out)
    } else {
        let path = yy_io::temp_path("eol");
        let mut write = |conv: &mut EolConverter| -> Result<u64, ReplaceError> {
            let file = std::fs::File::create(&path)?;
            let mut w = io::BufWriter::with_capacity(1 << 20, file);
            let mut err = Ok(());
            let mut n = 0u64;
            let mut emit = |b: &[u8]| {
                n += b.len() as u64;
                if err.is_ok() {
                    err = io::Write::write_all(&mut w, b);
                }
            };
            feed_all(conv, &mut emit)?;
            err?;
            // 一時ファイルなので永続化（sync）は不要
            io::Write::flush(&mut w)?;
            Ok(n)
        };
        let len = match write(&mut conv) {
            Ok(n) => n,
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                return Err(e);
            }
        };
        if !conv.changed {
            let _ = std::fs::remove_file(&path);
            return Ok(None);
        }
        let map: SourceRef = yy_io::map_temp(&path)?;
        Snapshot::from_source(map, 0..len, false)
    };
    Ok(Some((snapshot, conv.mapped_caret())))
}

impl Document {
    // ---- 置換 ------------------------------------------------------------

    /// 主選択がちょうど検索条件に一致していれば置き換える（1 つの Undo 単位）。
    pub fn replace_selection(&mut self, searcher: &Searcher, repl: &Replacement) -> bool {
        if self.is_busy() {
            return false;
        }
        let sel = *self.sels.primary();
        let r = sel.range();
        let hit = searcher
            .find_next(&self.snapshot, r.clone(), r.start, &mut |_| true)
            .ok()
            .flatten();
        if hit != Some(r.clone()) {
            return false;
        }
        let Ok(Some(edits)) =
            yy_search::collect_edits(searcher, &self.snapshot, r.clone(), repl, 1, &mut |_| true)
        else {
            return false;
        };
        let Some((range, bytes)) = edits.into_iter().next() else {
            return false;
        };
        let len = bytes.len() as u64;
        let change = Change::replace_bytes(range.clone(), bytes);
        self.apply_changes(vec![change], EditKind::Other, |_| {
            SelectionSet::single(Selection::caret(range.start + len))
        })
    }

    /// `range` 内をすべて置換する。小さな文書はその場で行って置き換えた数を返し、
    /// 大きな文書はバックグラウンドで始めて `Ok(None)` を返す（終わると [`Document::poll_indexing`]
    /// が `true` を返し、[`Document::take_replace_result`] で結果が分かる。その間は読み取り専用）。
    pub fn replace_all(
        &mut self,
        searcher: Arc<Searcher>,
        repl: Replacement,
        range: Range<u64>,
        pool: &JobPool,
        notify: Notifier,
    ) -> Result<Option<u64>, ReplaceError> {
        if self.is_busy() {
            return Err(ReplaceError::Io(io::Error::other("処理中です")));
        }
        let limit = self.replace_sync_limit;
        if self.snapshot.len() <= limit {
            let o = replace::run(&searcher, &self.snapshot, range, &repl, limit, &mut |_| {
                true
            })?;
            return Ok(Some(self.apply_replace(o)));
        }
        self.typing = None;
        self.replacing = Some(replace::ReplaceJob::start(
            pool,
            notify,
            searcher,
            repl,
            self.snapshot.clone(),
            range,
            limit,
        ));
        Ok(None)
    }

    /// 区切り文字形式の文書をレコードごとに書き直す（列の挿入・削除、区切り文字の変換）。
    /// 1 回の Undo で戻せる。小さな文書はその場で行って書き直したレコード数を返し、
    /// 大きな文書はバックグラウンドで始めて `Ok(None)` を返す（[`Document::replace_all`] と同じ）。
    pub fn transform_records(
        &mut self,
        dialect: yy_delimited::Dialect,
        op: csv::RecordOp,
        pool: &JobPool,
        notify: Notifier,
    ) -> Result<Option<u64>, ReplaceError> {
        if self.is_busy() {
            return Err(ReplaceError::Io(io::Error::other("処理中です")));
        }
        let limit = self.replace_sync_limit;
        let snap = self.snapshot.clone();
        let len = snap.len();
        if len <= limit {
            let o = replace::produce(len, limit, &mut |w| {
                csv::rewrite_records(&snap, dialect, op, w, &mut |_| true)
            })?;
            return Ok(Some(self.apply_replace(o)));
        }
        self.typing = None;
        self.replacing = Some(replace::ReplaceJob::start_task(
            pool,
            notify,
            0..len,
            Box::new(move |step| {
                replace::produce(len, limit, &mut |w| {
                    csv::rewrite_records(&snap, dialect, op, w, step)
                })
            }),
        ));
        Ok(None)
    }

    /// 各選択範囲（空なら単語）の文字列を `t` で変換する（1 つの Undo 単位）。変換後の文字列を
    /// 選択したままにする。文書に含まれる不正なバイトはそのまま残す。変更があれば `true`。
    pub fn transform_selections(&mut self, t: transform::Transform) -> bool {
        if self.is_busy() {
            return false;
        }
        let snap = self.snapshot.clone();
        let primary_head = self.sels.primary().head;
        let mut ranges: Vec<Range<u64>> = self
            .sels
            .iter()
            .map(|s| {
                if s.is_empty() {
                    motion::word_range(&snap, s.head)
                } else {
                    s.range()
                }
            })
            .filter(|r| !r.is_empty())
            .collect();
        ranges.sort_by_key(|r| r.start);
        let mut merged: Vec<Range<u64>> = Vec::new();
        for r in ranges {
            match merged.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => merged.push(r),
            }
        }
        let mut changes = Vec::new();
        let mut new_sels = Vec::new();
        let mut delta: i128 = 0;
        let mut primary = 0;
        for r in &merged {
            let old = snap.read(r.clone());
            let mut new = Vec::with_capacity(old.len());
            for chunk in old.utf8_chunks() {
                new.extend_from_slice(t.apply(chunk.valid()).as_bytes());
                new.extend_from_slice(chunk.invalid());
            }
            let start = (r.start as i128 + delta) as u64;
            if r.contains(&primary_head) || r.end == primary_head {
                primary = new_sels.len();
            }
            new_sels.push(Selection::new(start, start + new.len() as u64));
            delta += new.len() as i128 - old.len() as i128;
            if new != old {
                changes.push(Change::replace_bytes(r.clone(), new));
            }
        }
        if changes.is_empty() {
            return false;
        }
        self.apply_changes(changes, EditKind::Other, |_| {
            SelectionSet::from_vec(new_sels, primary)
        })
    }

    /// 重複する行を除く（前に同じ内容の行があれば、その行を削除する。改行コードの違いは無視）。
    /// 選択範囲があればそれを含む行の中で、なければ文書全体で。1 回の Undo で戻せる。
    /// 小さな文書はその場で行って除いた行数を返し、大きな文書はバックグラウンドで始めて
    /// `Ok(None)` を返す（[`Document::replace_all`] と同じ）。
    pub fn dedup_lines(
        &mut self,
        pool: &JobPool,
        notify: Notifier,
    ) -> Result<Option<u64>, ReplaceError> {
        if self.is_busy() {
            return Err(ReplaceError::Io(io::Error::other("処理中です")));
        }
        let snap = self.snapshot.clone();
        let len = snap.len();
        let range = if self.sels.iter().all(|s| s.is_empty()) {
            0..len
        } else {
            let start = self.sels.iter().map(|s| s.start()).min().unwrap_or(0);
            let end = self.sels.iter().map(|s| s.end()).max().unwrap_or(0);
            let first = motion::line_start(&snap, start);
            // 選択の終わりが行頭なら、その行は含めない
            let last = if end > start && motion::line_start(&snap, end) == end {
                end
            } else {
                snap.find_next(end..len, b'\n').map_or(len, |n| n + 1)
            };
            first..last
        };
        let limit = self.replace_sync_limit;
        if len <= limit {
            let o = replace::produce(len, limit, &mut |w| {
                transform::dedup_lines(&snap, range.clone(), w, &mut |_| true)
            })?;
            return Ok(Some(self.apply_replace(o)));
        }
        self.typing = None;
        self.replacing = Some(replace::ReplaceJob::start_task(
            pool,
            notify,
            0..len,
            Box::new(move |step| {
                replace::produce(len, limit, &mut |w| {
                    transform::dedup_lines(&snap, range.clone(), w, step)
                })
            }),
        ));
        Ok(None)
    }

    /// すべて置換をその場で行う文書の大きさの上限を変える（テスト用）。
    pub fn set_replace_sync_limit(&mut self, bytes: u64) {
        self.replace_sync_limit = bytes;
    }

    /// すべて置換の結果を文書に反映し、置き換えた数を返す。
    fn apply_replace(&mut self, o: replace::Outcome) -> u64 {
        let primary = self.sels.primary().head;
        match o {
            replace::Outcome::Edits(edits) => {
                let n = edits.len() as u64;
                let changes: Vec<Change> = edits
                    .into_iter()
                    .map(|(r, b)| Change::replace_bytes(r, b))
                    .collect();
                let caret = edit::map_offset(&changes, primary);
                self.apply_changes(changes, EditKind::Other, |_| {
                    SelectionSet::single(Selection::caret(caret))
                });
                n
            }
            replace::Outcome::Rewritten(n, snap) => {
                if n > 0 {
                    let snap = index_first_piece(snap);
                    let caret = primary.min(snap.len());
                    self.typing = None;
                    self.commit(
                        snap,
                        SelectionSet::single(Selection::caret(caret)),
                        EditKind::Other,
                    );
                }
                n
            }
            replace::Outcome::Eol(None) => 0,
            replace::Outcome::Eol(Some((snap, caret))) => {
                self.typing = None;
                self.commit(
                    index_first_piece(snap),
                    SelectionSet::single(Selection::caret(caret)),
                    EditKind::Other,
                );
                1
            }
        }
    }
}

/// 改行コードを揃えるストリーム変換。
struct EolConverter {
    eol: Eol,
    /// 前のチャンクが CR で終わっていた
    held_cr: bool,
    changed: bool,
    /// 入力位置
    pos: u64,
    caret: u64,
    /// カーソルより前の改行の長さの増減
    caret_delta: i64,
}

impl EolConverter {
    fn new(eol: Eol, caret: u64) -> EolConverter {
        EolConverter {
            eol,
            held_cr: false,
            changed: false,
            pos: 0,
            caret,
            caret_delta: 0,
        }
    }

    /// 入力位置 `at` から始まる改行（CRLF なら `crlf`）を出力する。
    fn line_end(&mut self, at: u64, crlf: bool, emit: &mut dyn FnMut(&[u8])) {
        emit(self.eol.as_bytes());
        let old = if crlf { 2 } else { 1 };
        let new = self.eol.as_bytes().len() as i64;
        if old != new {
            self.changed = true;
            if at < self.caret {
                self.caret_delta += new - old;
            }
        }
    }

    fn feed(&mut self, chunk: &[u8], emit: &mut dyn FnMut(&[u8])) {
        if chunk.is_empty() {
            return;
        }
        let mut start = 0;
        if self.held_cr {
            self.held_cr = false;
            if chunk.first() == Some(&b'\n') {
                self.line_end(self.pos - 1, true, emit);
                start = 1;
            } else {
                emit(b"\r");
            }
        }
        while let Some(k) = memchr::memchr(b'\n', &chunk[start..]) {
            let p = start + k;
            let crlf = p > start && chunk[p - 1] == b'\r';
            let end = if crlf { p - 1 } else { p };
            emit(&chunk[start..end]);
            self.line_end(self.pos + end as u64, crlf, emit);
            start = p + 1;
        }
        let tail = &chunk[start..];
        if tail.last() == Some(&b'\r') {
            emit(&tail[..tail.len() - 1]);
            self.held_cr = true;
        } else {
            emit(tail);
        }
        self.pos += chunk.len() as u64;
    }

    fn finish(&mut self, emit: &mut dyn FnMut(&[u8])) {
        if self.held_cr {
            self.held_cr = false;
            emit(b"\r");
        }
    }

    fn mapped_caret(&self) -> u64 {
        (self.caret as i64 + self.caret_delta).max(0) as u64
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

    /// チャンクの区切りが CR と LF の間にあっても正しく変換する。
    #[test]
    fn eol_converter_handles_chunk_boundaries() {
        let input = b"a\r\nb\nc\rd\r\n\r";
        for eol in [Eol::Lf, Eol::CrLf] {
            let expect: Vec<u8> = match eol {
                Eol::Lf => b"a\nb\nc\rd\n\r".to_vec(),
                Eol::CrLf => b"a\r\nb\r\nc\rd\r\n\r".to_vec(),
            };
            for cut in 0..=input.len() {
                for cut2 in cut..=input.len() {
                    let mut c = EolConverter::new(eol, 10);
                    let mut out = Vec::new();
                    let mut emit = |b: &[u8]| out.extend_from_slice(b);
                    c.feed(&input[..cut], &mut emit);
                    c.feed(&input[cut..cut2], &mut emit);
                    c.feed(&input[cut2..], &mut emit);
                    c.finish(&mut emit);
                    assert_eq!(out, expect, "{eol:?} {cut} {cut2}");
                    // 位置 10（"d" の後の改行の後）: LF なら CRLF 2 つ分前へ、CRLF なら LF 1 つ分後へ
                    let caret = if eol == Eol::Lf { 8 } else { 11 };
                    assert_eq!(c.mapped_caret(), caret);
                }
            }
        }
    }

    #[test]
    fn save_then_undo_and_save_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.txt");
        std::fs::write(&path, "\u{FEFF}first line\n".as_bytes()).unwrap();
        let mut d = Document::open(&path).unwrap();
        assert!(d.has_bom());
        assert_eq!(d.encoding(), Encoding::Utf8);
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

    fn wait_save(d: &mut Document) -> SaveDone {
        let t = std::time::Instant::now();
        loop {
            if let Some(done) = d.poll_save() {
                return done;
            }
            assert!(t.elapsed().as_secs() < 10, "save did not finish");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn background_save_keeps_edits_made_while_saving() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.txt");
        std::fs::write(&path, "abc\n").unwrap();
        let mut d = Document::open(&path).unwrap();
        let pool = JobPool::new(2);
        d.set_selections(SelectionSet::single(Selection::caret(3)));
        d.insert_text("1", false);
        d.start_save(&path, Encoding::Utf8, false, &pool, Arc::new(|| {}))
            .unwrap();
        assert!(d.is_saving());
        // 保存中の 2 回目の保存は断る
        assert!(
            d.start_save(&path, Encoding::Utf8, false, &pool, Arc::new(|| {}))
                .is_err()
        );
        // 保存中も編集できる（保存するのは始めたときの内容）
        d.insert_text("2", false);
        let done = wait_save(&mut d);
        assert!(done.result.is_ok());
        assert!(done.edited);
        assert!(!d.is_saving());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abc1\n");
        assert_eq!(text(&d), "abc12\n");
        assert!(d.is_modified());
        // Undo で保存した内容に戻すと変更なしになる
        assert!(d.undo());
        assert_eq!(text(&d), "abc1\n");
        assert!(!d.is_modified());
        assert!(d.redo());
        d.start_save(&path, Encoding::Utf8, false, &pool, Arc::new(|| {}))
            .unwrap();
        let done = wait_save(&mut d);
        assert!(done.result.is_ok() && !done.edited);
        assert!(!d.is_modified());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abc12\n");
    }

    #[test]
    fn cancelled_background_save_leaves_file_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.txt");
        std::fs::write(&path, "old\n").unwrap();
        let mut d = Document::open(&path).unwrap();
        d.insert_text(&"x".repeat(8 << 20), false);
        // ワーカーを塞いでおき、保存が始まる前に中止する
        let pool = JobPool::new(1);
        let (tx, rx) = crossbeam_channel::bounded::<()>(0);
        pool.spawn(move |_| {
            let _ = rx.recv();
        });
        d.start_save(&path, Encoding::Utf8, false, &pool, Arc::new(|| {}))
            .unwrap();
        d.cancel_save();
        drop(tx);
        let done = wait_save(&mut d);
        match done.result {
            Err(SaveError::Io(e)) => assert!(yy_io::is_cancelled(&e), "{e}"),
            r => panic!("{r:?}"),
        }
        assert!(d.is_modified());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old\n");
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn background_save_reports_unmappable_characters() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.txt");
        let mut d = Document::new_empty();
        d.insert_text("a😀b", false);
        let pool = JobPool::new(1);
        d.start_save(&path, Encoding::Cp932, false, &pool, Arc::new(|| {}))
            .unwrap();
        match wait_save(&mut d).result {
            Err(SaveError::Unmappable { ranges, total, .. }) => {
                assert_eq!(total, 1);
                assert_eq!(ranges, vec![1..5]);
            }
            r => panic!("{r:?}"),
        }
        assert!(!path.exists());
        assert!(d.is_modified());
        assert_eq!(d.path(), None);
    }

    fn remote_file(name: &str, len: u64) -> RemoteFile {
        RemoteFile {
            uri: format!("ssh://host/home/u/{name}"),
            name: name.into(),
            id: Some(FileId {
                dev: 1,
                ino: 2,
                len,
                mtime_ns: 3,
            }),
        }
    }

    #[test]
    fn remote_documents_save_by_uploading() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache.tmp");
        let remote = dir.path().join("remote.txt");
        // Shift_JIS のファイルを取り寄せた写し
        let sjis = b"\x93\xfa\x96\x7b\x8c\xea\n"; // 「日本語」
        std::fs::write(&cache, sjis).unwrap();
        std::fs::write(&remote, sjis).unwrap();
        let mut d = Document::open_remote(&cache, &OpenOptions::default(), remote_file("a.txt", 7))
            .unwrap();
        // 写しの名前は開いたら消す
        assert!(!cache.exists());
        assert_eq!(d.display_name(), "a.txt");
        assert_eq!(
            d.location().unwrap().extension().unwrap(),
            std::ffi::OsStr::new("txt")
        );
        assert_eq!(d.location(), Some(PathBuf::from("ssh://host/home/u/a.txt")));
        assert_eq!(d.encoding(), Encoding::Cp932);
        assert!(d.save().is_err());
        d.set_selections(SelectionSet::single(Selection::caret(0)));
        d.insert_text("新しい", false);

        let pool = JobPool::new(1);
        let target = remote.clone();
        let upload: Upload = Box::new(move |local, progress| {
            assert!(progress(1));
            std::fs::copy(local, &target)?;
            Ok(FileId {
                dev: 1,
                ino: 2,
                len: std::fs::metadata(&target)?.len(),
                mtime_ns: 4,
            })
        });
        d.start_save_remote(
            Encoding::Cp932,
            false,
            &pool,
            Arc::new(|| {}),
            remote_file("a.txt", 7),
            upload,
        )
        .unwrap();
        let done = wait_save(&mut d);
        assert!(done.result.is_ok(), "{:?}", done.result);
        assert!(!d.is_modified());
        let expected = b"\x90\x56\x82\xb5\x82\xa2\x93\xfa\x96\x7b\x8c\xea\n"; // 「新しい日本語」
        assert_eq!(std::fs::read(&remote).unwrap(), expected);
        assert_eq!(d.remote().unwrap().id.unwrap().mtime_ns, 4);
        // 書き出した手元の一時ファイルも名前は残さない
        assert!(!d.path().unwrap().exists());
        assert_eq!(text(&d), "新しい日本語\n");

        // 外部で変更されていれば保存しない（変更ありのまま、出所も変えない）
        d.insert_text("!", false);
        let upload: Upload = Box::new(|_, _| Err(SaveError::Conflict("changed".into())));
        d.start_save_remote(
            Encoding::Cp932,
            false,
            &pool,
            Arc::new(|| {}),
            remote_file("a.txt", 0),
            upload,
        )
        .unwrap();
        let done = wait_save(&mut d);
        assert!(matches!(done.result, Err(SaveError::Conflict(_))));
        assert!(d.is_modified());
        assert_eq!(d.remote().unwrap().id.unwrap().mtime_ns, 4);
        assert_eq!(std::fs::read(&remote).unwrap(), expected);

        // 手元に保存し直せば手元のファイルになる
        let local = dir.path().join("local.txt");
        d.start_save(&local, Encoding::Utf8, false, &pool, Arc::new(|| {}))
            .unwrap();
        assert!(wait_save(&mut d).result.is_ok());
        assert!(d.remote().is_none());
        assert_eq!(d.display_name(), "local.txt");
    }

    #[test]
    fn empty_remote_files_open() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache.tmp");
        std::fs::write(&cache, b"").unwrap();
        let d =
            Document::open_remote(&cache, &OpenOptions::default(), remote_file("e", 0)).unwrap();
        assert!(!cache.exists());
        assert_eq!(d.snapshot().len(), 0);
        let e = Document::open_remote(&cache, &OpenOptions::default(), remote_file("e", 0));
        assert!(e.is_err());
    }
}

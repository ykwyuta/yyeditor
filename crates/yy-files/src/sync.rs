//! 共有フォルダへの同期（18 章 4）。
//!
//! 1. 両側の目録を突き合わせて計画（[`Plan`]）を作る（[`plan`]）。前回の同期の記録（[`SyncState`]）と
//!    比べて、送り先がその後に変わっていれば「衝突」にする。
//! 2. 計画から実行（[`Run`]）を作り、ジャーナルに書きながら送る（[`execute`]）。送り先には
//!    `<名前>.yypart` に書き、終わったら大きさを確かめてから置き換える。
//! 3. 途中で切れたら（回線・アプリの終了・電源断）、ジャーナルから続ける（同じ [`execute`]）。送りかけの
//!    ファイルは「ジャーナルの位置」と「途中のファイルの大きさ」の小さい方から、直前の 64 KiB を照合して
//!    続ける。
//! 4. 大きなファイル（既定 64 MiB 以上）は、前回送ったときの送り先のブロックごとのハッシュを覚えておき、
//!    送り先が前回のままなら、送り先を `.yypart` にサーバー側でコピーして、違うブロックだけを書く
//!    （差分の送り方。18 章 4.3）。

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::fs::{Fs, Meta, is_crash, is_transient};
use crate::scan::{Catalog, FileEntry};

/// 送りかけのファイルの名前に付ける拡張子。
pub const PART_EXT: &str = "yypart";

/// やり方。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// 新しいファイルと変わったファイルを送る（送り先にだけあるものはそのまま）
    #[default]
    Update,
    /// それに加えて、送り元にないファイルを送り先から消す（隔離フォルダへ移す）
    Mirror,
}

/// 比べ方。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Compare {
    /// 大きさと更新日時（rsync と同じ）
    #[default]
    SizeTime,
    /// 大きさが同じで日時だけ違うものは中身で比べる
    Content,
}

/// 同期の設定。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncOptions {
    pub mode: Mode,
    pub compare: Compare,
    /// 更新日時の誤差の許し幅（ナノ秒。既定 2 秒）
    pub time_tolerance: i64,
    /// 同時に送るファイルの数
    pub threads: usize,
    /// 送った後で中身のハッシュも比べる
    pub verify_hash: bool,
    /// これだけ書くごとに、確かな位置を進める（時間でも 1 秒ごとに進める）
    pub checkpoint_bytes: u64,
    /// この大きさ以上のファイルは差分の送り方にする（0 は使わない）
    pub delta_min: u64,
    /// 差分の送り方のブロックの大きさ
    pub delta_block: u64,
}

impl Default for SyncOptions {
    fn default() -> Self {
        SyncOptions {
            mode: Mode::Update,
            compare: Compare::SizeTime,
            time_tolerance: 2_000_000_000,
            threads: 4,
            verify_hash: false,
            checkpoint_bytes: 64 << 20,
            delta_min: 64 << 20,
            delta_block: 1 << 20,
        }
    }
}

/// 計画の 1 件の扱い。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// 送り先にない
    New,
    /// 送り先が古い
    Update,
    /// 中身は同じで日時だけ違う（送り先の日時を合わせる）
    Touch,
    /// 送り元にない（ミラー。隔離フォルダへ移す）
    Delete,
    /// 送り先が前回の同期のあとで変わった・送り先の方が新しい（上書きしない）
    Conflict,
    /// 衝突を「両方残す」にした（送り先を別の名前にしてから送る）
    KeepBoth,
    /// 同じ（送らない）
    Same,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::New => "新規",
            Action::Update => "更新",
            Action::Touch => "日時だけ",
            Action::Delete => "削除",
            Action::Conflict => "衝突",
            Action::KeepBoth => "両方残す",
            Action::Same => "同じ",
        }
    }

    /// 実行で何かするか。
    pub fn acts(self) -> bool {
        !matches!(self, Action::Conflict | Action::Same)
    }
}

/// 計画の 1 件。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// 相対パス（送り元の書き方。送り先にだけあるものは送り先の書き方）
    pub rel: String,
    /// 送り先の相対パス（大文字・小文字の違う同じファイルがあればその書き方）
    pub dst_rel: String,
    pub action: Action,
    pub src: Option<Meta>,
    pub dst: Option<Meta>,
    /// 理由（一覧に出す）
    pub reason: String,
}

/// 計画。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Plan {
    pub src_root: PathBuf,
    pub dst_root: PathBuf,
    pub items: Vec<Item>,
}

impl Plan {
    /// 実行で送る大きさの合計と件数。
    pub fn totals(&self) -> (usize, u64) {
        let acts: Vec<&Item> = self.items.iter().filter(|i| i.action.acts()).collect();
        let bytes = acts
            .iter()
            .filter(|i| matches!(i.action, Action::New | Action::Update | Action::KeepBoth))
            .filter_map(|i| i.src.map(|m| m.size))
            .sum();
        (acts.len(), bytes)
    }

    pub fn count(&self, a: Action) -> usize {
        self.items.iter().filter(|i| i.action == a).count()
    }
}

/// 前回の同期のあとの送り先の状態（同期ジョブごとに保存する。衝突の判定と差分の送り方に使う）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    /// 送り先の相対パスの比べる形（[`crate::rel_key`]） → 大きさ・更新日時
    pub files: HashMap<String, (u64, i64)>,
    /// 送り先の相対パスの比べる形 → 送ったときの中身のブロックごとのハッシュ（大きなファイルだけ。
    /// `files` の大きさ・日時のときのもの）
    pub blocks: HashMap<String, Blocks>,
}

/// ブロックごとのハッシュ。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocks {
    /// このハッシュを求めたときの送り先の大きさ・更新日時（違えば使わない）
    pub size: u64,
    pub mtime: i64,
    pub block: u64,
    /// BLAKE3 の先頭 16 バイト
    pub hashes: Vec<[u8; 16]>,
}

/// 前の形の状態（ブロックのハッシュがない）。
#[derive(Deserialize)]
struct SyncStateV1 {
    files: HashMap<String, (u64, i64)>,
}

/// 状態のファイルの印（前の形にはない）。
const STATE_MAGIC: &[u8; 8] = b"YYFMST02";

impl SyncState {
    pub fn load(path: &Path) -> io::Result<SyncState> {
        let raw = std::fs::read(path)?;
        let bad = |e: postcard::Error| io::Error::new(io::ErrorKind::InvalidData, e);
        match raw.strip_prefix(STATE_MAGIC.as_slice()) {
            Some(body) => postcard::from_bytes(body).map_err(bad),
            None => {
                let v1: SyncStateV1 = postcard::from_bytes(&raw).map_err(bad)?;
                Ok(SyncState {
                    files: v1.files,
                    blocks: HashMap::new(),
                })
            }
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut data = STATE_MAGIC.to_vec();
        data.extend(postcard::to_allocvec(self).map_err(io::Error::other)?);
        crate::index::write_atomic(path, &data)
    }
}

/// ブロックごとのハッシュを求める（流し込みながら）。
pub struct BlockHasher {
    block: u64,
    filled: u64,
    cur: blake3::Hasher,
    out: Vec<[u8; 16]>,
}

impl BlockHasher {
    pub fn new(block: u64) -> BlockHasher {
        BlockHasher {
            block: block.max(1),
            filled: 0,
            cur: blake3::Hasher::new(),
            out: Vec::new(),
        }
    }

    pub fn feed(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            let take = ((self.block - self.filled) as usize).min(data.len());
            self.cur.update(&data[..take]);
            self.filled += take as u64;
            data = &data[take..];
            if self.filled == self.block {
                self.out.push(short(&self.cur));
                self.cur = blake3::Hasher::new();
                self.filled = 0;
            }
        }
    }

    pub fn finish(mut self) -> Blocks {
        if self.filled > 0 {
            self.out.push(short(&self.cur));
        }
        Blocks {
            size: 0,
            mtime: 0,
            block: self.block,
            hashes: self.out,
        }
    }
}

fn short(h: &blake3::Hasher) -> [u8; 16] {
    let mut o = [0u8; 16];
    o.copy_from_slice(&h.finalize().as_bytes()[..16]);
    o
}

/// ファイルのブロックごとのハッシュ。
pub fn file_blocks(fs: &dyn Fs, path: &Path, block: u64) -> io::Result<Blocks> {
    let mut r = fs.open_read(path)?;
    let mut h = BlockHasher::new(block);
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => h.feed(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(h.finish())
}

fn close(a: i64, b: i64, tol: i64) -> bool {
    (a - b).abs() <= tol
}

/// 計画を作る。`content_eq(送り元, 送り先)` は中身で比べるときに使う（大きさが同じで日時が違うもの）。
pub fn plan(
    src: &Catalog,
    dst: &Catalog,
    state: Option<&SyncState>,
    opts: &SyncOptions,
    content_eq: &mut dyn FnMut(&FileEntry, &FileEntry) -> io::Result<bool>,
) -> io::Result<Plan> {
    let tol = opts.time_tolerance;
    let mut by_key: HashMap<String, &FileEntry> = dst
        .files
        .iter()
        .map(|f| (crate::rel_key(&f.rel), f))
        .collect();
    let mut items = Vec::new();
    for s in &src.files {
        let key = crate::rel_key(&s.rel);
        let Some(d) = by_key.remove(&key) else {
            items.push(Item {
                rel: s.rel.clone(),
                dst_rel: s.rel.clone(),
                action: Action::New,
                src: Some(s.meta),
                dst: None,
                reason: "送り先にない".into(),
            });
            continue;
        };
        let (action, reason) =
            if s.meta.size == d.meta.size && close(s.meta.mtime, d.meta.mtime, tol) {
                (Action::Same, String::new())
            } else {
                let recorded = state.and_then(|st| st.files.get(&key));
                let dst_changed = recorded.is_some_and(|&(size, mtime)| {
                    size != d.meta.size || !close(mtime, d.meta.mtime, tol)
                });
                if dst_changed {
                    (
                        Action::Conflict,
                        "送り先が前回の同期のあとで変わっています".into(),
                    )
                } else if recorded.is_none() && d.meta.mtime > s.meta.mtime + tol {
                    (Action::Conflict, "送り先の方が新しい".into())
                } else if opts.compare == Compare::Content
                    && s.meta.size == d.meta.size
                    && content_eq(s, d)?
                {
                    (Action::Touch, "中身は同じで日時だけ違う".into())
                } else if s.meta.size != d.meta.size {
                    (Action::Update, "大きさが違う".into())
                } else {
                    (Action::Update, "日時が違う".into())
                }
            };
        items.push(Item {
            rel: s.rel.clone(),
            dst_rel: d.rel.clone(),
            action,
            src: Some(s.meta),
            dst: Some(d.meta),
            reason,
        });
    }
    if opts.mode == Mode::Mirror {
        let mut rest: Vec<&FileEntry> = by_key.into_values().collect();
        rest.sort_by(|a, b| a.rel.cmp(&b.rel));
        for d in rest {
            items.push(Item {
                rel: d.rel.clone(),
                dst_rel: d.rel.clone(),
                action: Action::Delete,
                src: None,
                dst: Some(d.meta),
                reason: "送り元にない".into(),
            });
        }
    }
    Ok(Plan {
        src_root: src.root.clone(),
        dst_root: dst.root.clone(),
        items,
    })
}

// ---- 実行 -------------------------------------------------------------------------------

/// 実行の 1 件の状態。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemState {
    Pending,
    /// 送りかけ（送り先の途中のファイルの、確かに書いた位置）
    Partial(u64),
    Done,
    Failed(String),
}

/// 実行の 1 件。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunItem {
    pub item: Item,
    pub state: ItemState,
}

/// 実行（ジャーナルに書く）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: u64,
    pub src_root: PathBuf,
    pub dst_root: PathBuf,
    pub mode: Mode,
    /// 削除（ミラー）で移す隔離フォルダの名前（日時。`.yyfm-trash\<この名前>`）
    pub trash_stamp: String,
    /// 隔離フォルダを作る場所（人が選ぶ。既定は送り先。`<ここ>\.yyfm-trash\<日時>\…`）
    pub trash_root: PathBuf,
    /// 「両方残す」で送り先に付ける名前の印（`名前 (衝突 <この印>).拡張子`）
    pub conflict_stamp: String,
    pub items: Vec<RunItem>,
}

impl Run {
    /// 計画の、何かする項目から実行を作る。
    pub fn new(id: u64, plan: &Plan, mode: Mode, stamp: &str) -> Run {
        Run {
            id,
            src_root: plan.src_root.clone(),
            dst_root: plan.dst_root.clone(),
            mode,
            trash_stamp: stamp.replace([' ', ':'], "-"),
            trash_root: plan.dst_root.clone(),
            conflict_stamp: stamp.to_owned(),
            items: plan
                .items
                .iter()
                .filter(|i| i.action.acts())
                .map(|i| RunItem {
                    item: i.clone(),
                    state: ItemState::Pending,
                })
                .collect(),
        }
    }

    pub fn load(path: &Path) -> io::Result<Run> {
        let raw = std::fs::read(path)?;
        let bad = |e: postcard::Error| io::Error::new(io::ErrorKind::InvalidData, e);
        if let Some(body) = raw.strip_prefix(RUN_MAGIC.as_slice()) {
            return postcard::from_bytes(body).map_err(bad);
        }
        let v1: RunV1 = postcard::from_bytes(&raw).map_err(bad)?;
        Ok(Run {
            id: v1.id,
            trash_root: v1.dst_root.clone(),
            src_root: v1.src_root,
            dst_root: v1.dst_root,
            mode: v1.mode,
            trash_stamp: v1.trash_stamp,
            conflict_stamp: v1.conflict_stamp,
            items: v1.items,
        })
    }

    /// ジャーナルに書く形。
    fn encode(&self) -> io::Result<Vec<u8>> {
        let mut data = RUN_MAGIC.to_vec();
        data.extend(postcard::to_allocvec(self).map_err(io::Error::other)?);
        Ok(data)
    }

    /// 終わったか（すべて済み・失敗）。
    pub fn finished(&self) -> bool {
        self.items
            .iter()
            .all(|i| matches!(i.state, ItemState::Done | ItemState::Failed(_)))
    }

    pub fn counts(&self) -> RunCounts {
        let mut c = RunCounts::default();
        for i in &self.items {
            match i.state {
                ItemState::Done => c.done += 1,
                ItemState::Failed(_) => c.failed += 1,
                _ => c.pending += 1,
            }
        }
        c
    }
}

/// ジャーナルの印（前の形にはない）。
const RUN_MAGIC: &[u8; 8] = b"YYFMRN02";

/// 前の形の実行（隔離フォルダの場所がない。送り先に作る）。
#[derive(Deserialize)]
struct RunV1 {
    id: u64,
    src_root: PathBuf,
    dst_root: PathBuf,
    mode: Mode,
    trash_stamp: String,
    conflict_stamp: String,
    items: Vec<RunItem>,
}

/// 実行の件数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunCounts {
    pub done: usize,
    pub failed: usize,
    pub pending: usize,
}

/// 実行の知らせ（画面・記録へ）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Started(usize),
    /// 送った量（このファイルで今回送った分の増え）
    Progress(usize, u64),
    Done(usize),
    Failed(usize, String),
    /// 回線が切れたので待ってから続ける（秒・何回目）
    Retry(usize, u64, u32),
    /// 記録に書くこと
    Log(String),
}

/// 実行の外とのやりとり。
pub struct Hooks<'a> {
    pub event: &'a (dyn Fn(Event) + Sync),
    /// 待つ（試験では待たない）
    pub sleep: &'a (dyn Fn(Duration) + Sync),
    pub cancel: &'a AtomicBool,
}

/// 再試行の間隔（秒）。
const BACKOFF: [u64; 6] = [2, 5, 10, 20, 30, 60];
/// 進みのないまま続けて失敗してよい回数。
const MAX_RETRIES: u32 = 10;
/// 確かな位置を進める間隔。
const CHECKPOINT: Duration = Duration::from_secs(1);

struct Exec<'a> {
    fs: &'a dyn Fs,
    run: Mutex<Run>,
    journal: &'a Path,
    last_save: Mutex<Instant>,
    opts: &'a SyncOptions,
    hooks: &'a Hooks<'a>,
    /// 全体を止める理由（回線が戻らない・落ちた・中止）
    abort: Mutex<Option<io::Error>>,
    /// 送り先の状態の書き換え（`None` は消した）
    state_updates: Mutex<Vec<StateUpdate>>,
    /// 始めたときの送り先の状態（差分の送り方に使う）
    prior: &'a SyncState,
}

impl Exec<'_> {
    /// 項目の状態を変える。送りかけの位置はすぐにジャーナルに書き、ほかは 1 秒に 1 回まとめて書く
    /// （済みの項目を書く前に落ちても、続けるときに送り先を見て済みと分かる）。ジャーナルは状態を
    /// 借りたまま書く（2 つのスレッドが古い内容で上書きしないように）。
    fn set_state(&self, i: usize, st: ItemState) -> io::Result<()> {
        let mut run = self.run.lock().unwrap();
        let now = matches!(st, ItemState::Partial(_));
        run.items[i].state = st;
        let mut last = self.last_save.lock().unwrap();
        if !now && last.elapsed() < CHECKPOINT {
            return Ok(());
        }
        *last = Instant::now();
        crate::index::write_atomic(self.journal, &run.encode()?)
    }

    fn save_now(&self) -> io::Result<()> {
        let run = self.run.lock().unwrap();
        *self.last_save.lock().unwrap() = Instant::now();
        crate::index::write_atomic(self.journal, &run.encode()?)
    }

    fn stopped(&self) -> bool {
        self.hooks.cancel.load(Ordering::Relaxed) || self.abort.lock().unwrap().is_some()
    }
}

/// 送り先の状態の書き換え（比べる形の相対パス・大きさと更新日時（`None` は消した）・ブロックの
/// ハッシュ（`None` は覚えない））。
type StateUpdate = (String, Option<(u64, i64)>, Option<Blocks>);

/// 実行のジャーナルの置き場所。
pub fn journal_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("{id}.run"))
}

/// 実行する（ジャーナルから続けるときも同じ）。済み・失敗の項目は飛ばす。回線が戻らない・中止した
/// ときはエラーを返す（ジャーナルは残るので、あとで続けられる）。`state` には送り先の状態を書き足す。
pub fn execute(
    fs: &dyn Fs,
    run: &mut Run,
    journal: &Path,
    opts: &SyncOptions,
    state: &mut SyncState,
    hooks: &Hooks<'_>,
) -> io::Result<RunCounts> {
    let ex = Exec {
        fs,
        run: Mutex::new(run.clone()),
        journal,
        last_save: Mutex::new(Instant::now() - CHECKPOINT),
        opts,
        hooks,
        abort: Mutex::new(None),
        state_updates: Mutex::new(Vec::new()),
        prior: &*state,
    };
    ex.save_now()?;
    let n = run.items.len();
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..opts.threads.clamp(1, 64) {
            s.spawn(|| {
                loop {
                    if ex.stopped() {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        return;
                    }
                    let st = ex.run.lock().unwrap().items[i].state.clone();
                    if matches!(st, ItemState::Done | ItemState::Failed(_)) {
                        continue;
                    }
                    if let Err(e) = do_item(&ex, i) {
                        let mut a = ex.abort.lock().unwrap();
                        if a.is_none() {
                            *a = Some(e);
                        }
                        return;
                    }
                }
            });
        }
    });
    *run = ex.run.into_inner().unwrap();
    let updates = ex.state_updates.into_inner().unwrap();
    let abort = ex.abort.into_inner().unwrap();
    for (k, v, b) in updates {
        match v {
            Some(v) => state.files.insert(k.clone(), v),
            None => state.files.remove(&k),
        };
        match b {
            Some(b) => state.blocks.insert(k, b),
            None => state.blocks.remove(&k),
        };
    }
    // 落ちたとき（試験）はジャーナルを書けない
    if !abort.as_ref().is_some_and(is_crash) {
        crate::index::write_atomic(journal, &run.encode()?)?;
    }
    if let Some(e) = abort {
        return Err(e);
    }
    if hooks.cancel.load(Ordering::Relaxed) {
        return Err(crate::cancelled());
    }
    Ok(run.counts())
}

/// 1 件を行う。回線が切れたら待って続け、戻らなければ全体を止めるエラーを返す。そのほかの失敗は
/// その項目を失敗にして `Ok`。
fn do_item(ex: &Exec<'_>, i: usize) -> io::Result<()> {
    let mut failures = 0u32;
    (ex.hooks.event)(Event::Started(i));
    loop {
        let before = ex.run.lock().unwrap().items[i].state.clone();
        match attempt(ex, i) {
            Ok(()) => {
                (ex.hooks.event)(Event::Done(i));
                return Ok(());
            }
            Err(e) if is_crash(&e) || crate::is_cancelled(&e) => return Err(e),
            Err(e) if is_transient(&e) => {
                let after = ex.run.lock().unwrap().items[i].state.clone();
                if after != before {
                    failures = 0; // 進んでいれば数え直す
                }
                failures += 1;
                if failures > MAX_RETRIES {
                    return Err(e);
                }
                let wait = BACKOFF[(failures as usize - 1).min(BACKOFF.len() - 1)];
                (ex.hooks.event)(Event::Retry(i, wait, failures));
                (ex.hooks.event)(Event::Log(format!(
                    "{}: {e}（{wait} 秒後に続けます。{failures} 回目）",
                    ex.run.lock().unwrap().items[i].item.rel
                )));
                (ex.hooks.sleep)(Duration::from_secs(wait));
                if ex.hooks.cancel.load(Ordering::Relaxed) {
                    return Err(crate::cancelled());
                }
            }
            Err(e) => {
                let msg = e.to_string();
                ex.set_state(i, ItemState::Failed(msg.clone()))?;
                (ex.hooks.event)(Event::Failed(i, msg));
                return Ok(());
            }
        }
    }
}

fn part_path(dst: &Path) -> PathBuf {
    let mut name = dst.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(PART_EXT);
    dst.with_file_name(name)
}

/// 「両方残す」で送り先に付ける名前（`名前 (衝突 <印>).拡張子`）。
pub fn conflict_name(rel: &str, stamp: &str) -> String {
    let (dir, name) = rel.rsplit_once('/').map_or(("", rel), |(d, n)| (d, n));
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    let renamed = format!("{stem} (衝突 {stamp}){ext}");
    if dir.is_empty() {
        renamed
    } else {
        format!("{dir}/{renamed}")
    }
}

fn attempt(ex: &Exec<'_>, i: usize) -> io::Result<()> {
    let (item, state, run_dst, run_src, trash, trash_root, cstamp) = {
        let run = ex.run.lock().unwrap();
        let ri = &run.items[i];
        (
            ri.item.clone(),
            ri.state.clone(),
            run.dst_root.clone(),
            run.src_root.clone(),
            run.trash_stamp.clone(),
            run.trash_root.clone(),
            run.conflict_stamp.clone(),
        )
    };
    let fs = ex.fs;
    let dst = crate::join(&run_dst, &item.dst_rel);
    match item.action {
        Action::Touch => {
            let m = item.src.unwrap_or_default();
            fs.set_mtime(&dst, m.mtime)?;
            // 中身は同じなのでブロックのハッシュはそのまま使える
            let key = crate::rel_key(&item.dst_rel);
            let blocks = ex.prior.blocks.get(&key).cloned();
            record(ex, &item, &dst, blocks)?;
            ex.set_state(i, ItemState::Done)
        }
        Action::Delete => {
            // 隔離フォルダへ移す（すぐには消さない）。送り先のほかの場所を選んだときは、送り先の
            // ルートの名前のフォルダに分ける
            let rel = if trash_root == run_dst {
                item.dst_rel.clone()
            } else {
                format!("{}/{}", crate::purge::root_label(&run_dst), item.dst_rel)
            };
            let to = crate::purge::trash_path(&trash_root, &trash, &rel);
            match fs.metadata(&dst) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
                Ok(_) => crate::purge::move_to(fs, &dst, &to, true)?,
            }
            ex.state_updates
                .lock()
                .unwrap()
                .push((crate::rel_key(&item.dst_rel), None, None));
            ex.set_state(i, ItemState::Done)
        }
        Action::New | Action::Update | Action::KeepBoth => {
            if item.action == Action::KeepBoth && matches!(state, ItemState::Pending) {
                let keep = crate::join(&run_dst, &conflict_name(&item.dst_rel, &cstamp));
                if fs.metadata(&dst).is_ok() && fs.metadata(&keep).is_err() {
                    fs.rename_new(&dst, &keep)?;
                }
            }
            let src = crate::join(&run_src, &item.rel);
            copy_file(ex, i, &item, &state, &src, &dst)
        }
        Action::Conflict | Action::Same => ex.set_state(i, ItemState::Done),
    }
}

/// 送り先の状態を前回の同期の記録に書く。
fn record(ex: &Exec<'_>, item: &Item, dst: &Path, blocks: Option<Blocks>) -> io::Result<()> {
    let m = ex.fs.metadata(dst)?;
    let blocks = blocks.map(|b| Blocks {
        size: m.size,
        mtime: m.mtime,
        ..b
    });
    ex.state_updates.lock().unwrap().push((
        crate::rel_key(&item.dst_rel),
        Some((m.size, m.mtime)),
        blocks,
    ));
    Ok(())
}

/// 末尾の `len` バイトが同じか。
fn tails_match(fs: &dyn Fs, a: &Path, b: &Path, end: u64, len: u64) -> io::Result<bool> {
    let read = |p: &Path| -> io::Result<Vec<u8>> {
        let mut f = fs.open_read(p)?;
        f.seek(SeekFrom::Start(end - len))?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    };
    Ok(read(a)? == read(b)?)
}

fn copy_file(
    ex: &Exec<'_>,
    i: usize,
    item: &Item,
    state: &ItemState,
    src: &Path,
    dst: &Path,
) -> io::Result<()> {
    let fs = ex.fs;
    let planned = item.src.unwrap_or_default();
    let now = fs.metadata(src)?;
    let part = part_path(dst);
    // 前回、置き換えたあとで記録する前に落ちたとき: 送り先が送り元と同じで途中のファイルがなければ済み
    if matches!(state, ItemState::Pending | ItemState::Partial(_))
        && fs.metadata(&part).is_err()
        && let Ok(d) = fs.metadata(dst)
        && d.size == now.size
        && close(d.mtime, now.mtime, ex.opts.time_tolerance)
        && (item.action == Action::New || item.dst.is_none_or(|old| old != d))
    {
        let blocks = if ex.opts.delta_min > 0 && now.size >= ex.opts.delta_min {
            Some(file_blocks(fs, src, ex.opts.delta_block)?)
        } else {
            None
        };
        record(ex, item, dst, blocks)?;
        (ex.hooks.event)(Event::Log(format!(
            "{}: 送り終えていたので済みにしました",
            item.rel
        )));
        return ex.set_state(i, ItemState::Done);
    }
    if let Some(p) = dst.parent() {
        fs.create_dir_all(p)?;
    }
    let delta = ex.opts.delta_min > 0 && now.size >= ex.opts.delta_min;
    if delta
        && item.action == Action::Update
        && matches!(state, ItemState::Pending)
        && now.size == planned.size
        && now.mtime == planned.mtime
        && let Some(done) = try_delta(ex, i, item, src, dst, &part, &now)?
    {
        return finish(ex, i, item, src, dst, &part, &now, Some(done));
    }
    // 続ける位置
    let mut offset = match state {
        ItemState::Partial(o) => {
            let have = fs.metadata(&part).map(|m| m.size).unwrap_or(0);
            (*o).min(have)
        }
        _ => 0,
    };
    // 送り元が始めたときと変わっていたら最初から
    if now.size != planned.size || now.mtime != planned.mtime {
        if offset > 0 {
            (ex.hooks.event)(Event::Log(format!(
                "{}: 送り元が変わったので最初から送ります",
                item.rel
            )));
        }
        offset = 0;
        ex.run.lock().unwrap().items[i].item.src = Some(now);
    }
    if offset > now.size {
        offset = 0;
    }
    if offset > 0 {
        let len = offset.min(crate::hash::EDGE);
        if !tails_match(fs, src, &part, offset, len)? {
            (ex.hooks.event)(Event::Log(format!(
                "{}: 途中のファイルが送り元と合わないので最初から送ります",
                item.rel
            )));
            offset = 0;
        } else {
            (ex.hooks.event)(Event::Log(format!(
                "{}: {} バイト目から続けます",
                item.rel, offset
            )));
        }
    }
    let mut r = fs.open_read(src)?;
    r.seek(SeekFrom::Start(offset))?;
    let mut w = fs.open_write(&part, offset)?;
    // 大きなファイルは、次の差分の送り方のためにブロックのハッシュを求めながら送る
    let mut hasher = (delta && offset == 0).then(|| BlockHasher::new(ex.opts.delta_block));
    let mut buf = vec![0u8; 1 << 20];
    let mut pos = offset;
    let mut last = Instant::now();
    let mut unsynced = 0u64;
    loop {
        if ex.stopped() {
            // 書いた分を確かにしてから止める
            w.sync()?;
            ex.set_state(i, ItemState::Partial(pos))?;
            return Err(crate::cancelled());
        }
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        w.write_all(&buf[..n])?;
        if let Some(h) = &mut hasher {
            h.feed(&buf[..n]);
        }
        pos += n as u64;
        unsynced += n as u64;
        (ex.hooks.event)(Event::Progress(i, n as u64));
        if last.elapsed() >= CHECKPOINT || unsynced >= ex.opts.checkpoint_bytes {
            w.sync()?;
            ex.set_state(i, ItemState::Partial(pos))?;
            last = Instant::now();
            unsynced = 0;
        }
    }
    w.sync()?;
    drop(w);
    ex.set_state(i, ItemState::Partial(pos))?;
    let blocks = match hasher {
        Some(h) => Some(h.finish()),
        // 続きから送ったときは送り元から求め直す（手元を読むだけ）
        None if delta => Some(file_blocks(fs, src, ex.opts.delta_block)?),
        None => None,
    };
    finish(ex, i, item, src, dst, &part, &now, blocks)
}

/// 送り終えた途中のファイルを確かめて、本来の名前に置き換える。
#[allow(clippy::too_many_arguments)]
fn finish(
    ex: &Exec<'_>,
    i: usize,
    item: &Item,
    src: &Path,
    dst: &Path,
    part: &Path,
    now: &Meta,
    blocks: Option<Blocks>,
) -> io::Result<()> {
    let fs = ex.fs;
    let got = fs.metadata(part)?;
    if got.size != now.size {
        fs.remove_file(part)?;
        ex.set_state(i, ItemState::Pending)?;
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "送った大きさが違います（{} / {}）。最初から送り直してください",
                got.size, now.size
            ),
        ));
    }
    if ex.opts.verify_hash {
        let a = crate::hash::full(fs, src, &mut |_| true)?;
        let b = crate::hash::full(fs, part, &mut |_| true)?;
        if a != b {
            fs.remove_file(part)?;
            ex.set_state(i, ItemState::Pending)?;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "送った中身が送り元と違います",
            ));
        }
    }
    fs.set_mtime(part, now.mtime)?;
    // 読み取り専用の送り先は置き換えられないので外す
    if let Ok(d) = fs.metadata(dst)
        && d.readonly
    {
        fs.set_readonly(dst, false)?;
    }
    fs.rename_replace(part, dst)?;
    if now.readonly {
        fs.set_readonly(dst, true)?;
    }
    record(ex, item, dst, blocks)?;
    ex.set_state(i, ItemState::Done)
}

/// 差分の送り方（18 章 4.3）。送り先が前回送ったときのままで、そのときのブロックのハッシュがあれば、
/// 送り先を途中のファイルにサーバー側でコピーし、ハッシュの違うブロックだけを書く。できないときは
/// `None`（ファイル全体を送る）。
fn try_delta(
    ex: &Exec<'_>,
    i: usize,
    item: &Item,
    src: &Path,
    dst: &Path,
    part: &Path,
    now: &Meta,
) -> io::Result<Option<Blocks>> {
    let fs = ex.fs;
    let key = crate::rel_key(&item.dst_rel);
    let block = ex.opts.delta_block.max(1);
    let Some(old) = ex.prior.blocks.get(&key).filter(|b| b.block == block) else {
        return Ok(None);
    };
    let Some(&(size, mtime)) = ex.prior.files.get(&key) else {
        return Ok(None);
    };
    let d = fs.metadata(dst)?;
    if d.size != size
        || d.mtime != mtime
        || old.size != size
        || old.mtime != mtime
        || old.hashes.len() as u64 != size.div_ceil(block)
    {
        return Ok(None);
    }
    let log = |m: String| (ex.hooks.event)(Event::Log(format!("{}: {m}", item.rel)));
    if let Err(e) = fs.copy_file(dst, part) {
        if is_transient(&e) || is_crash(&e) {
            return Err(e);
        }
        log(format!(
            "サーバー側のコピーができないので全体を送ります（{e}）"
        ));
        let _ = fs.remove_file(part);
        return Ok(None);
    }
    let mut r = fs.open_read(src)?;
    let mut w = fs.open_patch(part)?;
    let mut buf = vec![0u8; block as usize];
    let mut out = Vec::with_capacity(now.size.div_ceil(block) as usize);
    let (mut written, mut changed) = (0u64, 0usize);
    let mut pos = 0u64;
    while pos < now.size {
        if ex.stopped() {
            return Err(crate::cancelled());
        }
        let len = (now.size - pos).min(block) as usize;
        r.read_exact(&mut buf[..len])?;
        let mut h = blake3::Hasher::new();
        h.update(&buf[..len]);
        let hash = short(&h);
        let k = out.len();
        if old.hashes.get(k) != Some(&hash) || pos + len as u64 > size {
            w.seek(SeekFrom::Start(pos))?;
            w.write_all(&buf[..len])?;
            written += len as u64;
            changed += 1;
            (ex.hooks.event)(Event::Progress(i, len as u64));
        }
        out.push(hash);
        pos += len as u64;
    }
    w.set_len(now.size)?;
    w.sync()?;
    drop(w);
    log(format!(
        "差分の送り方: {} ブロック中 {changed} ブロック（{}）を書きました",
        out.len(),
        crate::human_size(written)
    ));
    Ok(Some(Blocks {
        size: 0,
        mtime: 0,
        block,
        hashes: out,
    }))
}

/// 日時の印（UTC。`2026-10-08 1530`）。
pub fn stamp(ns: i64) -> String {
    let secs = ns.div_euclid(1_000_000_000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    // 日数から年月日（Howard Hinnant の方法）
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}{:02}",
        sod / 3600,
        (sod % 3600) / 60
    )
}

#[cfg(test)]
mod tests;

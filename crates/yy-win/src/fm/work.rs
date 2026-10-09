//! 作業スレッド（走査・ハッシュ・同期・削除など）と、画面への知らせ。画面なしの同期（`--sync`）も
//! ここ。

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use yy_config::Config;
use yy_files::dupes::{FileRef, Group};
use yy_files::jobs::{Dirs, JobList};
use yy_files::purge::PurgeReport;
use yy_files::scan::{Catalog, ScanOptions};
use yy_files::similar::VersionGroup;
use yy_files::sync::{Event, Plan, Run, RunCounts};

use super::WM_APP_FM;

/// 検索で見つかった 1 件（ファイル・行の番号・行）。
pub(super) type SearchHit = (FileRef, u64, String);

/// 作業スレッドから画面への知らせ。
pub(super) enum Msg {
    /// ステータスバーに出す進み
    Progress(String),
    /// 記録に書く
    Log(String),
    Planned(Result<Plan, String>),
    SyncEvent(Event),
    /// 実行・ジャーナルのパス・結果
    SyncFinished(Run, PathBuf, Result<RunCounts, String>),
    /// 走査し直す前に、前回の目録で見つかったもの（名前・属性だけの検索）
    SearchPartial(Vec<Catalog>, Vec<SearchHit>),
    /// 走査しながら見つかったもの（何番目の場所・そのルート・ファイル。名前・属性だけの検索）
    SearchFound(usize, PathBuf, Vec<yy_files::FileEntry>),
    /// 目録と、見つかったファイル（行の番号・行。名前だけの検索なら 0 と空）
    Searched(Result<(Vec<Catalog>, Vec<SearchHit>), String>),
    Similar(Result<(Vec<Catalog>, Vec<VersionGroup>), String>),
    Dupes(Result<(Vec<Catalog>, Vec<Group>), String>),
    /// 結果と、削除の記録（CSV）のパス
    Purged(Result<PurgeReport, String>, PathBuf),
    /// 処理が終わった（ステータスバーと記録に出す）
    Done(String),
    /// 作業スレッドが異常終了した
    Panicked(String),
}

/// 画面へ知らせる（溜まっている間は PostMessage を 1 回だけにする）。
pub(super) struct Notify {
    frame: isize,
    pending: AtomicBool,
    tx: mpsc::Sender<Msg>,
    /// 最後に進みを送った時刻（多すぎないように間引く）
    last_progress: Mutex<Option<Instant>>,
}

impl Notify {
    pub(super) fn new(frame: HWND, tx: mpsc::Sender<Msg>) -> Notify {
        Notify {
            frame: frame.0 as isize,
            pending: AtomicBool::new(false),
            tx,
            last_progress: Mutex::new(None),
        }
    }

    fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
        if !self.pending.swap(true, Ordering::AcqRel) {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.frame as *mut _)),
                    WM_APP_FM,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }

    /// 画面の側: 知らせを 1 つ受け取る。
    pub(super) fn take_pending(&self, rx: &mpsc::Receiver<Msg>) -> Option<Msg> {
        self.pending.store(false, Ordering::Release);
        rx.try_recv().ok()
    }
}

/// 処理に渡すもの（知らせる・中止を見る）。
pub(super) struct Ctx {
    pub(super) notify: Arc<Notify>,
    pub(super) cancel: Arc<AtomicBool>,
}

impl Ctx {
    /// 進みを出す（0.1 秒に 1 回まで）。中止されていなければ `true`。
    pub(super) fn progress(&self, s: &str) -> bool {
        let now = Instant::now();
        let due = {
            let mut last = self
                .notify
                .last_progress
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let due = last.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(100));
            if due {
                *last = Some(now);
            }
            due
        };
        if due {
            self.notify.send(Msg::Progress(s.to_owned()));
        }
        !self.cancelled()
    }

    pub(super) fn send(&self, m: Msg) {
        self.notify.send(m);
    }

    pub(super) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// 作業スレッドで行う処理。
pub(super) struct Task {
    pub(super) ctx: Ctx,
    pub(super) run: Box<dyn FnOnce(&Ctx) + Send>,
}

/// 処理を新しいスレッドで始める。
pub(super) fn spawn(task: Task) {
    let r = std::thread::Builder::new()
        .name("yyfilemanager-work".into())
        .spawn(move || {
            let Task { ctx, run } = task;
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&ctx)));
            if let Err(p) = r {
                let s = p
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                ctx.send(Msg::Panicked(s));
            }
        });
    if let Err(e) = r {
        // スレッドを作れない: 画面に知らせる（送り手はまだ持っていない）
        append_log_file(&format!("作業スレッドを作れません: {e}\r\n"));
    }
}

/// エラーの説明。
pub(super) fn describe(e: &std::io::Error) -> String {
    if yy_files::is_cancelled(e) {
        return "中止しました".into();
    }
    match e.kind() {
        std::io::ErrorKind::NotFound => format!("見つかりません（{e}）"),
        std::io::ErrorKind::PermissionDenied => format!("アクセスが拒否されました（{e}）"),
        _ => e.to_string(),
    }
}

/// 設定から走査の設定を作る。
pub(super) fn scan_options(config: &Config) -> ScanOptions {
    let mut o = ScanOptions {
        threads: config.filemanager.scan_threads.max(1),
        ..ScanOptions::default()
    };
    if !config.filemanager.exclude.is_empty() {
        let mut pats: Vec<String> = o.exclude_files.items().to_vec();
        pats.extend(config.filemanager.exclude.iter().cloned());
        o.exclude_files = yy_files::pattern::Patterns::new(&pats);
    }
    o
}

/// 保存した目録の使い方（[`yy_files::catalogs`]）。
#[derive(Clone)]
pub(super) struct CatalogCache {
    pub(super) dir: PathBuf,
    /// これより新しい目録を使う（ナノ秒）
    pub(super) max_age: i64,
    pub(super) now: i64,
    /// 必ず走査する（結果は保存する）
    pub(super) force: bool,
}

/// 走査しながら見つかったファイルを渡す口（何番目の場所か・ファイル）。
pub(super) type FoundIn<'a> = dyn Fn(usize, &[yy_files::FileEntry]) + Sync + 'a;

/// いくつかの場所を走査する（`cache` があれば、新しい保存した目録を使い、走査した目録は保存する）。
/// `found` があれば、走査し直す場所で見つかったファイルを順に渡す（何番目の場所か・ファイル）。
pub(super) fn scan_all(
    cx: &Ctx,
    roots: &[PathBuf],
    scan: &ScanOptions,
    cache: Option<&CatalogCache>,
    found: Option<&FoundIn<'_>>,
) -> Result<Vec<Catalog>, String> {
    let mut cats = Vec::new();
    for (ri, r) in roots.iter().enumerate() {
        let each = |fs: &[yy_files::FileEntry]| {
            if let Some(f) = found {
                f(ri, fs);
            }
        };
        let each: Option<&yy_files::scan::Found<'_>> = found.map(|_| &each as _);
        let progress = |p: &yy_files::scan::ScanProgress| {
            cx.progress(&format!("{} を走査しています… {} 個", r.display(), p.files))
        };
        let c = match cache {
            Some(k) => yy_files::catalogs::scan_cached(
                &yy_files::Local,
                r,
                scan,
                &k.dir,
                k.max_age,
                k.now,
                k.force,
                &progress,
                each,
            )
            .map(|(c, used)| {
                if let Some(t) = used {
                    cx.send(Msg::Log(format!(
                        "{}: {} 分前の目録を使います（走査し直すには「走査し直す」をチェック）",
                        r.display(),
                        (k.now - t) / 60_000_000_000
                    )));
                }
                c
            }),
            None => yy_files::scan_with(&yy_files::Local, r, scan, &progress, each),
        }
        .map_err(|e| format!("{}: {}", r.display(), describe(&e)))?;
        if !c.errors.is_empty() {
            cx.send(Msg::Log(format!(
                "{}: 読めなかったところが {} か所あります（最初: {}）",
                r.display(),
                c.errors.len(),
                c.errors[0].1
            )));
        }
        cats.push(c);
    }
    Ok(cats)
}

/// 重複を探す（ハッシュは索引から使い、求めたものは索引に残す）。
pub(super) fn find_dupes(
    cx: &Ctx,
    cats: &[Catalog],
    index_dir: &std::path::Path,
    threads: usize,
) -> std::io::Result<Vec<Group>> {
    let paths: Vec<PathBuf> = cats
        .iter()
        .map(|c| yy_files::index::path_for(index_dir, &c.root))
        .collect();
    let mut indexes: Vec<yy_files::index::Index> = cats
        .iter()
        .zip(&paths)
        .map(|(c, p)| {
            yy_files::index::Index::load(p).unwrap_or_else(|_| yy_files::index::Index::new(&c.root))
        })
        .collect();
    let mut refs: Vec<Option<&mut yy_files::index::Index>> = indexes.iter_mut().map(Some).collect();
    let opts = yy_files::dupes::DupeOptions {
        threads,
        ..yy_files::dupes::DupeOptions::default()
    };
    let r = yy_files::dupes::find(&yy_files::Local, cats, &mut refs, &opts, &|p| {
        cx.progress(&format!(
            "重複を探しています（段階 {}/3）… {} 個・{}",
            p.stage,
            p.files,
            yy_files::human_size(p.bytes)
        ))
    });
    for ((ix, p), c) in indexes.iter_mut().zip(&paths).zip(cats) {
        let keep: std::collections::HashSet<&str> =
            c.files.iter().map(|f| f.rel.as_str()).collect();
        ix.retain(&|r| keep.contains(r));
        let _ = ix.save(p);
    }
    r
}

/// 記録のファイル（設定のフォルダの `logs\filemanager.log`）。
pub(super) fn log_path() -> Option<PathBuf> {
    Some(
        yy_config::config_dir()?
            .join("logs")
            .join("filemanager.log"),
    )
}

/// 記録のファイルに書き足す（書けなくても止めない）。
pub(super) fn append_log_file(line: &str) {
    let Some(p) = log_path() else {
        return;
    };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

fn log(s: &str) {
    append_log_file(&format!("{} {s}\r\n", crate::remote::local_clock()));
}

/// 置き場所（設定のフォルダの `filemanager\`）。
pub(super) fn dirs() -> Dirs {
    Dirs::new(
        yy_config::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("filemanager"),
    )
}

/// 画面を出さずに同期ジョブを実行する（タスク スケジューラーから。18 章 4.6）。終了コードを返す
/// （0: 済んだ、1: 失敗があった、2: ジョブがない・設定を読めない）。
pub(super) fn headless_sync(name: &str) -> i32 {
    let (config, err) = Config::load();
    if let Some(e) = err {
        log(&format!(
            "設定を読めませんでした（既定の設定で続けます）: {e}"
        ));
    }
    let dirs = dirs();
    let jobs = match JobList::load(&dirs.jobs_file()) {
        Ok(j) => j,
        Err(e) => {
            log(&format!("同期ジョブを読めません: {e}"));
            return 2;
        }
    };
    let Some(job) = jobs.get(name) else {
        log(&format!("同期ジョブ「{name}」がありません"));
        return 2;
    };
    log(&format!(
        "同期ジョブ「{name}」を始めます（{} → {}）",
        job.src.display(),
        job.dst.display()
    ));
    let stamp = super::local_stamp();
    let r = yy_files::jobs::run_job(
        &yy_files::Local,
        job,
        &dirs,
        &scan_options(&config),
        &stamp,
        &|e| match e {
            Event::Failed(i, m) => log(&format!("{} 番目の項目: 失敗: {m}", i + 1)),
            Event::Log(s) => log(&s),
            _ => {}
        },
    );
    match r {
        Ok(rep) => {
            log(&format!(
                "同期ジョブ「{name}」が終わりました: 済み {}・失敗 {}・衝突（送らなかった）{}",
                rep.counts.done, rep.counts.failed, rep.conflicts
            ));
            i32::from(rep.counts.failed > 0)
        }
        Err(e) => {
            log(&format!(
                "同期ジョブ「{name}」を中断しました: {}",
                describe(&e)
            ));
            1
        }
    }
}

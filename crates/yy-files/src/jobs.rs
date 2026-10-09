//! 同期ジョブの保存と、置き場所・実行のまとめ（18 章 4.1・4.5・4.6）。

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::fs::Fs;
use crate::scan::{ScanOptions, scan};
use crate::sync::{self, Action, Hooks, Plan, Run, RunCounts, SyncOptions, SyncState};

/// 保存する同期ジョブ。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyncJob {
    pub name: String,
    /// 送り元（手元のフォルダ）
    pub src: PathBuf,
    /// 送り先（共有フォルダ）
    pub dst: PathBuf,
    #[serde(default)]
    pub options: SyncOptions,
}

/// ジョブの一覧（`jobs.toml`）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JobList {
    #[serde(default, rename = "job")]
    pub jobs: Vec<SyncJob>,
}

impl JobList {
    pub fn load(path: &Path) -> io::Result<JobList> {
        match std::fs::read_to_string(path) {
            Ok(t) => toml::from_str(&t).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(JobList::default()),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let t = toml::to_string_pretty(self).map_err(io::Error::other)?;
        crate::index::write_atomic(path, t.as_bytes())
    }

    pub fn get(&self, name: &str) -> Option<&SyncJob> {
        self.jobs.iter().find(|j| j.name == name)
    }

    /// 同じ名前があれば置き換え、なければ足す。
    pub fn put(&mut self, job: SyncJob) {
        match self.jobs.iter_mut().find(|j| j.name == job.name) {
            Some(j) => *j = job,
            None => self.jobs.push(job),
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.jobs.retain(|j| j.name != name);
    }
}

/// 保存した検索（18 章 8.1）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSearch {
    pub name: String,
    /// 1 行の検索欄の書き方
    pub query: String,
    /// 探す場所
    pub roots: Vec<PathBuf>,
    /// Office の文書の中も探す
    #[serde(default = "yes")]
    pub office: bool,
}

fn yes() -> bool {
    true
}

/// 保存した検索の一覧（`searches.toml`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchList {
    #[serde(default, rename = "search")]
    pub searches: Vec<SavedSearch>,
}

impl SearchList {
    pub fn load(path: &Path) -> io::Result<SearchList> {
        match std::fs::read_to_string(path) {
            Ok(t) => toml::from_str(&t).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(SearchList::default()),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let t = toml::to_string_pretty(self).map_err(io::Error::other)?;
        crate::index::write_atomic(path, t.as_bytes())
    }

    pub fn get(&self, name: &str) -> Option<&SavedSearch> {
        self.searches.iter().find(|j| j.name == name)
    }

    /// 同じ名前があれば置き換え、なければ足す。
    pub fn put(&mut self, s: SavedSearch) {
        match self.searches.iter_mut().find(|j| j.name == s.name) {
            Some(j) => *j = s,
            None => self.searches.push(s),
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.searches.retain(|j| j.name != name);
    }
}

/// 置き場所（設定のフォルダの `filemanager\`）。
#[derive(Clone, Debug)]
pub struct Dirs {
    pub root: PathBuf,
}

impl Dirs {
    pub fn new(root: PathBuf) -> Dirs {
        Dirs { root }
    }
    pub fn jobs_file(&self) -> PathBuf {
        self.root.join("jobs.toml")
    }
    pub fn searches_file(&self) -> PathBuf {
        self.root.join("searches.toml")
    }
    /// 保存した目録（[`crate::catalogs`]）。
    pub fn catalogs(&self) -> PathBuf {
        self.root.join("catalogs")
    }
    pub fn runs(&self) -> PathBuf {
        self.root.join("runs")
    }
    pub fn index(&self) -> PathBuf {
        self.root.join("index")
    }
    pub fn reviews(&self) -> PathBuf {
        self.root.join("reviews")
    }
    pub fn purges(&self) -> PathBuf {
        self.root.join("purges")
    }
    /// 同期ジョブの前回の状態（送り先ごと）。
    pub fn state_file(&self, dst: &Path) -> PathBuf {
        let p = crate::index::path_for(&self.root.join("states"), dst);
        p.with_extension("state")
    }
    /// 送り先ごとの実行の記録（どの実行がどの送り先か）を含めた、次の実行の番号。
    pub fn next_run_id(&self) -> u64 {
        std::fs::read_dir(self.runs())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_str()?
                    .strip_suffix(".run")?
                    .parse::<u64>()
                    .ok()
            })
            .max()
            .map_or(1, |m| m + 1)
    }
    /// 終わっていない実行（番号の順）。
    pub fn unfinished_runs(&self) -> Vec<(PathBuf, Run)> {
        let mut out: Vec<(PathBuf, Run)> = std::fs::read_dir(self.runs())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".run"))
            .filter_map(|e| {
                let p = e.path();
                let r = Run::load(&p).ok()?;
                (!r.finished()).then_some((p, r))
            })
            .collect();
        out.sort_by_key(|(_, r)| r.id);
        out
    }
}

/// 比べる（両側を走査して計画を作る）。送り先がなければ作る。
pub fn compare(
    fs: &dyn Fs,
    src: &Path,
    dst: &Path,
    opts: &SyncOptions,
    scan_opts: &ScanOptions,
    state: Option<&SyncState>,
    progress: &(dyn Fn(&crate::scan::ScanProgress) -> bool + Sync),
) -> io::Result<Plan> {
    let s = scan(fs, src, scan_opts, progress)?;
    if fs.metadata(dst).is_err() {
        fs.create_dir_all(dst)?;
    }
    let d = scan(fs, dst, scan_opts, progress)?;
    sync::plan(&s, &d, state, opts, &mut |a, b| {
        let ha = crate::hash::full(fs, &s.path(a), &mut |_| true)?;
        let hb = crate::hash::full(fs, &d.path(b), &mut |_| true)?;
        Ok(ha == hb)
    })
}

/// 同期の結果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobReport {
    pub counts: RunCounts,
    pub conflicts: usize,
    pub run_id: u64,
}

/// ジョブを画面なしで実行する（比べる → 衝突は送らない → 実行 → 状態を保存）。コマンドラインの
/// 定期実行（18 章 4.6）に使う。
pub fn run_job(
    fs: &dyn Fs,
    job: &SyncJob,
    dirs: &Dirs,
    scan_opts: &ScanOptions,
    stamp: &str,
    event: &(dyn Fn(sync::Event) + Sync),
) -> io::Result<JobReport> {
    let state_path = dirs.state_file(&job.dst);
    let mut state = SyncState::load(&state_path).unwrap_or_default();
    let plan = compare(
        fs,
        &job.src,
        &job.dst,
        &job.options,
        scan_opts,
        Some(&state),
        &|_| true,
    )?;
    let conflicts = plan.count(Action::Conflict);
    let id = dirs.next_run_id();
    let mut run = Run::new(id, &plan, job.options.mode, stamp);
    let journal = sync::journal_path(&dirs.runs(), id);
    let cancel = AtomicBool::new(false);
    let r = sync::execute(
        fs,
        &mut run,
        &journal,
        &job.options,
        &mut state,
        &Hooks {
            event,
            sleep: &|d| std::thread::sleep(d),
            cancel: &cancel,
        },
    );
    state.save(&state_path)?;
    let counts = r?;
    if run.finished() && counts.failed == 0 {
        let _ = std::fs::remove_file(&journal);
    }
    Ok(JobReport {
        counts,
        conflicts,
        run_id: id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::Local;

    #[test]
    fn saves_jobs_and_runs_headless() {
        let d = tempfile::tempdir().unwrap();
        let dirs = Dirs::new(d.path().join("fm"));
        let src = d.path().join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), "a").unwrap();
        std::fs::write(src.join("sub/b.txt"), "b").unwrap();
        let job = SyncJob {
            name: "案件A".into(),
            src: src.clone(),
            dst: d.path().join("share/案件A"),
            options: SyncOptions::default(),
        };
        let mut list = JobList::default();
        list.put(job.clone());
        list.save(&dirs.jobs_file()).unwrap();
        let back = JobList::load(&dirs.jobs_file()).unwrap();
        assert_eq!(back, list);
        assert!(
            JobList::load(&d.path().join("none.toml"))
                .unwrap()
                .jobs
                .is_empty()
        );
        let rep = run_job(
            &Local,
            back.get("案件A").unwrap(),
            &dirs,
            &ScanOptions::default(),
            "s",
            &|_| {},
        )
        .unwrap();
        assert_eq!(rep.counts.done, 2);
        assert_eq!(
            std::fs::read_to_string(d.path().join("share/案件A/sub/b.txt")).unwrap(),
            "b"
        );
        // 済んだ実行のジャーナルは消す。状態は残る
        assert!(dirs.unfinished_runs().is_empty());
        assert!(dirs.state_file(&job.dst).exists());
        // 2 回目は何もしない
        let rep = run_job(&Local, &job, &dirs, &ScanOptions::default(), "s", &|_| {}).unwrap();
        assert_eq!(rep.counts.done, 0);
        // 送り先で変わったファイルは衝突（送らない）
        std::fs::write(d.path().join("share/案件A/a.txt"), "theirs").unwrap();
        Local
            .set_mtime(
                &d.path().join("share/案件A/a.txt"),
                crate::fs::to_nanos(std::time::SystemTime::now()) + 60_000_000_000,
            )
            .unwrap();
        std::fs::write(src.join("a.txt"), "mine!").unwrap();
        let rep = run_job(&Local, &job, &dirs, &ScanOptions::default(), "s", &|_| {}).unwrap();
        assert_eq!(rep.conflicts, 1);
        assert_eq!(
            std::fs::read_to_string(d.path().join("share/案件A/a.txt")).unwrap(),
            "theirs"
        );
        let mut ss = SearchList::default();
        ss.put(SavedSearch {
            name: "見積".into(),
            query: "見積 ext:xlsx".into(),
            roots: vec![src.clone()],
            office: false,
        });
        ss.save(&dirs.searches_file()).unwrap();
        assert_eq!(SearchList::load(&dirs.searches_file()).unwrap(), ss);
        assert!(ss.get("見積").is_some());
        ss.remove("見積");
        assert!(ss.searches.is_empty());
        list.remove("案件A");
        assert!(list.jobs.is_empty());
        assert!(dirs.next_run_id() >= 1);
    }
}

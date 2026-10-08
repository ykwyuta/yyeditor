//! 確かめてからの一括削除（18 章 7）。
//!
//! 削除の候補（古い版・重複の写し）を一覧にし、人がチェックしたものだけを消す。消す直前に、走査した
//! ときと変わっていないこと（大きさ・更新日時。重複は残すファイルと中身が同じこと）を確かめ、グループの
//! 最後の 1 つは消さない。手元のファイルはごみ箱へ（呼び出し側が行う）、共有フォルダのファイルは同じ共有の
//! 隔離フォルダ（`.yyfm-trash\<日時>\<元の相対パス>`）へ名前の変更で移す。消した記録（CSV）から元に戻せる。

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fs::{Fs, Meta};
use crate::hash::Hash;

/// 共有フォルダの隔離フォルダの名前。
pub const TRASH_DIR: &str = ".yyfm-trash";

/// 削除の候補。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// 目録のルート（隔離フォルダはこの下に作る）
    pub root: PathBuf,
    /// ルートからの相対パス
    pub rel: String,
    /// 走査したときの情報
    pub meta: Meta,
    /// グループの番号（同じグループの最後の 1 つは消さない）
    pub group: usize,
    /// 残すもの（チェックできない）
    pub keep: bool,
    pub checked: bool,
    /// 理由（「古い版」「重複の写し」）
    pub reason: String,
    /// 判定の自信（`高`・`中`・`低`）
    pub confidence: String,
    /// 重複の写しなら、中身のハッシュと残すファイル（消す前に同じことを確かめる）
    pub dupe_of: Option<(Hash, PathBuf)>,
}

impl Candidate {
    pub fn path(&self) -> PathBuf {
        crate::join(&self.root, &self.rel)
    }
}

/// 確かめの一覧（保存して、あとで続けられる）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub items: Vec<Candidate>,
}

impl Review {
    pub fn load(path: &Path) -> io::Result<Review> {
        let raw = std::fs::read(path)?;
        postcard::from_bytes(&raw).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        crate::index::write_atomic(
            path,
            &postcard::to_allocvec(self).map_err(io::Error::other)?,
        )
    }

    /// チェックしたものの数と大きさ。
    pub fn checked_totals(&self) -> (usize, u64) {
        let c: Vec<&Candidate> = self.items.iter().filter(|c| c.checked && !c.keep).collect();
        (c.len(), c.iter().map(|c| c.meta.size).sum())
    }

    /// 全部にチェックが付いたグループ（消すと何も残らない）。
    pub fn emptied_groups(&self) -> Vec<usize> {
        let mut groups: std::collections::BTreeMap<usize, bool> = std::collections::BTreeMap::new();
        for c in &self.items {
            let remains = groups.entry(c.group).or_insert(false);
            if c.keep || !c.checked {
                *remains = true;
            }
        }
        groups
            .into_iter()
            .filter(|&(_, remains)| !remains)
            .map(|(g, _)| g)
            .collect()
    }
}

/// 消し方。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Disposal {
    /// ごみ箱へ（手元のドライブ。呼び出し側が行う）
    RecycleBin,
    /// 隔離フォルダへ（共有フォルダ）
    Trash,
    /// すぐに消す
    Delete,
}

impl Disposal {
    fn name(self) -> &'static str {
        match self {
            Disposal::RecycleBin => "recycle",
            Disposal::Trash => "trash",
            Disposal::Delete => "delete",
        }
    }
}

/// 1 件の結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Removed(Disposal, Option<PathBuf>),
    /// 走査したあとで変わった・残すファイルと中身が違う など（消していない）
    Skipped(String),
    /// 隔離フォルダを作れない（すぐに消すかを人に尋ねる）
    NoTrash(String),
    Failed(String),
}

/// 削除の結果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PurgeReport {
    pub outcomes: Vec<(usize, Outcome)>,
    pub removed: usize,
    pub bytes: u64,
}

/// 隔離フォルダの中の移す先。
pub fn trash_path(root: &Path, stamp: &str, rel: &str) -> PathBuf {
    crate::join(&root.join(TRASH_DIR).join(stamp), rel)
}

/// 消す前の確かめ。消してよければ `None`、だめならその理由。
pub fn verify(fs: &dyn Fs, c: &Candidate) -> io::Result<Option<String>> {
    let now = match fs.metadata(&c.path()) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Some("もうありません".into()));
        }
        Err(e) => return Err(e),
    };
    if now.size != c.meta.size || now.mtime != c.meta.mtime {
        return Ok(Some("走査したあとで変わっています".into()));
    }
    if let Some((h, keeper)) = &c.dupe_of {
        match fs.metadata(keeper) {
            Ok(_) => {}
            Err(_) => {
                return Ok(Some(format!(
                    "残すファイル {} がありません",
                    keeper.display()
                )));
            }
        }
        let mine = crate::hash::full(fs, &c.path(), &mut |_| true)?;
        let theirs = crate::hash::full(fs, keeper, &mut |_| true)?;
        if mine != *h || theirs != *h {
            return Ok(Some("残すファイルと中身が同じでなくなっています".into()));
        }
    }
    Ok(None)
}

/// 削除の外とのやりとり。
pub struct PurgeHooks<'a> {
    /// 消し方を決める
    pub dispose: &'a dyn Fn(&Candidate) -> Disposal,
    /// ごみ箱へ送る（手元のドライブ）
    pub recycle: &'a dyn Fn(&Path) -> io::Result<()>,
    /// 1 件ごとの結果（`false` を返したら止める）
    pub progress: &'a mut dyn FnMut(usize, &Outcome) -> bool,
}

/// チェックしたものを消す（日時の印 `stamp` の隔離フォルダへ。消したものは記録 `log`（CSV）に書く）。
/// 全部にチェックの付いたグループ・守るフォルダ（`protect`）の中は消さない。
pub fn execute(
    fs: &dyn Fs,
    review: &Review,
    stamp: &str,
    protect: &[PathBuf],
    log: &Path,
    hooks: PurgeHooks<'_>,
) -> io::Result<PurgeReport> {
    let PurgeHooks {
        dispose,
        recycle,
        progress,
    } = hooks;
    let emptied = review.emptied_groups();
    let mut report = PurgeReport::default();
    let mut rows: Vec<String> = Vec::new();
    for (i, c) in review.items.iter().enumerate() {
        if !c.checked || c.keep {
            continue;
        }
        let path = c.path();
        let outcome = if emptied.contains(&c.group) {
            Outcome::Skipped("グループの最後の 1 つは消しません".into())
        } else if protect.iter().any(|p| path.starts_with(p)) {
            Outcome::Skipped("守るフォルダの中です".into())
        } else {
            match verify(fs, c) {
                Ok(Some(why)) => Outcome::Skipped(why),
                Err(e) => Outcome::Failed(e.to_string()),
                Ok(None) => {
                    let how = dispose(c);
                    match remove(fs, c, how, stamp, recycle) {
                        Ok(moved) => {
                            rows.push(csv_row(&[
                                how.name(),
                                &path.to_string_lossy(),
                                &moved
                                    .as_ref()
                                    .map(|p| p.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                                &c.meta.size.to_string(),
                                &c.meta.mtime.to_string(),
                                &c.dupe_of
                                    .as_ref()
                                    .map(|(h, _)| crate::hash::hex(h))
                                    .unwrap_or_default(),
                                &c.reason,
                            ]));
                            report.removed += 1;
                            report.bytes += c.meta.size;
                            Outcome::Removed(how, moved)
                        }
                        Err(e) if how == Disposal::Trash && is_trash_error(&e) => {
                            Outcome::NoTrash(e.to_string())
                        }
                        Err(e) => Outcome::Failed(e.to_string()),
                    }
                }
            }
        };
        let go_on = progress(i, &outcome);
        report.outcomes.push((i, outcome));
        if !go_on {
            break;
        }
    }
    if !rows.is_empty() {
        append_log(log, &rows)?;
    }
    Ok(report)
}

/// 隔離フォルダを作れない・移せないエラーか（権限がない・違うボリュームなど）。
fn is_trash_error(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::PermissionDenied
            | io::ErrorKind::CrossesDevices
            | io::ErrorKind::ReadOnlyFilesystem
    )
}

fn remove(
    fs: &dyn Fs,
    c: &Candidate,
    how: Disposal,
    stamp: &str,
    recycle: &dyn Fn(&Path) -> io::Result<()>,
) -> io::Result<Option<PathBuf>> {
    let path = c.path();
    match how {
        Disposal::RecycleBin => {
            recycle(&path)?;
            Ok(None)
        }
        Disposal::Delete => {
            fs.remove_file(&path)?;
            Ok(None)
        }
        Disposal::Trash => {
            let to = trash_path(&c.root, stamp, &c.rel);
            if let Some(p) = to.parent() {
                fs.create_dir_all(p)?;
            }
            fs.rename_new(&path, &to)?;
            Ok(Some(to))
        }
    }
}

/// CSV の 1 行（RFC 4180）。
fn csv_row(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|f| {
            if f.contains([',', '"', '\n', '\r']) {
                format!("\"{}\"", f.replace('"', "\"\""))
            } else {
                (*f).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// CSV の 1 行を読む。
fn csv_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        match (c, quoted) {
            ('"', true) if it.peek() == Some(&'"') => {
                cur.push('"');
                it.next();
            }
            ('"', true) => quoted = false,
            ('"', false) if cur.is_empty() => quoted = true,
            (',', false) => out.push(std::mem::take(&mut cur)),
            (c, _) => cur.push(c),
        }
    }
    out.push(cur);
    out
}

const LOG_HEADER: &str = "how,path,moved_to,size,mtime_ns,hash,reason";

fn append_log(log: &Path, rows: &[String]) -> io::Result<()> {
    if let Some(d) = log.parent() {
        std::fs::create_dir_all(d)?;
    }
    let new = !log.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?;
    let mut text = String::new();
    if new {
        // Excel・yysheet で開けるよう UTF-8 の BOM を付ける
        text.push('\u{feff}');
        text.push_str(LOG_HEADER);
        text.push_str("\r\n");
    }
    for r in rows {
        text.push_str(r);
        text.push_str("\r\n");
    }
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

/// 取り消しの 1 件の結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Restore {
    Restored(PathBuf),
    /// 元の場所に同じ名前がある（戻していない）
    Exists(PathBuf),
    /// ごみ箱へ送ったもの・すぐに消したものは、ここでは戻せない
    NotHere(PathBuf, &'static str),
    Failed(PathBuf, String),
}

/// 消した記録から元に戻す（隔離フォルダへ移したものだけ）。
pub fn undo(fs: &dyn Fs, log: &Path) -> io::Result<Vec<Restore>> {
    let text = std::fs::read_to_string(log)?;
    let mut out = Vec::new();
    for line in text.trim_start_matches('\u{feff}').lines().skip(1) {
        if line.is_empty() {
            continue;
        }
        let f = csv_fields(line);
        if f.len() < 3 {
            continue;
        }
        let orig = PathBuf::from(&f[1]);
        match f[0].as_str() {
            "trash" => {
                let from = PathBuf::from(&f[2]);
                if fs.metadata(&orig).is_ok() {
                    out.push(Restore::Exists(orig));
                    continue;
                }
                let r = (|| {
                    if let Some(p) = orig.parent() {
                        fs.create_dir_all(p)?;
                    }
                    fs.rename_new(&from, &orig)
                })();
                out.push(match r {
                    Ok(()) => Restore::Restored(orig),
                    Err(e) => Restore::Failed(orig, e.to_string()),
                });
            }
            "recycle" => out.push(Restore::NotHere(orig, "ごみ箱から戻してください")),
            _ => out.push(Restore::NotHere(orig, "すぐに消したので戻せません")),
        }
    }
    Ok(out)
}

/// 隔離フォルダの中で、`older_than`（UNIX 時刻からのナノ秒）より前の日時のフォルダ（`<日時>`）。
pub fn expired_trash(fs: &dyn Fs, root: &Path, older_than_stamp: &str) -> io::Result<Vec<PathBuf>> {
    let dir = root.join(TRASH_DIR);
    let entries = match fs.read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out: Vec<PathBuf> = entries
        .into_iter()
        .filter(|e| e.meta.dir && e.name.as_str() < older_than_stamp)
        .map(|e| dir.join(e.name))
        .collect();
    out.sort();
    Ok(out)
}

/// フォルダを中身ごと消す（隔離フォルダの期限切れ）。
pub fn remove_tree(fs: &dyn Fs, dir: &Path) -> io::Result<()> {
    for e in fs.read_dir(dir)? {
        let p = dir.join(&e.name);
        if e.meta.dir && !e.meta.link {
            remove_tree(fs, &p)?;
        } else {
            fs.remove_file(&p)?;
        }
    }
    fs.remove_dir(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::Local;

    fn cand(root: &Path, rel: &str, group: usize, keep: bool) -> Candidate {
        let meta = Local.metadata(&crate::join(root, rel)).unwrap();
        Candidate {
            root: root.to_path_buf(),
            rel: rel.to_owned(),
            meta,
            group,
            keep,
            checked: !keep,
            reason: "古い版".into(),
            confidence: "高".into(),
            dupe_of: None,
        }
    }

    #[test]
    fn purges_safely_and_undoes() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("share");
        for (rel, body) in [
            ("a/報告書_v1.docx", "v1"),
            ("a/報告書_v2.docx", "v2"),
            ("a/報告書_v3.docx", "v3"),
            ("b/x.txt", "dup"),
            ("b/x copy.txt", "dup"),
            ("c/only1.txt", "1"),
            ("c/only2.txt", "2"),
            ("keep/protected.txt", "p"),
            ("keep/protected2.txt", "p"),
            ("changed.txt", "c"),
            ("changed2.txt", "c"),
        ] {
            let p = crate::join(&root, rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let h = crate::hash::bytes(b"dup");
        let mut dup = cand(&root, "b/x copy.txt", 1, false);
        dup.dupe_of = Some((h, root.join("b/x.txt")));
        let mut review = Review {
            items: vec![
                cand(&root, "a/報告書_v3.docx", 0, true),
                cand(&root, "a/報告書_v2.docx", 0, false),
                cand(&root, "a/報告書_v1.docx", 0, false),
                cand(&root, "b/x.txt", 1, true),
                dup,
                // グループの全部にチェック（消さない）
                cand(&root, "c/only1.txt", 2, false),
                cand(&root, "c/only2.txt", 2, false),
                cand(&root, "keep/protected.txt", 3, false),
                cand(&root, "keep/protected2.txt", 3, true),
                cand(&root, "changed.txt", 4, false),
                cand(&root, "changed2.txt", 4, true),
            ],
        };
        review.items[1].checked = true;
        assert_eq!(review.emptied_groups(), vec![2]);
        assert_eq!(review.checked_totals().0, 7);
        // 走査したあとで変わった
        std::fs::write(root.join("changed.txt"), "changed!").unwrap();
        let log = d.path().join("purges/1.csv");
        let rp = review.clone();
        rp.save(&d.path().join("r.review")).unwrap();
        assert_eq!(Review::load(&d.path().join("r.review")).unwrap(), rp);
        let rep = execute(
            &Local,
            &review,
            "2026-10-08-1530",
            &[root.join("keep")],
            &log,
            PurgeHooks {
                dispose: &|_| Disposal::Trash,
                recycle: &|_| unreachable!(),
                progress: &mut |_, _| true,
            },
        )
        .unwrap();
        assert_eq!(rep.removed, 3);
        assert!(!root.join("a/報告書_v1.docx").exists());
        assert!(root.join("a/報告書_v3.docx").exists());
        assert_eq!(
            std::fs::read_to_string(trash_path(&root, "2026-10-08-1530", "a/報告書_v2.docx"))
                .unwrap(),
            "v2"
        );
        assert!(!root.join("b/x copy.txt").exists());
        assert!(root.join("c/only1.txt").exists() && root.join("c/only2.txt").exists());
        assert!(root.join("keep/protected.txt").exists());
        assert!(root.join("changed.txt").exists());
        let skipped: Vec<&str> = rep
            .outcomes
            .iter()
            .filter_map(|(_, o)| match o {
                Outcome::Skipped(w) => Some(w.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(skipped.len(), 4, "{skipped:?}");
        // 記録から元に戻す
        let back = undo(&Local, &log).unwrap();
        assert_eq!(back.len(), 3);
        assert!(
            back.iter().all(|r| matches!(r, Restore::Restored(_))),
            "{back:?}"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("a/報告書_v2.docx")).unwrap(),
            "v2"
        );
        // 2 回目は元の場所にあるので戻さない
        assert!(
            undo(&Local, &log)
                .unwrap()
                .iter()
                .all(|r| matches!(r, Restore::Exists(_)))
        );
        // 期限切れの隔離フォルダ
        let old = expired_trash(&Local, &root, "2026-10-09").unwrap();
        assert_eq!(old.len(), 1);
        remove_tree(&Local, &old[0]).unwrap();
        assert!(
            expired_trash(&Local, &root, "2026-10-09")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn duplicate_must_still_match_its_keeper() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::write(root.join("a"), "same").unwrap();
        std::fs::write(root.join("b"), "same").unwrap();
        let mut c = cand(root, "b", 0, false);
        c.dupe_of = Some((crate::hash::bytes(b"same"), root.join("a")));
        assert_eq!(verify(&Local, &c).unwrap(), None);
        std::fs::write(root.join("a"), "different").unwrap();
        assert!(verify(&Local, &c).unwrap().is_some());
        std::fs::remove_file(root.join("a")).unwrap();
        assert!(verify(&Local, &c).unwrap().unwrap().contains("ありません"));
        // すぐに消す・ごみ箱
        std::fs::write(root.join("a"), "same").unwrap();
        let review = Review {
            items: vec![cand(root, "a", 0, false), cand(root, "b", 0, true)],
        };
        let recycled = std::sync::Mutex::new(Vec::new());
        execute(
            &Local,
            &review,
            "s",
            &[],
            &root.join("log.csv"),
            PurgeHooks {
                dispose: &|_| Disposal::RecycleBin,
                recycle: &|p| {
                    recycled.lock().unwrap().push(p.to_path_buf());
                    Ok(())
                },
                progress: &mut |_, _| true,
            },
        )
        .unwrap();
        assert_eq!(recycled.lock().unwrap().len(), 1);
        assert!(matches!(
            undo(&Local, &root.join("log.csv")).unwrap()[0],
            Restore::NotHere(..)
        ));
        assert_eq!(
            csv_fields(&csv_row(&["a,b", "q\"q", ""])),
            ["a,b", "q\"q", ""]
        );
    }
}

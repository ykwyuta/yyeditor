//! 閲覧履歴とダウンロード履歴（19 章 4.2・4.3）、検索エンジンの候補（19 章 4.4）。
//!
//! 履歴はプロファイルのデータのフォルダに、1 行 1 件のタブ区切りのテキストで足していく（クッキーなどと
//! 同じくプロファイルごとに分ける）。多くなったら古いものから捨てる。

use std::io::{self, Write};
use std::path::Path;

/// 閲覧履歴に残す上限（これの 1.25 倍を超えたら古いものを捨てる）。
pub const MAX_VISITS: usize = 20_000;
/// ダウンロード履歴に残す上限。
pub const MAX_DOWNLOADS: usize = 2_000;

/// 1 行 1 件で保存できるもの。
pub trait Record: Sized {
    fn to_fields(&self) -> Vec<String>;
    fn from_fields(f: &[&str]) -> Option<Self>;
}

/// タブ・改行を空白にする（1 行 1 件・タブ区切りを壊さない）。
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '\t' || c == '\n' || c == '\r' {
                ' '
            } else {
                c
            }
        })
        .collect()
}

fn to_line<T: Record>(r: &T) -> String {
    let mut s = r
        .to_fields()
        .iter()
        .map(|f| clean(f))
        .collect::<Vec<_>>()
        .join("\t");
    s.push('\n');
    s
}

/// 読む（古い順。読めない行は飛ばす。なければ空）。
pub fn load<T: Record>(path: &Path) -> Vec<T> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| T::from_fields(&l.split('\t').collect::<Vec<_>>()))
        .collect()
}

/// 1 件足す。上限の 1.25 倍を超えたら、新しい `max` 件だけに書き直す。
pub fn append<T: Record>(path: &Path, r: &T, max: usize) -> io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(to_line(r).as_bytes())?;
    drop(f);
    // 書き直すかは大きさでおおまかに決める（毎回数えない）
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len > (max as u64) * 160 {
        let all: Vec<T> = load(path);
        if all.len() > max + max / 4 {
            save(path, &all[all.len() - max..])?;
        }
    }
    Ok(())
}

/// すべて書き直す（一時ファイルから置き換える）。
pub fn save<T: Record>(path: &Path, items: &[T]) -> io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    let mut text = String::new();
    for r in items {
        text.push_str(&to_line(r));
    }
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// 消す（なければ何もしない）。
pub fn clear(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// 閲覧した 1 件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visit {
    /// 時刻（UNIX 時間の秒）
    pub time: u64,
    pub url: String,
    pub title: String,
}

impl Record for Visit {
    fn to_fields(&self) -> Vec<String> {
        vec![self.time.to_string(), self.url.clone(), self.title.clone()]
    }
    fn from_fields(f: &[&str]) -> Option<Visit> {
        Some(Visit {
            time: f.first()?.parse().ok()?,
            url: f.get(1)?.to_string(),
            title: f.get(2).unwrap_or(&"").to_string(),
        })
    }
}

/// 履歴に残すページか（`about:`・`data:`・空・エラーのページは残さない）。
pub fn worth_recording(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    !(u.is_empty() || u.starts_with("about:") || u.starts_with("data:") || u.starts_with("edge:"))
}

/// 履歴を語で絞る（すべての語を題名か URL に含むもの。大文字・小文字は区別しない）。新しい順。
pub fn search_visits<'a>(visits: &'a [Visit], query: &str) -> Vec<&'a Visit> {
    search_visit_indices(visits, query)
        .into_iter()
        .map(|i| &visits[i])
        .collect()
}

/// [`search_visits`] の番号版（`visits` の番号。新しい順）。
pub fn search_visit_indices(visits: &[Visit], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    (0..visits.len())
        .rev()
        .filter(|&i| {
            let v = &visits[i];
            let hay = format!("{} {}", v.title.to_lowercase(), v.url.to_lowercase());
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

/// `since` 以降（UNIX 時間の秒）のものを消す（`None` ならすべて）。消した数を返す。
pub fn remove_since<T: Record + Timed>(path: &Path, since: Option<u64>) -> io::Result<usize> {
    let Some(since) = since else {
        let n = load::<T>(path).len();
        clear(path)?;
        return Ok(n);
    };
    let all: Vec<T> = load(path);
    let before = all.len();
    let kept: Vec<T> = all.into_iter().filter(|r| r.time() < since).collect();
    let removed = before - kept.len();
    if removed > 0 {
        save(path, &kept)?;
    }
    Ok(removed)
}

/// 時刻を持つもの。
pub trait Timed {
    fn time(&self) -> u64;
}

impl Timed for Visit {
    fn time(&self) -> u64 {
        self.time
    }
}

impl Timed for DownloadRecord {
    fn time(&self) -> u64 {
        self.time
    }
}

/// ダウンロードの結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadState {
    Completed,
    Cancelled,
    Failed,
    /// 同じ内容のファイルがフォルダにあったので、新しく落としたものは消した（`path` は前からあるファイル）
    Duplicate,
}

impl DownloadState {
    pub fn label(self) -> &'static str {
        match self {
            DownloadState::Completed => "完了",
            DownloadState::Cancelled => "中止",
            DownloadState::Failed => "失敗",
            DownloadState::Duplicate => "重複のため破棄",
        }
    }
    fn code(self) -> &'static str {
        match self {
            DownloadState::Completed => "done",
            DownloadState::Cancelled => "cancelled",
            DownloadState::Failed => "failed",
            DownloadState::Duplicate => "dup",
        }
    }
    fn parse(s: &str) -> Option<DownloadState> {
        Some(match s {
            "done" => DownloadState::Completed,
            "cancelled" => DownloadState::Cancelled,
            "failed" => DownloadState::Failed,
            "dup" => DownloadState::Duplicate,
            _ => return None,
        })
    }
}

/// ダウンロードの 1 件（終わったもの）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadRecord {
    pub time: u64,
    pub state: DownloadState,
    pub bytes: u64,
    pub url: String,
    /// 保存したファイル
    pub path: String,
}

impl Record for DownloadRecord {
    fn to_fields(&self) -> Vec<String> {
        vec![
            self.time.to_string(),
            self.state.code().into(),
            self.bytes.to_string(),
            self.url.clone(),
            self.path.clone(),
        ]
    }
    fn from_fields(f: &[&str]) -> Option<DownloadRecord> {
        Some(DownloadRecord {
            time: f.first()?.parse().ok()?,
            state: DownloadState::parse(f.get(1)?)?,
            bytes: f.get(2)?.parse().ok()?,
            url: f.get(3)?.to_string(),
            path: f.get(4)?.to_string(),
        })
    }
}

/// ファイルの SHA-256。
fn sha256_file(path: &Path) -> io::Result<[u8; 32]> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().into())
}

/// ダウンロードしたファイルと同じ内容（SHA-256 が同じ）のファイルが、同じフォルダ（サブフォルダは見ない）に
/// 前からあれば、そのパス。大きさが同じものだけハッシュを比べる。ダウンロード途中のファイル
/// （`.crdownload`・`.partial`・`.tmp`）は比べない。
pub fn find_duplicate(new_file: &Path) -> io::Result<Option<std::path::PathBuf>> {
    let len = std::fs::metadata(new_file)?.len();
    let Some(dir) = new_file.parent() else {
        return Ok(None);
    };
    let me = std::fs::canonicalize(new_file).unwrap_or_else(|_| new_file.to_path_buf());
    let mut mine: Option<[u8; 32]> = None;
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() || meta.len() != len {
            continue;
        }
        let p = entry.path();
        let lower = p.to_string_lossy().to_ascii_lowercase();
        if [".crdownload", ".partial", ".tmp"]
            .iter()
            .any(|e| lower.ends_with(e))
        {
            continue;
        }
        if std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone()) == me {
            continue;
        }
        let a = match mine {
            Some(h) => h,
            None => {
                let h = sha256_file(new_file)?;
                mine = Some(h);
                h
            }
        };
        if sha256_file(&p).ok() == Some(a) {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// ダウンロードしたファイルが重複なら消して、前からあるファイルのパスを返す（重複でなければ `None`）。
pub fn discard_if_duplicate(new_file: &Path) -> io::Result<Option<std::path::PathBuf>> {
    match find_duplicate(new_file)? {
        Some(existing) => {
            std::fs::remove_file(new_file)?;
            Ok(Some(existing))
        }
        None => Ok(None),
    }
}

/// 検索エンジンの候補。
pub struct SearchEngine {
    pub name: &'static str,
    /// `%s` を検索語に置き換える
    pub url: &'static str,
}

pub const SEARCH_ENGINES: [SearchEngine; 6] = [
    SearchEngine {
        name: "Bing",
        url: "https://www.bing.com/search?q=%s",
    },
    SearchEngine {
        name: "Google",
        url: "https://www.google.com/search?q=%s",
    },
    SearchEngine {
        name: "Yahoo! JAPAN",
        url: "https://search.yahoo.co.jp/search?p=%s",
    },
    SearchEngine {
        name: "DuckDuckGo",
        url: "https://duckduckgo.com/?q=%s",
    },
    SearchEngine {
        name: "Brave Search",
        url: "https://search.brave.com/search?q=%s",
    },
    SearchEngine {
        name: "Startpage",
        url: "https://www.startpage.com/do/search?q=%s",
    },
];

/// 検索の URL を確かめる（http・https で、検索語の `%s` があること）。
pub fn validate_search_url(u: &str) -> Result<(), String> {
    let u = u.trim();
    let lower = u.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err("検索の URL は https:// か http:// で始めてください".into());
    }
    if !u.contains("%s") {
        return Err(
            "検索語を入れる場所に %s を書いてください（例: https://example.com/search?q=%s）"
                .into(),
        );
    }
    if u.chars().any(|c| c.is_whitespace() || c == '"') {
        return Err("検索の URL に空白や引用符は使えません".into());
    }
    Ok(())
}

/// 検索の URL の、候補の中での名前（なければ `None`）。
pub fn search_engine_name(url: &str) -> Option<&'static str> {
    SEARCH_ENGINES
        .iter()
        .find(|e| e.url == url.trim())
        .map(|e| e.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_searches_visits() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("history.tsv");
        assert!(load::<Visit>(&p).is_empty());
        for (t, u, title) in [
            (1, "https://example.com/", "Example\tDomain"),
            (2, "https://www.rust-lang.org/", "Rust 言語"),
            (3, "https://example.com/news", "ニュース\n速報"),
        ] {
            append(
                &p,
                &Visit {
                    time: t,
                    url: u.into(),
                    title: title.into(),
                },
                MAX_VISITS,
            )
            .unwrap();
        }
        let v: Vec<Visit> = load(&p);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].title, "Example Domain");
        assert_eq!(v[2].title, "ニュース 速報");
        let found = search_visits(&v, "EXAMPLE");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].time, 3); // 新しい順
        assert_eq!(search_visits(&v, "rust 言語").len(), 1);
        assert_eq!(search_visits(&v, "").len(), 3);
        assert_eq!(search_visit_indices(&v, "example"), [2, 0]);
        // 期間で消す
        let p2 = d.path().join("h2.tsv");
        save(&p2, &v).unwrap();
        assert_eq!(remove_since::<Visit>(&p2, Some(2)).unwrap(), 2);
        assert_eq!(load::<Visit>(&p2).len(), 1);
        assert_eq!(remove_since::<Visit>(&p2, None).unwrap(), 1);
        assert!(load::<Visit>(&p2).is_empty());
        // 1 件消して書き直す・すべて消す
        save(&p, &v[1..]).unwrap();
        assert_eq!(load::<Visit>(&p).len(), 2);
        clear(&p).unwrap();
        clear(&p).unwrap();
        assert!(load::<Visit>(&p).is_empty());
        assert!(!worth_recording("about:blank"));
        assert!(worth_recording("https://a/"));
    }

    #[test]
    fn caps_the_history() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("h.tsv");
        let max = 40;
        for t in 0..200u64 {
            let v = Visit {
                time: t,
                url: format!("https://example.com/{}", "x".repeat(200)),
                title: "t".into(),
            };
            append(&p, &v, max).unwrap();
        }
        let v: Vec<Visit> = load(&p);
        assert!(v.len() <= max + max / 4 + 1, "{}", v.len());
        assert_eq!(v.last().unwrap().time, 199);
    }

    #[test]
    fn download_records_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("downloads.tsv");
        let r = DownloadRecord {
            time: 10,
            state: DownloadState::Completed,
            bytes: 12345,
            url: "https://example.com/a.zip".into(),
            path: "C:\\Users\\u\\Downloads\\a.zip".into(),
        };
        append(&p, &r, MAX_DOWNLOADS).unwrap();
        let mut c = r.clone();
        c.state = DownloadState::Cancelled;
        append(&p, &c, MAX_DOWNLOADS).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&p)
            .unwrap()
            .write_all(b"garbage line\n")
            .unwrap();
        assert_eq!(load::<DownloadRecord>(&p), [r, c]);
    }

    #[test]
    fn discards_duplicate_downloads() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        std::fs::write(dir.join("report.pdf"), b"same content").unwrap();
        std::fs::write(dir.join("other.pdf"), b"same-content").unwrap(); // 同じ大きさで中身が違う
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("deep.pdf"), b"unique data!").unwrap();
        // 同じ内容: 新しいほうを消す
        let new = dir.join("report (1).pdf");
        std::fs::write(&new, b"same content").unwrap();
        let found = discard_if_duplicate(&new).unwrap();
        assert_eq!(found.as_deref(), Some(dir.join("report.pdf").as_path()));
        assert!(!new.exists());
        assert!(dir.join("report.pdf").exists());
        // 違う内容（大きさは同じ）は残す
        let new2 = dir.join("x.pdf");
        std::fs::write(&new2, b"different!!!").unwrap();
        assert_eq!(discard_if_duplicate(&new2).unwrap(), None);
        assert!(new2.exists());
        // サブフォルダは見ない
        let new3 = dir.join("deep (copy).pdf");
        std::fs::write(&new3, b"unique data!").unwrap();
        assert_eq!(discard_if_duplicate(&new3).unwrap(), None);
        assert!(new3.exists());
        // 途中のファイルは比べない
        std::fs::write(dir.join("y.bin.crdownload"), b"abc").unwrap();
        let new4 = dir.join("y.bin");
        std::fs::write(&new4, b"abc").unwrap();
        assert_eq!(discard_if_duplicate(&new4).unwrap(), None);
        // 履歴に残せる
        let p = dir.join("downloads.tsv");
        let r = DownloadRecord {
            time: 1,
            state: DownloadState::Duplicate,
            bytes: 12,
            url: "https://example.com/report.pdf".into(),
            path: dir.join("report.pdf").to_string_lossy().into_owned(),
        };
        append(&p, &r, MAX_DOWNLOADS).unwrap();
        assert_eq!(load::<DownloadRecord>(&p), [r]);
    }

    #[test]
    fn search_engines() {
        for e in &SEARCH_ENGINES {
            assert!(validate_search_url(e.url).is_ok(), "{}", e.name);
        }
        assert_eq!(
            search_engine_name("https://www.google.com/search?q=%s"),
            Some("Google")
        );
        assert_eq!(search_engine_name("https://x/?q=%s"), None);
        assert!(validate_search_url("https://example.com/search?q=").is_err());
        assert!(validate_search_url("ftp://x/%s").is_err());
        assert!(validate_search_url("https://x/?q=%s a").is_err());
        // 既定（yy-config）の Bing は候補に入っている
        assert_eq!(
            search_engine_name("https://www.bing.com/search?q=%s"),
            Some("Bing")
        );
    }
}

//! フィルタリストのキャッシュと更新（20 章 3.2）。
//!
//! キャッシュのフォルダに、URL ごとに本文（`<印>.txt`）と記録（`<印>.meta`）を置く。記録は
//! `key=value` の行（取得した時刻・次に更新する時刻・規則の数・最後の失敗）。

use std::io;
use std::path::{Path, PathBuf};

/// 更新の間隔の既定（リストに `! Expires:` がないとき）: 4 日。
pub const DEFAULT_EXPIRES: u64 = 4 * 24 * 3600;
/// 間隔の下限と上限（1 時間〜14 日）。
const MIN_EXPIRES: u64 = 3600;
const MAX_EXPIRES: u64 = 14 * 24 * 3600;
/// ダウンロードの上限（1 つ 30 MB）。
pub const MAX_LIST_BYTES: usize = 30 * 1024 * 1024;
/// 失敗したときに次に試すまでの間（1 時間）。
const RETRY_AFTER: u64 = 3600;

/// 今の時刻（UNIX 時間の秒）。
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// キャッシュのファイルの名前の元（URL から作る。読める部分 + FNV-1a の印）。
pub fn cache_stem(url: &str) -> String {
    let url = url.trim();
    let tail = url.rsplit(['/', '\\']).next().unwrap_or("list");
    let readable: String = tail
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(24)
        .collect();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{readable}-{h:016x}")
}

/// 更新の記録。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheMeta {
    /// 最後に取得できた時刻（0 なら一度も）
    pub fetched: u64,
    /// 次に更新する時刻
    pub next: u64,
    /// 規則の数
    pub rules: usize,
    /// 最後の失敗（空なら成功）
    pub error: String,
}

impl CacheMeta {
    fn to_text(&self) -> String {
        format!(
            "fetched={}\nnext={}\nrules={}\nerror={}\n",
            self.fetched,
            self.next,
            self.rules,
            self.error.replace(['\r', '\n'], " ")
        )
    }

    fn parse(text: &str) -> CacheMeta {
        let mut m = CacheMeta::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match k {
                "fetched" => m.fetched = v.parse().unwrap_or(0),
                "next" => m.next = v.parse().unwrap_or(0),
                "rules" => m.rules = v.parse().unwrap_or(0),
                "error" => m.error = v.to_owned(),
                _ => {}
            }
        }
        m
    }

    /// 更新する時期か（記録がない・本文がない・期限が来た）。
    pub fn is_due(meta: Option<&CacheMeta>, now: u64) -> bool {
        match meta {
            None => true,
            Some(m) => m.next <= now,
        }
    }
}

/// 1 つのリストのキャッシュ。
#[derive(Clone, Debug)]
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: impl Into<PathBuf>) -> Cache {
        Cache { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn text_path(&self, url: &str) -> PathBuf {
        self.dir.join(format!("{}.txt", cache_stem(url)))
    }

    fn meta_path(&self, url: &str) -> PathBuf {
        self.dir.join(format!("{}.meta", cache_stem(url)))
    }

    /// 本文（なければ `None`）。
    pub fn text(&self, url: &str) -> Option<String> {
        std::fs::read_to_string(self.text_path(url)).ok()
    }

    /// 記録（なければ `None`）。
    pub fn meta(&self, url: &str) -> Option<CacheMeta> {
        std::fs::read_to_string(self.meta_path(url))
            .ok()
            .map(|t| CacheMeta::parse(&t))
    }

    /// 取得できた本文を書き、記録を更新する（一時ファイルから置き換える）。
    pub fn store(&self, url: &str, text: &str, now: u64) -> io::Result<CacheMeta> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.text_path(url);
        let tmp = path.with_extension("txt.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)?;
        let meta = CacheMeta {
            fetched: now,
            next: now + expires_secs(text),
            rules: count_rules(text),
            error: String::new(),
        };
        std::fs::write(self.meta_path(url), meta.to_text())?;
        Ok(meta)
    }

    /// 失敗を記録する（本文は残す。1 時間後にまた試す）。
    pub fn fail(&self, url: &str, error: &str, now: u64) -> io::Result<CacheMeta> {
        std::fs::create_dir_all(&self.dir)?;
        let mut meta = self.meta(url).unwrap_or_default();
        meta.error = error.replace(['\r', '\n'], " ");
        meta.next = now + RETRY_AFTER;
        std::fs::write(self.meta_path(url), meta.to_text())?;
        Ok(meta)
    }
}

/// `! Expires: 4 days`・`! Expires: 12 hours` を秒に（1 時間〜14 日に収める。なければ既定）。
pub fn expires_secs(text: &str) -> u64 {
    for line in text.lines().take(100) {
        let l = line.trim();
        if !(l.starts_with('!') || l.starts_with('#')) {
            if l.is_empty() || l.starts_with('[') {
                continue;
            }
            break;
        }
        let body = l.trim_start_matches(['!', '#']).trim();
        let Some(v) = body
            .strip_prefix("Expires:")
            .or_else(|| body.strip_prefix("expires:"))
        else {
            continue;
        };
        let mut words = v.split_whitespace();
        let Some(n) = words.next().and_then(|n| n.parse::<u64>().ok()) else {
            continue;
        };
        let unit = words.next().unwrap_or("days").to_ascii_lowercase();
        let secs = if unit.starts_with('h') {
            n * 3600
        } else if unit.starts_with('d') {
            n * 24 * 3600
        } else {
            continue;
        };
        return secs.clamp(MIN_EXPIRES, MAX_EXPIRES);
    }
    DEFAULT_EXPIRES
}

/// 規則の行の数（空行・注釈・`[Adblock Plus 2.0]` を除く）。
pub fn count_rules(text: &str) -> usize {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('!') && !l.starts_with('['))
        .count()
}

/// ダウンロードしたものがフィルタリストらしいか確かめ、文字列にする（BOM を除く）。HTML（ログインの
/// 画面・エラーのページ）や空のものは断る（キャッシュを壊さないため）。
pub fn validate_download(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > MAX_LIST_BYTES {
        return Err(format!(
            "大きすぎます（{} MB を超えています）",
            MAX_LIST_BYTES / 1024 / 1024
        ));
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = String::from_utf8_lossy(bytes).into_owned();
    let head: String = text
        .chars()
        .take(2048)
        .collect::<String>()
        .to_ascii_lowercase();
    let first = head.trim_start();
    if first.starts_with('<') || head.contains("<html") || head.contains("<!doctype") {
        return Err("フィルタリストではなく HTML のページが返りました".into());
    }
    if count_rules(&text) == 0 {
        return Err("規則が 1 つもありません".into());
    }
    Ok(text)
}

/// ローカルのファイルのパス（`file:///C:/a.txt` も）。
pub fn local_path(url: &str) -> PathBuf {
    let u = url.trim();
    match u
        .strip_prefix("file:///")
        .or_else(|| u.strip_prefix("FILE:///"))
    {
        Some(rest) => {
            // file:///C:/x → C:/x、file:///home/x → /home/x
            let decoded = rest.replace("%20", " ");
            if decoded.as_bytes().get(1) == Some(&b':') {
                PathBuf::from(decoded)
            } else {
                PathBuf::from(format!("/{decoded}"))
            }
        }
        None => PathBuf::from(u),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_expiry_headers() {
        assert_eq!(
            expires_secs(
                "[Adblock Plus 2.0]\n! Title: EasyList\n! Expires: 4 days (update frequency)\n||a^"
            ),
            4 * 24 * 3600
        );
        assert_eq!(expires_secs("! Expires: 12 hours\n"), 12 * 3600);
        assert_eq!(expires_secs("! Expires: 1 hours\n"), 3600);
        assert_eq!(expires_secs("! Expires: 0 hours\n"), 3600);
        assert_eq!(expires_secs("! Expires: 99 days\n"), 14 * 24 * 3600);
        assert_eq!(
            expires_secs("! Title: x\n||a^\n! Expires: 1 days\n"),
            DEFAULT_EXPIRES
        );
        assert_eq!(expires_secs("! Expires: soon\n"), DEFAULT_EXPIRES);
        assert_eq!(expires_secs(""), DEFAULT_EXPIRES);
    }

    #[test]
    fn caches_lists_and_records_failures() {
        let d = tempfile::tempdir().unwrap();
        let c = Cache::new(d.path().join("filters"));
        let url = "https://easylist.to/easylist/easylist.txt";
        assert!(c.text(url).is_none());
        assert!(CacheMeta::is_due(c.meta(url).as_ref(), 1000));
        let text = "! Expires: 2 days\n||ads.example^\n##.ad\n\n! note\n";
        let m = c.store(url, text, 1000).unwrap();
        assert_eq!(m.rules, 2);
        assert_eq!(m.next, 1000 + 2 * 24 * 3600);
        assert_eq!(c.text(url).unwrap(), text);
        assert_eq!(c.meta(url).unwrap(), m);
        assert!(!CacheMeta::is_due(Some(&m), 2000));
        assert!(CacheMeta::is_due(Some(&m), m.next));
        // 失敗しても本文は残り、1 時間後にまた試す
        let f = c.fail(url, "つなげません\n(12029)", 5000).unwrap();
        assert_eq!(f.error, "つなげません (12029)");
        assert_eq!(f.next, 5000 + 3600);
        assert_eq!(f.fetched, 1000);
        assert_eq!(c.text(url).unwrap(), text);
        assert_eq!(c.meta(url).unwrap(), f);
        // 違う URL は違うファイル
        assert_ne!(
            cache_stem(url),
            cache_stem("https://easylist.to/easylist/easyprivacy.txt")
        );
        assert!(cache_stem(url).starts_with("easylisttxt-"));
    }

    #[test]
    fn validates_downloads() {
        assert_eq!(
            validate_download(b"\xEF\xBB\xBF[Adblock Plus 2.0]\n||a^\n").unwrap(),
            "[Adblock Plus 2.0]\n||a^\n"
        );
        assert!(validate_download(b"<!DOCTYPE html><html><body>login</body></html>").is_err());
        assert!(validate_download(b"  <html>").is_err());
        assert!(validate_download(b"! only comments\n\n").is_err());
        assert!(validate_download(&vec![b'a'; MAX_LIST_BYTES + 1]).is_err());
    }

    #[test]
    fn local_paths() {
        assert_eq!(
            local_path("file:///C:/filters/my%20list.txt"),
            PathBuf::from("C:/filters/my list.txt")
        );
        assert_eq!(
            local_path("file:///home/u/a.txt"),
            PathBuf::from("/home/u/a.txt")
        );
        assert_eq!(local_path(" C:\\f\\a.txt "), PathBuf::from("C:\\f\\a.txt"));
    }
}

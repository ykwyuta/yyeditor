//! ファイルの検索（18 章 8）。
//!
//! 名前・パス・種類・大きさ・日時・属性は目録（[`crate::scan`]）から探し（読み直さない）、中身はエディタの
//! Grep と同じ部品（`yy_core::grep`。文字コードを判別する）でファイル単位に並列に読む。Office の文書
//! （`docx`・`xlsx`・`pptx`）は中の文字列を取り出して探す。
//!
//! 1 行の検索欄の書き方（[`parse_query`]）: `見積 ext:xlsx size:>1MB modified:>=2026-09-01 content:"税込"`。

use std::collections::HashSet;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rayon::prelude::*;

use crate::dupes::FileRef;
use crate::scan::{Catalog, FileEntry};
use crate::similar::fold;

/// 名前の条件。
#[derive(Clone, Debug)]
pub enum NameMatch {
    /// 語をすべて含む（比べる形にそろえた語）
    Words(Vec<String>),
    /// ワイルドカード（`*`・`?`）
    Wildcard(String),
    /// 正規表現
    Regex(regex_automata::meta::Regex),
}

/// 検索の条件（すべて AND）。
#[derive(Clone, Debug, Default)]
pub struct Query {
    pub name: Vec<NameMatch>,
    /// パスに含む（フォルダの名前など）
    pub path: Vec<String>,
    /// パスに含まない
    pub not_path: Vec<String>,
    /// 拡張子（小文字、`.` なし。空ならどれでも）
    pub exts: Vec<String>,
    /// 大きさの範囲（両端を含む）
    pub size: (Option<u64>, Option<u64>),
    /// 更新日時の範囲（ナノ秒。始めを含み、終わりを含まない）
    pub modified: (Option<i64>, Option<i64>),
    pub created: (Option<i64>, Option<i64>),
    pub readonly: Option<bool>,
    pub hidden: Option<bool>,
    /// 中身
    pub content: Option<yy_search::Query>,
    /// 全角・半角、大文字・小文字、ひらがな・カタカナの違いを無視する
    pub loose: bool,
    /// 中身が同じファイルがほかにある（`Some(false)` はない）
    pub dupe: Option<bool>,
    /// 版の判定（似たファイル）の結果
    pub version: Option<VersionMark>,
}

/// 版の判定の条件（18 章 8.1「整理の結果」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionMark {
    /// 古い版と判定された（新しい版がほかにある）
    Old,
    /// グループの最新と判定された
    Newest,
    /// 版のグループに入る
    Any,
}

/// 整理の結果（重複・版の判定）。検索した目録と同じ並びの [`FileRef`]。
#[derive(Clone, Debug, Default)]
pub struct Marks {
    /// 中身が同じファイルがほかにあるもの
    pub dupes: HashSet<FileRef>,
    /// 古い版
    pub old: HashSet<FileRef>,
    /// グループの最新
    pub newest: HashSet<FileRef>,
}

impl Marks {
    /// 版の判定の結果を入れる。
    pub fn set_versions(&mut self, groups: &[crate::similar::VersionGroup]) {
        for g in groups {
            for (k, m) in g.members.iter().enumerate() {
                if k == 0 {
                    self.newest.insert(m.file);
                } else {
                    self.old.insert(m.file);
                }
            }
        }
    }

    /// 重複の結果を入れる。
    pub fn set_dupes(&mut self, groups: &[crate::dupes::Group]) {
        for g in groups {
            self.dupes.extend(g.files.iter().copied());
        }
    }
}

/// 種類（仲間でまとめた拡張子）。
pub const KINDS: &[(&str, &[&str])] = &[
    (
        "文書",
        &[
            "doc", "docx", "docm", "pdf", "txt", "md", "rtf", "odt", "ppt", "pptx",
        ],
    ),
    (
        "表計算",
        &["xls", "xlsx", "xlsm", "xlsb", "csv", "tsv", "ods", "yys"],
    ),
    (
        "画像",
        &[
            "jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "heic", "svg", "ico",
        ],
    ),
    (
        "動画",
        &[
            "mp4", "mov", "avi", "mkv", "wmv", "m4v", "webm", "mpg", "mpeg",
        ],
    ),
    ("音声", &["mp3", "wav", "m4a", "aac", "flac", "wma", "ogg"]),
    (
        "圧縮",
        &[
            "zip", "7z", "rar", "lzh", "tar", "gz", "tgz", "bz2", "xz", "cab",
        ],
    ),
    (
        "テキスト",
        &[
            "txt", "md", "log", "csv", "tsv", "json", "xml", "yml", "yaml", "ini", "toml", "html",
            "htm",
        ],
    ),
];

fn kind_alias(k: &str) -> &str {
    match k {
        "doc" | "document" => "文書",
        "sheet" | "spreadsheet" => "表計算",
        "image" | "picture" | "写真" => "画像",
        "video" | "movie" => "動画",
        "audio" | "sound" => "音声",
        "archive" | "zip" => "圧縮",
        "text" => "テキスト",
        k => k,
    }
}

/// 比べる形（`loose` なら [`fold`]、でなければ小文字だけ）。
fn norm(s: &str, loose: bool) -> String {
    if loose { fold(s) } else { s.to_lowercase() }
}

impl Query {
    /// 名前・属性の条件に合うか（中身は見ない）。
    pub fn matches(&self, f: &FileEntry) -> bool {
        let name = f.name();
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        if !self.exts.is_empty() && !self.exts.contains(&ext) {
            return false;
        }
        let m = &f.meta;
        let within = |v: u64, (lo, hi): (Option<u64>, Option<u64>)| {
            lo.is_none_or(|l| v >= l) && hi.is_none_or(|h| v <= h)
        };
        let within_t = |v: i64, (lo, hi): (Option<i64>, Option<i64>)| {
            lo.is_none_or(|l| v >= l) && hi.is_none_or(|h| v < h)
        };
        if !within(m.size, self.size)
            || !within_t(m.mtime, self.modified)
            || !within_t(m.ctime, self.created)
            || self.readonly.is_some_and(|r| r != m.readonly)
            || self.hidden.is_some_and(|h| h != m.hidden)
        {
            return false;
        }
        if !self.path.is_empty() || !self.not_path.is_empty() {
            let p = norm(&f.rel, self.loose).replace('\\', "/");
            if !self.path.iter().all(|x| p.contains(x.as_str())) {
                return false;
            }
            if self.not_path.iter().any(|x| p.contains(x.as_str())) {
                return false;
            }
        }
        if !self.name.is_empty() {
            let n = norm(name, self.loose);
            for c in &self.name {
                let ok = match c {
                    NameMatch::Words(w) => w.iter().all(|w| n.contains(w.as_str())),
                    NameMatch::Wildcard(p) => yy_core::grep::wildcard_match(p, &n),
                    NameMatch::Regex(r) => r.is_match(name),
                };
                if !ok {
                    return false;
                }
            }
        }
        true
    }

    /// 整理の結果の条件を使うか（重複・版の判定）。
    pub fn needs_marks(&self) -> (bool, bool) {
        (self.dupe.is_some(), self.version.is_some())
    }

    /// 整理の結果の条件で絞る。
    pub fn apply_marks(&self, found: Vec<FileRef>, marks: &Marks) -> Vec<FileRef> {
        found
            .into_iter()
            .filter(|r| self.dupe.is_none_or(|d| marks.dupes.contains(r) == d))
            .filter(|r| match self.version {
                None => true,
                Some(VersionMark::Old) => marks.old.contains(r),
                Some(VersionMark::Newest) => marks.newest.contains(r),
                Some(VersionMark::Any) => marks.old.contains(r) || marks.newest.contains(r),
            })
            .collect()
    }

    /// 名前・属性の条件で目録から探す（目録の順。整理の結果の条件は [`Query::apply_marks`]）。
    pub fn filter(&self, cats: &[Catalog]) -> Vec<FileRef> {
        let mut out = Vec::new();
        for (ri, c) in cats.iter().enumerate() {
            let hits: Vec<usize> = c
                .files
                .par_iter()
                .enumerate()
                .filter(|(_, f)| self.matches(f))
                .map(|(i, _)| i)
                .collect();
            out.extend(hits.into_iter().map(|index| FileRef { root: ri, index }));
        }
        out
    }
}

/// 大きさ（`100MB`・`1.5GB`・`0`）。
fn parse_size(s: &str) -> Result<u64, String> {
    let t = s.trim().to_ascii_uppercase();
    let (num, mul) = [
        ("TB", 1u64 << 40),
        ("GB", 1 << 30),
        ("MB", 1 << 20),
        ("KB", 1 << 10),
        ("T", 1 << 40),
        ("G", 1 << 30),
        ("M", 1 << 20),
        ("K", 1 << 10),
        ("B", 1),
    ]
    .iter()
    .find_map(|(u, m)| t.strip_suffix(u).map(|n| (n.trim().to_owned(), *m)))
    .unwrap_or((t.clone(), 1));
    let v: f64 = num
        .parse()
        .map_err(|_| format!("大きさが読めません: {s}"))?;
    Ok((v * mul as f64) as u64)
}

const DAY: i64 = 86_400_000_000_000;

/// 日付の始めのナノ秒（`2026-09-01`・`2026/9/1`・`20260901`）。`tz` は UTC との差（秒）。
fn parse_day(s: &str, tz: i64) -> Option<i64> {
    let parts: Vec<&str> = if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
        vec![&s[..4], &s[4..6], &s[6..]]
    } else {
        s.split(['-', '/', '.']).collect()
    };
    let n: Vec<i64> = parts
        .iter()
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let (y, m, d) = match n.as_slice() {
        [y, m, d] => (*y, *m, *d),
        [y, m] => (*y, *m, 1),
        _ => return None,
    };
    if !(1970..=2200).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d) * DAY - tz * 1_000_000_000)
}

/// 年月日から 1970-01-01 からの日数（Howard Hinnant の方法）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 日時の条件（範囲）。`now` は今（ナノ秒）、`tz` は UTC との差（秒）。
fn parse_when(v: &str, now: i64, tz: i64) -> Result<(Option<i64>, Option<i64>), String> {
    let today = (now + tz * 1_000_000_000).div_euclid(DAY) * DAY - tz * 1_000_000_000;
    let err = || format!("日時が読めません: {v}");
    let days_n = |s: &str| s.trim().parse::<i64>().map_err(|_| err());
    Ok(match v {
        "today" | "今日" => (Some(today), Some(today + DAY)),
        "yesterday" | "昨日" => (Some(today - DAY), Some(today)),
        "week" | "thisweek" | "今週" => {
            // 月曜から
            let wd = ((today + tz * 1_000_000_000).div_euclid(DAY) + 3).rem_euclid(7);
            (Some(today - wd * DAY), None)
        }
        "month" | "thismonth" | "今月" => {
            let s = super::sync::stamp(now + tz * 1_000_000_000);
            (parse_day(&s[..7], tz), None)
        }
        _ => {
            if let Some(n) = v
                .strip_suffix("日より前")
                .or_else(|| v.strip_suffix("日以前"))
            {
                (None, Some(now - days_n(n)? * DAY))
            } else if let Some(n) = v.strip_suffix("日以内").or_else(|| v.strip_suffix('d')) {
                (Some(now - days_n(n)? * DAY), None)
            } else if let Some((a, b)) = v.split_once("..") {
                let a = if a.is_empty() {
                    None
                } else {
                    Some(parse_day(a, tz).ok_or_else(err)?)
                };
                let b = if b.is_empty() {
                    None
                } else {
                    Some(parse_day(b, tz).ok_or_else(err)? + DAY)
                };
                (a, b)
            } else if let Some(x) = v.strip_prefix(">=") {
                (Some(parse_day(x, tz).ok_or_else(err)?), None)
            } else if let Some(x) = v.strip_prefix("<=") {
                (None, Some(parse_day(x, tz).ok_or_else(err)? + DAY))
            } else if let Some(x) = v.strip_prefix('>') {
                (Some(parse_day(x, tz).ok_or_else(err)? + DAY), None)
            } else if let Some(x) = v.strip_prefix('<') {
                (None, Some(parse_day(x, tz).ok_or_else(err)?))
            } else {
                let start = parse_day(v, tz).ok_or_else(err)?;
                // `2026-09` はその月
                let end = if v.split(['-', '/', '.']).count() == 2 {
                    let (y, m): (i64, i64) = {
                        let mut it = v.split(['-', '/', '.']);
                        (
                            it.next().and_then(|x| x.parse().ok()).ok_or_else(err)?,
                            it.next().and_then(|x| x.parse().ok()).ok_or_else(err)?,
                        )
                    };
                    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
                    days_from_civil(ny, nm, 1) * DAY - tz * 1_000_000_000
                } else {
                    start + DAY
                };
                (Some(start), Some(end))
            }
        }
    })
}

fn parse_size_range(v: &str) -> Result<(Option<u64>, Option<u64>), String> {
    Ok(match v {
        "empty" | "空" | "0" => (Some(0), Some(0)),
        _ => {
            if let Some((a, b)) = v.split_once("..") {
                (
                    if a.is_empty() {
                        None
                    } else {
                        Some(parse_size(a)?)
                    },
                    if b.is_empty() {
                        None
                    } else {
                        Some(parse_size(b)?)
                    },
                )
            } else if let Some(x) = v.strip_prefix(">=") {
                (Some(parse_size(x)?), None)
            } else if let Some(x) = v.strip_prefix("<=") {
                (None, Some(parse_size(x)?))
            } else if let Some(x) = v.strip_prefix('>') {
                (Some(parse_size(x)? + 1), None)
            } else if let Some(x) = v.strip_prefix('<') {
                (None, Some(parse_size(x)?.saturating_sub(1)))
            } else {
                let n = parse_size(v)?;
                (Some(n), Some(n))
            }
        }
    })
}

/// 語に分ける（`"…"` の中の空白は分けない）。
fn tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn name_regex(p: &str) -> Result<regex_automata::meta::Regex, String> {
    regex_automata::meta::Regex::builder()
        .syntax(regex_automata::util::syntax::Config::new().case_insensitive(true))
        .build(p)
        .map_err(|e| format!("正規表現が読めません: {e}"))
}

/// 1 行の検索欄を読む。`now` は今（ナノ秒）、`tz` は UTC との差（秒。日本は 32400）。
pub fn parse_query(line: &str, now: i64, tz: i64) -> Result<Query, String> {
    let mut q = Query {
        loose: true,
        ..Query::default()
    };
    let mut words = Vec::new();
    for t in tokens(line) {
        let (key, val) = match t.split_once(':') {
            Some((k, v))
                if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphabetic() || c == '-') =>
            {
                (k.to_ascii_lowercase(), v.to_owned())
            }
            _ => (String::new(), t.clone()),
        };
        match key.as_str() {
            "ext" | "type" => q.exts.extend(
                val.split([';', ','])
                    .map(|e| {
                        e.trim()
                            .trim_start_matches("*.")
                            .trim_start_matches('.')
                            .to_ascii_lowercase()
                    })
                    .filter(|e| !e.is_empty()),
            ),
            "kind" => {
                let k = kind_alias(&val);
                let exts = KINDS
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, e)| *e)
                    .ok_or_else(|| format!("知らない種類です: {val}"))?;
                q.exts.extend(exts.iter().map(|e| (*e).to_owned()));
            }
            "size" => q.size = parse_size_range(&val)?,
            "modified" | "mtime" | "date" | "updated" => q.modified = parse_when(&val, now, tz)?,
            "created" | "ctime" => q.created = parse_when(&val, now, tz)?,
            "path" | "in" => q.path.push(norm(&val, true).replace('\\', "/")),
            "-path" | "notpath" | "notin" => q.not_path.push(norm(&val, true).replace('\\', "/")),
            "content" | "text" | "grep" => {
                q.content = Some(yy_search::Query {
                    pattern: val,
                    regex: false,
                    case_sensitive: false,
                    whole_word: false,
                })
            }
            "re" | "regex" => q.name.push(NameMatch::Regex(name_regex(&val)?)),
            "attr" | "is" => match val.as_str() {
                "readonly" | "読み取り専用" => q.readonly = Some(true),
                "hidden" | "隠し" => q.hidden = Some(true),
                _ => return Err(format!("知らない属性です: {val}")),
            },
            "case" => q.loose = !matches!(val.as_str(), "on" | "yes" | "true"),
            "dupe" | "dup" | "duplicate" => {
                q.dupe = Some(match val.as_str() {
                    "yes" | "on" | "true" | "あり" | "ある" => true,
                    "no" | "off" | "false" | "なし" | "ない" => false,
                    _ => return Err(format!("dupe: は yes か no です: {val}")),
                })
            }
            "has" if matches!(val.as_str(), "dupe" | "dup" | "重複") => q.dupe = Some(true),
            "version" | "ver" => {
                q.version = Some(match val.as_str() {
                    "old" | "古い" | "older" => VersionMark::Old,
                    "newest" | "new" | "latest" | "最新" => VersionMark::Newest,
                    "any" | "yes" | "あり" => VersionMark::Any,
                    _ => {
                        return Err(format!("version: は old・newest・any のどれかです: {val}"));
                    }
                })
            }
            "name" => words.push(val),
            _ => {
                if val.len() > 2 && val.starts_with('/') && val.ends_with('/') {
                    q.name
                        .push(NameMatch::Regex(name_regex(&val[1..val.len() - 1])?));
                } else if val.contains(['*', '?']) {
                    q.name.push(NameMatch::Wildcard(norm(&val, true)));
                } else {
                    words.push(val);
                }
            }
        }
    }
    if !words.is_empty() {
        let loose = q.loose;
        q.name.insert(
            0,
            NameMatch::Words(words.iter().map(|w| norm(w, loose)).collect()),
        );
    }
    Ok(q)
}

/// 中身で見つかった行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub file: FileRef,
    /// 行番号（1 始まり。Office の文書は取り出した文字列の行）
    pub line: u64,
    pub text: String,
}

/// 中身の検索の結果の数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContentStats {
    pub files: u64,
    pub matched_files: u64,
    pub hits: u64,
    /// バイナリ・大きすぎる・読めないなどで飛ばした
    pub skipped: u64,
    /// 中身を探すファイルのパターンに合わないので読まなかった
    pub excluded: u64,
    /// 中身の索引で候補から外した（読まなかった）
    pub pruned: u64,
    /// 読んで中身の索引に足した
    pub indexed: u64,
    /// メモリの上限のため中身の索引に足さなかった
    pub not_indexed: u64,
}

/// 中身の検索の設定。
#[derive(Clone, Debug)]
pub struct ContentOptions {
    /// Office の文書（docx・xlsx・pptx）の中も探す
    pub office: bool,
    /// PDF の中も探す
    pub pdf: bool,
    /// 中身を探す・索引に足すファイルの名前のパターン（空ならすべて）
    pub patterns: crate::pattern::Patterns,
    /// これより大きなファイルは飛ばす
    pub max_size: u64,
    /// 並列に読むファイルの数
    pub threads: usize,
    /// 同時に読むファイルの大きさの見積もりと、中身の索引に使ってよいメモリ（バイト。0 は上限なし）
    pub memory_limit: u64,
    /// 裏の処理として優先度を下げる（Windows のバックグラウンド モード）
    pub background: bool,
    /// 1 ファイルで知らせる行の上限
    pub max_hits_per_file: u64,
}

impl Default for ContentOptions {
    fn default() -> Self {
        ContentOptions {
            office: true,
            pdf: true,
            patterns: crate::pattern::Patterns::new(crate::pattern::DEFAULT_CONTENT_PATTERNS),
            max_size: 1 << 30,
            threads: 8,
            memory_limit: 0,
            background: false,
            max_hits_per_file: 1000,
        }
    }
}

/// `files` の中身を探す。見つかった行は `hit` に渡す（`false` を返すか `progress` が `false` を返したら中止）。
pub fn search_content(
    cats: &[Catalog],
    files: &[FileRef],
    q: &yy_search::Query,
    opts: &ContentOptions,
    progress: &(dyn Fn(u64) -> bool + Sync),
    hit: &(dyn Fn(Hit) -> bool + Sync),
) -> io::Result<ContentStats> {
    search_inner(cats, files, q, opts, None, progress, hit)
}

/// 中身の索引（[`crate::fulltext`]）を使って探す。`indexes` は `cats` と同じ並び。索引で候補から外れた
/// ファイルは読まず、索引にない・変わったファイルは読んで索引に足す（保存は呼び出し側）。
pub fn search_content_indexed(
    cats: &[Catalog],
    files: &[FileRef],
    q: &yy_search::Query,
    opts: &ContentOptions,
    indexes: &mut [crate::fulltext::FtIndex],
    progress: &(dyn Fn(u64) -> bool + Sync),
    hit: &(dyn Fn(Hit) -> bool + Sync),
) -> io::Result<ContentStats> {
    search_inner(cats, files, q, opts, Some(indexes), progress, hit)
}

/// 中身の索引を場所（ルート）ごとに 1 つずつ読み込んで探す（同時にメモリに置く索引を 1 つにする）。
/// `load(何番目)` で索引を読み、探し終えたら `save(何番目, 索引)` で保存して手放す。
#[allow(clippy::too_many_arguments)]
pub fn search_content_indexed_by_root(
    cats: &[Catalog],
    files: &[FileRef],
    q: &yy_search::Query,
    opts: &ContentOptions,
    load: &dyn Fn(usize) -> crate::fulltext::FtIndex,
    save: &dyn Fn(usize, &mut crate::fulltext::FtIndex),
    progress: &(dyn Fn(u64) -> bool + Sync),
    hit: &(dyn Fn(Hit) -> bool + Sync),
) -> io::Result<ContentStats> {
    let mut total = ContentStats::default();
    let base = AtomicU64::new(0);
    for ri in 0..cats.len() {
        let mine: Vec<FileRef> = files.iter().copied().filter(|r| r.root == ri).collect();
        if mine.is_empty() {
            continue;
        }
        let mut ixs: Vec<crate::fulltext::FtIndex> = cats
            .iter()
            .map(|c| crate::fulltext::FtIndex::new(&c.root))
            .collect();
        ixs[ri] = load(ri);
        let b = base.load(Ordering::Relaxed);
        let r = search_inner(
            cats,
            &mine,
            q,
            opts,
            Some(&mut ixs),
            &|n| progress(b + n),
            hit,
        );
        save(ri, &mut ixs[ri]);
        drop(ixs);
        let st = r?;
        base.fetch_add(st.files + st.excluded, Ordering::Relaxed);
        total.files += st.files;
        total.matched_files += st.matched_files;
        total.hits += st.hits;
        total.skipped += st.skipped;
        total.excluded += st.excluded;
        total.pruned += st.pruned;
        total.indexed += st.indexed;
        total.not_indexed += st.not_indexed;
    }
    Ok(total)
}

/// 中身を読む（Office・PDF は文字列を取り出す。種類の選択は見ない）。
enum Loaded {
    Text(yy_buffer::Snapshot),
    Binary,
    TooBig,
}

fn load_text(e: &FileEntry, path: &std::path::Path, max_size: u64) -> Loaded {
    if e.meta.size > max_size {
        return Loaded::TooBig;
    }
    let snap = if crate::office::is_office(e.name()) {
        std::fs::File::open(path)
            .and_then(|mut f| crate::office::extract_text(&mut f))
            .ok()
            .map(yy_buffer::Snapshot::from_bytes)
    } else if crate::pdf::is_pdf(e.name()) {
        std::fs::read(path)
            .and_then(|d| crate::pdf::extract_text(&d))
            .ok()
            .map(yy_buffer::Snapshot::from_bytes)
    } else {
        yy_core::grep::load(path).ok().flatten()
    };
    snap.map_or(Loaded::Binary, Loaded::Text)
}

#[allow(clippy::too_many_arguments)]
fn search_inner(
    cats: &[Catalog],
    files: &[FileRef],
    q: &yy_search::Query,
    opts: &ContentOptions,
    indexes: Option<&mut [crate::fulltext::FtIndex]>,
    progress: &(dyn Fn(u64) -> bool + Sync),
    hit: &(dyn Fn(Hit) -> bool + Sync),
) -> io::Result<ContentStats> {
    use crate::fulltext::{FtState, GramSet, query_grams};
    let searcher = yy_search::Searcher::new(q)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.0))?;
    // 索引で絞る: (ファイル, 索引に足すか)
    let qgrams = if q.regex {
        None
    } else {
        query_grams(&q.pattern)
    };
    let mut pruned = 0u64;
    let mut index_skipped = 0u64;
    // 名指ししたパターンに合わないファイルは読まない（索引にも足さない）
    let named: Vec<FileRef> = files
        .iter()
        .copied()
        .filter(|r| {
            opts.patterns.is_empty() || opts.patterns.matches(cats[r.root].files[r.index].name())
        })
        .collect();
    let excluded = (files.len() - named.len()) as u64;
    let work: Vec<(FileRef, bool)> = match indexes.as_deref() {
        None => named.iter().map(|&r| (r, false)).collect(),
        Some(ix) => named
            .iter()
            .filter_map(|&r| {
                let e = &cats[r.root].files[r.index];
                match ix[r.root].lookup(e) {
                    Some((id, FtState::Text)) => {
                        if qgrams.as_ref().is_none_or(|g| ix[r.root].has_all(id, g)) {
                            Some((r, false))
                        } else {
                            pruned += 1;
                            None
                        }
                    }
                    Some((_, FtState::Binary)) => {
                        index_skipped += 1;
                        None
                    }
                    Some((_, FtState::TooBig(limit))) if opts.max_size <= limit => {
                        index_skipped += 1;
                        None
                    }
                    _ => Some((r, true)),
                }
            })
            .collect(),
    };
    // 中身の索引に使ってよいのはメモリの上限の半分まで（残りは読むファイルに）。超えたら足さない
    let index_room = (opts.memory_limit > 0).then(|| {
        let now: u64 = indexes
            .as_deref()
            .map_or(0, |ix| ix.iter().map(|i| i.approx_bytes()).sum());
        (opts.memory_limit / 2).saturating_sub(now)
    });
    let index_used = AtomicU64::new(0);
    let not_indexed = AtomicU64::new(0);
    let budget = crate::limits::MemoryBudget::new(opts.memory_limit / 2);
    let stop = AtomicBool::new(false);
    let done = AtomicU64::new(pruned + index_skipped);
    let matched = AtomicU64::new(0);
    let hits = AtomicU64::new(0);
    let skipped = AtomicU64::new(index_skipped);
    let indexed = AtomicU64::new(0);
    let new_entries: std::sync::Mutex<Vec<(FileRef, FtState, Vec<u16>)>> =
        std::sync::Mutex::new(Vec::new());
    let background = opts.background;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(opts.threads.max(1))
        .start_handler(move |_| {
            if background {
                crate::limits::enter_background();
            }
        })
        .build()
        .map_err(io::Error::other)?;
    pool.install(|| {
        work.par_iter().for_each(|&(r, add)| {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let e = &cats[r.root].files[r.index];
            let path = cats[r.root].path(e);
            let office = crate::office::is_office(e.name());
            let pdf = crate::pdf::is_pdf(e.name());
            // 読むのに要るメモリの見積もり（文字コードの変換・取り出しで大きくなる分を見込む）
            let need = e
                .meta
                .size
                .min(opts.max_size)
                .saturating_mul(if office || pdf { 4 } else { 2 });
            let Some(_lease) = budget.acquire(need, &|| stop.load(Ordering::Relaxed)) else {
                return;
            };

            let wanted = (!office || opts.office) && (!pdf || opts.pdf);
            // 索引に足さず、種類で探さないものは読まない
            let loaded = if !add && !wanted {
                Loaded::Binary
            } else {
                load_text(e, &path, opts.max_size)
            };
            if add {
                let (state, grams) = match &loaded {
                    Loaded::Text(snap) => {
                        let mut g = GramSet::default();
                        for c in snap.chunks(0..snap.len()) {
                            g.feed(c);
                        }
                        (FtState::Text, g.finish())
                    }
                    Loaded::Binary => (FtState::Binary, Vec::new()),
                    Loaded::TooBig => (FtState::TooBig(opts.max_size), Vec::new()),
                };
                // 索引に足す余裕がなければ、読んで探すだけにする
                let est = grams.len() as u64 * 4 + 128 + e.rel.len() as u64 * 2;
                let room_ok = index_room
                    .is_none_or(|room| index_used.fetch_add(est, Ordering::Relaxed) + est <= room);
                if room_ok {
                    indexed.fetch_add(1, Ordering::Relaxed);
                    new_entries.lock().unwrap().push((r, state, grams));
                } else {
                    not_indexed.fetch_add(1, Ordering::Relaxed);
                }
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            let snap = match loaded {
                Loaded::Text(s) if wanted => s,
                _ => {
                    skipped.fetch_add(1, Ordering::Relaxed);
                    if !progress(n) {
                        stop.store(true, Ordering::Relaxed);
                    }
                    return;
                }
            };
            let mut count = 0u64;
            yy_core::grep::grep_snapshot(
                &searcher,
                &snap,
                &path,
                &|| stop.load(Ordering::Relaxed),
                &mut |h| {
                    count += 1;
                    if !hit(Hit {
                        file: r,
                        line: h.line,
                        text: h.text,
                    }) {
                        stop.store(true, Ordering::Relaxed);
                        return false;
                    }
                    count < opts.max_hits_per_file
                },
            );
            if count > 0 {
                matched.fetch_add(1, Ordering::Relaxed);
                hits.fetch_add(count, Ordering::Relaxed);
            }
            if !progress(n) {
                stop.store(true, Ordering::Relaxed);
            }
        })
    });
    // 読んだものは中止しても索引に足す
    if let Some(ix) = indexes {
        for (r, state, grams) in new_entries.into_inner().unwrap() {
            ix[r.root].add(&cats[r.root].files[r.index], state, &grams);
        }
    }
    let stats = ContentStats {
        files: done.load(Ordering::Relaxed),
        matched_files: matched.load(Ordering::Relaxed),
        hits: hits.load(Ordering::Relaxed),
        skipped: skipped.load(Ordering::Relaxed),
        excluded,
        pruned,
        indexed: indexed.load(Ordering::Relaxed),
        not_indexed: not_indexed.load(Ordering::Relaxed),
    };
    if stop.load(Ordering::Relaxed) {
        return Err(crate::cancelled());
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::Meta;

    const NOW: i64 = 1_791_461_400_000_000_000; // 2026-10-08 12:10 UTC
    const JST: i64 = 9 * 3600;

    fn entry(rel: &str, size: u64, mtime: i64) -> FileEntry {
        FileEntry {
            rel: rel.to_owned(),
            meta: Meta {
                size,
                mtime,
                ..Meta::default()
            },
        }
    }

    #[test]
    fn parses_search_lines() {
        let q = parse_query(
            r#"見積 ext:xlsx;csv size:>1MB modified:>=2026-09-01 content:"税込 合計" path:案件A -path:old"#,
            NOW,
            JST,
        )
        .unwrap();
        assert!(matches!(&q.name[0], NameMatch::Words(w) if w == &["見積"]));
        assert_eq!(q.exts, ["xlsx", "csv"]);
        assert_eq!(q.size, (Some((1 << 20) + 1), None));
        assert_eq!(q.content.as_ref().unwrap().pattern, "税込 合計");
        assert_eq!(q.path, ["案件a"]);
        assert_eq!(q.not_path, ["old"]);
        // 2026-09-01 0:00 JST
        assert_eq!(
            q.modified.0,
            Some((days_from_civil(2026, 9, 1) * 86_400 - JST) * 1_000_000_000)
        );
        assert!(parse_query("size:abc", NOW, JST).is_err());
        assert!(parse_query("kind:謎", NOW, JST).is_err());
        let q = parse_query("kind:画像 modified:今日", NOW, JST).unwrap();
        assert!(q.exts.contains(&"png".to_owned()));
        let (a, b) = q.modified;
        assert!(a.unwrap() <= NOW && NOW < b.unwrap());
        assert_eq!(b.unwrap() - a.unwrap(), DAY);
        let q = parse_query("modified:30日より前 size:1KB..10KB", NOW, JST).unwrap();
        assert_eq!(q.modified, (None, Some(NOW - 30 * DAY)));
        assert_eq!(q.size, (Some(1024), Some(10240)));
        let q = parse_query("modified:2026-09", NOW, JST).unwrap();
        assert_eq!(q.modified.1.unwrap() - q.modified.0.unwrap(), 30 * DAY);
        assert_eq!(parse_size("1.5GB").unwrap(), 3 << 29);
    }

    #[test]
    fn filters_by_marks() {
        let q = parse_query("dupe:yes version:old", NOW, JST).unwrap();
        assert_eq!(q.needs_marks(), (true, true));
        assert_eq!(q.version, Some(VersionMark::Old));
        assert_eq!(parse_query("has:重複", NOW, JST).unwrap().dupe, Some(true));
        assert_eq!(
            parse_query("dupe:なし", NOW, JST).unwrap().dupe,
            Some(false)
        );
        assert!(parse_query("version:謎", NOW, JST).is_err());
        assert!(parse_query("dupe:maybe", NOW, JST).is_err());
        let r = |i| FileRef { root: 0, index: i };
        let mut m = Marks::default();
        m.dupes.extend([r(0), r(1)]);
        m.old.insert(r(1));
        m.newest.insert(r(2));
        let all = vec![r(0), r(1), r(2), r(3)];
        assert_eq!(q.apply_marks(all.clone(), &m), [r(1)]);
        let q = parse_query("dupe:no", NOW, JST).unwrap();
        assert_eq!(q.apply_marks(all.clone(), &m), [r(2), r(3)]);
        let q = parse_query("version:any", NOW, JST).unwrap();
        assert_eq!(q.apply_marks(all.clone(), &m), [r(1), r(2)]);
        let q = parse_query("version:最新", NOW, JST).unwrap();
        assert_eq!(q.apply_marks(all, &m), [r(2)]);
    }

    #[test]
    fn filters_catalogs() {
        let mut c = Catalog::default();
        let t = |d: i64| (days_from_civil(2026, 9, d) * 86_400) * 1_000_000_000;
        c.files = vec![
            entry("案件A/見積_v2.xlsx", 2 << 20, t(15)),
            entry("案件A/ﾐﾂﾓﾘ_old.xlsx", 10, t(2)),
            entry("案件B/見積.csv", 5 << 20, t(20)),
            entry("案件A/議事録.docx", 100, t(15)),
            entry("old/見積.xlsx", 5 << 20, t(20)),
        ];
        let rels = |q: &Query| -> Vec<String> {
            q.filter(std::slice::from_ref(&c))
                .iter()
                .map(|r| c.files[r.index].rel.clone())
                .collect()
        };
        let q = parse_query("見積 ext:xlsx size:>1MB -path:old", NOW, JST).unwrap();
        assert_eq!(rels(&q), ["案件A/見積_v2.xlsx"]);
        // 全角・半角・かなの違いを無視
        let q = parse_query("みつもり", NOW, JST).unwrap();
        assert_eq!(rels(&q), ["案件A/ﾐﾂﾓﾘ_old.xlsx"]);
        let q = parse_query("見積*.csv", NOW, JST).unwrap();
        assert_eq!(rels(&q), ["案件B/見積.csv"]);
        let q = parse_query(r"/^見積_v\d+\.xlsx$/", NOW, JST).unwrap();
        assert_eq!(rels(&q), ["案件A/見積_v2.xlsx"]);
        let q = parse_query("path:案件a modified:2026-09-15", NOW, 0).unwrap();
        assert_eq!(rels(&q), ["案件A/見積_v2.xlsx", "案件A/議事録.docx"]);
        let q = parse_query("kind:文書", NOW, JST).unwrap();
        assert_eq!(rels(&q), ["案件A/議事録.docx"]);
    }

    #[test]
    fn searches_contents_in_parallel() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::write(root.join("utf8.txt"), "一行目\n税込の金額\n").unwrap();
        let sjis = yy_encoding::encode_all(
            yy_encoding::Encoding::Cp932,
            "見出し\r\n税込 1,200 円\r\n".as_bytes(),
            yy_encoding::EscapeMode::Literal,
        )
        .unwrap();
        std::fs::write(root.join("sjis.csv"), sjis).unwrap();
        std::fs::write(root.join("bin.dat"), [0u8, 1, 2, 0, 0, 0, 7, 0]).unwrap();
        let docx = crate::office::tests::zip(&[(
            "word/document.xml",
            "<w:document><w:p><w:t>契約の税込金額</w:t></w:p></w:document>",
            true,
        )]);
        std::fs::write(root.join("契約.docx"), docx).unwrap();
        let pdf = crate::pdf::tests::build(&[
            ("<< /Type /Catalog /Pages 2 0 R >>", None),
            ("<< /Type /Pages /Kids [3 0 R] /Count 1 >>", None),
            (
                "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
                None,
            ),
            ("", Some(b"BT /F1 9 Tf <00010002> Tj ET")),
            ("<< /Type /Font /Subtype /Type0 /ToUnicode 6 0 R >>", None),
            (
                "",
                Some(b"1 begincodespacerange <0000> <FFFF> endcodespacerange 2 beginbfchar <0001> <7A0E> <0002> <8FBC> endbfchar"),
            ),
        ]);
        std::fs::write(root.join("請求.pdf"), pdf).unwrap();
        let c = crate::scan::scan(&crate::fs::Local, root, &Default::default(), &|_| true).unwrap();
        let all: Vec<FileRef> = (0..c.files.len())
            .map(|index| FileRef { root: 0, index })
            .collect();
        let q = yy_search::Query {
            pattern: "税込".into(),
            regex: false,
            case_sensitive: false,
            whole_word: false,
        };
        let hits = std::sync::Mutex::new(Vec::new());
        let st = search_content(
            std::slice::from_ref(&c),
            &all,
            &q,
            &ContentOptions::default(),
            &|_| true,
            &|h| {
                hits.lock()
                    .unwrap()
                    .push((c.files[h.file.index].rel.clone(), h.line, h.text));
                true
            },
        )
        .unwrap();
        let mut hits = hits.into_inner().unwrap();
        hits.sort();
        assert_eq!(
            hits,
            [
                ("sjis.csv".to_owned(), 2, "税込 1,200 円".to_owned()),
                ("utf8.txt".to_owned(), 2, "税込の金額".to_owned()),
                ("契約.docx".to_owned(), 1, "契約の税込金額".to_owned()),
                ("請求.pdf".to_owned(), 1, "税込".to_owned()),
            ]
        );
        // bin.dat は中身を探すパターンに合わないので読まない
        assert_eq!(
            (st.files, st.matched_files, st.skipped, st.excluded),
            (4, 4, 0, 1)
        );
        // パターンを絞ると、合うものだけを読む
        let only_csv = search_content(
            std::slice::from_ref(&c),
            &all,
            &q,
            &ContentOptions {
                patterns: crate::pattern::Patterns::new(&["*.csv"]),
                ..ContentOptions::default()
            },
            &|_| true,
            &|_| true,
        )
        .unwrap();
        assert_eq!(
            (only_csv.files, only_csv.matched_files, only_csv.excluded),
            (1, 1, 4)
        );
        // Office を探さない
        let st = search_content(
            std::slice::from_ref(&c),
            &all,
            &q,
            &ContentOptions {
                office: false,
                pdf: false,
                ..ContentOptions::default()
            },
            &|_| true,
            &|_| true,
        )
        .unwrap();
        assert_eq!(st.matched_files, 2);
        // 中止
        let e = search_content(
            std::slice::from_ref(&c),
            &all,
            &q,
            &ContentOptions::default(),
            &|_| false,
            &|_| true,
        )
        .unwrap_err();
        assert!(crate::is_cancelled(&e));
    }

    #[test]
    fn indexed_search_matches_plain_search_and_reads_less() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let words = [
            "税込", "金額", "見積", "合計", "Total", "請求", "納品", "単価",
        ];
        for i in 0..40usize {
            let body: String = (0..6)
                .map(|k| words[(i * 7 + k * 3) % words.len()])
                .collect::<Vec<_>>()
                .join(if i % 2 == 0 { " " } else { "、" });
            std::fs::write(
                root.join(format!("f{i:02}.txt")),
                format!("{body}\n二行目 {i}\n"),
            )
            .unwrap();
        }
        std::fs::write(root.join("bin.dat"), [0u8, 1, 0, 2]).unwrap();
        let mut c =
            crate::scan::scan(&crate::fs::Local, root, &Default::default(), &|_| true).unwrap();
        let all = |c: &Catalog| -> Vec<FileRef> {
            (0..c.files.len())
                .map(|index| FileRef { root: 0, index })
                .collect()
        };
        let run = |c: &Catalog, pat: &str, ix: Option<&mut [crate::fulltext::FtIndex]>| {
            let q = yy_search::Query {
                pattern: pat.into(),
                regex: false,
                case_sensitive: false,
                whole_word: false,
            };
            let hits = std::sync::Mutex::new(Vec::new());
            let cats = std::slice::from_ref(c);
            let rec = |h: Hit| {
                hits.lock()
                    .unwrap()
                    .push((c.files[h.file.index].rel.clone(), h.line));
                true
            };
            let st = match ix {
                Some(ix) => search_content_indexed(
                    cats,
                    &all(c),
                    &q,
                    &ContentOptions::default(),
                    ix,
                    &|_| true,
                    &rec,
                ),
                None => search_content(
                    cats,
                    &all(c),
                    &q,
                    &ContentOptions::default(),
                    &|_| true,
                    &rec,
                ),
            }
            .unwrap();
            let mut h = hits.into_inner().unwrap();
            h.sort();
            (h, st)
        };
        let mut ix = vec![crate::fulltext::FtIndex::new(root)];
        // 1 回目は全部読んで索引に足す
        let (h1, st1) = run(&c, "税込", Some(&mut ix));
        assert_eq!(h1, run(&c, "税込", None).0);
        // bin.dat はパターンに合わないので索引に足さない
        assert_eq!((st1.indexed, st1.pruned, st1.excluded), (40, 0, 1));
        // 2 回目からは候補だけを読む。結果は索引なしと同じ
        for pat in [
            "税込",
            "見積 合計",
            "total",
            "込金",
            "二行目 3",
            "存在しない語",
        ] {
            let (h, st) = run(&c, pat, Some(&mut ix));
            assert_eq!(h, run(&c, pat, None).0, "{pat}");
            assert_eq!(st.indexed, 0, "{pat}");
            assert!(st.pruned > 0, "{pat}: {st:?}");
            assert_eq!(st.files + st.excluded, 41, "{pat}");
        }
        // 1 文字・正規表現は索引を使わない（全部読む）が、結果は同じ
        let (h, st) = run(&c, "税", Some(&mut ix));
        assert_eq!(h, run(&c, "税", None).0);
        assert_eq!(st.pruned, 0);
        // 変わったファイルは読み直して足す
        std::fs::write(root.join("f00.txt"), "新しい語 ユニーク").unwrap();
        crate::fs::Fs::set_mtime(
            &crate::fs::Local,
            &root.join("f00.txt"),
            1_900_000_000_000_000_000,
        )
        .unwrap();
        c = crate::scan::scan(&crate::fs::Local, root, &Default::default(), &|_| true).unwrap();
        let (h, st) = run(&c, "ユニーク", Some(&mut ix));
        assert_eq!(h, [("f00.txt".to_owned(), 1)]);
        assert_eq!(st.indexed, 1);
        // 保存して読み直しても同じ
        let p = d.path().join("ix.fti");
        ix[0].retain_catalog(&c);
        ix[0].save(&p).unwrap();
        let mut back = vec![crate::fulltext::FtIndex::load(&p).unwrap()];
        let (h, st) = run(&c, "見積", Some(&mut back));
        assert_eq!(h, run(&c, "見積", None).0);
        assert_eq!(st.indexed, 0);
    }

    #[test]
    fn indexed_search_never_misses_on_random_texts() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let alphabet: Vec<char> = "aAbB税込ｱｲ ß\nσΣ".chars().collect();
        let mut seed = 12345u64;
        let mut rnd = |n: usize| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as usize) % n
        };
        for i in 0..30 {
            let len = 5 + rnd(40);
            let t: String = (0..len).map(|_| alphabet[rnd(alphabet.len())]).collect();
            std::fs::write(root.join(format!("r{i}.txt")), t).unwrap();
        }
        let c = crate::scan::scan(&crate::fs::Local, root, &Default::default(), &|_| true).unwrap();
        let all: Vec<FileRef> = (0..c.files.len())
            .map(|index| FileRef { root: 0, index })
            .collect();
        let mut ix = vec![crate::fulltext::FtIndex::new(root)];
        let find = |pat: &str, cs: bool, ix: Option<&mut [crate::fulltext::FtIndex]>| {
            let q = yy_search::Query {
                pattern: pat.into(),
                regex: false,
                case_sensitive: cs,
                whole_word: false,
            };
            let got = std::sync::Mutex::new(Vec::new());
            let cats = std::slice::from_ref(&c);
            let rec = |h: Hit| {
                got.lock().unwrap().push((h.file.index, h.line));
                true
            };
            match ix {
                Some(ix) => search_content_indexed(
                    cats,
                    &all,
                    &q,
                    &ContentOptions::default(),
                    ix,
                    &|_| true,
                    &rec,
                ),
                None => search_content(cats, &all, &q, &ContentOptions::default(), &|_| true, &rec),
            }
            .unwrap();
            let mut v = got.into_inner().unwrap();
            v.sort();
            v
        };
        let _ = find("aa", false, Some(&mut ix)); // 索引を作る
        for _ in 0..200 {
            let len = 2 + rnd(2);
            let pat: String = (0..len)
                .map(|_| alphabet[rnd(alphabet.len())])
                .filter(|c| *c != '\n')
                .collect();
            if pat.chars().count() < 2 {
                continue;
            }
            let cs = rnd(2) == 0;
            assert_eq!(
                find(&pat, cs, Some(&mut ix)),
                find(&pat, cs, None),
                "{pat:?} cs={cs}"
            );
        }
    }

    #[test]
    fn respects_memory_limit_and_searches_root_by_root() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        for (root, n) in [(&a, 12usize), (&b, 8)] {
            std::fs::create_dir_all(root).unwrap();
            for i in 0..n {
                let body = format!("{} 税込 {i}\n", "あいうえおかきくけこ".repeat(50 + i));
                std::fs::write(root.join(format!("{i}.txt")), body).unwrap();
            }
        }
        let cats: Vec<Catalog> = [&a, &b]
            .iter()
            .map(|r| {
                crate::scan::scan(&crate::fs::Local, r, &Default::default(), &|_| true).unwrap()
            })
            .collect();
        let files: Vec<FileRef> = cats
            .iter()
            .enumerate()
            .flat_map(|(ri, c)| (0..c.files.len()).map(move |index| FileRef { root: ri, index }))
            .collect();
        let q = yy_search::Query {
            pattern: "税込".into(),
            regex: false,
            case_sensitive: false,
            whole_word: false,
        };
        let store = d.path().join("ft");
        let load = |ri: usize| {
            crate::fulltext::FtIndex::load(&crate::fulltext::path_for(&store, &cats[ri].root))
                .unwrap_or_else(|_| crate::fulltext::FtIndex::new(&cats[ri].root))
        };
        let loaded = std::sync::Mutex::new(Vec::new());
        let save = |ri: usize, ix: &mut crate::fulltext::FtIndex| {
            loaded.lock().unwrap().push(ri);
            ix.save(&crate::fulltext::path_for(&store, &cats[ri].root))
                .unwrap();
        };
        // メモリの上限がとても小さいと、索引に足さずに探すだけ（結果は同じ）
        let tiny = ContentOptions {
            memory_limit: 4096,
            background: true,
            threads: 2,
            ..ContentOptions::default()
        };
        let hits = AtomicU64::new(0);
        let st = search_content_indexed_by_root(
            &cats,
            &files,
            &q,
            &tiny,
            &load,
            &save,
            &|_| true,
            &|_| {
                hits.fetch_add(1, Ordering::Relaxed);
                true
            },
        )
        .unwrap();
        assert_eq!(hits.load(Ordering::Relaxed), 20);
        assert_eq!(st.matched_files, 20);
        assert!(st.not_indexed > 0, "{st:?}");
        // 場所ごとに 1 つずつ読み込んで保存した
        assert_eq!(*loaded.lock().unwrap(), [0, 1]);
        // 上限が十分なら全部足す
        let roomy = ContentOptions {
            memory_limit: 64 << 20,
            ..tiny
        };
        let st = search_content_indexed_by_root(
            &cats,
            &files,
            &q,
            &roomy,
            &load,
            &save,
            &|_| true,
            &|_| true,
        )
        .unwrap();
        assert_eq!(st.not_indexed, 0);
        assert_eq!(st.matched_files, 20);
        let st = search_content_indexed_by_root(
            &cats,
            &files,
            &q,
            &roomy,
            &load,
            &save,
            &|_| true,
            &|_| true,
        )
        .unwrap();
        assert_eq!((st.indexed, st.matched_files), (0, 20));
    }
}

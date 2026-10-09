//! 似た名前のファイルと新しい版の判定（18 章 5）。
//!
//! 1. ファイル名から版・日付・新しさの言葉・写しの印を取り除いて「元の名前」を作る（[`parse_name`]）。
//! 2. 拡張子の仲間ごとに、元の名前の似ている度合い（文字の 2-gram の Dice 係数と Jaro-Winkler）で
//!    グループにまとめる。総当たりにならないよう、MinHash の LSH で候補の組を作ってから比べる。
//! 3. グループの中を「名前の版・日付 → 新しさの言葉 → 更新日時 → 大きさ」の順に並べ、いちばん上を
//!    最新の提案にする。手がかりが食い違えば「自信が低い」にして理由を出す。

use std::collections::{HashMap, HashSet};

use unicode_normalization::UnicodeNormalization;

use crate::dupes::FileRef;
use crate::scan::{Catalog, FileEntry};

/// 名前から読み取ったこと。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameInfo {
    /// 元の名前（比べる形。空白で区切った語）
    pub base: String,
    /// 拡張子の仲間（`docx` と `doc` は `doc`）
    pub family: String,
    /// 版の番号（`v1.3` は `[1, 3]`）
    pub version: Option<Vec<u32>>,
    /// 名前の日付（`yyyymmddhhmm` の数）
    pub date: Option<u64>,
    /// 新しさ（新しさの言葉は +、古さの言葉は −）
    pub fresh: i32,
    /// 写しの印（`- コピー`・`(1)` など）
    pub copy: bool,
}

/// 新しさの言葉と強さ。
const FRESH: &[(&str, i32)] = &[
    ("最新", 3),
    ("最終", 3),
    ("最終版", 3),
    ("確定", 3),
    ("確定版", 3),
    ("決定", 2),
    ("決定版", 2),
    ("正式", 2),
    ("完成", 2),
    ("final", 3),
    ("latest", 3),
    ("fixed", 1),
    ("new", 1),
    ("修正", 1),
    ("修正版", 1),
    ("改", 1),
    ("改訂", 1),
    ("差替", 1),
    ("差替え", 1),
    ("差し替え", 1),
];

/// 古さの言葉と強さ。
const OLD: &[(&str, i32)] = &[
    ("旧", 3),
    ("old", 3),
    ("bak", 3),
    ("backup", 3),
    ("古い", 3),
    ("draft", 2),
    ("下書き", 2),
    ("案", 1),
    ("仮", 1),
    ("tmp", 2),
    ("temp", 2),
];

/// 写しの言葉（[`fold`] した形。カタカナはひらがな）。
const COPY: &[&str] = &["こぴー", "のこぴー", "copy", "copyof", "複製"];

/// 拡張子の仲間。
pub fn family(ext: &str) -> String {
    let e = ext.to_ascii_lowercase();
    match e.as_str() {
        "doc" | "docx" | "docm" => "doc",
        "xls" | "xlsx" | "xlsm" | "xlsb" => "xls",
        "ppt" | "pptx" | "pptm" => "ppt",
        "jpg" | "jpeg" | "jfif" => "jpg",
        "tif" | "tiff" => "tif",
        "htm" | "html" => "html",
        "md" | "markdown" => "md",
        "txt" | "text" => "txt",
        _ => return e,
    }
    .to_owned()
}

/// 比べる形にそろえる（NFKC・小文字・カタカナはひらがなに）。
pub fn fold(s: &str) -> String {
    s.nfkc()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            // カタカナ（ァ〜ヶ）→ ひらがな
            '\u{30A1}'..='\u{30F6}' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
            c => c,
        })
        .collect()
}

fn is_sep(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '_' | '-'
                | '.'
                | '・'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '【'
                | '】'
                | '「'
                | '」'
                | '『'
                | '』'
                | '〔'
                | '〕'
                | ','
                | '、'
                | '~'
                | '+'
                | '#'
                | '&'
        )
}

/// 日付らしい数字の並び（`yyyymmdd`・`yymmdd`）なら `yyyymmdd`。
fn date8(d: &str) -> Option<u64> {
    let ok = |y: u64, m: u64, dd: u64| {
        (1980..=2099).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&dd)
    };
    match d.len() {
        8 => {
            let v: u64 = d.parse().ok()?;
            let (y, m, dd) = (v / 10000, v / 100 % 100, v % 100);
            ok(y, m, dd).then_some(v)
        }
        6 => {
            let v: u64 = d.parse().ok()?;
            let (y, m, dd) = (2000 + v / 10000, v / 100 % 100, v % 100);
            ok(y, m, dd).then_some(y * 10000 + m * 100 + dd)
        }
        _ => None,
    }
}

/// 名前（拡張子を含む）を読む。
pub fn parse_name(name: &str) -> NameInfo {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.len() <= 8 && !e.contains(' ') => (s, e),
        _ => (name, ""),
    };
    let mut info = NameInfo {
        family: family(ext),
        ..NameInfo::default()
    };
    let s = fold(stem);
    // 「2026年9月1日」「2026-09-01」「2026.9.1」の日付を先に取り出す
    let chars: Vec<char> = s.chars().collect();
    let mut rest = String::new();
    let mut i = 0;
    let digits_at = |i: usize| -> usize {
        let mut j = i;
        while j < chars.len() && chars[j].is_ascii_digit() {
            j += 1;
        }
        j - i
    };
    while i < chars.len() {
        let n = digits_at(i);
        if n == 4 && (i == 0 || !chars[i - 1].is_ascii_digit()) {
            // yyyy<区切り>m<区切り>d
            let y: u64 = chars[i..i + 4]
                .iter()
                .collect::<String>()
                .parse()
                .unwrap_or(0);
            let mut j = i + 4;
            let sep_ok = |c: char| matches!(c, '-' | '.' | '年' | '/');
            if j < chars.len() && sep_ok(chars[j]) {
                let m_len = digits_at(j + 1);
                if (1..=2).contains(&m_len) {
                    let m: u64 = chars[j + 1..j + 1 + m_len]
                        .iter()
                        .collect::<String>()
                        .parse()
                        .unwrap_or(0);
                    let k = j + 1 + m_len;
                    if k < chars.len() && matches!(chars[k], '-' | '.' | '月' | '/') {
                        let d_len = digits_at(k + 1);
                        if (1..=2).contains(&d_len) {
                            let d: u64 = chars[k + 1..k + 1 + d_len]
                                .iter()
                                .collect::<String>()
                                .parse()
                                .unwrap_or(0);
                            if (1980..=2099).contains(&y)
                                && (1..=12).contains(&m)
                                && (1..=31).contains(&d)
                            {
                                j = k + 1 + d_len;
                                if j < chars.len() && chars[j] == '日' {
                                    j += 1;
                                }
                                info.date = Some((y * 10000 + m * 100 + d) * 10000);
                                rest.push(' ');
                                i = j;
                                continue;
                            }
                        }
                    }
                }
            }
        }
        rest.push(chars[i]);
        i += 1;
    }
    // 語に分ける
    let tokens: Vec<String> = rest
        .split(is_sep)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect();
    let mut base: Vec<String> = Vec::new();
    let mut k = 0;
    let last = tokens.len().saturating_sub(1);
    while k < tokens.len() {
        let t = tokens[k].as_str();
        let next = tokens.get(k + 1).map(String::as_str);
        let num = |x: &str| x.parse::<u32>().ok();
        // 版: v2・v1.3（`.` で分かれた続きの数字）・ver3・rev4・rev 4・r5・第2版・2版
        let ver_prefix = ["ver", "rev", "version", "v", "r"].iter().find_map(|p| {
            t.strip_prefix(p)
                .filter(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()))
        });
        if let Some(n) = ver_prefix.and_then(num) {
            let mut v = vec![n];
            // v1.3 は「v1」「3」に分かれる
            let mut j = k + 1;
            while let Some(x) = tokens.get(j).and_then(|x| num(x)) {
                if tokens[j].len() > 3 {
                    break;
                }
                v.push(x);
                j += 1;
            }
            info.version = Some(v);
            k = j;
            continue;
        }
        if matches!(t, "ver" | "rev" | "version" | "v")
            && let Some(n) = next.and_then(num)
        {
            info.version = Some(vec![n]);
            k += 2;
            continue;
        }
        if let Some(n) = t
            .strip_prefix('第')
            .and_then(|r| r.strip_suffix('版'))
            .and_then(num)
            .or_else(|| t.strip_suffix('版').and_then(num))
        {
            info.version = Some(vec![n]);
            k += 1;
            continue;
        }
        // 日付・時刻（数字だけの語）
        if t.chars().all(|c| c.is_ascii_digit()) {
            if let Some(d) = date8(t) {
                info.date = Some(d * 10000);
                k += 1;
                // 続く 4 桁は時刻
                if let Some(hm) = tokens
                    .get(k)
                    .filter(|x| x.len() == 4)
                    .and_then(|x| x.parse::<u64>().ok())
                    && hm % 100 < 60
                    && hm / 100 < 24
                {
                    info.date = Some(d * 10000 + hm);
                    k += 1;
                }
                continue;
            }
            // 12 桁（yyyymmddhhmm）
            if t.len() == 12
                && let Some(d) = date8(&t[..8])
            {
                info.date = Some(d * 10000 + t[8..].parse::<u64>().unwrap_or(0));
                k += 1;
                continue;
            }
            // 最後の短い番号は版（`報告書_02`）。括弧の番号（`(2)`）は写し
            if t.len() <= 3 && k == last && k > 0 {
                let paren = name.contains(&format!("({t})")) || name.contains(&format!("（{t}）"));
                if paren {
                    info.copy = true;
                } else if info.version.is_none() {
                    info.version = num(t).map(|n| vec![n]);
                }
                k += 1;
                continue;
            }
        }
        if let Some((_, w)) = FRESH.iter().find(|(w, _)| *w == t) {
            info.fresh += w;
            k += 1;
            continue;
        }
        if let Some((_, w)) = OLD.iter().find(|(w, _)| *w == t) {
            info.fresh -= w;
            k += 1;
            continue;
        }
        if COPY.contains(&t) || (t == "copy" && next == Some("of")) {
            info.copy = true;
            k += if t == "copy" && next == Some("of") {
                2
            } else {
                1
            };
            continue;
        }
        // 語の中に含まれる新しさの言葉（`報告書最新`）
        let mut word = t.to_owned();
        for (w, n) in FRESH {
            if word.chars().count() > w.chars().count() && word.ends_with(w) {
                word.truncate(word.len() - w.len());
                info.fresh += n;
                break;
            }
        }
        for (w, n) in OLD {
            if word.chars().count() > w.chars().count() && word.ends_with(w) && !w.is_ascii() {
                word.truncate(word.len() - w.len());
                info.fresh -= n;
                break;
            }
        }
        if let Some(stripped) = word.strip_suffix("のこぴー") {
            word = stripped.to_owned();
            info.copy = true;
        }
        base.push(word);
        k += 1;
    }
    info.base = base.join(" ");
    info
}

fn bigrams(s: &str) -> Vec<(char, char)> {
    let c: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    if c.len() == 1 {
        return vec![(c[0], '\0')];
    }
    c.windows(2).map(|w| (w[0], w[1])).collect()
}

/// 文字の 2-gram の Dice 係数。
fn dice(a: &str, b: &str) -> f64 {
    let (x, y) = (bigrams(a), bigrams(b));
    if x.is_empty() || y.is_empty() {
        return if a == b { 1.0 } else { 0.0 };
    }
    let mut counts: HashMap<(char, char), i32> = HashMap::new();
    for g in &x {
        *counts.entry(*g).or_default() += 1;
    }
    let mut common = 0;
    for g in &y {
        if let Some(c) = counts.get_mut(g)
            && *c > 0
        {
            *c -= 1;
            common += 1;
        }
    }
    2.0 * common as f64 / (x.len() + y.len()) as f64
}

/// Jaro-Winkler の似ている度合い。
fn jaro_winkler(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().filter(|c| !c.is_whitespace()).collect();
    let b: Vec<char> = b.chars().filter(|c| !c.is_whitespace()).collect();
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let window = (a.len().max(b.len()) / 2).saturating_sub(1);
    let mut am = vec![false; a.len()];
    let mut bm = vec![false; b.len()];
    let mut m = 0usize;
    for i in 0..a.len() {
        let lo = i.saturating_sub(window);
        let hi = (i + window + 1).min(b.len());
        for j in lo..hi {
            if !bm[j] && a[i] == b[j] {
                am[i] = true;
                bm[j] = true;
                m += 1;
                break;
            }
        }
    }
    if m == 0 {
        return 0.0;
    }
    let mut t = 0usize;
    let mut j = 0;
    for i in 0..a.len() {
        if am[i] {
            while !bm[j] {
                j += 1;
            }
            if a[i] != b[j] {
                t += 1;
            }
            j += 1;
        }
    }
    let m = m as f64;
    let jaro = (m / a.len() as f64 + m / b.len() as f64 + (m - t as f64 / 2.0) / m) / 3.0;
    let prefix = a.iter().zip(&b).take(4).take_while(|(x, y)| x == y).count() as f64;
    jaro + prefix * 0.1 * (1.0 - jaro)
}

/// 元の名前どうしの似ている度合い（0〜1）。
pub fn similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    0.6 * dice(a, b) + 0.4 * jaro_winkler(a, b)
}

/// 似たファイルの設定。
#[derive(Clone, Debug)]
pub struct SimilarOptions {
    /// 似ている度合いのしきい値
    pub threshold: f64,
    /// 大きさがこの倍以上違えば、名前が似ていても別のグループ
    pub max_size_ratio: f64,
}

impl Default for SimilarOptions {
    fn default() -> Self {
        SimilarOptions {
            threshold: 0.85,
            max_size_ratio: 10.0,
        }
    }
}

/// 判定の自信。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Confidence::High => "高",
            Confidence::Medium => "中",
            Confidence::Low => "低",
        }
    }
}

/// グループの 1 ファイル。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub file: FileRef,
    pub info: NameInfo,
}

/// 版のグループ（新しい順。先頭が最新の提案）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionGroup {
    pub members: Vec<Member>,
    pub confidence: Confidence,
    /// 判定の理由
    pub reason: String,
}

/// MinHash の数と、帯の数（1 つの帯に `MINHASH / BANDS` 個）。
const MINHASH: usize = 16;
const BANDS: usize = 8;
/// 1 つの候補のバケツの大きさの上限（ありふれた名前で組が爆発しないように）。
const BUCKET_LIMIT: usize = 400;

fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^ (x >> 33)
}

fn minhash(base: &str) -> [u64; MINHASH] {
    let mut out = [u64::MAX; MINHASH];
    for (a, b) in bigrams(base) {
        let g = ((a as u64) << 32) | b as u64;
        for (k, o) in out.iter_mut().enumerate() {
            *o = (*o).min(mix(g ^ (k as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)));
        }
    }
    out
}

struct Dsu(Vec<usize>);

impl Dsu {
    fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.0[r] != r {
            r = self.0[r];
        }
        let mut x = x;
        while self.0[x] != r {
            let n = self.0[x];
            self.0[x] = r;
            x = n;
        }
        r
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b)] = a.min(b);
        }
    }
}

/// 似た名前のファイルのグループ（2 つ以上のもの）を求める。
pub fn find_versions(cats: &[Catalog], opts: &SimilarOptions) -> Vec<VersionGroup> {
    let mut refs: Vec<FileRef> = Vec::new();
    let mut infos: Vec<NameInfo> = Vec::new();
    for (ri, c) in cats.iter().enumerate() {
        for (fi, f) in c.files.iter().enumerate() {
            let info = parse_name(f.name());
            if info.base.is_empty() {
                continue;
            }
            refs.push(FileRef {
                root: ri,
                index: fi,
            });
            infos.push(info);
        }
    }
    let n = refs.len();
    let mut dsu = Dsu((0..n).collect());
    let entry = |i: usize| -> &FileEntry { &cats[refs[i].root].files[refs[i].index] };
    // 元の名前が同じものはまとめる
    let mut exact: HashMap<(&str, &str), usize> = HashMap::new();
    for (i, info) in infos.iter().enumerate() {
        let key = (info.family.as_str(), info.base.as_str());
        match exact.get(&key) {
            Some(&j) => {
                if size_ok(entry(i).meta.size, entry(j).meta.size, opts.max_size_ratio) {
                    dsu.union(i, j);
                }
            }
            None => {
                exact.insert(key, i);
            }
        }
    }
    // 似ている候補（LSH）。代表（元の名前ごとに 1 つ）どうしで比べる
    let reps: Vec<usize> = exact.values().copied().collect();
    let mut buckets: HashMap<(&str, usize, [u64; MINHASH / BANDS]), Vec<usize>> = HashMap::new();
    for &i in &reps {
        let mh = minhash(&infos[i].base);
        for b in 0..BANDS {
            let mut key = [0u64; MINHASH / BANDS];
            key.copy_from_slice(&mh[b * MINHASH / BANDS..(b + 1) * MINHASH / BANDS]);
            buckets
                .entry((infos[i].family.as_str(), b, key))
                .or_default()
                .push(i);
        }
    }
    let mut tried: HashSet<(usize, usize)> = HashSet::new();
    for v in buckets.values() {
        if v.len() < 2 || v.len() > BUCKET_LIMIT {
            continue;
        }
        for x in 0..v.len() {
            for y in x + 1..v.len() {
                let (a, b) = (v[x].min(v[y]), v[x].max(v[y]));
                if !tried.insert((a, b)) {
                    continue;
                }
                let mut s = similarity(&infos[a].base, &infos[b].base);
                // 同じフォルダのものは少し上げる
                if entry(a).dir() == entry(b).dir() && refs[a].root == refs[b].root {
                    s += 0.05;
                }
                if s >= opts.threshold
                    && size_ok(entry(a).meta.size, entry(b).meta.size, opts.max_size_ratio)
                {
                    dsu.union(a, b);
                }
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        groups.entry(dsu.find(i)).or_default().push(i);
    }
    let mut out: Vec<VersionGroup> = groups
        .into_values()
        .filter(|g| g.len() > 1)
        .map(|g| {
            let members: Vec<Member> = g
                .into_iter()
                .map(|i| Member {
                    file: refs[i],
                    info: infos[i].clone(),
                })
                .collect();
            rank(cats, members)
        })
        .collect();
    out.sort_by(|a, b| {
        b.members
            .len()
            .cmp(&a.members.len())
            .then_with(|| a.members[0].file.cmp(&b.members[0].file))
    });
    out
}

/// 1 つのファイルの別の版を探す（18 章 8.4「このファイルの別の版を探す」）。同じ拡張子の仲間で、元の
/// 名前が同じか、しきい値以上に似ているファイルを集めて順位を付ける。見つからなければ `None`。
pub fn versions_of(
    cats: &[Catalog],
    target: FileRef,
    opts: &SimilarOptions,
) -> Option<VersionGroup> {
    let t = cats.get(target.root)?.files.get(target.index)?;
    let ti = parse_name(t.name());
    if ti.base.is_empty() {
        return None;
    }
    let mut members = vec![Member {
        file: target,
        info: ti.clone(),
    }];
    for (ri, c) in cats.iter().enumerate() {
        for (fi, f) in c.files.iter().enumerate() {
            let r = FileRef {
                root: ri,
                index: fi,
            };
            if r == target {
                continue;
            }
            let info = parse_name(f.name());
            if info.family != ti.family || info.base.is_empty() {
                continue;
            }
            let mut s = similarity(&info.base, &ti.base);
            if f.dir() == t.dir() && ri == target.root {
                s += 0.05;
            }
            if s >= opts.threshold && size_ok(f.meta.size, t.meta.size, opts.max_size_ratio) {
                members.push(Member { file: r, info });
            }
        }
    }
    (members.len() > 1).then(|| rank(cats, members))
}

fn size_ok(a: u64, b: u64, ratio: f64) -> bool {
    let (lo, hi) = (a.min(b).max(1) as f64, a.max(b).max(1) as f64);
    hi / lo < ratio
}

/// 更新日時の差がこれ以下なら同じとみなす（2 秒）。
const MTIME_TOL: i64 = 2_000_000_000;

/// グループの中を新しい順に並べて、自信と理由を決める（18 章 5.3）。
pub fn rank(cats: &[Catalog], mut members: Vec<Member>) -> VersionGroup {
    let meta = |m: &Member| cats[m.file.root].files[m.file.index].meta;
    let name_key = |m: &Member| (m.info.version.clone(), m.info.date);
    members.sort_by(|a, b| {
        let (ma, mb) = (meta(a), meta(b));
        name_key(b)
            .cmp(&name_key(a))
            .then(b.info.fresh.cmp(&a.info.fresh))
            .then(a.info.copy.cmp(&b.info.copy))
            .then(mb.mtime.cmp(&ma.mtime))
            .then(mb.size.cmp(&ma.size))
            .then(a.file.cmp(&b.file))
    });
    let (top, second) = (&members[0], &members[1]);
    let (mt, ms) = (meta(top), meta(second));
    let name_decides = name_key(top) != name_key(second);
    let mtime_agrees = mt.mtime + MTIME_TOL >= ms.mtime;
    let describe = |m: &Member| {
        let mut parts = Vec::new();
        if let Some(v) = &m.info.version {
            parts.push(format!(
                "版 {}",
                v.iter().map(u32::to_string).collect::<Vec<_>>().join(".")
            ));
        }
        if let Some(d) = m.info.date {
            parts.push(format!("日付 {}", d / 10000));
        }
        parts.join("・")
    };
    let (confidence, reason) = if name_decides {
        if mtime_agrees {
            (
                Confidence::High,
                format!("名前の{}が新しい（更新日時も新しい）", describe(top)),
            )
        } else {
            let days = (ms.mtime - mt.mtime) / 86_400_000_000_000;
            (
                Confidence::Low,
                format!(
                    "名前の{}が新しいが、更新日時は「{}」の方が{}新しい",
                    describe(top),
                    cats[second.file.root].files[second.file.index].name(),
                    if days > 0 {
                        format!(" {days} 日")
                    } else {
                        String::new()
                    }
                ),
            )
        }
    } else if top.info.fresh != second.info.fresh {
        if mtime_agrees {
            (Confidence::Medium, "名前に新しさの言葉がある".to_owned())
        } else {
            (
                Confidence::Low,
                "名前に新しさの言葉があるが、更新日時は古い".to_owned(),
            )
        }
    } else if (mt.mtime - ms.mtime).abs() > MTIME_TOL {
        (Confidence::Medium, "更新日時が新しい".to_owned())
    } else if mt.size != ms.size {
        (
            Confidence::Low,
            "大きさが大きい（ほかの手がかりがない）".to_owned(),
        )
    } else {
        (Confidence::Low, "見分ける手がかりがない".to_owned())
    };
    VersionGroup {
        members,
        confidence,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::Meta;

    fn p(name: &str) -> NameInfo {
        parse_name(name)
    }

    #[test]
    fn parses_version_marks() {
        let i = p("報告書_v2.docx");
        assert_eq!((i.base.as_str(), i.family.as_str()), ("報告書", "doc"));
        assert_eq!(i.version, Some(vec![2]));
        assert_eq!(p("報告書 v1.3.docx").version, Some(vec![1, 3]));
        assert_eq!(p("報告書_ver3.doc").version, Some(vec![3]));
        assert_eq!(p("仕様書 rev.4.xlsx").version, Some(vec![4]));
        assert_eq!(p("仕様書_R5.xlsx").version, Some(vec![5]));
        assert_eq!(p("規程（第2版）.pdf").version, Some(vec![2]));
        assert_eq!(p("見積_02.xlsx").version, Some(vec![2]));
        assert_eq!(p("見積_02.xlsx").base, "見積");
        // 日付
        assert_eq!(p("20260901_報告書.docx").date, Some(202609010000));
        assert_eq!(p("報告書_2026-09-01.docx").date, Some(202609010000));
        assert_eq!(p("報告書 2026年9月1日.docx").date, Some(202609010000));
        assert_eq!(p("報告書_260901.docx").date, Some(202609010000));
        assert_eq!(p("報告書_20260901_1530.docx").date, Some(202609011530));
        assert_eq!(p("報告書_2026.9.1.docx").base, "報告書");
        // 言葉
        assert!(p("報告書_最新.docx").fresh > 0);
        assert!(p("報告書_最終版.docx").fresh > 0);
        assert!(p("報告書【確定】.docx").fresh > 0);
        assert!(p("報告書_FINAL.docx").fresh > 0);
        assert!(p("報告書_旧.docx").fresh < 0);
        assert!(p("報告書_old.docx").fresh < 0);
        assert!(p("報告書_下書き.docx").fresh < 0);
        assert_eq!(p("報告書最新.docx").base, "報告書");
        // 写し
        assert!(p("報告書 - コピー.docx").copy);
        assert!(p("報告書 - Copy.docx").copy);
        assert!(p("報告書 (1).docx").copy);
        assert!(p("Copy of 報告書.docx").copy);
        assert_eq!(p("Copy of 報告書.docx").base, "報告書");
        // 全角・半角・カタカナ
        assert_eq!(p("ﾚﾎﾟｰﾄ_Ｖ２.txt").base, p("レポート.txt").base);
        assert_eq!(p("ﾚﾎﾟｰﾄ_Ｖ２.txt").version, Some(vec![2]));
        assert_eq!(p("README").base, "readme");
        // 版の印がない名前はそのまま
        let i = p("2026年度 予算.xlsx");
        assert_eq!(i.base, "2026年度 予算");
        assert_eq!(i.version, None);
    }

    #[test]
    fn similarity_measures() {
        assert_eq!(similarity("報告書", "報告書"), 1.0);
        assert!(similarity("月次報告書", "月次報告") > 0.85);
        assert!(similarity("見積書 a社", "見積書 b社") < 0.95);
        assert!(similarity("議事録", "請求書") < 0.5);
        assert!((jaro_winkler("martha", "marhta") - 0.961).abs() < 0.01);
        assert!((dice("night", "nacht") - 0.25).abs() < 1e-9);
    }

    fn catalog(files: &[(&str, u64, i64)]) -> Catalog {
        let mut c = Catalog {
            root: "/r".into(),
            ..Catalog::default()
        };
        for &(rel, size, mtime) in files {
            c.files.push(FileEntry {
                rel: rel.to_owned(),
                meta: Meta {
                    size,
                    mtime: mtime * 86_400_000_000_000,
                    ..Meta::default()
                },
            });
        }
        c.files.sort_by(|a, b| a.rel.cmp(&b.rel));
        c
    }

    #[test]
    fn groups_and_ranks_versions() {
        let c = catalog(&[
            ("a/報告書_v2.docx", 1000, 10),
            ("a/報告書_v3_最終.docx", 1200, 12),
            ("a/報告書 - コピー.docx", 900, 5),
            ("b/報告書.docx", 900, 5),
            ("a/見積_v1.xlsx", 500, 20),
            ("a/見積_v2.xlsx", 520, 18), // 名前は新しいが日時は古い
            ("a/議事録.docx", 100, 1),
            ("a/画像.png", 100, 1),
            ("a/報告書.pdf", 50_000, 1), // 拡張子の仲間が違う
            ("a/巨大_v2.docx", 10_000_000, 3),
            ("a/巨大.docx", 10, 2), // 大きさが違いすぎる
        ]);
        let g = find_versions(std::slice::from_ref(&c), &SimilarOptions::default());
        let names = |g: &VersionGroup| -> Vec<&str> {
            g.members
                .iter()
                .map(|m| c.files[m.file.index].rel.as_str())
                .collect()
        };
        assert_eq!(g.len(), 2, "{:?}", g.iter().map(names).collect::<Vec<_>>());
        assert_eq!(
            names(&g[0]),
            [
                "a/報告書_v3_最終.docx",
                "a/報告書_v2.docx",
                "b/報告書.docx",
                "a/報告書 - コピー.docx"
            ]
        );
        assert_eq!(g[0].confidence, Confidence::High);
        assert_eq!(names(&g[1]), ["a/見積_v2.xlsx", "a/見積_v1.xlsx"]);
        assert_eq!(g[1].confidence, Confidence::Low);
        assert!(g[1].reason.contains("2 日"), "{}", g[1].reason);
        // 1 つのファイルの別の版
        let idx = |rel: &str| c.files.iter().position(|f| f.rel == rel).unwrap();
        let t = FileRef {
            root: 0,
            index: idx("b/報告書.docx"),
        };
        let one = versions_of(std::slice::from_ref(&c), t, &SimilarOptions::default()).unwrap();
        assert_eq!(names(&one), names(&g[0]));
        let lone = FileRef {
            root: 0,
            index: idx("a/画像.png"),
        };
        assert!(versions_of(std::slice::from_ref(&c), lone, &SimilarOptions::default()).is_none());
    }

    #[test]
    fn lsh_matches_brute_force_on_small_sets() {
        // 名前を少しずつ変えた集合で、LSH の結果が総当たりと同じ
        let words = [
            "月次報告書",
            "週報",
            "見積書",
            "議事録",
            "請求書",
            "納品書",
            "設計書",
        ];
        let mut files = Vec::new();
        for (wi, w) in words.iter().enumerate() {
            for v in 0..4 {
                let name = match v {
                    0 => format!("{w}.docx"),
                    1 => format!("{w}_v{v}.docx"),
                    2 => format!("{w}（修正）.docx"),
                    _ => format!("{w}s.docx"),
                };
                files.push((format!("d{wi}/{name}"), 1000u64 + v, v as i64));
            }
        }
        let refs: Vec<(&str, u64, i64)> =
            files.iter().map(|(n, s, t)| (n.as_str(), *s, *t)).collect();
        let c = catalog(&refs);
        let opts = SimilarOptions::default();
        let got = find_versions(std::slice::from_ref(&c), &opts);
        // 総当たり
        let infos: Vec<NameInfo> = c.files.iter().map(|f| parse_name(f.name())).collect();
        let mut dsu = Dsu((0..infos.len()).collect());
        for a in 0..infos.len() {
            for b in a + 1..infos.len() {
                let mut s = similarity(&infos[a].base, &infos[b].base);
                if c.files[a].dir() == c.files[b].dir() {
                    s += 0.05;
                }
                if infos[a].family == infos[b].family && s >= opts.threshold {
                    dsu.union(a, b);
                }
            }
        }
        let mut brute: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..infos.len() {
            brute.entry(dsu.find(i)).or_default().push(i);
        }
        let mut want: Vec<Vec<usize>> = brute.into_values().filter(|v| v.len() > 1).collect();
        let mut have: Vec<Vec<usize>> = got
            .iter()
            .map(|g| {
                let mut v: Vec<usize> = g.members.iter().map(|m| m.file.index).collect();
                v.sort();
                v
            })
            .collect();
        want.sort();
        have.sort();
        assert_eq!(have, want);
        assert_eq!(have.len(), words.len());
    }
}

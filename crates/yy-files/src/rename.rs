//! 一括リネーム（2026-10-27 の提案 7 章）。
//!
//! 選んだ項目に、上から順に (1) 置き換え（そのまま・正規表現）、(2) ひな形（`{name}`・`{n:3}`・
//! `{exif.date:%Y%m%d}` など）、(3) 大文字・小文字、(4) スクリプト（Rhai）を行い、新しい名前を作る。
//! [`preview`] は全件の新しい名前と状態（使えない名前・重複・既にある）を返し、[`execute`] は
//! いったん一時の名前に変えてから新しい名前に変える 2 段で実行する（入れ替え・連なりでもぶつからない。
//! 大文字と小文字だけの変更も Windows で正しく変わる）。途中で失敗したらそこまでを元に戻す。
//! 実行の記録は [`write_log`]、取り消しは [`undo`]。

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use regex_automata::meta::Regex;

use crate::props::{Props, Value};

/// 名前を変える項目。
#[derive(Clone, Debug, Default)]
pub struct Item {
    pub path: PathBuf,
    pub dir: bool,
    pub size: u64,
    /// 更新日時・作成日時（UNIX 時刻からのナノ秒）
    pub mtime: i64,
    pub ctime: i64,
}

impl Item {
    fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// 置き換える対象。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Target {
    /// 拡張子を除いた名前
    #[default]
    Stem,
    /// 名前全体
    Name,
    /// 拡張子（`.` を除く）
    Ext,
}

/// 大文字・小文字。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Case {
    #[default]
    Keep,
    Lower,
    Upper,
    /// 語の頭だけ大文字
    Title,
    /// 拡張子だけ小文字
    LowerExt,
}

/// 規則。
#[derive(Clone, Debug, Default)]
pub struct Rules {
    pub find: String,
    pub replace: String,
    pub regex: bool,
    pub case_sensitive: bool,
    /// すべて置き換える（`false` なら最初の 1 つ）
    pub all: bool,
    pub target: Target,
    /// ひな形（空なら `{name}{ext}`）
    pub template: String,
    /// 連番の始まり・増分
    pub start: i64,
    pub step: i64,
    /// 連番をフォルダごとに振り直す
    pub per_folder: bool,
    pub case: Case,
    /// スクリプト（空ならなし）
    pub script: String,
}

/// ひな形の部品。
#[derive(Clone, Debug, PartialEq)]
enum Part {
    Lit(String),
    Tok {
        name: String,
        fmt: Option<String>,
        default: Option<String>,
    },
}

/// ひな形の組み込みの名前
const BUILTIN: &[&str] = &[
    "name", "ext", "n", "orig", "origname", "parent", "size", "modified", "created", "today",
];

/// ひな形で使える組み込みの名前と説明（画面に出す）。
pub const TEMPLATE_HELP: &[(&str, &str)] = &[
    ("{name}", "拡張子を除いた名前（置き換えの後）"),
    ("{ext}", "拡張子（.jpg のように . を含む）"),
    ("{n}", "連番。{n:3} で 3 桁にそろえる"),
    ("{orig}", "元の名前（拡張子を含む）"),
    ("{origname}", "元の名前（拡張子を除く）"),
    ("{parent}", "フォルダの名前"),
    ("{size}", "大きさ（バイト）"),
    (
        "{modified:%Y%m%d}",
        "更新日時（%Y %m %d %H %M %S %y %j %a %A %b）",
    ),
    ("{created:%Y-%m-%d}", "作成日時"),
    ("{today:%Y%m%d}", "今日の日付"),
    (
        "{name:1-8}",
        "名前の 1〜8 文字目（{name:3-} は 3 文字目から）",
    ),
    (
        "{exif.model|不明}",
        "メタデータ。| の後は値がないときの文字",
    ),
];

fn parse_template(t: &str) -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    let mut lit = String::new();
    let mut it = t.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '{' if it.peek() == Some(&'{') => {
                it.next();
                lit.push('{');
            }
            '}' if it.peek() == Some(&'}') => {
                it.next();
                lit.push('}');
            }
            '{' => {
                let mut body = String::new();
                loop {
                    match it.next() {
                        Some('}') => break,
                        Some(c) => body.push(c),
                        None => return Err(format!("ひな形の {{{body} が閉じていません")),
                    }
                }
                if !lit.is_empty() {
                    parts.push(Part::Lit(std::mem::take(&mut lit)));
                }
                let (body, default) = match body.split_once('|') {
                    Some((b, d)) => (b.to_owned(), Some(d.to_owned())),
                    None => (body, None),
                };
                let (name, fmt) = match body.split_once(':') {
                    Some((n, f)) => (n.trim().to_ascii_lowercase(), Some(f.to_owned())),
                    None => (body.trim().to_ascii_lowercase(), None),
                };
                if !BUILTIN.contains(&name.as_str()) && crate::props::key(&name).is_none() {
                    return Err(format!("ひな形の {{{name}}} は使えない名前です"));
                }
                parts.push(Part::Tok { name, fmt, default });
            }
            '}' => return Err("ひな形の } に対応する { がありません（} は }} と書きます）".into()),
            c => lit.push(c),
        }
    }
    if !lit.is_empty() {
        parts.push(Part::Lit(lit));
    }
    Ok(parts)
}

/// 使うメタデータがあるか（なければメタデータを読まなくてよい）。
fn uses_props(parts: &[Part], script: &str) -> bool {
    parts
        .iter()
        .any(|p| matches!(p, Part::Tok { name, .. } if !BUILTIN.contains(&name.as_str())))
        || script.contains("meta")
}

/// 組み立てた規則。
pub struct Compiled {
    rules: Rules,
    find: Option<Regex>,
    replace: String,
    parts: Vec<Part>,
    script: Option<(rhai::Engine, rhai::AST)>,
    /// メタデータを使う
    pub needs_props: bool,
}

/// 正規表現の特別な文字を逃がす。
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// スクリプトの演算の数の上限（1 項目あたり）
const SCRIPT_OPS: u64 = 200_000;

fn engine() -> rhai::Engine {
    let mut e = rhai::Engine::new();
    e.set_max_operations(SCRIPT_OPS);
    e.set_max_expr_depths(64, 64);
    e.set_max_call_levels(32);
    e.set_max_string_size(64 * 1024);
    e.set_max_array_size(100_000);
    e.set_max_map_size(10_000);
    e.register_fn("pad", |n: i64, w: i64| {
        format!("{n:0w$}", w = w.clamp(0, 32) as usize)
    });
    e.register_fn("fmt_time", |secs: i64, f: &str| {
        crate::props::strftime(secs, f)
    });
    e.register_fn(
        "re_replace",
        |s: &str, pat: &str, rep: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            let re = Regex::new(pat).map_err(|e| e.to_string())?;
            Ok(regex_replace(&re, s, rep, true))
        },
    );
    e.register_fn(
        "re_match",
        |s: &str, pat: &str| -> Result<bool, Box<rhai::EvalAltResult>> {
            let re = Regex::new(pat).map_err(|e| e.to_string())?;
            Ok(re.is_match(s))
        },
    );
    e.register_fn(
        "re_capture",
        |s: &str, pat: &str, group: i64| -> Result<String, Box<rhai::EvalAltResult>> {
            let re = Regex::new(pat).map_err(|e| e.to_string())?;
            let mut caps = re.create_captures();
            re.captures(s, &mut caps);
            Ok(caps
                .get_group(group.max(0) as usize)
                .map(|m| s[m.range()].to_owned())
                .unwrap_or_default())
        },
    );
    e
}

/// 正規表現で置き換える（`$1`・`${name}` が使える）。
fn regex_replace(re: &Regex, hay: &str, rep: &str, all: bool) -> String {
    let mut out = String::new();
    let mut last = 0;
    for caps in re.captures_iter(hay) {
        let Some(m) = caps.get_match() else { continue };
        out.push_str(&hay[last..m.start()]);
        caps.interpolate_string_into(hay, rep, &mut out);
        last = m.end();
        if !all {
            break;
        }
    }
    out.push_str(&hay[last..]);
    out
}

impl Rules {
    pub fn compile(&self) -> Result<Compiled, String> {
        let find = if self.find.is_empty() {
            None
        } else {
            let pat = if self.regex {
                self.find.clone()
            } else {
                escape(&self.find)
            };
            Some(
                Regex::builder()
                    .syntax(
                        regex_automata::util::syntax::Config::new()
                            .case_insensitive(!self.case_sensitive)
                            .unicode(true)
                            .utf8(true),
                    )
                    .build(&pat)
                    .map_err(|e| format!("検索する文字の正規表現が読めません: {e}"))?,
            )
        };
        let replace = if self.regex {
            self.replace.clone()
        } else {
            self.replace.replace('$', "$$")
        };
        let template = if self.template.trim().is_empty() {
            "{name}{ext}"
        } else {
            self.template.as_str()
        };
        let parts = parse_template(template)?;
        let script = if self.script.trim().is_empty() {
            None
        } else {
            let e = engine();
            let ast = e
                .compile(&self.script)
                .map_err(|err| format!("スクリプトの誤り: {err}"))?;
            Some((e, ast))
        };
        Ok(Compiled {
            needs_props: uses_props(&parts, &self.script),
            rules: self.clone(),
            find,
            replace,
            parts,
            script,
        })
    }
}

/// 名前を拡張子の前と後に分ける（`.bashrc` は拡張子なし）。
pub fn split_ext(name: &str, dir: bool) -> (&str, &str) {
    if dir {
        return (name, "");
    }
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// 項目の状態。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// 変わらない
    Same,
    /// 変わる
    Ok,
    /// 使えない名前（理由）
    Invalid(String),
    /// この一括の中で同じ名前になる
    Duplicate,
    /// 一括に入っていない別のものと同じ名前
    Exists,
    /// スクリプトのエラー
    Error(String),
}

impl Status {
    pub fn is_problem(&self) -> bool {
        !matches!(self, Status::Same | Status::Ok)
    }

    pub fn label(&self) -> String {
        match self {
            Status::Same => "変わらない".into(),
            Status::Ok => "変わる".into(),
            Status::Invalid(r) => format!("使えない名前: {r}"),
            Status::Duplicate => "重複".into(),
            Status::Exists => "既にある".into(),
            Status::Error(e) => format!("エラー: {e}"),
        }
    }
}

/// 1 項目の結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub name: String,
    pub status: Status,
}

/// Windows で使えない名前か（理由を返す）。
pub fn invalid_reason(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("空です".into());
    }
    if let Some(c) = name
        .chars()
        .find(|&c| "\\/:*?\"<>|".contains(c) || (c as u32) < 0x20)
    {
        return Some(if (c as u32) < 0x20 {
            "制御文字を含みます".into()
        } else {
            format!("{c} を含みます")
        });
    }
    if name.ends_with(' ') || name.ends_with('.') {
        return Some("終わりが空白か . です".into());
    }
    if name == "." || name == ".." {
        return Some("使えません".into());
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0');
    if reserved {
        return Some(format!("{stem} は Windows の予約された名前です"));
    }
    if name.encode_utf16().count() > 255 {
        return Some("長すぎます（255 文字まで）".into());
    }
    None
}

fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start = false;
        } else {
            out.push(c);
            start = true;
        }
    }
    out
}

/// `{name:1-8}` の範囲（1 から数える）。
fn substring(s: &str, spec: &str) -> Option<String> {
    let (a, b) = spec.split_once('-')?;
    let chars: Vec<char> = s.chars().collect();
    let a: usize = if a.trim().is_empty() {
        1
    } else {
        a.trim().parse().ok()?
    };
    let b: usize = if b.trim().is_empty() {
        chars.len()
    } else {
        b.trim().parse().ok()?
    };
    let a = a.max(1) - 1;
    let b = b.min(chars.len());
    Some(if a >= b {
        String::new()
    } else {
        chars[a..b].iter().collect()
    })
}

/// 数を桁数（`3`）でそろえる。数でなければそのまま。
fn pad_num(v: &str, fmt: &str) -> String {
    match (fmt.trim().parse::<usize>(), v.parse::<i64>()) {
        (Ok(w), Ok(n)) if w <= 32 => {
            if n < 0 {
                format!("-{:0w$}", -n, w = w)
            } else {
                format!("{n:0w$}")
            }
        }
        _ => v.to_owned(),
    }
}

/// 1 項目の計算に使うもの。
struct Ctx<'a> {
    item: &'a Item,
    index: usize,
    n: i64,
    props: Option<&'a Props>,
    tz: i64,
    now: i64,
}

impl Compiled {
    fn token(
        &self,
        cx: &Ctx<'_>,
        stem: &str,
        ext: &str,
        name: &str,
        fmt: Option<&str>,
    ) -> Option<String> {
        let time = |ns: i64| -> Option<String> {
            if ns == 0 {
                return None;
            }
            let secs = ns.div_euclid(1_000_000_000) + cx.tz;
            Some(crate::props::strftime(secs, fmt.unwrap_or("%Y%m%d")))
        };
        let orig = cx.item.name();
        let v = match name {
            "name" | "origname" => {
                let s = if name == "name" {
                    stem.to_owned()
                } else {
                    split_ext(&orig, cx.item.dir).0.to_owned()
                };
                return Some(match fmt.and_then(|f| substring(&s, f)) {
                    Some(sub) => sub,
                    None => s,
                });
            }
            "ext" => ext.to_owned(),
            "n" => return Some(pad_num(&cx.n.to_string(), fmt.unwrap_or("0"))),
            "orig" => orig,
            "parent" => cx
                .item
                .path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            "size" => cx.item.size.to_string(),
            "modified" => return time(cx.item.mtime),
            "created" => return time(cx.item.ctime),
            "today" => return time(cx.now),
            key => {
                let v = cx.props?.get(crate::props::key(key)?)?;
                return Some(match (v, fmt) {
                    (Value::Time { .. }, f) => v.format_time(f.unwrap_or("%Y%m%d"), cx.tz)?,
                    (_, Some(f)) => pad_num(&v.text(cx.tz), f),
                    (_, None) => v.text(cx.tz),
                });
            }
        };
        Some(match fmt.and_then(|f| substring(&v, f)) {
            Some(sub) => sub,
            None => v,
        })
    }

    /// 1 項目の新しい名前。
    fn new_name(&self, cx: &Ctx<'_>) -> Result<String, String> {
        let r = &self.rules;
        let orig = cx.item.name();
        let (mut stem, mut ext) = {
            let (s, e) = split_ext(&orig, cx.item.dir);
            (s.to_owned(), e.to_owned())
        };
        // (1) 置き換え
        if let Some(re) = &self.find {
            match r.target {
                Target::Stem => stem = regex_replace(re, &stem, &self.replace, r.all),
                Target::Name => {
                    let whole = regex_replace(re, &format!("{stem}{ext}"), &self.replace, r.all);
                    let (s, e) = split_ext(&whole, cx.item.dir);
                    (stem, ext) = (s.to_owned(), e.to_owned());
                }
                Target::Ext => {
                    let e = ext.strip_prefix('.').unwrap_or(&ext).to_owned();
                    let e = regex_replace(re, &e, &self.replace, r.all);
                    ext = if e.is_empty() {
                        String::new()
                    } else {
                        format!(".{e}")
                    };
                }
            }
        }
        // (2) ひな形
        let mut name = String::new();
        for p in &self.parts {
            match p {
                Part::Lit(s) => name.push_str(s),
                Part::Tok {
                    name: t,
                    fmt,
                    default,
                } => match self.token(cx, &stem, &ext, t, fmt.as_deref()) {
                    Some(v) if !v.is_empty() => name.push_str(&v),
                    _ => name.push_str(default.as_deref().unwrap_or("")),
                },
            }
        }
        // (3) 大文字・小文字
        name = match r.case {
            Case::Keep => name,
            Case::Lower => name.to_lowercase(),
            Case::Upper => name.to_uppercase(),
            Case::Title => {
                let (s, e) = split_ext(&name, cx.item.dir);
                format!("{}{}", title_case(s), e.to_lowercase())
            }
            Case::LowerExt => {
                let (s, e) = split_ext(&name, cx.item.dir);
                format!("{s}{}", e.to_lowercase())
            }
        };
        // (4) スクリプト
        if let Some((engine, ast)) = &self.script {
            let mut scope = rhai::Scope::new();
            let (s, e) = split_ext(&name, cx.item.dir);
            scope.push("name", name.clone());
            scope.push("stem", s.to_owned());
            scope.push("ext", e.to_owned());
            scope.push("orig", orig.clone());
            scope.push("n", cx.n);
            scope.push("index", cx.index as i64);
            scope.push(
                "folder",
                cx.item
                    .path
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
            scope.push("path", cx.item.path.to_string_lossy().into_owned());
            scope.push("size", cx.item.size as i64);
            scope.push("modified", cx.item.mtime.div_euclid(1_000_000_000) + cx.tz);
            scope.push("is_dir", cx.item.dir);
            let mut meta = rhai::Map::new();
            if let Some(p) = cx.props {
                for (k, v) in p {
                    let d = match v {
                        Value::Num(n) if n.fract() == 0.0 => rhai::Dynamic::from(*n as i64),
                        Value::Num(n) => rhai::Dynamic::from(*n),
                        Value::Time { .. } => rhai::Dynamic::from(v.text(cx.tz)),
                        Value::Text(s) => rhai::Dynamic::from(s.clone()),
                    };
                    meta.insert((*k).into(), d);
                }
            }
            scope.push("meta", meta);
            let out = engine
                .eval_ast_with_scope::<rhai::Dynamic>(&mut scope, ast)
                .map_err(|e| e.to_string())?;
            if out.is_unit() {
                // 何も返さなければそのまま
            } else if let Some(s) = out.clone().try_cast::<rhai::ImmutableString>() {
                name = s.to_string();
            } else {
                return Err("スクリプトは文字列を返してください".into());
            }
        }
        Ok(name)
    }
}

/// 新しい名前と状態を全件で計算する。`props(i)` は項目 `i` のメタデータ（読んでいなければ `None`）、
/// `exists(p)` はパスに何かがあるか、`tz` は手元の時刻の時差（秒）、`now` は今（ナノ秒）。
pub fn preview<'p>(
    c: &Compiled,
    items: &[Item],
    props: &dyn Fn(usize) -> Option<&'p Props>,
    exists: &dyn Fn(&Path) -> bool,
    tz: i64,
    now: i64,
) -> Vec<Outcome> {
    let r = &c.rules;
    let step = if r.step == 0 { 1 } else { r.step };
    let mut per: HashMap<PathBuf, i64> = HashMap::new();
    let mut out: Vec<Outcome> = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let k = if r.per_folder {
            let e = per
                .entry(
                    item.path
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_default(),
                )
                .or_insert(0);
            *e += 1;
            *e - 1
        } else {
            i as i64
        };
        let cx = Ctx {
            item,
            index: i,
            n: r.start.saturating_add(step.saturating_mul(k)),
            props: props(i),
            tz,
            now,
        };
        out.push(match c.new_name(&cx) {
            Ok(name) => {
                let status = if name == item.name() {
                    Status::Same
                } else if let Some(reason) = invalid_reason(&name) {
                    Status::Invalid(reason)
                } else {
                    Status::Ok
                };
                Outcome { name, status }
            }
            Err(e) => Outcome {
                name: item.name(),
                status: Status::Error(e),
            },
        });
    }
    // 重複（同じフォルダで同じ名前。大文字と小文字は区別しない）
    let key = |p: &Path, name: &str| {
        (
            p.parent().map(Path::to_path_buf).unwrap_or_default(),
            name.to_lowercase(),
        )
    };
    let mut count: HashMap<(PathBuf, String), usize> = HashMap::new();
    for (item, o) in items.iter().zip(&out) {
        *count.entry(key(&item.path, &o.name)).or_insert(0) += 1;
    }
    let originals: std::collections::HashSet<(PathBuf, String)> =
        items.iter().map(|it| key(&it.path, &it.name())).collect();
    for (item, o) in items.iter().zip(out.iter_mut()) {
        if matches!(o.status, Status::Invalid(_) | Status::Error(_)) {
            continue;
        }
        if count[&key(&item.path, &o.name)] > 1 {
            o.status = Status::Duplicate;
            continue;
        }
        if o.status == Status::Ok {
            let k = key(&item.path, &o.name);
            // 大文字と小文字だけの変更、または一括の中の別の項目の元の名前（そちらも変わる）はよい
            let case_only = o.name.to_lowercase() == item.name().to_lowercase();
            if !case_only && !originals.contains(&k) && exists(&item.path.with_file_name(&o.name)) {
                o.status = Status::Exists;
            }
        }
    }
    // 一括の中の別の項目の元の名前に変えるのに、その項目が変わらないなら重複
    let unchanged: std::collections::HashSet<(PathBuf, String)> = items
        .iter()
        .zip(&out)
        .filter(|(_, o)| o.status != Status::Ok)
        .map(|(it, _)| key(&it.path, &it.name()))
        .collect();
    for (item, o) in items.iter().zip(out.iter_mut()) {
        if o.status == Status::Ok
            && o.name.to_lowercase() != item.name().to_lowercase()
            && unchanged.contains(&key(&item.path, &o.name))
        {
            o.status = Status::Duplicate;
        }
    }
    out
}

/// 実行する名前の変更（元・新しいパス）。変わるものだけ。問題があれば `Err`（件数）。
pub fn moves(
    items: &[Item],
    outcomes: &[Outcome],
    skip_problems: bool,
) -> Result<Vec<(PathBuf, PathBuf)>, usize> {
    let problems = outcomes.iter().filter(|o| o.status.is_problem()).count();
    if problems > 0 && !skip_problems {
        return Err(problems);
    }
    Ok(items
        .iter()
        .zip(outcomes)
        .filter(|(_, o)| o.status == Status::Ok)
        .map(|(it, o)| (it.path.clone(), it.path.with_file_name(&o.name)))
        .collect())
}

/// 実行の記録: 実行した順の段（深いフォルダから）。段の中は（元・新しいパス）。
pub type Log = Vec<Vec<(PathBuf, PathBuf)>>;

/// 名前を変える操作（`to` があれば失敗する）。
pub type RenameFn<'a> = &'a dyn Fn(&Path, &Path) -> io::Result<()>;

fn depth(p: &Path) -> usize {
    p.components().count()
}

/// 1 つの段（同じ深さ）を 2 段で変える。失敗したらこの段を元に戻して `Err`。
fn run_level(level: &[(PathBuf, PathBuf)], rename: RenameFn, tag: &str) -> io::Result<()> {
    let temp: Vec<PathBuf> = level
        .iter()
        .enumerate()
        .map(|(i, (from, _))| from.with_file_name(format!(".yyren-{tag}-{i}")))
        .collect();
    let mut stage1 = 0;
    let mut stage2 = 0;
    let res = (|| -> io::Result<()> {
        for ((from, _), t) in level.iter().zip(&temp) {
            rename(from, t)
                .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", from.display())))?;
            stage1 += 1;
        }
        for ((_, to), t) in level.iter().zip(&temp) {
            rename(t, to)
                .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", to.display())))?;
            stage2 += 1;
        }
        Ok(())
    })();
    if let Err(e) = res {
        // 戻す（新しい名前 → 一時の名前 → 元の名前）
        let mut lost = Vec::new();
        for ((_, to), t) in level.iter().zip(&temp).take(stage2) {
            if rename(to, t).is_err() {
                lost.push(to.display().to_string());
            }
        }
        for ((from, _), t) in level.iter().zip(&temp).take(stage1) {
            if rename(t, from).is_err() {
                lost.push(t.display().to_string());
            }
        }
        if !lost.is_empty() {
            return Err(io::Error::new(
                e.kind(),
                format!("{e}（元に戻せなかったもの: {}）", lost.join("、")),
            ));
        }
        return Err(e);
    }
    Ok(())
}

/// 段を順に行う。失敗したら済んだ段を元に戻す。
fn run_levels(
    levels: &[Vec<(PathBuf, PathBuf)>],
    rename: RenameFn,
    progress: &mut dyn FnMut(usize),
) -> io::Result<()> {
    let tag = format!(
        "{:x}",
        crate::fs::to_nanos(std::time::SystemTime::now()) as u64 & 0xffff_ffff
    );
    let mut done = 0;
    for (k, level) in levels.iter().enumerate() {
        if let Err(e) = run_level(level, rename, &format!("{tag}{k}")) {
            for (j, l) in levels[..k].iter().enumerate().rev() {
                let back: Vec<(PathBuf, PathBuf)> =
                    l.iter().map(|(a, b)| (b.clone(), a.clone())).collect();
                if let Err(e2) = run_level(&back, rename, &format!("{tag}u{j}")) {
                    return Err(io::Error::new(
                        e.kind(),
                        format!("{e}。さらに元に戻すのにも失敗しました: {e2}"),
                    ));
                }
            }
            return Err(e);
        }
        done += level.len();
        progress(done);
    }
    Ok(())
}

/// 名前を変える。深いフォルダの項目から段ごとに行う（フォルダとその中を一緒に変えられる）。
/// 成功すれば記録を返す。失敗したらすべてを元に戻して `Err`。
pub fn execute(
    moves: &[(PathBuf, PathBuf)],
    rename: RenameFn,
    progress: &mut dyn FnMut(usize),
) -> io::Result<Log> {
    let mut by_depth: std::collections::BTreeMap<usize, Vec<(PathBuf, PathBuf)>> =
        Default::default();
    for (a, b) in moves {
        by_depth
            .entry(depth(a))
            .or_default()
            .push((a.clone(), b.clone()));
    }
    let levels: Log = by_depth.into_values().rev().collect();
    run_levels(&levels, rename, progress)?;
    Ok(levels)
}

/// 記録から元に戻す（浅い段から）。
pub fn undo(log: &Log, rename: RenameFn, progress: &mut dyn FnMut(usize)) -> io::Result<()> {
    let levels: Log = log
        .iter()
        .rev()
        .map(|l| l.iter().map(|(a, b)| (b.clone(), a.clone())).collect())
        .collect();
    run_levels(&levels, rename, progress)
}

fn csv_field(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// 記録を CSV（段,元,新しい名前）に書く。
pub fn write_log(path: &Path, log: &Log) -> io::Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut f = io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all("\u{feff}段,元のパス,新しいパス\r\n".as_bytes())?;
    for (k, level) in log.iter().enumerate() {
        for (a, b) in level {
            writeln!(
                f,
                "{k},{},{}\r",
                csv_field(&a.to_string_lossy()),
                csv_field(&b.to_string_lossy())
            )?;
        }
    }
    f.flush()
}

/// CSV の 1 行を分ける（引用符に対応）。
fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' if q && it.peek() == Some(&'"') => {
                it.next();
                cur.push('"');
            }
            '"' => q = !q,
            ',' if !q => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// [`write_log`] の記録を読む。
pub fn read_log(path: &Path) -> io::Result<Log> {
    let f = io::BufReader::new(std::fs::File::open(path)?);
    let mut log: Log = Vec::new();
    for (i, line) in f.lines().enumerate() {
        let line = line?;
        let line = line.trim_end_matches('\r');
        if i == 0 || line.is_empty() {
            continue;
        }
        let v = split_csv(line);
        if v.len() != 3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} 行目が読めません", i + 1),
            ));
        }
        let k: usize = v[0].parse().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} 行目が読めません", i + 1),
            )
        })?;
        if log.len() <= k {
            log.resize_with(k + 1, Vec::new);
        }
        log[k].push((PathBuf::from(&v[1]), PathBuf::from(&v[2])));
    }
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Fs;

    fn item(p: &str) -> Item {
        Item {
            path: PathBuf::from(p),
            mtime: (crate::archive::days_from_civil(2024, 5, 6) * 86_400 + 3600) * 1_000_000_000,
            ..Default::default()
        }
    }

    fn run(rules: Rules, items: &[Item]) -> Vec<Outcome> {
        let c = rules.compile().unwrap();
        preview(&c, items, &|_| None, &|_| false, 9 * 3600, 0)
    }

    fn names(o: &[Outcome]) -> Vec<&str> {
        o.iter().map(|o| o.name.as_str()).collect()
    }

    #[test]
    fn replace_and_template() {
        let items = [
            item("/a/IMG_0001.JPG"),
            item("/a/IMG_0002.jpg"),
            item("/b/IMG_0003.JPG"),
        ];
        let o = run(
            Rules {
                find: r"IMG_(\d+)".into(),
                replace: "photo-$1".into(),
                regex: true,
                all: true,
                template: "{modified:%Y%m%d}_{n:3}_{name}{ext}".into(),
                start: 1,
                step: 1,
                case: Case::LowerExt,
                ..Default::default()
            },
            &items,
        );
        assert_eq!(
            names(&o),
            [
                "20240506_001_photo-0001.jpg",
                "20240506_002_photo-0002.jpg",
                "20240506_003_photo-0003.jpg"
            ]
        );
        assert!(o.iter().all(|o| o.status == Status::Ok));
        // フォルダごとの連番・そのままの文字の置き換え（$ は特別でない）・大文字と小文字を区別しない
        let o = run(
            Rules {
                find: "img_".into(),
                replace: "$x".into(),
                template: "{parent}-{n:2}{ext}".into(),
                start: 10,
                step: 5,
                per_folder: true,
                ..Default::default()
            },
            &items,
        );
        assert_eq!(names(&o), ["a-10.JPG", "a-15.jpg", "b-10.JPG"]);
        let o = run(
            Rules {
                find: "img_".into(),
                replace: "$x".into(),
                ..Default::default()
            },
            &items,
        );
        assert_eq!(o[0].name, "$x0001.JPG");
        // 最初の 1 つだけ・名前全体・拡張子
        let o = run(
            Rules {
                find: "a".into(),
                replace: "b".into(),
                case_sensitive: true,
                target: Target::Name,
                ..Default::default()
            },
            &[item("/x/aaa.a")],
        );
        assert_eq!(o[0].name, "baa.a");
        let o = run(
            Rules {
                find: "jpeg".into(),
                replace: "jpg".into(),
                target: Target::Ext,
                ..Default::default()
            },
            &[item("/x/p.JPEG")],
        );
        assert_eq!(o[0].name, "p.jpg");
        // 部分・元の名前・既定値
        let o = run(
            Rules {
                template: "{name:1-3}_{orig}_{exif.model|不明}{ext}".into(),
                case: Case::Title,
                ..Default::default()
            },
            &[item("/x/hello world.TXT")],
        );
        assert_eq!(o[0].name, "Hel_Hello World.Txt_不明.txt");
    }

    #[test]
    fn metadata_in_template() {
        let mut p = Props::new();
        p.insert(
            "exif.date",
            Value::Time {
                secs: crate::archive::days_from_civil(2023, 12, 31) * 86_400 + 23 * 3600,
                local: true,
            },
        );
        p.insert("exif.model", Value::Text("EOS R5".into()));
        p.insert("id3.track", Value::Num(7.0));
        let c = Rules {
            template: "{exif.date:%Y-%m-%d_%H%M}_{exif.model}_{id3.track:2}{ext}".into(),
            ..Default::default()
        }
        .compile()
        .unwrap();
        assert!(c.needs_props);
        let o = preview(
            &c,
            &[item("/x/a.jpg")],
            &|_| Some(&p),
            &|_| false,
            9 * 3600,
            0,
        );
        assert_eq!(o[0].name, "2023-12-31_2300_EOS R5_07.jpg");
    }

    #[test]
    fn script_changes_names() {
        let items = [item("/x/Track 1 - Song.mp3"), item("/x/b.txt")];
        let o = run(
            Rules {
                script: r#"
                    if ext == ".txt" { return; }
                    let t = re_capture(stem, "Track (\\d+) - (.*)", 2);
                    pad(n, 3) + "_" + t.to_lower() + ext
                "#
                .into(),
                start: 1,
                step: 1,
                ..Default::default()
            },
            &items,
        );
        assert_eq!(names(&o), ["001_song.mp3", "b.txt"]);
        assert_eq!(o[1].status, Status::Same);
        // 誤りは項目のエラー、終わらないスクリプトは止まる
        let o = run(
            Rules {
                script: "loop { }".into(),
                ..Default::default()
            },
            &items,
        );
        assert!(matches!(o[0].status, Status::Error(_)));
        let o = run(
            Rules {
                script: "42".into(),
                ..Default::default()
            },
            &items,
        );
        assert!(matches!(&o[0].status, Status::Error(e) if e.contains("文字列")));
        assert!(
            Rules {
                script: "let = ;".into(),
                ..Default::default()
            }
            .compile()
            .is_err()
        );
        assert!(
            Rules {
                template: "{nope}".into(),
                ..Default::default()
            }
            .compile()
            .is_err()
        );
        assert!(
            Rules {
                template: "{name".into(),
                ..Default::default()
            }
            .compile()
            .is_err()
        );
        assert!(
            Rules {
                find: "(".into(),
                regex: true,
                ..Default::default()
            }
            .compile()
            .is_err()
        );
    }

    #[test]
    fn detects_problems() {
        assert!(invalid_reason("a:b").is_some());
        assert!(invalid_reason("con.txt").is_some());
        assert!(invalid_reason("COM1").is_some());
        assert!(invalid_reason("COM0").is_none());
        assert!(invalid_reason("console.txt").is_none());
        assert!(invalid_reason("a.").is_some());
        assert!(invalid_reason(&"あ".repeat(256)).is_some());
        let items = [item("/x/a.txt"), item("/x/b.txt"), item("/x/c.txt")];
        // 全部が同じ名前 → 重複
        let o = run(
            Rules {
                template: "same{ext}".into(),
                ..Default::default()
            },
            &items,
        );
        assert!(o.iter().all(|o| o.status == Status::Duplicate));
        // 既にある（一括の外）
        let c = Rules {
            template: "{name}2{ext}".into(),
            ..Default::default()
        }
        .compile()
        .unwrap();
        let o = preview(&c, &items, &|_| None, &|p| p.ends_with("a2.txt"), 0, 0);
        assert_eq!(o[0].status, Status::Exists);
        assert_eq!(o[1].status, Status::Ok);
        // 一括の中の項目の元の名前で、その項目も変わる（入れ替え）ならよい
        let c = Rules {
            script: r#"if stem == "a" { "b.txt" } else if stem == "b" { "a.txt" }"#.into(),
            ..Default::default()
        }
        .compile()
        .unwrap();
        let o = preview(&c, &items, &|_| None, &|_| true, 0, 0);
        assert_eq!(o[0].status, Status::Ok);
        assert_eq!(o[1].status, Status::Ok);
        assert_eq!(o[2].status, Status::Same);
        // 変わらない項目の名前に変えるのは重複
        let c = Rules {
            script: r#"if stem == "a" { "c.txt" }"#.into(),
            ..Default::default()
        }
        .compile()
        .unwrap();
        let o = preview(&c, &items, &|_| None, &|_| true, 0, 0);
        assert_eq!(o[0].status, Status::Duplicate);
        // 使えない名前
        let o = run(
            Rules {
                template: "{name}?{ext}".into(),
                ..Default::default()
            },
            &items,
        );
        assert!(matches!(o[0].status, Status::Invalid(_)));
        assert_eq!(moves(&items, &o, false), Err(3));
        assert_eq!(moves(&items, &o, true).unwrap().len(), 0);
    }

    #[test]
    fn executes_swaps_chains_folders_and_undo() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for n in ["1.txt", "2.txt", "3.txt", "a.txt", "b.txt"] {
            std::fs::write(d.join(n), n).unwrap();
        }
        std::fs::create_dir(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/x.txt"), "x").unwrap();
        let mv = |a: &str, b: &str| (d.join(a), d.join(b));
        let moves = vec![
            mv("1.txt", "2.txt"),
            mv("2.txt", "3.txt"),
            mv("3.txt", "4.txt"),
            mv("a.txt", "b.txt"),
            mv("b.txt", "a.txt"),
            mv("sub/x.txt", "sub/y.txt"),
            mv("sub", "folder"),
        ];
        let local = crate::Local;
        let rename = |a: &Path, b: &Path| local.rename_new(a, b);
        let mut seen = 0;
        let log = execute(&moves, &rename, &mut |n| seen = n).unwrap();
        assert_eq!(seen, 7);
        let read = |p: &str| std::fs::read_to_string(d.join(p)).unwrap();
        assert_eq!(read("2.txt"), "1.txt");
        assert_eq!(read("3.txt"), "2.txt");
        assert_eq!(read("4.txt"), "3.txt");
        assert_eq!(read("a.txt"), "b.txt");
        assert_eq!(read("b.txt"), "a.txt");
        assert_eq!(read("folder/y.txt"), "x");
        // 記録を書いて読み、元に戻す
        let lp = d.join("logs/rename.csv");
        write_log(&lp, &log).unwrap();
        let back = read_log(&lp).unwrap();
        assert_eq!(back, log);
        undo(&back, &rename, &mut |_| {}).unwrap();
        assert_eq!(read("1.txt"), "1.txt");
        assert_eq!(read("a.txt"), "a.txt");
        assert_eq!(read("sub/x.txt"), "x");
        assert!(!d.join("4.txt").exists());
        let left: Vec<_> = std::fs::read_dir(d)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".yyren"))
            .collect();
        assert!(left.is_empty());
    }

    #[test]
    fn failure_rolls_back_everything() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for n in ["a.txt", "b.txt"] {
            std::fs::write(d.join(n), n).unwrap();
        }
        std::fs::create_dir(d.join("s")).unwrap();
        std::fs::write(d.join("s/c.txt"), "c").unwrap();
        let local = crate::Local;
        // 浅い段の 2 つ目（b → fail）で失敗させる。深い段（s/c）は済んでいる
        let rename = |a: &Path, b: &Path| {
            if b.file_name().is_some_and(|n| n == "fail.txt") {
                return Err(io::Error::other("だめ"));
            }
            local.rename_new(a, b)
        };
        let moves = vec![
            (d.join("s/c.txt"), d.join("s/c2.txt")),
            (d.join("a.txt"), d.join("ok.txt")),
            (d.join("b.txt"), d.join("fail.txt")),
        ];
        let e = execute(&moves, &rename, &mut |_| {}).unwrap_err();
        assert!(e.to_string().contains("だめ"), "{e}");
        for (n, v) in [("a.txt", "a.txt"), ("b.txt", "b.txt"), ("s/c.txt", "c")] {
            assert_eq!(std::fs::read_to_string(d.join(n)).unwrap(), v);
        }
        assert!(!d.join("ok.txt").exists());
        assert!(!d.join("s/c2.txt").exists());
    }

    #[test]
    fn case_only_rename() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("readme.TXT"), "r").unwrap();
        let items = [Item {
            path: dir.path().join("readme.TXT"),
            ..Default::default()
        }];
        let c = Rules {
            case: Case::Upper,
            ..Default::default()
        }
        .compile()
        .unwrap();
        let o = preview(&c, &items, &|_| None, &|p| p.exists(), 0, 0);
        assert_eq!(o[0].name, "README.TXT");
        assert_eq!(o[0].status, Status::Ok);
        let m = moves(&items, &o, false).unwrap();
        let local = crate::Local;
        execute(&m, &|a, b| local.rename_new(a, b), &mut |_| {}).unwrap();
        assert!(dir.path().join("README.TXT").exists());
    }
}

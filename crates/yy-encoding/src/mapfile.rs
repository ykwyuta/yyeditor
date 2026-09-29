//! 外部の対応表（`.map` ファイル、03 章 2.3・4.3）。
//!
//! 富士通 JEF・日立 KEIS・NEC JIPS などの公開されていないベンダー漢字コードや、
//! 外字（ユーザー定義文字）を利用者の手元の対応表で扱う。1 行に 1 つの対応を
//! `符号<TAB>文字` の形で書く。
//!
//! ```text
//! # コメント
//! @name JEF            文字コードの名前（省略するとファイル名）
//! @base IBM-930        土台にする文字コード（省略すると空の EBCDIC）
//! @shift 0x28 0x29     2 バイト部に入る・出るシフト（EBCDIC のみ。既定は 0x0E 0x0F）
//! 0x4E6F  U+6F22       2 バイトの符号（16 進 4 桁）
//! 0xC1    U+0041       1 バイトの符号（16 進 2 桁）
//! 0xECC3  U+00E6+0300  2 文字に対応する符号（EBCDIC のみ）
//! ```
//!
//! 土台は EBCDIC（IBM-930 など）か、Shift_JIS・CP932・EUC-JP などの表駆動の文字コード。
//! 同じ符号の土台の対応は置き換える（置き換えた元の文字はその符号に変換しなくなる）。

use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::{Encoding, dbcs, ebcdic};

/// 登録できる対応表の数の上限（メニューの項目数）。
const MAX_MAPPINGS: usize = 32;

/// 読み込んだ外部の対応表。
pub struct Mapping {
    name: &'static str,
    /// 土台の文字コードの名前（説明用）
    base: String,
    path: Option<PathBuf>,
    pub(crate) kind: Kind,
}

pub(crate) enum Kind {
    Ebcdic(ebcdic::Table),
    Dbcs(dbcs::Table),
}

impl Mapping {
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// 読み込んだファイル。
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub(crate) fn description(&self) -> String {
        if self.base.is_empty() {
            "外部の対応表".to_owned()
        } else {
            format!("外部の対応表、{} ベース", self.base)
        }
    }

    pub(crate) fn is_ebcdic(&self) -> bool {
        matches!(self.kind, Kind::Ebcdic(_))
    }
}

impl fmt::Debug for Mapping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

impl PartialEq for Mapping {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for Mapping {}

impl Hash for Mapping {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

static REGISTRY: RwLock<Vec<&'static Mapping>> = RwLock::new(Vec::new());

/// 登録済みの対応表。
pub fn mappings() -> Vec<&'static Mapping> {
    REGISTRY.read().map(|r| r.clone()).unwrap_or_default()
}

/// 名前で探す（大文字小文字、`-` `_` 空白の違いは無視）。
pub(crate) fn find(name: &str) -> Option<&'static Mapping> {
    let key = normalize(name);
    mappings().into_iter().find(|m| normalize(m.name) == key)
}

fn normalize(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '-' | '_' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

/// 対応表を登録する（同じ名前のものがあれば置き換える）。
///
/// 対応表は終了まで使われるため、登録したものは解放しない。
pub fn register(mapping: Mapping) -> Result<Encoding, String> {
    let mut reg = REGISTRY.write().map_err(|e| e.to_string())?;
    let key = normalize(mapping.name);
    let existing = reg.iter().position(|m| normalize(m.name) == key);
    if existing.is_none() && reg.len() >= MAX_MAPPINGS {
        return Err(format!("対応表は {MAX_MAPPINGS} 個まで登録できます"));
    }
    if Encoding::from_builtin_name(mapping.name).is_some() {
        return Err(format!(
            "{}: 組み込みの文字コードと同じ名前です",
            mapping.name
        ));
    }
    let m: &'static Mapping = Box::leak(Box::new(mapping));
    match existing {
        Some(i) => reg[i] = m,
        None => reg.push(m),
    }
    Ok(Encoding::Custom(m, ebcdic::Records::Nl))
}

/// フォルダ内の `*.map` をすべて読み込んで登録する。読めなかったファイルの説明を返す。
pub fn load_dir(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("map")))
        .collect();
    paths.sort();
    let mut errors = Vec::new();
    for path in paths {
        let r = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                // UTF-8（BOM 付きも可）
                let text =
                    String::from_utf8(bytes).map_err(|_| "UTF-8 で書いてください".to_owned())?;
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut m = parse(&stem, text.trim_start_matches('\u{FEFF}'))?;
                m.path = Some(path.clone());
                register(m)
            });
        if let Err(e) = r {
            errors.push(format!("{}: {e}", path.display()));
        }
    }
    errors
}

fn parse_hex_bytes(s: &str) -> Option<Vec<u8>> {
    let s = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    if s.is_empty() || s.len() % 2 != 0 || s.len() > 6 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// `U+6F22`・`U+304B+309A`・`U+304B+U+309A`
fn parse_chars(s: &str) -> Option<(u32, u32)> {
    let s = s.strip_prefix("U+").or_else(|| s.strip_prefix("u+"))?;
    let mut parts = s
        .split('+')
        .filter(|p| !p.is_empty() && *p != "U" && *p != "u");
    let parse = |p: &str| {
        let p = p.trim_start_matches(['U', 'u']);
        u32::from_str_radix(p, 16)
            .ok()
            .filter(|v| char::from_u32(*v).is_some())
    };
    let a = parse(parts.next()?)?;
    let b = match parts.next() {
        Some(p) => parse(p)?,
        None => 0,
    };
    parts.next().is_none().then_some((a, b))
}

/// 対応表の内容を読む。`default_name` は `@name` がない場合の名前。
pub fn parse(default_name: &str, text: &str) -> Result<Mapping, String> {
    let mut name = default_name.to_owned();
    let mut base: Option<Encoding> = None;
    let mut shift: Option<(u8, u8)> = None;
    // (行番号, 符号, 文字, 2 文字目)
    let mut entries: Vec<(usize, Vec<u8>, u32, u32)> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(d) = line.strip_prefix('@') {
            let mut it = d.split_whitespace();
            match it.next() {
                Some("name") => {
                    name = it.collect::<Vec<_>>().join(" ");
                }
                Some("base") => {
                    let b = it.next().unwrap_or_default();
                    let e = Encoding::from_builtin_name(b)
                        .filter(|e| matches!(e, Encoding::Ebcdic(..)) || dbcs::is_dbcs(*e))
                        .ok_or_else(|| format!("{n} 行目: 土台にできない文字コードです: {b}"))?;
                    base = Some(e);
                }
                Some("shift") => {
                    let so = it.next().and_then(parse_hex_bytes);
                    let si = it.next().and_then(parse_hex_bytes);
                    match (so.as_deref(), si.as_deref()) {
                        (Some([so]), Some([si])) if so != si => shift = Some((*so, *si)),
                        _ => return Err(format!("{n} 行目: @shift <SO> <SI> と書いてください")),
                    }
                }
                _ => return Err(format!("{n} 行目: 不明な指定です: @{d}")),
            }
            continue;
        }
        let mut cols = line.split(['\t', ' ']).filter(|c| !c.is_empty());
        let (Some(code), Some(chars)) = (cols.next(), cols.next()) else {
            return Err(format!("{n} 行目: 「符号<TAB>文字」と書いてください"));
        };
        let bytes =
            parse_hex_bytes(code).ok_or_else(|| format!("{n} 行目: 不正な符号です: {code}"))?;
        let (a, b) =
            parse_chars(chars).ok_or_else(|| format!("{n} 行目: 不正な文字です: {chars}"))?;
        entries.push((n, bytes, a, b));
    }
    let name = name.trim();
    if name.is_empty() || name.contains('/') {
        return Err("@name に名前を指定してください（/ は使えません）".to_owned());
    }
    let kind = match base {
        Some(e) if dbcs::is_dbcs(e) => {
            if shift.is_some() {
                return Err("@shift は EBCDIC の土台にだけ指定できます".to_owned());
            }
            let mut list = Vec::with_capacity(entries.len());
            for (n, bytes, a, b) in entries {
                if b != 0 {
                    return Err(format!("{n} 行目: 2 文字への対応は EBCDIC でのみ使えます"));
                }
                list.push((bytes, a));
            }
            Kind::Dbcs(dbcs::table(e).overlay(&list)?)
        }
        _ => {
            let mut builder = match base {
                Some(Encoding::Ebcdic(c, _)) => ebcdic::TableBuilder::from_ccsid(c),
                _ => ebcdic::TableBuilder::new(false),
            };
            if let Some((so, si)) = shift {
                builder.set_mixed();
                builder.shifts(so, si);
            }
            for (n, bytes, a, b) in entries {
                match bytes.len() {
                    1 => {}
                    2 => builder.set_mixed(),
                    _ => return Err(format!("{n} 行目: EBCDIC の符号は 1〜2 バイトです")),
                }
                builder.add(&bytes, a, b, 0);
            }
            Kind::Ebcdic(builder.finish())
        }
    };
    let base = match base {
        Some(Encoding::Ebcdic(c, _)) => c.name().to_owned(),
        Some(e) => e.name().to_owned(),
        None => String::new(),
    };
    Ok(Mapping {
        name: Box::leak(name.to_owned().into_boxed_str()),
        base,
        path: None,
        kind,
    })
}

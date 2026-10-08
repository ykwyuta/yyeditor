//! レイアウトカタログ（15 章 6.6）。
//!
//! 1 つのフォルダ（ルート。設定の `[sheet] layout_catalog`、空なら `%APPDATA%\yyeditor\layouts`）の下に、
//! 固定長ファイルのレイアウトの定義ファイルを置く。サブフォルダで分けてよく、カタログの中の名前はルートからの
//! 相対パス（区切りは `/`。例: `受注/ORDER.yyl`）。
//!
//! - **定義ファイル（`.yyl`）**: TOML。レイアウト（名前とコピーブック）のほか、文字コード・レコードの区切り・
//!   2 進数の並び・説明、マルチレイアウトなら 1 行のデータ長（`data_len`）を持つ。`data_len` があれば
//!   マルチレイアウト。
//! - **コピーブック（`.cpy`・`.cbl`・`.cob`・`.copy`）**: COBOL のコピーブックをそのまま置いてもよい（読むだけ
//!   でなく、単一のレイアウトなら書ける）。文字コードなどは持たないので、ダイアログで選ぶ。文字コードは推定する
//!   （UTF-8・Shift_JIS など）。
//!
//! 名前はルートの外を指せない（`..`・絶対パス・ドライブ名を受け付けない）。

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use yy_cobol::{Charset, Codec};

use crate::fixed::{FixedSpec, RecordSep};

/// 定義ファイルの拡張子。
pub const DEF_EXT: &str = "yyl";
/// カタログに出すコピーブックの拡張子。
pub const COPYBOOK_EXTS: [&str; 4] = ["cpy", "cbl", "cob", "copy"];
/// 下のフォルダをたどる深さの上限。
const MAX_DEPTH: usize = 16;

/// 定義ファイルの種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `.yyl`
    Definition,
    /// コピーブックだけ
    Copybook,
}

/// カタログの 1 件。
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// ルートからの相対パス（区切りは `/`）
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
}

/// レイアウトの定義（定義ファイルの中身）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutDef {
    /// （名前, コピーブック）。単一のレイアウトなら 1 つ
    pub layouts: Vec<(String, String)>,
    /// 1 行のデータ長（マルチレイアウトなら必ずある）
    pub data_len: Option<usize>,
    pub charset: Option<Charset>,
    pub separator: Option<RecordSep>,
    pub little_endian: Option<bool>,
    /// 説明（任意）
    pub description: String,
}

impl LayoutDef {
    /// マルチレイアウトか。
    pub fn is_multi(&self) -> bool {
        self.data_len.is_some()
    }

    /// シートの設定から。
    pub fn from_spec(spec: &FixedSpec, description: &str) -> LayoutDef {
        let layouts = if spec.is_multi() {
            spec.multi
                .iter()
                .map(|m| (m.name.to_string(), m.copybook.to_string()))
                .collect()
        } else {
            vec![(String::new(), spec.copybook.to_string())]
        };
        LayoutDef {
            layouts,
            data_len: spec.is_multi().then_some(spec.data_len),
            charset: Some(spec.codec.charset),
            separator: Some(spec.separator),
            little_endian: Some(spec.codec.little_endian),
            description: description.to_string(),
        }
    }

    /// 単一のレイアウトのコピーブック（マルチレイアウトなら最初のもの）。
    pub fn copybook(&self) -> &str {
        self.layouts.first().map_or("", |l| l.1.as_str())
    }

    /// 設定にする。定義にない文字コード・区切り・2 進数の並びは `codec`・`separator` から。
    pub fn to_spec(&self, codec: Codec, separator: RecordSep) -> Result<FixedSpec, String> {
        let codec = Codec {
            charset: self.charset.unwrap_or(codec.charset),
            little_endian: self.little_endian.unwrap_or(codec.little_endian),
        };
        let sep = self.separator.unwrap_or(separator);
        match self.data_len {
            Some(n) => FixedSpec::new_multi(&self.layouts, n, codec, sep),
            None => FixedSpec::new(self.copybook(), codec, sep),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DefFile {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    charset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    separator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    little_endian: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data_len: Option<usize>,
    #[serde(default)]
    layout: Vec<LayoutFile>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutFile {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    name: String,
    copybook: String,
}

/// 定義ファイル（`.yyl`）の文字列を読む。
pub fn parse_def(text: &str) -> Result<LayoutDef, String> {
    let f: DefFile = toml::from_str(text).map_err(|e| format!("定義ファイルを読めません: {e}"))?;
    if f.layout.is_empty() {
        return Err("レイアウト（[[layout]]）がありません".into());
    }
    let charset = match &f.charset {
        Some(s) => {
            Some(Charset::from_name(s).ok_or_else(|| format!("文字コード {s} は使えません"))?)
        }
        None => None,
    };
    let separator = match &f.separator {
        Some(s) => Some(
            RecordSep::from_name(&s.to_ascii_lowercase()).ok_or_else(|| {
                format!("レコードの区切り {s} は使えません（none・crlf・lf・nl）")
            })?,
        ),
        None => None,
    };
    if f.data_len.is_none() && f.layout.len() > 1 {
        return Err(
            "レイアウトが 2 つ以上あるときは、マルチレイアウトの 1 行のデータ長（data_len）が要ります".into(),
        );
    }
    Ok(LayoutDef {
        layouts: f.layout.into_iter().map(|l| (l.name, l.copybook)).collect(),
        data_len: f.data_len,
        charset,
        separator,
        little_endian: f.little_endian,
        description: f.description,
    })
}

/// 定義ファイル（`.yyl`）の文字列にする。
pub fn def_text(def: &LayoutDef) -> String {
    let f = DefFile {
        description: def.description.clone(),
        charset: def.charset.map(|c| c.name().to_string()),
        separator: def.separator.map(|s| s.name().to_string()),
        little_endian: def.little_endian,
        data_len: def.data_len,
        layout: def
            .layouts
            .iter()
            .map(|(name, copybook)| LayoutFile {
                name: name.clone(),
                copybook: copybook.replace("\r\n", "\n"),
            })
            .collect(),
    };
    let body = toml::to_string(&f).unwrap_or_default();
    format!("# yysheet のレイアウト定義（レイアウトカタログ）\n{body}")
}

fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if ext == DEF_EXT {
        Some(Kind::Definition)
    } else if COPYBOOK_EXTS.contains(&ext.as_str()) {
        Some(Kind::Copybook)
    } else {
        None
    }
}

/// カタログの定義ファイルの一覧（名前の順）。ルートがなければ空。
pub fn list(root: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    if !root.is_dir() {
        return Ok(out);
    }
    walk(root, root, 0, &mut out)?;
    out.sort_by_key(|e| e.name.to_lowercase());
    Ok(out)
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<Entry>) -> io::Result<()> {
    for e in std::fs::read_dir(dir)?.flatten() {
        let path = e.path();
        let file_name = e.file_name().to_string_lossy().into_owned();
        // 隠しフォルダ・ファイル（.git など）は見ない
        if file_name.starts_with('.') {
            continue;
        }
        let Ok(ft) = e.file_type() else {
            continue;
        };
        if ft.is_dir() {
            if depth + 1 < MAX_DEPTH {
                // 読めないフォルダは飛ばす
                let _ = walk(root, &path, depth + 1, out);
            }
            continue;
        }
        let Some(kind) = kind_of(&path) else {
            continue;
        };
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let name = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        out.push(Entry { name, path, kind });
    }
    Ok(())
}

/// カタログの名前を確かめて、ファイルの場所にする（ルートの外は指せない）。拡張子がなければ `.yyl` を付ける。
pub fn resolve(root: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("名前を入力してください".into());
    }
    let mut path = root.to_path_buf();
    let parts: Vec<&str> = name.split(['/', '\\']).collect();
    for (i, p) in parts.iter().enumerate() {
        let p = p.trim();
        if p.is_empty() || p == "." || p == ".." {
            return Err(format!(
                "名前 {name} は使えません（ルートの下の相対パス。例: 受注/ORDER）"
            ));
        }
        if p.chars()
            .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || c.is_control())
        {
            return Err(format!(
                "名前 {name} に使えない文字があります（< > : \" | ? *）"
            ));
        }
        if p.starts_with('.') {
            return Err(format!("名前 {name} は . で始められません"));
        }
        if i + 1 == parts.len() && p.ends_with([' ', '.']) {
            return Err(format!("名前 {name} は空白や . で終われません"));
        }
        path.push(p);
    }
    if kind_of(&path).is_none() {
        let file = path
            .file_name()
            .map(|f| format!("{}.{DEF_EXT}", f.to_string_lossy()))
            .unwrap_or_default();
        path.set_file_name(file);
    }
    Ok(path)
}

/// カタログの定義を読む。
pub fn read(root: &Path, name: &str) -> Result<LayoutDef, String> {
    let path = resolve(root, name)?;
    read_path(&path)
}

/// 定義ファイルかコピーブックを読む。
pub fn read_path(path: &Path) -> Result<LayoutDef, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{} を読めません: {e}", path.display()))?;
    let det = yy_encoding::detect(&bytes, true);
    let (text, _) = yy_encoding::decode_all(det.encoding, &bytes[det.bom_len..], false);
    let text = String::from_utf8_lossy(&text).into_owned();
    match kind_of(path) {
        Some(Kind::Copybook) => Ok(LayoutDef {
            layouts: vec![(
                path.file_stem()
                    .map(|s| s.to_string_lossy().to_uppercase())
                    .unwrap_or_default(),
                text,
            )],
            ..LayoutDef::default()
        }),
        _ => parse_def(&text).map_err(|e| format!("{}: {e}", path.display())),
    }
}

/// カタログに書く（同じ名前があれば上書き。フォルダは作る）。コピーブックの拡張子なら、単一のレイアウトの
/// コピーブックだけを書く（文字コードなどは残らない）。書いたファイルを返す。
pub fn write(root: &Path, name: &str, def: &LayoutDef) -> Result<PathBuf, String> {
    let path = resolve(root, name)?;
    let text = match kind_of(&path) {
        Some(Kind::Copybook) => {
            if def.is_multi() {
                return Err(format!(
                    "マルチレイアウトはコピーブック（.{}）には書けません。.{DEF_EXT} で保存してください",
                    path.extension()
                        .map(|e| e.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
            }
            let mut t = def.copybook().replace("\r\n", "\n");
            if !t.ends_with('\n') {
                t.push('\n');
            }
            t
        }
        _ => def_text(def),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("フォルダ {} を作れません: {e}", dir.display()))?;
    }
    // 途中で失敗しても元のファイルを壊さない（一時ファイルに書いて置き換える）
    let tmp = path.with_extension("yyl-tmp");
    std::fs::write(&tmp, text.as_bytes())
        .and_then(|_| std::fs::rename(&tmp, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("{} に書けません: {e}", path.display())
        })?;
    Ok(path)
}

/// ファイルの場所からカタログの名前（ルートの下でなければ `None`）。
pub fn name_of(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use yy_encoding::Ccsid;

    const ORDER: &str = "01 ORDER-REC.\n 05 ORDER-ID PIC 9(6).\n 05 AMOUNT PIC S9(7)V99 COMP-3.\n";
    const HDR: &str = "01 H.\n 05 TYP PIC X.\n 05 DT PIC 9(8).\n";

    #[test]
    fn writes_lists_and_reads_definitions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("layouts");
        // ルートがなければ空
        assert!(list(&root).unwrap().is_empty());
        let codec = Codec::new(Charset::Ebcdic(Ccsid::Ibm930));
        let spec = FixedSpec::new(ORDER, codec, RecordSep::None).unwrap();
        let def = LayoutDef::from_spec(&spec, "受注ファイル");
        let p = write(&root, "受注/ORDER", &def).unwrap();
        assert_eq!(p, root.join("受注").join("ORDER.yyl"));
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("charset = \"IBM-930\""), "{text}");
        assert!(text.contains("separator = \"none\""), "{text}");
        // マルチレイアウト
        let multi = FixedSpec::new_multi(
            &[("HDR".into(), HDR.into()), ("DTL".into(), ORDER.into())],
            20,
            Codec::new(Charset::Ms932),
            RecordSep::Crlf,
        )
        .unwrap();
        write(&root, "multi/TRAN.yyl", &LayoutDef::from_spec(&multi, "")).unwrap();
        // コピーブックをそのまま置いたもの（Shift_JIS の注記つき）・関係のないファイル・隠しフォルダ
        let mut sjis = b"      * \x8e\xf3\x92\x8d\n".to_vec();
        sjis.extend_from_slice(HDR.as_bytes());
        std::fs::write(root.join("hdr.cpy"), &sjis).unwrap();
        std::fs::write(root.join("memo.txt"), "x").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git").join("x.yyl"), "x").unwrap();
        let names: Vec<(String, Kind)> = list(&root)
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.kind))
            .collect();
        assert_eq!(
            names,
            [
                ("hdr.cpy".to_string(), Kind::Copybook),
                ("multi/TRAN.yyl".to_string(), Kind::Definition),
                ("受注/ORDER.yyl".to_string(), Kind::Definition),
            ]
        );
        // 読み直すと同じ設定
        let back = read(&root, "受注/ORDER.yyl").unwrap();
        assert_eq!(back, def);
        let again = back
            .to_spec(Codec::new(Charset::Ms932), RecordSep::Crlf)
            .unwrap();
        assert_eq!(again, spec);
        let back = read(&root, "multi/TRAN").unwrap();
        assert!(back.is_multi());
        let again = back
            .to_spec(Codec::new(Charset::Ms932), RecordSep::Crlf)
            .unwrap();
        assert_eq!(again, multi);
        // コピーブック: 文字コードなどはダイアログの値、名前はファイル名
        let cb = read(&root, "hdr.cpy").unwrap();
        assert_eq!(cb.layouts[0].0, "HDR");
        assert!(cb.copybook().contains("受注"), "{}", cb.copybook());
        assert_eq!(cb.charset, None);
        let s = cb
            .to_spec(Codec::new(Charset::Ms932), RecordSep::Lf)
            .unwrap();
        assert_eq!((s.layout.record_len, s.separator), (9, RecordSep::Lf));
        // コピーブックとして書く（単一のレイアウトだけ）
        write(&root, "out/ORDER.cpy", &def).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("out").join("ORDER.cpy")).unwrap(),
            ORDER
        );
        assert!(write(&root, "out/TRAN.cpy", &LayoutDef::from_spec(&multi, "")).is_err());
        // 上書き
        let mut d2 = def.clone();
        d2.description = "改訂".into();
        write(&root, "受注/ORDER", &d2).unwrap();
        assert_eq!(read(&root, "受注/ORDER").unwrap().description, "改訂");
        assert_eq!(name_of(&root, &p).as_deref(), Some("受注/ORDER.yyl"));
    }

    #[test]
    fn names_stay_inside_the_root() {
        let root = Path::new("/catalog");
        for bad in [
            "",
            "../x",
            "a/../b",
            "/etc/passwd",
            "\\\\server\\x",
            "C:\\x",
            "a//b",
            ".hidden",
            "a/b.",
            "a?b",
        ] {
            assert!(resolve(root, bad).is_err(), "{bad}");
        }
        assert_eq!(
            resolve(root, "受注\\ORDER").unwrap(),
            root.join("受注").join("ORDER.yyl")
        );
        assert_eq!(resolve(root, "x.cpy").unwrap(), root.join("x.cpy"));
    }

    #[test]
    fn reads_the_example_in_the_guide() {
        // yysheet のヘルプ（レイアウトカタログ）の例と同じ
        let text = r#"description = "受注ファイル"
charset = "IBM-930"      # MS932・IBM-930・IBM-939・IBM-1399 など
separator = "none"       # none・crlf・lf・nl
little_endian = false
# data_len = 120         # マルチレイアウトなら 1 行のデータ長（あるとマルチレイアウト）

[[layout]]
name = "ORDER"           # マルチレイアウトでは必須
copybook = """
01 ORDER-REC.
   05 ORDER-ID  PIC 9(6).
   05 AMOUNT    PIC S9(7)V99 COMP-3.
"""
"#;
        let def = parse_def(text).unwrap();
        assert_eq!(def.description, "受注ファイル");
        let spec = def
            .to_spec(Codec::new(Charset::Ms932), RecordSep::Crlf)
            .unwrap();
        assert_eq!(spec.layout.record_len, 11);
        assert_eq!(spec.codec.charset, Charset::Ebcdic(Ccsid::Ibm930));
        assert_eq!(spec.separator, RecordSep::None);
    }

    #[test]
    fn rejects_broken_definitions() {
        assert!(parse_def("").is_err());
        assert!(parse_def("charset = \"XX\"\n[[layout]]\ncopybook = \"01 A PIC X.\"\n").is_err());
        assert!(parse_def("unknown = 1\n[[layout]]\ncopybook = \"01 A PIC X.\"\n").is_err());
        let two = "[[layout]]\nname = \"A\"\ncopybook = \"01 A PIC X.\"\n[[layout]]\nname = \"B\"\ncopybook = \"01 B PIC X.\"\n";
        assert!(parse_def(two).is_err());
        let d = parse_def(&format!("data_len = 5\n{two}")).unwrap();
        assert!(d.is_multi());
        let d =
            parse_def("separator = \"CRLF\"\n[[layout]]\ncopybook = \"01 A PIC X.\"\n").unwrap();
        assert_eq!(d.separator, Some(RecordSep::Crlf));
    }
}

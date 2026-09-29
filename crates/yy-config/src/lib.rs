//! yyeditor の設定。
//!
//! `%APPDATA%\yyeditor\config.toml`（Windows 以外では `$XDG_CONFIG_HOME/yyeditor/config.toml`）
//! から読み込む。ファイルがない・項目が欠けている場合は既定値を使う。

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("設定ファイルを読み込めません: {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("設定ファイルの形式が正しくありません: {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub editor: EditorConfig,
    pub view: ViewConfig,
    pub colors: Colors,
    /// ファイル種類（拡張子 → モード・文字コード等）。M3 以降で使用する。
    pub filetype: BTreeMap<String, FileType>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            editor: EditorConfig::default(),
            view: ViewConfig::default(),
            colors: Colors::default(),
            filetype: default_filetypes(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorConfig {
    /// フォント名。日本語グリフは DirectWrite のフォールバックで補われる
    pub font_family: String,
    /// フォントサイズ（ポイント）
    pub font_size: f32,
    pub tab_width: u32,
    /// 東アジアの曖昧幅文字（①、○ など）を全角（2 桁）として数えるか。矩形選択の桁計算に使う
    pub ambiguous_wide: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        EditorConfig {
            font_family: "Consolas".into(),
            font_size: 11.0,
            tab_width: 4,
            ambiguous_wide: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ViewConfig {
    pub line_numbers: bool,
    /// 長大行を表示用に分割する単位（バイト）。02 章 3.5 参照
    pub max_row_bytes: u32,
    /// 行数がこれ以下なら縦スクロールバーを行数比例にする（超えるとバイト位置比例）
    pub line_scroll_limit: u64,
    /// マウスホイール 1 目盛りでスクロールする行数
    pub wheel_lines: u32,
}

impl Default for ViewConfig {
    fn default() -> Self {
        ViewConfig {
            line_numbers: true,
            max_row_bytes: 8192,
            line_scroll_limit: 1_000_000,
            wheel_lines: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Colors {
    pub background: Color,
    pub foreground: Color,
    pub gutter_background: Color,
    pub line_number: Color,
    /// 行数が未確定の範囲で推定した行番号
    pub line_number_estimated: Color,
    /// 不正なバイト列（`\xNN` 表示）
    pub invalid_byte: Color,
    /// 制御文字
    pub control: Color,
    /// 選択範囲の背景
    pub selection: Color,
    pub caret: Color,
}

impl Default for Colors {
    fn default() -> Self {
        Colors {
            background: Color::rgb(0xFF, 0xFF, 0xFF),
            foreground: Color::rgb(0x1E, 0x1E, 0x1E),
            gutter_background: Color::rgb(0xF3, 0xF3, 0xF3),
            line_number: Color::rgb(0x23, 0x78, 0x93),
            line_number_estimated: Color::rgb(0xB0, 0xB0, 0xB0),
            invalid_byte: Color::rgb(0xC0, 0x30, 0x30),
            control: Color::rgb(0x80, 0x80, 0xC0),
            selection: Color::rgb(0xAD, 0xD6, 0xFF),
            caret: Color::rgb(0x00, 0x00, 0x00),
        }
    }
}

/// `#RRGGBB` 形式の色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b }
    }

    /// 0.0〜1.0 の RGB。
    pub fn to_f32(self) -> [f32; 3] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
        ]
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

impl FromStr for Color {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix('#')
            .filter(|h| h.len() == 6 && h.is_ascii())
            .ok_or_else(|| format!("色は #RRGGBB 形式で指定してください: {s}"))?;
        let v = u32::from_str_radix(hex, 16).map_err(|_| format!("不正な色: {s}"))?;
        Ok(Color::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditMode {
    #[default]
    Text,
    Delimited,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileType {
    pub extensions: Vec<String>,
    pub mode: EditMode,
    pub delimiter: Option<String>,
    pub quote: Option<char>,
    pub rfc4180: Option<bool>,
    pub encoding: Option<String>,
    pub syntax: Option<String>,
}

fn default_filetypes() -> BTreeMap<String, FileType> {
    let mut m = BTreeMap::new();
    m.insert(
        "csv".into(),
        FileType {
            extensions: vec!["csv".into()],
            mode: EditMode::Delimited,
            delimiter: Some(",".into()),
            quote: Some('"'),
            rfc4180: Some(true),
            ..Default::default()
        },
    );
    m.insert(
        "tsv".into(),
        FileType {
            extensions: vec!["tsv".into(), "tab".into()],
            mode: EditMode::Delimited,
            delimiter: Some("\t".into()),
            rfc4180: Some(false),
            ..Default::default()
        },
    );
    m
}

impl Config {
    /// TOML 文字列から読み込む。
    /// ユーザーが定義したファイル種類は、同名の組み込み定義を置き換え、それ以外の組み込み定義は残す。
    pub fn from_toml(s: &str) -> Result<Config, toml::de::Error> {
        let mut c: Config = toml::from_str(s)?;
        for (k, v) in default_filetypes() {
            c.filetype.entry(k).or_insert(v);
        }
        Ok(c)
    }

    /// 既定の設定ファイルの場所。
    pub fn default_path() -> Option<PathBuf> {
        config_dir().map(|d| d.join("config.toml"))
    }

    /// `path` から読み込む。ファイルが存在しなければ既定値を返す。
    pub fn load_from(path: &Path) -> Result<Config, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(s) => Config::from_toml(&s).map_err(|source| ConfigError::Parse {
                path: path.to_owned(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(source) => Err(ConfigError::Io {
                path: path.to_owned(),
                source,
            }),
        }
    }

    /// 既定の場所から読み込む。失敗した場合は既定値とエラーを返す（起動は継続する）。
    pub fn load() -> (Config, Option<ConfigError>) {
        match Config::default_path() {
            Some(p) => match Config::load_from(&p) {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(e)),
            },
            None => (Config::default(), None),
        }
    }

    /// 拡張子（先頭の `.` なし、大文字小文字を区別しない）からファイル種類を探す。
    pub fn filetype_for_extension(&self, ext: &str) -> Option<(&str, &FileType)> {
        self.filetype
            .iter()
            .find(|(_, ft)| ft.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)))
            .map(|(k, v)| (k.as_str(), v))
    }
}

/// 設定ディレクトリ。
pub fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("yyeditor"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|p| p.join("yyeditor"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_toml_gives_defaults() {
        assert_eq!(Config::from_toml("").unwrap(), Config::default());
    }

    #[test]
    fn partial_override_keeps_other_defaults() {
        let c = Config::from_toml(
            r##"
            [editor]
            font_family = "BIZ UDゴシック"
            [colors]
            background = "#1e1e1e"
            [filetype.log]
            extensions = ["log"]
            "##,
        )
        .unwrap();
        assert_eq!(c.editor.font_family, "BIZ UDゴシック");
        assert_eq!(c.editor.tab_width, 4);
        assert_eq!(c.colors.background, Color::rgb(0x1E, 0x1E, 0x1E));
        assert_eq!(c.colors.foreground, Colors::default().foreground);
        assert_eq!(c.filetype_for_extension("LOG").unwrap().0, "log");
        assert_eq!(c.filetype_for_extension("csv").unwrap().0, "csv");
    }

    #[test]
    fn rejects_bad_color_and_unknown_keys() {
        assert!(Config::from_toml("[colors]\nbackground = \"red\"").is_err());
        assert!(Config::from_toml("[editor]\nfont = \"x\"").is_err());
    }

    #[test]
    fn roundtrips_through_toml() {
        let c = Config::default();
        let s = toml::to_string(&c).unwrap();
        assert_eq!(Config::from_toml(&s).unwrap(), c);
    }

    #[test]
    fn load_from_missing_file_is_default() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load_from(&dir.path().join("none.toml")).unwrap();
        assert_eq!(c, Config::default());
        let p = dir.path().join("bad.toml");
        std::fs::write(&p, "[[[").unwrap();
        assert!(matches!(
            Config::load_from(&p),
            Err(ConfigError::Parse { .. })
        ));
    }
}

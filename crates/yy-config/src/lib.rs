//! yyeditor の設定。
//!
//! `%APPDATA%\yyeditor\config.toml`（Windows 以外では `$XDG_CONFIG_HOME/yyeditor/config.toml`）
//! から読み込む。ファイルがない・項目が欠けている場合は既定値を使う。

pub mod recent;
pub mod workspace;

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
    pub workspace: WorkspaceConfig,
    /// ファイル種類（拡張子 → モード・文字コード等）。M3 以降で使用する。
    pub filetype: BTreeMap<String, FileType>,
    /// SSH 接続先のファイルの編集（11 章 4.4）
    pub remote: RemoteConfig,
    /// ターミナル（yyterm。12 章）
    pub terminal: TerminalConfig,
    /// ファイル転送（yysftp。13 章）
    pub transfer: TransferConfig,
    pub tn3270: Tn3270Config,
    /// スプレッドシート（yysheet。15 章）
    pub sheet: SheetConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            editor: EditorConfig::default(),
            view: ViewConfig::default(),
            colors: Colors::default(),
            workspace: WorkspaceConfig::default(),
            filetype: default_filetypes(),
            remote: RemoteConfig::default(),
            terminal: TerminalConfig::default(),
            transfer: TransferConfig::default(),
            tn3270: Tn3270Config::default(),
            sheet: SheetConfig::default(),
        }
    }
}

/// スプレッドシート（yysheet。15 章）の設定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SheetConfig {
    /// アプリが使うメモリの上限（GB。15 章 3.6）
    pub memory_limit_gb: f64,
    /// 作業ファイルのフォルダ（空なら OS の一時フォルダ）
    pub work_dir: String,
    /// 格子のフォント（空なら Yu Gothic UI）
    pub font_family: String,
    /// 格子の文字の大きさ（ポイント）
    pub font_size: f32,
    /// 既定の列幅（半角の文字数）
    pub column_width: f32,
    /// レイアウトカタログのルート（固定長ファイルのレイアウトの定義ファイルを置くフォルダ。空なら
    /// `%APPDATA%\yyeditor\layouts`。15 章 6.6）
    pub layout_catalog: String,
    /// 接続先のファイル・フォルダの読み書きに、接続先のエージェントを使う（使わなければ SFTP。
    /// 接続先に何も置かない）
    pub use_agent: bool,
}

impl Default for SheetConfig {
    fn default() -> Self {
        SheetConfig {
            memory_limit_gb: 8.0,
            work_dir: String::new(),
            font_family: String::new(),
            font_size: 11.0,
            column_width: 8.43,
            layout_catalog: String::new(),
            use_agent: false,
        }
    }
}

/// 3270 のセッション（yyterm。14 章）の設定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tn3270Config {
    /// モデル（2: 24×80、3: 32×80、4: 43×80、5: 27×132）
    pub model: u8,
    /// 文字コード（CCSID。037・1047・930・939・1390・1399 など）
    pub ccsid: u32,
    /// 端末の種類（空なら IBM-3278-<model>-E）
    pub terminal_type: String,
    /// TN3270E を使う（断られたら TN3270）
    pub tn3270e: bool,
    /// ホストにしかないキー（PF13〜24・PA・Clear など）のボタンを画面の右に出す
    pub keypad: bool,
    /// 3270 のタブを開いたときから通信の記録（`logs\tn3270-trace-*.log`）をとる
    pub trace: bool,
    /// TLS で加えて信頼する認証局の証明書（PEM のファイル。空なら OS の信頼する認証局だけ）
    pub ca_file: String,
    /// TLS のクライアント証明書（PEM のファイル。秘密鍵も入っていてよい。空なら使わない）
    pub client_cert: String,
    /// クライアント証明書の秘密鍵（PEM。空なら `client_cert` から読む）
    pub client_key: String,
    /// プリンター（3287）の出力
    pub printer: Tn3270Printer,
    /// マクロ
    #[serde(rename = "macro")]
    pub macros: Tn3270Macro,
    /// 接続先ごとの設定（名前 → 値）
    pub host: BTreeMap<String, Tn3270Host>,
}

impl Default for Tn3270Config {
    fn default() -> Self {
        Tn3270Config {
            model: 2,
            ccsid: 930,
            terminal_type: String::new(),
            tn3270e: true,
            keypad: true,
            trace: false,
            ca_file: String::new(),
            client_cert: String::new(),
            client_key: String::new(),
            printer: Tn3270Printer::default(),
            macros: Tn3270Macro::default(),
            host: BTreeMap::new(),
        }
    }
}

/// 3270 のマクロ（14 章 13）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tn3270Macro {
    /// マクロのフォルダ（空なら `%APPDATA%\yyeditor\macros`）
    pub folder: String,
    /// マクロが書き出すフォルダ（空ならドキュメントの `yyterm\macros-out`）
    pub output: String,
    /// 待つ操作の既定の時間の上限（秒）
    pub timeout: u64,
}

impl Default for Tn3270Macro {
    fn default() -> Self {
        Tn3270Macro {
            folder: String::new(),
            output: String::new(),
            timeout: 30,
        }
    }
}

/// 3270 のプリンター（3287）の出力（14 章 12.3）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tn3270Printer {
    /// 印刷を受け取ったら: `printer`（Windows のプリンター）・`pdf`・`text`・`ask`（一覧に溜める）
    pub output: String,
    /// Windows のプリンターの名前（空なら既定のプリンター）
    pub printer_name: String,
    /// PDF・テキストを保存するフォルダー（空ならドキュメントの `yyterm-print`）
    pub folder: String,
    /// PRINT-EOJ のないホストで、この秒数データが来なければジョブの終わりとみなす
    pub eoj_timeout: u64,
}

impl Default for Tn3270Printer {
    fn default() -> Self {
        Tn3270Printer {
            output: "ask".into(),
            printer_name: String::new(),
            folder: String::new(),
            eoj_timeout: 5,
        }
    }
}

/// 3270 の接続先ごとの設定（書いていない項目は `[tn3270]` の値）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tn3270Host {
    pub host: String,
    /// ポート（省略すると 23。`tls = "tls"` なら 992）
    pub port: Option<u16>,
    /// TLS: `none`（既定）・`tls`（暗黙の TLS）・`starttls`（Telnet の STARTTLS）
    pub tls: Option<String>,
    /// TLS で加えて信頼する認証局の証明書（省略すると `[tn3270]` の値）
    pub ca_file: Option<String>,
    /// TLS のクライアント証明書（省略すると `[tn3270]` の値。空の文字列なら使わない）
    pub client_cert: Option<String>,
    /// クライアント証明書の秘密鍵
    pub client_key: Option<String>,
    /// TN3270E で求める LU 名
    pub lu: Option<String>,
    pub model: Option<u8>,
    pub ccsid: Option<u32>,
    /// この SSH の接続（`~/.ssh/config` の Host・`ユーザー@ホスト`）を経由する
    pub ssh: Option<String>,
    /// プリンターの LU 名（省略すると端末の LU に対応するプリンター。ASSOCIATE）
    pub printer_lu: Option<String>,
    /// `auto` なら端末を開いたらプリンターも開く（既定は `manual`）
    pub printer: Option<String>,
    /// 接続したら実行するマクロ（マクロのフォルダからの名前。例: `logon.rhai`）
    pub on_connect: Option<String>,
}

/// ファイル転送（yysftp）の設定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TransferConfig {
    /// 転送の方式（`sftp` か `scp`）
    pub protocol: String,
    /// 進みがないまま再接続してよい回数
    pub retries: u32,
    /// SCP のアップロードで 1 回に送る量（MiB。切断で失うのは多くてもこの量）
    pub scp_chunk_mb: u32,
    /// 名前が `.` で始まるファイルも表示する
    pub show_hidden: bool,
    /// ダウンロードの既定の保存先（空ならダウンロード フォルダ）
    pub download_dir: String,
    /// 接続先にエージェントを置いて使う（一覧・ファイル操作と、送り終えた内容の SHA-256 の照合）。
    /// 使わなければ SFTP だけ（接続先に何も置かない）
    pub use_agent: bool,
}

impl Default for TransferConfig {
    fn default() -> Self {
        TransferConfig {
            protocol: "sftp".into(),
            retries: 10,
            scp_chunk_mb: 32,
            show_hidden: false,
            download_dir: String::new(),
            use_agent: false,
        }
    }
}

/// ターミナル（yyterm）の設定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalConfig {
    /// 手元で起動するシェルのコマンドと引数。空なら PowerShell 7（pwsh.exe）、なければ
    /// Windows PowerShell、なければコマンド プロンプト
    pub shell: Vec<String>,
    /// フォント名（空ならエディタと同じ。既定は同梱の UDEV Gothic）
    pub font_family: String,
    /// フォントサイズ（ポイント。0 ならエディタと同じ）
    pub font_size: f32,
    /// スクロールバックの行数
    pub scrollback: u32,
    /// 東アジアの幅が曖昧な文字（○、※ など）を全角として扱う（接続先の設定と合わせる）
    pub ambiguous_wide: bool,
    /// SSH の接続先に知らせる端末の種類（TERM）
    pub term: String,
    /// 選択したら自動でコピーする
    pub copy_on_select: bool,
    /// 右クリックでメニューを出す（コピー・貼り付け・リンクを開く・作業フォルダを yysftp で開く など）。
    /// `false` なら、選択していればコピー、していなければ貼り付け（コンソールと同じ）
    pub right_click_menu: bool,
    /// ワークスペースのリモートのフォルダの一覧に、接続先のエージェントを使う（使わなければ SFTP。
    /// 接続先に何も置かない）。シェルにはエージェントを使わない
    pub use_agent: bool,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        TerminalConfig {
            shell: Vec::new(),
            font_family: String::new(),
            font_size: 0.0,
            scrollback: 10_000,
            ambiguous_wide: false,
            term: "xterm-256color".into(),
            copy_on_select: false,
            right_click_menu: true,
            use_agent: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceConfig {
    /// 「ターミナルで開く」で起動するコマンドと引数（`{dir}` はフォルダに置き換える）。
    /// 空なら Windows Terminal（`wt.exe -d {dir}`）、なければコマンド プロンプト
    pub terminal: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorConfig {
    /// フォント名。既定は実行ファイルに同梱の UDEV Gothic。
    /// ほかのフォントの日本語グリフは DirectWrite のフォールバックで補われる
    pub font_family: String,
    /// フォントサイズ（ポイント）
    pub font_size: f32,
    pub tab_width: u32,
    /// 東アジアの曖昧幅文字（①、○ など）を全角（2 桁）として数えるか。矩形選択の桁計算に使う
    pub ambiguous_wide: bool,
    /// 文字コードの自動判別に EBCDIC を含める（03 章 3.3）
    pub detect_ebcdic: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        EditorConfig {
            font_family: "UDEV Gothic".into(),
            font_size: 11.0,
            tab_width: 4,
            ambiguous_wide: true,
            detect_ebcdic: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ViewConfig {
    pub line_numbers: bool,
    /// 半角スペース・タブ・改行（CRLF・LF）を記号で表示する
    pub show_whitespace: bool,
    /// 長大行を表示用に分割する単位（バイト）。02 章 3.5 参照
    pub max_row_bytes: u32,
    /// 区切り文字モードで、これより短い行は分割せずに列を揃える（バイト。04 章 4）
    pub csv_max_row_bytes: u32,
    /// 区切り文字モードの 1 列の幅の上限（桁）。これより長いフィールドはその行だけ列がずれる
    pub csv_max_column_width: u32,
    /// 行数がこれ以下なら縦スクロールバーを行数比例にする（超えるとバイト位置比例）
    pub line_scroll_limit: u64,
    /// マウスホイール 1 目盛りでスクロールする行数
    pub wheel_lines: u32,
}

impl Default for ViewConfig {
    fn default() -> Self {
        ViewConfig {
            line_numbers: true,
            show_whitespace: true,
            max_row_bytes: 1 << 20,
            csv_max_row_bytes: 16 << 20,
            csv_max_column_width: 1000,
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
    /// 検索に一致した箇所の背景
    pub search_match: Color,
    pub caret: Color,
    /// カーソル位置の括弧と対応する括弧の背景
    pub bracket_match: Color,
    /// 空白・タブ・改行の記号
    pub whitespace: Color,
    /// シンタックスハイライトのトークンの色（`comment`・`keyword.control` など）。
    /// 細分類（`keyword.control`）がなければ親（`keyword`）の色を使う
    pub syntax: BTreeMap<String, Color>,
}

/// 既定のトークンの色（ライト）。
fn default_syntax_colors() -> BTreeMap<String, Color> {
    [
        ("comment", "#008000"),
        ("string", "#A31515"),
        ("escape", "#EE0000"),
        ("keyword", "#0000FF"),
        ("type", "#267F99"),
        ("builtin", "#267F99"),
        ("constant", "#0070C1"),
        ("number", "#098658"),
        ("function", "#795E26"),
        ("preprocessor", "#AF00DB"),
        ("variable", "#001080"),
        ("property", "#0451A5"),
        ("attribute", "#E50000"),
        ("tag", "#800000"),
        ("punctuation", "#808080"),
        ("label", "#795E26"),
        ("section", "#0000FF"),
        ("heading", "#800000"),
        ("strong", "#000080"),
        ("emphasis", "#800080"),
        ("link", "#0066CC"),
        ("code", "#A31515"),
        ("regex", "#811F3F"),
        ("meta", "#808080"),
        ("inserted", "#098658"),
        ("deleted", "#A31515"),
        ("changed", "#0451A5"),
        ("error", "#CD3131"),
        ("warning", "#BF8803"),
        ("info", "#1A85FF"),
        ("debug", "#808080"),
        ("date", "#0451A5"),
        ("line-number", "#237893"),
        ("data", "#5A5A5A"),
        ("operator", "#000000"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.parse().unwrap()))
    .collect()
}

impl Colors {
    /// トークン名の色（細分類がなければ親の色）。
    pub fn syntax_color(&self, token: &str) -> Option<Color> {
        let mut name = token;
        loop {
            if let Some(c) = self.syntax.get(name) {
                return Some(*c);
            }
            name = &name[..name.rfind('.')?];
        }
    }
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
            search_match: Color::rgb(0xFF, 0xE0, 0x8A),
            caret: Color::rgb(0x00, 0x00, 0x00),
            bracket_match: Color::rgb(0xD0, 0xE8, 0xD0),
            whitespace: Color::rgb(0xA8, 0xB4, 0xC8),
            syntax: default_syntax_colors(),
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

/// SSH 接続先のファイルの編集の設定（11 章 4.4）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteConfig {
    /// `%USERPROFILE%\.ssh\config` を読む（読むだけ。OpenSSH のプログラムは使わない）
    pub read_ssh_config: bool,
    /// `%USERPROFILE%\.ssh\known_hosts` を読む（読むだけ。承認した鍵は yyeditor の known_hosts に書く）
    pub read_ssh_known_hosts: bool,
    /// 接続の死活確認の間隔（秒）
    pub keepalive_secs: u32,
    /// 接続先にエージェントを置くフォルダ（空なら `~/.yyeditor/agent`）
    pub agent_dir: String,
    /// すべての接続先に使うプロキシ（`http://ホスト:ポート`・`socks5://ホスト:ポート`。空なら使わない）
    pub proxy: String,
    /// パスワードを Windows の資格情報マネージャーに保存できるようにする（保存するかは入力のたびに選ぶ）
    pub remember_passwords: bool,
    /// 接続先ごとの設定（`[remote.host.<名前>]`）
    pub host: BTreeMap<String, RemoteHost>,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        RemoteConfig {
            read_ssh_config: true,
            read_ssh_known_hosts: true,
            keepalive_secs: 15,
            agent_dir: String::new(),
            proxy: String::new(),
            remember_passwords: true,
            host: BTreeMap::new(),
        }
    }
}

/// 接続先ごとの設定。書いていない項目は `~/.ssh/config`、それもなければ既定値を使う。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteHost {
    /// 接続するホスト名（省略すると設定の名前）
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    /// 秘密鍵のファイル
    pub identity_file: Option<String>,
    /// 踏み台（`[ユーザー@]ホスト[:ポート]` のカンマ区切り。`none` で ~/.ssh/config の指定も使わない）
    pub proxy_jump: Option<String>,
    /// この接続先に使うプロキシ（`none` で共通のプロキシも使わない）
    pub proxy: Option<String>,
    /// この接続先でエージェントを置くフォルダ（ホームが noexec の場合など）
    pub agent_dir: Option<String>,
    /// ポートフォワーディング（ターミナルだけが使う。`L 8080:localhost:80`・`R 9000:localhost:3000`・
    /// `D 1080`。`~/.ssh/config` の LocalForward なども使う）
    pub forward: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileType {
    pub extensions: Vec<String>,
    pub mode: EditMode,
    pub delimiter: Option<String>,
    pub quote: Option<char>,
    pub rfc4180: Option<bool>,
    /// 開くときの文字コード（`IBM-930/fixed:80` など。省略すると自動判別）
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
        // トークンの色は書いたものだけを既定値に上書きする
        for (k, v) in default_syntax_colors() {
            c.colors.syntax.entry(k).or_insert(v);
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

    /// 設定ファイルがないときに作る内容（既定値をすべて書き出したもの）。
    pub fn default_file_contents() -> String {
        let body = toml::to_string(&Config::default()).unwrap_or_default();
        format!(
            "# yyeditor の設定（既定値）。変更は次に起動したときから反映されます。\n\
             # 項目の説明はヘルプ（F1）の「設定」を参照してください。\n\n{body}"
        )
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
    fn syntax_colors_merge_and_inherit() {
        let c = Config::from_toml(
            "[colors.syntax]\ncomment = \"#111111\"\n\"keyword.control\" = \"#222222\"\n",
        )
        .unwrap();
        assert_eq!(
            c.colors.syntax_color("comment"),
            Some(Color::rgb(0x11, 0x11, 0x11))
        );
        assert_eq!(
            c.colors.syntax_color("keyword.control"),
            Some(Color::rgb(0x22, 0x22, 0x22))
        );
        // 書いていない色は既定値、細分類は親の色
        assert_eq!(
            c.colors.syntax_color("string"),
            Colors::default().syntax_color("string")
        );
        assert_eq!(
            c.colors.syntax_color("string.quoted"),
            c.colors.syntax_color("string")
        );
        assert_eq!(c.colors.syntax_color("nothing"), None);
    }

    #[test]
    fn remote_hosts() {
        let c = Config::from_toml(
            r#"
            [remote]
            agent_dir = "/work/agent"
            proxy = "socks5://socks.example.co.jp:1080"
            [remote.host.build]
            hostname = "build01.example.co.jp"
            user = "yamada"
            port = 2222
            proxy_jump = "bastion,admin@gw:2022"
            [remote.host.lab]
            proxy = "none"
            "#,
        )
        .unwrap();
        assert!(c.remote.read_ssh_config);
        assert!(c.remote.remember_passwords);
        assert_eq!(c.terminal.scrollback, 10_000);
        assert_eq!(c.terminal.term, "xterm-256color");
        assert_eq!(c.transfer.protocol, "sftp");
        assert_eq!(c.transfer.retries, 10);
        assert_eq!(c.remote.agent_dir, "/work/agent");
        let h = &c.remote.host["build"];
        assert_eq!(h.hostname.as_deref(), Some("build01.example.co.jp"));
        assert_eq!(h.port, Some(2222));
        assert_eq!(h.identity_file, None);
        assert_eq!(h.proxy_jump.as_deref(), Some("bastion,admin@gw:2022"));
        assert_eq!(c.remote.proxy, "socks5://socks.example.co.jp:1080");
        assert_eq!(c.remote.host["lab"].proxy.as_deref(), Some("none"));
        assert!(
            Config::from_toml(
                "[remote.host.x]
portt = 1"
            )
            .is_err()
        );
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
    fn default_file_contents_load_as_default() {
        let s = Config::default_file_contents();
        assert!(s.starts_with("# yyeditor"));
        assert_eq!(Config::from_toml(&s).unwrap(), Config::default());
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

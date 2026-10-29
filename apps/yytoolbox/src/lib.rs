//! Offline tools. Text operations perform no network, clipboard or filesystem I/O.
//! Batch images use only explicitly supplied input and output paths.
mod dates;
mod dummy;
mod finance;
mod formats;
pub mod images;
mod random;
mod text;
pub mod video;
mod webtools;

use std::collections::BTreeMap;

pub type Params = BTreeMap<String, String>;
pub type Result<T> = std::result::Result<T, String>;
pub const MAX_INPUT: usize = 1024 * 1024;
pub const MAX_OUTPUT: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Regex,
    Base64Encode,
    Base64Decode,
    Hash,
    Jwt,
    Json,
    Sql,
    Password,
    DummyCsv,
    Compound,
    Loan,
    Calendar,
    AgeGrade,
    TimeZones,
    QrCode,
    Url,
    Convert,
    Html,
    Css,
    JavaScript,
    Recording,
    ResizeImages,
    VideoEdit,
}

pub const TOOLS: [Tool; 23] = [
    Tool::Regex,
    Tool::Base64Encode,
    Tool::Base64Decode,
    Tool::Hash,
    Tool::Jwt,
    Tool::Json,
    Tool::Sql,
    Tool::Password,
    Tool::DummyCsv,
    Tool::Compound,
    Tool::Loan,
    Tool::Calendar,
    Tool::AgeGrade,
    Tool::TimeZones,
    Tool::QrCode,
    Tool::Url,
    Tool::Convert,
    Tool::Html,
    Tool::Css,
    Tool::JavaScript,
    Tool::Recording,
    Tool::ResizeImages,
    Tool::VideoEdit,
];

#[derive(Clone, Debug)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub default: String,
    pub choices: &'static [&'static str],
}

impl Field {
    fn new(key: &'static str, label: &'static str, default: impl ToString) -> Self {
        Self {
            key,
            label,
            default: default.to_string(),
            choices: &[],
        }
    }
    fn select(key: &'static str, label: &'static str, choices: &'static [&'static str]) -> Self {
        Self {
            key,
            label,
            default: choices[0].into(),
            choices,
        }
    }
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Self::Regex => "正規表現テスト",
            Self::Base64Encode => "Base64 エンコード",
            Self::Base64Decode => "Base64 デコード",
            Self::Hash => "各種ハッシュ",
            Self::Jwt => "JWT デコード",
            Self::Json => "JSON 整形",
            Self::Sql => "SQL 整形",
            Self::Password => "条件付きパスワード候補",
            Self::DummyCsv => "定義からダミーCSV生成",
            Self::Compound => "複利・積立計算",
            Self::Loan => "返済計算",
            Self::Calendar => "和暦・西暦変換",
            Self::AgeGrade => "年齢・学年対応",
            Self::TimeZones => "時差一覧",
            Self::QrCode => "QRコード生成",
            Self::Url => "URL エンコード・デコード",
            Self::Convert => "XML・YAML・CSV・INI 相互変換",
            Self::Html => "HTML 整形",
            Self::Css => "CSS 整形",
            Self::JavaScript => "JavaScript 整形",
            Self::Recording => "画面録画",
            Self::ResizeImages => "画像の一括リサイズ",
            Self::VideoEdit => "動画編集（カット・クリップ）",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Regex => {
                "Rust正規表現。キャプチャ・名前付きグループを表示。位置はUTF-8バイト。後読み・後方参照は非対応。"
            }
            Self::Base64Encode => "入力テキストのUTF-8バイト列をエンコードします。",
            Self::Base64Decode => {
                "UTF-8以外は16進表示。結果の保存ではデコードしたバイト列をそのまま保存します。"
            }
            Self::Hash => "入力のUTF-8バイト列を計算。MD5・SHA-1は互換性確認用です。",
            Self::Jwt => {
                "JWSのヘッダー・ペイロードを表示します。署名・発行者・有効期限の検証は行いません。"
            }
            Self::Json => "JSONを検査し、文字列・数値の原文とキー順序を保って整形します。",
            Self::Sql => "SQLを整形します。データベースへの接続・実行は行いません。",
            Self::Password => {
                "各文字種の最低数を満たす候補をOSの安全な乱数で生成。最低数0の文字種は使いません。"
            }
            Self::DummyCsv => {
                "入力はYAMLの列定義です。行数で件数を指定し、「入力例」で分野の定義を読み込みます。連絡先・識別子はテスト用です。"
            }
            Self::Compound => {
                "年率を年の複利回数で割って計算します。税金・手数料なしの一定利率による試算です。"
            }
            Self::Loan => {
                "毎月返済・固定年率。利息は各回で丸め、最終回で残高を調整します。税金・手数料は含みません。"
            }
            Self::Calendar => {
                "西暦または明治以降の和暦。日付変換はグレゴリオ暦導入後の1873年以降。元年・M/T/S/H/Rにも対応。"
            }
            Self::AgeGrade => {
                "日本の4月2日〜翌4月1日生まれの学年区分。標準進級・4年制大学までの目安です。"
            }
            Self::TimeZones => {
                "同じ瞬間の現地日時・UTCオフセット・基準地域との時差。入力はIANA名を改行区切り。*で全地域。"
            }
            Self::QrCode => {
                "UTF-8テキストをQRコードにします。表示・SVG／PNG保存。コピーはSVGです。"
            }
            Self::Url => "UTF-8のパーセント変換。URL部品・フォーム・URI全体を選べます。",
            Self::Convert => {
                "構造保持は型と入れ子を専用表現で往復変換。標準形式は出力先で表せる構造に限定します。"
            }
            Self::Html | Self::Css | Self::JavaScript => {
                "構文を解析して整形します。コードは実行しません。字下げと折り返し幅を指定できます。"
            }
            Self::Recording => {
                "時間指定のMP4録画。小型バーの停止ボタンで途中保存。Escでは停止しません。操作バーは録画から除外します。"
            }
            Self::VideoEdit => {
                "時間範囲を残すクリップ／取り除くカット。例: 5-10, 20-30（秒または時:分:秒）。元動画を保ちMP4へ再エンコード。先頭の映像・音声を使用。"
            }
            Self::ResizeImages => {
                "実行で画像と出力フォルダを選択。比率維持・背景補完・透明背景。入力と既存ファイルを上書きしません。"
            }
        }
    }

    pub fn fields(self) -> Vec<Field> {
        let f = Field::new;
        let s = Field::select;
        match self {
            Self::Recording => vec![
                s(
                    "area",
                    "録画する範囲",
                    &["全画面（メイン）", "全画面（すべて）", "矩形選択"],
                ),
                f("seconds", "録画時間（秒）", "60"),
                f("fps", "フレーム毎秒", "15"),
                f("max_width", "最大録画幅px", "1920"),
                s("system_audio", "PC再生音", &["なし", "あり"]),
                s("microphone", "マイク音", &["なし", "あり"]),
                s(
                    "stop_key",
                    "録画終了・保存キー",
                    &[
                        "なし（ボタンのみ）",
                        "Ctrl+Shift+F9",
                        "Ctrl+Shift+F10",
                        "Ctrl+Shift+F11",
                        "Ctrl+Alt+F9",
                        "Ctrl+Alt+F10",
                    ],
                ),
            ],
            Self::VideoEdit => vec![
                s("edit_mode", "操作", &["クリップ（残す）", "カット（削除）"]),
                f("ranges", "時間範囲（開始-終了）", "0-10"),
            ],
            Self::ResizeImages => vec![
                f("width", "最大幅／枠幅px", "1280"),
                f("height", "最大高／枠高px", "720"),
                s(
                    "resize_mode",
                    "サイズの合わせ方",
                    &["縦横比維持", "背景補完", "引き伸ばし"],
                ),
                f("background", "背景色 #RRGGBB", "#FFFFFF"),
                s("transparent", "透明な背景", &["なし", "あり"]),
                s("image_format", "出力形式", &["PNG", "JPEG", "WebP", "BMP"]),
                s("enlarge", "拡大を許可", &["なし", "あり"]),
            ],
            Self::QrCode => vec![
                s("format", "保存形式", &["SVG", "PNG"]),
                s("ecc", "誤り訂正", &["M", "L", "Q", "H"]),
                f("scale", "1マスの画素数", "8"),
            ],
            Self::Url => vec![
                s("operation", "操作", &["エンコード", "デコード"]),
                s("url_mode", "方式", &["URL部品", "フォーム", "URI全体"]),
            ],
            Self::Convert => vec![
                s("from", "入力形式", &["YAML", "XML", "CSV", "INI"]),
                s("to", "出力形式", &["XML", "YAML", "CSV", "INI"]),
                s("structure", "構造の扱い", &["構造保持", "標準形式"]),
            ],
            Self::Html | Self::Css | Self::JavaScript => vec![
                f("indent", "字下げ（1〜8）", "2"),
                f("width", "折り返し幅", "100"),
            ],
            Self::Regex => vec![
                f("pattern", "正規表現", "(?P<word>日本語|[a-z]+)"),
                f("flags", "フラグ i/m/s/x/U", "i"),
            ],
            Self::Base64Encode | Self::Base64Decode => {
                vec![s("alphabet", "方式", &["標準", "URL-safe"])]
            }
            Self::Hash => vec![s(
                "algorithm",
                "ハッシュ",
                &[
                    "すべて",
                    "MD5",
                    "SHA-1",
                    "SHA-224",
                    "SHA-256",
                    "SHA-384",
                    "SHA-512",
                    "BLAKE3",
                    "CRC32",
                ],
            )],
            Self::Json => vec![
                s("layout", "表示", &["整形", "圧縮"]),
                f("indent", "字下げ（1〜8）", "2"),
            ],
            Self::Sql => vec![
                s(
                    "dialect",
                    "SQL方言",
                    &["Generic", "PostgreSQL", "SQL Server"],
                ),
                s("case", "キーワード", &["変更しない", "大文字", "小文字"]),
            ],
            Self::Password => vec![
                f("length", "文字数（4〜256）", "20"),
                f("count", "候補数（1〜1000）", "10"),
                f("lower", "小文字の最低数", "1"),
                f("upper", "大文字の最低数", "1"),
                f("digits", "数字の最低数", "1"),
                f("symbols_min", "記号の最低数", "1"),
                f("symbols", "使用できる記号", "!@#$%&*+-_=?:"),
                f("exclude", "除外する文字", "Il1O0o"),
            ],
            Self::DummyCsv => vec![
                s(
                    "preset",
                    "定義例の分野",
                    &[
                        "個人", "企業", "EC注文", "医療", "金融", "物流", "ITログ", "IoT",
                    ],
                ),
                f("rows", "行数（1〜100000）", "100"),
                f("seed", "seed（空欄は乱数）", "42"),
                s("bom", "UTF-8 BOM", &["あり", "なし"]),
                s(
                    "csv_safe",
                    "文字列の数式対策",
                    &["表計算ソフト向け", "原文のまま"],
                ),
            ],
            Self::Compound => vec![
                f("principal", "元金", "1000000"),
                f("rate", "年率（%）", "3"),
                f("years", "年数（1〜100）", "10"),
                s(
                    "periods",
                    "年の複利回数",
                    &["12", "1", "2", "4", "52", "365"],
                ),
                f("contribution", "毎期の積立額", "10000"),
                s("timing", "積立時点", &["期末", "期首"]),
                s("decimals", "金額の小数桁", &["0", "2"]),
            ],
            Self::Loan => vec![
                f("principal", "借入元金", "1000000"),
                f("rate", "年率（%）", "3"),
                f("months", "返済回数（月）", "120"),
                s("method", "返済方式", &["元利均等", "元金均等"]),
                s("decimals", "金額の小数桁", &["0", "2"]),
            ],
            Self::AgeGrade => vec![
                f("birth", "生年月日", "2015-04-02"),
                Field::new("asof", "基準日", chrono::Local::now().format("%Y-%m-%d")),
            ],
            Self::TimeZones => vec![
                f("instant", "日時（ISO8601）", "now"),
                f("origin", "時差の基準地域", "Asia/Tokyo"),
            ],
            _ => vec![],
        }
    }

    pub fn defaults(self) -> Params {
        self.fields()
            .into_iter()
            .map(|f| (f.key.into(), f.default))
            .collect()
    }

    pub fn example(self, params: &Params) -> String {
        match self {
            Self::QrCode => "https://example.com/日本語?q=yytoolbox".into(),
            Self::Url => if value(params, "operation", "エンコード") == "デコード" { "%E6%97%A5%E6%9C%AC%E8%AA%9E%20%26%20test%2B1" } else { "日本語 & test+1" }.into(),
            Self::Convert => match value(params, "from", "YAML") {
                "XML" => "<settings><server host=\"localhost\"><port>8080</port><enabled>true</enabled></server></settings>".into(),
                "CSV" => "name,value\n日本語,001\ntest,002\n".into(),
                "INI" => "name=テスト\n\n[server]\nhost=localhost\nport=8080\n".into(),
                _ => "name: テスト\nserver:\n  host: localhost\n  port: 8080\n  enabled: true\nitems: [one, two]\n".into(),
            },
            Self::Html => "<!doctype html><html><head><title>テスト</title></head><body><main><h1>Hello</h1><p>日本語 <strong>テスト</strong></p></main></body></html>".into(),
            Self::Css => ".card{color:red;padding:8px 16px}@media(min-width:600px){.card{display:grid;gap:12px}}".into(),
            Self::JavaScript => "// テスト\nconst greet=(name)=>{const message=`Hello, ${name}!`;return message;};console.log(greet('yytoolbox'));".into(),
            Self::Regex => "Hello 日本語\nabc 123 XYZ".into(),
            Self::Base64Encode | Self::Hash => "こんにちは yytoolbox".into(),
            Self::Base64Decode => "44GT44KT44Gr44Gh44Gv".into(),
            Self::Jwt => "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJzdWIiOiJ0ZXN0LXVzZXIiLCJleHAiOjE3NjcyMjU2MDB9.".into(),
            Self::Json => r#"{"name":"テスト","number":123456789012345678901234567890,"items":[true,null,{"value":"a,b"}]}"#.into(),
            Self::Sql => "select customer_id, sum(amount) as total from test_orders where status = 'pending' group by customer_id order by total desc;".into(),
            Self::DummyCsv => dummy::preset(value(params, "preset", "個人")),
            Self::Calendar => "令和元年5月1日".into(),
            Self::TimeZones => "Asia/Tokyo\nUTC\nAsia/Shanghai\nAsia/Kolkata\nEurope/London\nEurope/Paris\nAmerica/New_York\nAmerica/Los_Angeles\nAustralia/Sydney\nPacific/Auckland".into(),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Output {
    pub text: String,
    pub bytes: Vec<u8>,
    pub filename: &'static str,
    pub notice: String,
    pub qr: Option<QrImage>,
}

#[derive(Debug, Clone)]
pub struct QrImage {
    pub width: usize,
    pub dark: Vec<bool>,
}

impl Output {
    fn text(text: String, filename: &'static str, notice: impl Into<String>) -> Self {
        Self {
            bytes: text.as_bytes().to_vec(),
            text,
            filename,
            notice: notice.into(),
            qr: None,
        }
    }
    fn csv(rows: Vec<Vec<String>>, notice: impl Into<String>) -> Result<Self> {
        let mut writer = csv::WriterBuilder::new()
            .terminator(csv::Terminator::CRLF)
            .from_writer(Vec::new());
        for row in rows {
            writer.write_record(row).map_err(|e| e.to_string())?;
        }
        let bytes = writer.into_inner().map_err(|e| e.to_string())?;
        let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        Ok(Self::text(text, "result.csv", notice))
    }
}

pub fn value<'a>(params: &'a Params, key: &str, fallback: &'a str) -> &'a str {
    params.get(key).map(String::as_str).unwrap_or(fallback)
}
pub fn integer(params: &Params, key: &str, min: u32, max: u32) -> Result<u32> {
    let n = value(params, key, "")
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("{key}: 整数を指定してください"))?;
    if n < min || n > max {
        return Err(format!("{key}: {min}〜{max}で指定してください"));
    }
    Ok(n)
}
pub(crate) fn number(params: &Params, key: &str, min: f64, max: f64) -> Result<f64> {
    let n = value(params, key, "")
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("{key}: 数値を指定してください"))?;
    if !n.is_finite() || n < min || n > max {
        return Err(format!("{key}: {min}〜{max}の有限な数を指定してください"));
    }
    Ok(n)
}
pub(crate) fn checkpoint(cancelled: &impl Fn() -> bool) -> Result<()> {
    if cancelled() {
        Err("中止しました".into())
    } else {
        Ok(())
    }
}

pub fn execute(
    tool: Tool,
    input: &str,
    params: &Params,
    cancelled: &impl Fn() -> bool,
) -> Result<Output> {
    if input.len() > MAX_INPUT {
        return Err("入力は1 MiBまでです".into());
    }
    if params.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() > MAX_INPUT {
        return Err("条件の合計は1 MiBまでです".into());
    }
    checkpoint(cancelled)?;
    let mut defaults = tool.defaults();
    defaults.extend(params.clone());
    let params = &defaults;
    let output = match tool {
        Tool::Recording | Tool::ResizeImages | Tool::VideoEdit => {
            Err("Windows画面の「実行」で対象・保存先を選んでください".into())
        }
        Tool::QrCode => webtools::qr(input, params),
        Tool::Url => webtools::url(input, params),
        Tool::Convert => formats::convert(input, params, cancelled),
        Tool::Html | Tool::Css | Tool::JavaScript => webtools::format(input, params, tool),
        Tool::Regex => text::regex(input, params, cancelled),
        Tool::Base64Encode | Tool::Base64Decode => {
            text::base64(input, params, tool == Tool::Base64Decode)
        }
        Tool::Hash => text::hash(input, params),
        Tool::Jwt => text::jwt(input),
        Tool::Json => text::json(input, params),
        Tool::Sql => text::sql(input, params),
        Tool::Password => random::passwords(params, cancelled),
        Tool::DummyCsv => dummy::generate(input, params, cancelled),
        Tool::Compound => finance::compound(params),
        Tool::Loan => finance::loan(params, cancelled),
        Tool::Calendar => dates::calendar(input),
        Tool::AgeGrade => dates::age_grade(params),
        Tool::TimeZones => dates::time_zones(input, params),
    }?;
    checkpoint(cancelled)?;
    if output.bytes.len() > MAX_OUTPUT || output.text.len() > MAX_OUTPUT {
        return Err("出力は16 MiBまでです。条件を絞ってください".into());
    }
    Ok(output)
}

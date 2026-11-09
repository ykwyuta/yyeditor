//! Command line interface shared by `yytoolbox-cli` (all platforms) and
//! `yytoolbox` on platforms without the Windows GUI.
use crate::steps::{self, WalkOptions};
use crate::{Params, TOOLS, Tool, execute};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;

/// Exit status for invalid command lines.
pub const USAGE_ERROR: i32 = 2;

const NAME: &str = "yytoolbox-cli";

fn usage() -> String {
    format!(
        "\
yytoolbox コマンドライン {version}

使い方:
  {NAME} list                         ツールの一覧
  {NAME} <ツール> --help              ツールの条件と既定値
  {NAME} <ツール> [条件...] [入力]    ツールを実行
  {NAME} steps [オプション] <パス...> ステップカウンタ（ファイル・フォルダー）
  {NAME} steps --languages            対応言語と拡張子

条件:
  --<キー> <値> または --<キー>=<値>  ツールの条件（{NAME} <ツール> --help で確認）

入力（入力を使うツールのみ。省略時は標準入力）:
  --text <文字列>                     文字列を入力にする
  --input <ファイル>                  ファイルを入力にする（- は標準入力。文字コードは自動判別）
  --example                           画面の「入力例」と同じ入力を使う

出力:
  --output <ファイル>                 結果をファイルへ保存（既定は標準出力）
  -q, --quiet                         補足メッセージを標準エラーに出さない

終了コード: 0 成功、1 実行エラー、2 指定の誤り
",
        version = env!("CARGO_PKG_VERSION"),
    )
}

fn steps_usage() -> String {
    format!(
        "\
ステップカウンタ (steps)

使い方: {NAME} steps [オプション] [パス...]

ファイル・フォルダーのソースコードを言語ごとに集計します。フォルダーは再帰的にたどり、
拡張子・ファイル名から言語を判定します。文字コード（UTF-8/UTF-16/Shift_JIS/EUC-JP等）は自動判別します。
パスを省略するか - を指定すると、標準入力を --lang の言語として数えます。

オプション:
  --lang <言語>                       言語を固定する（例: java, c++, python, cobol）
  --format <text|csv|json>            出力形式（既定 text）
  --files                             text 形式でファイルごとの明細も出す
  --exclude <名前>                    除外するファイル・フォルダー名または *.拡張子（複数指定可）
  --no-default-excludes               既定の除外（{excludes}）を使わない
  --hidden                            . で始まるファイル・フォルダーも数える
  --output <ファイル>                 結果をファイルへ保存
  --languages                         対応言語と拡張子を表示

数え方: 空白文字だけの行は空行、コメント以外の字句を含む行はステップ、それ以外はコメント行です。
文字列内のコメント記号は数えません。COBOLは固定形式の7桁目（*・/）と *> を扱います。
",
        excludes = steps::DEFAULT_EXCLUDES.join(", ")
    )
}

struct Io<'a> {
    stdin: &'a mut dyn Read,
    stdout: &'a mut dyn Write,
    stderr: &'a mut dyn Write,
}

enum Failure {
    Usage(String),
    Run(String),
}

impl From<String> for Failure {
    fn from(s: String) -> Self {
        Failure::Run(s)
    }
}

type Outcome = std::result::Result<(), Failure>;

/// Runs the command line `args` (without the program name). Returns the exit status.
pub fn run(
    args: Vec<OsString>,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut io = Io {
        stdin,
        stdout,
        stderr,
    };
    let result = dispatch(args, &mut io);
    let _ = io.stdout.flush();
    match result {
        Ok(()) => 0,
        Err(Failure::Usage(msg)) => {
            let _ = writeln!(io.stderr, "{NAME}: {msg}");
            let _ = writeln!(io.stderr, "{NAME} --help で使い方を表示します。");
            USAGE_ERROR
        }
        Err(Failure::Run(msg)) => {
            let _ = writeln!(io.stderr, "{NAME}: {msg}");
            1
        }
    }
}

fn write(io: &mut Io<'_>, s: &str) -> Outcome {
    io.stdout
        .write_all(s.as_bytes())
        .map_err(|e| Failure::Run(format!("標準出力: {e}")))
}

fn dispatch(args: Vec<OsString>, io: &mut Io<'_>) -> Outcome {
    let Some(first) = args.first() else {
        return write(io, &usage());
    };
    let command = first
        .to_str()
        .ok_or_else(|| Failure::Usage("コマンド名が不正です".into()))?
        .to_string();
    let rest = args[1..].to_vec();
    match command.as_str() {
        "-h" | "--help" | "help" => {
            if let Some(tool) = rest.first().and_then(|a| a.to_str()) {
                return tool_help(io, tool);
            }
            write(io, &usage())
        }
        "-V" | "--version" | "version" => {
            write(io, &format!("yytoolbox {}\n", env!("CARGO_PKG_VERSION")))
        }
        "list" => list(io),
        "steps" => steps_command(rest, io),
        other => match Tool::from_id(other) {
            Some(tool) => tool_command(tool, rest, io),
            None => Err(Failure::Usage(format!(
                "不明なコマンドです: {other}（{NAME} list でツールの一覧）"
            ))),
        },
    }
}

fn list(io: &mut Io<'_>) -> Outcome {
    let width = TOOLS.iter().map(|t| t.id().len()).max().unwrap_or(0);
    let mut out = String::new();
    for tool in TOOLS {
        let note = if gui_only(tool) {
            "（画面専用）"
        } else {
            ""
        };
        out.push_str(&format!("{:width$}  {}{note}\n", tool.id(), tool.label()));
    }
    write(io, &out)
}

fn gui_only(tool: Tool) -> bool {
    matches!(tool, Tool::Recording | Tool::ResizeImages | Tool::VideoEdit)
}

fn tool_help(io: &mut Io<'_>, id: &str) -> Outcome {
    if id == "steps" {
        return write(io, &steps_usage());
    }
    let tool =
        Tool::from_id(id).ok_or_else(|| Failure::Usage(format!("不明なツールです: {id}")))?;
    let mut out = format!("{} ({})\n{}\n", tool.label(), tool.id(), tool.description());
    if gui_only(tool) {
        out.push_str("\nこのツールはWindowsの画面から実行します。\n");
        return write(io, &out);
    }
    out.push_str(&format!(
        "\n使い方: {NAME} {} [条件...]{}\n",
        tool.id(),
        if tool.uses_input() {
            " [--text <文字列> | --input <ファイル> | --example]"
        } else {
            ""
        }
    ));
    let fields = tool.fields();
    if !fields.is_empty() {
        out.push_str("\n条件:\n");
        for f in fields {
            out.push_str(&format!(
                "  --{}  {}（既定: {}）\n",
                f.key, f.label, f.default
            ));
            if !f.choices.is_empty() {
                out.push_str(&format!("      選択肢: {}\n", f.choices.join(" / ")));
            }
        }
    }
    write(io, &out)
}

/// Named options (`--key value`) and positional arguments.
type Parsed = (Vec<(String, OsString)>, Vec<OsString>);

/// Splits `--key value` / `--key=value` options. Flags without values are listed in `flags`.
fn parse_options(args: Vec<OsString>, flags: &[&str]) -> std::result::Result<Parsed, Failure> {
    let mut options = Vec::new();
    let mut positional = Vec::new();
    let mut it = args.into_iter();
    let mut only_positional = false;
    while let Some(arg) = it.next() {
        let text = arg.to_str();
        match text {
            Some("--") if !only_positional => only_positional = true,
            Some(t) if !only_positional && t.starts_with("--") => {
                let body = &t[2..];
                if let Some((k, v)) = body.split_once('=') {
                    options.push((k.to_string(), OsString::from(v)));
                } else if flags.contains(&body) {
                    options.push((body.to_string(), OsString::new()));
                } else {
                    let v = it
                        .next()
                        .ok_or_else(|| Failure::Usage(format!("--{body} の値がありません")))?;
                    options.push((body.to_string(), v));
                }
            }
            Some("-q") if !only_positional => options.push(("quiet".into(), OsString::new())),
            Some("-h") if !only_positional => options.push(("help".into(), OsString::new())),
            Some(t) if !only_positional && t.starts_with('-') && t != "-" => {
                return Err(Failure::Usage(format!("不明なオプションです: {t}")));
            }
            _ => positional.push(arg),
        }
    }
    Ok((options, positional))
}

fn utf8(key: &str, v: OsString) -> std::result::Result<String, Failure> {
    v.into_string()
        .map_err(|_| Failure::Usage(format!("--{key} の値をUTF-8として読めません")))
}

fn read_all(io: &mut Io<'_>) -> std::result::Result<Vec<u8>, Failure> {
    let mut bytes = Vec::new();
    io.stdin
        .read_to_end(&mut bytes)
        .map_err(|e| Failure::Run(format!("標準入力: {e}")))?;
    Ok(bytes)
}

fn read_input(io: &mut Io<'_>, path: &OsString) -> std::result::Result<String, Failure> {
    let bytes = if path == "-" {
        read_all(io)?
    } else {
        std::fs::read(path)
            .map_err(|e| Failure::Run(format!("{}: {e}", PathBuf::from(path).display())))?
    };
    Ok(steps::decode(&bytes).0)
}

fn save(path: &OsString, bytes: &[u8]) -> Outcome {
    std::fs::write(path, bytes)
        .map_err(|e| Failure::Run(format!("{}: {e}", PathBuf::from(path).display())))
}

fn tool_command(tool: Tool, args: Vec<OsString>, io: &mut Io<'_>) -> Outcome {
    let (options, positional) = parse_options(args, &["help", "example", "quiet"])?;
    if options.iter().any(|(k, _)| k == "help") {
        return tool_help(io, tool.id());
    }
    if gui_only(tool) {
        return Err(Failure::Run(format!(
            "{} はWindowsの画面（yytoolbox.exe）から実行してください",
            tool.label()
        )));
    }
    if let Some(p) = positional.first() {
        return Err(Failure::Usage(format!(
            "余分な引数です: {}（ファイルは --input で指定します）",
            p.to_string_lossy()
        )));
    }
    let fields = tool.fields();
    let mut params = Params::new();
    let mut text = None;
    let mut input_path = None;
    let mut output = None;
    let mut example = false;
    let mut quiet = false;
    for (key, v) in options {
        match key.as_str() {
            "text" => text = Some(utf8(&key, v)?),
            "input" => input_path = Some(v),
            "output" => output = Some(v),
            "example" => example = true,
            "quiet" => quiet = true,
            _ => {
                let Some(field) = fields.iter().find(|f| f.key == key) else {
                    let keys: Vec<_> = fields.iter().map(|f| format!("--{}", f.key)).collect();
                    return Err(Failure::Usage(format!(
                        "{} に条件 --{key} はありません（指定できる条件: {}）",
                        tool.id(),
                        if keys.is_empty() {
                            "なし".into()
                        } else {
                            keys.join(" ")
                        }
                    )));
                };
                let v = utf8(&key, v)?;
                if !field.choices.is_empty() && !field.choices.contains(&v.as_str()) {
                    return Err(Failure::Usage(format!(
                        "--{key} は {} のいずれかを指定してください",
                        field.choices.join(" / ")
                    )));
                }
                params.insert(key, v);
            }
        }
    }
    let sources = [text.is_some(), input_path.is_some(), example]
        .iter()
        .filter(|b| **b)
        .count();
    if sources > 1 {
        return Err(Failure::Usage(
            "--text・--input・--example はいずれか1つを指定してください".into(),
        ));
    }
    let input = if !tool.uses_input() {
        if sources > 0 {
            return Err(Failure::Usage(format!("{} は入力を使いません", tool.id())));
        }
        String::new()
    } else if let Some(t) = text {
        t
    } else if let Some(p) = input_path {
        read_input(io, &p)?
    } else if example {
        let mut merged = tool.defaults();
        merged.extend(params.clone());
        tool.example(&merged)
    } else {
        read_input(io, &OsString::from("-"))?
    };
    let result = execute(tool, &input, &params, &|| false)?;
    if !quiet && !result.notice.is_empty() {
        let _ = writeln!(io.stderr, "{}", result.notice);
    }
    match output {
        Some(path) => save(&path, &result.bytes),
        None => {
            let mut text = result.text;
            if !text.ends_with('\n') {
                text.push('\n');
            }
            write(io, &text)
        }
    }
}

fn languages_table() -> String {
    let mut out = String::new();
    for l in steps::LANGUAGES {
        let mut names: Vec<String> = l.extensions.iter().map(|e| format!(".{e}")).collect();
        names.extend(l.file_names.iter().map(|n| n.to_string()));
        out.push_str(&format!("{:14} {}\n", l.name, names.join(" ")));
    }
    out
}

fn steps_command(args: Vec<OsString>, io: &mut Io<'_>) -> Outcome {
    let (options, positional) = parse_options(
        args,
        &[
            "help",
            "files",
            "no-default-excludes",
            "hidden",
            "languages",
            "quiet",
        ],
    )?;
    let mut opts = WalkOptions::default();
    let mut format = "text".to_string();
    let mut per_file = false;
    let mut output = None;
    let mut quiet = false;
    let mut lang: Option<String> = None;
    for (key, v) in options {
        match key.as_str() {
            "help" => return write(io, &steps_usage()),
            "languages" => return write(io, &languages_table()),
            "lang" | "language" => lang = Some(utf8(&key, v)?),
            "format" => format = utf8(&key, v)?.to_ascii_lowercase(),
            "files" => per_file = true,
            "exclude" => opts.excludes.push(utf8(&key, v)?),
            "no-default-excludes" => opts.no_default_excludes = true,
            "hidden" => opts.hidden = true,
            "output" => output = Some(v),
            "quiet" => quiet = true,
            _ => return Err(Failure::Usage(format!("steps に --{key} はありません"))),
        }
    }
    if !matches!(format.as_str(), "text" | "csv" | "json") {
        return Err(Failure::Usage(
            "--format は text・csv・json のいずれかです".into(),
        ));
    }
    let forced = match &lang {
        Some(name) => Some(steps::find_language(name).ok_or_else(|| {
            Failure::Usage(format!(
                "未対応の言語です: {name}（{NAME} steps --languages で一覧）"
            ))
        })?),
        None => None,
    };
    opts.language = forced.map(|l| l.name);
    let stdin_only = positional.is_empty() || positional.iter().all(|p| p == "-");
    let (files, warnings) = if stdin_only {
        let lang = forced.ok_or_else(|| {
            Failure::Usage("標準入力を数えるときは --lang で言語を指定してください".into())
        })?;
        let bytes = read_all(io)?;
        let (text, encoding) = steps::decode(&bytes);
        let file = steps::FileCount {
            path: PathBuf::from("-"),
            language: lang.name,
            encoding,
            counts: steps::count(&text, lang),
        };
        (vec![file], Vec::new())
    } else {
        if positional.iter().any(|p| p == "-") {
            return Err(Failure::Usage(
                "標準入力（-）とパスは同時に指定できません".into(),
            ));
        }
        let paths: Vec<PathBuf> = positional.into_iter().map(PathBuf::from).collect();
        steps::count_paths(&paths, &opts, &|| false)?
    };
    if !quiet {
        for w in &warnings {
            let _ = writeln!(io.stderr, "{w}");
        }
    }
    let rendered = match format.as_str() {
        "csv" => steps::render_csv(&files)?,
        "json" => steps::render_json(&files),
        _ => steps::render_text(&files, per_file),
    };
    match output {
        Some(path) => save(&path, rendered.as_bytes()),
        None => write(io, &rendered),
    }
}

//! Step counter: classifies source lines into code (steps), comment and blank lines.
//!
//! The scanner understands each language's line comments, block comments
//! (optionally nested), string literals and a few line-oriented forms
//! (COBOL fixed format, Ruby `=begin`, Perl POD, `REM`). A line containing any
//! code outside comments is a step. A line with only whitespace is blank, even
//! inside a block comment. Everything else is a comment line.
use crate::{Output, Params, Result, checkpoint, value};
use std::path::{Path, PathBuf};

/// Kind of string literal delimiter.
#[derive(Clone, Copy)]
struct Quote {
    open: &'static str,
    close: &'static str,
    /// Backslash escapes the next character.
    escape: bool,
    /// The literal may continue on the next line.
    multiline: bool,
}

const fn q(open: &'static str, close: &'static str, escape: bool, multiline: bool) -> Quote {
    Quote {
        open,
        close,
        escape,
        multiline,
    }
}

const DQ: Quote = q("\"", "\"", true, false);
const SQ: Quote = q("'", "'", true, false);
const BT: Quote = q("`", "`", true, true);
const DQ_RAW: Quote = q("\"", "\"", false, false);
const SQ_RAW: Quote = q("'", "'", false, false);

/// Comment and literal syntax of one language.
pub struct Language {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    /// Exact file names (case-insensitive), e.g. `Makefile`.
    pub file_names: &'static [&'static str],
    line: &'static [&'static str],
    block: &'static [(&'static str, &'static str)],
    nested: bool,
    quotes: &'static [Quote],
    /// Words that start a comment when they are the first token (case-insensitive).
    line_start_words: &'static [&'static str],
    /// Comment regions starting/ending with lines that begin at column 1.
    line_blocks: &'static [(&'static str, &'static str)],
    /// Literals that are comments when they are the first token on a line (Python docstrings).
    doc_strings: &'static [&'static str],
    /// Rust-style character literals: `'a'` and `'\n'` are literals, `'a` is a lifetime.
    char_literals: bool,
    case_insensitive: bool,
    cobol: bool,
}

const fn lang(name: &'static str, extensions: &'static [&'static str]) -> Language {
    Language {
        name,
        extensions,
        file_names: &[],
        line: &[],
        block: &[],
        nested: false,
        quotes: &[],
        line_start_words: &[],
        line_blocks: &[],
        doc_strings: &[],
        char_literals: false,
        case_insensitive: false,
        cobol: false,
    }
}

const C_LINE: &[&str] = &["//"];
const C_BLOCK: &[(&str, &str)] = &[("/*", "*/")];
const C_QUOTES: &[Quote] = &[DQ, SQ];

const fn c_like(name: &'static str, extensions: &'static [&'static str]) -> Language {
    Language {
        line: C_LINE,
        block: C_BLOCK,
        quotes: C_QUOTES,
        ..lang(name, extensions)
    }
}

const fn hash_like(name: &'static str, extensions: &'static [&'static str]) -> Language {
    Language {
        line: &["#"],
        quotes: &[DQ, SQ],
        ..lang(name, extensions)
    }
}

pub static LANGUAGES: &[Language] = &[
    c_like("C", &["c", "h"]),
    c_like(
        "C++",
        &["cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx", "h++", "ino"],
    ),
    Language {
        quotes: &[q("@\"", "\"", false, true), DQ, SQ],
        ..c_like("C#", &["cs"])
    },
    Language {
        quotes: &[q("\"\"\"", "\"\"\"", true, true), DQ, SQ],
        ..c_like("Java", &["java"])
    },
    Language {
        quotes: &[q("\"\"\"", "\"\"\"", true, true), DQ, SQ],
        nested: true,
        ..c_like("Kotlin", &["kt", "kts"])
    },
    Language {
        quotes: &[q("\"\"\"", "\"\"\"", false, true), DQ, SQ],
        nested: true,
        ..c_like("Scala", &["scala", "sc"])
    },
    Language {
        quotes: &[DQ, SQ, BT],
        ..c_like("JavaScript", &["js", "mjs", "cjs", "jsx"])
    },
    Language {
        quotes: &[DQ, SQ, BT],
        ..c_like("TypeScript", &["ts", "mts", "cts", "tsx"])
    },
    Language {
        quotes: &[DQ, SQ, q("`", "`", false, true)],
        ..c_like("Go", &["go"])
    },
    Language {
        quotes: &[
            q("r#\"", "\"#", false, true),
            q("r\"", "\"", false, true),
            q("\"", "\"", true, true),
        ],
        nested: true,
        char_literals: true,
        ..c_like("Rust", &["rs"])
    },
    Language {
        quotes: &[q("\"\"\"", "\"\"\"", true, true), DQ],
        nested: true,
        ..c_like("Swift", &["swift"])
    },
    c_like("Objective-C", &["m", "mm"]),
    Language {
        quotes: &[
            q("'''", "'''", true, true),
            q("\"\"\"", "\"\"\"", true, true),
            DQ,
            SQ,
        ],
        ..c_like("Dart", &["dart"])
    },
    Language {
        line: &["//", "#"],
        quotes: &[q("\"", "\"", true, true), q("'", "'", true, true), BT],
        ..c_like("PHP", &["php", "phtml"])
    },
    Language {
        quotes: &[DQ, SQ],
        ..c_like("CSS", &["css"])
    },
    Language {
        quotes: &[DQ, SQ],
        ..c_like("SCSS/Less", &["scss", "less"])
    },
    Language {
        line: &["#"],
        quotes: &[
            q("\"\"\"", "\"\"\"", true, true),
            q("'''", "'''", true, true),
            DQ,
            SQ,
        ],
        doc_strings: &["\"\"\"", "'''"],
        ..lang("Python", &["py", "pyw", "pyi"])
    },
    Language {
        line_blocks: &[("=begin", "=end")],
        file_names: &["Rakefile", "Gemfile"],
        ..hash_like("Ruby", &["rb", "rake", "gemspec"])
    },
    Language {
        line_blocks: &[("=", "=cut")],
        ..hash_like("Perl", &["pl", "pm", "t"])
    },
    Language {
        quotes: &[DQ_RAW, SQ_RAW],
        file_names: &[".bashrc", ".profile", ".zshrc"],
        ..hash_like("Shell", &["sh", "bash", "zsh", "ksh"])
    },
    Language {
        block: &[("<#", "#>")],
        quotes: &[q("\"", "\"", false, false), SQ_RAW],
        case_insensitive: true,
        ..hash_like("PowerShell", &["ps1", "psm1", "psd1"])
    },
    Language {
        line: &["::"],
        line_start_words: &["rem", "@rem"],
        quotes: &[DQ_RAW],
        case_insensitive: true,
        ..lang("Batch", &["bat", "cmd"])
    },
    hash_like("R", &["r"]),
    Language {
        line: &["--"],
        block: &[("--[[", "]]"), ("--[==[", "]==]"), ("--[=[", "]=]")],
        quotes: &[q("[[", "]]", false, true), DQ, SQ],
        ..lang("Lua", &["lua"])
    },
    Language {
        line: &["--"],
        block: C_BLOCK,
        quotes: &[SQ_RAW, DQ_RAW],
        case_insensitive: true,
        ..lang("SQL", &["sql", "ddl", "dml", "pls", "plsql", "pkb", "pks"])
    },
    Language {
        line: &["'"],
        line_start_words: &["rem"],
        quotes: &[DQ_RAW],
        case_insensitive: true,
        ..lang("Visual Basic", &["vb", "bas", "cls", "frm", "vbs", "vba"])
    },
    Language {
        line: &["//"],
        block: &[("{", "}"), ("(*", "*)")],
        quotes: &[SQ_RAW],
        case_insensitive: true,
        ..lang("Pascal/Delphi", &["pas", "dpr", "dpk", "pp", "lpr"])
    },
    Language {
        line: &["--"],
        block: &[("{-", "-}")],
        nested: true,
        quotes: &[DQ],
        ..lang("Haskell", &["hs", "lhs"])
    },
    Language {
        block: &[("<!--", "-->")],
        ..lang("HTML", &["html", "htm", "xhtml", "vue", "svelte"])
    },
    Language {
        block: &[("<!--", "-->")],
        ..lang(
            "XML",
            &[
                "xml", "xsd", "xsl", "xslt", "svg", "xaml", "csproj", "vbproj", "props", "targets",
                "config", "resx", "pom", "jsp",
            ],
        )
    },
    Language {
        line: &["#"],
        quotes: &[DQ, SQ_RAW],
        ..lang("YAML", &["yml", "yaml"])
    },
    Language {
        line: &["#"],
        file_names: &["Makefile", "GNUmakefile", "makefile"],
        ..lang("Makefile", &["mk", "mak"])
    },
    Language {
        line: &["#"],
        file_names: &["Dockerfile"],
        quotes: &[DQ],
        ..lang("Dockerfile", &["dockerfile"])
    },
    Language {
        line: &["#"],
        quotes: &[DQ, SQ_RAW],
        file_names: &["CMakeLists.txt"],
        block: &[("#[[", "]]")],
        ..lang("CMake", &["cmake"])
    },
    Language {
        line: &["!"],
        quotes: &[DQ_RAW, SQ_RAW],
        case_insensitive: true,
        ..lang("Fortran", &["f90", "f95", "f03", "f08"])
    },
    Language {
        line: &["*>"],
        quotes: &[DQ_RAW, SQ_RAW],
        case_insensitive: true,
        cobol: true,
        ..lang("COBOL", &["cbl", "cob", "cpy", "cobol", "pco"])
    },
];

/// Language for a display name (case-insensitive) or a file extension.
pub fn find_language(name: &str) -> Option<&'static Language> {
    let name = name.trim().trim_start_matches('.');
    LANGUAGES
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(name))
        .or_else(|| {
            let aliases: &[(&str, &str)] = &[
                ("cpp", "C++"),
                ("cxx", "C++"),
                ("csharp", "C#"),
                ("cs", "C#"),
                ("js", "JavaScript"),
                ("ts", "TypeScript"),
                ("objc", "Objective-C"),
                ("vb", "Visual Basic"),
                ("vba", "Visual Basic"),
                ("vbnet", "Visual Basic"),
                ("pascal", "Pascal/Delphi"),
                ("delphi", "Pascal/Delphi"),
                ("bash", "Shell"),
                ("sh", "Shell"),
                ("pwsh", "PowerShell"),
                ("ps1", "PowerShell"),
                ("scss", "SCSS/Less"),
                ("less", "SCSS/Less"),
                ("bat", "Batch"),
                ("plsql", "SQL"),
                ("golang", "Go"),
            ];
            aliases
                .iter()
                .find(|(a, _)| a.eq_ignore_ascii_case(name))
                .and_then(|(_, n)| LANGUAGES.iter().find(|l| l.name == *n))
        })
        .or_else(|| {
            LANGUAGES
                .iter()
                .find(|l| l.extensions.iter().any(|e| e.eq_ignore_ascii_case(name)))
        })
}

/// Language for a path, from its file name or extension.
pub fn language_for_path(path: &Path) -> Option<&'static Language> {
    let file = path.file_name()?.to_str()?;
    if let Some(l) = LANGUAGES
        .iter()
        .find(|l| l.file_names.iter().any(|n| n.eq_ignore_ascii_case(file)))
    {
        return Some(l);
    }
    let ext = path.extension()?.to_str()?;
    LANGUAGES
        .iter()
        .find(|l| l.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)))
}

/// Line counts of a file or a total.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub files: u64,
    pub total: u64,
    pub code: u64,
    pub comment: u64,
    pub blank: u64,
}

impl std::ops::AddAssign for Counts {
    fn add_assign(&mut self, o: Self) {
        self.files += o.files;
        self.total += o.total;
        self.code += o.code;
        self.comment += o.comment;
        self.blank += o.blank;
    }
}

enum State {
    Normal,
    Block { close: &'static str, depth: u32 },
    Quote(Quote),
    LineBlock(&'static str),
}

fn starts(s: &[char], at: usize, pat: &str, ci: bool) -> bool {
    let mut i = at;
    for p in pat.chars() {
        match s.get(i) {
            Some(c) if *c == p || (ci && c.eq_ignore_ascii_case(&p)) => i += 1,
            _ => return false,
        }
    }
    true
}

fn len(pat: &str) -> usize {
    pat.chars().count()
}

/// Counts the lines of `text` written in `lang`.
pub fn count(text: &str, lang: &Language) -> Counts {
    let mut counts = Counts {
        files: 1,
        ..Counts::default()
    };
    let mut state = State::Normal;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.is_empty() {
        return counts;
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    for raw in body.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        counts.total += 1;
        let (code, comment) = if lang.cobol {
            cobol_line(raw, lang, &mut state)
        } else {
            scan_line(raw, lang, &mut state)
        };
        if code {
            counts.code += 1;
        } else if comment {
            counts.comment += 1;
        } else {
            counts.blank += 1;
        }
    }
    counts
}

/// COBOL fixed format: columns 1-6 sequence area, 7 indicator, 8-72 program text.
/// Lines whose first six columns are not digits or spaces are read as free format.
fn cobol_line(raw: &str, lang: &Language, state: &mut State) -> (bool, bool) {
    let chars: Vec<char> = raw.chars().collect();
    let sequence = &chars[..chars.len().min(6)];
    if !sequence.iter().all(|c| c.is_ascii_digit() || *c == ' ') {
        return scan_line(raw, lang, state);
    }
    match chars.get(6) {
        None => scan_line("", lang, state),
        Some('*' | '/') => (false, true),
        Some(_) => {
            let area: String = chars[7..chars.len().min(72)].iter().collect();
            scan_line(&area, lang, state)
        }
    }
}

/// Returns (has code, has comment) for one line and updates the multi-line state.
fn scan_line(raw: &str, lang: &Language, state: &mut State) -> (bool, bool) {
    let s: Vec<char> = raw.chars().collect();
    let ci = lang.case_insensitive;
    let mut code = false;
    let mut comment = false;
    let mut i = 0;
    let first = s.iter().position(|c| !c.is_whitespace());
    let Some(first) = first else {
        if matches!(state, State::Quote(q) if !q.multiline) {
            *state = State::Normal;
        }
        if matches!(state, State::Quote(_)) {
            return (true, false);
        }
        return (false, false);
    };
    if let State::LineBlock(close) = state {
        if starts(&s, 0, close, false) {
            *state = State::Normal;
        }
        return (false, true);
    }
    if matches!(state, State::Normal) {
        for (open, close) in lang.line_blocks {
            let next = s.get(len(open));
            let word_end = if *open == "=" {
                next.is_some_and(|c| c.is_ascii_alphabetic())
            } else {
                next.is_none_or(|c| c.is_whitespace())
            };
            if starts(&s, 0, open, false) && word_end {
                *state = State::LineBlock(close);
                return (false, true);
            }
        }
        for word in lang.line_start_words {
            if starts(&s, first, word, true)
                && s.get(first + len(word))
                    .is_none_or(|c| c.is_whitespace() || *c == '.')
            {
                return (false, true);
            }
        }
        for doc in lang.doc_strings {
            if starts(&s, first, doc, false) {
                *state = State::Block {
                    close: doc,
                    depth: 1,
                };
                i = first + len(doc);
                comment = true;
                break;
            }
        }
    }
    while i < s.len() {
        match state {
            State::LineBlock(_) => unreachable!(),
            State::Block { close, depth } => {
                if starts(&s, i, close, ci) {
                    i += len(close);
                    *depth -= 1;
                    if *depth == 0 {
                        *state = State::Normal;
                    }
                    comment = true;
                    continue;
                }
                if lang.nested
                    && let Some((open, _)) = lang
                        .block
                        .iter()
                        .find(|(o, c)| *c == *close && starts(&s, i, o, ci))
                {
                    *depth += 1;
                    i += len(open);
                    continue;
                }
                if !s[i].is_whitespace() {
                    comment = true;
                }
                i += 1;
            }
            State::Quote(quote) => {
                code = true;
                if quote.escape && s[i] == '\\' {
                    i += 2;
                    continue;
                }
                if starts(&s, i, quote.close, false) {
                    i += len(quote.close);
                    *state = State::Normal;
                    continue;
                }
                i += 1;
            }
            State::Normal => {
                if s[i].is_whitespace() {
                    i += 1;
                    continue;
                }
                if let Some((open, close)) = lang.block.iter().find(|(o, _)| starts(&s, i, o, ci)) {
                    *state = State::Block { close, depth: 1 };
                    i += len(open);
                    comment = true;
                    continue;
                }
                if lang.line.iter().any(|l| starts(&s, i, l, ci)) {
                    comment = true;
                    break;
                }
                if lang.char_literals && s[i] == '\'' {
                    code = true;
                    if s.get(i + 1) == Some(&'\\') {
                        let end = s
                            .get(i + 3..)
                            .and_then(|t| t.iter().position(|c| *c == '\''));
                        i = end.map_or(s.len(), |e| i + 4 + e);
                    } else if s.get(i + 2) == Some(&'\'') {
                        i += 3;
                    } else {
                        i += 1;
                    }
                    continue;
                }
                if let Some(quote) = lang.quotes.iter().find(|qt| starts(&s, i, qt.open, false)) {
                    // Identifier characters before `r"` mean it is not a raw string prefix.
                    let ident_before = i > 0 && (s[i - 1].is_alphanumeric() || s[i - 1] == '_');
                    if !(ident_before && quote.open.starts_with(|c: char| c.is_alphabetic())) {
                        code = true;
                        i += len(quote.open);
                        *state = State::Quote(*quote);
                        continue;
                    }
                }
                code = true;
                i += 1;
            }
        }
    }
    if let State::Quote(quote) = state
        && !quote.multiline
    {
        *state = State::Normal;
    }
    (code, comment)
}

/// One counted file.
#[derive(Clone, Debug)]
pub struct FileCount {
    pub path: PathBuf,
    pub language: &'static str,
    pub encoding: String,
    pub counts: Counts,
}

/// Directories skipped while walking unless named explicitly.
pub const DEFAULT_EXCLUDES: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    "node_modules",
    "target",
    "obj",
    "__pycache__",
    "venv",
];

/// Options for counting files on disk.
#[derive(Clone, Debug, Default)]
pub struct WalkOptions {
    /// Force one language for every file.
    pub language: Option<&'static str>,
    /// Directory or file names to skip (exact, case-insensitive) or `*.ext` patterns.
    pub excludes: Vec<String>,
    /// Do not apply [`DEFAULT_EXCLUDES`].
    pub no_default_excludes: bool,
    /// Follow hidden files and directories (names starting with `.`).
    pub hidden: bool,
}

fn excluded(name: &str, opts: &WalkOptions, dir: bool) -> bool {
    if !opts.hidden && name.starts_with('.') && name.len() > 1 {
        return true;
    }
    if dir
        && !opts.no_default_excludes
        && DEFAULT_EXCLUDES
            .iter()
            .any(|e| e.eq_ignore_ascii_case(name))
    {
        return true;
    }
    opts.excludes.iter().any(|e| {
        if let Some(ext) = e.strip_prefix("*.") {
            !dir && Path::new(name)
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case(ext))
        } else {
            e.eq_ignore_ascii_case(name)
        }
    })
}

/// Decodes a source file. Returns the text and the detected encoding name.
pub fn decode(bytes: &[u8]) -> (String, String) {
    let detected = yy_encoding::detect(bytes, true);
    let body = &bytes[detected.bom_len..];
    let (utf8, _) = yy_encoding::decode_all(detected.encoding, body, false);
    (
        String::from_utf8_lossy(&utf8).into_owned(),
        detected.encoding.name().to_string(),
    )
}

/// Counts files and directories. Files with unknown languages are skipped
/// unless the language is forced or the file was named explicitly.
pub fn count_paths(
    paths: &[PathBuf],
    opts: &WalkOptions,
    cancelled: &impl Fn() -> bool,
) -> Result<(Vec<FileCount>, Vec<String>)> {
    let forced = match opts.language {
        Some(name) => Some(find_language(name).ok_or_else(|| format!("未対応の言語です: {name}"))?),
        None => None,
    };
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    let mut stack: Vec<(PathBuf, bool)> = paths.iter().rev().map(|p| (p.clone(), true)).collect();
    while let Some((path, explicit)) = stack.pop() {
        checkpoint(cancelled)?;
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                if explicit {
                    return Err(format!("{}: {e}", path.display()));
                }
                warnings.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        if meta.is_dir() {
            let mut entries: Vec<(PathBuf, bool)> = match std::fs::read_dir(&path) {
                Ok(rd) => rd
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        let dir = e.file_type().ok()?.is_dir();
                        (!excluded(&name, opts, dir)).then(|| (e.path(), false))
                    })
                    .collect(),
                Err(e) => {
                    warnings.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            entries.sort_by(|a, b| b.0.cmp(&a.0));
            stack.extend(entries);
            continue;
        }
        let lang = match forced.or_else(|| language_for_path(&path)) {
            Some(l) => l,
            None if explicit => {
                warnings.push(format!(
                    "{}: 言語を判定できないため除外しました（--lang で指定できます）",
                    path.display()
                ));
                continue;
            }
            None => continue,
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                warnings.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        if bytes.iter().take(8000).any(|b| *b == 0) && !looks_utf16(&bytes) {
            warnings.push(format!("{}: バイナリのため除外しました", path.display()));
            continue;
        }
        let (text, encoding) = decode(&bytes);
        files.push(FileCount {
            counts: count(&text, lang),
            path,
            language: lang.name,
            encoding,
        });
    }
    Ok((files, warnings))
}

fn looks_utf16(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF])
}

/// Totals per language, sorted by code lines (descending), then name.
pub fn by_language(files: &[FileCount]) -> Vec<(&'static str, Counts)> {
    let mut map: std::collections::BTreeMap<&'static str, Counts> = Default::default();
    for f in files {
        *map.entry(f.language).or_default() += f.counts;
    }
    let mut v: Vec<_> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.code.cmp(&a.1.code).then(a.0.cmp(b.0)));
    v
}

pub fn total(files: &[FileCount]) -> Counts {
    let mut t = Counts::default();
    for f in files {
        t += f.counts;
    }
    t
}

fn comment_ratio(c: &Counts) -> String {
    let denom = c.code + c.comment;
    if denom == 0 {
        "-".into()
    } else {
        format!("{:.1}%", c.comment as f64 * 100.0 / denom as f64)
    }
}

/// Plain text table (per language, optional per file, total).
pub fn render_text(files: &[FileCount], per_file: bool) -> String {
    let mut rows: Vec<[String; 7]> = vec![[
        "言語".into(),
        "ファイル".into(),
        "総行数".into(),
        "ステップ".into(),
        "コメント".into(),
        "空行".into(),
        "コメント率".into(),
    ]];
    let row = |name: String, c: &Counts| {
        [
            name,
            c.files.to_string(),
            c.total.to_string(),
            c.code.to_string(),
            c.comment.to_string(),
            c.blank.to_string(),
            comment_ratio(c),
        ]
    };
    for (name, c) in by_language(files) {
        rows.push(row(name.into(), &c));
    }
    rows.push(row("合計".into(), &total(files)));
    let mut out = table(&rows, 1);
    if per_file && !files.is_empty() {
        let mut detail: Vec<[String; 7]> = vec![[
            "ファイル".into(),
            "言語".into(),
            "文字コード".into(),
            "総行数".into(),
            "ステップ".into(),
            "コメント".into(),
            "空行".into(),
        ]];
        for f in files {
            detail.push([
                f.path.display().to_string(),
                f.language.into(),
                f.encoding.clone(),
                f.counts.total.to_string(),
                f.counts.code.to_string(),
                f.counts.comment.to_string(),
                f.counts.blank.to_string(),
            ]);
        }
        out.push('\n');
        out.push_str(&table(&detail, 3));
    }
    out
}

fn width(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

fn table<const N: usize>(rows: &[[String; N]], left: usize) -> String {
    let mut widths = [0usize; N];
    for r in rows {
        for (w, c) in widths.iter_mut().zip(r) {
            *w = (*w).max(width(c));
        }
    }
    let mut out = String::new();
    for (ri, r) in rows.iter().enumerate() {
        let mut line = String::new();
        for (ci, (c, w)) in r.iter().zip(widths).enumerate() {
            let pad = " ".repeat(w - width(c));
            if ci > 0 {
                line.push_str("  ");
            }
            // Names left-aligned, numbers right-aligned.
            if ci < left {
                line.push_str(c);
                line.push_str(&pad);
            } else {
                line.push_str(&pad);
                line.push_str(c);
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
        if ri == 0 {
            let total: usize = widths.iter().sum::<usize>() + 2 * (N - 1);
            out.push_str(&"-".repeat(total));
            out.push('\n');
        }
    }
    out
}

/// CSV with one row per file and a total row.
pub fn render_csv(files: &[FileCount]) -> Result<String> {
    let mut w = csv::WriterBuilder::new()
        .terminator(csv::Terminator::CRLF)
        .from_writer(Vec::new());
    let err = |e: csv::Error| e.to_string();
    w.write_record([
        "path", "language", "encoding", "total", "code", "comment", "blank",
    ])
    .map_err(err)?;
    for f in files {
        w.write_record([
            f.path.display().to_string(),
            f.language.into(),
            f.encoding.clone(),
            f.counts.total.to_string(),
            f.counts.code.to_string(),
            f.counts.comment.to_string(),
            f.counts.blank.to_string(),
        ])
        .map_err(err)?;
    }
    let t = total(files);
    w.write_record([
        "(total)".into(),
        String::new(),
        String::new(),
        t.total.to_string(),
        t.code.to_string(),
        t.comment.to_string(),
        t.blank.to_string(),
    ])
    .map_err(err)?;
    let bytes = w.into_inner().map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

fn counts_json(c: &Counts) -> serde_json::Value {
    serde_json::json!({
        "files": c.files,
        "total": c.total,
        "code": c.code,
        "comment": c.comment,
        "blank": c.blank,
    })
}

/// JSON with files, per-language totals and the grand total.
pub fn render_json(files: &[FileCount]) -> String {
    let v = serde_json::json!({
        "files": files.iter().map(|f| serde_json::json!({
            "path": f.path.display().to_string(),
            "language": f.language,
            "encoding": f.encoding,
            "total": f.counts.total,
            "code": f.counts.code,
            "comment": f.counts.comment,
            "blank": f.counts.blank,
        })).collect::<Vec<_>>(),
        "languages": by_language(files).iter().map(|(n, c)| {
            let mut o = counts_json(c);
            o["language"] = (*n).into();
            o
        }).collect::<Vec<_>>(),
        "total": counts_json(&total(files)),
    });
    let mut s = serde_json::to_string_pretty(&v).unwrap_or_default();
    s.push('\n');
    s
}

/// Names shown in the GUI language selector.
pub const LANGUAGE_CHOICES: &[&str] = &[
    "Java",
    "C",
    "C++",
    "C#",
    "JavaScript",
    "TypeScript",
    "Python",
    "Go",
    "Rust",
    "Kotlin",
    "Swift",
    "PHP",
    "Ruby",
    "Visual Basic",
    "COBOL",
    "SQL",
    "Shell",
    "PowerShell",
    "Batch",
    "Perl",
    "Scala",
    "Dart",
    "Objective-C",
    "Lua",
    "R",
    "Haskell",
    "Pascal/Delphi",
    "Fortran",
    "HTML",
    "XML",
    "CSS",
    "SCSS/Less",
    "YAML",
    "Makefile",
    "Dockerfile",
    "CMake",
];

/// GUI/text tool: counts the input text as one file.
pub fn tool(input: &str, params: &Params) -> Result<Output> {
    let name = value(params, "language", "Java");
    let lang = find_language(name).ok_or_else(|| format!("未対応の言語です: {name}"))?;
    let file = FileCount {
        path: PathBuf::from("(入力)"),
        language: lang.name,
        encoding: "UTF-8".into(),
        counts: count(input, lang),
    };
    let files = [file];
    let text = render_text(&files, false);
    Ok(Output::text(
        text,
        "steps.txt",
        "ステップは、コメントと空白以外を含む行です。フォルダー単位の集計は yytoolbox-cli steps を使います。",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(lang: &str, text: &str) -> (u64, u64, u64) {
        let r = count(text, find_language(lang).unwrap());
        assert_eq!(r.total, r.code + r.comment + r.blank);
        (r.code, r.comment, r.blank)
    }

    #[test]
    fn c_family() {
        let src = "/* header\n * text\n */\n#include <stdio.h>\n\nint main() { // entry\n  printf(\"/* not a comment */\");\n  return 0; /* trailing */\n}\n";
        assert_eq!(c("C", src), (5, 3, 1));
        assert_eq!(c("Java", "String s = \"//\"; \n// x\n"), (1, 1, 0));
        assert_eq!(
            c("JavaScript", "const a = `\n// in template\n`;\n"),
            (3, 0, 0)
        );
        assert_eq!(c("C", "char c = '\"'; // q\nx();\n"), (2, 0, 0));
        assert_eq!(c("C", "/* a */ /* b */\n"), (0, 1, 0));
        assert_eq!(c("C", "/* a */ x /* b */\n"), (1, 0, 0));
        assert_eq!(c("C", "a\r\n\r\n// b\r\n"), (1, 1, 1));
        assert_eq!(c("C", "/*\n\n*/\n"), (0, 2, 1));
    }

    #[test]
    fn rust_nesting_lifetimes_and_raw_strings() {
        let src = "/* outer /* inner */ still comment */\nfn f<'a>(x: &'a str) -> char { 'x' }\nlet s = r#\"/* \"# ;\n// done\n";
        assert_eq!(c("Rust", src), (2, 2, 0));
        assert_eq!(c("Rust", "let c = '\\'';\n// x\n"), (1, 1, 0));
    }

    #[test]
    fn python_docstrings_and_hash() {
        let src = "\"\"\"Module doc.\n\nMore.\n\"\"\"\nimport os  # comment\n\ndef f():\n    '''doc'''\n    x = \"\"\"\n# not a comment\n\"\"\"\n    return '#'\n";
        assert_eq!(c("Python", src), (6, 4, 2));
    }

    #[test]
    fn line_oriented_languages() {
        let cobol = "000100 IDENTIFICATION DIVISION.\n000200*COMMENT LINE\n000300 PROGRAM-ID. TEST.                                             SEQ00001\n000400\n000500     DISPLAY 'HELLO'. *> trailing\n000600/\n       *> free comment\n";
        assert_eq!(c("COBOL", cobol), (3, 3, 1));
        assert_eq!(c("Visual Basic", "' c\nREM x\nDim s = \"'\"\n"), (1, 2, 0));
        assert_eq!(c("Batch", "@echo off\nrem x\n:: y\nREMARK\n"), (2, 2, 0));
        assert_eq!(c("Ruby", "=begin\ndoc\n=end\nputs 1 # x\n"), (1, 3, 0));
        assert_eq!(c("SQL", "SELECT '--' FROM t; -- c\n/* x */\n"), (1, 1, 0));
        assert_eq!(c("Lua", "--[[ a\nb ]]\nprint(1) -- c\n"), (1, 2, 0));
        assert_eq!(c("HTML", "<!-- a\nb -->\n<p>x</p>\n"), (1, 2, 0));
        assert_eq!(c("Haskell", "{- a {- b -} c -}\nmain = 1\n"), (1, 1, 0));
        assert_eq!(c("PowerShell", "<# a\n#>\nWrite-Host 'x' # c\n"), (1, 2, 0));
        assert_eq!(
            c("Pascal/Delphi", "{ a }\n(* b *)\nbegin end.\n"),
            (1, 2, 0)
        );
    }

    #[test]
    fn edge_cases() {
        assert_eq!(c("C", ""), (0, 0, 0));
        assert_eq!(c("C", "x"), (1, 0, 0));
        assert_eq!(c("C", "\u{feff}x\n\n"), (1, 0, 1));
        assert!(find_language("cpp").is_some());
        assert!(find_language("csharp").is_some());
        assert_eq!(find_language(".py").unwrap().name, "Python");
        assert_eq!(
            language_for_path(Path::new("dir/Makefile")).unwrap().name,
            "Makefile"
        );
        assert_eq!(language_for_path(Path::new("A.JAVA")).unwrap().name, "Java");
        for name in LANGUAGE_CHOICES {
            assert!(find_language(name).is_some(), "{name}");
        }
    }

    #[test]
    fn shift_jis_backslash_bytes_do_not_escape_quotes() {
        // "表" in Shift_JIS ends with 0x5C. Decoding first keeps the string closed.
        let mut bytes = b"char *s = \"".to_vec();
        bytes.extend([0x95, 0x5C]);
        bytes.extend(b"\"; // c\n/* x */\n");
        let (text, enc) = decode(&bytes);
        assert!(
            enc.to_ascii_lowercase().contains("shift") || enc.contains("932"),
            "{enc}"
        );
        assert_eq!(count(&text, find_language("C").unwrap()).comment, 1);
    }
}

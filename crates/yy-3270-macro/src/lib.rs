//! 3270 のマクロ（14 章 13 節）。
//!
//! スクリプトの言語は [Rhai](https://rhai.rs)。スクリプトは画面を持つ側（yyterm の UI のスレッド）とは
//! 別のスレッドで動き、[`Host`] を通して画面を読み・キーを送り・変化を待つ。スクリプトに渡すのは
//! 下の操作だけで、任意のプログラムの実行・ネットワークへの接続はできない。ファイルに書けるのは
//! マクロの出力のフォルダの中だけ。パスワードは [`Host`] が資格情報から入れ、スクリプトには渡さない。
//!
//! | 操作 | 説明 |
//! |------|------|
//! | `wait_unlocked(秒)` | キーボードのロックが解けるまで待つ |
//! | `wait_text("文字", 秒)`・`wait_text_at(行, 桁, "文字", 秒)` | 画面（その位置）に文字が出るまで待つ。秒が 0 なら今あるかを返す |
//! | `text_at(行, 桁, 長さ)`・`row(行)`・`screen_lines()`・`cursor()`・`rows()`・`cols()` | 画面を読む |
//! | `fields()`・`field_at(行, 桁)` | フィールド（位置・長さ・保護・数字・中身） |
//! | `move_to(行, 桁)`・`tab()`・`home()`・`type("文字")`・`key("PF8")` | 入力する |
//! | `password("名前")` | 資格情報マネージャーのパスワードを入れる |
//! | `transfer_get(ホスト, 手元, #{…})`・`transfer_put(手元, ホスト, #{…})` | IND$FILE |
//! | `print_screen()` | 画面を印刷の出力先へ |
//! | `csv_open(パス)`・`text_open(パス)` → `write_row([…])`・`write_line(文字)` | 出力のフォルダに書く |
//! | `log(…)`・`sleep(ミリ秒)`・`ask(質問)`・`message(…)` | 記録・待ち・問い合わせ・表示 |
//!
//! 行・桁は 1 から数える。2 バイト文字は 2 桁を占める（`text_at` は桁で切る）。

use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map, Position};
use yy_3270::Key;
use yy_3270::codes::*;
use yy_3270::ind_file::{Direction, HostKind, Mode as FtMode, Recfm, Request};
use yy_encoding::Ccsid;

pub mod record;
pub mod tcp;
pub use record::Recorder;

/// 待つ操作の既定の時間の上限（秒）
pub const DEFAULT_TIMEOUT: u64 = 30;

/// 画面の 1 つのフィールド。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldInfo {
    /// 中身の始まりの行・桁（1 から）
    pub row: usize,
    pub col: usize,
    pub len: usize,
    pub protected: bool,
    pub numeric: bool,
    /// 非表示（パスワードなど）。中身は渡さない
    pub hidden: bool,
    pub text: String,
}

/// スクリプトから見た画面と状態。
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// 行ごとのセルの文字（2 バイト文字の 2 セル目は空文字。非表示・属性・null は空白）
    pub cells: Vec<Vec<String>>,
    /// カーソルの行・桁（1 から）
    pub cursor: (usize, usize),
    /// キーボードがロックされている（ホストの応答待ち・操作の誤り）
    pub locked: bool,
    pub connected: bool,
    pub fields: Vec<FieldInfo>,
    pub ccsid: Ccsid,
}

impl Snapshot {
    /// セッションの今の画面から作る。
    pub fn of(session: &yy_3270::Session, connected: bool) -> Snapshot {
        let screen = session.screen();
        let (rows, cols) = (screen.rows, screen.cols);
        let disp = session.display();
        let mut cells = vec![vec![String::new(); cols]; rows];
        for (i, c) in disp.iter().enumerate().take(rows * cols) {
            cells[i / cols][i % cols] = if c.width == 0 {
                String::new()
            } else if c.hidden || c.attribute || c.text.is_empty() {
                " ".to_owned()
            } else {
                c.text.clone()
            };
        }
        let mut fields = Vec::new();
        let attrs = screen.field_attrs();
        let n = screen.len();
        for (k, &a) in attrs.iter().enumerate() {
            let fa = screen.cells[a].fa.unwrap_or(0);
            let next = attrs.get(k + 1).copied().unwrap_or(attrs[0] + n);
            let start = (a + 1) % n;
            let len = (next + n - a - 1) % n;
            let hidden = fa & FA_DISPLAY_MASK == FA_NONDISPLAY;
            let text = if hidden {
                String::new()
            } else {
                (0..len)
                    .map(|j| {
                        let p = (start + j) % n;
                        cells[p / cols][p % cols].as_str()
                    })
                    .collect::<String>()
            };
            fields.push(FieldInfo {
                row: start / cols + 1,
                col: start % cols + 1,
                len,
                protected: fa & FA_PROTECT != 0,
                numeric: fa & FA_NUMERIC != 0,
                hidden,
                text,
            });
        }
        let cur = screen.cursor;
        Snapshot {
            cells,
            cursor: (cur / cols + 1, cur % cols + 1),
            locked: session.oia().lock != yy_3270::Lock::None,
            // 交渉中も接続している（キーボードはロックされているので、待つ操作は待ち続ける）
            connected,
            fields,
            ccsid: session.ccsid(),
        }
    }

    pub fn rows(&self) -> usize {
        self.cells.len()
    }

    pub fn cols(&self) -> usize {
        self.cells.first().map_or(0, Vec::len)
    }

    /// 行（1 から）の文字。
    pub fn row(&self, r: usize) -> String {
        self.text_at(r, 1, self.cols())
    }

    /// 行・桁（1 から）から `len` 桁の文字。画面の外は空。
    pub fn text_at(&self, r: usize, c: usize, len: usize) -> String {
        let Some(line) = r.checked_sub(1).and_then(|r| self.cells.get(r)) else {
            return String::new();
        };
        let from = c.saturating_sub(1).min(line.len());
        let to = (from + len).min(line.len());
        line[from..to].concat()
    }

    pub fn lines(&self) -> Vec<String> {
        (1..=self.rows()).map(|r| self.row(r)).collect()
    }
}

/// スクリプトがホストに頼む操作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Key(Key),
    /// 文字を入力する（入力の規則を守る）
    Type(String),
    /// 行・桁（1 から）へカーソルを移す
    MoveTo(usize, usize),
    /// 資格情報の名前のパスワードを、今のフィールドに入れる
    Password(String),
    /// IND$FILE（終わるまで待って結果を返す）
    Transfer {
        request: Request,
        local: PathBuf,
    },
    PrintScreen,
    Log(String),
    Message(String),
    /// 利用者に尋ねる（答えを返す。キャンセルは `None`）
    Ask(String),
}

/// 操作の結果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Answer {
    #[default]
    Done,
    Text(Option<String>),
    Transfer {
        ok: bool,
        message: String,
    },
}

/// スクリプトから見たホスト（yyterm のタブ、テストの模擬ホスト）。
pub trait Host: Send + Sync {
    fn snapshot(&self) -> Result<Snapshot, String>;
    /// 画面・状態が変わるたびに増える数。
    fn generation(&self) -> u64;
    /// `generation` が `since` から変わるか、`timeout` まで待つ。
    fn wait_change(&self, since: u64, timeout: Duration);
    fn act(&self, op: Op) -> Result<Answer, String>;
    /// 利用者が止めた。
    fn stopped(&self) -> bool;
}

/// マクロの誤り（行番号つき）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacroError {
    /// 行（1 から。分からなければ 0）
    pub line: usize,
    pub message: String,
    /// 利用者が止めた
    pub stopped: bool,
}

impl std::fmt::Display for MacroError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 {
            write!(f, "{} 行目: {}", self.line, self.message)
        } else {
            f.write_str(&self.message)
        }
    }
}

/// マクロの実行の設定。
#[derive(Clone, Debug)]
pub struct Options {
    /// ファイルを書けるフォルダ（`csv_open`・`text_open`・`transfer_get` の手元のファイル）
    pub out_dir: PathBuf,
    /// 待つ操作の既定の時間の上限（秒）
    pub timeout: u64,
}

const STOPPED: &str = "停止しました";

type Res<T> = Result<T, Box<EvalAltResult>>;

fn err<T>(msg: impl Into<String>) -> Res<T> {
    Err(msg.into().into())
}

/// スクリプトを実行する（終わるまで戻らない。別のスレッドで呼ぶ）。
pub fn run(script: &str, host: Arc<dyn Host>, opts: Options) -> Result<(), MacroError> {
    let engine = engine(host.clone(), opts);
    let r = engine.run(script);
    r.map_err(|e| {
        let line = e.position().line().unwrap_or(0);
        let stopped = host.stopped();
        let message = match *e {
            EvalAltResult::ErrorRuntime(ref v, _) => v.to_string(),
            EvalAltResult::ErrorTerminated(..) => STOPPED.to_owned(),
            ref other => {
                // 位置は行番号として別に出す
                let mut s = other.to_string();
                if let Some(i) = s.rfind(" (line ") {
                    s.truncate(i);
                }
                s
            }
        };
        MacroError {
            line,
            message: if stopped { STOPPED.to_owned() } else { message },
            stopped,
        }
    })
}

/// スクリプトを実行せずに文法だけ確かめる。
pub fn check(script: &str) -> Result<(), MacroError> {
    let engine = Engine::new_raw();
    engine.compile(script).map(|_| ()).map_err(|e| MacroError {
        line: e.position().line().unwrap_or(0),
        message: e.err_type().to_string(),
        stopped: false,
    })
}

fn to_usize(v: i64, what: &str) -> Res<usize> {
    usize::try_from(v)
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(|| format!("{what}は 1 以上で指定してください（{v}）").into())
}

/// キーの名前（`Enter`・`PF8`・`PA1`・`Clear` など。大文字・小文字は問わない）。
pub fn parse_key(name: &str) -> Option<Key> {
    let n = name.trim().to_ascii_uppercase();
    let num = |p: &str| n.strip_prefix(p).and_then(|d| d.parse::<u8>().ok());
    if let Some(k) = num("PF").filter(|k| (1..=24).contains(k)) {
        return Some(Key::Pf(k));
    }
    if let Some(k) = num("PA").filter(|k| (1..=3).contains(k)) {
        return Some(Key::Pa(k));
    }
    Some(match n.as_str() {
        "ENTER" => Key::Enter,
        "CLEAR" => Key::Clear,
        "SYSREQ" => Key::SysReq,
        "ATTN" => Key::Attn,
        "RESET" => Key::Reset,
        "TAB" => Key::Tab,
        "BACKTAB" => Key::BackTab,
        "HOME" => Key::Home,
        "NEWLINE" => Key::NewLine,
        "UP" => Key::Up,
        "DOWN" => Key::Down,
        "LEFT" => Key::Left,
        "RIGHT" => Key::Right,
        "BACKSPACE" => Key::Backspace,
        "DELETE" => Key::Delete,
        "ERASEEOF" => Key::EraseEof,
        "ERASEINPUT" => Key::EraseInput,
        "INSERT" => Key::Insert,
        "DUP" => Key::Dup,
        "FIELDMARK" => Key::FieldMark,
        _ => return None,
    })
}

/// 書き出すファイル（`csv_open`・`text_open`）。
#[derive(Clone)]
pub struct OutFile {
    inner: Arc<Mutex<Option<std::io::BufWriter<std::fs::File>>>>,
    path: PathBuf,
}

impl OutFile {
    fn write(&self, s: &str) -> Res<()> {
        let mut g = self.inner.lock().unwrap();
        let Some(w) = g.as_mut() else {
            return err(format!("{} は閉じています", self.path.display()));
        };
        w.write_all(s.as_bytes())
            .and_then(|()| w.flush())
            .or_else(|e| err(format!("{} に書けません: {e}", self.path.display())))
    }
}

/// CSV の 1 項目（カンマ・引用符・改行を含めば引用符で囲む）。
pub fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// 出力のフォルダの中のパス（絶対パス・`..` で外に出るものは断る）。
pub fn resolve_out(out_dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel.trim());
    if rel.trim().is_empty() {
        return Err("ファイルの名前が空です".into());
    }
    if p.is_absolute()
        || p.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Err(format!(
            "マクロが書けるのは出力のフォルダ（{}）の中だけです: {rel}",
            out_dir.display()
        ));
    }
    Ok(out_dir.join(p))
}

fn open_out(out_dir: &Path, rel: &str, bom: bool) -> Res<OutFile> {
    let path = resolve_out(out_dir, rel)?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)
            .or_else(|e| err(format!("{} を作れません: {e}", d.display())))?;
    }
    let f = std::fs::File::create(&path)
        .or_else(|e| err(format!("{} を作れません: {e}", path.display())))?;
    let mut w = std::io::BufWriter::new(f);
    if bom {
        // Excel で開いても文字化けしないように
        let _ = w.write_all("\u{FEFF}".as_bytes());
    }
    Ok(OutFile {
        inner: Arc::new(Mutex::new(Some(w))),
        path,
    })
}

/// IND$FILE の指定（スクリプトの `#{host: "cms", mode: "binary", …}`）。
fn transfer_request(
    direction: Direction,
    host_file: &str,
    opts: &Map,
    ccsid: Ccsid,
) -> Res<Request> {
    let get = |k: &str| opts.get(k).map(|v| v.to_string());
    let host = match get("host")
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("tso") => HostKind::Tso,
        Some("cms") => HostKind::Cms,
        Some("cics") => HostKind::Cics,
        Some(h) => return err(format!("host は tso・cms・cics のどれかです（{h}）")),
    };
    let mode = match get("mode")
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("text") => FtMode::Text(ccsid),
        Some("ascii") => FtMode::HostAscii,
        Some("binary") => FtMode::Binary,
        Some(m) => return err(format!("mode は text・ascii・binary のどれかです（{m}）")),
    };
    let recfm = match get("recfm")
        .as_deref()
        .map(str::to_ascii_uppercase)
        .as_deref()
    {
        None | Some("") => Recfm::Default,
        Some("F") => Recfm::Fixed,
        Some("V") => Recfm::Variable,
        Some("U") => Recfm::Undefined,
        Some(r) => return err(format!("recfm は F・V・U のどれかです（{r}）")),
    };
    let num = |k: &str| -> Res<u32> {
        match opts.get(k) {
            None => Ok(0),
            Some(v) => v
                .as_int()
                .ok()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| format!("{k} は 0 以上の数で指定してください").into()),
        }
    };
    Ok(Request {
        host,
        direction,
        host_file: host_file.to_owned(),
        mode,
        recfm,
        lrecl: num("lrecl")?,
        space: num("space")?,
        append: opts
            .get("append")
            .and_then(|v| v.as_bool().ok())
            .unwrap_or(false),
    })
}

fn field_map(f: &FieldInfo) -> Map {
    let mut m = Map::new();
    m.insert("row".into(), (f.row as i64).into());
    m.insert("col".into(), (f.col as i64).into());
    m.insert("len".into(), (f.len as i64).into());
    m.insert("protected".into(), f.protected.into());
    m.insert("numeric".into(), f.numeric.into());
    m.insert("hidden".into(), f.hidden.into());
    m.insert("text".into(), f.text.clone().into());
    m
}

/// 条件が満たされるまで待つ。`secs` が 0 なら今の状態だけを見て返す。
/// 時間切れ・切断・停止はエラー。
fn wait_until(
    host: &dyn Host,
    secs: i64,
    what: &str,
    cond: impl Fn(&Snapshot) -> bool,
) -> Res<bool> {
    let deadline = Instant::now() + Duration::from_secs(secs.max(0) as u64);
    loop {
        if host.stopped() {
            return err(STOPPED);
        }
        let generation = host.generation();
        let snap = host.snapshot()?;
        if cond(&snap) {
            return Ok(true);
        }
        if secs <= 0 {
            return Ok(false);
        }
        if !snap.connected {
            return err(format!("{what}を待っている間に切断されました"));
        }
        let now = Instant::now();
        if now >= deadline {
            return err(format!("{what}を {secs} 秒待ちましたが、時間切れです"));
        }
        host.wait_change(generation, (deadline - now).min(Duration::from_millis(250)));
    }
}

fn act(host: &dyn Host, op: Op) -> Res<Answer> {
    if host.stopped() {
        return err(STOPPED);
    }
    host.act(op).or_else(err)
}

/// 操作を登録したエンジン。
fn engine(host: Arc<dyn Host>, opts: Options) -> Engine {
    let mut e = Engine::new();
    e.set_max_expr_depths(64, 64);
    // 止められたら次の文で終える（無限の繰り返しでも止まる）
    {
        let h = host.clone();
        e.on_progress(move |_| h.stopped().then(|| Dynamic::from(STOPPED)));
    }
    {
        let h = host.clone();
        e.on_print(move |s| {
            let _ = h.act(Op::Log(s.to_owned()));
        });
    }
    {
        let h = host.clone();
        e.on_debug(move |s, _, pos| {
            let _ = h.act(Op::Log(format!("{pos:?}: {s}")));
        });
    }
    // Rhai の trim は文字列をその場で変えて何も返さない。`text_at(…).trim()` と書けるよう、
    // 変えた上で結果も返す
    e.register_fn("trim", |s: &mut ImmutableString| -> ImmutableString {
        let t: ImmutableString = s.trim().into();
        *s = t.clone();
        t
    });
    let timeout = opts.timeout.max(1) as i64;
    let out_dir = opts.out_dir.clone();

    macro_rules! reg {
        ($name:expr, |$h:ident $(, $a:ident : $t:ty)*| $body:expr) => {{
            let $h = host.clone();
            e.register_fn($name, move |$($a: $t),*| -> Res<_> {
                let $h: &dyn Host = &*$h;
                $body
            });
        }};
    }

    // ---- 待つ ----
    reg!("wait_unlocked", |h, secs: i64| wait_until(
        h,
        secs,
        "ロックが解けるの",
        |s| !s.locked
    ));
    reg!("wait_unlocked", |h| wait_until(
        h,
        timeout,
        "ロックが解けるの",
        |s| !s.locked
    ));
    reg!("wait_text", |h, text: ImmutableString, secs: i64| {
        let t = text.to_string();
        wait_until(h, secs, &format!("「{t}」"), |s| {
            s.lines().iter().any(|l| l.contains(t.as_str()))
        })
    });
    reg!("wait_text", |h, text: ImmutableString| {
        let t = text.to_string();
        wait_until(h, timeout, &format!("「{t}」"), |s| {
            s.lines().iter().any(|l| l.contains(t.as_str()))
        })
    });
    {
        let at = move |h: &dyn Host, r: i64, c: i64, text: &str, secs: i64| -> Res<bool> {
            let (r, c) = (to_usize(r, "行")?, to_usize(c, "桁")?);
            let len = text
                .chars()
                .map(|ch| if yy_3270::print::is_wide(ch) { 2 } else { 1 })
                .sum();
            wait_until(h, secs, &format!("{r} 行 {c} 桁の「{text}」"), |s| {
                s.text_at(r, c, len) == text
            })
        };
        let h = host.clone();
        e.register_fn(
            "wait_text_at",
            move |r: i64, c: i64, text: ImmutableString, secs: i64| at(&*h, r, c, &text, secs),
        );
        let h = host.clone();
        e.register_fn(
            "wait_text_at",
            move |r: i64, c: i64, text: ImmutableString| at(&*h, r, c, &text, timeout),
        );
    }
    reg!("wait_change", |h, secs: i64| {
        let g = h.generation();
        h.wait_change(g, Duration::from_millis((secs.max(0) as u64) * 1000));
        Ok(h.generation() != g)
    });

    // ---- 読む ----
    reg!("text_at", |h, r: i64, c: i64, len: i64| {
        let s = h.snapshot()?;
        Ok(s.text_at(to_usize(r, "行")?, to_usize(c, "桁")?, len.max(0) as usize))
    });
    reg!("row", |h, r: i64| Ok(h.snapshot()?.row(to_usize(r, "行")?)));
    reg!("screen_lines", |h| {
        Ok(h.snapshot()?
            .lines()
            .into_iter()
            .map(Dynamic::from)
            .collect::<Array>())
    });
    reg!("screen_text", |h| Ok(h.snapshot()?.lines().join("\n")));
    reg!("rows", |h| Ok(h.snapshot()?.rows() as i64));
    reg!("cols", |h| Ok(h.snapshot()?.cols() as i64));
    reg!("cursor", |h| {
        let s = h.snapshot()?;
        let mut m = Map::new();
        m.insert("row".into(), (s.cursor.0 as i64).into());
        m.insert("col".into(), (s.cursor.1 as i64).into());
        Ok(m)
    });
    reg!("locked", |h| Ok(h.snapshot()?.locked));
    reg!("fields", |h| {
        Ok(h.snapshot()?
            .fields
            .iter()
            .map(|f| Dynamic::from_map(field_map(f)))
            .collect::<Array>())
    });
    reg!("field_at", |h, r: i64, c: i64| {
        let s = h.snapshot()?;
        let (r, c) = (to_usize(r, "行")?, to_usize(c, "桁")?);
        let cols = s.cols();
        let at = (r - 1) * cols + (c - 1);
        let n = s.rows() * cols;
        Ok(s.fields
            .iter()
            .find(|f| {
                let start = (f.row - 1) * cols + (f.col - 1);
                (at + n - start) % n.max(1) < f.len
            })
            .map_or(Dynamic::UNIT, |f| Dynamic::from_map(field_map(f))))
    });

    // ---- 入力する ----
    reg!("move_to", |h, r: i64, c: i64| {
        act(h, Op::MoveTo(to_usize(r, "行")?, to_usize(c, "桁")?)).map(|_| ())
    });
    reg!("tab", |h| act(h, Op::Key(Key::Tab)).map(|_| ()));
    reg!("home", |h| act(h, Op::Key(Key::Home)).map(|_| ()));
    reg!("type", |h, text: ImmutableString| act(
        h,
        Op::Type(text.to_string())
    )
    .map(|_| ()));
    reg!("input", |h, text: ImmutableString| act(
        h,
        Op::Type(text.to_string())
    )
    .map(|_| ()));
    reg!("key", |h, name: ImmutableString| {
        let k = parse_key(&name).ok_or_else(|| {
            format!("キー「{name}」はありません（Enter・PF1〜PF24・PA1〜PA3・Clear など）")
        })?;
        act(h, Op::Key(k)).map(|_| ())
    });
    reg!("password", |h, name: ImmutableString| act(
        h,
        Op::Password(name.to_string())
    )
    .map(|_| ()));

    // ---- 転送・印刷 ----
    {
        let h = host.clone();
        let dir = out_dir.clone();
        let get = move |host_file: &str, local: &str, opts: &Map| -> Res<Map> {
            let snap = h.snapshot()?;
            let request = transfer_request(Direction::Receive, host_file, opts, snap.ccsid)?;
            let local = resolve_out(&dir, local)?;
            transfer_answer(act(&*h, Op::Transfer { request, local })?)
        };
        let g2 = get.clone();
        e.register_fn(
            "transfer_get",
            move |hf: ImmutableString, local: ImmutableString| g2(&hf, &local, &Map::new()),
        );
        e.register_fn(
            "transfer_get",
            move |hf: ImmutableString, local: ImmutableString, opts: Map| get(&hf, &local, &opts),
        );
        let h = host.clone();
        let dir = out_dir.clone();
        let put = move |local: &str, host_file: &str, opts: &Map| -> Res<Map> {
            let snap = h.snapshot()?;
            let request = transfer_request(Direction::Send, host_file, opts, snap.ccsid)?;
            // 送るファイルは出力のフォルダの外でもよい（読むだけ）
            let p = Path::new(local);
            let local = if p.is_absolute() {
                p.to_path_buf()
            } else {
                dir.join(p)
            };
            transfer_answer(act(&*h, Op::Transfer { request, local })?)
        };
        let p2 = put.clone();
        e.register_fn(
            "transfer_put",
            move |local: ImmutableString, hf: ImmutableString| p2(&local, &hf, &Map::new()),
        );
        e.register_fn(
            "transfer_put",
            move |local: ImmutableString, hf: ImmutableString, opts: Map| put(&local, &hf, &opts),
        );
    }
    reg!("print_screen", |h| act(h, Op::PrintScreen).map(|_| ()));

    // ---- ファイル ----
    e.register_type_with_name::<OutFile>("OutFile");
    {
        let dir = out_dir.clone();
        e.register_fn("csv_open", move |p: ImmutableString| {
            open_out(&dir, &p, true)
        });
        let dir = out_dir.clone();
        e.register_fn("text_open", move |p: ImmutableString| {
            open_out(&dir, &p, false)
        });
    }
    e.register_fn("write_row", |f: &mut OutFile, row: Array| -> Res<()> {
        let line: Vec<String> = row.iter().map(|v| csv_field(&v.to_string())).collect();
        f.write(&format!("{}\r\n", line.join(",")))
    });
    e.register_fn(
        "write_line",
        |f: &mut OutFile, s: ImmutableString| -> Res<()> { f.write(&format!("{s}\r\n")) },
    );
    e.register_fn("close", |f: &mut OutFile| {
        f.inner.lock().unwrap().take();
    });
    e.register_get("path", |f: &mut OutFile| {
        f.path.to_string_lossy().into_owned()
    });

    // ---- そのほか ----
    reg!("log", |h, s: Dynamic| act(h, Op::Log(s.to_string()))
        .map(|_| ()));
    reg!("message", |h, s: Dynamic| act(
        h,
        Op::Message(s.to_string())
    )
    .map(|_| ()));
    reg!(
        "ask",
        |h, q: ImmutableString| match act(h, Op::Ask(q.to_string()))? {
            Answer::Text(Some(t)) => Ok(Dynamic::from(t)),
            _ => Ok(Dynamic::UNIT),
        }
    );
    reg!("sleep", |h, ms: i64| {
        let deadline = Instant::now() + Duration::from_millis(ms.max(0) as u64);
        while Instant::now() < deadline {
            if h.stopped() {
                return err(STOPPED);
            }
            std::thread::sleep((deadline - Instant::now()).min(Duration::from_millis(100)));
        }
        Ok(())
    });
    e
}

fn transfer_answer(a: Answer) -> Res<Map> {
    let (ok, message) = match a {
        Answer::Transfer { ok, message } => (ok, message),
        _ => (false, "転送の結果がありません".to_owned()),
    };
    let mut m = Map::new();
    m.insert("ok".into(), ok.into());
    m.insert("message".into(), message.into());
    Ok(m)
}

/// 位置の説明（記録用）。
pub fn position_text(p: Position) -> String {
    p.line().map(|l| format!("{l} 行目")).unwrap_or_default()
}

#[cfg(test)]
mod tests;

//! 3270 のタブ（14 章 9）。
//!
//! 接続（TCP、または SSH の direct-tcpip の中継）はシェルのタブと同じ読み書きのスレッドに乗せ、
//! Telnet・3270 データストリームの処理は UI のスレッドで `yy-3270` の [`Session`] に通す。
//! 画面は、3270 の画面（と OIA の 1 行）を端末の画面（`yy-term`）に色・位置の制御で写して描く
//! （描画・選択・コピーはシェルのタブと共通）。

use std::io;
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::HFONT;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;
use yy_3270::ind_file::{self, Direction, Local, Mode as FtMode};
use yy_3270::{Config as SessionConfig, DisplayCell, Key, Lock, Mode, Session};
use yy_config::Config;
use yy_encoding::Ccsid;
use yy_remote::uri::Target;
use yy_term::Terminal;
use yy_term::keys::Mods;

use super::ftdlg::Choice;
use super::pty::Backend;

/// 既定のポート
const DEFAULT_PORT: u16 = 23;
/// 接続を待つ時間の上限
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// 3270 の接続先。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target3270 {
    /// 表示用の名前（設定の名前、または `ホスト`）
    pub label: String,
    pub host: String,
    pub port: u16,
    pub lu: Option<String>,
    pub model: u8,
    pub ccsid: Ccsid,
    /// この SSH の接続を経由する
    pub ssh: Option<String>,
    pub terminal_type: Option<String>,
    pub tn3270e: bool,
    /// プリンター（3287）のセッション
    pub printer: bool,
    /// プリンターで、対応づける端末の LU（ASSOCIATE）
    pub associate: Option<String>,
    /// 設定のプリンターの LU 名
    pub printer_lu: Option<String>,
    /// 端末を開いたらプリンターも開く
    pub auto_printer: bool,
    /// 接続したら実行するマクロ
    pub on_connect: Option<String>,
}

impl Target3270 {
    /// 入力を読む: `tn3270://[LU@]ホスト[:ポート]`・`[LU@]ホスト[:ポート]`・設定の名前。
    pub(crate) fn parse(input: &str, cfg: &Config) -> Result<Target3270, String> {
        let t = &cfg.tn3270;
        let text = input.trim();
        let ccsid_of =
            |n: u32| Ccsid::from_number(n).ok_or_else(|| format!("CCSID {n} には対応していません"));
        let base_ccsid = ccsid_of(t.ccsid)?;
        let terminal_type = Some(t.terminal_type.clone()).filter(|s| !s.trim().is_empty());
        if let Some(h) = t
            .host
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(text))
            .map(|(_, h)| h)
        {
            return Ok(Target3270 {
                label: text.to_owned(),
                host: h.host.clone(),
                port: h.port.unwrap_or(DEFAULT_PORT),
                lu: h.lu.clone().filter(|l| !l.is_empty()),
                model: h.model.unwrap_or(t.model),
                ccsid: match h.ccsid {
                    Some(n) => ccsid_of(n)?,
                    None => base_ccsid,
                },
                ssh: h.ssh.clone().filter(|s| !s.is_empty()),
                terminal_type,
                tn3270e: t.tn3270e,
                printer: false,
                associate: None,
                printer_lu: h.printer_lu.clone().filter(|l| !l.is_empty()),
                auto_printer: h
                    .printer
                    .as_deref()
                    .is_some_and(|p| p.eq_ignore_ascii_case("auto")),
                on_connect: h.on_connect.clone().filter(|m| !m.trim().is_empty()),
            });
        }
        let rest = text
            .strip_prefix("tn3270://")
            .unwrap_or(text)
            .trim_end_matches('/');
        let (lu, hostport) = match rest.rsplit_once('@') {
            Some((l, h)) if !l.is_empty() => (Some(l.to_owned()), h),
            _ => (None, rest),
        };
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') || h.starts_with('[') => (
                h.trim_matches(['[', ']']).to_owned(),
                p.parse::<u16>()
                    .map_err(|_| format!("ポート（{p}）を読めません"))?,
            ),
            _ => (hostport.trim_matches(['[', ']']).to_owned(), DEFAULT_PORT),
        };
        if host.is_empty() || host.contains(char::is_whitespace) {
            return Err(format!(
                "接続先（{text}）を読めません。例: mvs01.example.co.jp、tn3270://TCP00042@mvs01:23"
            ));
        }
        Ok(Target3270 {
            label: host.clone(),
            host,
            port,
            lu,
            model: t.model,
            ccsid: base_ccsid,
            ssh: None,
            terminal_type,
            tn3270e: t.tn3270e,
            printer: false,
            associate: None,
            printer_lu: None,
            auto_printer: false,
            on_connect: None,
        })
    }

    /// この端末に対応するプリンターの接続先。設定のプリンターの LU 名があればそれを、なければ
    /// 端末に割り当てられた LU（`terminal_lu`）に対応するプリンター（ASSOCIATE）を求める。
    pub(crate) fn printer_target(&self, terminal_lu: Option<&str>) -> Result<Target3270, String> {
        let associate = match (&self.printer_lu, terminal_lu) {
            (Some(_), _) => None,
            (None, Some(lu)) if !lu.is_empty() => Some(lu.to_owned()),
            _ => {
                return Err(
                    "プリンターの LU が分かりません。TN3270E で接続して LU が割り当てられてから選ぶか、\
                     設定の [tn3270.host.名前] に printer_lu を書いてください。"
                        .into(),
                );
            }
        };
        Ok(Target3270 {
            lu: self.printer_lu.clone(),
            printer: true,
            associate,
            auto_printer: false,
            on_connect: None,
            tn3270e: true,
            terminal_type: None,
            ..self.clone()
        })
    }

    /// `tn3270://[LU@]ホスト:ポート` の形。
    pub(crate) fn uri(&self) -> String {
        let lu = self
            .lu
            .as_deref()
            .map(|l| format!("{l}@"))
            .unwrap_or_default();
        format!("tn3270://{lu}{}:{}", self.host, self.port)
    }

    fn session_config(&self) -> SessionConfig {
        SessionConfig {
            model: self.model,
            ccsid: self.ccsid,
            terminal_type: self.terminal_type.clone(),
            lu: self.lu.clone(),
            tn3270e: self.tn3270e,
            printer: self.printer,
            associate: self.associate.clone(),
        }
    }
}

/// 3270 のタブの状態。
pub(crate) struct Tn3270 {
    pub session: Session,
    pub target: Target3270,
    /// 最後の知らせ（OIA に出す。接続先が断った理由など）
    pub message: String,
    /// 実行中のファイル転送
    pub ft: Option<FtJob>,
    /// プリンター: 受け取った印刷
    pub jobs: Vec<super::printer::StoredJob>,
    /// プリンター: 最後にデータを受け取った時刻（PRINT-EOJ のないホストのジョブの区切り）
    pub last_data: Instant,
    /// プリンター: タブの画面の行数
    pub view_rows: usize,
    /// 通信の記録のファイル（記録中）
    pub trace: Option<TraceFile>,
}

/// 通信の記録のファイル（`logs\tn3270-trace-<日時>-<名前>.log`）。
pub(crate) struct TraceFile {
    pub path: PathBuf,
    w: io::BufWriter<std::fs::File>,
}

/// 記録の時刻（`HH:MM:SS.mmm`）。
fn trace_time() -> String {
    let c = crate::remote::local_clock();
    c.get(11..).unwrap_or(&c).to_owned()
}

impl Tn3270 {
    pub(crate) fn new(target: Target3270) -> Tn3270 {
        Tn3270 {
            session: Session::new(target.session_config()),
            target,
            message: String::new(),
            ft: None,
            jobs: Vec::new(),
            last_data: Instant::now(),
            view_rows: 30,
            trace: None,
        }
    }

    /// 通信の記録を始める（`dir` に新しいファイルを作る）。
    pub(crate) fn start_trace(&mut self, dir: &Path) -> Result<PathBuf, String> {
        use std::io::Write;
        if let Some(t) = &self.trace {
            return Ok(t.path.clone());
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
        let stamp: String = crate::remote::local_clock()
            .chars()
            .filter(char::is_ascii_digit)
            .take(14)
            .collect();
        let name: String = self
            .target
            .label
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let kind = if self.target.printer { "printer-" } else { "" };
        let path = dir.join(format!(
            "tn3270-trace-{}-{}-{kind}{name}.log",
            &stamp[..8.min(stamp.len())],
            stamp.get(8..).unwrap_or("")
        ));
        let f = std::fs::File::create(&path)
            .map_err(|e| format!("{} を作れません: {e}", path.display()))?;
        let mut w = io::BufWriter::new(f);
        let o = self.session.oia();
        let _ = writeln!(
            w,
            "# yyterm 3270 通信の記録  {}  {}:{}{}  {}  CCSID {}  モデル {}  {} {}\n\
             # < はホストから、> はホストへ。非表示のフィールドの中身は ** と ****** に置き換えています。\n",
            crate::remote::local_clock(),
            self.target.host,
            self.target.port,
            self.target
                .ssh
                .as_deref()
                .map(|s| format!("（SSH {s} 経由）"))
                .unwrap_or_default(),
            self.session.config().terminal_type(),
            self.session.ccsid().name(),
            self.target.model,
            o.mode.label(),
            o.device.as_deref().unwrap_or(""),
        );
        let _ = w.flush();
        self.session.set_trace(true);
        self.trace = Some(TraceFile {
            path: path.clone(),
            w,
        });
        Ok(path)
    }

    /// 通信の記録をやめる（ファイルの場所を返す）。
    pub(crate) fn stop_trace(&mut self) -> Option<PathBuf> {
        self.session.set_trace(false);
        let mut t = self.trace.take()?;
        self.trace_note_to(&mut t, "記録を終えました");
        Some(t.path)
    }

    fn trace_note_to(&self, t: &mut TraceFile, text: &str) {
        use std::io::Write;
        let _ = writeln!(t.w, "{} -- {text}", trace_time());
        let _ = t.w.flush();
    }

    /// 記録に知らせを 1 行書く（切断など）。
    pub(crate) fn trace_note(&mut self, text: &str) {
        if let Some(mut t) = self.trace.take() {
            self.trace_note_to(&mut t, text);
            self.trace = Some(t);
        }
    }

    /// セッションの出力の記録を書く。
    pub(crate) fn write_trace(&mut self, entries: &[yy_3270::trace::TraceEntry]) {
        use std::io::Write;
        if entries.is_empty() {
            return;
        }
        if let Some(t) = &mut self.trace {
            let time = trace_time();
            for e in entries {
                let _ = t.w.write_all(e.format(&time).as_bytes());
            }
            let _ = t.w.flush();
        }
    }
}

// ---- ファイル転送（IND$FILE） -------------------------------------------------------

/// 実行中のファイル転送。
pub(crate) struct FtJob {
    pub choice: Choice,
    /// 受け取っている途中のファイル（受け取るとき）
    pub part: Option<PathBuf>,
    pub started: Instant,
}

/// 受け取っている途中のファイルの名前（`名前.yy3270part`）。
fn part_path(local: &Path) -> PathBuf {
    let mut name = local.file_name().unwrap_or_default().to_os_string();
    name.push(".yy3270part");
    local.with_file_name(name)
}

/// 手元のテキストを読む（文字コードは自動判別）。
fn read_local_text(bytes: &[u8]) -> String {
    let d = yy_encoding::detect(bytes, true);
    let (utf8, _) = yy_encoding::decode_all(d.encoding, &bytes[d.bom_len..], false);
    String::from_utf8_lossy(&utf8).into_owned()
}

/// 転送の準備: 端末側のデータ（受け取るなら途中のファイル、送るなら読み口）。
pub(crate) fn prepare(choice: &Choice) -> Result<(Local, Option<PathBuf>), String> {
    let local = &choice.local;
    match choice.request.direction {
        Direction::Receive => {
            if let Some(dir) = local.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
            }
            let part = part_path(local);
            let f = std::fs::File::create(&part)
                .map_err(|e| format!("{} を作れません: {e}", part.display()))?;
            Ok((Local::Sink(Box::new(io::BufWriter::new(f))), Some(part)))
        }
        Direction::Send => {
            let bytes = std::fs::read(local)
                .map_err(|e| format!("{} を読めません: {e}", local.display()))?;
            let data = match choice.request.mode {
                FtMode::Text(ccsid) => ind_file::text_to_records(
                    &read_local_text(&bytes),
                    ccsid,
                    choice.request.record_limit(),
                )
                .map_err(|e| format!("{}: {}", local.display(), e.describe(ccsid)))?,
                FtMode::HostAscii | FtMode::Binary => bytes,
            };
            Ok((Local::Source(Box::new(io::Cursor::new(data))), None))
        }
    }
}

/// 転送の後始末。受け取ったデータを変換して手元のファイルにする。
/// 成功なら手元のファイルの大きさを返す。
pub(crate) fn finish(job: &FtJob, ok: bool) -> Result<u64, String> {
    let local = &job.choice.local;
    let Some(part) = &job.part else {
        return std::fs::metadata(local)
            .map(|m| m.len())
            .map_err(|e| e.to_string());
    };
    if !ok {
        let _ = std::fs::remove_file(part);
        return Ok(0);
    }
    let result = (|| -> io::Result<u64> {
        let mut raw = std::fs::read(part)?;
        let out = match job.choice.request.mode {
            FtMode::Text(ccsid) => ind_file::records_to_text(&raw, ccsid).into_bytes(),
            FtMode::HostAscii => {
                ind_file::strip_ascii_eof(&mut raw);
                raw
            }
            FtMode::Binary => raw,
        };
        std::fs::write(local, &out)?;
        Ok(out.len() as u64)
    })();
    let _ = std::fs::remove_file(part);
    result.map_err(|e| format!("{} に書けません: {e}", local.display()))
}

/// プリンターのタブの画面: 受け取った印刷の一覧（古い順）と、最後の行に状態。
fn render_printer(tn: &Tn3270, exited: bool) -> Terminal {
    let o = tn.session.oia();
    let state = if exited {
        "切断されました".to_owned()
    } else if o.mode == Mode::Negotiating {
        "接続中".to_owned()
    } else if tn.session.printing() {
        "受け取り中".to_owned()
    } else {
        "待機中".to_owned()
    };
    let status = format!(
        " 3287 {:<8} {:<10} {state}  {}件  {}  {}",
        o.mode.label(),
        o.device.as_deref().unwrap_or(""),
        tn.jobs.len(),
        tn.session.ccsid().name(),
        tn.message
    );
    let lines = super::printer::listing(&status, &tn.jobs, 50);
    let cols = lines
        .iter()
        .map(|l| yy_3270::print::text_width(l))
        .max()
        .unwrap_or(0)
        .clamp(80, 240);
    let rows = tn.view_rows.max(5);
    let mut term = Terminal::new(cols, rows, 20000);
    let mut out = String::from("\x1b[?7l\x1b[2J\x1b[H");
    let last = lines.len() - 1;
    for (i, l) in lines.iter().enumerate() {
        if i == last {
            out.push_str(&format!(
                "\x1b[0;38;2;210;210;210;48;2;45;45;60m{l:<cols$}\x1b[0m"
            ));
        } else if l.starts_with("━━") {
            out.push_str(&format!("\x1b[1;96m{l}\x1b[0m\r\n"));
        } else if l.starts_with("──") {
            out.push_str(&format!("\x1b[2m{l}\x1b[0m\r\n"));
        } else {
            out.push_str(l);
            out.push_str("\r\n");
        }
    }
    out.push_str("\x1b[?25l");
    term.feed(out.as_bytes());
    term
}

/// 接続する（`show` に進みを表示する。接続中は呼び出し側の状態を借りないこと）。
pub(crate) fn connect(t: &Target3270, show: &dyn Fn(&str)) -> Result<Backend, String> {
    let (host, port) = (t.host.clone(), t.port);
    if let Some(via) = &t.ssh {
        let target = Target::parse(via)
            .ok_or_else(|| format!("経由する SSH の接続先（{via}）を読めません"))?;
        let tr = crate::remote::transport(&target, show)?;
        show(&format!(
            "{via} 経由で {host}:{port} に接続しています…（Esc で中止）"
        ));
        let h = host.clone();
        let r = crate::remote::wait(show, move |_| tr.direct_tcpip(&h, port));
        show("");
        let p = r.map_err(|e| e.to_string())?;
        let (input, output, finish) = p.into_parts();
        return Ok(Backend {
            input,
            output,
            resize: Box::new(|_, _| {}),
            wait: Box::new(move || {
                let _ = finish();
                None
            }),
            // 入力を閉じると中継を終える（タブを閉じたとき）
            kill: Box::new(|| {}),
        });
    }
    show(&format!("{host}:{port} に接続しています…（Esc で中止）"));
    let h = host.clone();
    let r = crate::remote::wait(show, move |_| -> io::Result<TcpStream> {
        let mut last = None;
        for addr in (h.as_str(), port).to_socket_addrs()? {
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(s) => return Ok(s),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| io::Error::other("アドレスが見つかりません")))
    });
    show("");
    let stream = r.map_err(|e| format!("{host}:{port} に接続できませんでした: {e}"))?;
    let _ = stream.set_nodelay(true);
    let out = stream.try_clone().map_err(|e| e.to_string())?;
    let killer = stream.try_clone().map_err(|e| e.to_string())?;
    Ok(Backend {
        input: Box::new(stream),
        output: Box::new(out),
        resize: Box::new(|_, _| {}),
        wait: Box::new(|| None),
        kill: Box::new(move || {
            let _ = killer.shutdown(Shutdown::Both);
        }),
    })
}

// ---- 描画（端末の画面に写す） --------------------------------------------------------

/// 3279 の色（青・赤・ピンク・緑・トルコ石・黄・白）。
fn rgb(c: u8) -> Option<(u8, u8, u8)> {
    Some(match c {
        0xF1 => (100, 150, 255),
        0xF2 => (255, 85, 85),
        0xF3 => (255, 130, 210),
        0xF4 => (90, 230, 110),
        0xF5 => (70, 225, 215),
        0xF6 => (255, 235, 90),
        0xF7 => (245, 245, 245),
        _ => return None,
    })
}

fn sgr(c: &DisplayCell) -> String {
    let mut s = String::from("\x1b[0");
    let fg = rgb(c.fg).unwrap_or((90, 230, 110));
    s.push_str(&format!(";38;2;{};{};{}", fg.0, fg.1, fg.2));
    if let Some(bg) = rgb(c.bg) {
        s.push_str(&format!(";48;2;{};{};{}", bg.0, bg.1, bg.2));
    }
    if c.intensified {
        s.push_str(";1");
    }
    match c.hl {
        yy_3270::codes::HL_REVERSE => s.push_str(";7"),
        yy_3270::codes::HL_UNDERSCORE => s.push_str(";4"),
        yy_3270::codes::HL_BLINK => s.push_str(";5"),
        _ => {}
    }
    s.push('m');
    s
}

/// OIA（最下行）の文字。
fn oia_text(tn: &Tn3270, exited: bool) -> String {
    let o = tn.session.oia();
    let lock = if exited {
        "切断されました".to_owned()
    } else {
        match &o.lock {
            Lock::None => String::new(),
            Lock::System if o.mode == Mode::Negotiating => "X 接続中".into(),
            Lock::System => "X SYSTEM".into(),
            Lock::Operator(e) => e.label().to_owned(),
        }
    };
    let via = tn
        .target
        .ssh
        .as_deref()
        .map(|s| format!(" SSH:{s}"))
        .unwrap_or_default();
    let mut s = format!(
        " 4B {:<8} {:<10} {:<14} {:<4} {:03}/{:03}  {}{via}",
        o.mode.label(),
        o.device.as_deref().unwrap_or(""),
        lock,
        if o.insert { "INS" } else { "" },
        o.cursor.0,
        o.cursor.1,
        tn.session.ccsid().name(),
    );
    if tn.trace.is_some() {
        s.push_str("  TRACE");
    }
    if !tn.message.is_empty() {
        s.push_str("  ");
        s.push_str(&tn.message);
    }
    s
}

/// 3270 の画面と OIA を端末の画面の形にする（行数は画面＋1）。
pub(crate) fn render(tn: &Tn3270, exited: bool) -> Terminal {
    if tn.target.printer {
        return render_printer(tn, exited);
    }
    let screen = tn.session.screen();
    let (rows, cols) = (screen.rows, screen.cols);
    let mut term = Terminal::new(cols, rows + 1, 0);
    let mut out = String::with_capacity(rows * cols * 12);
    // 折り返さない（最後の桁に書いても次の行に進まない）
    out.push_str("\x1b[?7l\x1b[2J");
    let cells = tn.session.display();
    for r in 0..rows {
        for c in 0..cols {
            let cell = &cells[r * cols + c];
            if cell.width == 0 {
                continue;
            }
            out.push_str(&format!("\x1b[{};{}H", r + 1, c + 1));
            out.push_str(&sgr(cell));
            if cell.hidden || cell.attribute || cell.text.is_empty() {
                out.push(' ');
            } else {
                out.push_str(&cell.text);
            }
        }
    }
    // OIA: 区切りの色で最下行に
    let oia = oia_text(tn, exited);
    out.push_str(&format!(
        "\x1b[{};1H\x1b[0;38;2;210;210;210;48;2;45;45;60m{:<width$}\x1b[0m",
        rows + 1,
        oia,
        width = cols
    ));
    // カーソル（挿入モードは縦線）
    let cur = screen.cursor;
    out.push_str(if tn.session.oia().insert {
        "\x1b[6 q"
    } else {
        "\x1b[2 q"
    });
    out.push_str(&format!("\x1b[{};{}H", cur / cols + 1, cur % cols + 1));
    if exited {
        out.push_str("\x1b[?25l");
    }
    term.feed(out.as_bytes());
    term
}

// ---- キー ----------------------------------------------------------------------

/// キーの組み合わせ（WM_KEYDOWN）を 3270 のキーにする（独自の割り当て。14 章 7）。
pub(crate) fn map_key(vk: VIRTUAL_KEY, m: Mods) -> Option<Key> {
    let k = match (vk, m.ctrl, m.shift, m.alt) {
        (VK_RETURN, false, true, false) => Key::NewLine,
        (VK_RETURN, false, false, false) => Key::Enter,
        (v, false, shift, false) if (VK_F1.0..=VK_F12.0).contains(&v.0) => {
            let n = (v.0 - VK_F1.0 + 1) as u8;
            Key::Pf(if shift { n + 12 } else { n })
        }
        // ISPF の上下（PF7・PF8）
        (VK_PRIOR, false, false, false) => Key::Pf(7),
        (VK_NEXT, false, false, false) => Key::Pf(8),
        (VK_1, false, false, true) => Key::Pa(1),
        (VK_2, false, false, true) => Key::Pa(2),
        (VK_3, false, false, true) => Key::Pa(3),
        (VK_ESCAPE, false, false, false) => Key::Clear,
        (VK_R, true, false, false) => Key::Reset,
        (VK_D, true, false, false) => Key::Dup,
        (VK_M, true, false, false) => Key::FieldMark,
        (VK_END, false, false, false) => Key::EraseEof,
        (VK_END, false, true, false) => Key::EraseInput,
        (VK_INSERT, false, false, false) => Key::Insert,
        (VK_TAB, false, false, false) => Key::Tab,
        (VK_TAB, false, true, false) => Key::BackTab,
        (VK_HOME, false, false, false) => Key::Home,
        (VK_UP, false, false, false) => Key::Up,
        (VK_DOWN, false, false, false) => Key::Down,
        (VK_LEFT, false, false, false) => Key::Left,
        (VK_RIGHT, false, false, false) => Key::Right,
        (VK_BACK, false, false, false) => Key::Backspace,
        (VK_DELETE, false, false, false) => Key::Delete,
        (VK_CANCEL | VK_PAUSE, true, false, false) => Key::Attn,
        (VK_SNAPSHOT, false, false, true) => Key::SysReq,
        _ => return None,
    };
    Some(k)
}

// ---- キーパッド（ホストにしかないキーのボタン） -------------------------------------------

/// キーパッドのボタンの ID の始まり
pub(crate) const ID_KEYPAD: u16 = 3600;

/// キーパッドのボタン（表示名とキー）。
pub(crate) fn keypad_keys() -> Vec<(String, Key)> {
    let mut v: Vec<(String, Key)> = (1..=24).map(|n| (format!("PF{n}"), Key::Pf(n))).collect();
    v.extend([
        ("PA1".into(), Key::Pa(1)),
        ("PA2".into(), Key::Pa(2)),
        ("PA3".into(), Key::Pa(3)),
        ("Clear".into(), Key::Clear),
        ("Reset".into(), Key::Reset),
        ("Attn".into(), Key::Attn),
        ("SysReq".into(), Key::SysReq),
        ("ErEOF".into(), Key::EraseEof),
        ("ErInp".into(), Key::EraseInput),
        ("FldMk".into(), Key::FieldMark),
        ("Dup".into(), Key::Dup),
        ("Enter".into(), Key::Enter),
    ]);
    v
}

/// 列の数
const KEYPAD_COLS: i32 = 3;

/// キーパッド。
pub(crate) struct Keypad {
    pub buttons: Vec<HWND>,
}

impl Keypad {
    pub(crate) fn create(frame: HWND, instance: HINSTANCE, font: HFONT) -> Keypad {
        let mut buttons = Vec::new();
        for (i, (label, _)) in keypad_keys().iter().enumerate() {
            let h = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    windows::core::w!("BUTTON"),
                    &HSTRING::from(label.as_str()),
                    WS_CHILD | WINDOW_STYLE(BS_PUSHBUTTON as u32),
                    0,
                    0,
                    0,
                    0,
                    Some(frame),
                    Some(HMENU((ID_KEYPAD as usize + i) as *mut _)),
                    Some(instance),
                    None,
                )
            };
            if let Ok(h) = h {
                unsafe {
                    SendMessageW(
                        h,
                        WM_SETFONT,
                        Some(WPARAM(font.0 as usize)),
                        Some(LPARAM(1)),
                    );
                }
                buttons.push(h);
            }
        }
        Keypad { buttons }
    }

    /// キーパッドの幅（`dpi` に合わせる）。
    pub(crate) fn width(dpi: i32) -> i32 {
        KEYPAD_COLS * 52 * dpi / 96 + 8 * dpi / 96
    }

    /// ボタンの位置（キーパッドの領域 `area` の中）。
    pub(crate) fn rects(&self, area: RECT, dpi: i32) -> Vec<(HWND, RECT)> {
        let pad = 4 * dpi / 96;
        let w = (area.right - area.left - pad * 2) / KEYPAD_COLS;
        let h = 26 * dpi / 96;
        self.buttons
            .iter()
            .enumerate()
            .map(|(i, &b)| {
                let (row, col) = (i as i32 / KEYPAD_COLS, i as i32 % KEYPAD_COLS);
                let left = area.left + pad + col * w;
                let top = area.top + pad + row * h;
                (
                    b,
                    RECT {
                        left,
                        top,
                        right: left + w - 2,
                        bottom: top + h - 2,
                    },
                )
            })
            .collect()
    }
}

/// キーパッドのボタンの ID のキー。
pub(crate) fn keypad_key(id: u16) -> Option<Key> {
    let i = usize::from(id.checked_sub(ID_KEYPAD)?);
    keypad_keys().get(i).map(|(_, k)| k.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets() {
        let mut cfg = Config::default();
        let t = Target3270::parse("tn3270://TCP00042@mvs01.example.co.jp:2323", &cfg).unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.lu.as_deref()),
            ("mvs01.example.co.jp", 2323, Some("TCP00042"))
        );
        assert_eq!(t.uri(), "tn3270://TCP00042@mvs01.example.co.jp:2323");
        let t = Target3270::parse("mvs01", &cfg).unwrap();
        assert_eq!((t.port, t.lu.clone(), t.ccsid), (23, None, Ccsid::Ibm930));
        assert!(Target3270::parse("", &cfg).is_err());
        assert!(Target3270::parse("mvs01:abc", &cfg).is_err());
        cfg.tn3270.host.insert(
            "prod".into(),
            yy_config::Tn3270Host {
                host: "10.0.0.5".into(),
                port: Some(992),
                lu: Some("LU1".into()),
                ccsid: Some(939),
                ssh: Some("bastion".into()),
                ..Default::default()
            },
        );
        let t = Target3270::parse("PROD", &cfg).unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.ccsid, t.ssh.as_deref()),
            ("10.0.0.5", 992, Ccsid::Ibm939, Some("bastion"))
        );
        // プリンター: 設定の LU がなければ端末の LU に対応づける（ASSOCIATE）
        assert!(t.printer_target(None).is_err());
        let p = t.printer_target(Some("TCP00042")).unwrap();
        assert!(p.printer && !p.auto_printer);
        assert_eq!(
            (p.lu.as_deref(), p.associate.as_deref()),
            (None, Some("TCP00042"))
        );
        assert_eq!(p.session_config().terminal_type(), "IBM-3287-1");
        cfg.tn3270.host.get_mut("prod").unwrap().printer_lu = Some("PRT01".into());
        cfg.tn3270.host.get_mut("prod").unwrap().printer = Some("auto".into());
        let t = Target3270::parse("prod", &cfg).unwrap();
        assert!(t.auto_printer);
        let p = t.printer_target(None).unwrap();
        assert_eq!((p.lu.as_deref(), p.associate), (Some("PRT01"), None));
    }

    #[test]
    fn renders_screen_and_oia() {
        let cfg = Config::default();
        let mut tn = Tn3270::new(Target3270::parse("mvs01", &cfg).unwrap());
        // TN3270 の交渉を済ませて画面を送る
        let mut neg = vec![255, 253, 25, 255, 251, 25, 255, 253, 0, 255, 251, 0];
        neg.extend([0xF5, 0x02, 0x1D, 0x20, 0xC8, 0xC9, 255, 239]);
        tn.session.receive(&neg);
        let term = render(&tn, false);
        assert_eq!((term.cols(), term.rows()), (80, 25));
        let text = term.text(
            yy_term::Pos { line: 0, col: 0 },
            yy_term::Pos { line: 0, col: 80 },
        );
        assert!(text.starts_with(" HI"), "{text:?}");
        let oia = term.text(
            yy_term::Pos { line: 24, col: 0 },
            yy_term::Pos { line: 24, col: 80 },
        );
        assert!(oia.contains("TN3270") && oia.contains("IBM-930"), "{oia}");
        assert_eq!(map_key(VK_F1, Mods::default()), Some(Key::Pf(1)));
        assert_eq!(
            map_key(
                VK_F12,
                Mods {
                    shift: true,
                    ..Mods::default()
                }
            ),
            Some(Key::Pf(24))
        );
        assert_eq!(keypad_key(ID_KEYPAD + 23), Some(Key::Pf(24)));
        assert_eq!(keypad_key(ID_KEYPAD + 1000), None);
    }

    #[test]
    fn prepares_and_finishes_transfers() {
        use std::io::{Read, Write};
        use yy_3270::ind_file::{HostKind, Recfm, Request};
        let dir = std::env::temp_dir().join(format!("yy3270-ft-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut choice = Choice::initial(Ccsid::Ibm930);
        choice.request = Request {
            host: HostKind::Tso,
            direction: Direction::Receive,
            host_file: "'A.B'".into(),
            mode: FtMode::Text(Ccsid::Ibm930),
            recfm: Recfm::Default,
            lrecl: 0,
            space: 0,
            append: false,
        };
        choice.local = dir.join("sub").join("a.txt");
        // 受け取り: 途中のファイルに書き、終わったら変換して置く
        let (local, part) = prepare(&choice).unwrap();
        let part = part.unwrap();
        assert!(part.ends_with("a.txt.yy3270part"));
        let Local::Sink(mut w) = local else { panic!() };
        let mut raw = vec![0x0E];
        for c in "日本".chars() {
            let Some(yy_encoding::EbcdicCode::Double(d)) = Ccsid::Ibm930.encode_char(c) else {
                panic!()
            };
            raw.extend_from_slice(&d.to_be_bytes());
        }
        raw.extend_from_slice(&[0x0F, 0x0D, 0x25]);
        w.write_all(&raw).unwrap();
        drop(w);
        let job = FtJob {
            choice: choice.clone(),
            part: Some(part.clone()),
            started: Instant::now(),
        };
        assert_eq!(finish(&job, true).unwrap(), "日本\r\n".len() as u64);
        assert_eq!(std::fs::read_to_string(&choice.local).unwrap(), "日本\r\n");
        assert!(!part.exists());
        // 失敗なら途中のファイルを消すだけ
        let (_, part) = prepare(&choice).unwrap();
        let job = FtJob { part, ..job };
        assert_eq!(finish(&job, false).unwrap(), 0);
        assert!(!job.part.as_ref().unwrap().exists());
        // 送る: 手元のテキストを EBCDIC のレコードにする
        choice.request.direction = Direction::Send;
        let (local, part) = prepare(&choice).unwrap();
        assert!(part.is_none());
        let Local::Source(mut r) = local else {
            panic!()
        };
        let mut sent = Vec::new();
        r.read_to_end(&mut sent).unwrap();
        assert_eq!(sent, raw);
        // 変換できない文字は行番号つきで断る
        std::fs::write(&choice.local, "ok\n😀\n").unwrap();
        let err = prepare(&choice).err().unwrap();
        assert!(err.contains("2 行目"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_trace_files() {
        let cfg = Config::default();
        let mut tn = Tn3270::new(Target3270::parse("mvs01", &cfg).unwrap());
        let dir = std::env::temp_dir().join(format!("yy3270-trace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = tn.start_trace(&dir).unwrap();
        assert!(tn.session.tracing());
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("tn3270-trace-") && name.ends_with("-mvs01.log"),
            "{name}"
        );
        // TN3270 の交渉と画面
        let neg = [255, 253, 25, 255, 251, 25, 255, 253, 0, 255, 251, 0];
        let o = tn.session.receive(&neg);
        tn.write_trace(&o.trace);
        let o = tn
            .session
            .receive(&[0xF5, 0xC2, 0x1D, 0x60, 0xC1, 255, 239]);
        tn.write_trace(&o.trace);
        tn.trace_note("切断されました");
        assert_eq!(tn.stop_trace(), Some(path.clone()));
        assert!(!tn.session.tracing());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# yyterm 3270 通信の記録"), "{text}");
        assert!(text.contains("< TEL IAC DO EOR"), "{text}");
        assert!(text.contains("> TEL IAC WILL EOR"), "{text}");
        assert!(text.contains("| EraseWrite WCC(reset,restore)"), "{text}");
        assert!(text.contains("-- 切断されました"), "{text}");
        assert!(text.contains("-- 記録を終えました"), "{text}");
        // 記録から同じ画面を作れる
        let mut s2 = Session::new(SessionConfig::default());
        s2.receive(&yy_3270::trace::replay(&text));
        assert_eq!(s2.screen().cells, tn.session.screen().cells);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

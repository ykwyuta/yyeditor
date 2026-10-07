//! 画面なしでマクロを動かす（Hercules などの実機での確かめ・調査用）。
//!
//! ```text
//! cargo run -p yy-3270-macro --example run -- ホスト:ポート マクロ.rhai [出力のフォルダ]
//! ```
//!
//! - `password("名前")` は環境変数 `YY3270_PASSWORD_名前`（英大文字）か `YY3270_PASSWORD` の値を入れる。
//! - `YY3270_TRACE=ファイル` で通信の記録を書く。`YY3270_CCSID`（既定 37）・`YY3270_MODEL`（既定 2）・
//!   `YY3270_TN3270E=0` で TN3270 にする。
//! - `log`・`message`・`print_screen` は標準出力に出す。終わったら最後の画面を出す。
//! - `YY3270_PRINTER=1` でプリンター（3287）として接続し、受け取った印刷を標準出力に出す。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use yy_3270::ind_file::{Direction, Local, Mode};
use yy_3270::{Config, Event, FtEvent, Key, Session};
use yy_3270_macro::{Answer, Host, Op, Snapshot};
use yy_encoding::Ccsid;

struct Transfer {
    local: PathBuf,
    mode: Mode,
    direction: Direction,
    lrecl: u32,
    buf: Arc<Mutex<Vec<u8>>>,
    result: Option<(bool, String)>,
}

struct State {
    session: Session,
    generation: u64,
    connected: bool,
    transfer: Option<Transfer>,
    trace: Option<std::fs::File>,
}

struct TcpHost {
    state: Mutex<State>,
    changed: Condvar,
    out: Mutex<TcpStream>,
}

/// 受け取ったデータを書き込む先（あとで変換する）。
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TcpHost {
    /// 出力を送り、記録を書き、出来事を処理する（状態を借りている間に呼ぶ）。
    fn handle(&self, s: &mut State, o: yy_3270::Output) {
        if !o.send.is_empty() {
            let _ = self.out.lock().unwrap().write_all(&o.send);
        }
        if let Some(f) = s.trace.as_mut() {
            for e in &o.trace {
                let _ = f.write_all(e.format("--:--:--.---").as_bytes());
            }
        }
        for e in o.events {
            match e {
                Event::Transfer(FtEvent::Done { ok, message }) => {
                    if let Some(t) = s.transfer.as_mut() {
                        let mut ok = ok;
                        let mut message = message;
                        if ok && t.direction == Direction::Receive {
                            let lrecl = t.lrecl;
                            let mut raw = t.buf.lock().unwrap().clone();
                            let data = match t.mode {
                                Mode::Text(c) => {
                                    yy_3270::ind_file::records_to_text(&raw, c, lrecl).into_bytes()
                                }
                                Mode::HostAscii => {
                                    yy_3270::ind_file::strip_ascii_eof(&mut raw);
                                    raw
                                }
                                Mode::Binary => raw,
                            };
                            if let Err(e) = std::fs::write(&t.local, data) {
                                ok = false;
                                message = format!("{}: {e}", t.local.display());
                            }
                        }
                        t.result = Some((ok, message));
                    }
                }
                Event::Transfer(FtEvent::Progress(n)) => eprintln!("[転送] {n} バイト"),
                Event::Text(t) => eprintln!("[ホスト] {t}"),
                Event::Mode(m) => eprintln!("[接続] {}", m.label()),
                Event::Device(d) => eprintln!("[LU] {d}"),
                Event::DeviceRejected(r) => eprintln!("[LU を断られた] {r}"),
                Event::Negotiation(n) => eprintln!("[交渉] {n}"),
                Event::PrintJob(job) => {
                    println!("[印刷] {} ページ、{} バイト", job.pages.len(), job.bytes);
                    print!("{}", job.to_text());
                }
                _ => {}
            }
        }
        s.generation += 1;
        self.changed.notify_all();
    }
}

impl Host for TcpHost {
    fn snapshot(&self) -> Result<Snapshot, String> {
        let s = self.state.lock().unwrap();
        Ok(Snapshot::of(&s.session, s.connected))
    }
    fn generation(&self) -> u64 {
        self.state.lock().unwrap().generation
    }
    fn wait_change(&self, since: u64, timeout: Duration) {
        let s = self.state.lock().unwrap();
        let _ = self
            .changed
            .wait_timeout_while(s, timeout, |s| s.generation == since);
    }
    fn act(&self, op: Op) -> Result<Answer, String> {
        let mut s = self.state.lock().unwrap();
        let locked = s.session.oia().lock != yy_3270::Lock::None;
        let key = |s: &mut State, k: Key| {
            let o = s.session.key(k);
            self.handle(s, o);
        };
        match op {
            Op::Key(k) => {
                if locked && !matches!(k, Key::Reset | Key::Attn | Key::SysReq) {
                    return Err("キーボードがロックされています".into());
                }
                key(&mut s, k);
            }
            Op::Type(t) => {
                if locked {
                    return Err("キーボードがロックされています".into());
                }
                for c in t.chars() {
                    key(&mut s, Key::Char(c));
                }
            }
            Op::MoveTo(r, c) => {
                let cols = s.session.screen().cols;
                key(&mut s, Key::MoveTo((r - 1) * cols + c - 1));
            }
            Op::Password(name) => {
                let var = format!("YY3270_PASSWORD_{}", name.to_ascii_uppercase());
                let p = std::env::var(&var)
                    .or_else(|_| std::env::var("YY3270_PASSWORD"))
                    .map_err(|_| format!("{var} か YY3270_PASSWORD を設定してください"))?;
                for c in p.chars() {
                    key(&mut s, Key::Char(c));
                }
            }
            Op::Transfer { request, local } => {
                let buf = Arc::new(Mutex::new(Vec::new()));
                let l = match request.direction {
                    Direction::Receive => Local::Sink(Box::new(Sink(buf.clone()))),
                    Direction::Send => {
                        let bytes = std::fs::read(&local)
                            .map_err(|e| format!("{}: {e}", local.display()))?;
                        let data = match request.mode {
                            Mode::Text(c) => yy_3270::ind_file::encode_upload(
                                &request,
                                &String::from_utf8_lossy(&bytes),
                                c,
                            )
                            .map_err(|e| e.describe(c))?,
                            _ => bytes,
                        };
                        Local::Source(Box::new(std::io::Cursor::new(data)))
                    }
                };
                println!("[転送] {}", request.command());
                s.transfer = Some(Transfer {
                    local,
                    mode: request.mode,
                    direction: request.direction,
                    lrecl: request.fixed_text().unwrap_or(0) as u32,
                    buf,
                    result: None,
                });
                let o = s.session.transfer(&request, l)?;
                self.handle(&mut s, o);
                // 終わるまで待つ
                let deadline = std::time::Instant::now() + Duration::from_secs(600);
                loop {
                    if let Some(r) = s.transfer.as_mut().and_then(|t| t.result.take()) {
                        s.transfer = None;
                        return Ok(Answer::Transfer {
                            ok: r.0,
                            message: r.1,
                        });
                    }
                    if !s.connected || std::time::Instant::now() > deadline {
                        return Err("転送が終わりません".into());
                    }
                    s = self
                        .changed
                        .wait_timeout(s, Duration::from_millis(500))
                        .unwrap()
                        .0;
                }
            }
            Op::PrintScreen => {
                for l in Snapshot::of(&s.session, true).lines() {
                    println!("| {}", l.trim_end());
                }
            }
            Op::Log(t) => println!("[log] {t}"),
            Op::Message(t) => println!("[message] {t}"),
            Op::Ask(q) => {
                println!("[ask] {q}");
                return Ok(Answer::Text(None));
            }
        }
        Ok(Answer::Done)
    }
    fn stopped(&self) -> bool {
        false
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("使い方: run ホスト:ポート マクロ.rhai [出力のフォルダ]");
        std::process::exit(2);
    }
    let script = std::fs::read_to_string(&args[2]).expect("マクロを読めません");
    let out_dir = PathBuf::from(args.get(3).map_or("macros-out", String::as_str));
    let env_num = |k: &str, d: u32| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let ccsid = Ccsid::from_number(env_num("YY3270_CCSID", 37)).expect("CCSID");
    let mut session = Session::new(Config {
        ccsid,
        model: env_num("YY3270_MODEL", 2) as u8,
        tn3270e: std::env::var("YY3270_TN3270E").map_or(true, |v| v != "0"),
        lu: std::env::var("YY3270_LU").ok(),
        printer: std::env::var("YY3270_PRINTER").is_ok_and(|v| v == "1"),
        ..Config::default()
    });
    let trace = std::env::var("YY3270_TRACE")
        .ok()
        .map(|p| std::fs::File::create(p).expect("記録のファイル"));
    session.set_trace(trace.is_some());
    let stream = TcpStream::connect(&args[1]).expect("接続できません");
    let host = Arc::new(TcpHost {
        state: Mutex::new(State {
            session,
            generation: 0,
            connected: true,
            transfer: None,
            trace,
        }),
        changed: Condvar::new(),
        out: Mutex::new(stream.try_clone().unwrap()),
    });
    {
        let host = host.clone();
        let mut r = stream;
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                let n = r.read(&mut buf).unwrap_or(0);
                let mut s = host.state.lock().unwrap();
                if n == 0 {
                    s.connected = false;
                    s.generation += 1;
                    host.changed.notify_all();
                    eprintln!("[切断]");
                    return;
                }
                let o = s.session.receive(&buf[..n]);
                host.handle(&mut s, o);
            }
        });
    }
    let r = yy_3270_macro::run(
        &script,
        host.clone(),
        yy_3270_macro::Options {
            out_dir,
            timeout: 60,
        },
    );
    println!("---- 最後の画面 ----");
    for l in host.snapshot().unwrap().lines() {
        println!("| {}", l.trim_end());
    }
    match r {
        Ok(()) => println!("マクロが終わりました"),
        Err(e) => {
            println!("マクロのエラー: {e}");
            std::process::exit(1);
        }
    }
}

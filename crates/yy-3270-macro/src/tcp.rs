//! 画面なしで 3270 のセッションを動かすホスト（TCP など。実機・模擬ホストでの確かめ・試験用）。
//!
//! 受け取りのスレッドが [`Session`] にバイトを渡し、マクロ（[`crate::run`]）は [`Host`] として
//! 画面を読み・キーを送る。セッションの出来事（LU・TN3270E・印刷のジョブ・転送）はすべて残す。

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use yy_3270::ind_file::{self, Direction, Local, Mode, Request};
use yy_3270::{Config, Event, FtEvent, Key, Lock, Output, PrintJob, Session};

use crate::{Answer, Host, Op, Snapshot};

/// ホストが IND$FILE の転送を始めるまで待つ時間
pub const START_TIMEOUT: Duration = Duration::from_secs(30);

/// パスワードを返す（資格情報の名前から）。
pub type PasswordFn = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

struct Transfer {
    local: PathBuf,
    request: Request,
    buf: Arc<Mutex<Vec<u8>>>,
    result: Option<(bool, String)>,
}

struct State {
    session: Session,
    generation: u64,
    connected: bool,
    transfer: Option<Transfer>,
    events: Vec<Event>,
    logs: Vec<String>,
    trace: Option<Box<dyn Write + Send>>,
}

/// 画面なしの 3270 のセッション。
pub struct TcpHost {
    state: Mutex<State>,
    changed: Condvar,
    out: Mutex<Box<dyn Write + Send>>,
    password: Mutex<Option<PasswordFn>>,
    verbose: AtomicBool,
}

/// 受け取ったデータを書き込む先（あとで変換する）。
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl TcpHost {
    /// TCP で接続する。
    pub fn connect(addr: &str, cfg: Config) -> io::Result<Arc<TcpHost>> {
        let stream = TcpStream::connect(addr)?;
        let _ = stream.set_nodelay(true);
        let w = stream.try_clone()?;
        Ok(TcpHost::start(Box::new(stream), Box::new(w), cfg))
    }

    /// 読み口・書き口から始める（TLS など）。
    pub fn start(
        mut reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        cfg: Config,
    ) -> Arc<TcpHost> {
        let host = Arc::new(TcpHost {
            state: Mutex::new(State {
                session: Session::new(cfg),
                generation: 0,
                connected: true,
                transfer: None,
                events: Vec::new(),
                logs: Vec::new(),
                trace: None,
            }),
            changed: Condvar::new(),
            out: Mutex::new(writer),
            password: Mutex::new(None),
            verbose: AtomicBool::new(false),
        });
        let h = Arc::downgrade(&host);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                let n = reader.read(&mut buf).unwrap_or(0);
                let Some(host) = h.upgrade() else { return };
                let mut s = host.state.lock().unwrap();
                if n == 0 {
                    s.connected = false;
                    s.generation += 1;
                    host.changed.notify_all();
                    return;
                }
                let o = s.session.receive(&buf[..n]);
                host.handle(&mut s, o);
            }
        });
        host
    }

    /// `log`・`message`・出来事を標準出力・標準エラーにも出す。
    pub fn set_verbose(&self, on: bool) {
        self.verbose.store(on, Ordering::Relaxed);
    }

    /// `password("名前")` で入れるパスワード。
    pub fn set_password(&self, f: impl Fn(&str) -> Option<String> + Send + Sync + 'static) {
        *self.password.lock().unwrap() = Some(Box::new(f));
    }

    /// 通信の記録を書く。
    pub fn set_trace(&self, w: Box<dyn Write + Send>) {
        let mut s = self.state.lock().unwrap();
        s.session.set_trace(true);
        s.trace = Some(w);
    }

    /// セッションの出来事（これまでのすべて）。
    pub fn events(&self) -> Vec<Event> {
        self.state.lock().unwrap().events.clone()
    }

    /// `log`・`message` などの記録。
    pub fn logs(&self) -> Vec<String> {
        self.state.lock().unwrap().logs.clone()
    }

    /// 受け取った印刷のジョブ。
    pub fn print_jobs(&self) -> Vec<PrintJob> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                Event::PrintJob(j) => Some(j),
                _ => None,
            })
            .collect()
    }

    /// 条件に合う出来事が来るまで待つ。
    pub fn wait_event(&self, timeout: Duration, pred: impl Fn(&Event) -> bool) -> Option<Event> {
        let deadline = Instant::now() + timeout;
        let mut s = self.state.lock().unwrap();
        loop {
            if let Some(e) = s.events.iter().find(|e| pred(e)) {
                return Some(e.clone());
            }
            let now = Instant::now();
            if now >= deadline || !s.connected {
                return None;
            }
            s = self.changed.wait_timeout(s, deadline - now).unwrap().0;
        }
    }

    /// セッションに触る（画面・OIA を見る）。
    pub fn with_session<R>(&self, f: impl FnOnce(&mut Session) -> R) -> R {
        f(&mut self.state.lock().unwrap().session)
    }

    pub fn connected(&self) -> bool {
        self.state.lock().unwrap().connected
    }

    /// 出力を送り、記録を書き、出来事を処理する（状態を借りている間に呼ぶ）。
    fn handle(&self, s: &mut State, o: Output) {
        if !o.send.is_empty() {
            let _ = self.out.lock().unwrap().write_all(&o.send);
        }
        if let Some(w) = s.trace.as_mut() {
            for e in &o.trace {
                let _ = w.write_all(e.format("--:--:--.---").as_bytes());
            }
        }
        for e in &o.events {
            if let Event::Transfer(FtEvent::Done { ok, message }) = e
                && let Some(t) = s.transfer.as_mut()
            {
                let (ok, message) = finish_transfer(t, *ok, message.clone());
                t.result = Some((ok, message));
            }
            if self.verbose.load(Ordering::Relaxed) {
                match e {
                    Event::Transfer(FtEvent::Progress(_)) => {}
                    e => eprintln!("[event] {e:?}"),
                }
            }
        }
        s.events.extend(o.events);
        s.generation += 1;
        self.changed.notify_all();
    }

    fn log(&self, s: &mut State, text: String) {
        if self.verbose.load(Ordering::Relaxed) {
            println!("{text}");
        }
        s.logs.push(text);
    }
}

/// 受け取った転送を手元のファイルにする。
fn finish_transfer(t: &Transfer, ok: bool, message: String) -> (bool, String) {
    if !ok || t.request.direction != Direction::Receive {
        return (ok, message);
    }
    let mut raw = t.buf.lock().unwrap().clone();
    let data = match t.request.mode {
        Mode::Text(c) => ind_file::decode_download(&t.request, &raw, c).into_bytes(),
        Mode::HostAscii => {
            ind_file::strip_ascii_eof(&mut raw);
            raw
        }
        Mode::Binary => raw,
    };
    if let Some(d) = t.local.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    match std::fs::write(&t.local, data) {
        Ok(()) => (true, message),
        Err(e) => (false, format!("{}: {e}", t.local.display())),
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
            .wait_timeout_while(s, timeout, |s| s.generation == since && s.connected);
    }

    fn act(&self, op: Op) -> Result<Answer, String> {
        let mut s = self.state.lock().unwrap();
        if !s.connected && !matches!(op, Op::Log(_) | Op::Message(_)) {
            return Err("切断されています".into());
        }
        let locked = s.session.oia().lock != Lock::None;
        let key = |s: &mut State, k: Key| {
            let o = s.session.key(k);
            self.handle(s, o);
        };
        let type_text = |s: &mut State, t: &str| -> Result<(), String> {
            for c in t.chars() {
                key(s, Key::Char(c));
                if let Lock::Operator(e) = s.session.oia().lock {
                    key(s, Key::Reset);
                    return Err(format!("入力できません（{}）", e.label()));
                }
            }
            Ok(())
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
                type_text(&mut s, &t)?;
            }
            Op::MoveTo(r, c) => {
                let cols = s.session.screen().cols;
                key(&mut s, Key::MoveTo((r - 1) * cols + c - 1));
            }
            Op::Password(name) => {
                let p = self
                    .password
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|f| f(&name))
                    .ok_or_else(|| format!("資格情報「{name}」がありません"))?;
                type_text(&mut s, &p)?;
            }
            Op::Transfer { request, local } => {
                let buf = Arc::new(Mutex::new(Vec::new()));
                let l = match request.direction {
                    Direction::Receive => Local::Sink(Box::new(Sink(buf.clone()))),
                    Direction::Send => {
                        let bytes = std::fs::read(&local)
                            .map_err(|e| format!("{}: {e}", local.display()))?;
                        let data = match request.mode {
                            Mode::Text(c) => ind_file::encode_upload(
                                &request,
                                &String::from_utf8_lossy(&bytes),
                                c,
                            )
                            .map_err(|e| e.describe(c))?,
                            _ => bytes,
                        };
                        Local::Source(Box::new(io::Cursor::new(data)))
                    }
                };
                let command = request.command();
                self.log(&mut s, format!("[転送] {command}"));
                s.transfer = Some(Transfer {
                    local,
                    request: request.clone(),
                    buf,
                    result: None,
                });
                let o = s.session.transfer(&request, l)?;
                self.handle(&mut s, o);
                let deadline = Instant::now() + Duration::from_secs(600);
                loop {
                    if let Some(r) = s.transfer.as_mut().and_then(|t| t.result.take()) {
                        s.transfer = None;
                        return Ok(Answer::Transfer {
                            ok: r.0,
                            message: r.1,
                        });
                    }
                    if !s.connected || Instant::now() > deadline {
                        s.transfer = None;
                        return Err("転送が終わりません".into());
                    }
                    // ホストが転送を始めない（IND$FILE がないなど）
                    let ev = s.session.check_transfer_start(START_TIMEOUT);
                    if let Some(Event::Transfer(FtEvent::Done { message, .. })) = ev.last() {
                        let message = message.clone();
                        s.events.extend(ev);
                        s.transfer = None;
                        return Ok(Answer::Transfer { ok: false, message });
                    }
                    s = self
                        .changed
                        .wait_timeout(s, Duration::from_millis(200))
                        .unwrap()
                        .0;
                }
            }
            Op::PrintScreen => {
                let lines = Snapshot::of(&s.session, true).lines();
                for l in lines {
                    let l = format!("| {}", l.trim_end());
                    self.log(&mut s, l);
                }
            }
            Op::Log(t) => self.log(&mut s, format!("[log] {t}")),
            Op::Message(t) => self.log(&mut s, format!("[message] {t}")),
            Op::Ask(q) => {
                self.log(&mut s, format!("[ask] {q}"));
                return Ok(Answer::Text(None));
            }
        }
        Ok(Answer::Done)
    }

    fn stopped(&self) -> bool {
        false
    }
}

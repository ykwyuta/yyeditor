//! 3270 の模擬ホスト（TN3270E のサーバー）。yyterm の 3270 の試験用。
//!
//! 実機（z/OS）や Hercules では試せない機能を、決まった動きで確かめるためのホスト:
//!
//! - TN3270E: DEVICE-TYPE の CONNECT（LU 名の指定）・ASSOCIATE（端末に対応するプリンター）・拒否の理由、
//!   FUNCTIONS（RESPONSES・SCS-CTL-CODES・DATA-STREAM-CTL）、応答（ALWAYS-RESPONSE と肯定の応答）。
//!   TN3270E を断られたら TN3270（TERMINAL-TYPE・BINARY・EOR）。
//! - 端末: ログオンの画面（日本語の見出し、利用者 ID、非表示のパスワード、DBCS のフィールド、混在の
//!   フィールド、拡張属性）、READY のコマンド（`QUERY`・`NIHONGO`・`IND$FILE`・`PRINT`・`LOGOFF`）。
//! - Query: Read Partition Query を送り、端末の Query Reply を読んで記録する（文字セットの CGCSGID など）。
//! - IND$FILE（DFT）: ホスト側。z/OS 風（`CRLF` で区切る）と MVS 3.8j 風（`CRLF` を無視し、メッセージの
//!   段階を閉じない）を選べる。
//! - プリンター: 端末の `PRINT` で、対応するプリンターの LU に SCS（日本語）または LU3 の印刷を送り、
//!   PRINT-EOJ で終える。
//! - TLS: 暗黙の TLS か Telnet の STARTTLS。クライアント証明書を求めることもできる（[`tls`]）。
//!
//! 出来事は [`MockHost::events`] に文字列で残る（試験で確かめる）。同じホストを x3270（s3270・pr3287）
//! でも確かめられる（`check-with-x3270.sh`）。

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use yy_encoding::Ccsid;

pub mod codes;
mod dft;
pub mod ebcdic;
mod printer;
mod terminal;
pub mod tls;

use codes::*;

/// IND$FILE の振る舞い。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndFileStyle {
    /// z/OS 風: `CRLF` でレコードを区切る（`ASCII` なしは EBCDIC の CR LF）。メッセージの段階を閉じる
    Zos,
    /// MVS 3.8j の IND$FILE 2.0.5 風: `ASCII` なしの `CRLF` を無視する。メッセージの段階を閉じない
    Mvs38,
}

/// 模擬ホストの設定。
#[derive(Clone, Debug)]
pub struct MockConfig {
    /// TN3270E を申し出る
    pub tn3270e: bool,
    /// 端末の LU
    pub terminal_lus: Vec<String>,
    /// プリンターの LU（同じ位置の端末の LU に対応する）
    pub printer_lus: Vec<String>,
    pub ccsid: Ccsid,
    pub password: String,
    pub ind_file: IndFileStyle,
    /// ホストが送る 3270 のデータに ALWAYS-RESPONSE を付ける（RESPONSES を合意したとき）
    pub responses: bool,
    /// 出来事を標準エラーにも出す
    pub verbose: bool,
    /// TLS（なければ平文）
    pub tls: Option<tls::MockTls>,
}

impl Default for MockConfig {
    fn default() -> Self {
        MockConfig {
            tn3270e: true,
            terminal_lus: (1..=4).map(|n| format!("TCP{n:05}")).collect(),
            printer_lus: (1..=4).map(|n| format!("PRT{n:05}")).collect(),
            ccsid: Ccsid::Ibm930,
            password: "SECRET".into(),
            ind_file: IndFileStyle::Zos,
            responses: true,
            verbose: false,
            tls: None,
        }
    }
}

/// データセット（EBCDIC のレコード）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dataset {
    /// `F` か `V`
    pub recfm: char,
    pub lrecl: usize,
    pub records: Vec<Vec<u8>>,
}

/// 印刷のジョブ。
#[derive(Clone, Debug)]
pub(crate) enum Job {
    /// SCS のデータ
    Scs(Vec<u8>),
    /// LU3 の 3270 データストリーム（Write から）
    Lu3(Vec<u8>),
}

/// ホストの状態（接続のスレッドで共有する）。
pub(crate) struct Shared {
    pub cfg: MockConfig,
    tls: Option<(tls::TlsMode, Arc<rustls::ServerConfig>)>,
    events: Mutex<Vec<String>>,
    changed: Condvar,
    lus: Mutex<LuState>,
    pub datasets: Mutex<BTreeMap<String, Dataset>>,
}

#[derive(Default)]
struct LuState {
    in_use: HashSet<String>,
    /// 接続しているプリンターへの送り口
    printers: HashMap<String, Sender<Job>>,
}

impl Shared {
    pub fn log(&self, e: impl Into<String>) {
        let e = e.into();
        if self.cfg.verbose || std::env::var_os("YY3270_MOCK_VERBOSE").is_some() {
            eprintln!("[mock] {e}");
        }
        self.events.lock().unwrap().push(e);
        self.changed.notify_all();
    }

    /// 端末の LU に対応するプリンターの LU。
    pub fn printer_for(&self, terminal: &str) -> Option<String> {
        let i = self.cfg.terminal_lus.iter().position(|l| l == terminal)?;
        self.cfg.printer_lus.get(i).cloned()
    }

    /// プリンターにジョブを送る（つながっていなければ `false`）。
    pub fn print(&self, printer: &str, job: Job) -> bool {
        let lus = self.lus.lock().unwrap();
        lus.printers
            .get(printer)
            .is_some_and(|tx| tx.send(job).is_ok())
    }
}

/// 起動した模擬ホスト。
pub struct MockHost {
    addr: SocketAddr,
    shared: Arc<Shared>,
}

impl MockHost {
    /// `127.0.0.1` の空いたポートで始める。
    pub fn start(cfg: MockConfig) -> io::Result<MockHost> {
        MockHost::bind("127.0.0.1:0", cfg)
    }

    pub fn bind(addr: &str, cfg: MockConfig) -> io::Result<MockHost> {
        let listener = TcpListener::bind(addr)?;
        let addr = listener.local_addr()?;
        let tls = match &cfg.tls {
            Some(t) => Some((t.mode, tls::server_config(t)?)),
            None => None,
        };
        let shared = Arc::new(Shared {
            tls,
            datasets: Mutex::new(dft::initial_datasets(cfg.ccsid)),
            cfg,
            events: Mutex::new(Vec::new()),
            changed: Condvar::new(),
            lus: Mutex::new(LuState::default()),
        });
        let s = shared.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let s = s.clone();
                std::thread::spawn(move || {
                    let peer = stream
                        .peer_addr()
                        .map(|a| a.to_string())
                        .unwrap_or_default();
                    let _ = stream.set_nodelay(true);
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
                    let conn = match &s.tls {
                        None => Conn::new(Box::new(stream)),
                        Some((mode, cfg)) => match tls::accept(Box::new(stream), *mode, cfg, &s) {
                            Ok(c) => c,
                            Err(e) => {
                                s.log(format!("tls failed {peer}: {e}"));
                                return;
                            }
                        },
                    };
                    if let Err(e) = serve(conn, &s) {
                        s.log(format!("disconnect {peer}: {e}"));
                    }
                });
            }
        });
        Ok(MockHost { addr, shared })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// これまでの出来事。
    pub fn events(&self) -> Vec<String> {
        self.shared.events.lock().unwrap().clone()
    }

    /// 条件に合う出来事が来るまで待つ。
    pub fn wait_event(&self, timeout: Duration, pred: impl Fn(&str) -> bool) -> Option<String> {
        let deadline = Instant::now() + timeout;
        let mut ev = self.shared.events.lock().unwrap();
        loop {
            if let Some(e) = ev.iter().find(|e| pred(e)) {
                return Some(e.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            ev = self
                .shared
                .changed
                .wait_timeout(ev, deadline - now)
                .unwrap()
                .0;
        }
    }

    pub fn dataset(&self, name: &str) -> Option<Dataset> {
        self.shared.datasets.lock().unwrap().get(name).cloned()
    }

    pub fn put_dataset(&self, name: &str, ds: Dataset) {
        self.shared
            .datasets
            .lock()
            .unwrap()
            .insert(name.to_owned(), ds);
    }
}

// ---- Telnet の読み書き -------------------------------------------------------------

/// 読み書きできる接続（TCP、TLS）。
pub trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

/// Telnet の単位。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unit {
    Cmd(u8, u8),
    Sb(Vec<u8>),
    Rec(Vec<u8>),
}

pub(crate) struct Conn {
    s: Box<dyn Stream>,
    rx: Rx,
    rec: Vec<u8>,
    sb: Vec<u8>,
    units: VecDeque<Unit>,
    closed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rx {
    Data,
    Iac,
    Cmd(u8),
    Sb,
    SbIac,
}

impl Conn {
    pub fn new(s: Box<dyn Stream>) -> Conn {
        Conn {
            s,
            rx: Rx::Data,
            rec: Vec::new(),
            sb: Vec::new(),
            units: VecDeque::new(),
            closed: false,
        }
    }

    /// 下の接続（STARTTLS で TLS に移るとき。読み残しがないこと）。
    pub fn into_inner(self) -> Box<dyn Stream> {
        debug_assert!(self.units.is_empty() && self.rec.is_empty());
        self.s
    }

    /// 次の単位（`timeout` までに来なければ `None`）。切れたらエラー。
    pub fn next(&mut self, timeout: Duration) -> io::Result<Option<Unit>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(u) = self.units.pop_front() {
                return Ok(Some(u));
            }
            if self.closed {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "切断されました",
                ));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            let mut buf = [0u8; 4096];
            match self.s.read(&mut buf) {
                Ok(0) => self.closed = true,
                Ok(n) => self.feed(&buf[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e),
            }
        }
    }

    fn feed(&mut self, data: &[u8]) {
        for &b in data {
            self.rx = match self.rx {
                Rx::Data if b == IAC => Rx::Iac,
                Rx::Data => {
                    self.rec.push(b);
                    Rx::Data
                }
                Rx::Iac => match b {
                    IAC => {
                        self.rec.push(IAC);
                        Rx::Data
                    }
                    EOR => {
                        self.units
                            .push_back(Unit::Rec(std::mem::take(&mut self.rec)));
                        Rx::Data
                    }
                    SB => {
                        self.sb.clear();
                        Rx::Sb
                    }
                    DO | DONT | WILL | WONT => Rx::Cmd(b),
                    _ => Rx::Data,
                },
                Rx::Cmd(c) => {
                    self.units.push_back(Unit::Cmd(c, b));
                    Rx::Data
                }
                Rx::Sb if b == IAC => Rx::SbIac,
                Rx::Sb => {
                    self.sb.push(b);
                    Rx::Sb
                }
                Rx::SbIac if b == SE => {
                    self.units.push_back(Unit::Sb(std::mem::take(&mut self.sb)));
                    Rx::Data
                }
                Rx::SbIac => {
                    self.sb.push(b);
                    Rx::Sb
                }
            };
        }
    }

    pub fn send_raw(&mut self, b: &[u8]) -> io::Result<()> {
        self.s.write_all(b)?;
        self.s.flush()
    }

    pub fn send_cmd(&mut self, cmd: u8, opt: u8) -> io::Result<()> {
        self.send_raw(&[IAC, cmd, opt])
    }

    pub fn send_sb(&mut self, body: &[u8]) -> io::Result<()> {
        let mut v = vec![IAC, SB];
        for &b in body {
            v.push(b);
            if b == IAC {
                v.push(IAC);
            }
        }
        v.extend_from_slice(&[IAC, SE]);
        self.send_raw(&v)
    }

    pub fn send_rec(&mut self, data: &[u8]) -> io::Result<()> {
        let mut v = Vec::with_capacity(data.len() + 4);
        for &b in data {
            v.push(b);
            if b == IAC {
                v.push(IAC);
            }
        }
        v.extend_from_slice(&[IAC, EOR]);
        self.send_raw(&v)
    }
}

// ---- 交渉 ---------------------------------------------------------------------------

/// 交渉の結果。
pub(crate) struct Link {
    pub conn: Conn,
    pub tn3270e: bool,
    pub lu: Option<String>,
    pub device_type: String,
    pub functions: Vec<u8>,
    seq: u16,
    /// 送った ALWAYS-RESPONSE のうち、まだ応答のない番号
    pub awaiting: Vec<u16>,
}

impl Link {
    pub fn printer(&self) -> bool {
        self.device_type.starts_with("IBM-3287")
    }

    pub fn responses(&self) -> bool {
        self.tn3270e && self.functions.contains(&FN_RESPONSES)
    }

    /// 3270 のデータなどを送る（TN3270E ならヘッダーをつけ、合意していれば応答を求める）。
    pub fn send(&mut self, shared: &Shared, data_type: u8, data: &[u8]) -> io::Result<()> {
        if !self.tn3270e {
            return self.conn.send_rec(data);
        }
        self.seq = self.seq.wrapping_add(1);
        let want = shared.cfg.responses
            && self.responses()
            && matches!(data_type, DT_3270_DATA | DT_SCS_DATA);
        let mut rec = vec![
            data_type,
            0,
            if want {
                RSP_ALWAYS_RESPONSE
            } else {
                RSP_NO_RESPONSE
            },
        ];
        rec.extend_from_slice(&self.seq.to_be_bytes());
        rec.extend_from_slice(data);
        if want {
            self.awaiting.push(self.seq);
        }
        self.conn.send_rec(&rec)
    }

    /// 端末からのレコードを受け取る（応答は記録して読み飛ばす）。`timeout` までに来なければ `None`。
    pub fn recv(&mut self, shared: &Shared, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(u) = self.conn.next(left)? else {
                return Ok(None);
            };
            match u {
                Unit::Rec(r) if self.tn3270e => {
                    if r.len() < 5 {
                        continue;
                    }
                    let seq = u16::from_be_bytes([r[3], r[4]]);
                    match r[0] {
                        DT_RESPONSE => {
                            let kind = if r.get(2) == Some(&RSP_POSITIVE) {
                                "positive"
                            } else {
                                "negative"
                            };
                            self.awaiting.retain(|&s| s != seq);
                            shared.log(format!(
                                "response {kind} seq={seq} lu={}",
                                self.lu.as_deref().unwrap_or("")
                            ));
                        }
                        DT_3270_DATA => return Ok(Some(r[5..].to_vec())),
                        t => shared.log(format!("ignored data type {t}")),
                    }
                }
                Unit::Rec(r) => return Ok(Some(r)),
                Unit::Cmd(c, o) => shared.log(format!("telnet cmd {c} {o} after negotiation")),
                Unit::Sb(_) => {}
            }
        }
    }
}

fn ascii(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// LU を割り当てる（使用中なら `None`）。
fn take_lu(shared: &Shared, lu: &str) -> bool {
    shared.lus.lock().unwrap().in_use.insert(lu.to_owned())
}

fn release_lu(shared: &Shared, lu: &str) {
    let mut s = shared.lus.lock().unwrap();
    s.in_use.remove(lu);
    s.printers.remove(lu);
}

const NEG: Duration = Duration::from_secs(10);

/// TN3270E の DEVICE-TYPE REQUEST に答える。成功なら（端末の種類、LU）。
fn device_request(
    conn: &mut Conn,
    shared: &Shared,
    body: &[u8],
) -> io::Result<Option<(String, String)>> {
    // [TN3270E, DEVICE-TYPE, REQUEST, 種類…, (CONNECT|ASSOCIATE 名前)]
    let rest = &body[3..];
    let split = rest
        .iter()
        .position(|&b| b == E_CONNECT || b == E_ASSOCIATE);
    let (ty, how, name) = match split {
        Some(i) => (
            ascii(&rest[..i]),
            Some(rest[i]),
            Some(ascii(&rest[i + 1..])),
        ),
        None => (ascii(rest), None, None),
    };
    let printer = ty.starts_with("IBM-3287");
    let reject = |conn: &mut Conn, reason: u8, why: &str| -> io::Result<Option<(String, String)>> {
        shared.log(format!("device rejected {why} type={ty}"));
        conn.send_sb(&[OPT_TN3270E, E_DEVICE_TYPE, E_REJECT, E_REASON, reason])?;
        Ok(None)
    };
    if !(printer || ty.starts_with("IBM-3278") || ty.starts_with("IBM-3279") || ty == "IBM-DYNAMIC")
    {
        return reject(conn, REASON_INV_DEVICE_TYPE, "INV-DEVICE-TYPE");
    }
    let lu = match (how, name) {
        (Some(E_CONNECT), Some(n)) => {
            let known_t = shared.cfg.terminal_lus.contains(&n);
            let known_p = shared.cfg.printer_lus.contains(&n);
            if !known_t && !known_p {
                return reject(conn, REASON_INV_NAME, "INV-NAME");
            }
            if known_t == printer {
                return reject(conn, REASON_TYPE_NAME_ERROR, "TYPE-NAME-ERROR");
            }
            if !take_lu(shared, &n) {
                return reject(conn, REASON_DEVICE_IN_USE, "DEVICE-IN-USE");
            }
            n
        }
        (Some(E_ASSOCIATE), Some(term)) => {
            if !printer {
                return reject(conn, REASON_INV_ASSOCIATE, "INV-ASSOCIATE");
            }
            let Some(p) = shared.printer_for(&term) else {
                return reject(conn, REASON_INV_ASSOCIATE, "INV-ASSOCIATE");
            };
            if !shared.lus.lock().unwrap().in_use.contains(&term) {
                return reject(
                    conn,
                    REASON_INV_ASSOCIATE,
                    "INV-ASSOCIATE (terminal not connected)",
                );
            }
            if !take_lu(shared, &p) {
                return reject(conn, REASON_DEVICE_IN_USE, "DEVICE-IN-USE");
            }
            p
        }
        _ => {
            let pool = if printer {
                &shared.cfg.printer_lus
            } else {
                &shared.cfg.terminal_lus
            };
            let Some(n) = pool.iter().find(|l| take_lu(shared, l)).cloned() else {
                return reject(conn, REASON_DEVICE_IN_USE, "no free LU");
            };
            n
        }
    };
    let mut is = vec![OPT_TN3270E, E_DEVICE_TYPE, E_IS];
    is.extend_from_slice(ty.as_bytes());
    is.push(E_CONNECT);
    is.extend_from_slice(lu.as_bytes());
    conn.send_sb(&is)?;
    let how = match how {
        Some(E_ASSOCIATE) => "associate",
        Some(_) => "connect",
        None => "pool",
    };
    shared.log(format!("device {ty} lu={lu} ({how})"));
    Ok(Some((ty, lu)))
}

/// 交渉する（TN3270E、だめなら TN3270）。
fn negotiate(mut conn: Conn, shared: &Shared) -> io::Result<Link> {
    let mut tn3270e = false;
    if shared.cfg.tn3270e {
        conn.send_cmd(DO, OPT_TN3270E)?;
        let mut device: Option<(String, String)> = None;
        loop {
            let Some(u) = conn.next(NEG)? else {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "交渉が終わりません",
                ));
            };
            match u {
                Unit::Cmd(WILL, OPT_TN3270E) => {
                    tn3270e = true;
                    conn.send_sb(&[OPT_TN3270E, E_SEND, E_DEVICE_TYPE])?;
                }
                Unit::Cmd(WONT, OPT_TN3270E) => {
                    shared.log("client refused TN3270E");
                    break;
                }
                Unit::Sb(b) if b.starts_with(&[OPT_TN3270E, E_DEVICE_TYPE, E_REQUEST]) => {
                    device = device_request(&mut conn, shared, &b)?;
                    // 断ったら、端末は別の要求をするか TN3270E をやめる
                }
                Unit::Sb(b) if b.starts_with(&[OPT_TN3270E, E_FUNCTIONS, E_REQUEST]) => {
                    let asked = &b[3..];
                    let ours: Vec<u8> = asked
                        .iter()
                        .copied()
                        .filter(|f| {
                            matches!(*f, FN_RESPONSES | FN_SCS_CTL_CODES | FN_DATA_STREAM_CTL)
                        })
                        .collect();
                    if ours.len() == asked.len() {
                        let mut is = vec![OPT_TN3270E, E_FUNCTIONS, E_IS];
                        is.extend_from_slice(&ours);
                        conn.send_sb(&is)?;
                        let Some((ty, lu)) = device else {
                            return Err(io::Error::other(
                                "FUNCTIONS の前に DEVICE-TYPE がありません",
                            ));
                        };
                        shared.log(format!("functions {ours:?} lu={lu}"));
                        return Ok(Link {
                            conn,
                            tn3270e: true,
                            lu: Some(lu),
                            device_type: ty,
                            functions: ours,
                            seq: 0,
                            awaiting: Vec::new(),
                        });
                    }
                    // 使えるものだけを提案し直す
                    let mut req = vec![OPT_TN3270E, E_FUNCTIONS, E_REQUEST];
                    req.extend_from_slice(&ours);
                    conn.send_sb(&req)?;
                }
                Unit::Sb(b) if b.starts_with(&[OPT_TN3270E, E_FUNCTIONS, E_IS]) => {
                    let Some((ty, lu)) = device else {
                        return Err(io::Error::other(
                            "FUNCTIONS の前に DEVICE-TYPE がありません",
                        ));
                    };
                    let f = b[3..].to_vec();
                    shared.log(format!("functions {f:?} lu={lu}"));
                    return Ok(Link {
                        conn,
                        tn3270e: true,
                        lu: Some(lu),
                        device_type: ty,
                        functions: f,
                        seq: 0,
                        awaiting: Vec::new(),
                    });
                }
                Unit::Cmd(DONT, OPT_TN3270E) => break,
                _ => {}
            }
        }
        if tn3270e {
            if let Some((_, lu)) = device.take() {
                release_lu(shared, &lu);
            }
            conn.send_cmd(DONT, OPT_TN3270E)?;
        }
    }
    // TN3270
    conn.send_cmd(DO, OPT_TTYPE)?;
    let mut ty = String::new();
    let (mut eor, mut bin) = (false, false);
    loop {
        let Some(u) = conn.next(NEG)? else {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "交渉が終わりません",
            ));
        };
        match u {
            Unit::Cmd(WILL, OPT_TTYPE) => conn.send_sb(&[OPT_TTYPE, TTYPE_SEND])?,
            Unit::Sb(b) if b.starts_with(&[OPT_TTYPE, TTYPE_IS]) => {
                ty = ascii(&b[2..]);
                conn.send_cmd(DO, OPT_EOR)?;
                conn.send_cmd(WILL, OPT_EOR)?;
                conn.send_cmd(DO, OPT_BINARY)?;
                conn.send_cmd(WILL, OPT_BINARY)?;
            }
            Unit::Cmd(WILL, OPT_EOR) => eor = true,
            Unit::Cmd(WILL, OPT_BINARY) => bin = true,
            _ => {}
        }
        if eor && bin && !ty.is_empty() {
            break;
        }
    }
    shared.log(format!("tn3270 type={ty}"));
    Ok(Link {
        conn,
        tn3270e: false,
        lu: None,
        device_type: ty,
        functions: Vec::new(),
        seq: 0,
        awaiting: Vec::new(),
    })
}

/// 1 つの接続を扱う。
fn serve(conn: Conn, shared: &Arc<Shared>) -> io::Result<()> {
    let mut link = negotiate(conn, shared)?;
    let lu = link.lu.clone();
    let r = if link.printer() {
        let (tx, rx): (Sender<Job>, Receiver<Job>) = channel();
        if let Some(l) = &lu {
            shared.lus.lock().unwrap().printers.insert(l.clone(), tx);
        }
        printer::run(&mut link, shared, rx)
    } else {
        terminal::run(&mut link, shared)
    };
    if let Some(l) = &lu {
        release_lu(shared, l);
    }
    r
}

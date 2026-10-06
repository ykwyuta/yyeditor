//! 3270 端末エミュレーションの中核（TN3270・TN3270E。14 章）。OS に依存しない。
//!
//! [`Session`] は「受け取ったバイト列で画面を更新し、送るバイト列を返す」状態機械で、ソケットは
//! 持たない。Telnet のオプションの交渉（TERMINAL-TYPE・BINARY・EOR・TN3270E）、TN3270E の
//! デバイスの交渉とヘッダー・応答、3270 データストリーム（[`emu`]）、キー操作を扱う。

pub mod codes;
pub mod emu;
pub mod ind_file;
pub mod print;
pub mod query;
pub mod screen;

use yy_encoding::Ccsid;

use codes::*;
pub use emu::{Key, Lock, OperatorError};
pub use ind_file::FtEvent;
pub use print::PrintJob;
pub use screen::{DisplayCell, Screen};

/// 接続の設定。
#[derive(Clone, Debug)]
pub struct Config {
    /// モデル（2: 24×80、3: 32×80、4: 43×80、5: 27×132）
    pub model: u8,
    pub ccsid: Ccsid,
    /// 端末の種類（`None` なら `IBM-3278-<model>-E`）
    pub terminal_type: Option<String>,
    /// TN3270E で求める LU 名
    pub lu: Option<String>,
    /// TN3270E を使う（断られたら TN3270）
    pub tn3270e: bool,
    /// プリンター（3287）のセッション
    pub printer: bool,
    /// プリンターで、LU 名の代わりに対応づける端末の LU（ASSOCIATE）
    pub associate: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            model: 2,
            ccsid: Ccsid::Ibm037,
            terminal_type: None,
            lu: None,
            tn3270e: true,
            printer: false,
            associate: None,
        }
    }
}

impl Config {
    pub fn terminal_type(&self) -> String {
        self.terminal_type
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| {
                if self.printer {
                    "IBM-3287-1".to_owned()
                } else {
                    format!("IBM-3278-{}-E", self.model.clamp(2, 5))
                }
            })
    }
}

/// 接続の段階。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 交渉中（まだ 3270 のデータを受け取れない）
    Negotiating,
    /// TN3270（RFC 1576）
    Tn3270,
    /// TN3270E（RFC 2355）
    Tn3270e,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Negotiating => "交渉中",
            Mode::Tn3270 => "TN3270",
            Mode::Tn3270e => "TN3270E",
        }
    }
}

/// 処理の中の出来事（記録・表示用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// 接続の段階が変わった
    Mode(Mode),
    /// TN3270E で割り当てられたデバイス（LU）
    Device(String),
    /// TN3270E のデバイスの要求を断られた（理由）
    DeviceRejected(String),
    /// 交渉の 1 段階（接続の記録用）
    Negotiation(String),
    /// 警報音（WCC）
    Alarm,
    /// 3270 になる前に届いた文字（NVT。サーバーのメッセージなど）
    Text(String),
    /// IND$FILE の転送の進み具合・結果
    Transfer(FtEvent),
    /// プリンターの印刷のジョブが終わった（PRINT-EOJ）
    PrintJob(PrintJob),
}

/// 処理の結果。
#[derive(Debug, Default)]
pub struct Output {
    /// ホストへ送るバイト列（Telnet の枠つき）
    pub send: Vec<u8>,
    /// 画面・状態が変わった（描き直す）
    pub changed: bool,
    pub events: Vec<Event>,
}

/// OIA（画面の最下行の状態表示）の内容。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Oia {
    pub mode: Mode,
    pub lock: Lock,
    pub insert: bool,
    /// カーソルの位置（行, 桁。1 から）
    pub cursor: (usize, usize),
    pub device: Option<String>,
}

/// Telnet の受け取りの状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rx {
    Data,
    Iac,
    Opt(u8),
    Sb,
    SbIac,
}

/// TN3270・TN3270E のセッション。
pub struct Session {
    cfg: Config,
    pub emu: emu::Emulator,
    rx: Rx,
    record: Vec<u8>,
    sb: Vec<u8>,
    /// 交渉の状態（相手に WILL/DO を送った・受けた）
    will_binary: bool,
    do_binary: bool,
    will_eor: bool,
    do_eor: bool,
    will_ttype: bool,
    tn3270e: bool,
    functions: Vec<u8>,
    mode: Mode,
    device: Option<String>,
    nvt: Vec<u8>,
    /// プリンターの組み立て中のジョブ
    scs: print::Scs,
    lu3: Vec<print::Page>,
    lu3_bytes: usize,
}

impl Session {
    pub fn new(cfg: Config) -> Session {
        let emu = emu::Emulator::new(cfg.model, cfg.ccsid);
        let cfg_ccsid = cfg.ccsid;
        Session {
            cfg,
            emu,
            rx: Rx::Data,
            record: Vec::new(),
            sb: Vec::new(),
            will_binary: false,
            do_binary: false,
            will_eor: false,
            do_eor: false,
            will_ttype: false,
            tn3270e: false,
            functions: Vec::new(),
            mode: Mode::Negotiating,
            device: None,
            nvt: Vec::new(),
            scs: print::Scs::new(cfg_ccsid),
            lu3: Vec::new(),
            lu3_bytes: 0,
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn screen(&self) -> &Screen {
        &self.emu.screen
    }

    pub fn ccsid(&self) -> Ccsid {
        self.emu.ccsid
    }

    /// TN3270E で使っている関数（BIND-IMAGE・RESPONSES など）。
    pub fn functions(&self) -> &[u8] {
        &self.functions
    }

    pub fn oia(&self) -> Oia {
        let s = &self.emu.screen;
        Oia {
            mode: self.mode,
            lock: self.emu.lock.clone(),
            insert: self.emu.insert,
            cursor: (s.cursor / s.cols + 1, s.cursor % s.cols + 1),
            device: self.device.clone(),
        }
    }

    /// 画面の表示用のセル。
    pub fn display(&self) -> Vec<DisplayCell> {
        self.emu.screen.display(self.emu.ccsid)
    }

    // ---- 受け取り ----------------------------------------------------------------------

    /// ホストから受け取ったバイト列を処理する。
    pub fn receive(&mut self, data: &[u8]) -> Output {
        let mut out = Output::default();
        for &b in data {
            match self.rx {
                Rx::Data => {
                    if b == IAC {
                        self.rx = Rx::Iac;
                    } else if self.mode == Mode::Negotiating {
                        self.nvt.push(b);
                    } else {
                        self.record.push(b);
                    }
                }
                Rx::Iac => {
                    self.rx = Rx::Data;
                    match b {
                        IAC => {
                            if self.mode == Mode::Negotiating {
                                self.nvt.push(IAC);
                            } else {
                                self.record.push(IAC);
                            }
                        }
                        EOR => {
                            let rec = std::mem::take(&mut self.record);
                            self.process_record(&rec, &mut out);
                        }
                        DO | DONT | WILL | WONT => self.rx = Rx::Opt(b),
                        SB => {
                            self.sb.clear();
                            self.rx = Rx::Sb;
                        }
                        _ => {}
                    }
                }
                Rx::Opt(cmd) => {
                    self.rx = Rx::Data;
                    self.option(cmd, b, &mut out);
                }
                Rx::Sb => {
                    if b == IAC {
                        self.rx = Rx::SbIac;
                    } else {
                        self.sb.push(b);
                    }
                }
                Rx::SbIac => {
                    if b == SE {
                        self.rx = Rx::Data;
                        let sb = std::mem::take(&mut self.sb);
                        self.subnegotiation(&sb, &mut out);
                    } else {
                        // IAC IAC は 0xFF
                        self.sb.push(b);
                        self.rx = Rx::Sb;
                    }
                }
            }
        }
        if !self.nvt.is_empty() && self.rx == Rx::Data {
            let text: String = String::from_utf8_lossy(&std::mem::take(&mut self.nvt))
                .chars()
                .filter(|c| !c.is_control() || *c == '\n')
                .collect();
            if !text.trim().is_empty() {
                out.events.push(Event::Text(text.trim().to_owned()));
            }
        }
        out
    }

    fn send_cmd(out: &mut Output, cmd: u8, opt: u8) {
        out.send.extend_from_slice(&[IAC, cmd, opt]);
    }

    fn send_sb(out: &mut Output, body: &[u8]) {
        out.send.extend_from_slice(&[IAC, SB]);
        for &b in body {
            out.send.push(b);
            if b == IAC {
                out.send.push(IAC);
            }
        }
        out.send.extend_from_slice(&[IAC, SE]);
    }

    fn option(&mut self, cmd: u8, opt: u8, out: &mut Output) {
        let name = match opt {
            OPT_BINARY => "BINARY".to_owned(),
            OPT_EOR => "END-OF-RECORD".to_owned(),
            OPT_TTYPE => "TERMINAL-TYPE".to_owned(),
            OPT_TN3270E => "TN3270E".to_owned(),
            o => format!("オプション {o}"),
        };
        let what = match cmd {
            DO => "DO",
            DONT => "DONT",
            WILL => "WILL",
            _ => "WONT",
        };
        out.events
            .push(Event::Negotiation(format!("受信: {what} {name}")));
        match (cmd, opt) {
            (DO, OPT_TN3270E) if self.cfg.tn3270e => {
                if !self.tn3270e {
                    self.tn3270e = true;
                    Self::send_cmd(out, WILL, OPT_TN3270E);
                }
            }
            (DO, OPT_TN3270E) => Self::send_cmd(out, WONT, OPT_TN3270E),
            (DONT, OPT_TN3270E) => {
                if self.tn3270e {
                    self.tn3270e = false;
                    Self::send_cmd(out, WONT, OPT_TN3270E);
                    out.events.push(Event::Negotiation(
                        "TN3270E を使わず、TN3270 で続けます".into(),
                    ));
                }
            }
            (DO, OPT_TTYPE) => {
                if !self.will_ttype {
                    self.will_ttype = true;
                    Self::send_cmd(out, WILL, OPT_TTYPE);
                }
            }
            (DO, OPT_BINARY) => {
                if !self.will_binary {
                    self.will_binary = true;
                    Self::send_cmd(out, WILL, OPT_BINARY);
                }
            }
            (WILL, OPT_BINARY) => {
                if !self.do_binary {
                    self.do_binary = true;
                    Self::send_cmd(out, DO, OPT_BINARY);
                }
            }
            (DO, OPT_EOR) => {
                if !self.will_eor {
                    self.will_eor = true;
                    Self::send_cmd(out, WILL, OPT_EOR);
                }
            }
            (WILL, OPT_EOR) => {
                if !self.do_eor {
                    self.do_eor = true;
                    Self::send_cmd(out, DO, OPT_EOR);
                }
            }
            (DONT | WONT, OPT_BINARY | OPT_EOR) => {
                match (cmd, opt) {
                    (DONT, OPT_BINARY) => self.will_binary = false,
                    (WONT, OPT_BINARY) => self.do_binary = false,
                    (DONT, _) => self.will_eor = false,
                    _ => self.do_eor = false,
                }
                self.set_mode(Mode::Negotiating, out);
            }
            (DO, _) => Self::send_cmd(out, WONT, opt),
            (WILL, _) => Self::send_cmd(out, DONT, opt),
            _ => {}
        }
        self.update_mode(out);
    }

    fn subnegotiation(&mut self, sb: &[u8], out: &mut Output) {
        match sb {
            [OPT_TTYPE, TTYPE_SEND, ..] => {
                let tt = self.cfg.terminal_type();
                out.events
                    .push(Event::Negotiation(format!("端末の種類: {tt}")));
                let mut body = vec![OPT_TTYPE, TTYPE_IS];
                body.extend_from_slice(tt.as_bytes());
                Self::send_sb(out, &body);
            }
            [OPT_TN3270E, E_SEND, E_DEVICE_TYPE, ..] => {
                let tt = self.cfg.terminal_type();
                let mut body = vec![OPT_TN3270E, E_DEVICE_TYPE, E_REQUEST];
                body.extend_from_slice(tt.as_bytes());
                let associate = self
                    .cfg
                    .associate
                    .as_deref()
                    .filter(|l| self.cfg.printer && !l.is_empty());
                if let Some(lu) = self.cfg.lu.as_deref().filter(|l| !l.is_empty()) {
                    body.push(E_CONNECT);
                    body.extend_from_slice(lu.as_bytes());
                    out.events.push(Event::Negotiation(format!(
                        "TN3270E: 端末の種類 {tt}、LU {lu} を求めます"
                    )));
                } else if let Some(term) = associate {
                    body.push(E_ASSOCIATE);
                    body.extend_from_slice(term.as_bytes());
                    out.events.push(Event::Negotiation(format!(
                        "TN3270E: 端末の種類 {tt}、端末の LU {term} に対応するプリンターを求めます"
                    )));
                } else {
                    out.events.push(Event::Negotiation(format!(
                        "TN3270E: 端末の種類 {tt} を求めます（LU はサーバーが選ぶ）"
                    )));
                }
                Self::send_sb(out, &body);
            }
            [OPT_TN3270E, E_DEVICE_TYPE, E_IS, rest @ ..] => {
                let device = rest
                    .iter()
                    .position(|&b| b == E_CONNECT)
                    .map(|i| String::from_utf8_lossy(&rest[i + 1..]).into_owned());
                if let Some(d) = &device {
                    out.events.push(Event::Device(d.clone()));
                }
                self.device = device;
                // RESPONSES（プリンターは SCS-CTL-CODES・DATA-STREAM-CTL も）を求める
                // （BIND-IMAGE を使うと SSCP-LU の画面の扱いが要る）
                let mut body = vec![OPT_TN3270E, E_FUNCTIONS, E_REQUEST];
                body.extend_from_slice(&self.wanted_functions());
                Self::send_sb(out, &body);
            }
            [OPT_TN3270E, E_DEVICE_TYPE, E_REJECT, rest @ ..] => {
                let reason = rest
                    .iter()
                    .position(|&b| b == E_REASON)
                    .and_then(|i| rest.get(i + 1))
                    .map_or("（理由なし）", |&c| reason_text(c));
                out.events.push(Event::DeviceRejected(reason.to_owned()));
            }
            [OPT_TN3270E, E_FUNCTIONS, E_IS, list @ ..] => {
                self.functions = list.to_vec();
                out.events.push(Event::Negotiation(format!(
                    "TN3270E: 関数 {}",
                    function_names(list)
                )));
                self.set_mode(Mode::Tn3270e, out);
            }
            [OPT_TN3270E, E_FUNCTIONS, E_REQUEST, list @ ..] => {
                // サーバーの提案のうち、使えるものだけで答える
                let wanted = self.wanted_functions();
                let ours: Vec<u8> = list
                    .iter()
                    .copied()
                    .filter(|f| wanted.contains(f))
                    .collect();
                let mut body = vec![OPT_TN3270E, E_FUNCTIONS];
                if ours.len() == list.len() {
                    body.push(E_IS);
                    self.functions = ours.clone();
                    body.extend_from_slice(&ours);
                    Self::send_sb(out, &body);
                    self.set_mode(Mode::Tn3270e, out);
                } else {
                    body.push(E_REQUEST);
                    body.extend_from_slice(&ours);
                    Self::send_sb(out, &body);
                }
            }
            _ => {}
        }
    }

    /// 使える TN3270E の関数。
    fn wanted_functions(&self) -> Vec<u8> {
        if self.cfg.printer {
            vec![FN_RESPONSES, FN_SCS_CTL_CODES, FN_DATA_STREAM_CTL]
        } else {
            vec![FN_RESPONSES]
        }
    }

    /// プリンターの書きかけのジョブを終える（PRINT-EOJ のないホストで、一定時間データが
    /// 来なかったとき・切断したとき）。何もなければ `None`。
    pub fn flush_print(&mut self) -> Option<PrintJob> {
        let mut job = self.scs.finish().unwrap_or_default();
        if !self.lu3.is_empty() {
            job.pages.append(&mut self.lu3);
            job.bytes += std::mem::take(&mut self.lu3_bytes);
            job.columns = job.columns.max(job.width());
        }
        (job.bytes > 0).then_some(job)
    }

    /// プリンターのジョブを組み立て中か。
    pub fn printing(&self) -> bool {
        self.scs.has_data() || self.lu3_bytes > 0
    }

    fn set_mode(&mut self, m: Mode, out: &mut Output) {
        if self.mode != m {
            self.mode = m;
            out.events.push(Event::Mode(m));
            out.changed = true;
        }
    }

    /// TN3270（TN3270E を使わない）の交渉が済んだか。
    fn update_mode(&mut self, out: &mut Output) {
        if !self.tn3270e
            && self.will_binary
            && self.do_binary
            && self.will_eor
            && self.do_eor
            && self.mode == Mode::Negotiating
        {
            self.set_mode(Mode::Tn3270, out);
        }
    }

    fn process_record(&mut self, rec: &[u8], out: &mut Output) {
        let (data, header) = if self.mode == Mode::Tn3270e {
            if rec.len() < 5 {
                return;
            }
            (&rec[5..], Some([rec[0], rec[1], rec[2], rec[3], rec[4]]))
        } else {
            (rec, None)
        };
        if let Some(h) = header {
            match h[0] {
                DT_3270_DATA => {}
                DT_NVT_DATA | DT_SSCP_LU_DATA => {
                    let text = String::from_utf8_lossy(data).trim().to_owned();
                    if !text.is_empty() {
                        out.events.push(Event::Text(text));
                    }
                    return;
                }
                DT_SCS_DATA if self.cfg.printer => {
                    self.scs.feed(data);
                    self.respond(h, out);
                    return;
                }
                DT_PRINT_EOJ if self.cfg.printer => {
                    if let Some(job) = self.flush_print() {
                        out.events.push(Event::PrintJob(job));
                    }
                    self.respond(h, out);
                    return;
                }
                // それ以外（BIND など）は使わない
                _ => return,
            }
        }
        let r = self.emu.process(data);
        if self.cfg.printer
            && let Some(wcc) = self.emu.print_wcc.take()
        {
            self.lu3
                .extend(print::lu3_pages(&self.emu.screen, self.emu.ccsid, wcc));
            self.lu3_bytes += data.len();
        }
        if r.alarm {
            out.events.push(Event::Alarm);
        }
        out.changed |= r.changed;
        if let Some(h) = header {
            self.respond(h, out);
        }
        if let Some(d) = r.data {
            self.send_data(&d, out);
        }
        out.events
            .extend(self.emu.ft.take_events().into_iter().map(Event::Transfer));
    }

    /// 3270 のデータを送る（TN3270E ならヘッダーを付け、IAC を二重にして IAC EOR で終える）。
    /// 応答を求められたら肯定の応答（Device End）。
    fn respond(&self, h: [u8; 5], out: &mut Output) {
        if h[2] == RSP_ALWAYS_RESPONSE && self.functions.contains(&FN_RESPONSES) {
            let mut body = vec![DT_RESPONSE, 0, RSP_POSITIVE, h[3], h[4], 0x00];
            frame(&mut body);
            out.send.extend_from_slice(&body);
        }
    }

    fn send_data(&self, data: &[u8], out: &mut Output) {
        let mut rec = Vec::with_capacity(data.len() + 8);
        if self.mode == Mode::Tn3270e {
            rec.extend_from_slice(&[DT_3270_DATA, 0, 0, 0, 0]);
        }
        rec.extend_from_slice(data);
        frame(&mut rec);
        out.send.extend_from_slice(&rec);
    }

    // ---- キー操作 ------------------------------------------------------------------------

    /// キーを押した。
    pub fn key(&mut self, k: Key) -> Output {
        let mut out = Output::default();
        if self.mode == Mode::Negotiating {
            return out;
        }
        let r = self.emu.key(k);
        out.changed = r.changed;
        if r.attn {
            out.send.extend_from_slice(&[IAC, IP]);
        }
        if let Some(d) = r.data {
            self.send_data(&d, &mut out);
        }
        out
    }

    /// 文字列を貼り付ける。
    /// IND$FILE の転送を始める: カーソルの位置にコマンドを入力して Enter を押す
    /// （TSO の READY・CMS の Ready の後で使う）。
    pub fn transfer(
        &mut self,
        req: &ind_file::Request,
        local: ind_file::Local,
    ) -> Result<Output, String> {
        if self.mode == Mode::Negotiating {
            return Err("3270 で接続していません".into());
        }
        if self.emu.ft.active() {
            return Err("ほかの転送を実行中です".into());
        }
        if self.emu.lock != Lock::None {
            return Err(
                "キーボードがロックされています（応答を待つか、リセットしてください）".into(),
            );
        }
        if self.emu.screen.is_protected(self.emu.screen.cursor) {
            return Err("カーソルが入力できない位置にあります".into());
        }
        self.emu.key(Key::EraseEof);
        for c in req.command().chars() {
            self.emu.key(Key::Char(c));
            if let Lock::Operator(e) = self.emu.lock {
                self.emu.key(Key::Reset);
                return Err(format!("コマンドを入力できません（{}）", e.label()));
            }
        }
        self.emu.ft.start(local);
        let mut out = self.key(Key::Enter);
        out.changed = true;
        Ok(out)
    }

    /// 転送を取り消す。
    pub fn cancel_transfer(&mut self) -> Vec<Event> {
        self.emu.ft.cancel();
        self.emu
            .ft
            .take_events()
            .into_iter()
            .map(Event::Transfer)
            .collect()
    }

    /// 接続が切れたときなど、転送をやめる。
    pub fn abandon_transfer(&mut self, why: &str) -> Vec<Event> {
        self.emu.ft.abandon(why);
        self.emu
            .ft
            .take_events()
            .into_iter()
            .map(Event::Transfer)
            .collect()
    }

    pub fn transferring(&self) -> bool {
        self.emu.ft.active()
    }

    pub fn paste(&mut self, text: &str) -> Output {
        let mut out = Output::default();
        if self.mode != Mode::Negotiating && self.emu.paste(text) > 0 {
            out.changed = true;
        }
        out
    }
}

/// IAC を二重にして IAC EOR で終える。
fn frame(rec: &mut Vec<u8>) {
    let mut out = Vec::with_capacity(rec.len() + 2);
    for &b in rec.iter() {
        out.push(b);
        if b == IAC {
            out.push(IAC);
        }
    }
    out.extend_from_slice(&[IAC, EOR]);
    *rec = out;
}

fn function_names(list: &[u8]) -> String {
    let names: Vec<&str> = list
        .iter()
        .map(|f| match *f {
            FN_BIND_IMAGE => "BIND-IMAGE",
            FN_DATA_STREAM_CTL => "DATA-STREAM-CTL",
            FN_RESPONSES => "RESPONSES",
            FN_SCS_CTL_CODES => "SCS-CTL-CODES",
            FN_SYSREQ => "SYSREQ",
            _ => "?",
        })
        .collect();
    if names.is_empty() {
        "なし".into()
    } else {
        names.join("・")
    }
}

#[cfg(test)]
mod tests;

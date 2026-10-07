//! 通信の記録（データストリームのトレース。14 章 10 節）。
//!
//! 送受信の単位（Telnet の命令・サブネゴシエーション・3270 のレコード）ごとに、16 進と
//! 解釈（コマンド・WCC・オーダー・属性・構造化フィールド）を残す。非表示のフィールドの中身
//! （パスワード）は、16 進でも解釈でも `**`・`******` に置き換える。
//!
//! 書式（[`TraceEntry::format`]）:
//!
//! ```text
//! 10:15:02.123 < REC 3270-DATA seq=1 (16 バイト)
//!     0000  F5 C2 11 40 40 1D 60 D3 D6 C7 D6 D5
//!     EraseWrite WCC(reset,restore)
//!     SBA 1,1  SF(protected)  "LOGON"
//! 10:15:05.456 > REC AID Enter, カーソル 5,12 (9 バイト)
//!     0000  7D 4C 4B 11 4C 4B ** ** **
//!     SBA 5,12  "******"
//! ```
//!
//! [`parse`] でこの書式から受信の単位を取り出し、[`replay`] で Telnet のバイト列に戻せる
//! （障害の再現・テストの入力）。

use yy_encoding::Ccsid;

use crate::codes::*;

/// 向き。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// ホストから
    In,
    /// ホストへ
    Out,
}

/// 単位の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// 3270 のレコード（IAC EOR まで。TN3270E のヘッダーを含む。IAC は二重にしない）
    Record,
    /// Telnet の命令・サブネゴシエーション（そのままのバイト列）
    Telnet,
    /// 3270 になる前の文字（NVT）
    Nvt,
}

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Kind::Record => "REC",
            Kind::Telnet => "TEL",
            Kind::Nvt => "NVT",
        }
    }
}

/// 記録の 1 単位。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceEntry {
    pub dir: Dir,
    pub kind: Kind,
    /// 1 行の要約
    pub summary: String,
    /// 解釈（オーダーなど）
    pub detail: Vec<String>,
    pub bytes: Vec<u8>,
    /// 隠したバイトの範囲（`bytes` の位置）
    pub masked: Vec<std::ops::Range<usize>>,
}

/// 16 進の 1 行のバイト数
const HEX_PER_LINE: usize = 16;

impl TraceEntry {
    /// 書き出す文字（`time` は先頭の時刻。行ごとに改行で終わる）。
    pub fn format(&self, time: &str) -> String {
        let arrow = match self.dir {
            Dir::In => '<',
            Dir::Out => '>',
        };
        let mut s = format!(
            "{time} {arrow} {} {} ({} バイト)\n",
            self.kind.tag(),
            self.summary,
            self.bytes.len()
        );
        for (n, chunk) in self.bytes.chunks(HEX_PER_LINE).enumerate() {
            s.push_str(&format!("    {:04X} ", n * HEX_PER_LINE));
            for (k, b) in chunk.iter().enumerate() {
                let pos = n * HEX_PER_LINE + k;
                if self.masked.iter().any(|r| r.contains(&pos)) {
                    s.push_str(" **");
                } else {
                    s.push_str(&format!(" {b:02X}"));
                }
            }
            s.push('\n');
        }
        for d in &self.detail {
            s.push_str("    | ");
            s.push_str(d);
            s.push('\n');
        }
        s
    }
}

/// 記録から取り出した 1 単位（隠したバイトは 0x00）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub dir: Dir,
    pub kind: Kind,
    pub bytes: Vec<u8>,
}

/// 記録の文字から単位を取り出す。
pub fn parse(text: &str) -> Vec<Parsed> {
    let mut out: Vec<Parsed> = Vec::new();
    for line in text.lines() {
        if let Some(hex) = line.strip_prefix("    ")
            && !hex.starts_with('|')
        {
            let Some(cur) = out.last_mut() else { continue };
            // 先頭の位置（4 桁）を飛ばす
            for tok in hex.split_whitespace().skip(1) {
                cur.bytes.push(u8::from_str_radix(tok, 16).unwrap_or(0x00));
            }
            continue;
        }
        let mut parts = line.split_whitespace().skip(1);
        let dir = match parts.next() {
            Some("<") => Dir::In,
            Some(">") => Dir::Out,
            _ => continue,
        };
        let kind = match parts.next() {
            Some("REC") => Kind::Record,
            Some("TEL") => Kind::Telnet,
            Some("NVT") => Kind::Nvt,
            _ => continue,
        };
        out.push(Parsed {
            dir,
            kind,
            bytes: Vec::new(),
        });
    }
    out
}

/// 記録の受信の単位を、ホストから届いた Telnet のバイト列に戻す（[`crate::Session::receive`] に渡せる）。
pub fn replay(text: &str) -> Vec<u8> {
    let mut wire = Vec::new();
    for p in parse(text).into_iter().filter(|p| p.dir == Dir::In) {
        match p.kind {
            Kind::Record => {
                for b in p.bytes {
                    wire.push(b);
                    if b == IAC {
                        wire.push(IAC);
                    }
                }
                wire.extend_from_slice(&[IAC, EOR]);
            }
            Kind::Telnet | Kind::Nvt => wire.extend_from_slice(&p.bytes),
        }
    }
    wire
}

// ---- Telnet ----------------------------------------------------------------------

fn option_name(opt: u8) -> String {
    match opt {
        OPT_BINARY => "BINARY".into(),
        OPT_TTYPE => "TERMINAL-TYPE".into(),
        OPT_EOR => "EOR".into(),
        OPT_TN3270E => "TN3270E".into(),
        o => format!("option {o}"),
    }
}

fn command_name(cmd: u8) -> &'static str {
    match cmd {
        DO => "DO",
        DONT => "DONT",
        WILL => "WILL",
        WONT => "WONT",
        IP => "IP",
        _ => "?",
    }
}

fn function_list(list: &[u8]) -> String {
    list.iter()
        .map(|f| match *f {
            FN_BIND_IMAGE => "BIND-IMAGE".to_owned(),
            FN_DATA_STREAM_CTL => "DATA-STREAM-CTL".to_owned(),
            FN_RESPONSES => "RESPONSES".to_owned(),
            FN_SCS_CTL_CODES => "SCS-CTL-CODES".to_owned(),
            FN_SYSREQ => "SYSREQ".to_owned(),
            f => format!("{f}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// サブネゴシエーションの中身（IAC SB と IAC SE の間）の説明。
fn describe_sb(body: &[u8]) -> String {
    let ascii = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    match body {
        [OPT_TTYPE, TTYPE_SEND, ..] => "TERMINAL-TYPE SEND".into(),
        [OPT_TTYPE, TTYPE_IS, rest @ ..] => format!("TERMINAL-TYPE IS {}", ascii(rest)),
        [OPT_TN3270E, E_SEND, E_DEVICE_TYPE, ..] => "TN3270E SEND DEVICE-TYPE".into(),
        [OPT_TN3270E, E_DEVICE_TYPE, verb, rest @ ..] => {
            let verb = match *verb {
                E_REQUEST => "REQUEST",
                E_IS => "IS",
                E_REJECT => "REJECT",
                _ => "?",
            };
            if verb == "REJECT" {
                let reason = rest
                    .iter()
                    .position(|&b| b == E_REASON)
                    .and_then(|i| rest.get(i + 1))
                    .map_or("", |&c| reason_text(c));
                return format!("TN3270E DEVICE-TYPE REJECT {reason}");
            }
            let mut s = format!("TN3270E DEVICE-TYPE {verb} ");
            let mut start = 0;
            for (i, &b) in rest.iter().enumerate() {
                if b == E_CONNECT || b == E_ASSOCIATE {
                    s.push_str(&ascii(&rest[start..i]));
                    s.push_str(if b == E_CONNECT {
                        " CONNECT "
                    } else {
                        " ASSOCIATE "
                    });
                    start = i + 1;
                }
            }
            s.push_str(&ascii(&rest[start..]));
            s
        }
        [OPT_TN3270E, E_FUNCTIONS, verb, list @ ..] => format!(
            "TN3270E FUNCTIONS {} {}",
            match *verb {
                E_REQUEST => "REQUEST",
                E_IS => "IS",
                _ => "?",
            },
            function_list(list)
        ),
        [opt, ..] => format!("{} …", option_name(*opt)),
        [] => String::new(),
    }
}

/// Telnet の命令（`IAC 命令 オプション`・`IAC IP`）。
pub fn telnet_command(dir: Dir, cmd: u8, opt: Option<u8>) -> TraceEntry {
    let mut bytes = vec![IAC, cmd];
    let summary = match opt {
        Some(o) => {
            bytes.push(o);
            format!("IAC {} {}", command_name(cmd), option_name(o))
        }
        None => format!("IAC {}", command_name(cmd)),
    };
    TraceEntry {
        dir,
        kind: Kind::Telnet,
        summary,
        detail: Vec::new(),
        bytes,
        masked: Vec::new(),
    }
}

/// サブネゴシエーション（中身。記録のバイト列は IAC SB … IAC SE）。
pub fn telnet_sb(dir: Dir, body: &[u8]) -> TraceEntry {
    let mut bytes = vec![IAC, SB];
    for &b in body {
        bytes.push(b);
        if b == IAC {
            bytes.push(IAC);
        }
    }
    bytes.extend_from_slice(&[IAC, SE]);
    TraceEntry {
        dir,
        kind: Kind::Telnet,
        summary: format!("SB {}", describe_sb(body)),
        detail: Vec::new(),
        bytes,
        masked: Vec::new(),
    }
}

/// 3270 になる前に届いた文字。
pub fn nvt(dir: Dir, bytes: &[u8]) -> TraceEntry {
    TraceEntry {
        dir,
        kind: Kind::Nvt,
        summary: format!("{:?}", String::from_utf8_lossy(bytes)),
        detail: Vec::new(),
        bytes: bytes.to_vec(),
        masked: Vec::new(),
    }
}

/// 送るバイト列（Telnet の枠つき）を単位に分ける。`record` で 3270 のレコードを説明する。
pub fn split_outbound(wire: &[u8], mut record: impl FnMut(&[u8]) -> TraceEntry) -> Vec<TraceEntry> {
    let mut out = Vec::new();
    let mut rec = Vec::new();
    let mut i = 0;
    while i < wire.len() {
        let b = wire[i];
        if b != IAC {
            rec.push(b);
            i += 1;
            continue;
        }
        let Some(&c) = wire.get(i + 1) else { break };
        match c {
            IAC => {
                rec.push(IAC);
                i += 2;
            }
            EOR => {
                out.push(record(&std::mem::take(&mut rec)));
                i += 2;
            }
            DO | DONT | WILL | WONT => {
                let opt = wire.get(i + 2).copied().unwrap_or(0);
                out.push(telnet_command(Dir::Out, c, Some(opt)));
                i += 3;
            }
            SB => {
                let mut body = Vec::new();
                let mut j = i + 2;
                while j < wire.len() {
                    if wire[j] == IAC && wire.get(j + 1) == Some(&SE) {
                        break;
                    }
                    if wire[j] == IAC && wire.get(j + 1) == Some(&IAC) {
                        j += 1;
                    }
                    body.push(wire[j]);
                    j += 1;
                }
                out.push(telnet_sb(Dir::Out, &body));
                i = j + 2;
            }
            other => {
                out.push(telnet_command(Dir::Out, other, None));
                i += 2;
            }
        }
    }
    out
}

// ---- 3270 のデータストリーム -------------------------------------------------------

fn pos(addr: usize, cols: usize) -> String {
    let cols = cols.max(1);
    format!("{},{}", addr / cols + 1, addr % cols + 1)
}

/// フィールド属性の説明。
pub fn fa_text(fa: u8) -> String {
    let mut v = Vec::new();
    if fa & FA_PROTECT != 0 {
        v.push("protected");
    }
    if fa & FA_NUMERIC != 0 {
        v.push(if fa & FA_PROTECT != 0 {
            "skip"
        } else {
            "numeric"
        });
    }
    match fa & FA_DISPLAY_MASK {
        FA_NONDISPLAY => v.push("nondisplay"),
        FA_INTENSIFIED => v.push("intense"),
        FA_DETECTABLE => v.push("detectable"),
        _ => {}
    }
    if fa & FA_MDT != 0 {
        v.push("mdt");
    }
    if v.is_empty() {
        "unprotected".into()
    } else {
        v.join(",")
    }
}

fn xa_text(t: u8, v: u8) -> String {
    let color = |c: u8| match c {
        0x00 => "default".to_owned(),
        0xF1 => "blue".into(),
        0xF2 => "red".into(),
        0xF3 => "pink".into(),
        0xF4 => "green".into(),
        0xF5 => "turquoise".into(),
        0xF6 => "yellow".into(),
        0xF7 => "white".into(),
        c => format!("{c:02X}"),
    };
    match t {
        XA_3270 => format!("field={}", fa_text(v)),
        XA_HIGHLIGHTING => format!(
            "highlight={}",
            match v {
                HL_DEFAULT => "default",
                HL_NORMAL => "normal",
                HL_BLINK => "blink",
                HL_REVERSE => "reverse",
                HL_UNDERSCORE => "underscore",
                _ => "?",
            }
        ),
        XA_FOREGROUND => format!("fg={}", color(v)),
        XA_BACKGROUND => format!("bg={}", color(v)),
        XA_CHARSET => {
            if v == CS_DBCS {
                "charset=DBCS".into()
            } else {
                format!("charset={v:02X}")
            }
        }
        XA_ALL => "reset".into(),
        t => format!("{t:02X}={v:02X}"),
    }
}

fn wcc_text(wcc: u8) -> String {
    let mut v = Vec::new();
    if wcc & WCC_RESET != 0 {
        v.push("reset".to_owned());
    }
    if wcc & WCC_START_PRINTER != 0 {
        v.push("print".into());
        v.push(
            match (wcc >> 4) & 0x03 {
                1 => "40",
                2 => "64",
                3 => "80",
                _ => "NL/EM",
            }
            .into(),
        );
    }
    if wcc & WCC_ALARM != 0 {
        v.push("alarm".into());
    }
    if wcc & WCC_RESTORE != 0 {
        v.push("restore".into());
    }
    if wcc & WCC_RESET_MDT != 0 {
        v.push("reset-mdt".into());
    }
    format!("WCC({})", v.join(","))
}

/// 文字の並び（SO/SI の 2 バイト文字を含む）を読む。
fn text_of(bytes: &[u8], ccsid: Ccsid) -> String {
    let mut s = String::new();
    let mut shift = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            FC_SO => {
                shift = true;
                i += 1;
            }
            FC_SI => {
                shift = false;
                i += 1;
            }
            _ if shift && i + 1 < bytes.len() => {
                let code = u16::from_be_bytes([b, bytes[i + 1]]);
                s.push_str(&if code == 0x4040 {
                    "\u{3000}".to_owned()
                } else {
                    ccsid
                        .decode_double(code)
                        .unwrap_or_else(|| format!("<{code:04X}>"))
                });
                i += 2;
            }
            0x00 => {
                s.push('·');
                i += 1;
            }
            b => {
                match ccsid.decode_single(b).filter(|c| !c.is_control()) {
                    Some(c) => s.push(c),
                    None => s.push_str(&format!("<{b:02X}>")),
                }
                i += 1;
            }
        }
    }
    s
}

/// 1 行ずつの解釈を組み立てる（SBA で行を改める）。
struct Lines {
    lines: Vec<String>,
    cur: Vec<String>,
}

impl Lines {
    fn new() -> Lines {
        Lines {
            lines: Vec::new(),
            cur: Vec::new(),
        }
    }
    fn push(&mut self, s: String) {
        self.cur.push(s);
    }
    fn break_line(&mut self) {
        if !self.cur.is_empty() {
            self.lines.push(std::mem::take(&mut self.cur).join("  "));
        }
    }
    fn finish(mut self) -> Vec<String> {
        self.break_line();
        self.lines
    }
}

/// ホストからのオーダーと文字（Write の WCC の後ろ）。非表示のフィールドに書いた文字は隠す。
fn describe_orders(
    data: &[u8],
    base: usize,
    cols: usize,
    ccsid: Ccsid,
    masked: &mut Vec<std::ops::Range<usize>>,
) -> Vec<String> {
    let mut l = Lines::new();
    let mut i = 0;
    let mut hidden = false;
    let mut text_start: Option<usize> = None;
    let flush = |l: &mut Lines,
                 from: Option<usize>,
                 to: usize,
                 hidden: bool,
                 masked: &mut Vec<std::ops::Range<usize>>| {
        if let Some(f) = from
            && f < to
        {
            if hidden {
                masked.push(base + f..base + to);
                l.push("\"******\"".into());
            } else {
                l.push(format!("{:?}", text_of(&data[f..to], ccsid)));
            }
        }
    };
    while i < data.len() {
        let b = data[i];
        let order = matches!(
            b,
            ORDER_SBA
                | ORDER_SF
                | ORDER_SFE
                | ORDER_SA
                | ORDER_MF
                | ORDER_IC
                | ORDER_PT
                | ORDER_RA
                | ORDER_EUA
                | ORDER_GE
        );
        if !order {
            text_start.get_or_insert(i);
            i += 1;
            continue;
        }
        flush(&mut l, text_start.take(), i, hidden, masked);
        let addr = |k: usize| {
            data.get(k)
                .zip(data.get(k + 1))
                .map(|(a, b)| decode_address(*a, *b))
        };
        match b {
            ORDER_SBA => {
                l.break_line();
                l.push(format!(
                    "SBA {}",
                    addr(i + 1).map_or("?".into(), |a| pos(a, cols))
                ));
                i += 3;
            }
            ORDER_SF => {
                let fa = data.get(i + 1).copied().unwrap_or(0);
                hidden = fa & FA_DISPLAY_MASK == FA_NONDISPLAY;
                l.push(format!("SF({})", fa_text(fa)));
                i += 2;
            }
            ORDER_SFE | ORDER_MF => {
                let n = usize::from(data.get(i + 1).copied().unwrap_or(0));
                let pairs: Vec<String> = (0..n)
                    .filter_map(|k| {
                        let t = *data.get(i + 2 + 2 * k)?;
                        let v = *data.get(i + 3 + 2 * k)?;
                        if t == XA_3270 && b == ORDER_SFE {
                            hidden = v & FA_DISPLAY_MASK == FA_NONDISPLAY;
                        }
                        Some(xa_text(t, v))
                    })
                    .collect();
                l.push(format!(
                    "{}({})",
                    if b == ORDER_SFE { "SFE" } else { "MF" },
                    pairs.join(",")
                ));
                i += 2 + 2 * n;
            }
            ORDER_SA => {
                let (t, v) = (
                    data.get(i + 1).copied().unwrap_or(0),
                    data.get(i + 2).copied().unwrap_or(0),
                );
                l.push(format!("SA({})", xa_text(t, v)));
                i += 3;
            }
            ORDER_IC => {
                l.push("IC".into());
                i += 1;
            }
            ORDER_PT => {
                l.push("PT".into());
                i += 1;
            }
            ORDER_RA => {
                let to = addr(i + 1).map_or("?".into(), |a| pos(a, cols));
                let (len, text) = match data.get(i + 3) {
                    Some(&ORDER_GE) => (
                        5,
                        format!("GE {:02X}", data.get(i + 4).copied().unwrap_or(0)),
                    ),
                    Some(&c) => (4, format!("{:?}", text_of(&[c], ccsid))),
                    None => (3, String::new()),
                };
                l.push(format!("RA {to} {text}"));
                i += len;
            }
            ORDER_EUA => {
                l.push(format!(
                    "EUA {}",
                    addr(i + 1).map_or("?".into(), |a| pos(a, cols))
                ));
                i += 3;
            }
            ORDER_GE => {
                l.push(format!("GE {:02X}", data.get(i + 1).copied().unwrap_or(0)));
                i += 2;
            }
            _ => i += 1,
        }
    }
    flush(&mut l, text_start.take(), data.len(), hidden, masked);
    l.finish()
}

fn command_text(c: Command) -> &'static str {
    match c {
        Command::Write => "Write",
        Command::EraseWrite => "EraseWrite",
        Command::EraseWriteAlternate => "EraseWriteAlternate",
        Command::ReadBuffer => "ReadBuffer",
        Command::ReadModified => "ReadModified",
        Command::ReadModifiedAll => "ReadModifiedAll",
        Command::EraseAllUnprotected => "EraseAllUnprotected",
        Command::WriteStructuredField => "WriteStructuredField",
    }
}

/// IND$FILE（DFT）の要求・応答の名前。
fn dft_text(sf: &[u8]) -> String {
    let Some(code) = sf.get(3..5).map(|c| u16::from_be_bytes([c[0], c[1]])) else {
        return "DFT".into();
    };
    let name = match code {
        0x0012 => {
            let n = &sf[sf.len().saturating_sub(7)..];
            return format!("DFT Open {:?}", String::from_utf8_lossy(n));
        }
        0x0009 => "Open の応答",
        0x4112 => "Close",
        0x4109 => "Close の応答",
        0x4511 => "Set Cursor",
        0x4611 => "Get",
        0x4605 => {
            let n = sf
                .get(15..17)
                .map_or(0, |b| u16::from_be_bytes([b[0], b[1]]).saturating_sub(5));
            return format!("DFT Get の応答（データ {n} バイト）");
        }
        0x4711 => "Insert",
        0x4704 => {
            let n = sf
                .get(8..10)
                .map_or(0, |b| u16::from_be_bytes([b[0], b[1]]).saturating_sub(5));
            return format!("DFT Data Insert（データ {n} バイト）");
        }
        0x4705 => "Insert の応答",
        c if c & 0xFF == 0x08 => {
            return format!(
                "DFT エラー（{:04X}）",
                u16::from_be_bytes([
                    sf.get(7).copied().unwrap_or(0),
                    sf.get(8).copied().unwrap_or(0)
                ])
            );
        }
        _ => return format!("DFT {code:04X}"),
    };
    format!("DFT {name}")
}

fn qr_name(code: u8) -> String {
    match code {
        QR_SUMMARY => "Summary".into(),
        QR_USABLE_AREA => "UsableArea".into(),
        QR_ALPHA_PARTITIONS => "AlphanumericPartitions".into(),
        QR_CHARSETS => "CharacterSets".into(),
        QR_COLOR => "Color".into(),
        QR_HIGHLIGHTING => "Highlighting".into(),
        QR_REPLY_MODES => "ReplyModes".into(),
        QR_DBCS_ASIA => "DBCS-Asia".into(),
        QR_DDM => "DDM".into(),
        QR_RPQ_NAMES => "RPQNames".into(),
        QR_IMPLICIT_PART => "ImplicitPartition".into(),
        QR_NULL => "Null".into(),
        c => format!("{c:02X}"),
    }
}

/// 構造化フィールドの並び（ホストから、または端末から）。
fn describe_sfs(mut data: &[u8], cols: usize, ccsid: Ccsid, inbound: bool) -> Vec<String> {
    let mut out = Vec::new();
    while data.len() >= 3 {
        let len = usize::from(u16::from_be_bytes([data[0], data[1]]));
        let len = if len == 0 {
            data.len()
        } else {
            len.min(data.len())
        };
        if len < 3 {
            break;
        }
        let (sf, rest) = data.split_at(len);
        data = rest;
        let id = sf[2];
        if !inbound {
            match id {
                0x81 => out.push(format!(
                    "QueryReply {}",
                    sf.get(3).map_or("?".into(), |&c| qr_name(c))
                )),
                SF_DATA_CHUNK => out.push(dft_text(sf)),
                id => out.push(format!("SF {id:02X}（{len} バイト）")),
            }
            continue;
        }
        match id {
            SF_READ_PARTITION => out.push(format!(
                "ReadPartition {}",
                match sf.get(4) {
                    Some(&RP_QUERY) => "Query",
                    Some(&RP_QUERY_LIST) => "QueryList",
                    Some(&RP_RB) => "ReadBuffer",
                    Some(&RP_RM) => "ReadModified",
                    Some(&RP_RMA) => "ReadModifiedAll",
                    _ => "?",
                }
            )),
            SF_ERASE_RESET => out.push(format!(
                "EraseReset{}",
                if sf.get(3).is_some_and(|f| f & 0x80 != 0) {
                    " alternate"
                } else {
                    ""
                }
            )),
            SF_SET_REPLY_MODE => out.push(format!(
                "SetReplyMode {}",
                match sf.get(4) {
                    Some(1) => "ExtendedField",
                    Some(2) => "Character",
                    _ => "Field",
                }
            )),
            SF_OUTBOUND_3270DS if sf.len() > 4 => {
                out.push("Outbound3270DS".into());
                let mut masked = Vec::new();
                out.extend(
                    describe_3270_in(&sf[4..], cols, ccsid, 0, &mut masked)
                        .into_iter()
                        .map(|l| format!("  {l}")),
                );
            }
            SF_DATA_CHUNK => out.push(dft_text(sf)),
            id => out.push(format!("SF {id:02X}（{len} バイト）")),
        }
    }
    out
}

/// ホストからの 3270 データ（コマンドから）の解釈。`base` は記録のバイト列の中での位置。
fn describe_3270_in(
    data: &[u8],
    cols: usize,
    ccsid: Ccsid,
    base: usize,
    masked: &mut Vec<std::ops::Range<usize>>,
) -> Vec<String> {
    let Some(&c) = data.first() else {
        return Vec::new();
    };
    let Some(cmd) = Command::from_byte(c) else {
        return vec![format!("不明なコマンド {c:02X}")];
    };
    match cmd {
        Command::Write | Command::EraseWrite | Command::EraseWriteAlternate => {
            let wcc = data.get(1).copied().unwrap_or(0);
            let mut v = vec![format!("{} {}", command_text(cmd), wcc_text(wcc))];
            if data.len() > 2 {
                v.extend(describe_orders(&data[2..], base + 2, cols, ccsid, masked));
            }
            v
        }
        Command::WriteStructuredField => {
            let mut v = vec!["WriteStructuredField".to_owned()];
            v.extend(describe_sfs(&data[1..], cols, ccsid, true));
            v
        }
        other => vec![command_text(other).to_owned()],
    }
}

/// AID の名前。
pub fn aid_name(aid: u8) -> String {
    match aid {
        AID_ENTER => return "Enter".into(),
        AID_CLEAR => return "Clear".into(),
        AID_PA1 => return "PA1".into(),
        AID_PA2 => return "PA2".into(),
        AID_PA3 => return "PA3".into(),
        AID_SYSREQ => return "SysReq".into(),
        AID_SF => return "StructuredField".into(),
        AID_NONE => return "なし".into(),
        AID_SELECT => return "Select".into(),
        _ => {}
    }
    (1..=24)
        .find(|&n| pf_aid(n) == Some(aid))
        .map_or_else(|| format!("{aid:02X}"), |n| format!("PF{n}"))
}

/// TN3270E のヘッダーの説明。
fn header_text(h: &[u8]) -> String {
    let ty = match h[0] {
        DT_3270_DATA => "3270-DATA",
        DT_SCS_DATA => "SCS-DATA",
        DT_RESPONSE => "RESPONSE",
        DT_BIND_IMAGE => "BIND-IMAGE",
        DT_UNBIND => "UNBIND",
        DT_NVT_DATA => "NVT-DATA",
        DT_REQUEST => "REQUEST",
        DT_SSCP_LU_DATA => "SSCP-LU-DATA",
        DT_PRINT_EOJ => "PRINT-EOJ",
        _ => "?",
    };
    let rsp = match (h[0], h[2]) {
        (DT_RESPONSE, RSP_POSITIVE) => " positive",
        (DT_RESPONSE, RSP_NEGATIVE) => " negative",
        (_, RSP_ERROR_RESPONSE) => " error-response",
        (_, RSP_ALWAYS_RESPONSE) => " always-response",
        _ => "",
    };
    format!("{ty}{rsp} seq={}", u16::from_be_bytes([h[3], h[4]]))
}

/// ホストから届いたレコード（TN3270E のヘッダーつきなら `tn3270e`）。
pub fn inbound_record(rec: &[u8], tn3270e: bool, cols: usize, ccsid: Ccsid) -> TraceEntry {
    let mut masked = Vec::new();
    let (summary, detail) = if tn3270e && rec.len() >= 5 {
        let h = &rec[..5];
        let data = &rec[5..];
        let detail = match h[0] {
            DT_3270_DATA => describe_3270_in(data, cols, ccsid, 5, &mut masked),
            DT_SCS_DATA => vec![format!("SCS {:?}", text_of(data, ccsid))],
            DT_NVT_DATA | DT_SSCP_LU_DATA => {
                vec![format!("{:?}", text_of(data, ccsid))]
            }
            _ => Vec::new(),
        };
        (header_text(h), detail)
    } else {
        (
            "3270".to_owned(),
            describe_3270_in(rec, cols, ccsid, 0, &mut masked),
        )
    };
    TraceEntry {
        dir: Dir::In,
        kind: Kind::Record,
        summary,
        detail,
        bytes: rec.to_vec(),
        masked,
    }
}

/// 端末から送るレコード。`hidden_at(位置)` はその位置が非表示のフィールドか（送る直前の画面）。
pub fn outbound_record(
    rec: &[u8],
    tn3270e: bool,
    cols: usize,
    ccsid: Ccsid,
    hidden_at: impl Fn(usize) -> bool,
) -> TraceEntry {
    let (head, data, base) = if tn3270e && rec.len() >= 5 {
        (Some(&rec[..5]), &rec[5..], 5)
    } else {
        (None, rec, 0)
    };
    let mut masked = Vec::new();
    let mut detail = Vec::new();
    let mut summary = head.map(header_text).unwrap_or_else(|| "3270".into());
    let is_data = head.is_none_or(|h| h[0] == DT_3270_DATA);
    if is_data && let Some(&aid) = data.first() {
        if aid == AID_SF {
            summary.push_str(&format!(" AID {}", aid_name(aid)));
            detail = describe_sfs(&data[1..], cols, ccsid, false);
        } else {
            let cursor = data
                .get(1..3)
                .map(|c| pos(decode_address(c[0], c[1]), cols))
                .unwrap_or_default();
            summary.push_str(&format!(" AID {}", aid_name(aid)));
            if !cursor.is_empty() {
                summary.push_str(&format!(", カーソル {cursor}"));
            }
            // SBA 位置 文字… の並び（Read Buffer の応答では SF も来る）
            let mut l = Lines::new();
            let mut i = 3.min(data.len());
            let mut addr = 0usize;
            let mut start: Option<(usize, usize)> = None;
            let mut field_hidden: Option<bool> = None;
            let flush = |l: &mut Lines,
                         from: Option<(usize, usize)>,
                         to: usize,
                         masked: &mut Vec<std::ops::Range<usize>>,
                         field_hidden: Option<bool>| {
                if let Some((f, a)) = from
                    && f < to
                {
                    if field_hidden.unwrap_or_else(|| hidden_at(a)) {
                        masked.push(base + f..base + to);
                        l.push("\"******\"".into());
                    } else {
                        l.push(format!("{:?}", text_of(&data[f..to], ccsid)));
                    }
                }
            };
            while i < data.len() {
                match data[i] {
                    ORDER_SBA if i + 2 < data.len() => {
                        flush(&mut l, start.take(), i, &mut masked, field_hidden);
                        addr = decode_address(data[i + 1], data[i + 2]);
                        field_hidden = None;
                        l.break_line();
                        l.push(format!("SBA {}", pos(addr, cols)));
                        i += 3;
                    }
                    ORDER_SF if i + 1 < data.len() => {
                        flush(&mut l, start.take(), i, &mut masked, field_hidden);
                        let fa = data[i + 1];
                        field_hidden = Some(fa & FA_DISPLAY_MASK == FA_NONDISPLAY);
                        l.push(format!("SF({})", fa_text(fa)));
                        addr += 1;
                        i += 2;
                    }
                    ORDER_SA if i + 2 < data.len() => {
                        flush(&mut l, start.take(), i, &mut masked, field_hidden);
                        l.push(format!("SA({})", xa_text(data[i + 1], data[i + 2])));
                        i += 3;
                    }
                    _ => {
                        start.get_or_insert((i, addr));
                        addr += 1;
                        i += 1;
                    }
                }
            }
            flush(&mut l, start.take(), data.len(), &mut masked, field_hidden);
            detail = l.finish();
        }
    }
    TraceEntry {
        dir: Dir::Out,
        kind: Kind::Record,
        summary,
        detail,
        bytes: rec.to_vec(),
        masked,
    }
}

#[cfg(test)]
mod tests;

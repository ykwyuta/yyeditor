//! IND$FILE によるファイル転送（DFT 方式。14 章 11 節）。
//!
//! 端末で `IND$FILE GET/PUT ...` を実行すると、ホストは構造化フィールド `0xD0` で
//! 開く・データを送る（Insert）・データを求める（Get）・閉じるを要求してくる。
//! 端末はそれぞれに応答する。データの段階（`FT:DATA`）のあとに結果のメッセージの段階
//! （`FT:MSG`）が続き、`TRANS03` などのメッセージで成否がわかる。
//!
//! 日本語のテキストは、ホストの ASCII 変換（日本語を扱えないことが多い）を使わず、
//! バイナリで転送して端末側で CCSID の変換をする（[`Mode::Text`]）。レコードの区切りは
//! `CRLF` オプションでホストに入れてもらう（EBCDIC の CR LF `0x0D 0x25`）。

use std::io::{Read, Write};

use yy_encoding::{Ccsid, Encoding, EscapeMode, Records};

use crate::codes::*;

/// 転送のバッファの大きさ（DDM の Query Reply で知らせる。ホストはこれを超えて送らない）。
pub const BUFFER_SIZE: usize = 4096;

const OPEN_REQ: u16 = 0x0012;
const OPEN_REPLY: u16 = 0x0009;
const CLOSE_REQ: u16 = 0x4112;
const CLOSE_REPLY: u16 = 0x4109;
const SET_CUR_REQ: u16 = 0x4511;
const GET_REQ: u16 = 0x4611;
const GET_REPLY: u16 = 0x4605;
const INSERT_REQ: u16 = 0x4711;
const DATA_INSERT: u16 = 0x4704;
const INSERT_REPLY: u16 = 0x4705;
const ERROR_REPLY: u8 = 0x08;
const RECNUM_HDR: u16 = 0x6306;
const ERROR_HDR: u16 = 0x6904;
const NOT_COMPRESSED: u16 = 0xC080;
const BEGIN_DATA: u8 = 0x61;
const ERR_EOF: u16 = 0x2200;
const ERR_CMDFAIL: u16 = 0x0100;

/// 転送の向き（端末から見て）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// ホストから受け取る（`IND$FILE GET`）
    Receive,
    /// ホストへ送る（`IND$FILE PUT`）
    Send,
}

/// ホストの種類（コマンドの書き方が違う）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKind {
    Tso,
    Cms,
    Cics,
}

impl HostKind {
    pub const ALL: [HostKind; 3] = [HostKind::Tso, HostKind::Cms, HostKind::Cics];

    pub fn label(self) -> &'static str {
        match self {
            HostKind::Tso => "TSO",
            HostKind::Cms => "CMS",
            HostKind::Cics => "CICS",
        }
    }
}

/// データの扱い。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// テキスト。バイナリで転送し、端末側でこの CCSID と UTF-8 を変換する（日本語の既定）
    Text(Ccsid),
    /// テキスト。ホストの ASCII 変換を使う（`ASCII CRLF`）。ファイルはそのまま
    HostAscii,
    /// バイナリ（変換しない）
    Binary,
}

/// 送るデータセットのレコードの形式（PUT で新しく作るとき）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recfm {
    /// ホストに任せる
    Default,
    Fixed,
    Variable,
    Undefined,
}

/// 転送の指定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub host: HostKind,
    pub direction: Direction,
    /// ホストのファイル（TSO はデータセット名、CMS は `fn ft fm`）
    pub host_file: String,
    pub mode: Mode,
    pub recfm: Recfm,
    /// レコード長（PUT。0 ならホストに任せる）
    pub lrecl: u32,
    /// TSO の新しいデータセットの大きさ（トラック数。0 ならホストに任せる）
    pub space: u32,
    /// 既にあるファイルに追加する（PUT）
    pub append: bool,
}

impl Request {
    /// テキスト（端末で変換）を固定長のレコードとして扱うときのレコード長。
    ///
    /// 送るときは RECFM F と LRECL、受け取るときは LRECL を指定した場合。`CRLF` を付けず、
    /// 行を LRECL まで空白で詰めて送り、受け取ったものを LRECL ごとに分ける。ホストの
    /// `CRLF` の扱いに頼らない（MVS 3.8j の IND$FILE 2.0.5 は、`ASCII` なしの `CRLF` では
    /// 区切りを入れも除きもしない）。
    pub fn fixed_text(&self) -> Option<usize> {
        let fixed = matches!(self.mode, Mode::Text(_))
            && self.lrecl > 0
            && (self.direction == Direction::Receive || self.recfm == Recfm::Fixed);
        fixed.then_some(self.lrecl as usize)
    }

    /// 端末で入力するコマンド。
    pub fn command(&self) -> String {
        let verb = match self.direction {
            Direction::Receive => "GET",
            Direction::Send => "PUT",
        };
        let mut opts: Vec<String> = Vec::new();
        match self.mode {
            Mode::HostAscii => {
                opts.push("ASCII".into());
                opts.push("CRLF".into());
            }
            // 固定長のテキストは区切りを使わず、端末側で LRECL ごとに詰める・分ける
            Mode::Text(_) if self.fixed_text().is_none() => opts.push("CRLF".into()),
            Mode::Text(_) => {}
            Mode::Binary => {}
        }
        if self.direction == Direction::Send {
            if self.append {
                opts.push("APPEND".into());
            }
            let recfm = match self.recfm {
                Recfm::Default => None,
                Recfm::Fixed => Some("F"),
                Recfm::Variable => Some("V"),
                Recfm::Undefined => Some("U"),
            };
            match self.host {
                HostKind::Tso => {
                    if let Some(r) = recfm {
                        opts.push(format!("RECFM({r})"));
                    }
                    if self.lrecl > 0 {
                        opts.push(format!("LRECL({})", self.lrecl));
                    }
                    if self.space > 0 && !self.append {
                        opts.push(format!("SPACE({},{})", self.space, self.space.div_ceil(2)));
                        opts.push("TRACKS".into());
                    }
                }
                HostKind::Cms => {
                    if let Some(r) = recfm {
                        opts.push(format!("RECFM {r}"));
                    }
                    if self.lrecl > 0 {
                        opts.push(format!("LRECL {}", self.lrecl));
                    }
                }
                HostKind::Cics => {}
            }
        }
        let file = self.host_file.trim();
        let mut cmd = format!("IND$FILE {verb} {file}");
        if !opts.is_empty() {
            match self.host {
                // TSO は空白で並べる。CMS・CICS は ( のあとに並べる
                HostKind::Tso => cmd.push(' '),
                HostKind::Cms | HostKind::Cics => cmd.push_str(" ("),
            }
            cmd.push_str(&opts.join(" "));
        }
        cmd
    }
}

/// 転送の進み具合と結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FtEvent {
    /// ホストが転送を始めた
    Started,
    /// これまでに転送したバイト数
    Progress(u64),
    /// 終わった。`ok` はホストのメッセージが成功（TRANS03・TRANS04）
    Done { ok: bool, message: String },
}

/// 転送の相手（端末側のデータ）。
pub enum Local {
    /// 受け取ったデータを書く
    Sink(Box<dyn Write + Send>),
    /// 送るデータを読む
    Source(Box<dyn Read + Send>),
}

/// 実行中の転送。
struct Job {
    local: Local,
    bytes: u64,
    /// 転送のデータの段階に入った
    started: bool,
    eof: bool,
    /// 利用者が取り消した（次の要求で失敗を返す）
    cancel: bool,
    /// 書き込み・読み込みの失敗（次の要求で失敗を返す）
    error: Option<String>,
}

/// DFT の状態。
#[derive(Default)]
pub struct Dft {
    job: Option<Job>,
    /// 今開いているのがメッセージ（`FT:MSG`）
    message_phase: bool,
    /// メッセージの段階の結果を知らせた（Close を送らないホストがあるため、メッセージで終える）
    message_done: bool,
    message: String,
    recnum: u32,
    events: Vec<FtEvent>,
}

impl Dft {
    /// 転送を始める（コマンドを送る前に呼ぶ）。
    pub fn start(&mut self, local: Local) {
        self.job = Some(Job {
            local,
            bytes: 0,
            started: false,
            eof: false,
            cancel: false,
            error: None,
        });
        self.message.clear();
        self.message_phase = false;
    }

    /// 転送中（ホストの応答を待っている）。
    pub fn active(&self) -> bool {
        self.job.is_some()
    }

    /// 取り消す。ホストの次の要求に失敗を返し、ホストのメッセージを待つ。
    /// ホストがまだ始めていなければ、すぐにやめる。
    pub fn cancel(&mut self) {
        match &mut self.job {
            Some(j) if j.started => j.cancel = true,
            Some(_) => self.abandon("取り消しました"),
            None => {}
        }
    }

    /// 転送をやめる（ホストに知らせない。接続が切れたときなど）。
    pub fn abandon(&mut self, why: &str) {
        if self.job.take().is_some() {
            self.events.push(FtEvent::Done {
                ok: false,
                message: why.to_owned(),
            });
        }
    }

    pub fn take_events(&mut self) -> Vec<FtEvent> {
        std::mem::take(&mut self.events)
    }

    /// 構造化フィールド `0xD0`（長さから）を処理し、ホストへ返すデータ（AID から）を返す。
    pub fn handle(&mut self, sf: &[u8]) -> Option<Vec<u8>> {
        if sf.len() < 5 {
            return None;
        }
        let req = u16::from_be_bytes([sf[3], sf[4]]);
        match req {
            OPEN_REQ => Some(self.open(sf)),
            INSERT_REQ | SET_CUR_REQ => None,
            DATA_INSERT => Some(self.insert(sf)),
            GET_REQ => Some(self.get()),
            CLOSE_REQ => Some(self.close()),
            _ => None,
        }
    }

    fn open(&mut self, sf: &[u8]) -> Vec<u8> {
        // 名前は最後の 7 バイト（ASCII の "FT:DATA" か "FT:MSG "）
        let name = &sf[sf.len().saturating_sub(7)..];
        self.recnum = 1;
        self.message_phase = name.starts_with(b"FT:MSG");
        if self.message_phase {
            self.message.clear();
            self.message_done = false;
        } else {
            match &mut self.job {
                Some(j) => {
                    j.started = true;
                    j.eof = false;
                    self.events.push(FtEvent::Started);
                }
                // 頼んでいない転送は断る
                None => return abort(OPEN_REQ),
            }
        }
        reply(OPEN_REPLY, &[])
    }

    fn insert(&mut self, sf: &[u8]) -> Vec<u8> {
        if sf.len() < 10 {
            return abort(DATA_INSERT);
        }
        let len = usize::from(u16::from_be_bytes([sf[8], sf[9]])).saturating_sub(5);
        let data = &sf[10..sf.len().min(10 + len)];
        if self.message_phase {
            self.message.push_str(&message_text(data));
            // 結果のメッセージ（TRANSnn）で終える。MVS 3.8j の IND$FILE 2.0.5 は、メッセージの
            // 段階を Close せずに READY の画面に戻る
            if self.message.starts_with("TRANS") {
                self.complete();
            }
        } else {
            let Some(j) = &mut self.job else {
                return abort(DATA_INSERT);
            };
            if j.cancel || j.error.is_some() {
                return abort(DATA_INSERT);
            }
            match &mut j.local {
                Local::Sink(w) => {
                    if let Err(e) = w.write_all(data) {
                        j.error = Some(format!("書き込めません: {e}"));
                        return abort(DATA_INSERT);
                    }
                }
                Local::Source(_) => return abort(DATA_INSERT),
            }
            j.bytes += data.len() as u64;
            self.events.push(FtEvent::Progress(j.bytes));
        }
        let mut body = RECNUM_HDR.to_be_bytes().to_vec();
        body.extend_from_slice(&self.recnum.to_be_bytes());
        self.recnum += 1;
        reply(INSERT_REPLY, &body)
    }

    fn get(&mut self) -> Vec<u8> {
        let Some(j) = &mut self.job else {
            return abort(GET_REQ);
        };
        if j.cancel || j.error.is_some() {
            return abort(GET_REQ);
        }
        let Local::Source(r) = &mut j.local else {
            return abort(GET_REQ);
        };
        // 見出し 17 バイトと余裕の分を除いた大きさを読む
        let mut buf = vec![0u8; BUFFER_SIZE - 27];
        let mut n = 0;
        while !j.eof && n < buf.len() {
            match r.read(&mut buf[n..]) {
                Ok(0) => j.eof = true,
                Ok(k) => n += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    j.error = Some(format!("読み込めません: {e}"));
                    return abort(GET_REQ);
                }
            }
        }
        if n == 0 {
            // 終わり
            let mut v = sf_head();
            v.push((GET_REQ >> 8) as u8);
            v.push(ERROR_REPLY);
            v.extend_from_slice(&ERROR_HDR.to_be_bytes());
            v.extend_from_slice(&ERR_EOF.to_be_bytes());
            return finish(v);
        }
        j.bytes += n as u64;
        self.events.push(FtEvent::Progress(j.bytes));
        let mut body = RECNUM_HDR.to_be_bytes().to_vec();
        body.extend_from_slice(&self.recnum.to_be_bytes());
        self.recnum += 1;
        body.extend_from_slice(&NOT_COMPRESSED.to_be_bytes());
        body.push(BEGIN_DATA);
        body.extend_from_slice(&((n + 5) as u16).to_be_bytes());
        body.extend_from_slice(&buf[..n]);
        reply(GET_REPLY, &body)
    }

    /// 転送を終える（メッセージを受け取った・メッセージの段階を閉じた）。2 度目は何もしない。
    fn complete(&mut self) {
        if self.message_done {
            return;
        }
        self.message_done = true;
        {
            let message = std::mem::take(&mut self.message);
            let job = self.job.take();
            let mut ok = message.starts_with("TRANS03") || message.starts_with("TRANS04");
            let mut message = message;
            if let Some(j) = job {
                if let Some(e) = j.error {
                    ok = false;
                    message = format!("{e}（{message}）");
                } else if j.cancel {
                    ok = false;
                    message = format!("取り消しました（{message}）");
                } else if let Local::Sink(mut w) = j.local
                    && let Err(e) = w.flush()
                {
                    ok = false;
                    message = format!("書き込めません: {e}");
                }
            }
            self.events.push(FtEvent::Done { ok, message });
        }
    }

    fn close(&mut self) -> Vec<u8> {
        if self.message_phase {
            // メッセージの段階が終われば転送の終わり
            self.complete();
            self.message_phase = false;
        } else if let Some(j) = &mut self.job
            && let Local::Sink(w) = &mut j.local
            && let Err(e) = w.flush()
        {
            j.error = Some(format!("書き込めません: {e}"));
        }
        reply(CLOSE_REPLY, &[])
    }
}

/// AID・長さの場所・`0xD0`。
fn sf_head() -> Vec<u8> {
    vec![AID_SF, 0, 0, SF_DATA_CHUNK]
}

/// 長さ（AID を除く）を入れる。
fn finish(mut v: Vec<u8>) -> Vec<u8> {
    let len = (v.len() - 1) as u16;
    v[1..3].copy_from_slice(&len.to_be_bytes());
    v
}

fn reply(code: u16, body: &[u8]) -> Vec<u8> {
    let mut v = sf_head();
    v.extend_from_slice(&code.to_be_bytes());
    v.extend_from_slice(body);
    finish(v)
}

/// 要求を失敗として返す（ホストは転送をやめてメッセージを送ってくる）。
fn abort(code: u16) -> Vec<u8> {
    let mut v = sf_head();
    v.push((code >> 8) as u8);
    v.push(ERROR_REPLY);
    v.extend_from_slice(&ERROR_HDR.to_be_bytes());
    v.extend_from_slice(&ERR_CMDFAIL.to_be_bytes());
    finish(v)
}

/// ホストのメッセージ（ASCII のことが多いが、EBCDIC でも読む）。末尾の `$` と空白を除く。
fn message_text(data: &[u8]) -> String {
    let text: String = if data.iter().all(|&b| b < 0x80) {
        String::from_utf8_lossy(data).into_owned()
    } else {
        data.iter()
            .map(|&b| Ccsid::Ibm037.decode_single(b).unwrap_or(' '))
            .collect()
    };
    text.trim_end_matches(|c: char| c == '$' || c.is_whitespace() || c.is_control())
        .trim_start()
        .to_owned()
}

/// DDM の Query Reply の中身（転送ができることと、バッファの大きさ）。
pub fn ddm_query_reply() -> Vec<u8> {
    let mut v = vec![0x00, 0x00];
    v.extend_from_slice(&(BUFFER_SIZE as u16).to_be_bytes());
    v.extend_from_slice(&(BUFFER_SIZE as u16).to_be_bytes());
    v.extend_from_slice(&[0x01, 0x01]);
    v
}

// ---- 端末側の変換（Mode::Text） --------------------------------------------------------

/// レコードの区切り（ホストが入れる CR LF）。CR のあとは EBCDIC の LF・ASCII の LF・NL を認める。
fn split_records(raw: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == 0x0D && matches!(raw.get(i + 1), Some(0x25 | 0x0A | 0x15)) {
            out.push(&raw[start..i]);
            i += 2;
            start = i;
        } else {
            i += 1;
        }
    }
    if start < raw.len() {
        out.push(&raw[start..]);
    }
    out
}

/// 受け取った EBCDIC のレコード（CR LF 区切り）を UTF-8 のテキスト（CRLF 区切り）にする。
/// 固定長のレコードの末尾の空白は除く。変換できないバイトは〓にする。
///
/// `lrecl` が 0 でなければ固定長のレコードとして `lrecl` バイトずつ分ける。0 なら CR LF で
/// 分け、区切りが 1 つもなく大きさが 80 の倍数なら 80 バイトずつ分ける（`CRLF` を付けても
/// 区切りを入れないホストのため）。
pub fn records_to_text(raw: &[u8], ccsid: Ccsid, lrecl: u32) -> String {
    let enc = Encoding::Ebcdic(ccsid, Records::Nl);
    let mut out = String::with_capacity(raw.len() * 2);
    let records: Vec<&[u8]> = if lrecl > 0 {
        raw.chunks(lrecl as usize).collect()
    } else {
        let r = split_records(raw);
        if r.len() <= 1 && !raw.is_empty() && raw.len() % 80 == 0 {
            raw.chunks(80).collect()
        } else {
            r
        }
    };
    for rec in records {
        let (bytes, _) = yy_encoding::decode_all(enc, rec, false);
        let line = String::from_utf8_lossy(&bytes);
        out.push_str(line.trim_end_matches(' ').trim_end_matches('\u{3000}'));
        out.push_str("\r\n");
    }
    out
}

/// 送るテキストの行の問題。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineError {
    /// CCSID にない文字がある
    Unmappable { line: usize, text: String },
    /// レコード長を超える（切り詰めない）
    TooLong {
        line: usize,
        len: usize,
        limit: usize,
    },
}

impl LineError {
    pub fn describe(&self, ccsid: Ccsid) -> String {
        match self {
            LineError::Unmappable { line, text } => {
                format!(
                    "{line} 行目に {} にない文字があります: {text}",
                    ccsid.name()
                )
            }
            LineError::TooLong { line, len, limit } => {
                format!("{line} 行目が {len} バイトで、レコード長（{limit} バイト）を超えます")
            }
        }
    }
}

impl Request {
    /// 送るレコードのデータの長さの上限（LRECL の指定から。可変長は RDW の 4 バイトを除く）。
    pub fn record_limit(&self) -> Option<usize> {
        if self.direction != Direction::Send || self.lrecl == 0 {
            return None;
        }
        let l = self.lrecl as usize;
        match self.recfm {
            Recfm::Variable => Some(l.saturating_sub(4)),
            _ => Some(l),
        }
    }
}

/// UTF-8 のテキストを EBCDIC のレコード（行ごとに CR LF）にする。
/// 変換できない文字や、`limit` を超える行があれば止める。
pub fn text_to_records(
    text: &str,
    ccsid: Ccsid,
    limit: Option<usize>,
) -> Result<Vec<u8>, LineError> {
    let enc = Encoding::Ebcdic(ccsid, Records::Nl);
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut out = Vec::with_capacity(text.len());
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    for (i, line) in lines.iter().enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let b =
            yy_encoding::encode_all(enc, line.as_bytes(), EscapeMode::Reject).map_err(|_| {
                LineError::Unmappable {
                    line: i + 1,
                    text: line.to_owned(),
                }
            })?;
        if let Some(limit) = limit
            && b.len() > limit
        {
            return Err(LineError::TooLong {
                line: i + 1,
                len: b.len(),
                limit,
            });
        }
        out.extend_from_slice(&b);
        out.extend_from_slice(&[0x0D, 0x25]);
    }
    Ok(out)
}

/// UTF-8 のテキストを固定長の EBCDIC のレコード（区切りなし。行を空白で `lrecl` まで詰める）にする。
pub fn text_to_fixed(text: &str, ccsid: Ccsid, lrecl: usize) -> Result<Vec<u8>, LineError> {
    let delimited = text_to_records(text, ccsid, Some(lrecl))?;
    let mut out = Vec::with_capacity(delimited.len() + lrecl);
    for rec in split_records(&delimited) {
        out.extend_from_slice(rec);
        out.resize(out.len() + (lrecl - rec.len()), 0x40);
    }
    Ok(out)
}

/// 送るテキストを、転送の指定（固定長か CRLF か）に合わせて EBCDIC にする（[`Mode::Text`]）。
pub fn encode_upload(req: &Request, text: &str, ccsid: Ccsid) -> Result<Vec<u8>, LineError> {
    match req.fixed_text() {
        Some(lrecl) => text_to_fixed(text, ccsid, lrecl),
        None => text_to_records(text, ccsid, req.record_limit()),
    }
}

/// 受け取ったデータを、転送の指定に合わせてテキストにする（[`Mode::Text`]）。
pub fn decode_download(req: &Request, raw: &[u8], ccsid: Ccsid) -> String {
    records_to_text(raw, ccsid, req.fixed_text().unwrap_or(0) as u32)
}

/// ASCII 変換で受け取ったデータの末尾の EOF（`0x1A`）を除く。
pub fn strip_ascii_eof(raw: &mut Vec<u8>) {
    if raw.last() == Some(&0x1A) {
        raw.pop();
    }
}

#[cfg(test)]
mod tests;

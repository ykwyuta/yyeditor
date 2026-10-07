//! IND$FILE のホストの側（DFT）。

use std::collections::BTreeMap;
use std::io;
use std::time::Duration;

use yy_encoding::Ccsid;

use crate::codes::*;
use crate::ebcdic::{decode, encode};
use crate::terminal::{Inbound, query_reply, read_input};
use crate::{Dataset, IndFileStyle, Link, Shared};

fn pad(mut v: Vec<u8>, n: usize, with: u8) -> Vec<u8> {
    v.resize(n, with);
    v
}

/// 最初からあるデータセット。
pub(crate) fn initial_datasets(ccsid: Ccsid) -> BTreeMap<String, Dataset> {
    let mut m = BTreeMap::new();
    let fb: Vec<Vec<u8>> = [
        "ＹＹ模擬ホストのテストデータ",
        "LINE 2 ABC 123",
        "ｶﾀｶﾅ ﾃﾞｰﾀ 終わり",
    ]
    .iter()
    .map(|l| pad(encode(ccsid, l), 80, 0x40))
    .collect();
    m.insert(
        "YY.TEST.FB80".into(),
        Dataset {
            recfm: 'F',
            lrecl: 80,
            records: fb,
        },
    );
    let vb: Vec<Vec<u8>> = ["可変長の 1 行目", "", "SHORT", "最後の行 END"]
        .iter()
        .map(|l| encode(ccsid, l))
        .collect();
    m.insert(
        "YY.TEST.VB".into(),
        Dataset {
            recfm: 'V',
            lrecl: 255,
            records: vb,
        },
    );
    m
}

/// IND$FILE のコマンドの指定。
struct Cmd {
    get: bool,
    name: String,
    ascii: bool,
    crlf: bool,
    recfm: Option<char>,
    lrecl: Option<usize>,
    append: bool,
}

fn parse(cmd: &str) -> Option<Cmd> {
    let toks: Vec<String> = cmd
        .replace('(', " ( ")
        .replace(')', " ) ")
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let get = match toks.get(1)?.to_ascii_uppercase().as_str() {
        "GET" => true,
        "PUT" => false,
        _ => return None,
    };
    let name = toks.get(2)?.trim_matches('\'').to_ascii_uppercase();
    let mut c = Cmd {
        get,
        name,
        ascii: false,
        crlf: false,
        recfm: None,
        lrecl: None,
        append: false,
    };
    let up: Vec<String> = toks[3..].iter().map(|t| t.to_ascii_uppercase()).collect();
    let mut i = 0;
    while i < up.len() {
        match up[i].as_str() {
            "ASCII" => c.ascii = true,
            "CRLF" => c.crlf = true,
            "APPEND" => c.append = true,
            "RECFM" => {
                // RECFM(F)・RECFM F
                let v = up[i + 1..]
                    .iter()
                    .find(|t| *t != "(")
                    .cloned()
                    .unwrap_or_default();
                c.recfm = v.chars().next();
            }
            "LRECL" => {
                c.lrecl = up[i + 1..]
                    .iter()
                    .find(|t| *t != "(")
                    .and_then(|t| t.parse().ok());
            }
            _ => {}
        }
        i += 1;
    }
    Some(c)
}

fn to_ascii(ccsid: Ccsid, rec: &[u8]) -> Vec<u8> {
    decode(ccsid, rec)
        .chars()
        .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
        .collect()
}

fn from_ascii(ccsid: Ccsid, b: &[u8]) -> Vec<u8> {
    encode(ccsid, &String::from_utf8_lossy(b))
}

fn split_on(data: &[u8], sep: [u8; 2]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + 1 < data.len() + 1 && i < data.len() {
        if data[i] == sep[0] && data.get(i + 1) == Some(&sep[1]) {
            out.push(data[start..i].to_vec());
            i += 2;
            start = i;
        } else {
            i += 1;
        }
    }
    if start < data.len() {
        out.push(data[start..].to_vec());
    }
    out
}

fn sf(id: &[u8], body: &[u8]) -> Vec<u8> {
    let len = (2 + id.len() + body.len()) as u16;
    let mut v = len.to_be_bytes().to_vec();
    v.extend_from_slice(id);
    v.extend_from_slice(body);
    v
}

fn open_sf(name: &[u8; 7]) -> Vec<u8> {
    // 長さ 0x23: 見出し 5 バイト・中身 23 バイト・名前 7 バイト
    let mut body = vec![
        0x01, 0x06, 0x01, 0x01, 0x03, 0x0A, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x11, 0x01, 0x01, 0x00, 0x50, 0x05, 0x52,
    ];
    body.extend_from_slice(name);
    sf(&[SF_DATA_CHUNK, 0x00, 0x12], &body)
}

fn insert_sfs(data: &[u8]) -> Vec<u8> {
    let mut v = sf(
        &[SF_DATA_CHUNK, 0x47, 0x11],
        &[0x01, 0x05, 0x00, 0x80, 0x00],
    );
    let mut body = vec![0xC0, 0x80, 0x61];
    body.extend_from_slice(&((data.len() + 5) as u16).to_be_bytes());
    body.extend_from_slice(data);
    v.extend(sf(&[SF_DATA_CHUNK, 0x47, 0x04], &body));
    v
}

fn wsf(sfs: &[u8]) -> Vec<u8> {
    let mut v = vec![CMD_WSF];
    v.extend_from_slice(sfs);
    v
}

/// 端末の DFT の返事（D0 の構造化フィールド）を待つ。返事の種類（2 バイト）と全体。
fn reply(link: &mut Link, shared: &Shared) -> io::Result<(u16, Vec<u8>)> {
    loop {
        if let Inbound::Sf(sfs) = read_input(link, shared)?
            && let Some(sf) = sfs
                .into_iter()
                .find(|s| s.len() >= 5 && s[2] == SF_DATA_CHUNK)
        {
            return Ok((u16::from_be_bytes([sf[3], sf[4]]), sf));
        }
    }
}

fn is_error(code: u16) -> bool {
    code & 0xFF == 0x08
}

/// メッセージの段階（`FT:MSG`）。MVS 3.8j 風は Close しない。
fn message(link: &mut Link, shared: &Shared, text: &str) -> io::Result<()> {
    link.send(shared, DT_3270_DATA, &wsf(&open_sf(b"FT:MSG ")))?;
    reply(link, shared)?;
    let mut msg = text.as_bytes().to_vec();
    msg.push(b'$');
    link.send(shared, DT_3270_DATA, &wsf(&insert_sfs(&msg)))?;
    reply(link, shared)?;
    if shared.cfg.ind_file == IndFileStyle::Zos {
        // 端末はメッセージで転送を終えるので、この Close には答えないことがある（x3270 は答えない）。
        // 答えがあれば読み捨てる
        link.send(
            shared,
            DT_3270_DATA,
            &wsf(&sf(&[SF_DATA_CHUNK, 0x41, 0x12], &[])),
        )?;
        while link.recv(shared, Duration::from_millis(300))?.is_some() {}
    }
    Ok(())
}

pub(crate) fn run(link: &mut Link, shared: &Shared, cmd: &str) -> io::Result<Vec<String>> {
    let ccsid = shared.cfg.ccsid;
    let style = shared.cfg.ind_file;
    let Some(c) = parse(cmd) else {
        return Ok(vec![
            "IND$FILE GET|PUT データセット名 [ASCII] [CRLF] [RECFM(F|V)] [LRECL(n)]".into(),
        ]);
    };
    // 端末の受け取れる大きさ（DDM）
    let sfs = query_reply(link, shared)?;
    let outlim = sfs
        .iter()
        .find(|s| s.len() >= 10 && s[2] == 0x81 && s[3] == 0x95)
        .map_or(2048, |s| usize::from(u16::from_be_bytes([s[8], s[9]])));
    let chunk = outlim.saturating_sub(64).max(256);
    let crlf_used = c.crlf && (c.ascii || style == IndFileStyle::Zos);
    let sep = if c.ascii { [0x0D, 0x0A] } else { [0x0D, 0x25] };
    if c.get {
        let Some(ds) = shared.datasets.lock().unwrap().get(&c.name).cloned() else {
            shared.log(format!("ind$file get {} not found", c.name));
            message(link, shared, "TRANS34 Data set not found")?;
            return Ok(vec!["TRANS34 Data set not found".into()]);
        };
        let mut data = Vec::new();
        for r in &ds.records {
            let mut r = if c.ascii {
                to_ascii(ccsid, r)
            } else {
                r.clone()
            };
            if c.ascii && ds.recfm == 'F' {
                while r.last() == Some(&b' ') {
                    r.pop();
                }
            }
            data.extend_from_slice(&r);
            if crlf_used {
                data.extend_from_slice(&sep);
            }
        }
        link.send(shared, DT_3270_DATA, &wsf(&open_sf(b"FT:DATA")))?;
        let (code, _) = reply(link, shared)?;
        if is_error(code) {
            shared.log("ind$file get refused by terminal");
            return Ok(vec!["TRANS13 Error".into()]);
        }
        let mut aborted = false;
        for part in data.chunks(chunk) {
            link.send(shared, DT_3270_DATA, &wsf(&insert_sfs(part)))?;
            let (code, _) = reply(link, shared)?;
            if is_error(code) {
                aborted = true;
                break;
            }
        }
        link.send(
            shared,
            DT_3270_DATA,
            &wsf(&sf(&[SF_DATA_CHUNK, 0x41, 0x12], &[])),
        )?;
        reply(link, shared)?;
        let text = if aborted {
            "TRANS15 Transfer canceled by the terminal"
        } else {
            "TRANS03 File transfer complete"
        };
        shared.log(format!(
            "ind$file get {} bytes={} ascii={} crlf={} {}",
            c.name,
            data.len(),
            c.ascii,
            crlf_used,
            if aborted { "aborted" } else { "ok" }
        ));
        message(link, shared, text)?;
        return Ok(vec![text.into()]);
    }
    // PUT
    link.send(shared, DT_3270_DATA, &wsf(&open_sf(b"FT:DATA")))?;
    let (code, _) = reply(link, shared)?;
    if is_error(code) {
        return Ok(vec!["TRANS13 Error".into()]);
    }
    let mut data = Vec::new();
    let aborted;
    loop {
        let mut req = sf(&[SF_DATA_CHUNK, 0x45, 0x11], &[0x01, 0x05, 0x00, 0x06]);
        req.extend(sf(
            &[SF_DATA_CHUNK, 0x46, 0x11],
            &[0x01, 0x04, 0x00, 0x80, 0x00],
        ));
        link.send(shared, DT_3270_DATA, &wsf(&req))?;
        let (code, sf) = reply(link, shared)?;
        match code {
            0x4605 if sf.len() >= 16 => {
                let n = usize::from(u16::from_be_bytes([sf[14], sf[15]])).saturating_sub(5);
                data.extend_from_slice(&sf[16..(16 + n).min(sf.len())]);
            }
            0x4608 => {
                // EOF（22 00）か取り消し
                let eof = sf.get(7..9) == Some(&[0x22, 0x00]);
                aborted = !eof;
                break;
            }
            c if is_error(c) => {
                aborted = true;
                break;
            }
            _ => {}
        }
    }
    link.send(
        shared,
        DT_3270_DATA,
        &wsf(&sf(&[SF_DATA_CHUNK, 0x41, 0x12], &[])),
    )?;
    reply(link, shared)?;
    if aborted {
        shared.log(format!("ind$file put {} aborted", c.name));
        message(link, shared, "TRANS15 Transfer canceled by the terminal")?;
        return Ok(vec!["TRANS15 Transfer canceled".into()]);
    }
    let old = shared.datasets.lock().unwrap().get(&c.name).cloned();
    let recfm = c.recfm.or(old.as_ref().map(|d| d.recfm)).unwrap_or('V');
    let lrecl = c.lrecl.or(old.as_ref().map(|d| d.lrecl)).unwrap_or(255);
    let raw_records: Vec<Vec<u8>> = if crlf_used {
        split_on(&data, sep)
    } else if recfm == 'F' {
        data.chunks(lrecl.max(1)).map(<[u8]>::to_vec).collect()
    } else {
        vec![data.clone()]
    };
    let mut records: Vec<Vec<u8>> = raw_records
        .into_iter()
        .map(|r| if c.ascii { from_ascii(ccsid, &r) } else { r })
        .map(|r| {
            if recfm == 'F' {
                // z/OS は空白、MVS 3.8j 風は 0x00 で詰める
                let fill = if style == IndFileStyle::Mvs38 && !c.ascii {
                    0x00
                } else {
                    0x40
                };
                pad(r, lrecl, fill)
            } else {
                r
            }
        })
        .collect();
    if c.append
        && let Some(o) = old
    {
        let mut all = o.records;
        all.append(&mut records);
        records = all;
    }
    shared.log(format!(
        "ind$file put {} records={} bytes={} recfm={recfm} lrecl={lrecl} ascii={} crlf={}",
        c.name,
        records.len(),
        data.len(),
        c.ascii,
        crlf_used
    ));
    shared.datasets.lock().unwrap().insert(
        c.name.clone(),
        Dataset {
            recfm,
            lrecl,
            records,
        },
    );
    message(link, shared, "TRANS03 File transfer complete")?;
    Ok(vec!["TRANS03 File transfer complete".into()])
}

//! 端末の業務: ログオンの画面と READY のコマンド。

use std::io;
use std::time::Duration;

use crate::codes::*;
use crate::ebcdic::{Screen, decode, decode_dbcs};
use crate::{Job, Link, Shared, dft};

/// 端末から届いたもの。
pub(crate) enum Inbound {
    /// AID・（フィールドの始まりの位置, 中身）
    Aid {
        aid: u8,
        fields: Vec<(usize, Vec<u8>)>,
    },
    /// 構造化フィールド（AID 0x88 の後ろ。長さから）
    Sf(Vec<Vec<u8>>),
}

pub(crate) fn parse_inbound(d: &[u8]) -> Option<Inbound> {
    let &aid = d.first()?;
    if aid == AID_SF {
        let mut sfs = Vec::new();
        let mut rest = &d[1..];
        while rest.len() >= 3 {
            let len = usize::from(u16::from_be_bytes([rest[0], rest[1]]));
            let len = if len == 0 {
                rest.len()
            } else {
                len.min(rest.len())
            };
            if len < 3 {
                break;
            }
            sfs.push(rest[..len].to_vec());
            rest = &rest[len..];
        }
        return Some(Inbound::Sf(sfs));
    }
    let mut fields: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut i = 3.min(d.len());
    while i < d.len() {
        if d[i] == ORDER_SBA && i + 2 < d.len() {
            fields.push((decode_addr(d[i + 1], d[i + 2]), Vec::new()));
            i += 3;
            continue;
        }
        if fields.is_empty() {
            // フィールドのない画面（SBA なし）
            fields.push((0, Vec::new()));
        }
        fields.last_mut().unwrap().1.push(d[i]);
        i += 1;
    }
    Some(Inbound::Aid { aid, fields })
}

const WAIT: Duration = Duration::from_secs(600);

/// 端末の入力を待つ（応答などは読み飛ばす）。
pub(crate) fn read_input(link: &mut Link, shared: &Shared) -> io::Result<Inbound> {
    loop {
        let Some(rec) = link.recv(shared, WAIT)? else {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "端末の入力がありません",
            ));
        };
        if let Some(i) = parse_inbound(&rec) {
            return Ok(i);
        }
    }
}

// ログオンの画面の位置（24 × 80）
const USERID: (usize, usize) = (3, 19);
const PASSWORD: (usize, usize) = (4, 19);
const NAME: (usize, usize) = (6, 19);
const NOTE: (usize, usize) = (8, 19);

fn logon_screen(shared: &Shared, message: &str) -> Vec<u8> {
    let mut s = Screen::new(shared.cfg.ccsid, 80);
    s.at(1, 1)
        .sfe(&[
            (XA_3270, FA_PROTECT | FA_INTENSIFIED),
            (XA_FOREGROUND, COLOR_TURQUOISE),
        ])
        .text("ＹＹ模擬ホスト　ログオン");
    s.at(3, 2).sf(FA_PROTECT).text("USERID   ===>");
    s.at(USERID.0, USERID.1)
        .sfe(&[
            (XA_3270, 0),
            (XA_HIGHLIGHTING, HL_UNDERSCORE),
            (XA_FOREGROUND, COLOR_GREEN),
        ])
        .ic();
    s.at(3, 28).sf(FA_PROTECT);
    s.at(4, 2).sf(FA_PROTECT).text("PASSWORD ===>");
    s.at(PASSWORD.0, PASSWORD.1).sf(FA_NONDISPLAY);
    s.at(4, 28).sf(FA_PROTECT);
    s.at(6, 2).sf(FA_PROTECT).text("名前 ===>");
    // DBCS のフィールド（2 バイト文字だけ）
    s.at(NAME.0, NAME.1).sfe(&[
        (XA_3270, 0),
        (XA_CHARSET, CS_DBCS),
        (XA_FOREGROUND, COLOR_YELLOW),
    ]);
    s.at(6, 40).sf(FA_PROTECT);
    s.at(8, 2).sf(FA_PROTECT).text("備考 ===>");
    // 混在のフィールド（SO/SI で日本語と英数字）
    s.at(NOTE.0, NOTE.1).sf(0);
    s.at(8, 50).sf(FA_PROTECT);
    s.at(24, 1)
        .sfe(&[(XA_3270, FA_PROTECT), (XA_FOREGROUND, COLOR_WHITE)])
        .text(message);
    s.record(CMD_EW, WCC_RESET | WCC_RESTORE | WCC_RESET_MDT)
}

/// フィールドの中身（フィールド属性の次の位置から）。
fn field<'a>(fields: &'a [(usize, Vec<u8>)], s: &Screen, at: (usize, usize)) -> Option<&'a [u8]> {
    let start = s.addr(at.0, at.1) + 1;
    fields
        .iter()
        .find(|(a, _)| *a == start)
        .map(|(_, d)| d.as_slice())
}

/// READY の画面（フィールドなし。`lines` を出してから READY）。
fn ready_screen(shared: &Shared, lines: &[String]) -> Vec<u8> {
    let mut s = Screen::new(shared.cfg.ccsid, 80);
    let mut row = 1;
    for l in lines.iter().take(22) {
        s.at(row, 1).text(l);
        row += 1;
    }
    s.at(row, 1).text("READY");
    s.at(row + 1, 1).ic();
    s.record(CMD_EW, WCC_RESET | WCC_RESTORE)
}

pub(crate) fn run(link: &mut Link, shared: &Shared) -> io::Result<()> {
    let ccsid = shared.cfg.ccsid;
    let lu = link.lu.clone().unwrap_or_default();
    let layout = Screen::new(ccsid, 80);
    link.send(
        shared,
        DT_3270_DATA,
        &logon_screen(shared, "利用者 ID とパスワードを入れてください"),
    )?;
    // ログオン
    loop {
        match read_input(link, shared)? {
            Inbound::Aid {
                aid: AID_ENTER,
                fields,
                ..
            } => {
                let user = decode(ccsid, field(&fields, &layout, USERID).unwrap_or(&[]));
                let pass = decode(ccsid, field(&fields, &layout, PASSWORD).unwrap_or(&[]));
                let name = decode_dbcs(ccsid, field(&fields, &layout, NAME).unwrap_or(&[]));
                let note = decode(ccsid, field(&fields, &layout, NOTE).unwrap_or(&[]));
                let ok = pass.trim() == shared.cfg.password;
                shared.log(format!(
                    "logon user={} password={} name={} note={} lu={lu}",
                    user.trim(),
                    if ok { "ok" } else { "wrong" },
                    name.trim(),
                    note.trim()
                ));
                if ok {
                    break;
                }
                let mut s = Screen::new(ccsid, 80);
                s.at(24, 2).text("パスワードが違います");
                link.send(
                    shared,
                    DT_3270_DATA,
                    &s.record(CMD_W, WCC_ALARM | WCC_RESTORE),
                )?;
            }
            Inbound::Aid { aid: AID_CLEAR, .. } => {
                link.send(
                    shared,
                    DT_3270_DATA,
                    &logon_screen(shared, "再表示しました"),
                )?;
            }
            Inbound::Aid { aid, .. } => {
                shared.log(format!("aid {aid:02X} on logon"));
                let mut s = Screen::new(ccsid, 80);
                s.at(24, 2).text("Enter を押してください");
                link.send(shared, DT_3270_DATA, &s.record(CMD_W, WCC_RESTORE))?;
            }
            Inbound::Sf(_) => {}
        }
    }
    link.send(
        shared,
        DT_3270_DATA,
        &ready_screen(shared, &["ログオンしました".into()]),
    )?;
    // READY のコマンド
    loop {
        let input = read_input(link, shared)?;
        let Inbound::Aid { aid, fields, .. } = input else {
            continue;
        };
        if aid == AID_CLEAR {
            link.send(shared, DT_3270_DATA, &ready_screen(shared, &[]))?;
            continue;
        }
        let text: String = fields.iter().map(|(_, d)| decode(ccsid, d)).collect();
        // TSO のコマンドの名前はバイトで決まる: IND$FILE は C9 D5 C4 5B C6 C9 D3 C5
        // （CCSID 930 などでは 0x5B は ¥ と表示される）
        let raw: Vec<u8> = fields.iter().flat_map(|(_, d)| d.iter().copied()).collect();
        let ind_file = raw
            .windows(8)
            .any(|w| w == [0xC9, 0xD5, 0xC4, 0x5B, 0xC6, 0xC9, 0xD3, 0xC5]);
        let cmd = text
            .rfind("READY")
            .map_or(text.as_str(), |i| &text[i + 5..])
            .trim()
            .to_owned();
        shared.log(format!("command {cmd:?} lu={lu}"));
        let upper = cmd.to_ascii_uppercase();
        let lines: Vec<String> = if upper == "LOGOFF" {
            link.send(
                shared,
                DT_3270_DATA,
                &ready_screen(shared, &["LOGGED OFF".into()]),
            )?;
            shared.log(format!("logoff lu={lu}"));
            return Ok(());
        } else if upper == "QUERY" {
            query(link, shared)?
        } else if upper == "NIHONGO" {
            nihongo(link, shared)?
        } else if ind_file {
            dft::run(link, shared, &cmd)?
        } else if upper.starts_with("IND$FILE") || upper.starts_with("IND¥FILE") {
            shared.log("ind$file with a wrong byte for $ (not 0x5B)");
            vec![format!(
                "コマンドがありません: {cmd}（$ が 0x5B ではありません）"
            )]
        } else if upper.starts_with("PRINT") {
            print(link, shared, &upper)?
        } else {
            vec![format!("コマンドがありません: {cmd}")]
        };
        link.send(shared, DT_3270_DATA, &ready_screen(shared, &lines))?;
    }
}

/// Read Partition Query を送り、Query Reply を読む。
pub(crate) fn query_reply(link: &mut Link, shared: &Shared) -> io::Result<Vec<Vec<u8>>> {
    link.send(
        shared,
        DT_3270_DATA,
        &[CMD_WSF, 0x00, 0x05, 0x01, 0xFF, 0x02],
    )?;
    loop {
        if let Inbound::Sf(sfs) = read_input(link, shared)?
            && sfs.iter().any(|s| s.len() >= 3 && s[2] == 0x81)
        {
            return Ok(sfs);
        }
    }
}

/// Query Reply の要約（記録・表示する行）。
pub(crate) fn describe_query(sfs: &[Vec<u8>]) -> Vec<String> {
    let mut codes = Vec::new();
    let mut lines = Vec::new();
    for sf in sfs {
        if sf.len() < 4 || sf[2] != 0x81 {
            continue;
        }
        let code = sf[3];
        let body = &sf[4..];
        codes.push(format!("{code:02X}"));
        match code {
            0x81 if body.len() >= 6 => {
                let cols = u16::from_be_bytes([body[2], body[3]]);
                let rows = u16::from_be_bytes([body[4], body[5]]);
                lines.push(format!("usable area {cols}x{rows}"));
            }
            0x85 if body.len() >= 9 => {
                // 記述ごとの最後の 4 バイトが CGCSGID（GCSGID 2 バイト・CPGID 2 バイト）
                let dl = usize::from(body[8]);
                let mut sets = Vec::new();
                if dl > 0 {
                    for d in body[9..].chunks(dl) {
                        if d.len() == dl && dl >= 4 {
                            let g = u16::from_be_bytes([d[dl - 4], d[dl - 3]]);
                            let c = u16::from_be_bytes([d[dl - 2], d[dl - 1]]);
                            sets.push(format!("set {:02X} gcsgid {g} cpgid {c}", d[0]));
                        }
                    }
                }
                for set in sets {
                    lines.push(format!("charset {set}"));
                }
            }
            0x95 if body.len() >= 6 => {
                let inlim = u16::from_be_bytes([body[2], body[3]]);
                let outlim = u16::from_be_bytes([body[4], body[5]]);
                lines.push(format!("ddm {inlim}/{outlim}"));
            }
            0x91 => lines.push("dbcs-asia".into()),
            _ => {}
        }
    }
    let mut out = vec![format!("query codes {}", codes.join(" "))];
    out.extend(lines);
    out
}

fn query(link: &mut Link, shared: &Shared) -> io::Result<Vec<String>> {
    let sfs = query_reply(link, shared)?;
    let lines = describe_query(&sfs);
    for l in &lines {
        shared.log(l.clone());
    }
    Ok(lines)
}

/// 日本語の画面: DBCS のフィールドと混在のフィールドに入れてもらい、読み取った文字を返す。
fn nihongo(link: &mut Link, shared: &Shared) -> io::Result<Vec<String>> {
    let ccsid = shared.cfg.ccsid;
    let mut s = Screen::new(ccsid, 80);
    s.at(1, 1)
        .sf(FA_PROTECT | FA_INTENSIFIED)
        .text("日本語の入力（全角・半角カナ・英数字）");
    s.at(3, 2).sf(FA_PROTECT).text("全角 ===>");
    s.at(3, 19).sfe(&[(XA_3270, 0), (XA_CHARSET, CS_DBCS)]).ic();
    s.at(3, 50).sf(FA_PROTECT);
    s.at(4, 2).sf(FA_PROTECT).text("混在 ===>");
    s.at(4, 19).sf(0);
    s.at(4, 70).sf(FA_PROTECT);
    s.at(6, 2).sf(FA_PROTECT).text("ｶﾀｶﾅ ﾄ ｴｲｽｳｼﾞ ﾉ ﾐﾀﾞｼ ABC123");
    link.send(
        shared,
        DT_3270_DATA,
        &s.record(CMD_EW, WCC_RESET | WCC_RESTORE),
    )?;
    let layout = Screen::new(ccsid, 80);
    loop {
        if let Inbound::Aid {
            aid: AID_ENTER,
            fields,
            ..
        } = read_input(link, shared)?
        {
            let dbcs = decode_dbcs(ccsid, field(&fields, &layout, (3, 19)).unwrap_or(&[]));
            let mixed = decode(ccsid, field(&fields, &layout, (4, 19)).unwrap_or(&[]));
            let raw = field(&fields, &layout, (4, 19))
                .map(|b| {
                    b.iter()
                        .map(|x| format!("{x:02X}"))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            shared.log(format!(
                "nihongo dbcs={} mixed={} mixed_hex={raw}",
                dbcs.trim(),
                mixed.trim()
            ));
            return Ok(vec![
                format!("全角: {}", dbcs.trim()),
                format!("混在: {}", mixed.trim()),
            ]);
        }
    }
}

/// 対応するプリンターに印刷を送る（`PRINT`・`PRINT LU3`）。
fn print(link: &mut Link, shared: &Shared, cmd: &str) -> io::Result<Vec<String>> {
    let Some(term) = link.lu.clone() else {
        return Ok(vec!["TN3270E で接続していないので印刷できません".into()]);
    };
    let Some(printer) = shared.printer_for(&term) else {
        return Ok(vec!["対応するプリンターがありません".into()]);
    };
    let job = if cmd.contains("LU3") {
        Job::Lu3(crate::printer::lu3_sample(shared.cfg.ccsid))
    } else {
        Job::Scs(crate::printer::scs_sample(shared.cfg.ccsid))
    };
    let kind = if cmd.contains("LU3") { "LU3" } else { "SCS" };
    if shared.print(&printer, job) {
        shared.log(format!("print {kind} queued to {printer}"));
        Ok(vec![format!("{printer} に印刷を送りました（{kind}）")])
    } else {
        shared.log(format!("print {kind} no printer {printer}"));
        Ok(vec![format!("プリンター {printer} がつながっていません")])
    }
}

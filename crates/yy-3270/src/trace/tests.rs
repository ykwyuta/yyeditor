//! 通信の記録のテスト。

use super::*;
use crate::{Config, Key, Session};
use yy_encoding::EbcdicCode;

fn e(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut shift = false;
    for c in s.chars() {
        match Ccsid::Ibm930.encode_char(c).unwrap() {
            EbcdicCode::Single(b) => {
                if shift {
                    out.push(FC_SI);
                    shift = false;
                }
                out.push(b);
            }
            EbcdicCode::Double(d) => {
                if !shift {
                    out.push(FC_SO);
                    shift = true;
                }
                out.extend_from_slice(&d.to_be_bytes());
            }
        }
    }
    if shift {
        out.push(FC_SI);
    }
    out
}

fn sb(body: &[u8]) -> Vec<u8> {
    let mut v = vec![IAC, SB];
    v.extend_from_slice(body);
    v.extend_from_slice(&[IAC, SE]);
    v
}

fn rec(data: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    for &b in data {
        v.push(b);
        if b == IAC {
            v.push(IAC);
        }
    }
    v.extend_from_slice(&[IAC, EOR]);
    v
}

/// TN3270E の交渉と、日本語の見出し・利用者 ID・非表示のパスワードの画面。
fn host_bytes() -> Vec<u8> {
    let mut w = vec![IAC, DO, OPT_TN3270E];
    w.extend(sb(&[OPT_TN3270E, E_SEND, E_DEVICE_TYPE]));
    let mut is = vec![OPT_TN3270E, E_DEVICE_TYPE, E_IS];
    is.extend_from_slice(b"IBM-3278-2-E");
    is.push(E_CONNECT);
    is.extend_from_slice(b"TCP00042");
    w.extend(sb(&is));
    w.extend(sb(&[OPT_TN3270E, E_FUNCTIONS, E_IS, FN_RESPONSES]));
    let mut screen = vec![
        DT_3270_DATA,
        0,
        RSP_ALWAYS_RESPONSE,
        0,
        1,
        0xF5,
        WCC_RESTORE,
    ];
    screen.extend([ORDER_SF, FA_PROTECT | FA_INTENSIFIED]);
    screen.extend(e("ログオン"));
    screen.push(ORDER_SBA);
    screen.extend(encode_address(80));
    screen.extend([ORDER_SF, 0x00, ORDER_IC]);
    screen.push(ORDER_SBA);
    screen.extend(encode_address(90));
    screen.extend([ORDER_SF, FA_PROTECT]);
    screen.push(ORDER_SBA);
    screen.extend(encode_address(160));
    screen.extend([ORDER_SF, FA_NONDISPLAY]);
    screen.push(ORDER_SBA);
    screen.extend(encode_address(170));
    screen.extend([ORDER_SFE, 2, XA_3270, FA_PROTECT, XA_FOREGROUND, 0xF2]);
    screen.extend([ORDER_RA]);
    screen.extend(encode_address(180));
    screen.push(e("-")[0]);
    w.extend(rec(&screen));
    w
}

fn traced_session() -> (Session, Vec<TraceEntry>) {
    let mut s = Session::new(Config {
        ccsid: Ccsid::Ibm930,
        ..Config::default()
    });
    s.set_trace(true);
    let mut entries = Vec::new();
    // 少しずつ届いても単位は同じ
    for chunk in host_bytes().chunks(7) {
        entries.extend(s.receive(chunk).trace);
    }
    (s, entries)
}

fn text(entries: &[TraceEntry]) -> String {
    entries.iter().map(|t| t.format("10:00:00.000")).collect()
}

#[test]
fn records_negotiation_screens_and_masks_passwords() {
    let (mut s, mut entries) = traced_session();
    // 利用者 ID を入れ、パスワードのフィールドへ移って入れ、Enter
    for c in "IBMUSER".chars() {
        entries.extend(s.key(Key::Char(c)).trace);
    }
    entries.extend(s.key(Key::MoveTo(161)).trace);
    for c in "SECRET".chars() {
        entries.extend(s.key(Key::Char(c)).trace);
    }
    entries.extend(s.key(Key::Enter).trace);
    let t = text(&entries);
    // 交渉: 受け取り・送り・その順序
    let lines: Vec<&str> = t.lines().filter(|l| l.starts_with("10:")).collect();
    assert_eq!(lines[0], "10:00:00.000 < TEL IAC DO TN3270E (3 バイト)");
    assert_eq!(lines[1], "10:00:00.000 > TEL IAC WILL TN3270E (3 バイト)");
    assert_eq!(
        lines[2],
        "10:00:00.000 < TEL SB TN3270E SEND DEVICE-TYPE (7 バイト)"
    );
    assert!(
        lines[3].starts_with("10:00:00.000 > TEL SB TN3270E DEVICE-TYPE REQUEST IBM-3278-2-E"),
        "{}",
        lines[3]
    );
    assert!(lines[4].contains("SB TN3270E DEVICE-TYPE IS IBM-3278-2-E CONNECT TCP00042"));
    assert!(lines[5].contains("> TEL SB TN3270E FUNCTIONS REQUEST RESPONSES"));
    assert!(lines[6].contains("< TEL SB TN3270E FUNCTIONS IS RESPONSES"));
    assert!(lines[7].contains("< REC 3270-DATA always-response seq=1"));
    // 肯定の応答
    assert!(
        lines[8].contains("> REC RESPONSE positive seq=1"),
        "{}",
        lines[8]
    );
    // 画面の解釈
    assert!(t.contains("| EraseWrite WCC(restore)"), "{t}");
    assert!(t.contains("| SF(protected,intense)  \"ログオン\""), "{t}");
    assert!(t.contains("| SBA 2,1  SF(unprotected)  IC"), "{t}");
    assert!(t.contains("| SBA 3,1  SF(nondisplay)"), "{t}");
    assert!(
        t.contains("SFE(field=protected,fg=red)  RA 3,21 \"-\""),
        "{t}"
    );
    // 送ったもの: 利用者 ID は見え、パスワードは隠す（16 進でも）
    let last = lines.last().unwrap();
    assert!(
        last.contains("> REC 3270-DATA seq=0 AID Enter, カーソル 3,8"),
        "{last}"
    );
    assert!(t.contains("\"IBMUSER\""), "{t}");
    assert!(t.contains("\"******\""), "{t}");
    assert!(!t.contains("SECRET"));
    let secret_hex: Vec<String> = e("SECRET").iter().map(|b| format!("{b:02X}")).collect();
    assert!(!t.contains(&secret_hex.join(" ")), "{t}");
    assert!(t.contains("** ** ** ** ** **"), "{t}");
}

#[test]
fn replays_a_trace_into_the_same_screen() {
    let (s, entries) = traced_session();
    let t = text(&entries);
    let parsed = parse(&t);
    assert_eq!(parsed.len(), entries.len());
    assert!(
        parsed
            .iter()
            .zip(&entries)
            .all(|(p, e)| p.dir == e.dir && p.kind == e.kind)
    );
    // 受け取ったものだけを流し直すと同じ画面になる
    let mut s2 = Session::new(Config {
        ccsid: Ccsid::Ibm930,
        ..Config::default()
    });
    s2.receive(&replay(&t));
    assert_eq!(s2.screen().cells, s.screen().cells);
    assert_eq!(s2.oia().device.as_deref(), Some("TCP00042"));
}

#[test]
fn describes_structured_fields_and_replies() {
    // Read Partition Query と、その応答
    let mut s = Session::new(Config {
        tn3270e: false,
        ..Config::default()
    });
    let mut neg = vec![IAC, DO, OPT_EOR, IAC, WILL, OPT_EOR];
    neg.extend([IAC, DO, OPT_BINARY, IAC, WILL, OPT_BINARY]);
    s.receive(&neg);
    s.set_trace(true);
    let o = s.receive(&rec(&[0xF3, 0x00, 0x05, 0x01, 0xFF, 0x02]));
    let t = text(&o.trace);
    assert!(t.contains("< REC 3270"), "{t}");
    assert!(t.contains("| WriteStructuredField"), "{t}");
    assert!(t.contains("| ReadPartition Query"), "{t}");
    assert!(t.contains("> REC 3270 AID StructuredField"), "{t}");
    assert!(t.contains("| QueryReply Summary"), "{t}");
    assert!(t.contains("| QueryReply DDM"), "{t}");
    // DFT の Open
    let mut open = vec![0xF3, 0x00, 0x23, 0xD0, 0x00, 0x12];
    open.resize(1 + 0x23 - 7, 0);
    open.extend_from_slice(b"FT:DATA");
    let o = s.receive(&rec(&open));
    let t = text(&o.trace);
    assert!(t.contains("| DFT Open \"FT:DATA\""), "{t}");
    // 頼んでいない転送は断る
    assert!(t.contains("| DFT エラー"), "{t}");
    // 記録をやめれば何も出さない
    s.set_trace(false);
    assert!(s.receive(&rec(&[0xF1, WCC_RESTORE])).trace.is_empty());
    assert_eq!(aid_name(pf_aid(13).unwrap()), "PF13");
    assert_eq!(fa_text(FA_PROTECT | FA_NUMERIC), "protected,skip");
}

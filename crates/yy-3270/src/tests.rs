//! Telnet・TN3270E の交渉とセッションのテスト。

use super::*;

fn iac(cmd: u8, opt: u8) -> Vec<u8> {
    vec![IAC, cmd, opt]
}

fn sb(body: &[u8]) -> Vec<u8> {
    let mut v = vec![IAC, SB];
    v.extend_from_slice(body);
    v.extend_from_slice(&[IAC, SE]);
    v
}

fn record(data: &[u8]) -> Vec<u8> {
    let mut v = data.to_vec();
    frame(&mut v);
    v
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// TN3270（TN3270E を使わない）の交渉。
fn tn3270_session() -> Session {
    let mut s = Session::new(Config {
        tn3270e: false,
        ..Config::default()
    });
    let o = s.receive(&iac(DO, OPT_TTYPE));
    assert_eq!(o.send, iac(WILL, OPT_TTYPE));
    let o = s.receive(&sb(&[OPT_TTYPE, TTYPE_SEND]));
    assert_eq!(o.send, sb(b"\x18\x00IBM-3278-2-E"));
    let mut neg = iac(DO, OPT_EOR);
    neg.extend(iac(WILL, OPT_EOR));
    neg.extend(iac(DO, OPT_BINARY));
    neg.extend(iac(WILL, OPT_BINARY));
    let o = s.receive(&neg);
    assert!(contains(&o.send, &iac(WILL, OPT_EOR)));
    assert!(contains(&o.send, &iac(DO, OPT_EOR)));
    assert!(contains(&o.send, &iac(WILL, OPT_BINARY)));
    assert!(contains(&o.send, &iac(DO, OPT_BINARY)));
    assert!(o.events.contains(&Event::Mode(Mode::Tn3270)));
    assert_eq!(s.mode(), Mode::Tn3270);
    s
}

#[test]
fn negotiates_tn3270_and_exchanges_records() {
    let mut s = tn3270_session();
    // 交渉を繰り返されても答えを繰り返さない
    assert!(s.receive(&iac(DO, OPT_BINARY)).send.is_empty());
    // 断るオプション
    assert_eq!(s.receive(&iac(DO, 1)).send, iac(WONT, 1));
    assert_eq!(s.receive(&iac(WILL, 3)).send, iac(DONT, 3));
    // 画面: 非保護のフィールドとカーソル。0xFF のデータは IAC IAC で届く
    let rec = record(&[0xF5, WCC_RESTORE, ORDER_SF, 0x00, ORDER_IC, 0xFF]);
    let o = s.receive(&rec);
    assert!(o.changed);
    assert_eq!(s.screen().cells[1].b, 0xFF);
    assert_eq!(s.oia().lock, Lock::None);
    // 少しずつ届いても同じ
    let mut s2 = tn3270_session();
    for b in &rec {
        s2.receive(std::slice::from_ref(b));
    }
    assert_eq!(s2.screen().cells, s.screen().cells);
    // Enter: AID・カーソル・変更したフィールド（0xFF は IAC IAC）、IAC EOR で終わる
    s.key(Key::Right);
    s.key(Key::Char('A'));
    let o = s.key(Key::Enter);
    assert_eq!(o.send.last_chunk::<2>(), Some(&[IAC, EOR]));
    assert_eq!(o.send[0], codes::AID_ENTER);
    assert!(contains(&o.send, &[0xFF, 0xFF]));
    assert_eq!(s.oia().lock, Lock::System);
}

#[test]
fn negotiates_tn3270e_with_a_named_lu() {
    let mut s = Session::new(Config {
        lu: Some("TCP00042".into()),
        ..Config::default()
    });
    assert_eq!(
        s.receive(&iac(DO, OPT_TN3270E)).send,
        iac(WILL, OPT_TN3270E)
    );
    let o = s.receive(&sb(&[OPT_TN3270E, E_SEND, E_DEVICE_TYPE]));
    let mut want = vec![OPT_TN3270E, E_DEVICE_TYPE, E_REQUEST];
    want.extend_from_slice(b"IBM-3278-2-E");
    want.push(E_CONNECT);
    want.extend_from_slice(b"TCP00042");
    assert_eq!(o.send, sb(&want));
    let mut is = vec![OPT_TN3270E, E_DEVICE_TYPE, E_IS];
    is.extend_from_slice(b"IBM-3278-2-E");
    is.push(E_CONNECT);
    is.extend_from_slice(b"TCP00042");
    let o = s.receive(&sb(&is));
    assert!(o.events.contains(&Event::Device("TCP00042".into())));
    assert_eq!(
        o.send,
        sb(&[OPT_TN3270E, E_FUNCTIONS, E_REQUEST, FN_RESPONSES])
    );
    let o = s.receive(&sb(&[OPT_TN3270E, E_FUNCTIONS, E_IS, FN_RESPONSES]));
    assert!(o.events.contains(&Event::Mode(Mode::Tn3270e)));
    assert_eq!(s.oia().device.as_deref(), Some("TCP00042"));
    // ヘッダーつきの画面。応答を求められたら肯定の応答
    let rec = record(&[
        DT_3270_DATA,
        0,
        RSP_ALWAYS_RESPONSE,
        0x00,
        0x07,
        0xF5,
        WCC_RESTORE,
        ORDER_SF,
        0x00,
        ORDER_IC,
    ]);
    let o = s.receive(&rec);
    assert!(o.changed);
    assert_eq!(
        o.send,
        record(&[DT_RESPONSE, 0, RSP_POSITIVE, 0x00, 0x07, 0x00])
    );
    // 送るデータにもヘッダー
    let o = s.key(Key::Enter);
    assert_eq!(&o.send[..6], &[DT_3270_DATA, 0, 0, 0, 0, codes::AID_ENTER]);
}

#[test]
fn reports_rejected_devices_and_falls_back() {
    let mut s = Session::new(Config {
        lu: Some("NOSUCH".into()),
        ..Config::default()
    });
    s.receive(&iac(DO, OPT_TN3270E));
    let o = s.receive(&sb(&[OPT_TN3270E, E_DEVICE_TYPE, E_REJECT, E_REASON, 3]));
    assert!(matches!(&o.events[0], Event::DeviceRejected(r) if r.contains("INV-NAME")));
    // サーバーが TN3270E をやめれば TN3270 で続ける
    let o = s.receive(&iac(DONT, OPT_TN3270E));
    assert_eq!(o.send, iac(WONT, OPT_TN3270E));
    let mut neg = iac(DO, OPT_EOR);
    neg.extend(iac(WILL, OPT_EOR));
    neg.extend(iac(DO, OPT_BINARY));
    neg.extend(iac(WILL, OPT_BINARY));
    s.receive(&neg);
    assert_eq!(s.mode(), Mode::Tn3270);
}

#[test]
fn shows_text_before_3270_and_ignores_keys_while_negotiating() {
    let mut s = Session::new(Config::default());
    let o = s.receive(b"Too many sessions\r\n");
    assert_eq!(o.events, vec![Event::Text("Too many sessions".into())]);
    assert!(s.key(Key::Enter).send.is_empty());
}

#[test]
fn query_reply_is_framed_for_the_host() {
    let mut s = tn3270_session();
    let o = s.receive(&record(&[0xF3, 0x00, 0x05, 0x01, 0xFF, 0x02]));
    assert_eq!(o.send[0], codes::AID_SF);
    assert_eq!(o.send.last_chunk::<2>(), Some(&[IAC, EOR]));
}

#[test]
fn printer_session_assembles_scs_jobs() {
    let mut s = Session::new(Config {
        printer: true,
        associate: Some("TCP00042".into()),
        ccsid: Ccsid::Ibm037,
        ..Config::default()
    });
    s.receive(&iac(DO, OPT_TN3270E));
    let o = s.receive(&sb(&[OPT_TN3270E, E_SEND, E_DEVICE_TYPE]));
    let mut want = vec![OPT_TN3270E, E_DEVICE_TYPE, E_REQUEST];
    want.extend_from_slice(b"IBM-3287-1");
    want.push(E_ASSOCIATE);
    want.extend_from_slice(b"TCP00042");
    assert_eq!(o.send, sb(&want));
    let mut is = vec![OPT_TN3270E, E_DEVICE_TYPE, E_IS];
    is.extend_from_slice(b"IBM-3287-1");
    is.push(E_CONNECT);
    is.extend_from_slice(b"TCP00043");
    let o = s.receive(&sb(&is));
    assert_eq!(
        o.send,
        sb(&[
            OPT_TN3270E,
            E_FUNCTIONS,
            E_REQUEST,
            FN_RESPONSES,
            FN_SCS_CTL_CODES,
            FN_DATA_STREAM_CTL
        ])
    );
    s.receive(&sb(&[
        OPT_TN3270E,
        E_FUNCTIONS,
        E_IS,
        FN_RESPONSES,
        FN_SCS_CTL_CODES,
    ]));
    assert_eq!(s.mode(), Mode::Tn3270e);
    // SCS のデータ（応答を求める）: "HI" NL "OK"
    let o = s.receive(&record(&[
        DT_SCS_DATA,
        0,
        RSP_ALWAYS_RESPONSE,
        0,
        9,
        0xC8,
        0xC9,
        0x15,
        0xD6,
        0xD2,
    ]));
    assert_eq!(o.send, record(&[DT_RESPONSE, 0, RSP_POSITIVE, 0, 9, 0x00]));
    assert!(o.events.is_empty());
    assert!(s.printing());
    // ジョブの終わり
    let o = s.receive(&record(&[DT_PRINT_EOJ, 0, RSP_NO_RESPONSE, 0, 10]));
    let Some(Event::PrintJob(job)) = o.events.first() else {
        panic!("{:?}", o.events)
    };
    assert_eq!(job.pages, vec![vec!["HI".to_owned(), "OK".to_owned()]]);
    assert!(!s.printing());
    assert!(s.flush_print().is_none());
    // PRINT-EOJ のないホスト: 呼び出し側が時間で区切る
    s.receive(&record(&[DT_SCS_DATA, 0, 0, 0, 11, 0xC1]));
    assert_eq!(s.flush_print().unwrap().pages, vec![vec!["A".to_owned()]]);
}

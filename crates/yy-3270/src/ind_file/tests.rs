//! IND$FILE（DFT）のテスト。

use std::sync::{Arc, Mutex};

use super::*;

/// 中身を後から見られる書き込み先。
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// ホストの構造化フィールド（長さから）。
fn sf(code: u16, body: &[u8]) -> Vec<u8> {
    let mut v = vec![0, 0, SF_DATA_CHUNK];
    v.extend_from_slice(&code.to_be_bytes());
    v.extend_from_slice(body);
    let len = v.len() as u16;
    v[..2].copy_from_slice(&len.to_be_bytes());
    v
}

/// Open（長さ 0x23、名前は最後の 7 バイト）。
fn open(name: &[u8; 7]) -> Vec<u8> {
    let mut body = vec![0u8; 0x23 - 5 - 7];
    body.extend_from_slice(name);
    let v = sf(OPEN_REQ, &body);
    assert_eq!(v.len(), 0x23);
    v
}

fn data_insert(data: &[u8]) -> Vec<u8> {
    let mut body = NOT_COMPRESSED.to_be_bytes().to_vec();
    body.push(BEGIN_DATA);
    body.extend_from_slice(&((data.len() + 5) as u16).to_be_bytes());
    body.extend_from_slice(data);
    sf(DATA_INSERT, &body)
}

/// 結果のメッセージの段階。
fn message(dft: &mut Dft, text: &str) {
    assert_eq!(dft.handle(&open(b"FT:MSG ")), Some(reply(OPEN_REPLY, &[])));
    dft.handle(&sf(INSERT_REQ, &[]));
    let mut t = text.as_bytes().to_vec();
    t.push(b'$');
    dft.handle(&data_insert(&t)).unwrap();
    assert_eq!(
        dft.handle(&sf(CLOSE_REQ, &[])),
        Some(reply(CLOSE_REPLY, &[]))
    );
}

#[test]
fn receives_a_file() {
    let out = Shared::default();
    let mut dft = Dft::default();
    dft.start(Local::Sink(Box::new(out.clone())));
    assert!(dft.active());
    // 開く: [88, 00 05, D0, 00 09]
    assert_eq!(
        dft.handle(&open(b"FT:DATA")).unwrap(),
        vec![AID_SF, 0x00, 0x05, 0xD0, 0x00, 0x09]
    );
    for (i, chunk) in [&b"\xC1\xC2"[..], b"\xFF\x0D\x25"].iter().enumerate() {
        assert_eq!(dft.handle(&sf(INSERT_REQ, &[])), None);
        let ack = dft.handle(&data_insert(chunk)).unwrap();
        // [88, 00 0B, D0, 47 05, 63 06, 番号]
        let mut want = vec![AID_SF, 0x00, 0x0B, 0xD0, 0x47, 0x05, 0x63, 0x06];
        want.extend_from_slice(&(i as u32 + 1).to_be_bytes());
        assert_eq!(ack, want);
    }
    assert_eq!(
        dft.handle(&sf(CLOSE_REQ, &[])).unwrap(),
        vec![AID_SF, 0x00, 0x05, 0xD0, 0x41, 0x09]
    );
    message(&mut dft, "TRANS03 File transfer complete");
    assert!(!dft.active());
    assert_eq!(*out.0.lock().unwrap(), b"\xC1\xC2\xFF\x0D\x25");
    let ev = dft.take_events();
    assert_eq!(ev[0], FtEvent::Started);
    assert!(ev.contains(&FtEvent::Progress(5)));
    assert_eq!(
        ev.last(),
        Some(&FtEvent::Done {
            ok: true,
            message: "TRANS03 File transfer complete".into()
        })
    );
}

#[test]
fn sends_a_file_in_chunks() {
    let data: Vec<u8> = (0..5000u32).map(|i| i as u8).collect();
    let mut dft = Dft::default();
    dft.start(Local::Source(Box::new(std::io::Cursor::new(data.clone()))));
    dft.handle(&open(b"FT:DATA")).unwrap();
    let mut got = Vec::new();
    let mut rec = 1u32;
    loop {
        assert_eq!(dft.handle(&sf(SET_CUR_REQ, &[])), None);
        let r = dft.handle(&sf(GET_REQ, &[])).unwrap();
        assert_eq!(&r[..4], &[AID_SF, r[1], r[2], 0xD0]);
        assert_eq!(usize::from(u16::from_be_bytes([r[1], r[2]])), r.len() - 1);
        assert!(r.len() <= BUFFER_SIZE);
        if r[4..6] == [0x46, 0x08] {
            // 終わり: [46 08, 69 04, 22 00]
            assert_eq!(&r[6..], &[0x69, 0x04, 0x22, 0x00]);
            break;
        }
        assert_eq!(&r[4..8], &[0x46, 0x05, 0x63, 0x06]);
        assert_eq!(&r[8..12], &rec.to_be_bytes());
        assert_eq!(&r[12..15], &[0xC0, 0x80, 0x61]);
        let n = usize::from(u16::from_be_bytes([r[15], r[16]])) - 5;
        assert_eq!(r.len(), 17 + n);
        got.extend_from_slice(&r[17..]);
        rec += 1;
    }
    assert_eq!(got, data);
    assert_eq!(rec, 3);
    dft.handle(&sf(CLOSE_REQ, &[])).unwrap();
    message(
        &mut dft,
        "TRANS13 Error writing file to host: file transfer canceled",
    );
    let ev = dft.take_events();
    assert!(
        matches!(ev.last(), Some(FtEvent::Done { ok: false, message }) if message.starts_with("TRANS13"))
    );
}

#[test]
fn cancels_and_refuses_unrequested_transfers() {
    // 頼んでいない転送は失敗で返す
    let mut dft = Dft::default();
    let r = dft.handle(&open(b"FT:DATA")).unwrap();
    assert_eq!(&r[3..], &[0xD0, 0x00, 0x08, 0x69, 0x04, 0x01, 0x00]);
    // 始まる前の取り消しはすぐ終わる
    dft.start(Local::Sink(Box::new(Shared::default())));
    dft.cancel();
    assert!(!dft.active());
    assert!(matches!(
        &dft.take_events()[..],
        [FtEvent::Done { ok: false, .. }]
    ));
    // 始まった後の取り消しは次の要求に失敗を返し、ホストのメッセージで終わる
    dft.start(Local::Sink(Box::new(Shared::default())));
    dft.handle(&open(b"FT:DATA")).unwrap();
    dft.cancel();
    assert!(dft.active());
    let r = dft.handle(&data_insert(b"x")).unwrap();
    assert_eq!(&r[4..], &[0x47, 0x08, 0x69, 0x04, 0x01, 0x00]);
    dft.handle(&sf(CLOSE_REQ, &[])).unwrap();
    message(&mut dft, "TRANS15 Transfer canceled");
    assert!(
        matches!(dft.take_events().last(), Some(FtEvent::Done { ok: false, message }) if message.contains("取り消し"))
    );
}

#[test]
fn builds_commands() {
    let mut r = Request {
        host: HostKind::Tso,
        direction: Direction::Receive,
        host_file: "'USER.JCL(TEST)'".into(),
        mode: Mode::Text(Ccsid::Ibm930),
        recfm: Recfm::Default,
        lrecl: 0,
        space: 0,
        append: false,
    };
    assert_eq!(r.command(), "IND$FILE GET 'USER.JCL(TEST)' CRLF");
    r.mode = Mode::HostAscii;
    assert_eq!(r.command(), "IND$FILE GET 'USER.JCL(TEST)' ASCII CRLF");
    r.direction = Direction::Send;
    r.mode = Mode::Binary;
    r.recfm = Recfm::Fixed;
    r.lrecl = 80;
    r.space = 10;
    assert_eq!(
        r.command(),
        "IND$FILE PUT 'USER.JCL(TEST)' RECFM(F) LRECL(80) SPACE(10,5) TRACKS"
    );
    r.host = HostKind::Cms;
    r.host_file = "PROFILE EXEC A".into();
    r.mode = Mode::Text(Ccsid::Ibm930);
    r.recfm = Recfm::Variable;
    r.lrecl = 255;
    r.append = true;
    assert_eq!(
        r.command(),
        "IND$FILE PUT PROFILE EXEC A (CRLF APPEND RECFM V LRECL 255"
    );
    r.direction = Direction::Receive;
    r.mode = Mode::Binary;
    assert_eq!(r.command(), "IND$FILE GET PROFILE EXEC A");
}

#[test]
fn converts_japanese_records_locally() {
    let text = "日本語 ABC\r\nｶﾅ\n\n末尾  \n";
    let raw = text_to_records(text, Ccsid::Ibm930, None).unwrap();
    // 1 行目: SO 日本語 SI 空白 ABC CR LF
    assert_eq!(raw[0], 0x0E);
    assert!(raw.windows(2).filter(|w| *w == [0x0D, 0x25]).count() == 4);
    let back = records_to_text(&raw, Ccsid::Ibm930, 0);
    // 末尾の空白は除く（固定長のレコードの詰め物）
    assert_eq!(back, "日本語 ABC\r\nｶﾅ\r\n\r\n末尾\r\n");
    // 固定長のホストが詰めた空白、ASCII の LF も区切りとして読む
    assert_eq!(
        records_to_text(b"\xC1\x40\x40\x0D\x0A\xC2", Ccsid::Ibm037, 0),
        "A\r\nB\r\n"
    );
    // 変換できない文字は行番号つきで断る
    assert_eq!(
        text_to_records("ok\n😀\n", Ccsid::Ibm930, None),
        Err(LineError::Unmappable {
            line: 2,
            text: "😀".into()
        })
    );
    // レコード長を超える行（SO・SI を含めて数える）は切り詰めずに断る
    assert!(text_to_records("ABCD\n", Ccsid::Ibm930, Some(4)).is_ok());
    assert_eq!(
        text_to_records("ok\n日本\n", Ccsid::Ibm930, Some(5)),
        Err(LineError::TooLong {
            line: 2,
            len: 6,
            limit: 5
        })
    );
    let mut req = Request {
        host: HostKind::Tso,
        direction: Direction::Send,
        host_file: "X".into(),
        mode: Mode::Binary,
        recfm: Recfm::Variable,
        lrecl: 255,
        space: 0,
        append: false,
    };
    assert_eq!(req.record_limit(), Some(251));
    req.recfm = Recfm::Fixed;
    assert_eq!(req.record_limit(), Some(255));
    req.direction = Direction::Receive;
    assert_eq!(req.record_limit(), None);
    // 区切りのない固定長のレコード（MVS 3.8j の IND$FILE）: 80 の倍数なら 80 バイトずつ
    let mut fb = vec![0x40u8; 160];
    fb[0] = 0xC1;
    fb[80] = 0xC2;
    assert_eq!(records_to_text(&fb, Ccsid::Ibm037, 0), "A\r\nB\r\n");
    // レコード長を指定すればその長さで
    assert_eq!(
        records_to_text(&fb[..120], Ccsid::Ibm037, 40),
        "A\r\n\r\nB\r\n"
    );
    let mut v = b"abc\x1A".to_vec();
    strip_ascii_eof(&mut v);
    assert_eq!(v, b"abc");
}

#[test]
fn completes_on_the_message_without_a_close() {
    // MVS 3.8j の IND$FILE 2.0.5: メッセージの段階を閉じずに READY に戻る
    let out = Shared::default();
    let mut dft = Dft::default();
    dft.start(Local::Sink(Box::new(out.clone())));
    dft.handle(&open(b"FT:DATA")).unwrap();
    dft.handle(&data_insert(b"\xC1")).unwrap();
    dft.handle(&sf(CLOSE_REQ, &[])).unwrap();
    dft.handle(&open(b"FT:MSG ")).unwrap();
    let mut msg = b"TRANS03   File transfer complete$".to_vec();
    msg.resize(85, b' ');
    dft.handle(&data_insert(&msg)).unwrap();
    assert!(!dft.active());
    let ev = dft.take_events();
    assert_eq!(
        ev.last(),
        Some(&FtEvent::Done {
            ok: true,
            message: "TRANS03   File transfer complete".into()
        })
    );
    // 後から Close が来ても 2 度は知らせない
    dft.handle(&sf(CLOSE_REQ, &[])).unwrap();
    assert!(dft.take_events().is_empty());
}

#[test]
fn fixed_length_text_does_not_rely_on_crlf() {
    let mut r = Request {
        host: HostKind::Tso,
        direction: Direction::Send,
        host_file: "'U.FB'".into(),
        mode: Mode::Text(Ccsid::Ibm930),
        recfm: Recfm::Fixed,
        lrecl: 10,
        space: 0,
        append: false,
    };
    // 固定長のテキストは CRLF を付けず、LRECL まで空白で詰める
    assert_eq!(r.fixed_text(), Some(10));
    assert_eq!(r.command(), "IND$FILE PUT 'U.FB' RECFM(F) LRECL(10)");
    let up = encode_upload(&r, "AB\n日本\n", Ccsid::Ibm930).unwrap();
    assert_eq!(up.len(), 20);
    assert_eq!(&up[..3], &[0xC1, 0xC2, 0x40]);
    assert_eq!(up[10], 0x0E);
    assert_eq!(&up[15..], &[0x0F, 0x40, 0x40, 0x40, 0x40]);
    // 長すぎる行は切り詰めずに断る
    assert!(matches!(
        encode_upload(&r, "ABCDEFGHIJK\n", Ccsid::Ibm930),
        Err(LineError::TooLong {
            line: 1,
            len: 11,
            limit: 10
        })
    ));
    // 受け取りは LRECL ごとに分ける
    r.direction = Direction::Receive;
    assert_eq!(r.command(), "IND$FILE GET 'U.FB'");
    assert_eq!(decode_download(&r, &up, Ccsid::Ibm930), "AB\r\n日本\r\n");
    // 可変長・LRECL なしは CRLF に頼る
    r.lrecl = 0;
    assert_eq!(r.fixed_text(), None);
    assert_eq!(r.command(), "IND$FILE GET 'U.FB' CRLF");
    r.direction = Direction::Send;
    r.recfm = Recfm::Variable;
    r.lrecl = 255;
    assert_eq!(r.fixed_text(), None);
    assert!(r.command().contains("CRLF"));
}

#[test]
fn gives_up_when_the_host_never_starts() {
    let mut dft = Dft::default();
    dft.start(Local::Sink(Box::new(Shared::default())));
    assert!(!dft.check_start(std::time::Duration::from_secs(30)));
    assert!(dft.active());
    assert!(dft.check_start(std::time::Duration::ZERO));
    assert!(!dft.active());
    assert!(
        matches!(&dft.take_events()[..], [FtEvent::Done { ok: false, message }] if message.contains("始めませんでした"))
    );
    // 始まった転送はやめない
    dft.start(Local::Sink(Box::new(Shared::default())));
    dft.handle(&open(b"FT:DATA")).unwrap();
    assert!(!dft.check_start(std::time::Duration::ZERO));
    assert!(dft.active());
}

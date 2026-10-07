//! 模擬ホストとの往復（TCP の上で TN3270E の交渉→画面→入力→Read Modified→次の画面）。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use yy_3270::codes::*;
use yy_3270::ind_file::{self, Direction, HostKind, Local, Recfm, Request};
use yy_3270::{Config, Event, FtEvent, Key, Lock, Mode, Session};
use yy_encoding::{Ccsid, EbcdicCode};

fn e(s: &str) -> Vec<u8> {
    // 続く 2 バイト文字は 1 組の SO〜SI で囲む
    let mut out = Vec::new();
    let mut shift = false;
    for c in s.chars() {
        match Ccsid::Ibm930.encode_char(c) {
            Some(EbcdicCode::Single(b)) => {
                if shift {
                    out.push(FC_SI);
                    shift = false;
                }
                out.push(b);
            }
            Some(EbcdicCode::Double(d)) => {
                if !shift {
                    out.push(FC_SO);
                    shift = true;
                }
                out.extend_from_slice(&d.to_be_bytes());
            }
            None => panic!("{c}"),
        }
    }
    if shift {
        out.push(FC_SI);
    }
    out
}

/// IAC を二重にして IAC EOR で終える。
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

/// レコード（IAC EOR まで）を 1 つ読む（Telnet の命令は飛ばす）。
fn read_record(s: &mut TcpStream) -> Vec<u8> {
    let mut out = Vec::new();
    let mut b = [0u8; 1];
    loop {
        s.read_exact(&mut b).unwrap();
        if b[0] == IAC {
            s.read_exact(&mut b).unwrap();
            match b[0] {
                IAC => out.push(IAC),
                EOR => return out,
                SB => {
                    // サブネゴシエーションは IAC SE まで読み飛ばす
                    let mut prev = 0;
                    loop {
                        s.read_exact(&mut b).unwrap();
                        if prev == IAC && b[0] == SE {
                            break;
                        }
                        prev = b[0];
                    }
                }
                DO | DONT | WILL | WONT => {
                    s.read_exact(&mut b).unwrap();
                }
                _ => {}
            }
        } else {
            out.push(b[0]);
        }
    }
}

/// TN3270E のホスト: LU を割り当てて、ログオン画面を送り、入力を受けて次の画面を送る。
fn host(listener: TcpListener, got: mpsc::Sender<Vec<u8>>) {
    let (mut s, _) = listener.accept().unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut send = |b: &[u8]| s.write_all(b).unwrap();
    send(&[IAC, DO, OPT_TN3270E]);
    send(&[IAC, SB, OPT_TN3270E, E_SEND, E_DEVICE_TYPE, IAC, SE]);
    let mut is = vec![IAC, SB, OPT_TN3270E, E_DEVICE_TYPE, E_IS];
    is.extend_from_slice(b"IBM-3278-2-E");
    is.push(E_CONNECT);
    is.extend_from_slice(b"YYLU0001");
    is.extend_from_slice(&[IAC, SE]);
    send(&is);
    send(&[
        IAC,
        SB,
        OPT_TN3270E,
        E_FUNCTIONS,
        E_IS,
        FN_RESPONSES,
        IAC,
        SE,
    ]);
    // ログオン画面（日本語の見出し、非保護のフィールド）
    let mut screen = vec![DT_3270_DATA, 0, RSP_NO_RESPONSE, 0, 1, 0xF5, WCC_RESTORE];
    screen.extend([ORDER_SF, FA_PROTECT]);
    screen.extend(e("ログオン 利用者"));
    screen.extend([ORDER_SF, 0x00, ORDER_IC]);
    screen.push(ORDER_SBA);
    screen.extend_from_slice(&encode_address(40));
    screen.extend([ORDER_SF, FA_PROTECT]);
    let mut s2 = s.try_clone().unwrap();
    s2.write_all(&rec(&screen)).unwrap();
    // 入力（Read Modified）を受け取って返す
    let r = read_record(&mut s2);
    got.send(r).unwrap();
    // 次の画面（応答を求める）
    let mut next = vec![
        DT_3270_DATA,
        0,
        RSP_ALWAYS_RESPONSE,
        0,
        2,
        0xF5,
        WCC_RESTORE,
    ];
    next.extend(e("READY"));
    s2.write_all(&rec(&next)).unwrap();
    let r = read_record(&mut s2);
    got.send(r).unwrap();
}

#[test]
fn round_trip_with_a_mock_tn3270e_host() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let h = std::thread::spawn(move || host(listener, tx));
    let mut sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut session = Session::new(Config {
        ccsid: Ccsid::Ibm930,
        ..Config::default()
    });
    let mut buf = [0u8; 4096];
    // 画面が届くまで受け取る
    let mut pump =
        |session: &mut Session, sock: &mut TcpStream, until: &dyn Fn(&Session) -> bool| {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !until(session) {
                assert!(std::time::Instant::now() < deadline, "時間切れ");
                match sock.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let o = session.receive(&buf[..n]);
                        sock.write_all(&o.send).unwrap();
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(e) => panic!("{e}"),
                }
            }
        };
    pump(&mut session, &mut sock, &|s| {
        s.mode() == Mode::Tn3270e && s.oia().lock == Lock::None
    });
    assert_eq!(session.oia().device.as_deref(), Some("YYLU0001"));
    let lines = session.screen().text_lines(Ccsid::Ibm930);
    // SI・空白・SO がそれぞれ 1 桁を占める
    assert_eq!(lines[0], "  ログオン   利用者");
    // 日本語を入れて Enter
    for c in "山田".chars() {
        session.key(Key::Char(c));
    }
    let o = session.key(Key::Enter);
    sock.write_all(&o.send).unwrap();
    let got = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    // TN3270E のヘッダー、AID、カーソル、SBA、SO 山田 SI
    assert_eq!(&got[..5], &[DT_3270_DATA, 0, 0, 0, 0]);
    assert_eq!(got[5], AID_ENTER);
    let field = e("山田");
    assert!(got.windows(field.len()).any(|w| w == field), "{got:02X?}");
    // 次の画面と、肯定の応答
    pump(&mut session, &mut sock, &|s| {
        s.screen().text_lines(Ccsid::Ibm930)[0].starts_with("READY")
    });
    let resp = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(resp, vec![DT_RESPONSE, 0, RSP_POSITIVE, 0, 2, 0x00]);
    h.join().unwrap();
}

/// 3270 のデータ（TN3270E のヘッダーつき）のレコード。
fn data_rec(seq: u16, body: &[u8]) -> Vec<u8> {
    let mut v = vec![DT_3270_DATA, 0, RSP_NO_RESPONSE];
    v.extend_from_slice(&seq.to_be_bytes());
    v.extend_from_slice(body);
    rec(&v)
}

/// WSF の DFT（長さ・0xD0・要求・中身）。
fn dft(code: u16, body: &[u8]) -> Vec<u8> {
    let mut sf = vec![0, 0, SF_DATA_CHUNK];
    sf.extend_from_slice(&code.to_be_bytes());
    sf.extend_from_slice(body);
    let len = sf.len() as u16;
    sf[..2].copy_from_slice(&len.to_be_bytes());
    let mut v = vec![0xF3];
    v.extend(sf);
    v
}

fn dft_open(name: &[u8; 7]) -> Vec<u8> {
    let mut body = vec![0u8; 0x23 - 5 - 7];
    body.extend_from_slice(name);
    dft(0x0012, &body)
}

fn dft_insert(data: &[u8]) -> Vec<u8> {
    let mut body = vec![0xC0, 0x80, 0x61];
    body.extend_from_slice(&((data.len() + 5) as u16).to_be_bytes());
    body.extend_from_slice(data);
    dft(0x4704, &body)
}

/// TSO の READY の画面（フィールドなし）で IND$FILE GET を受け、日本語のレコードを送る。
fn tso_host(listener: TcpListener, got: mpsc::Sender<Vec<u8>>) {
    let (s, _) = listener.accept().unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut w = s.try_clone().unwrap();
    let mut r = s;
    let mut neg = vec![IAC, DO, OPT_TN3270E];
    neg.extend([IAC, SB, OPT_TN3270E, E_DEVICE_TYPE, E_IS]);
    neg.extend_from_slice(b"IBM-3278-2-E");
    neg.extend([IAC, SE]);
    neg.extend([IAC, SB, OPT_TN3270E, E_FUNCTIONS, E_IS, IAC, SE]);
    w.write_all(&neg).unwrap();
    let mut ready = vec![0xF5, WCC_RESTORE];
    ready.extend(e("READY"));
    ready.push(ORDER_SBA);
    ready.extend_from_slice(&encode_address(80));
    ready.push(ORDER_IC);
    w.write_all(&data_rec(1, &ready)).unwrap();
    // コマンド（Read Modified）
    got.send(read_record(&mut r)).unwrap();
    let mut send_and_ack = |w: &mut TcpStream, seq: u16, body: Vec<u8>| -> Vec<u8> {
        w.write_all(&data_rec(seq, &body)).unwrap();
        read_record(&mut r)
    };
    let mut acks = Vec::new();
    acks.push(send_and_ack(&mut w, 2, dft_open(b"FT:DATA")));
    w.write_all(&data_rec(3, &dft(0x4711, &[]))).unwrap();
    let mut records = e("日本語のデータ");
    records.extend([0x0D, 0x25]);
    records.extend(e("ABC  "));
    records.extend([0x0D, 0x25]);
    acks.push(send_and_ack(&mut w, 4, dft_insert(&records)));
    acks.push(send_and_ack(&mut w, 5, dft(0x4112, &[])));
    acks.push(send_and_ack(&mut w, 6, dft_open(b"FT:MSG ")));
    acks.push(send_and_ack(
        &mut w,
        7,
        dft_insert(b"TRANS03 File transfer complete$"),
    ));
    acks.push(send_and_ack(&mut w, 8, dft(0x4112, &[])));
    for a in acks {
        got.send(a).unwrap();
    }
}

#[derive(Clone, Default)]
struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn ind_file_get_with_a_mock_tso_host() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let h = std::thread::spawn(move || tso_host(listener, tx));
    let mut sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut session = Session::new(Config {
        ccsid: Ccsid::Ibm930,
        ..Config::default()
    });
    let mut events = Vec::new();
    let mut buf = [0u8; 4096];
    let mut pump = |session: &mut Session,
                    events: &mut Vec<Event>,
                    until: &dyn Fn(&Session, &[Event]) -> bool| {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !until(session, events) {
            assert!(std::time::Instant::now() < deadline, "時間切れ");
            match sock.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let o = session.receive(&buf[..n]);
                    sock.write_all(&o.send).unwrap();
                    events.extend(o.events);
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => panic!("{e}"),
            }
        }
        sock.try_clone().unwrap()
    };
    let mut w = pump(&mut session, &mut events, &|s, _| {
        s.mode() == Mode::Tn3270e && s.oia().lock == Lock::None
    });
    let file = Shared::default();
    let req = Request {
        host: HostKind::Tso,
        direction: Direction::Receive,
        host_file: "'USER.DATA'".into(),
        mode: ind_file::Mode::Text(Ccsid::Ibm930),
        recfm: Recfm::Default,
        lrecl: 0,
        space: 0,
        append: false,
    };
    let o = session
        .transfer(&req, Local::Sink(Box::new(file.clone())))
        .unwrap();
    w.write_all(&o.send).unwrap();
    // 2 回目は断る
    assert!(
        session
            .transfer(&req, Local::Sink(Box::new(Shared::default())))
            .is_err()
    );
    let cmd = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let want = e("IND$FILE GET 'USER.DATA' CRLF");
    assert!(cmd.windows(want.len()).any(|x| x == want), "{cmd:02X?}");
    pump(&mut session, &mut events, &|_, ev| {
        // 結果のメッセージで終わる。ホストの最後の Close にも答えるまで受け取り続ける
        ev.iter()
            .any(|e| matches!(e, Event::Transfer(FtEvent::Done { .. })))
            && h.is_finished()
    });
    assert!(events.contains(&Event::Transfer(FtEvent::Started)));
    assert!(events.contains(&Event::Transfer(FtEvent::Done {
        ok: true,
        message: "TRANS03 File transfer complete".into()
    })));
    assert!(!session.transferring());
    // ホストへの応答: Open・Insert・Close・Open・Insert・Close
    let acks: Vec<Vec<u8>> = (0..6)
        .map(|_| rx.recv_timeout(Duration::from_secs(10)).unwrap())
        .collect();
    assert_eq!(&acks[0][5..], &[AID_SF, 0, 5, 0xD0, 0x00, 0x09]);
    assert_eq!(&acks[1][5..9], &[AID_SF, 0, 11, 0xD0]);
    assert_eq!(&acks[2][5..], &[AID_SF, 0, 5, 0xD0, 0x41, 0x09]);
    let raw = file.0.lock().unwrap().clone();
    assert_eq!(
        ind_file::records_to_text(&raw, Ccsid::Ibm930, 0),
        "日本語のデータ\r\nABC\r\n"
    );
    h.join().unwrap();
}

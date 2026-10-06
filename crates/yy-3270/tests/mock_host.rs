//! 模擬ホストとの往復（TCP の上で TN3270E の交渉→画面→入力→Read Modified→次の画面）。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use yy_3270::codes::*;
use yy_3270::{Config, Key, Lock, Mode, Session};
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

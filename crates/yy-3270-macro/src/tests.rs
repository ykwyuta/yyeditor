//! マクロのテスト: 本物の `Session` を模擬ホストとつないで、スクリプトを動かす。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, mpsc};

use super::*;
use yy_3270::{Config, Session};
use yy_encoding::EbcdicCode;

fn e(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match Ccsid::Ibm037.encode_char(c) {
            Some(EbcdicCode::Single(b)) => b,
            _ => panic!("{c}"),
        })
        .collect()
}

fn frame(data: &[u8]) -> Vec<u8> {
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

/// ログオンの画面: 利用者 ID（1 行 14 桁）と、非表示のパスワード（2 行 14 桁）。
fn logon_screen() -> Vec<u8> {
    let mut v = vec![0xF5, WCC_RESTORE, ORDER_SF, FA_PROTECT];
    v.extend(e("USERID ===>"));
    v.extend([ORDER_SBA]);
    v.extend(encode_address(12));
    v.extend([ORDER_SF, 0x00, ORDER_IC]);
    v.extend([ORDER_SBA]);
    v.extend(encode_address(21));
    v.extend([ORDER_SF, FA_PROTECT]);
    v.extend([ORDER_SBA]);
    v.extend(encode_address(80));
    v.extend(e("PASSWORD ==>"));
    v.extend([ORDER_SBA]);
    v.extend(encode_address(92));
    v.extend([ORDER_SF, FA_NONDISPLAY]);
    v.extend([ORDER_SBA]);
    v.extend(encode_address(101));
    v.extend([ORDER_SF, FA_PROTECT]);
    frame(&v)
}

fn ready_screen() -> Vec<u8> {
    let mut v = vec![0xF5, WCC_RESTORE];
    v.extend(e("READY"));
    v.push(ORDER_SBA);
    v.extend(encode_address(80));
    v.extend(e("USER.DS1"));
    v.push(ORDER_SBA);
    v.extend(encode_address(160));
    v.extend(e("USER.DS2"));
    frame(&v)
}

type Program = Box<dyn FnMut(&[u8]) -> Option<Vec<u8>> + Send>;

struct State {
    session: Session,
    generation: u64,
    program: Program,
}

/// 模擬ホストにつないだセッション。応答は少し遅れて届く（待つ操作を確かめるため）。
struct TestHost {
    state: Mutex<State>,
    changed: Condvar,
    stop: AtomicBool,
    log: Mutex<Vec<String>>,
    ops: Mutex<Vec<Op>>,
    replies: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
}

impl TestHost {
    fn new(program: Program) -> Arc<TestHost> {
        let mut session = Session::new(Config {
            tn3270e: false,
            ..Config::default()
        });
        let mut neg = vec![IAC, DO, OPT_EOR, IAC, WILL, OPT_EOR];
        neg.extend([IAC, DO, OPT_BINARY, IAC, WILL, OPT_BINARY]);
        session.receive(&neg);
        session.receive(&logon_screen());
        let host = Arc::new(TestHost {
            state: Mutex::new(State {
                session,
                generation: 0,
                program,
            }),
            changed: Condvar::new(),
            stop: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            ops: Mutex::new(Vec::new()),
            replies: Mutex::new(None),
        });
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        *host.replies.lock().unwrap() = Some(tx);
        let h = Arc::downgrade(&host);
        std::thread::spawn(move || {
            for bytes in rx {
                std::thread::sleep(Duration::from_millis(40));
                let Some(h) = h.upgrade() else { return };
                h.feed(&bytes);
            }
        });
        host
    }

    fn feed(&self, bytes: &[u8]) {
        let mut s = self.state.lock().unwrap();
        s.session.receive(bytes);
        s.generation += 1;
        self.changed.notify_all();
    }

    fn key(&self, s: &mut State, k: Key) {
        let o = s.session.key(k);
        s.generation += 1;
        if !o.send.is_empty()
            && let Some(reply) = (s.program)(&o.send)
        {
            let _ = self.replies.lock().unwrap().as_ref().unwrap().send(reply);
        }
    }
}

impl Host for TestHost {
    fn snapshot(&self) -> Result<Snapshot, String> {
        Ok(Snapshot::of(&self.state.lock().unwrap().session, true))
    }
    fn generation(&self) -> u64 {
        self.state.lock().unwrap().generation
    }
    fn wait_change(&self, since: u64, timeout: Duration) {
        let s = self.state.lock().unwrap();
        let _ = self
            .changed
            .wait_timeout_while(s, timeout, |s| s.generation == since);
    }
    fn act(&self, op: Op) -> Result<Answer, String> {
        self.ops.lock().unwrap().push(op.clone());
        let mut s = self.state.lock().unwrap();
        match op {
            Op::Key(k) => self.key(&mut s, k),
            Op::Type(t) => {
                for c in t.chars() {
                    self.key(&mut s, Key::Char(c));
                }
            }
            Op::MoveTo(r, c) => {
                let cols = s.session.screen().cols;
                self.key(&mut s, Key::MoveTo((r - 1) * cols + c - 1));
            }
            Op::Password(name) => {
                if name != "host" {
                    return Err(format!("資格情報「{name}」がありません"));
                }
                for c in "PW123".chars() {
                    self.key(&mut s, Key::Char(c));
                }
            }
            Op::Log(l) => self.log.lock().unwrap().push(l),
            Op::Ask(_) => return Ok(Answer::Text(Some("答え".into()))),
            Op::Transfer { .. } => {
                return Ok(Answer::Transfer {
                    ok: true,
                    message: "TRANS03 File transfer complete".into(),
                });
            }
            Op::PrintScreen | Op::Message(_) => {}
        }
        Ok(Answer::Done)
    }
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn opts(dir: &Path) -> Options {
    Options {
        out_dir: dir.to_path_buf(),
        timeout: 5,
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("yy3270-macro-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn logs_on_waits_and_writes_csv() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let s2 = sent.clone();
    let host = TestHost::new(Box::new(move |data: &[u8]| {
        s2.lock().unwrap().extend_from_slice(data);
        (data.first() == Some(&yy_3270::codes::AID_ENTER)).then(ready_screen)
    }));
    let dir = temp_dir("logon");
    let script = r#"
        wait_unlocked(5);
        let f = fields();
        log(`fields ${f.len()} ${f[1].row},${f[1].col} hidden=${f[3].hidden}`);
        type("IBMUSER");
        tab();
        password("host");
        key("Enter");
        wait_text("READY", 5);
        let out = csv_open("out/list.csv");
        for r in 2..=3 {
            out.write_row([row(r).trim(), "a,b"]);
        }
        out.close();
        log("first " + text_at(1, 1, 5));
        if !wait_text_at(24, 2, "NOPE", 0) { log("absent"); }
        let c = cursor();
        log(`cursor ${c.row}`);
        let t = transfer_get("'USER.DS1'", "ds1.txt", #{ mode: "binary" });
        log(t.message);
    "#;
    run(script, host.clone(), opts(&dir)).unwrap();
    let log = host.log.lock().unwrap().clone();
    assert_eq!(log[0], "fields 5 1,14 hidden=true");
    assert_eq!(&log[1..3], &["first READY", "absent"]);
    assert_eq!(log[4], "TRANS03 File transfer complete");
    // 送ったもの: 利用者 ID とパスワード（スクリプトはパスワードを知らない）
    let sent = sent.lock().unwrap().clone();
    assert!(contains(&sent, &e("IBMUSER")));
    assert!(contains(&sent, &e("PW123")));
    let csv = std::fs::read_to_string(dir.join("out/list.csv")).unwrap();
    assert_eq!(csv, "\u{FEFF}USER.DS1,\"a,b\"\r\nUSER.DS2,\"a,b\"\r\n");
    // 転送の指定
    let ops = host.ops.lock().unwrap().clone();
    let Some(Op::Transfer { request, local }) =
        ops.iter().find(|o| matches!(o, Op::Transfer { .. }))
    else {
        panic!()
    };
    assert_eq!(request.mode, FtMode::Binary);
    assert_eq!(request.direction, Direction::Receive);
    assert_eq!(local, &dir.join("ds1.txt"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn errors_have_line_numbers_and_stop_works() {
    let host = TestHost::new(Box::new(|_| None));
    let dir = temp_dir("errors");
    // 時間切れ
    let err = run(
        "wait_unlocked(1);\nwait_text(\"NEVER\", 1);",
        host.clone(),
        opts(&dir),
    )
    .unwrap_err();
    assert_eq!(err.line, 2);
    assert!(err.message.contains("時間切れ"), "{err}");
    // 知らないキー・出力のフォルダの外・資格情報がない
    let err = run("key(\"PF99\");", host.clone(), opts(&dir)).unwrap_err();
    assert!(err.message.contains("PF99"), "{err}");
    let err = run("\n\ncsv_open(\"../x.csv\");", host.clone(), opts(&dir)).unwrap_err();
    assert_eq!(err.line, 3);
    assert!(err.message.contains("出力のフォルダ"), "{err}");
    let err = run("password(\"other\");", host.clone(), opts(&dir)).unwrap_err();
    assert!(err.message.contains("other"), "{err}");
    // 文法の誤り
    let err = check("let x = ;").unwrap_err();
    assert_eq!(err.line, 1);
    // 止める: 無限の繰り返しでも止まる
    let h2 = host.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        h2.stop.store(true, Ordering::Relaxed);
    });
    let err = run("loop { let x = 1; }", host.clone(), opts(&dir)).unwrap_err();
    assert!(err.stopped);
    assert_eq!(err.message, "停止しました");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parses_keys_paths_and_reads_double_byte_text() {
    assert_eq!(parse_key("pf8"), Some(Key::Pf(8)));
    assert_eq!(parse_key("PA2"), Some(Key::Pa(2)));
    assert_eq!(parse_key("EraseEOF"), Some(Key::EraseEof));
    assert_eq!(parse_key("PF25"), None);
    let d = Path::new("out");
    assert!(resolve_out(d, "a/b.csv").is_ok());
    assert!(resolve_out(d, "../b.csv").is_err());
    assert!(resolve_out(d, "/etc/x").is_err());
    assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
    let snap = Snapshot {
        cells: vec![vec![
            " ".into(),
            "日".into(),
            String::new(),
            "本".into(),
            String::new(),
            "A".into(),
        ]],
        cursor: (1, 1),
        locked: false,
        connected: true,
        fields: Vec::new(),
        ccsid: Ccsid::Ibm930,
    };
    assert_eq!(snap.text_at(1, 2, 4), "日本");
    assert_eq!(snap.row(1), " 日本A");
    assert_eq!(snap.text_at(2, 1, 3), "");
    let mut m = Map::new();
    m.insert("host".into(), "cms".into());
    m.insert("recfm".into(), "v".into());
    m.insert("lrecl".into(), (255_i64).into());
    let r = transfer_request(Direction::Send, "A B C", &m, Ccsid::Ibm930).unwrap();
    assert_eq!(
        (r.host, r.recfm, r.lrecl, r.mode),
        (
            HostKind::Cms,
            Recfm::Variable,
            255,
            FtMode::Text(Ccsid::Ibm930)
        )
    );
    m.insert("mode".into(), "zip".into());
    assert!(transfer_request(Direction::Send, "X", &m, Ccsid::Ibm930).is_err());
}

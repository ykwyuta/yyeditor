//! yy-3270（yyterm の 3270 の中核）を模擬ホストで試す。Hercules（MVS 3.8j）では確かめられない、
//! TN3270E（LU 名・拒否・RESPONSES・ASSOCIATE・PRINT-EOJ）、日本語（DBCS）、Query Reply の申告、
//! z/OS 風の IND$FILE を確かめる。模擬ホストの振る舞いは x3270 で確かめてある（check-with-x3270.sh）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use yy_3270::{Config, Event, Mode};
use yy_3270_macro::tcp::TcpHost;
use yy_3270_macro::{MacroError, Options};
use yy_3270_mock::{IndFileStyle, MockConfig, MockHost};
use yy_encoding::Ccsid;

const T: Duration = Duration::from_secs(10);

fn mock(f: impl FnOnce(&mut MockConfig)) -> MockHost {
    let mut cfg = MockConfig::default();
    f(&mut cfg);
    MockHost::start(cfg).unwrap()
}

fn terminal(m: &MockHost, f: impl FnOnce(&mut Config)) -> Arc<TcpHost> {
    let mut cfg = Config {
        ccsid: Ccsid::Ibm930,
        ..Config::default()
    };
    f(&mut cfg);
    let h = TcpHost::connect(&m.addr().to_string(), cfg).unwrap();
    h.set_password(|name| (name == "mock").then(|| "SECRET".to_owned()));
    h
}

fn out_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("yy3270-mock-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(h: &Arc<TcpHost>, dir: &Path, script: &str) -> Result<(), MacroError> {
    yy_3270_macro::run(
        script,
        h.clone(),
        Options {
            out_dir: dir.to_path_buf(),
            timeout: 10,
        },
    )
}

/// ログオン（DBCS のフィールドと混在のフィールドに日本語を入れる）して READY まで。
const LOGON: &str = r#"
wait_unlocked();
type("IBMUSER");
tab();
password("mock");
tab();
type("山田太郎");
tab();
type("ﾃｽﾄ と 日本語 ABC");
key("Enter");
wait_text("READY");
"#;

const LOGOFF: &str = r#"
type("LOGOFF");
key("Enter");
wait_text("LOGGED OFF");
"#;

fn has(m: &MockHost, s: &str) -> bool {
    m.wait_event(T, |e| e.contains(s)).is_some()
}

#[test]
fn tn3270e_named_lu_responses_and_japanese_logon() {
    let m = mock(|_| {});
    let h = terminal(&m, |c| c.lu = Some("TCP00002".into()));
    let dir = out_dir("logon");
    run(&h, &dir, &format!("{LOGON}{LOGOFF}")).unwrap();
    assert!(
        has(&m, "device IBM-3278-2-E lu=TCP00002 (connect)"),
        "{:#?}",
        m.events()
    );
    assert!(has(&m, "functions [2] lu=TCP00002"));
    assert!(has(&m, "response positive seq=1 lu=TCP00002"));
    // DBCS のフィールドは 2 バイト文字だけ、混在のフィールドは SO/SI を自動で入れて送る
    assert!(
        has(
            &m,
            "logon user=IBMUSER password=ok name=山田太郎 note=ﾃｽﾄ と 日本語 ABC lu=TCP00002"
        ),
        "{:#?}",
        m.events()
    );
    assert!(has(&m, "logoff lu=TCP00002"));
    // 応答を求めたものにはすべて肯定の応答を返した
    assert!(!m.events().iter().any(|e| e.contains("negative")));
    assert!(h.events().contains(&Event::Device("TCP00002".into())));
    assert!(h.events().contains(&Event::Mode(Mode::Tn3270e)));
}

#[test]
fn rejected_lu_falls_back_to_tn3270_with_the_lu_in_the_terminal_type() {
    let m = mock(|_| {});
    // ない LU 名: TN3270E を断られたら TN3270 に戻り、端末の種類に @LU 名を付ける（RFC 1646）
    let h = terminal(&m, |c| c.lu = Some("NOSUCH".into()));
    assert!(has(&m, "device rejected INV-NAME"));
    assert!(
        h.wait_event(
            T,
            |e| matches!(e, Event::DeviceRejected(r) if r.contains("INV-NAME"))
        )
        .is_some()
    );
    assert!(
        h.wait_event(T, |e| *e == Event::Mode(Mode::Tn3270))
            .is_some(),
        "{:#?}",
        h.events()
    );
    assert!(
        has(&m, "tn3270 type=IBM-3278-2-E@NOSUCH"),
        "{:#?}",
        m.events()
    );
    // 使用中の LU
    let dir = out_dir("inuse");
    let first = terminal(&m, |c| c.lu = Some("TCP00003".into()));
    assert!(
        first
            .wait_event(T, |e| *e == Event::Mode(Mode::Tn3270e))
            .is_some()
    );
    let second = terminal(&m, |c| c.lu = Some("TCP00003".into()));
    assert!(
        second
            .wait_event(
                T,
                |e| matches!(e, Event::DeviceRejected(r) if r.contains("DEVICE-IN-USE"))
            )
            .is_some()
    );
    run(&first, &dir, &format!("{LOGON}{LOGOFF}")).unwrap();
}

#[test]
fn tn3270_fallback_when_the_host_does_not_offer_tn3270e() {
    let m = mock(|c| c.tn3270e = false);
    let h = terminal(&m, |_| {});
    let dir = out_dir("tn3270");
    run(&h, &dir, &format!("{LOGON}{LOGOFF}")).unwrap();
    assert!(has(&m, "tn3270 type=IBM-3278-2-E"));
    assert!(h.events().contains(&Event::Mode(Mode::Tn3270)));
    assert!(has(&m, "logon user=IBMUSER password=ok name=山田太郎"));
}

#[test]
fn query_reply_declares_japanese_character_sets_and_ddm() {
    let m = mock(|_| {});
    let h = terminal(&m, |_| {});
    let dir = out_dir("query");
    run(
        &h,
        &dir,
        &format!("{LOGON}type(\"QUERY\"); key(\"Enter\"); wait_text(\"ddm\"); {LOGOFF}"),
    )
    .unwrap();
    // x3270 と同じ CGCSGID（1 バイト部 1172/290、2 バイト部 370/300）
    assert!(
        has(&m, "charset set 00 gcsgid 1172 cpgid 290"),
        "{:#?}",
        m.events()
    );
    assert!(
        has(&m, "charset set 80 gcsgid 370 cpgid 300"),
        "{:#?}",
        m.events()
    );
    assert!(has(&m, "ddm 4096/4096"));
    assert!(has(&m, "dbcs-asia"));
    assert!(has(&m, "usable area 80x24"));
}

#[test]
fn japanese_screen_input() {
    let m = mock(|_| {});
    let h = terminal(&m, |_| {});
    let dir = out_dir("nihongo");
    let script = format!(
        r#"{LOGON}
        type("NIHONGO");
        key("Enter");
        wait_text("日本語の入力");
        // 半角の濁点（ﾞ）は 1 桁を占める（27 桁）
        if text_at(6, 3, 27) != "ｶﾀｶﾅ ﾄ ｴｲｽｳｼﾞ ﾉ ﾐﾀﾞｼ ABC123" {{ throw "見出し: " + row(6); }}
        type("全角漢字");
        tab();
        type("ABC 日本 ｶﾅ");
        key("Enter");
        wait_text("READY");
        {LOGOFF}"#
    );
    run(&h, &dir, &script).unwrap();
    assert!(
        has(&m, "nihongo dbcs=全角漢字 mixed=ABC 日本 ｶﾅ"),
        "{:#?}",
        m.events()
    );
}

#[test]
fn associated_printer_receives_scs_and_lu3_jobs() {
    let m = mock(|_| {});
    let term = terminal(&m, |_| {});
    // 端末に割り当てられた LU に対応するプリンター
    let Some(Event::Device(lu)) = term.wait_event(T, |e| matches!(e, Event::Device(_))) else {
        panic!("{:#?}", term.events())
    };
    let printer = terminal(&m, |c| {
        c.printer = true;
        c.associate = Some(lu.clone());
    });
    assert!(
        printer
            .wait_event(T, |e| *e == Event::Mode(Mode::Tn3270e))
            .is_some()
    );
    assert!(has(&m, "(associate)"));
    // 端末の側でつながっても、模擬ホストがプリンターを登録するまでは PRINT を受け付けない
    assert!(has(&m, "printer ready"), "{:#?}", m.events());
    let dir = out_dir("print");
    let script = format!(
        r#"{LOGON}
        type("PRINT"); key("Enter"); wait_text("SCS"); wait_unlocked();
        type("PRINT LU3"); key("Enter"); wait_text("LU3"); wait_unlocked();
        {LOGOFF}"#
    );
    run(&term, &dir, &script).unwrap();
    assert!(
        printer
            .wait_event(T, |e| matches!(e, Event::PrintJob(j) if j.pages.len() == 1))
            .is_some()
    );
    let jobs = printer.print_jobs();
    assert_eq!(jobs.len(), 2, "{jobs:#?}");
    // SCS: 日本語・改ページ（PRINT-EOJ で 1 つのジョブ）
    assert_eq!(
        jobs[0].pages,
        vec![
            vec![
                "請求書　No.0001".to_owned(),
                "品名        数量    金額".to_owned(),
                "ﾈｼﾞ M6       100   1,200".to_owned()
            ],
            vec!["２ページ目　END".to_owned()]
        ]
    );
    assert_eq!(jobs[0].columns, 80);
    // LU3: WCC の 80 桁の行
    // SO/SI は印刷の位置を 1 つずつ占める（空白として印刷する）
    assert_eq!(
        jobs[1].pages,
        vec![vec![
            "LU3  の印刷  PAGE 1".to_owned(),
            " ２行目  ABC".to_owned()
        ]]
    );
    let p = m.events();
    assert!(
        p.iter()
            .any(|e| e.contains("response positive seq=1 lu=PRT")),
        "{p:#?}"
    );
}

#[test]
fn ind_file_zos_style_text_records_and_round_trips() {
    let m = mock(|_| {});
    let h = terminal(&m, |_| {});
    let dir = out_dir("indfile");
    std::fs::write(
        dir.join("up.txt"),
        "日本語の行\r\nLINE TWO\r\n\r\nｶﾅ END\r\n",
    )
    .unwrap();
    let script = format!(
        r#"{LOGON}
        fn check(what, r) {{ if !r.ok {{ throw what + ": " + r.message; }} wait_unlocked(); }}
        check("get vb", transfer_get("'YY.TEST.VB'", "vb.txt"));
        check("get fb", transfer_get("'YY.TEST.FB80'", "fb.txt", #{{ lrecl: 80 }}));
        check("get ascii", transfer_get("'YY.TEST.VB'", "vb-ascii.txt", #{{ mode: "ascii" }}));
        check("put v", transfer_put("up.txt", "'YY.UP.V'", #{{ recfm: "V", lrecl: 255 }}));
        check("put f", transfer_put("up.txt", "'YY.UP.F'", #{{ recfm: "F", lrecl: 80 }}));
        check("get v back", transfer_get("'YY.UP.V'", "back-v.txt"));
        check("get f back", transfer_get("'YY.UP.F'", "back-f.txt", #{{ lrecl: 80 }}));
        {LOGOFF}"#
    );
    run(&h, &dir, &script).unwrap();
    let read = |n: &str| std::fs::read_to_string(dir.join(n)).unwrap();
    // 可変長は CRLF でホストが区切る（z/OS 風）。日本語は端末で変換する
    assert_eq!(
        read("vb.txt"),
        "可変長の 1 行目\r\n\r\nSHORT\r\n最後の行 END\r\n"
    );
    assert_eq!(
        read("fb.txt"),
        "ＹＹ模擬ホストのテストデータ\r\nLINE 2 ABC 123\r\nｶﾀｶﾅ ﾃﾞｰﾀ 終わり\r\n"
    );
    // ホストの ASCII 変換は日本語を扱えない
    assert!(read("vb-ascii.txt").contains("SHORT"));
    // 送ったテキスト: 可変長は CRLF で、固定長は LRECL で詰めて（CRLF なし）
    let v = m.dataset("YY.UP.V").unwrap();
    assert_eq!(v.records.len(), 4, "{v:?}");
    let f = m.dataset("YY.UP.F").unwrap();
    assert_eq!(f.records.len(), 4);
    assert!(f.records.iter().all(|r| r.len() == 80));
    assert_eq!(f.records[1][..8], v.records[1][..8]);
    assert!(f.records[1][8..].iter().all(|&b| b == 0x40));
    let original = "日本語の行\r\nLINE TWO\r\n\r\nｶﾅ END\r\n";
    assert_eq!(read("back-v.txt"), original);
    assert_eq!(read("back-f.txt"), original);
    assert!(has(&m, "ind$file put YY.UP.V records=4"));
    assert!(has(&m, "ascii=false crlf=false"), "{:#?}", m.events());
}

#[test]
fn ind_file_mvs38_style_message_without_close() {
    let m = mock(|c| c.ind_file = IndFileStyle::Mvs38);
    let h = terminal(&m, |_| {});
    let dir = out_dir("mvs38");
    let script = format!(
        r#"{LOGON}
        let r = transfer_get("'YY.TEST.FB80'", "fb.txt", #{{ lrecl: 80 }});
        if !r.ok {{ throw r.message; }}
        wait_unlocked();
        let b = transfer_get("'YY.TEST.FB80'", "fb.bin", #{{ mode: "binary" }});
        if !b.ok {{ throw b.message; }}
        wait_unlocked();
        {LOGOFF}"#
    );
    run(&h, &dir, &script).unwrap();
    assert_eq!(std::fs::read(dir.join("fb.bin")).unwrap().len(), 240);
    assert!(
        std::fs::read_to_string(dir.join("fb.txt"))
            .unwrap()
            .starts_with("ＹＹ模擬ホスト")
    );
}

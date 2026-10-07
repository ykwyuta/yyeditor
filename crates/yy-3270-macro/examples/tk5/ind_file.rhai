// MVS 3.8j TK5（Hercules）で、ログオン・IND$FILE の転送を確かめる。
//
//   TK5 を起動しておき（unattended/set_daemon_mode のあと ./mvs）、出力のフォルダに put.txt を置いて:
//   YY3270_PASSWORD_HERC01=CUL8TR cargo run -p yy-3270-macro --example run -- \
//       127.0.0.1:3270 crates/yy-3270-macro/examples/tk5/ind_file.rhai 出力のフォルダ
//
// 途中で止まると HERC01 がログオンしたまま残る。Hercules の操作卓で /c u=herc01 を入れて消す。

fn show(t) {
    log("---- " + t);
    for l in screen_lines() {
        let x = l;
        x.trim();
        if x != "" { log("| " + l); }
    }
}
fn next() { wait_unlocked(60); key("Clear"); wait_unlocked(30); }
fn check(what, r) {
    log(`${what}: ${r.ok} ${r.message}`);
    if !r.ok { throw what + " に失敗しました"; }
    next();
}

// Hercules の案内の画面のあと、Clear で VTAM のログオンの画面（NETSOL）
wait_unlocked(30);
key("Clear");
wait_text("Logon ===>", 30);
type("HERC01");
key("Enter");
wait_unlocked(60);
sleep(1500);
if !wait_text("PASSWORD", 0) { show("パスワードの画面になりません"); throw "ログオンできません"; }
password("herc01");
key("Enter");
// HERC01 は ISPF が始まる。X で抜けて READY へ
let n = 0;
loop {
    wait_unlocked(60);
    sleep(1500);
    if wait_text("READY", 0) { break; }
    if wait_text("Option  ===>", 0) { type("X"); }
    key("Enter");
    n += 1;
    if n > 8 { show("READY になりません"); throw "READY になりません"; }
}
next();

// 固定長（FB 80）の区分データセットのメンバー: 端末で変換・ホストの ASCII 変換・バイナリ
check("GET テキスト（LRECL 80）", transfer_get("'SYS2.JCLLIB(CREDITS)'", "credits.txt", #{ lrecl: 80 }));
check("GET ASCII", transfer_get("'SYS2.JCLLIB(CREDITS)'", "credits-ascii.txt", #{ mode: "ascii" }));
check("GET バイナリ", transfer_get("'SYS2.JCLLIB(CREDITS)'", "credits.bin", #{ mode: "binary" }));
// 固定長のテキストを送って受け取り直す（CRLF を使わず、LRECL まで空白で詰める）
check("PUT テキスト（RECFM F LRECL 80）",
      transfer_put("put.txt", "'HERC01.YYTEST.FB'", #{ recfm: "F", lrecl: 80, space: 1 }));
check("GET テキスト（LRECL 80）", transfer_get("'HERC01.YYTEST.FB'", "back.txt", #{ lrecl: 80 }));
check("GET バイナリ", transfer_get("'HERC01.YYTEST.FB'", "back.bin", #{ mode: "binary" }));

type("LOGOFF");
key("Enter");
sleep(2000);
log("終わりました");

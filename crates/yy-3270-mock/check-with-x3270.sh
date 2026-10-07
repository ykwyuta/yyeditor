#!/usr/bin/env bash
# 模擬ホストを x3270（s3270・pr3287）で確かめる。x3270 は広く使われている 3270 のエミュレーターで、
# これで期待どおりに動けば、模擬ホストは正しい端末に対して正しく振る舞っている（yyterm の試験の基準になる）。
#
#   crates/yy-3270-mock/check-with-x3270.sh
#
# 環境変数 S3270・PR3287 で使うプログラムを変えられる（既定は PATH の s3270・pr3287）。日本語（cp930）と
# TLS（暗黙の TLS・STARTTLS・クライアント証明書。x3270 は OpenSSL で）を確かめる。
set -u
S3270=${S3270:-s3270}
PR3287=${PR3287:-pr3287}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=$(mktemp -d)
PORT=${PORT:-32701}
export LANG=C.UTF-8
fail=0
check() { # 説明 コマンド…
    local what=$1; shift
    if "$@" > /dev/null 2>&1; then echo "OK   $what"; else echo "FAIL $what"; fail=1; fi
}

cargo build -q -p yy-3270-mock --manifest-path "$ROOT/Cargo.toml" || exit 1
"$ROOT/target/debug/yy-3270-mock" --port "$PORT" > "$WORK/mock.log" 2>&1 &
MOCK=$!
trap 'kill $MOCK ${PRT:-} ${MOCK_TLS:-} ${MOCK_STARTTLS:-} 2>/dev/null; rm -rf "$WORK"' EXIT
sleep 1

printf 'first line\r\nsecond line\r\n' > "$WORK/up.txt"

# 端末（TCP00001）
cat > "$WORK/s3270.in" <<IN
Connect(TCP00001@127.0.0.1:$PORT)
Wait(10,InputField)
String(IBMUSER)
Tab()
String(SECRET)
Tab()
String("山田太郎")
Tab()
String("ﾃｽﾄ ABC")
Enter()
Wait(5,Unlock)
String(QUERY)
Enter()
Wait(5,Unlock)
String(NIHONGO)
Enter()
Wait(5,Unlock)
String("日本語入力")
Tab()
String("ABC 123")
Enter()
Wait(5,Unlock)
Transfer(direction=receive,host=tso,hostfile='YY.TEST.VB',localfile=$WORK/vb-ascii.txt,mode=ascii,cr=remove,exist=replace)
Wait(5,Unlock)
Transfer(direction=receive,host=tso,hostfile='YY.TEST.FB80',localfile=$WORK/fb.bin,mode=binary,exist=replace)
Wait(5,Unlock)
Transfer(direction=send,host=tso,hostfile='YY.UP.TXT',localfile=$WORK/up.txt,mode=ascii,cr=remove,recfm=variable,lrecl=255)
Wait(5,Unlock)
Wait(2,Seconds)
String(PRINT)
Enter()
Wait(5,Unlock)
Wait(2,Seconds)
String("PRINT LU3")
Enter()
Wait(5,Unlock)
Wait(2,Seconds)
String(LOGOFF)
Enter()
Wait(2,Seconds)
Quit()
IN
# プリンター（端末 TCP00001 に対応づける）は、端末がつながってから
( sleep 2; exec "$PR3287" -assoc TCP00001 -command "cat >> $WORK/print.txt" -codepage cp930 "127.0.0.1:$PORT" ) > "$WORK/pr3287.log" 2>&1 &
PRT=$!
timeout 120 "$S3270" -model 3279-2 -codepage cp930 -utf8 < "$WORK/s3270.in" > "$WORK/s3270.out" 2>&1

L="$WORK/mock.log"
check "TN3270E で LU を指定して接続（CONNECT）" grep -q "device IBM-3278-2-E lu=TCP00001 (connect)" "$L"
check "RESPONSES を合意" grep -q "functions \[2\] lu=TCP00001" "$L"
check "肯定の応答" grep -q "response positive seq=1 lu=TCP00001" "$L"
check "ログオン（DBCS のフィールドの入力）" grep -q "logon user=IBMUSER password=ok name=山田太郎 note=ﾃｽﾄ ABC" "$L"
check "Query Reply（DBCS の文字セット 370/300）" grep -q "charset set 80 gcsgid 370 cpgid 300" "$L"
check "Query Reply（DDM）" grep -q "^\[mock\] ddm " "$L"
check "日本語の画面の入力" grep -q "nihongo dbcs=日本語入力 mixed=ABC 123" "$L"
check "IND\$FILE GET ASCII CRLF" grep -q "ind\$file get YY.TEST.VB bytes=.* ascii=true crlf=true ok" "$L"
check "IND\$FILE GET バイナリ" grep -q "ind\$file get YY.TEST.FB80 bytes=240 ascii=false crlf=false ok" "$L"
check "IND\$FILE PUT ASCII CRLF" grep -q "ind\$file put YY.UP.TXT records=2 " "$L"
check "受け取ったテキスト（ホストの ASCII 変換）" grep -qx "SHORT" "$WORK/vb-ascii.txt"
cat -A "$WORK/vb-ascii.txt" > "$WORK/vb-dump.txt" 2>&1
check "プリンターを対応づけ（ASSOCIATE）" grep -q "device IBM-3287-1 lu=PRT00001 (associate)" "$L"
check "印刷（SCS・PRINT-EOJ）" grep -q "printed SCS job=1 lu=PRT00001" "$L"
# pr3287 は SCS の 2 バイト文字を読まない（1 バイト部・半角カナだけ確かめる）
check "印刷の中身" grep -q "ﾈｼﾞ M6       100   1,200" "$WORK/print.txt"
check "印刷（LU3・PRINT-EOJ）" grep -q "printed LU3 job=2 lu=PRT00001" "$L"
check "LU3 の印刷の中身" grep -q "PAGE 1" "$WORK/print.txt"
check "ログオフ" grep -q "logoff lu=TCP00001" "$L"

# ---- TLS
# 暗黙の TLS（L:）: 認証局で検証し、クライアント証明書を求める
"$ROOT/target/debug/yy-3270-mock" --port $((PORT + 1)) --tls --require-client-cert \
    --cert-dir "$WORK/certs-tls" > "$WORK/mock-tls.log" 2>&1 &
MOCK_TLS=$!
# STARTTLS: 自己署名の証明書
"$ROOT/target/debug/yy-3270-mock" --port $((PORT + 2)) --starttls --self-signed \
    --cert-dir "$WORK/certs-starttls" > "$WORK/mock-starttls.log" 2>&1 &
MOCK_STARTTLS=$!
sleep 1
logon_logoff() { # 接続先
    printf 'Connect(%s)\nWait(10,InputField)\nString(IBMUSER)\nTab()\nString(SECRET)\nEnter()\nWait(5,Unlock)\nWait(3,Seconds)\nString(PRINT)\nEnter()\nWait(5,Unlock)\nWait(2,Seconds)\nString(LOGOFF)\nEnter()\nWait(2,Seconds)\nQuit()\n' "$1"
}
C="$WORK/certs-tls"
logon_logoff "L:TCP00001@localhost:$((PORT + 1))" | timeout 60 "$S3270" -cafile "$C/ca.pem" \
    -certfile "$C/client.pem" -keyfile "$C/client.key" -model 3279-2 > "$WORK/s3270-tls.out" 2>&1
logon_logoff "L:TCP00002@localhost:$((PORT + 1))" | timeout 60 "$S3270" -cafile "$C/ca.pem" \
    -model 3279-2 > "$WORK/s3270-tls-nocert.out" 2>&1
( sleep 2; exec "$PR3287" -noverifycert -assoc TCP00001 -command "cat >> $WORK/print-starttls.txt" \
    "127.0.0.1:$((PORT + 2))" ) > "$WORK/pr3287-starttls.log" 2>&1 &
PRT=$!
logon_logoff "TCP00001@127.0.0.1:$((PORT + 2))" | timeout 60 "$S3270" -noverifycert \
    -model 3279-2 > "$WORK/s3270-starttls.out" 2>&1
logon_logoff "TCP00002@127.0.0.1:$((PORT + 2))" | timeout 60 "$S3270" \
    -model 3279-2 > "$WORK/s3270-starttls-verify.out" 2>&1
LT="$WORK/mock-tls.log"
LS="$WORK/mock-starttls.log"
check "暗黙の TLS（クライアント証明書を検証）" grep -q "tls implicit version=TLSv1_[23] client=[0-9A-F][0-9A-F]:" "$LT"
check "暗黙の TLS の上の TN3270E" grep -q "logoff lu=TCP00001" "$LT"
check "クライアント証明書がなければ断る" grep -q "tls failed" "$LT"
check "クライアント証明書がなければ LU を渡さない" bash -c "! grep -q 'lu=TCP00002' '$LT'"
check "STARTTLS" grep -q "tls starttls version=TLSv1_[23] client=none" "$LS"
check "STARTTLS の上の TN3270E" grep -q "logoff lu=TCP00001" "$LS"
check "STARTTLS の上のプリンター（ASSOCIATE・SCS）" grep -q "printed SCS job=1 lu=PRT00001" "$LS"
check "自己署名の証明書は検証で断られる" grep -q "tls failed .*UnknownCA" "$LS"

if [ $fail -ne 0 ]; then
    for f in mock-tls.log mock-starttls.log pr3287-starttls.log; do echo "---- $f"; cat "$WORK/$f"; done
    for f in s3270-tls s3270-tls-nocert s3270-starttls s3270-starttls-verify; do
        echo "---- $f"; grep -v "^ok$" "$WORK/$f.out" | tail -5
    done
    echo "---- mock.log"; cat "$L"
    echo "---- s3270"; grep -v "^ok$" "$WORK/s3270.out" | tail -40
    echo "---- pr3287"; cat "$WORK/pr3287.log"
    echo "---- print"; cat "$WORK/print.txt" 2>/dev/null
    echo "---- vb"; cat "$WORK/vb-dump.txt" 2>/dev/null
fi
exit $fail

#!/usr/bin/env bash
# 模擬ホストを x3270（s3270・pr3287）で確かめる。x3270 は広く使われている 3270 のエミュレーターで、
# これで期待どおりに動けば、模擬ホストは正しい端末に対して正しく振る舞っている（yyterm の試験の基準になる）。
#
#   crates/yy-3270-mock/check-with-x3270.sh
#
# 環境変数 S3270・PR3287 で使うプログラムを変えられる（既定は PATH の s3270・pr3287）。日本語（cp930）と
# TLS を使う。
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
trap 'kill $MOCK ${PRT:-} 2>/dev/null; rm -rf "$WORK"' EXIT
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

if [ $fail -ne 0 ]; then
    echo "---- mock.log"; cat "$L"
    echo "---- s3270"; grep -v "^ok$" "$WORK/s3270.out" | tail -40
    echo "---- pr3287"; cat "$WORK/pr3287.log"
    echo "---- print"; cat "$WORK/print.txt" 2>/dev/null
    echo "---- vb"; cat "$WORK/vb-dump.txt" 2>/dev/null
fi
exit $fail

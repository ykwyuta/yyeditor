#!/usr/bin/env bash
# yy-cobol の解釈（コピーブックのレイアウト・項目の読み書き）を GnuCOBOL（cobc -std=ibm）と比べる。
#
#   crates/yy-cobol/check-with-gnucobol.sh
#
# 要るもの: cobc（GnuCOBOL 3.x。Ubuntu なら apt install gnucobol3）、iconv（glibc。EBCDIC の比べ合わせ）。
# 同じコピーブック（tests/gnucobol/*.cpy）を COPY する COBOL のプログラムを作って動かし、項目の位置・
# 長さ、COBOL が書いたファイルの yy-cobol での読み、yy-cobol が書いたファイルとのバイトの比べ合わせ、
# yy-cobol が書いたファイルの COBOL での読みを確かめる。GnuCOBOL の側の違いとわかっているものは
# 「参考」として出し、不一致に数えない。
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
cd "$ROOT"
cargo run -q -p yy-cobol --example gnucobol -- "$WORK"

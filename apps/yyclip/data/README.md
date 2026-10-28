# 日本の祝日・休業日CSV

`japan-holidays-2026-2031.csv`は2026～2031年の祝日、振替休日、祝日に挟まれた休日を収録します。
UTF-8 BOM付きの「日付,名称」2列で、yyclipの初期カレンダーにも埋め込みます。土日はCSVに列挙せず、アプリ側で休業日と判定します。
会社独自の休業日はこのCSVに追記し、タスク編集画面の「休業日CSV」から読み込めます。

2026・2027年の日付は[内閣府の祝日一覧・CSV](https://www8.cao.go.jp/chosei/shukujitsu/gaiyou.html)と照合しています。
2028～2031年は同ページ記載の現行の祝日法と[国立天文台の春分・秋分日予測](https://www.nao.ac.jp/faq/a0301.html)に基づく予定です。
将来分の正式な祝日一覧ではありません。春分・秋分の日は前年の暦要項で正式決定され、法改正・特例でも変わることがあるため、正式発表後はCSVを更新してください。
取得・照合日: 2026-10-28。

再生成・差分確認:

```powershell
python tools/yyclip/generate-holidays.py
python tools/yyclip/generate-holidays.py --check
# 内閣府CSVをダウンロード済みの場合
python tools/yyclip/generate-holidays.py --check --verify-cabinet target/syukujitsu.csv
```

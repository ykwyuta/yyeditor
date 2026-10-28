# yytoolbox ダミーCSV定義例

yytoolboxで「ダミーCSV」を選び、「入力を開く」でYAMLファイルを読み込みます。
件数は画面上部の「行数（1〜100000）」で指定します（初期値100件、見出し行は含みません）。
行数・seed・BOM・数式対策を選び、「実行」「結果を保存」でCSVを作れます。
各定義の列名、型、値の範囲や選択肢は編集できます。

| 定義 | 分野 |
| --- | --- |
| [person.yaml](person.yaml) | 個人 |
| [company.yaml](company.yaml) | 企業 |
| [orders.yaml](orders.yaml) | EC注文 |
| [medical.yaml](medical.yaml) | 医療 |
| [finance.yaml](finance.yaml) | 金融 |
| [logistics.yaml](logistics.yaml) | 物流 |
| [it-logs.yaml](it-logs.yaml) | ITログ |
| [iot.yaml](iot.yaml) | IoT |

すべてテスト用の架空値です。実在の個人・企業の情報や有効な公的番号を生成するためのものではありません。
仕様は[利用ガイド](../../docs/user/yytoolbox.md)と[同梱ヘルプ](../../apps/yytoolbox/help/help.md)を参照してください。

画面の入力例から定義ファイルを再生成する開発用コマンド:

```powershell
cargo run -p yytoolbox --example export_presets --locked
```

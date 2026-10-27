# yydbmanager 持ち込み実行計画の検証（2026-10-21）

> 検討・作業記録です。現在の操作・内部仕様は[文書案内](../../README.md)を参照してください。

## 対応と取得元

| DB | 形式 | 取得・検証 |
|---|---|---|
| SQL Server | Showplan XML／.sqlplan | 既存のSQL Server 2022実接続取得XML。BOM付きUTF-16 LE／BEも読み込み試験 |
| PostgreSQL | EXPLAIN JSON | デモPostgreSQL 17で同梱psqlスクリプトを実行して取得 |
| MySQL | EXPLAIN JSON v1／v2、TREE | デモMySQL 8.4でJSONとANALYZE TREEを取得。v2はパーサー試験 |
| Oracle | PLAN_TABLE CSV、所定列のカーソル統計CSV | デモOracle Freeで同梱SQL*PlusスクリプトからCSV取得。実測列と単位・未収集の拒否はパーサー試験 |
| Db2 LUW | 指定8列EXPORT DEL、見出し付きCSV | デモDb2 12.1.5で同梱CLPスクリプトを実行してDEL取得 |
| DuckDB | JSONプロファイル／演算子ツリー | 同梱エンジンでPRAGMAによるプロファイルを生成 |
| SQLite | EXPLAIN QUERY PLAN CSV／JSON | 同梱エンジンの結果からCSV生成。JSONは行配列の読み込み試験 |

デモは専用Composeプロジェクト`yydbmanager-demo`を使用し、ユーザーの他のDBコンテナーは変更していません。ファイルは`demo/yydbmanager/plans`、取得スクリプトは`demo/yydbmanager/sql/plan-files`にあります。

## 結果

- 全7DB・8サンプルをDBへ接続せずに読み込み、種類・推定／実測、ノード構造、描画、.yyplan再保存後の情報一致を確認。
- Windowsの非表示UI試験で全8サンプルのインポートと、ズーム・選択・プロパティ・ツールチップ・移動・ナビゲーターを確認。
- Oracle CSVを`--open-plan`で起動してWindowsシェル試験を完了。MySQL TREEを同じ引数で起動し、オフラインのヘルプ表示を確認。
- 8つのヘルプメニューを確認。「実行計画ファイルの形式と取得方法」には7DBの手順と同梱取得スクリプトが表示され、展開用プレースホルダーが残らないことを確認。
- 48件のテスト、clippy、フォーマット検査が成功。既存の任意実行の性能測定・別Dockerスタック試験は通常テストから除外。
- 不正な文字コード、JSON、空の計画、親の欠落・循環、未対応の保存版、32MiB超過、表形式の数値異常を拒否。

読み込み時にSQLは実行しません。CSV／DELはヘルプの列仕様に限定し、db2exfmt、DBMS_XPLAN、罫線付きの画面コピー、PostgreSQL TEXT等は対象外です。Db2の複数親は最小TARGET_IDの単一親として投影します。

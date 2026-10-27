# 持ち込み実行計画のサンプル

DB接続なしで「実行計画 → 実行計画を開く…」から開けます。

| ファイル | DB | 形式 | 種類 |
|---|---|---|---|
| sqlserver-plan.sqlplan | SQL Server 2022 | Showplan XML | 実測 |
| postgresql-plan.json | PostgreSQL 17 | EXPLAIN JSON | 実測 |
| mysql-plan.json | MySQL 8.4 | EXPLAIN JSON | 推定 |
| mysql-actual.txt | MySQL 8.4 | EXPLAIN ANALYZE TREE | 実測 |
| oracle-plan.csv | Oracle Free | PLAN_TABLEのCSV | 推定 |
| db2-plan.db2plan | Db2 LUW 12.1.5 | 指定8列のEXPORT DEL | 推定 |
| duckdb-plan.json | DuckDB同梱版 | JSONプロファイル | 実測 |
| sqlite-plan.csv | SQLite同梱版 | EXPLAIN QUERY PLAN CSV | アクセス方法 |

PostgreSQL・MySQL・Oracle・Db2は2026-10-21にこのデモDBから取得しました。SQL Serverは既存の実接続検証で取得した`apps/yydbmanager/tests/fixtures/sqlserver-hash-sort.sqlplan`と同じファイルです。DuckDB・SQLiteは`tools/prepare.rs`でアプリと同じエンジンから生成しています。実測時間は取得時点の環境の値で、性能の目標値ではありません。

取得方法はアプリの「ヘルプ → 実行計画ファイルの形式と取得方法…」を参照してください。[取得スクリプト](../sql/plan-files/)も収録しています。Db2のファイルは見出しのない8列DELで、拡張子`.db2plan`を維持してください。

```powershell
# リポジトリーのルートで実行
./apps/yydbmanager/target/release/yydbmanager.exe --open-plan demo/yydbmanager/plans/mysql-actual.txt
```

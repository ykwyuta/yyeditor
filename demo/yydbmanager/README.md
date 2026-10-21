# yydbmanager 動作検証デモ

SQL、実行計画、オブジェクトエクスプローラー、ER作図、ファイル入出力を試すための、架空の受注管理データです。**既定ではPostgreSQLだけを起動**し、初期データの投入まで自動で行います。MySQL・SQL Server・Oracle・Db2は追加プロファイルです。SQLite／DuckDBの作成済みファイルも入っています。

## まず起動する

Docker DesktopをLinuxコンテナーで起動し、このフォルダーで実行します。Docker Compose v2以降の`--wait`に対応した版を使用してください。

```powershell
cd demo/yydbmanager
docker compose -p yydbmanager-demo up -d --wait --wait-timeout 120
```

`healthy`になれば、接続とサンプルの読み込みが完了しています。イメージ取得が不要な場合は短時間で起動します。初回ダウンロードの時間は別です。

yydbmanagerを起動し、右側のオブジェクトエクスプローラーで「新規接続…」を選びます。

| 設定 | PostgreSQLの入力値 |
| --- | --- |
| 接続名 | yydbmanager デモ PostgreSQL |
| DB種類 | PostgreSQL |
| ドライバー | 導入済みの64bit PostgreSQL ODBCドライバー名 |
| サーバー | `127.0.0.1` |
| ポート | `25432` |
| データベース | `yydemo` |
| ユーザー | `yy_demo` |
| パスワード | `YyDemo_2026!` |

接続先を保存して接続し、`sql/01-overview.sql`をSQLタブで開いてF5で実行してください。DockerにDBクライアントが入っているため、次の件数検証にはホスト側のODBCドライバーは不要です。

```powershell
./verify.ps1
```

リポジトリーのルートにいる場合の起動コマンドは次のとおりです。

```powershell
docker compose -p yydbmanager-demo -f demo/yydbmanager/compose.yaml up -d --wait --wait-timeout 120
./demo/yydbmanager/verify.ps1
./apps/yydbmanager/target/release/yydbmanager.exe
```

## 他のDBも起動する

このフォルダーで実行します。各コマンドは既定のPostgreSQLと指定プロファイルのDBを起動します。

```powershell
docker compose -p yydbmanager-demo --profile mysql up -d --wait --wait-timeout 180
docker compose -p yydbmanager-demo --profile sqlserver up -d --wait --wait-timeout 240
docker compose -p yydbmanager-demo --profile oracle up -d --wait --wait-timeout 900
docker compose -p yydbmanager-demo --profile db2 up -d --wait --wait-timeout 1200
```

すべて起動する場合：

```powershell
docker compose -p yydbmanager-demo --profile "*" up -d --wait --wait-timeout 1200
./verify.ps1 -Services postgres,mysql,sqlserver,oracle,db2
```

Oracle／Db2はイメージ取得・初回DB作成に時間がかかるため既定で起動しません。Db2だけは公式イメージに初期化フックを追加したローカルイメージをビルドします。SQL Server Developer・Oracle Free・Db2 Communityは各ベンダーの利用条件に従う開発・検証用途です。Db2の`privileged`設定はこの専用コンテナーに限っています。

| DB / プロファイル | ホスト | ポート | DB / サービス名 | ユーザー | 既定スキーマ |
| --- | --- | ---: | --- | --- | --- |
| PostgreSQL / 既定 | 127.0.0.1 | 25432 | yydemo | yy_demo | public |
| MySQL / mysql | 127.0.0.1 | 23306 | yydemo | yy_demo | yydemo |
| SQL Server / sqlserver | 127.0.0.1 | 21433 | yydemo | yy_demo | dbo |
| Oracle / oracle | 127.0.0.1 | 21521 | FREEPDB1 | yy_demo | YY_DEMO |
| Db2 / db2 | 127.0.0.1 | 25000 | YYDEMO | db2inst1 | DB2INST1 |

パスワードはすべて`YyDemo_2026!`です。デモ専用の公開された固定値です。公開ポートはループバックに限定しています。既存のCI検証環境やホストの5432／50000ポートとも分けています。

SQL ServerはODBC Driver 18等の64bitドライバーを選びます。接続文字列の詳細設定では`Encrypt=yes;TrustServerCertificate=yes;`を指定します。OracleはOracle Instant Client ODBC、Db2はIBM Data Server Driver、MySQLはMySQLまたはMariaDBの64bit ODBCドライバーを使用します。ドライバー名はそのPCに登録されている名前に合わせてください。ComposeはWindows側のドライバーをインストールしません。

ポートが衝突する場合は`.env.example`を`.env`へコピーしてホスト側ポートを変更し、Composeを再実行してください。コンテナー内部のポートとデータは変更しません。

## データと確認シナリオ

| テーブル | 件数 | 用途 |
| --- | ---: | --- |
| departments | 3 | 部門 |
| employees | 8 | 社員、部門FK、上司への自己参照FK、日本語・NULL |
| customers | 24 | 顧客、顧客コードの一意制約 |
| products | 8 | 商品、金額・有効フラグ |
| orders | 1200 | 顧客・社員FK、日付・状態、検索用索引 |
| order_items | 3600 | 受注ID＋行番号の複合主キー、商品FK |
| support_tickets | 6 | 顧客コードによる論理関係。物理FKは意図的に未定義 |
| exact_values | 3 | BIGINT上限、2^53を超える整数、DECIMAL(31,8)、NULL・空文字 |
| demo_ready | 1 | データ投入完了のマーカー |

`customer_sales`ビューと複数の索引も作成します。顧客1に900件の受注を偏らせ、顧客24等との検索条件の違いを比較できるようにしています。

| ファイル | 試す内容 |
| --- | --- |
| `sql/00-verify-counts.sql` | 全テーブルの期待件数と実件数の比較 |
| `sql/01-overview.sql` | 複数結果タブ、日本語・NULL、ビュー |
| `sql/02-join-and-plan.sql` | 5表の結合・集計・ソート。推定計画・実測計画をGUIで取得 |
| `sql/03-index-selectivity.sql` | 同じ索引を使う条件で選択性の違いを比較 |
| `sql/04-exact-values-and-logical-relation.sql` | 整数・小数精度、物理FKのない関係の結合 |

SQLファイルは読み取り用です。初期投入用の`init/*.sql`を既存の検証DBにもう一度実行する必要はありません。各DBのオプティマイザーにより、実行計画の形・索引選択・数値は異なります。

SQLiteの`exact_values.amount`は小数の桁を保持するためTEXTで格納しています。他のDBではDECIMAL(31,8)です。通常の金額列は各DBの数値型です。Oracleでは空文字がNULLになるDB本来の挙動になります。

### ER図・列定義編集

1. DBに接続して「表示 → ER図 → DBからER図を取得」を選びます。
2. SQLタブと同じ列の「ER図」タブで作図します。
3. `order_items`の複合主キーと`orders`へのリレーションや、`employees`の自己参照を確認します。
4. `support_tickets.customer_code` → `customers.customer_code`を論理リレーションとして登録するか、「名前から論理リレーションを推薦…」を使います。
5. 主要項目を選び、「主キー・外部キー・主要項目のみ」に切り替えます。
6. YAML保存・再読込、Markdown設計書出力、箱のリサイズや自動サイズを試します。

接続なしですぐ試す場合は、`data/postgres-demo.yyer.yaml`を「ERモデルを開く…」で読み込んでください。配置、日本語のテーブル論理名、主要項目、論理リレーションを登録済みです。`data/sqlite-demo.yyer.yaml`は同梱SQLiteデータ用です。

`data/column-definitions.tsv`は「列定義をまとめて編集…」のグリッドへ貼り付ける専用サンプルです。新しいERモデルにテーブルを追加し、A1セルから貼り付けます。見出しなしで「物理名、論理名、型、NULL許可、既定値、主要項目」の6列です。テーブルへのデータインポートには使用しません。データ型セルのF2／F4でDB別候補を選び、桁数の入力や矩形コピーも試せます。

### ファイル入出力

| ファイル | 設定と期待結果 |
| --- | --- |
| `data/import_sample.csv` | CSV、UTF-8、見出しあり、5行。カンマ・引用符・項目内改行・タブ・空文字・NULL |
| `data/import_sample.tsv` | TSV、UTF-8、同じ5行 |
| `data/import_sample.psv` | 任意区切り `|`、UTF-8、同じ5行 |
| `data/import_sample.parquet` | 同じ5行。文字列列とNULLを保持 |
| `data/exact_values.parquet` | 型付きBIGINT・DECIMAL(31,8)の3行 |
| `data/fixed_sample.dat` + `fixed_sample.cbl` | 固定長、MS932、レコード長34バイト、区切りなし、3行 |

接続後「ファイル → データをインポート…」で形式を選び、対象ファイルを指定します。固定長では続いてコピー句を選択します。生成テーブル名はファイル名を基にするため、最初は空の検証DB／未使用の名前で試してください。同名テーブルがある状態で別形式を繰り返し取り込む場合は、ファイルを別名でコピーして使えます。

`\N`は区切り形式のNULLマーカーです。空文字とは区別します。結果をCSV／TSV／Parquet等に出力して、再度読み込んだ結果や値を比較できます。

### Docker不要のSQLite／DuckDB

yydbmanagerでSQLiteまたはDuckDBの接続先を新規作成し、ファイルに`data/demo.sqlite`または`data/demo.duckdb`の絶対パスを指定します。どちらも作成済みの同じデータを含みます。変更を試す場合はファイルをコピーしてから接続してください。これらの2DBにはDockerも追加ODBCドライバーも不要です。

## DB別の性能分析を試す

DBへ接続せず実行計画だけを試す場合は、[plansの各DBサンプル](plans/README.md)を「実行計画 → 実行計画を開く…」で開いてください。取得元で使うCLI／SQLは[sql/plan-files](sql/plan-files/)にあります。形式と取得手順はアプリの「ヘルプ → 実行計画ファイルの形式と取得方法…」で説明しています。

アプリの「ヘルプ → DB別の性能分析ビュー…」に、各ビューの用途、時間・読み取り数の単位、必要権限、収集設定、累積値の差分の読み方を掲載しています。[ヘルプ原稿](../../apps/yydbmanager/help/help.md#performance)も参照できます。

`sql/02-join-and-plan.sql`や`sql/03-index-selectivity.sql`を数回実行した後、接続DBに合う次のファイルをSQLタブで開き、文ごとに実行してください。

| ファイル | 確認できる内容 |
|---|---|
| [postgres.sql](sql/performance/postgres.sql) | 現在の処理・ブロック元、pg_stat_statements、DB全体の統計 |
| [mysql.sql](sql/performance/mysql.sql) | digest別の時間・走査行数、InnoDBロック待ち、現在の接続 |
| [sqlserver.sql](sql/performance/sqlserver.sql) | 実行中の要求、キャッシュ内の文のCPU・読み取り、待機統計 |
| [oracle.sql](sql/performance/oracle.sql) | アクティブなセッション、子カーソル別のCPU・経過時間・読み取り |
| [db2.sql](sql/performance/db2.sql) | 接続・パッケージキャッシュのメトリクス、ロック待ち |
| [duckdb.sql](sql/performance/duckdb.sql) | エンジンのメモリー・一時ファイル・設定、EXPLAIN ANALYZE |
| [sqlite.sql](sql/performance/sqlite.sql) | 索引のアクセス方法、ページ数・空きページ・ジャーナルモード |

このデモのPostgreSQLは`pg_stat_statements`のプリロードと拡張作成を準備します。MySQLの`yy_demo`にはPerformance Schema／sysのSELECT、ロック待ちビューが使う内部関数のEXECUTE、PROCESSを付与します。SQL ServerにはVIEW SERVER PERFORMANCE STATE、Oracleには必要なV_$ビューのSELECTを初期化時に付与します。Db2はデモのインスタンス所有者で接続します。公開デモ用の設定なので、本番のユーザー設計とは分けてください。

ロック待ちは通常0行です。ここに常時負荷をかける処理や、業務テーブルの更新・統計リセットは含めていません。DuckDBのEXPLAIN ANALYZEは読み取り用SELECTを実行します。SQLiteには他セッションの待機やSQL別累積CPUを返すサーバーDMVはありません。

## 停止・再開・初期化

通常の停止ではデータを残します。

```powershell
docker compose -p yydbmanager-demo --profile "*" stop
# 再開（PostgreSQLだけ）
docker compose -p yydbmanager-demo up -d --wait
# 状態・ログ
docker compose -p yydbmanager-demo --profile "*" ps
docker compose -p yydbmanager-demo logs --tail 100 postgres
```

初期データは空の専用ボリュームに1回だけ投入します。停止・起動で二重投入しません。投入SQLを変更して最初から作り直す場合に限り、次を実行します。**このデモの全DBと保存データを削除します。**

```powershell
docker compose -p yydbmanager-demo --profile "*" down -v
# その後、必要なプロファイルでupする
```

起動失敗時は`logs`を確認してください。ポート使用中は`.env`で変更、初期化途中の失敗はログの原因を解消してから専用ボリュームを作り直します。`healthy`判定にはDBプロセスの起動だけでなく、デモ投入完了も含めています。

## データを再生成する（開発者向け）

通常利用では不要です。テキストSQL・CSV等はPython標準ライブラリーだけで再生成できます。

```powershell
# リポジトリーのルートで実行
python demo/yydbmanager/tools/generate.py
cargo run --manifest-path apps/yydbmanager/Cargo.toml -p yy-db-core --example prepare_demo -- demo/yydbmanager demo/yydbmanager/generated
```

後者はアプリと同じSQLite／DuckDB・入出力ライブラリーを使い、件数、精度、計画、Parquet、固定長、ER YAMLを検証してバイナリを生成します。既存ファイルは上書きしません。`generated/`の出力を確認してから`data/`の配布サンプルを更新してください。JSONの期待件数は`data/expected-counts.json`です。

`verify.ps1 -Services ...`は起動中の指定DBの全テーブル件数、共通サンプルSQLとDB別の性能分析SQLを読み取り検証し、使用イメージID・件数・実行ファイル一覧を`reports/verification.json`へ記録します。Oracleは日本語の投入結果も検証します。プロファイルを指定しただけで未起動のDBへ接続することはありません。Db2 CLPの「0行取得」による終了コード1は成功として扱い、SQLエラーは失敗にします。

## 動作確認（2026-10-21）

- PostgreSQL 17、MySQL 8.4、SQL Server 2022、Oracle Free、Db2 12.1.5を専用Composeプロジェクトで起動し、デモ用ユーザーで9テーブルの期待件数と、共通サンプル4ファイル＋DB別性能分析SQLを実行しました。
- 同梱SQLite／DuckDBで投入・件数・整数／小数精度・計画取得・性能分析SQLを確認しました。CSV／Parquet／固定長とER YAMLの読み書きもアプリのライブラリーで検証しました。
- PostgreSQLは取得済みイメージ・空の専用ボリュームで約6秒で起動しました。初回ダウンロード、PC性能やDockerの設定によって所要時間は変わります。

検証時の全5DBはホストOSのODBCではなく、各コンテナーの標準クライアントを使用しています。Windows側ドライバーの導入・接続確認は別途必要です。大規模データでの負荷試験や、本番環境の監視設定を保証するものではありません。

## 公式イメージの参照先

- [PostgreSQL公式イメージ](https://hub.docker.com/_/postgres)
- [MySQL公式イメージ](https://hub.docker.com/_/mysql)
- [Microsoft: SQL Serverコンテナーの構成](https://learn.microsoft.com/en-us/sql/linux/containers/configure)
- [Oracle公式Dockerイメージと初期化スクリプト](https://github.com/oracle/docker-images/tree/main/OracleDatabase/SingleInstance)
- [IBM Db2 Community Editionコンテナー](https://www.ibm.com/docs/en/db2/12.1.0?topic=system-linux)

タグを固定したPostgreSQL 17／MySQL 8.4／SQL Server 2022／Db2 12.1.5を使用し、Oracleは公式Freeの`latest-lite`を使用します。パッチ更新やOracleの版は取得時点で変わるため、検証時のイメージIDも保存します。

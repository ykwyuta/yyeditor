# yydbmanager ヘルプ

Windows向けDBクライアント。説明は実行ファイルに埋め込まれ、オフラインでも利用できます。

**目次**

- [使い方・接続・SQL・ER設計](#usage)
- [Rust直接接続](#native-connections)
- [実行計画の取得と読み方](#plans)
- [実行計画ファイルを持ち込む・DB別の取得方法](#plan-files)
- [DB別の性能分析ビュー・待機・重いSQL](#performance)
- [Oracleの権限と実測取得の準備](#oracle)
- [Db2の権限と実測収集の準備](#db2)
- [実測取得のトラブルシューティング](#troubleshooting)
- [ERモデルYAMLファイル仕様と読み込み](#er-yaml)
- [列定義の表形式編集・Excelとのコピー](#er-columns)

## DB別の性能分析ビュー {#performance}

### 調査の進め方

右側のオブジェクトエクスプローラーで保存した接続先を右クリックし、「性能分析SQL（DB名）」から調べたい内容を選べます。その接続先の種類に合うSQLだけを表示します。接続先配下のオブジェクトからも利用できます。未接続の保存済み接続先でもテンプレートを開けます。

選択すると、対象接続先・SQLの用途・必要権限等のコメントを付けて新しいSQLタブを作成します。既存のSQLやER図は保持します。SQLは編集でき、テンプレート選択時には接続の切り替えやSQL実行をしません。対象接続先へ接続し、内容を確認してF5で実行してください。他の接続先のままでは実行を止め、対象接続先を案内します。接続設定を変更した場合はメニューからテンプレートを作り直します。別のSQLファイルをそのタブへ開いた場合はテンプレートの対象指定を解除します。

| DB | 右クリックから選べる分析 |
|---|---|
| SQL Server | 実行中の要求・ブロック元、重いSQL、サーバー全体の待機 |
| PostgreSQL | 現在の処理・長いトランザクション・ブロック元、pg_stat_statements、DB全体の統計 |
| MySQL | 重いSQL・走査行数・一時テーブル、InnoDBロック待ち、実行中の接続 |
| Oracle | アクティブセッション・待機・ブロック元、子カーソル別の重いSQL |
| Db2 LUW | 接続ごとのCPU・活動・待機、パッケージキャッシュ、ロック待ち |
| DuckDB | 現在のメモリー・一時領域、一時ファイル、スレッド数等の設定 |
| SQLite | ページ数・サイズ・空きページ、他接続の変更検知、ジャーナルモード |

これらは特定の業務テーブルに依存しない監視用のSELECT／参照用PRAGMAです。権限付与・統計リセット・監視設定の変更は含めません。SQLiteとDuckDBにはサーバー型DBと同じセッション待機ビューはないため、利用できる情報に合わせた項目を表示します。

「ヘルプ → DB別の性能分析ビュー…」から開けます。この節のSQLは、接続後SQLタブへコピーして必要な文だけ選択し、実行します。ヘルプ表示によるSQL実行、権限付与、設定変更はありません。実行計画の可視化は[実行計画の取得と読み方](#plans)も参照してください。

1. 遅い処理が進行中なら、実行中セッションとロック待ちを確認します。
2. 繰り返し発生する負荷なら、SQL単位の累積統計を比較します。
3. 対象SQLを絞って推定計画を取得します。実測計画は対象SQLを実際に実行するため、更新系SQLや重いSQLは影響を確認してから取得します。
4. 変更前後は同じ条件・パラメーター・取得件数で比較します。DB内の時間と、通信・全件取得・結果表示まで含むクライアントの時間を分けます。

| DB | 現在の処理・待機 | SQL単位の集計 | 補助情報 |
|---|---|---|---|
| SQL Server | sys.dm_exec_requests | sys.dm_exec_query_stats | sys.dm_os_wait_stats、Query Store |
| PostgreSQL | pg_stat_activity、pg_blocking_pids | pg_stat_statements（拡張） | pg_stat_database |
| MySQL | performance_schema.threads、sys.innodb_lock_waits | events_statements_summary_by_digest | Performance Schemaの計測設定 |
| Oracle | V$SESSION | V$SQL | V$SQL_PLAN_STATISTICS_ALL |
| Db2 LUW | MON_GET_CONNECTION、MON_LOCKWAITS | MON_GET_PKG_CACHE_STMT | MON_GET_ACTIVITY、アクティビティイベントモニター |
| DuckDB | duckdb_memory()、duckdb_temporary_files() | EXPLAIN ANALYZEで個別取得 | duckdb_settings() |
| SQLite | サーバーセッションの監視ビューはありません | EXPLAIN QUERY PLANでアクセス方法を確認 | PRAGMAでページ情報・設定を確認 |

### 累積値は差分で比較する

累積CPU時間の大きなSQLが、現在遅いとは限りません。時刻・DB・サーバー／メンバー・SQL識別子・カウンターを2回保存し、一定期間の差分を比較します。期間内の平均時間は「時間の差分 ÷ 実行回数の差分」です。実行回数の差分が0なら平均は算出しません。再起動・統計リセット・キャッシュ追い出し・再コンパイルなどで同一系列として比較できなくなったら、差分を破棄して取り直します。

集計単位も異なります。SQL Serverはキャッシュ内の文、PostgreSQLはDB・ユーザー・queryid等、MySQLはスキーマ・digest、OracleはSQL_ID・子カーソル、Db2はメンバー・実行セクションを識別します。表示SQLの文字列だけで合算しないでください。監視用SQL自身も統計に入ります。SQL本文には実データのリテラルを含むことがあるため、調査資料へ転載する際は内容を確認します。

CPU時間と経過時間、親子ノードの時間、並列ワーカーの時間は単純に足し合わせられません。待機時間の大きさだけで異常と判断せず、アイドル待機、観測期間、処理量、ブロック元を確認します。空の結果や0は、負荷がないほかに権限・収集無効・キャッシュ消失なども原因になります。

### SQL Server（2022を基準）

**現在の要求**は`sys.dm_exec_requests`で確認します。`blocking_session_id`が正ならブロック元セッションを追い、負の特殊値は公式仕様を確認します。`wait_type`と`wait_time`はその要求の現在の待機情報で、要求全体の待ち時間ではありません。次の時間列はミリ秒です。

```sql
SELECT TOP (20) r.session_id, r.status, r.blocking_session_id,
       r.wait_type, r.wait_time AS wait_ms, r.cpu_time AS cpu_ms,
       r.total_elapsed_time AS elapsed_ms, r.logical_reads,
       t.text AS batch_text
FROM sys.dm_exec_requests AS r
OUTER APPLY sys.dm_exec_sql_text(r.sql_handle) AS t
WHERE r.session_id <> @@SPID
ORDER BY r.total_elapsed_time DESC;
```

**完了したSQLの累積**は`sys.dm_exec_query_stats`です。CPU・経過時間の元の単位はマイクロ秒なので、例では1000で割ってミリ秒にしています。論理読み取りはページ数です。`batch_text`はバッチ全体で、各行の対象文は`statement_start_offset`等で区別します。キャッシュから消えた統計は残りません。

```sql
SELECT TOP (20) qs.sql_handle, qs.statement_start_offset,
       qs.execution_count, qs.total_worker_time / 1000.0 AS cpu_ms,
       qs.total_elapsed_time / 1000.0 AS elapsed_ms,
       qs.total_elapsed_time / 1000.0 / NULLIF(qs.execution_count, 0) AS average_ms,
       qs.total_logical_reads, qs.total_physical_reads, t.text AS batch_text
FROM sys.dm_exec_query_stats AS qs
CROSS APPLY sys.dm_exec_sql_text(qs.sql_handle) AS t
ORDER BY qs.total_worker_time DESC;
```

**サーバー全体の完了した待機**は`sys.dm_os_wait_stats`で、起動／リセット以降の累積ミリ秒です。現在の待機一覧でも、特定DBだけの統計でもありません。`signal_wait_time_ms`は`wait_time_ms`に含まれます。アイドル系も含むため、上位を一律に問題扱いしません。

```sql
SELECT TOP (20) wait_type, waiting_tasks_count, wait_time_ms, signal_wait_time_ms
FROM sys.dm_os_wait_stats
WHERE waiting_tasks_count > 0 ORDER BY wait_time_ms DESC;
```

**準備・権限**：SQL Server 2022以降のこれらのサーバーDMVは`VIEW SERVER PERFORMANCE STATE`、以前の版は`VIEW SERVER STATE`が基本です。Azure SQLはサービス・階層・DBスコープで異なります。管理者に必要な範囲だけの付与を依頼してください。履歴を継続保存したい場合はQuery Storeを検討します。yydbmanagerはQuery Storeを自動で有効化せず、統計をリセットしません。

公式：[実行中の要求](https://learn.microsoft.com/en-us/sql/relational-databases/system-dynamic-management-objects/sys-dm-exec-requests-transact-sql)、[文の累積統計](https://learn.microsoft.com/en-us/sql/relational-databases/system-dynamic-management-objects/sys-dm-exec-query-stats-transact-sql)、[待機統計](https://learn.microsoft.com/en-us/sql/relational-databases/system-dynamic-management-objects/sys-dm-os-wait-stats-transact-sql)、[Query Store](https://learn.microsoft.com/en-us/sql/relational-databases/performance/monitoring-performance-by-using-the-query-store)。

### PostgreSQL（17を基準）

**現在の処理**は`pg_stat_activity`、ブロック元は`pg_blocking_pids(pid)`です。`wait_event_type`／`wait_event`は現在の待機先で、累積待機時間ではありません。`idle in transaction`も確認し、長時間トランザクションを探します。非active状態の`query`は直前のSQLで、`query_age`は実行時間とは限りません。

```sql
SELECT pid, usename, state, wait_event_type, wait_event,
       clock_timestamp() - query_start AS query_age,
       clock_timestamp() - xact_start AS transaction_age,
       pg_blocking_pids(pid) AS blocking_pids, query
FROM pg_stat_activity
WHERE datname = current_database() AND pid <> pg_backend_pid()
  AND state <> 'idle'
ORDER BY xact_start NULLS LAST;
```

**SQLの累積**には`pg_stat_statements`拡張を使います。時間はミリ秒、読み書きはブロック数です。キャッシュ読み取りと実ディスクI/Oは同義ではありません。I/O時間の計測には`track_io_timing`、計画時間には`pg_stat_statements.track_planning`の設定が必要で、無効時の0を「時間がかからなかった」と解釈しません。

```sql
SELECT userid, queryid, calls, total_exec_time, mean_exec_time, rows,
       shared_blks_hit, shared_blks_read, temp_blks_written, query
FROM pg_stat_statements
WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database())
ORDER BY total_exec_time DESC LIMIT 20;
```

**DB全体**のトランザクション、一時ファイル、デッドロックは`pg_stat_database`で確認します。累積値と`stats_reset`を一緒に保存します。統計反映には遅れがあり、同じトランザクション内で統計スナップショットが再利用される場合があります。時間をあけて別トランザクションで採取します。

```sql
SELECT datname, xact_commit, xact_rollback, blks_hit, blks_read,
       temp_bytes, deadlocks, stats_reset
FROM pg_stat_database WHERE datname = current_database();
```

**準備・権限**：自分以外の活動・SQL本文の参照には`pg_read_all_stats`またはそれを含む`pg_monitor`等を管理者に相談します。拡張は、既存設定を保持して`shared_preload_libraries`へ`pg_stat_statements`を追加しサーバーを再起動した後、対象DBで`CREATE EXTENSION pg_stat_statements;`を実施します。`compute_query_id`も`auto`または`on`等で識別子を生成できる必要があります。拡張作成だけではプリロードの代わりになりません。未準備なら最初と最後の標準ビューのSQLだけ実行できます。

公式：[統計の監視と権限](https://www.postgresql.org/docs/17/monitoring-stats.html)、[pg_stat_statementsの導入と列仕様](https://www.postgresql.org/docs/17/pgstatstatements.html)。

### MySQL（8.4を基準）

**SQLの累積**はPerformance Schemaの`events_statements_summary_by_digest`です。digestはリテラル等を正規化したSQLの識別子で、スキーマと併せて扱います。タイマーはピコ秒なので例では10の12乗で割って秒にしています。`SUM_ROWS_EXAMINED`に対し`SUM_ROWS_SENT`が少ないSQL、一時ディスクテーブルの多いSQLは、対象を絞る候補です。これらの数値だけで索引追加の効果を断定せず計画と照合します。

```sql
SELECT SCHEMA_NAME, DIGEST, DIGEST_TEXT, COUNT_STAR,
       SUM_TIMER_WAIT / 1000000000000.0 AS total_seconds,
       AVG_TIMER_WAIT / 1000000000000.0 AS average_seconds,
       SUM_ROWS_EXAMINED, SUM_ROWS_SENT, SUM_CREATED_TMP_DISK_TABLES
FROM performance_schema.events_statements_summary_by_digest
WHERE SCHEMA_NAME = DATABASE() AND DIGEST IS NOT NULL
ORDER BY SUM_TIMER_WAIT DESC LIMIT 20;
```

**InnoDBのロック待ち**は`sys.innodb_lock_waits`で、待ち側・保持側の接続IDを追います。行がない場合、観測時点の対象ロック待ちがないことは分かりますが、過去の待ちやメタデータロックまで否定できません。メタデータロックは`sys.schema_table_lock_waits`も確認します。

```sql
SELECT waiting_pid, blocking_pid, locked_table_schema, locked_table_name,
       wait_age, waiting_query
FROM sys.innodb_lock_waits LIMIT 20;
```

**現在の接続**は次のように確認できます。`PROCESSLIST_TIME`は現在の状態にいた秒数で、SQLの累積CPU時間ではありません。

```sql
SELECT PROCESSLIST_ID, PROCESSLIST_USER, PROCESSLIST_DB,
       PROCESSLIST_COMMAND, PROCESSLIST_TIME, PROCESSLIST_STATE,
       PROCESSLIST_INFO
FROM performance_schema.threads
WHERE TYPE = 'FOREGROUND' AND PROCESSLIST_ID <> CONNECTION_ID()
  AND PROCESSLIST_COMMAND <> 'Sleep';
```

**準備・権限**：Performance Schemaが有効で、必要なstatement instruments・consumersとdigest収集が有効である必要があります。`performance_schema`の対象テーブルへの`SELECT`、sysビューと参照先の権限を依頼します。ロック待ちビューは`sys.quote_identifier`、`sys.format_statement`、`sys.sys_get_config`等の内部関数の`EXECUTE`も必要です。SELECTだけでは「ビューを参照できない」エラーになることがあります。InnoDBトランザクション等の他セッション情報には`PROCESS`も必要です。設定確認には`setup_instruments`／`setup_consumers`を参照し、計測設定の更新や集計テーブルの`TRUNCATE`を通常調査用SQLに混ぜないでください。

公式：[文の集計テーブル](https://dev.mysql.com/doc/refman/8.4/en/performance-schema-statement-summary-tables.html)、[ロック待ちビュー](https://dev.mysql.com/doc/refman/8.4/en/sys-innodb-lock-waits.html)、[threadsテーブル](https://dev.mysql.com/doc/refman/8.4/en/performance-schema-threads-table.html)、[sysの利用条件・権限](https://dev.mysql.com/doc/refman/8.4/en/sys-schema-prerequisites.html)、[Performance Schemaの設定](https://dev.mysql.com/doc/refman/8.4/en/performance-schema-configuration.html)。

### Oracle（19c以降／デモはFree）

**現在の処理**は`V$SESSION`の`SQL_ID`、`EVENT`、`WAIT_CLASS`、`STATE`、`BLOCKING_SESSION`を確認します。`STATE`が待機中でない場合のイベントは、現在待っている原因とは限りません。調査用セッション自身も表示されます。

```sql
SELECT sid, serial#, username, sql_id, event, wait_class, state,
       blocking_session
FROM v$session
WHERE type = 'USER' AND username IS NOT NULL AND status = 'ACTIVE';
```

**SQLの累積**は`V$SQL`です。1行は子カーソル単位なので`SQL_ID`だけで同じ計画と決めつけず、`CHILD_NUMBER`と`PLAN_HASH_VALUE`を併せて確認します。CPU・経過時間はマイクロ秒を秒に換算しています。並列実行では経過時間に複数プロセスの時間が含まれ、クライアントで測った時間とは一致しません。カーソルの消失・再ロードにも注意します。

```sql
SELECT sql_id, child_number, plan_hash_value, executions,
       cpu_time / 1000000 AS cpu_seconds,
       elapsed_time / 1000000 AS elapsed_seconds,
       elapsed_time / 1000000 / NULLIF(executions, 0) AS average_seconds,
       buffer_gets, disk_reads, rows_processed, sql_text
FROM v$sql
WHERE parsing_schema_name = USER
ORDER BY elapsed_time DESC FETCH FIRST 20 ROWS ONLY;
```

**準備・権限**：管理者が接続対象PDBで、調査ユーザーに`GRANT SELECT ON SYS.V_$SESSION TO 調査ユーザー;`と`GRANT SELECT ON SYS.V_$SQL TO 調査ユーザー;`を付与する方法があります。`V$...`は公開シノニム名で、付与対象は`SYS.V_$...`です。実測計画用の追加権限は[Oracleの準備](#oracle)を参照してください。RACの`GV$`は`INST_ID`、CDB／PDBは`CON_ID`等を含め、参照範囲を区別します。

この例はAWR・ASH・Real-Time SQL Monitoringを使用しません。それらの履歴や監視機能はエディション・契約・管理パックの利用条件を管理者が確認してから使用してください。

公式：[V$SESSION](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/V-SESSION.html)、[V$SQL](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/V-SQL.html)、[ライセンス情報](https://docs.oracle.com/en/database/oracle/oracle-database/19/dblic/Licensing-Information.html)。

### Db2 LUW（11.5／12.1）

**接続の累積**は`MON_GET_CONNECTION`、**SQLセクションの累積**は`MON_GET_PKG_CACHE_STMT`で確認します。以下はLUW用で、Db2 for z/OS用のSQLではありません。`-2`は全アクティブメンバーを対象とするため、SQLの行は`MEMBER`と`EXECUTABLE_ID`を併せて区別します。

```sql
SELECT member, application_handle, total_cpu_time, total_act_time, total_act_wait_time
FROM TABLE(SYSPROC.MON_GET_CONNECTION(NULL, -2)) AS m
ORDER BY total_act_wait_time DESC FETCH FIRST 20 ROWS ONLY;

SELECT member, HEX(executable_id) AS executable_id, num_executions,
       num_coord_exec_with_metrics, total_act_time, total_act_wait_time,
       total_cpu_time, rows_read, rows_returned, VARCHAR(stmt_text, 1000) AS stmt_text
FROM TABLE(SYSPROC.MON_GET_PKG_CACHE_STMT(NULL, NULL, NULL, -2)) AS m
ORDER BY total_act_time DESC FETCH FIRST 20 ROWS ONLY;
```

`TOTAL_CPU_TIME`はマイクロ秒、`TOTAL_ACT_TIME`／`TOTAL_ACT_WAIT_TIME`はミリ秒です。異なる単位のまま割合を計算しないでください。`NUM_EXECUTIONS`と、メトリクスが収集された調整ノードでの実行回数`NUM_COORD_EXEC_WITH_METRICS`は意味が異なります。収集範囲を確認せず前者で時間を割ると平均値を誤ります。`ROWS_READ`と`ROWS_RETURNED`の差が大きいSQLを調査し、計画の結合・索引と照合します。

**ロック待ち**は`SYSIBMADM.MON_LOCKWAITS`で待ち側・保持側のアプリケーションハンドルを追えます。実行中の活動を詳しく見る場合は`MON_GET_ACTIVITY`も利用できます。パッケージキャッシュの情報は履歴保存ではありません。

```sql
SELECT req_application_handle, hld_application_handle, req_stmt_text
FROM SYSIBMADM.MON_LOCKWAITS FETCH FIRST 20 ROWS ONLY;
```

**準備・権限**：MON_GET系関数の必要な`EXECUTE`権限、または`SQLADM`／`DBADM`／`DATAACCESS`等の適用可能な権限を、関数ごとの公式仕様に従って管理者へ依頼します。MON_LOCKWAITSビューの参照権限も別に確認します。`MON_ACT_METRICS`等の収集設定が無効だと該当メトリクスは取得できません。実測計画のノード情報には追加の収集設定が必要なので[Db2の準備](#db2)を参照してください。通常調査で`FLUSH PACKAGE CACHE`を実行してはいけません。

公式：[Db2モニター関数](https://www.ibm.com/docs/en/db2/11.5.x?topic=functions-monitor-mon-routines)、[パッケージキャッシュ関数・権限](https://www.ibm.com/docs/en/ias?topic=mpf-mon-get-pkg-cache-stmt-table-function-get-package-cache-statement-metrics)、[IBMの性能チューニング資料](https://public.dhe.ibm.com/ps/products/db2/info/vr115/pdf/en_US/db2_perf_tune_115.pdf)。

### DuckDB（同梱エンジン）

組み込み型なので、外部DBサーバーの全セッションや待機を列挙するDMVはありません。同じエンジン内の現在のメモリーと一時ファイルをテーブル関数で確認できます。単位はバイトで、`duckdb_memory()`はOS全体やプロセス全体のメモリー使用量ではありません。処理終了後の0は実行中のピークが0だったことを示しません。

```sql
SELECT tag, memory_usage_bytes, temporary_storage_bytes
FROM duckdb_memory() ORDER BY memory_usage_bytes DESC;

SELECT path, size FROM duckdb_temporary_files();

SELECT name, value FROM duckdb_settings()
WHERE name IN ('threads', 'memory_limit', 'temp_directory');
```

個別SQLは「実測して計画」または次の`EXPLAIN ANALYZE`で確認します。これはSQLを実行します。演算子の時間は秒で、並列実行や親子の包含を考慮し、ノード時間を足して実時間にしないでください。メモリー上限等を変える前に現設定を記録して比較します。SQL Serverのような権限付与や追加サーバーは不要です。

```sql
EXPLAIN ANALYZE SELECT customer_id, COUNT(*) AS order_count
FROM orders GROUP BY customer_id;
```

公式：[DuckDBの情報取得関数](https://duckdb.org/docs/stable/sql/meta/duckdb_table_functions)、[EXPLAIN ANALYZE](https://duckdb.org/docs/stable/guides/meta/explain_analyze)。使用可能な列は同梱版に依存します。上の例は本アプリ同梱の1.4系で確認しています。

### SQLite（同梱エンジン）

SQLiteにもサーバー型DBの動的性能ビューはありません。`EXPLAIN QUERY PLAN`で`SCAN`（走査）／`SEARCH`（条件による探索）、使用索引、一時B-treeの有無を確認します。この結果は推定アクセス方法で、実測CPU・待機時間や実行ノードごとの実行行数ではありません。

```sql
EXPLAIN QUERY PLAN SELECT order_id, ordered_on FROM orders
WHERE customer_id = 1 ORDER BY ordered_on;

PRAGMA page_count;
PRAGMA page_size;
PRAGMA freelist_count;
PRAGMA data_version;
PRAGMA journal_mode;
```

`page_count × page_size`はDBの論理ページ容量の目安です。WAL・一時ファイルまで含む実ディスク容量ではありません。`freelist_count`は再利用可能な空きページ数で、多いだけで異常ではありません。`data_version`は同じ接続で比較して他接続のコミットによる変更を検知する値で、グローバルな更新回数ではありません。上のPRAGMAは値の参照だけです。

`sqlite_stmt`仮想テーブルやC APIの`sqlite3_stmt_status`等にはビルド条件・API呼び出しが必要です。本アプリのSQL画面から常に利用できるビューとしては扱いません。ロックエラーはトランザクションの保持時間・他接続の書き込みを確認し、SQL実行の所要時間は同じ取得条件で別途比較してください。

公式：[EXPLAIN QUERY PLAN](https://www.sqlite.org/eqp.html)、[PRAGMA一覧](https://www.sqlite.org/pragma.html)、[sqlite_stmtの利用条件](https://www.sqlite.org/stmt.html)。

### 検証用データで試す

リポジトリーの`demo/yydbmanager/README.md`にDocker Composeの起動方法と接続情報があります。`sql/performance/`にこの節と同じDB別のSQLを収録しています。PostgreSQLのデモはpg_stat_statementsを準備済み、MySQL・SQL Server・Oracleのデモ調査ユーザーには上のSQL用の参照権限を付与します。これらはローカル検証環境の設定で、本番環境へ権限・設定を自動反映する仕組みではありません。

まず`sql/02-join-and-plan.sql`等を複数回実行し、DBに合う性能分析SQLを実行してください。負荷のない小さなデータでは時間が短く、ロック待ちは通常0行です。累積値の差、顧客ごとの件数の偏り、実行計画を比較する練習に使用できます。

## 使い方 {#usage}

### SQLとER図をタブで切り替える

SQLクエリとER図は主画面上部の同じタブ列に並びます。「ERモデルを新規作成」「ERモデルを開く」、またはDBからER図を取得すると「ER図」タブを開きます。ER図タブではSQLエディタと結果タブを隠し、文書領域全体で作図します。右側のオブジェクトエクスプローラーも引き続き使用できます。

SQLタブに戻るとSQL入力と結果・実行計画の表示を復元します。ER図の配置、定義、選択、ズームやスクロール位置、元に戻す履歴はタブ切り替えで保持します。ER図の作図中にSQLの処理が完了しても図を置き換えません。「DDLをSQLタブへ生成」は新しいSQLタブを選択し、元のER図はER図タブに残ります。ER図タブではSQL実行・SQL保存を無効にします。

### 接続先を登録して接続する

初期設定には「demo / SQLite」「demo / DuckDB」と、`demo/yydbmanager/compose.yaml`の5種類の外部DBに対応する「demo / PostgreSQL」「demo / MySQL」「demo / SQL Server」「demo / Oracle」「demo / Db2」を登録しています。右側のオブジェクトエクスプローラーで接続名をダブルクリックします。外部DBは事前にComposeで対象DBを起動してください。アプリからコンテナーの起動や自動接続は行いません。既存設定にも一度だけ追加し、同名設定を上書きせず、編集・削除後は再追加しません。

SQLiteとDuckDBはDocker不要で、すぐ接続できます。作成済みの受注デモデータを実行ファイルに同梱し、初回に接続設定ファイルの隣の`demo`フォルダーへコピーします。通常は`%APPDATA%/yydbmanager/demo/demo.sqlite`と`demo.duckdb`です。既存ファイルは上書きせず、デモ上での編集内容を保持します。5種類の外部DBのみを登録済みの環境にも、この2接続先だけを一度追加します。

デモ接続はRust直接接続、ホスト`127.0.0.1`、公開デモ用パスワード`YyDemo_2026!`、追加設定`{"tls":"disable"}`を使用します。DB別のポート・ユーザー・DB名は接続先の編集画面で確認できます。Composeのポートやパスワードを変更した場合は設定も編集してください。
1. 右側のオブジェクトエクスプローラーで「新規接続…」を押します。
   ツリーの右クリックメニューから「接続先を新規作成…」でも開けます。
2. 接続名とDB種類を選びます。SQLite／DuckDBはファイルのパスを指定します。
   空欄または :memory: は保存しないメモリDBです。
3. 外部DBはRust直接接続を使用し、サーバー・ポート・
   DB名（Oracleはサービス名）・ユーザー・パスワードを入力します。
   追加のDBクライアントやJavaは不要です。
4. 「保存」するとツリーに接続名が追加されます。ダブルクリックで接続します。
5. 編集・削除・接続はツリーの右クリックメニューから行えます。
   設定を編集しても、現在のDB接続は再接続するまで継続します。
接続の詳細は設定ダイアログで確認し、上部には常時表示しません。
パスワードは伏せて表示します。設定はユーザー単位で暗号化します。
旧版の接続文字列は使用できません。接続設定を開き、サーバー・ポート・DB名・認証情報を入力し直して保存してください。

### Rust直接接続 {#native-connections}

外部DBはRust製ドライバーで接続します。SQLiteとDuckDBは従来の同梱エンジンを使います。

| DB | 直接接続の実装 |
| --- | --- |
| PostgreSQL | Rustのpostgresドライバー。整数・NUMERICは浮動小数点に変換せず保持します。 |
| MySQL | Rustのmysqlドライバー。複数の結果セットに対応します。 |
| SQL Server | RustのTDSドライバーtiberius-ng。Windowsではユーザー名を空欄にするとWindows統合認証です。 |
| Oracle | Oracle公式のPure Rust Thinドライバーoracledb。Instant Clientは不要です。現在はベータ版のドライバーです。 |
| Db2 LUW | rust-db2-driverによるDRDAクライアント。IBM CLIやJCCは不要です。z/OS・IBM iは未検証です。 |

追加設定はJSONです。空欄ではTLSを必須にします。TLSが無効なローカル検証用DBには`{"tls":"disable"}`を明示します。PostgreSQLでは`{"tls":"prefer"}`も指定できます。CA証明書は`{"ca_file":"C:/certs/ca.pem"}`で指定します。Oracle／SQL ServerではOSの証明書ストアへ登録してください。自己署名証明書の検証を省略する設定は`{"trust_server_certificate":true}`です（Oracle／Db2では指定できません。Db2ではca_fileを使用してください）。

OracleのTLSはTCPS接続です。接続先のTCPSリスナーのポートを入力してください。Db2のTLS接続もサーバーのTLSポートを指定します。Db2のtlsはrequireまたはdisableで、暗号化できない場合の自動フォールバックはありません。これらの設定とパスワードは従来と同じDPAPI暗号化された接続プロファイルへ保存します。

Db2直接接続はDb2 LUWを対象とし、認証情報とDB名はLatin-1の制御文字を含まない255文字以内です。整数・DECIMAL・浮動小数点・文字列・日付時刻・固定長／可変長バイナリを扱います。BLOB/CLOB/XML・DECFLOAT等は未対応です。問い合わせ結果はドライバー内で全件をメモリへ読み込みます。列名が返らない場合はCOLUMN1等で表示します。インポートはDRDAのパラメーターバインドを使用します（DECIMALは31桁まで、入力データは1行32,757バイトまで）。実測計画はSQLブロック内でEXPLAIN_FROM_ACTIVITYを呼び、接続内の一時表から正確なステートメントキーを取得します。推定計画の述語は1件32,672バイトまでで、超過時はエラーにします。

停止操作はPostgreSQLではサーバーへ取消要求を送り、Db2ではTLS接続も含めて接続を閉じ、ブロック中のI/Oを中断します（再接続が必要です。サーバー上の即時停止やCOMMITの成否を保証する操作ではありません）。MySQL・SQL Server・Oracleでは行取得時にも取消状態を確認します。直接接続の読み取り専用指定はDB側の権限を使用してください。インポートは別接続で行い、SQLタブのトランザクションを確定しません。

### オブジェクト一覧を更新する
接続中のツリーを右クリックし「オブジェクト一覧を更新」を選びます。
DBの表・ビューなどを再取得する操作です。SQLの再実行やデータ更新は行いません。

### SQLと結果
「ファイル」でSQLタブの追加、SQLの読込・保存を行います。
SQLを入力してF5、または「クエリ → 実行」で実行します。
選択範囲がある場合は選択したSQLを実行します。
右側のオブジェクト エクスプローラーで表やビュー、列を確認できます。
結果の取込・出力は「ファイル」メニューから行います。
「データをインポート…」「結果をエクスポート…」を選ぶとファイル形式の設定が開きます。
CSV／TSVはUTF-8です。「任意区切り」の場合は区切り文字を1文字指定します。
COBOL固定長の場合は文字コードを指定し、データファイルの後にコピー句を選びます。
「ファイルを選択…」で次へ進みます。キャンセルした場合は取込・出力しません。
実行結果は「結果 1（○行）」などのタブで切り替えます。選択したタブがエクスポート対象です。
結果がない間は結果タブを表示しません。

実行計画
「実行計画」で推定／実測取得、保存計画の読込・保存を行います。
OracleとDb2の準備は、それぞれのヘルプ項目を参照してください。
Db2の準備SQLは「実行計画 → Db2実測収集の準備SQL」で開けます。

### ER図
### ゼロから設計する場合（DB接続不要）
1. 「ファイル → ERモデルを新規作成…」で対象のDB種類を選びます。
2. 空の図を右クリックし「テーブルを追加…」で物理名・論理名を指定します。
   最初のid列と主キーは自動で追加され、後から変更できます。
3. 表をクリックして選び、右クリック →「モデルを編集」から列・キー・索引を編集します。
4. リレーションは参照元の表を選び、同メニューの「リレーションを追加…」で作成します。
5. 表をドラッグして配置し、「ファイル → ERモデルを保存…」で .yyer.yaml に保存します。
6. DBの作成SQLが必要な場合は「表示 → ER図 → DDLをSQLタブへ生成」を選びます。
   SQLを生成するだけではDBは変更されません。接続先とSQLを確認して実行します。

### 既存のDBから取得する場合
「表示 → ER図」から表の選択、表示切替、モデル編集、保存やDDL生成を行います。
DDL生成は新規SQLタブへの出力です。内容を確認してから実行してください。

### 主要項目とER図の絞り込み
表を選び、右クリック →「主要項目を選択…」で必要な列にフラグを付けます。
列の編集画面の「主要項目」チェック欄からも指定できます。
右クリック →「主キー・外部キー・主要項目のみ」で表示を絞れます。
論理関係の接続列・参照先列も残します。★は主要項目、LFは論理関係です。

### 論理リレーション（外部キー制約のない意味的な関係）
参照元の表を選び「論理リレーションを追加…」を選択します。
参照先の表・列、対応する参照元列、名前を指定します。主キー・一意キーは不要です。
複合列も指定でき、紫の破線で表示します。DDLにはDB制約として出力しません。
「モデルを編集 → リレーションを編集／削除」から変更・削除できます。
「名前から論理リレーションを推薦…」は物理名・論理名・表名付き列名を照合します。
参照先候補はキー列／主要項目で、型の整合も確認します。裸のid等だけでは推薦しません。
候補は先頭200件まで未選択で表示します。意味を確認して必要なものを選び「適用」します。
推薦はDB内のデータを検証しません。関係の存在・件数はDBで保証されません。

### Markdownの設計資料を出力する
「ファイル → Markdown設計書をエクスポート…」で保存先を選びます。
表示対象テーブルのリンク付き一覧、Mermaid ER図、テーブルごとの設計書を1ファイルに出力します。
設計書には全列・キー・索引・関係を出力し、ER図はキー・主要項目・関係の接続列に絞ります。
主要項目と論理関係はERモデル（.yyer.yaml）にも保存できます。

### ヘルプの操作
F1または「ヘルプ → 使い方」でこの説明を開けます。
目次・見出しリンクから各節へ移動できます。必要な箇所を選択してCtrl+Cでコピーできます。EscまたはCtrl+Wでヘルプを閉じます。
WebView2が使えない環境では同じ本文を読み取り専用のテキストで表示します。
DBに接続していない状態や、ネットワークに接続していない状態でも読めます。

## 実行計画ファイルを持ち込む {#plan-files}

### 開き方と対応形式

DBへ接続せずに「実行計画 → 実行計画を開く…」から持ち込んだファイルを選択します。現在選択している接続先のDB種類には依存せず、内容から判別します。読み込みだけでSQLを実行することはありません。拡大縮小、ノードのプロパティ・原文表示、SVG出力、`.yyplan`での保存も使用できます。`.yyplan`はDB種類と推定／実測、取り込んだノードと原文を保持するので、共有や再保存に向いています。

| DB | 取り込める形式 | 主な情報・制限 |
|---|---|---|
| SQL Server | SSMSの`.sqlplan`／Showplan XML | 推定・実測、演算子、コスト、実測行数、XMLの統計 |
| PostgreSQL | EXPLAINのFORMAT JSON | 推定・実測、子計画、ループ数、BUFFERS等。TEXT／YAML／XMLは対象外 |
| MySQL | EXPLAIN FORMAT=JSON（v1／v2）、FORMAT=TREE／EXPLAIN ANALYZEのTREE | JSONの推定値、TREEの実測値。表罫線や列名を含むコンソール画面のコピーは対象外 |
| Oracle | 下記のPLAN_TABLEを出力した見出し付きCSV | 推定計画。所定の列を含むV$SQL_PLAN_STATISTICS_ALLのCSVなら実測値も参照可能 |
| Db2 LUW | 下記のDb2の8列DEL（`.db2plan`）、同じ列名の見出し付きCSV | EXPLAIN表から抽出した推定計画。db2exfmtのテキストやVisual Explain専用形式は対象外 |
| DuckDB | JSONプロファイル、演算子ツリーのJSON | プロファイルの演算子別行数・秒数。罫線付きのEXPLAIN表示は対象外 |
| SQLite | EXPLAIN QUERY PLANの見出し付きCSV、行オブジェクト配列のJSON | id／parent／detailで復元。ASCIIツリーやEXPLAINのバイトコードは対象外。実測時間はありません |
| 全DB共通 | yydbmanagerの`.yyplan` | 従来保存したファイルも読み込めます |

文字コードはUTF-8（BOM有無どちらも可）、またはBOM付きUTF-16 LE／BEです。ファイルは32MiB、ノードは1万件までです。CSVはカンマ区切り・引用符は二重引用符・見出しは大文字小文字を区別しません。改行入り・カンマ入りの項目はCSVの引用符で囲みます。NULLは空欄または`NULL`で表します。重複ノードID、存在しない親、循環、数値の不正はエラーとして扱います。空ファイルや未対応形式では対応形式の説明を確認し、取得し直してください。

コマンドラインからは`yydbmanager.exe --open-plan "C:\plans\mysql.txt"`で拡張子を問わず開けます。`.yyplan`／`.sqlplan`／`.json`／`.xml`／`.csv`／`.db2plan`／`.plan`は、単にファイルを引数で渡しても計画として開きます。`.txt`は通常のSQLテキストとも区別するため、メニューまたは`--open-plan`を使ってください。Windowsへの拡張子関連付けの自動登録は行いません。

以下の取得操作は**元DBに接続できる環境**で行います。持ち込み先のyydbmanagerには接続設定は不要です。例の接続先・ファイルパス・テーブル名・SQLは置き換えてください。ANALYZE／実測／プロファイルは実際に対象SQLを実行します。SQLを実行したくない場合は推定計画を使います。

### SQL Server：SSMSで保存

SSMSで推定計画（Ctrl+L）を表示、または「実際の実行プランを含める」（Ctrl+M）を有効にして対象SQLを実行します。計画の背景を右クリックし「実行プランに名前を付けて保存」で`.sqlplan`に保存します。画像や結果一覧のCSVではなく、Showplan XMLを持ち込みます。取得には対象SQLの参照権限等と、参照するDBのSHOWPLAN権限が必要です。実測統計のないXMLを実測値付きには変換しません。

公式：[XML形式での実行計画の保存](https://learn.microsoft.com/en-us/sql/relational-databases/performance/save-an-execution-plan-in-xml-format)。

### PostgreSQL：psqlのJSON出力

psqlで次を実行します。`\o`の出力先はpsqlを動かしているPCです。データを更新しないSELECTの例ですが、ANALYZEはそのSELECTを実行します。推定のみならオプションから`ANALYZE, BUFFERS,`を削除します。

```sql
\encoding UTF8
\pset format unaligned
\pset tuples_only on
\o postgresql-plan.json
EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
SELECT customer_id, COUNT(*) FROM orders GROUP BY customer_id;
\o
```

JSON本体だけを保存します。列名`QUERY PLAN`、行数フッター、端末のプロンプト、SQL文は含めません。通常のSQLを実行できる権限が必要です。追加拡張は不要です。

公式：[EXPLAINのFORMAT／ANALYZE](https://www.postgresql.org/docs/17/sql-explain.html)、[psqlの出力設定](https://www.postgresql.org/docs/17/app-psql.html)。

### MySQL：mysqlクライアントのJSON／TREE出力

PowerShellで、ヘッダーとエスケープを抑制して保存します。`-p`はパスワード入力用です。MySQL 8.4の例です。

```powershell
mysql --default-character-set=utf8mb4 --batch --raw --skip-column-names -h 127.0.0.1 -P 23306 -u yy_demo -p yydemo -e "EXPLAIN FORMAT=JSON SELECT customer_id, COUNT(*) FROM orders GROUP BY customer_id" | Set-Content -Encoding utf8 mysql-plan.json
mysql --default-character-set=utf8mb4 --batch --raw --skip-column-names -h 127.0.0.1 -P 23306 -u yy_demo -p yydemo -e "EXPLAIN ANALYZE SELECT customer_id, COUNT(*) FROM orders GROUP BY customer_id" | Set-Content -Encoding utf8 mysql-actual.txt
```

2行目は対象SQLを実行します。JSON v2は取得側MySQLの対応版で`explain_json_format_version`を設定した場合も取り込めます。TREEは各ノードが`->`で始まる原文を保存します。`\n`等が文字として並ぶ場合は`--raw`を確認してください。MySQLの通常の表形式EXPLAINやHTMLをJSONに拡張子だけ変更しても開けません。権限は対象SQLの実行に必要なものと、ビュー等へのSHOW VIEW権限を確認します。

公式：[MySQL EXPLAIN](https://dev.mysql.com/doc/refman/8.4/en/explain.html)、[mysqlのbatch／rawオプション](https://dev.mysql.com/doc/refman/8.4/en/mysql-command-options.html)。

### Oracle：SQL*PlusからPLAN_TABLEのCSVを保存

対象SQLのEXPLAINに必要な権限と、利用可能なPLAN_TABLEを準備します。SQL*Plus 12.2以降のCSV出力を使用します。日本語を含む場合はSQL*Plus起動前に`NLS_LANG=.AL32UTF8`を設定します。以下をUTF-8の`export-oracle.sql`として保存し、対象ユーザーで接続したSQL*Plusから`@export-oracle.sql`を実行します。SPOOLはSQL*Plusを動かしている環境のファイルへ書き込みます。

```sql
{{ORACLE_PLAN_EXPORT}}
```

このスクリプトは対象SELECTを実行せず、PLAN_TABLEの作業用STATEMENT_IDだけを削除・作成します。既存の作業と衝突しないSTATEMENT_IDを使います。例は専用接続で使用し、末尾でロールバックします。CSVにSQL文・メッセージ・行数フッターを混ぜないでください。必須列は`ID,PARENT_ID,OPERATION,CARDINALITY,COST`、任意列は`OPTIONS,OBJECT_OWNER,OBJECT_NAME`と述語等です。取り込んだ追加列もプロパティに保持します。

既に実測統計を収集したカーソルがある場合は、同じCSV設定で`V$SQL_PLAN_STATISTICS_ALL`をSQL_IDとCHILD_NUMBERで1計画に絞り、`ID,PARENT_ID,OPERATION,OPTIONS,OBJECT_OWNER,OBJECT_NAME,CARDINALITY,COST,LAST_STARTS,LAST_OUTPUT_ROWS,LAST_ELAPSED_TIME`等をSELECTして保存できます。該当するV_$ビューのSELECT権限が必要です。LAST_ELAPSED_TIMEはマイクロ秒からミリ秒へ変換して表示します。統計収集の設定は[Oracleの準備](#oracle)を参照してください。DBMS_XPLAN.DISPLAYの罫線付きテキストは直接取り込めません。

実測CSVは`LAST_OUTPUT_ROWS`と`LAST_STARTS`の両方が必要です。LAST_STARTSが全行0の場合は未収集としてエラーにします。計測のある計画内で実行されなかった枝の0行は保持します。

公式：[SQL*Plus SET MARKUP等](https://docs.oracle.com/en/database/oracle/oracle-database/19/sqpug/SET-system-variable-summary.html)、[PLAN_TABLE](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/PLAN_TABLE.html)、[V$SQL_PLAN_STATISTICS_ALL](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/V-SQL_PLAN_STATISTICS_ALL.html)。

### Db2 LUW：EXPLAIN表から8列DELを保存

Db2の8列DELは、下記のSELECTを`EXPORT ... OF DEL`で出力した**見出しなしの8列CSV**です。拡張子`.db2plan`を使います。Db2 for z/OSのファイルやdb2exfmtの帳票ではありません。対象ユーザーのEXPLAIN表・必要なEXPLAIN権限・表の参照権限を準備します。EXPLAIN表がなければ管理者と[Db2の準備](#db2)を確認してください。

下記を`export-db2.sql`として保存し、Db2 CLPで`db2 -stvf export-db2.sql`を実行します。DB名`YYDEMO`と対象SELECTを変更してください。出力ファイルはCLPを動かしている環境に作成します。

```sql
{{DB2_PLAN_EXPORT}}
```

8列の順序は`OPERATOR_ID,PARENT_ID,OPERATOR_TYPE,OBJECT_NAME,ESTIMATED_ROWS,TOTAL_COST,IO_COST,CPU_COST`です。同名の見出しを付けた通常の`.csv`も開けます。別の列順や一般のDb2データDELを`.db2plan`に変更しても対応しません。QUERYNOは作業と衝突しない値に変更し、複数ステートメントや異なるEXPLAIN時刻の行を混ぜないでください。例は最新の対象ステートメントを全キーで結合し、共有ノードに複数の親がある場合は最小TARGET_IDを表示上の親に選びます。そのため一般のDAGの全接続を再現する形式ではありません。実測情報を保持して共有する場合は、yydbmanagerで取得した`.yyplan`を利用してください。

公式：[Db2 EXPORT](https://www.ibm.com/docs/en/db2/11.5.x?topic=commands-export)、[EXPLAIN表](https://www.ibm.com/docs/en/db2/11.5.x?topic=information-explain-tables)。

### DuckDB：JSONプロファイルを保存

DuckDB CLI等で対象DBへ接続し、次を実行します。この例はSELECTを実行し、そのプロファイルをDuckDBを動かしている環境へ保存します。全結果を取得し終えてからファイルをコピーしてください。途中で次のSQLをプロファイルすると上書きされるため、対象SQLの直後に収集を停止します。普段の接続とは別の調査用接続で実行してください。

```sql
PRAGMA enable_profiling='json';
PRAGMA profiling_output='duckdb-plan.json';
SELECT customer_id, COUNT(*) FROM orders GROUP BY customer_id;
PRAGMA disable_profiling;
```

JSONの`children`／`operator_name`から演算子を復元します。CLIが表として表示したJSONのセルや、EXPLAINの罫線付き結果をそのまま保存したファイルは対象外です。演算子の時間は秒です。推定行数や実測行数がない場合は欠測として扱います。追加のサーバー権限設定は不要です。

公式：[DuckDBのプロファイリング](https://duckdb.org/docs/stable/dev/profiling)。

### SQLite：CLIでEXPLAIN QUERY PLANをCSV／JSON保存

sqlite3 CLIで対象ファイルを開き、次を実行します。`.explain off`で図式表示を抑止することが必要です。`.once`は次の1クエリだけをファイルへ出力します。CSVは`id,parent,notused,detail`の見出しを含めます。

```sql
.explain off
.headers on
.mode csv
.once sqlite-plan.csv
EXPLAIN QUERY PLAN SELECT order_id, ordered_on FROM orders
WHERE customer_id=1 ORDER BY ordered_on;
```

JSONで保存する場合は`.mode json`と`.once sqlite-plan.json`に変更します。`[{"id":3,"parent":0,"notused":0,"detail":"SCAN orders"}]`のような行配列を取り込みます。`parent=0`は根として扱います。これらはアクセス方法で、CPUや実測時間・行数を取得するものではありません。SQLiteの通常のファイル参照権限があれば使用できます。

公式：[SQLite CLIの出力形式と.once](https://www.sqlite.org/cli.html)、[EXPLAIN QUERY PLAN](https://www.sqlite.org/eqp.html)。

## 実行計画の取得と読み方 {#plans}

取得する
計画取得は1文を対象にします。複数のSQLがある場合は対象の1文を選択します。
「実行計画 → 推定実行計画を表示」は対象SQLを実行せず計画を取得します。
「実行して実測計画を取得」は対象SQLを実際に実行します。
UPDATEなどの更新SQLも実行されます。統計取得失敗時に自動再実行はしません。

対応する実測情報
SQL Server：実測行数、スレッド時間、警告など。
PostgreSQL：行数、ループ平均時間、ループ回数、BUFFERSなど。
MySQL 8：TREE形式の行数、ループ平均時間、ループ回数。
DuckDB：演算子時間と行数。
Oracle：最後の実行の行数、starts、経過時間、I/O、メモリなど。
Db2：section actualsの演算子ごとの行数。演算子時間は取得しません。
SQLite：QUERY PLANのアクセス方法を表示します。ノード実測値は取得しません。

図を読む
入力は右、結果は左です。矢印は右から左へ流れます。
線の太さは行数の目安です。未取得の実測値は0ではなく「—」と表示します。
アイコンを選択すると右側にプロパティ、マウスを置くと説明が出ます。
プロパティの値をダブルクリックすると全文を確認・コピーできます。
警告には、DBのディスク退避情報と、アプリが比較した行数推定の乖離があります。

数値の違い
SQL ServerのCost %は文内の推定演算子コスト比率で、時間比率ではありません。
各DBのコスト単位は異なります。Db2のtimeron・CPU命令数は時間ではありません。
PostgreSQL／MySQLの時間・行数は1ループ平均です。
Oracleの実測行数・時間は全startsの合計です。時間は子の処理を含みます。
並列処理や子を含む時間を単純に合算してクエリの経過時間としないでください。

移動・保存
Ctrl+ホイールでマウス位置を中心にズームできます。
通常ホイールは縦、Shift+ホイールは横移動です。背景をドラッグして移動できます。
右クリックメニューから100%、全体表示、検索などを行えます。
Ctrl+Fで検索、F3で次の候補へ移動します。右下の＋を押すとミニマップが開きます。
.yyplanを保存すると、後からDBに接続せず図と取得済み統計を確認できます。
SQL Serverの.sqlplanも読み込めます。SVG出力は「表示」メニューにあります。

## Oracleの権限と実測取得の準備 {#oracle}

### 接続する
OracleはRust直接接続を使用します。
右側のオブジェクトエクスプローラーの「新規接続…」で登録します。
DB種類をOracleにし、ドライバー、サーバー、ポート（通常1521）、
サービス名、ユーザー名、パスワードを各項目へ入力します。

上級者向けの「接続文字列を直接指定する」を使う場合の例：
```text
Driver={導入したドライバー名};DBQ=//ホスト:1521/サービス名;Uid=ユーザー;Pwd=パスワード;
```

### DB管理者に依頼する権限
実測取得に使う接続ユーザーには、対象SQLを実行する権限に加えて以下が必要です。
- ALTER SESSION
- SYS.V_$PARAMETERの参照
- SYS.V_$SESSIONの参照
- SYS.V_$SQL_PLAN_STATISTICS_ALLの参照

管理者向けの付与例（YYDB_USERを実際の接続ユーザー名へ置き換えます）：
```sql
GRANT ALTER SESSION TO YYDB_USER;
GRANT SELECT ON SYS.V_$PARAMETER TO YYDB_USER;
GRANT SELECT ON SYS.V_$SESSION TO YYDB_USER;
GRANT SELECT ON SYS.V_$SQL_PLAN_STATISTICS_ALL TO YYDB_USER;
```
この例は説明用です。ヘルプを開いてもSQLは実行されません。

### 実測取得
1. 接続後、対象のSQLを入力または1文を選択します。
2. 「実行計画 → 実行して実測計画を取得」を選びます。
3. 行数・時間・I/Oなどをアイコン下や右側のプロパティで確認します。

### 設定と数値
アプリは同じ接続のstatistics_levelを一時的にALLへ変更し、元へ戻します。
DB全体のALTER SYSTEMやAWR／SQL Monitorは使用しません。
PREV_SQL_IDとchild番号で実行カーソルを識別します。
実行を分離するため識別用のヒントコメントを追加します。
この識別子は無視され、既存ヒントの末尾へ置かれます。
実測用SQL_IDは、元のSQLをそのまま実行した場合のSQL_IDとは異なります。

LAST_OUTPUT_ROWSは最後の実行の全starts合計です。
LAST_ELAPSED_TIMEは子を含む経過時間で、画面ではmsに変換します。
未実行の分岐ではstartsと実測行数が0の場合があります。
復元失敗が表示された場合は接続を閉じ、管理者に状態を確認してください。

### 公式資料（URLをコピーして参照できます）
https://docs.oracle.com/en/database/oracle/oracle-database/26/refrn/V-SQL_PLAN_STATISTICS_ALL.html

## Db2の権限と実測収集の準備 {#db2}

### 接続と対応範囲
実測収集もRust直接接続を使用します。OUT引数はサーバー側のSQLブロックで受け取り、接続内の一時表を使って結果のキーを読み出します。
実測取得にはEXPLAIN表と、稼働中のYYDBMANAGER_ACTIVITYモニターが必要です。
USER TEMPORARY表スペースと、そのUSE権限も必要です。一時表は対象SQLの実行前に準備し、取得終了時に削除します。
行編成表のsection actualsのCARDINALITYを取得します。
列編成表など統計を提供しない計画では取得できない場合があります。
演算子ごとの実測時間は取得しません。timeronは推定コストです。

### DB管理者による準備
1. EXPLAIN表とYYDBMANAGER_ACTIVITYの既存状態を確認します。
2. 「実行計画 → Db2実測収集の準備SQL」を開きます。
3. 必要な文だけを選択して実行します。既存オブジェクトの再作成は不要です。
   SQLをタブへ開くだけでは実行しません。
   モニターを新規作成した場合はCOMMIT後に起動します。他の未確定変更を含めない専用接続で準備してください。


```sql
{{DB2_SETUP_SQL}}
```

### 権限と運用
接続ユーザーには、対象SQLの実行権限に加えて以下が必要です。
- EXPLAIN表の利用（EXPLAIN_FROM_ACTIVITYが書き込むスキーマでの必要権限）
- SYSCAT.EVENTTABLESとモニターの出力表の参照
- SYSPROC.WLM_GET_CONN_ENV、WLM_SET_CONN_ENV、EXPLAIN_FROM_ACTIVITYの実行
- YYDBMANAGER_ACTIVITYのFLUSHを行う権限
付与する権限・スキーマは管理者が環境の運用方針に合わせて確認してください。
モニター作成・起動と、蓄積した記録の保管・削除は管理者が行います。
既存ワークロードのactivity収集も、このモニターへ記録され得ます。

### 実測取得
対象の1文を選択して「実行して実測計画を取得」を選びます。
アプリは接続単位の収集設定を保存し、WITH DETAILS, SECTION／ALL／BASEを
一時的に設定してSQLを一度実行し、その後に元の設定へ戻します。
DB全体のSECTION_ACTUALS設定は変更しません。
モニターへの反映を最大10秒待ち、該当activityから計画を取得します。
図にはパーティションを合算した実測行数を表示します。

### 公式資料（URLをコピーして参照できます）
https://www.ibm.com/docs/en/db2/11.5.x?topic=wmr-wlm-get-conn-env-get-settings-activity-data-collection-connection
https://www.ibm.com/docs/en/db2/11.1?topic=tables-explain-actuals

## 実測取得のトラブルシューティング {#troubleshooting}

Oracleで権限エラーになる
「ヘルプ → Oracleの権限と実測取得の準備」を参照してください。
対象表の権限だけでは、カーソル統計の参照やALTER SESSIONができない場合があります。
DB管理者へ、表示されたエラーと必要なビュー・権限を伝えてください。

Db2でモニターがない／停止している
「ヘルプ → Db2の権限と実測収集の準備」を参照してください。
DB管理者がYYDBMANAGER_ACTIVITYの存在と稼働を確認します。
準備SQLは「実行計画 → Db2実測収集の準備SQL」で開けます。
アプリはモニターを自動作成・起動しません。

Db2でEXPLAIN表がない
接続ユーザーのCURRENT SCHEMAに必要なEXPLAIN表を管理者が準備します。
準備SQLのSYSINSTALLOBJECTSは、EXPLAIN表がない場合のみ実行します。
EXPLAIN表を更新・参照できることも確認してください。

Db2で10秒以内に記録されない／CARDINALITYがない
モニター出力表への記録・参照権限と、section収集の状態を管理者が確認します。
列編成表など、section actualsの提供範囲外の計画かどうかも確認します。
SQL自体は実行済みの場合があります。更新結果を確認してから再実行してください。

設定を復元できないと表示された
現在の接続を閉じ、DB管理者へエラーを伝えてください。
アプリは取消やSQL失敗の場合も設定復元を試みます。
通信断などで復元できなかった場合は、その事実をエラーで表示します。

SQLの実行後に統計取得だけ失敗した
自動再実行はしません。UPDATE／INSERT／DELETEなどの結果を先に確認してください。
計画取得のエラーは、SQLが実行されていないことを意味するとは限りません。

### 計画の値が「—」になる
未取得またはDBが提供していない統計です。実測0とは異なります。
推定計画では実測値はありません。SQLiteはノード単位の実測取得に未対応です。
Db2のtimeron、Oracleの推定TIME、PostgreSQLの推定コストを実測時間として扱わないでください。

## ERモデルYAMLファイル仕様と読み込み {#er-yaml}

### 保存・読み込み

1. **ファイル → ERモデルを保存…** で `.yyer.yaml` ファイルに保存します。
2. **ファイル → ERモデルを開く…** で保存したファイルを選ぶと、モデルと表示状態を復元します。DB接続は不要です。
3. `.yaml`・`.yml`・旧 `.yyer` も開けます。起動引数にファイルのパスを指定することもできます。
4. 旧 `.yyer` のJSON形式1～3は読み込んだ後、新しいYAMLに保存できます。旧形式にない主要項目・論理関係はfalse、サイズは自動、表示状態は既定値として扱います。

このファイルは**モデル全体**を保持します。ER図で非表示の表・列、表示を絞ったときの非主要項目も削除しません。
物理名・論理名・DB種類・型・NULL許可・既定値・キーの列順・索引・複合列の対応・論理関係・主要項目・逆生成の由来・配置・サイズ・表示対象・表示水準・倍率・表示位置・選択中の表を保存します。

SQLタブ、結果データ、DBの接続情報、Undo／Redoの操作履歴はER文書の構成情報に含みません。
YAMLのコメント・空白・記述順は再保存時に整形されます。コメントをテーブルの定義情報として利用しないでください。

### ファイル形式

UTF-8のYAML 1.2、1ファイル1文書です。拡張子より内容を判定します。未知の項目、重複キー、未対応バージョン、不正な列参照、非有限の座標・サイズはエラーとし、読み込み前の画面を変更しません。
アンカー・エイリアス・マージキー・外部ファイルの取り込み・環境変数展開は使用しません。
最大32MiB、入れ子32段、YAML解析イベント100万までです。表は500、表の列は4096、関係は10000までです。
上限を超える場合は保存もエラーにし、既存ファイルを置き換えません。

| ルート項目 | 型 | 意味 |
| --- | --- | --- |
| `format` | 文字列 | 固定値 `yydbmanager-er` |
| `format_version` | 整数 | このYAMLコンテナー仕様では `1` |
| `model` | マッピング | 下記のモデル定義。内部モデル形式は `version: 4` |
| `view` | マッピングまたは `null` | ER図の表示状態。`null` は既定表示 |

`model` の各項目：

| 項目 | 型・値 | 意味 |
| --- | --- | --- |
| `version` | 整数 `4` | モデル形式。YAMLは4で保存。旧JSONの1～3を移行可能 |
| `kind` | 列挙 | `Sqlite`, `DuckDb`, `Oracle`, `SqlServer`, `PostgreSql`, `MySql`, `Db2` |
| `names` | 列挙 | `Physical`（物理名）, `Logical`（論理名）, `Both`（両方） |
| `level` | 列挙 | `Entity`（表名）, `Keys`（主キー）, `Attributes`（列）, `Types`（列と型）, `Important`（主キー・外部キー・主要項目・関係の接続列） |
| `entities` | 配列 | 表・ビュー。配列順を保持 |
| `relations` | 配列 | 外部キー制約と論理リレーション。配列順を保持 |

`entities[]` の各項目：

| 項目 | 型 | 意味 |
| --- | --- | --- |
| `id` | `{schema: 文字列, name: 文字列}` | スキーマと物理表名。スキーマを指定しない場合は空文字 |
| `logical_name` | 文字列 | 論理表名。未設定は空文字 |
| `view` | 真偽値 | DBのビューならtrue |
| `fields` | 配列 | 下記の全列定義。配列順を保持 |
| `keys` | 配列 | `{name: キー名, primary: 真偽値, columns: [列名...]}`。primary=falseは一意キー |
| `indexes` | 配列 | `{name: 索引名, unique: 真偽値, columns: [列名...]}` |
| `position` | `{x: 数値, y: 数値}` | 図の配置。負の座標も可。有限値かつ絶対値10000000以下 |
| `size` | `{width: 数値, height: 数値}` または `null` | 手動サイズ、またはコンテンツに追従する自動サイズ。省略も自動 |
| `visible` | 真偽値 | 図に表示する表か。falseでも定義を保存 |
| `catalog_definition` | 真偽値 | DBから逆生成した定義か。DDLの注意書きに使用 |

`fields[]` の各項目：

| 項目 | 型 | 意味 |
| --- | --- | --- |
| `name` | 文字列 | 物理列名 |
| `logical_name` | 文字列 | 論理列名。未設定は空文字 |
| `data_type` | 文字列 | 桁数・精度等を含む型の定義 |
| `nullable` | 真偽値 | NULL許可 |
| `default_sql` | 文字列または `null` | SQLの既定値式。`null`は未設定、文字列 `'NULL'`等の式とは区別 |
| `important` | 真偽値 | 人が指定した主要項目フラグ |

`keys[].columns` と `indexes[].columns` は構成列の順番を表します。主キーは表ごとに1つまでです。
`relations[]` は次の項目を持ちます。

| 項目 | 型 | 意味 |
| --- | --- | --- |
| `name` | 文字列 | 制約名または論理関係名 |
| `logical` | 真偽値 | false=外部キー制約、true=論理関係。trueはDDLに制約として出力しない |
| `child` | `{schema: 文字列, name: 文字列}` | 参照元の表 |
| `parent` | 同上 | 参照先の表 |
| `columns` | 文字列配列 | 参照元列。空配列・重複不可 |
| `referenced_columns` | 文字列配列 | 同じ順序で対応する参照先列。参照元と同じ要素数 |

論理関係はモデル内の任意の列を参照でき、参照先に主キー・一意キーは不要です。外部キー制約の参照先が権限等によりモデル外の場合も、表名・列名を保持します。

`view` の各項目：

| 項目 | 型 | 意味 |
| --- | --- | --- |
| `zoom` | 数値 | 96DPI基準の倍率。0より大きく4以下。読み込み先の画面DPIを適用 |
| `origin` | `{x: 数値, y: 数値}` | 表示領域の左上の図座標 |
| `selected` | 表の`id`、または `null` | 選択中の表。指定する表はモデル内に存在すること |

未設定の文字列は `""`、未設定値は `null`、真偽値は `true`／`false` として区別します。
先頭ゼロ、数値のような名前、コロンや改行、SQLの引用符を含む文字列も保持します。手で編集する場合は引用符やYAMLの `|` ブロックスカラーを使ってください。

### 完全な記述例

```yaml
format: yydbmanager-er
format_version: 1
model:
  version: 4
  kind: PostgreSql
  names: Both
  level: Important
  entities:
    - id: {schema: public, name: department}
      logical_name: 部門
      view: false
      position: {x: 40.0, y: 32.0}
      size: {width: 420.0, height: 180.0}
      visible: true
      catalog_definition: false
      fields:
        - {name: id, logical_name: 部門ID, data_type: INTEGER, nullable: false, default_sql: null, important: false}
        - {name: tenant_id, logical_name: テナントID, data_type: VARCHAR(16), nullable: true, default_sql: null, important: true}
      keys:
        - {name: PK_department, primary: true, columns: [id]}
      indexes: []
    - id: {schema: public, name: employee}
      logical_name: 従業員
      view: false
      position: {x: 600.0, y: 32.0}
      size: null
      visible: true
      catalog_definition: false
      fields:
        - {name: id, logical_name: 従業員ID, data_type: INTEGER, nullable: false, default_sql: null, important: false}
        - {name: department_id, logical_name: 部門ID, data_type: INTEGER, nullable: true, default_sql: null, important: false}
        - {name: tenant_id, logical_name: テナントID, data_type: VARCHAR(16), nullable: true, default_sql: null, important: false}
        - {name: created_at, logical_name: 作成日時, data_type: TIMESTAMP, nullable: false, default_sql: CURRENT_TIMESTAMP, important: false}
      keys:
        - {name: PK_employee, primary: true, columns: [id]}
      indexes:
        - {name: IX_employee_department, unique: false, columns: [department_id, tenant_id]}
  relations:
    - name: FK_employee_department
      logical: false
      child: {schema: public, name: employee}
      parent: {schema: public, name: department}
      columns: [department_id]
      referenced_columns: [id]
    - name: LR_employee_tenant
      logical: true
      child: {schema: public, name: employee}
      parent: {schema: public, name: department}
      columns: [tenant_id]
      referenced_columns: [tenant_id]
view:
  zoom: 1.0
  origin: {x: 0.0, y: 0.0}
  selected: {schema: public, name: employee}
```

### 箱のサイズを変える

- 表を選択すると右下の角にハンドルが現れます。ドラッグで幅と高さを変えられます。操作中は枠でサイズを確認し、離すと内容と接続線を再配置します。Escでキャンセルできます。
- **右クリック → テーブルのサイズを指定…** から数値でも指定できます。幅160～10000、高さ48～120000の図座標です。
- **コンテンツにサイズを合わせる** は選択した表を自動サイズに戻します。表示する名前・列・型の長さと列数に合わせ、文字の大きさを変えずに箱の幅と高さを調整します。幅の上限は10000です。
- **すべての表をコンテンツに合わせる** でモデル全体を自動サイズにできます。非表示の表にも適用します。
- 自動サイズの表は列の追加・表示水準の切替にも追従します。手動サイズを小さくした場合は表示が切れますが、列情報は失われません。
- サイズ変更も元に戻す／やり直しに対応します。`size: null` と手動の幅・高さをYAMLに保存します。



## ER図の列定義を表形式でまとめて編集する {#er-columns}

ER図のテーブルを選択し、右クリックの「列定義をまとめて編集…」、または「表示」→「ER図」→「列定義をまとめて編集…」を開きます。接続せずに新規作成したERモデルでも使用できます。テーブル追加時の `id` 列もこの画面で変更できます。

yysheetと共通のグリッドで、1行を1つの列定義として編集します。項目は左から「物理名」「論理名」「データ型」「NULL許可」「既定値(SQL)」「主要項目」です。Excelでもこの順序で表を作り、見出しを除くセルをコピーして貼り付けると、複数の列を一括作成できます。貼り付け位置は選択中のセルです。途中の項目だけを矩形で貼り付けることもできます。末尾の空行への入力・貼り付けで行が増えます（最大4096行）。

| 操作 | 方法 |
| --- | --- |
| セル編集 | ダブルクリック、F2、または文字を直接入力。日本語入力にも対応 |
| 移動・編集確定 | 矢印キー、Tabで次の項目、Enterで次の行。Shift+Tab / Shift+Enterは逆方向 |
| 範囲選択 | マウスでドラッグ、Shift+クリック、Shift+矢印。行番号・列見出しで行・列を選択、Ctrl+Aで全体選択 |
| コピー・貼り付け | Ctrl+C / Ctrl+V、または上部のボタン。Excelと相互に利用可能 |
| 切り取り・クリア | Ctrl+X / Delete。Deleteは値だけを消します。列定義そのものを消すには「行を削除」 |
| 行の追加・順序変更 | 「行を挿入」「行を削除」「上へ移動」「下へ移動」。選択した複数行もまとめて操作可能 |
| 真偽値 | NULL許可・主要項目に `true` / `false` または `1` / `0` を入力。セル選択中のSpaceでも切り替え |
| 元に戻す | Ctrl+Z / Ctrl+Y、または上部のボタン（画面内で最大32操作） |
| スクロール | スクロールバー、ホイール。Shift+ホイールで横方向 |
| 編集取り消し | セル編集中のEscでその入力を取り消し。キャンセルで画面全体の変更を破棄 |

データ型セルをF2またはダブルクリックで編集すると、モデルのDB種類に応じた型をプルダウンで選べます。F4またはAlt+↓でも候補を開けます。SQLite・DuckDB・Oracle・SQL Server・PostgreSQL・MySQL・Db2ごとに組み込み型の候補を切り替え、モデルで既に使われている型も候補に含めます。接続せずに使用できます。個別の「列を編集」でも同じ候補を選べます。

プルダウンは直接入力も可能です。`VARCHAR(255)`や`DECIMAL(18,2)`などの桁数は例なので、用途に応じて書き換えてください。ユーザー定義型やバージョン固有の型も直接入力できます。型候補に入力を制限しないため、型のセルだけの貼り付けや、列定義全体のExcelとのコピー・貼り付けも引き続き利用できます。候補はDB種類別の組み込み一覧であり、接続先の全ユーザー定義型を自動取得するものではありません。

型候補の参照元: [SQLite](https://www.sqlite.org/datatype3.html)、[DuckDB](https://duckdb.org/docs/current/sql/data_types/overview)、[Oracle](https://docs.oracle.com/en/database/oracle/oracle-database/19/sqlrf/Data-Types.html)、[SQL Server](https://learn.microsoft.com/en-us/sql/t-sql/data-types/data-types-transact-sql)、[PostgreSQL](https://www.postgresql.org/docs/current/datatype.html)、[MySQL](https://dev.mysql.com/doc/refman/8.4/en/data-types.html)、[IBM Db2](https://www.ibm.com/docs/en/db2/12.1.0?topic=list-numbers)。

物理名とデータ型は必須です。既定値はSQL式をそのまま入力し、文字列リテラルは `'text'` のように引用符を含めます。空欄は既定値なしです。Excelとのコピーにはタブ区切り形式を使い、値にタブ・改行・引用符が含まれる場合も保持します。

「適用」でテーブル全体を一括検証してERモデルに反映します。列名変更や並べ替えでも、元の列の主キー・一意キー・索引・物理/論理リレーションの対応を維持します。列名の入れ替えにも対応します。キー・索引・リレーションで使用中の列は削除できず、主キーのNULL許可も変更できません。該当する制約の編集は別途「モデルを編集」の各メニューから行ってください。選択中の行のキー情報は画面下部に表示します。

入力エラー時は編集画面に戻るので、そのまま修正できます。適用はER図側の「元に戻す」1回で取り消せます。接続先データベースへのDDLの実行は行いません。変更した定義はERモデルYAML、Markdown設計書、DDL生成にも反映されます。

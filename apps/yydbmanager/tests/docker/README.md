# 外部DBの実接続検証

このフォルダーはyydbmanager専用の一時的な検証環境です。既存のDB・コンテナーには接続せず、`yydbmanager-validation`プロジェクト内だけでテーブルを作成します。接続設定のパスワードはこの使い捨て環境用の固定値です。

```powershell
docker compose -p yydbmanager-validation -f apps/yydbmanager/tests/docker/compose.yml up -d
docker build -t yydbmanager-validation-runner -f apps/yydbmanager/tests/docker/runner.Dockerfile .
./apps/yydbmanager/tests/docker/prepare-db2.ps1 -OutputDirectory target/yydbmanager-docker/profile
# ランナーが使う専用ボリュームへ、検証Db2のインスタンス設定を渡します。
docker volume create yydbmanager-validation-build
docker run --rm -v "${PWD}/target/yydbmanager-docker/profile:/profile:ro" `
  -v yydbmanager-validation-build:/build rust:1.99.0-bookworm `
  cp /profile/db2-instance.tgz /build/db2-instance.tgz
docker run --rm --network yydbmanager-validation_default --name yydbmanager-validation-runner `
  -v "${PWD}:/workspace" -v yydbmanager-validation-build:/build `
  -v yydbmanager-validation-registry:/usr/local/cargo/registry `
  -e YYDB_CONNECTIONS=/workspace/apps/yydbmanager/tests/docker/connections.json `
  -e YYDB_REPORT_DIR=/build/reports -e CARGO_BUILD_JOBS=4 -e CXXFLAGS=-g0 `
  yydbmanager-validation-runner `
  cargo test --manifest-path apps/yydbmanager/Cargo.toml -p yy-db-core --test docker_live --locked -- --ignored --nocapture
```

Oracle Free、Db2 Community、SQL Server Developerの利用条件に従う開発・検証用途です。Oracle／Db2の初期化には時間がかかります。Db2は公式の起動手順に従い専用コンテナーにprivilegedを付けています。ホストディレクトリやDockerソケットをDBへマウントしません。

ホストへの公開ポートは127.0.0.1に限定しています。検証ランナーにはPostgreSQL ODBC、MariaDB ODBC（MySQLへ接続）、Microsoft ODBC、Oracle Instant Client ODBC、IBM Db2公式コンテナーの64bit ODBC互換ライブラリーを導入します。Windowsホストのドライバー登録は変更しません。

Db2の`libdb2.so`はunixODBCの64bit SQLLENとABIが異なるため、`libdb2o.so`を利用します。このライブラリーの初期化に必要なインスタンス設定は`prepare-db2.ps1`で検証コンテナーから取得し、ランナー起動時にホスト名を合わせます。通常のアプリのDb2接続では、利用者が導入したWindows用64bit ODBCドライバーを使います。検証用イメージ・設定アーカイブを製品に同梱しません。

試験は接続・DDL/DML・DECIMAL(31,8)の精度・日本語・NULL・メタデータ・型付きバインド投入・CSV／Parquet出力を検証します。5DBすべてで推定計画、PostgreSQL／SQL Serverでは実測計画も検証します。Db2のEXPLAIN表は試験側が検証DBに明示的に作成し、通常のアプリ操作は自動作成しません。

`YYDB_EXPECT_DATABASES`は既定5です。DBごとのジョブでは、設定ファイルを1DB分にし、同変数を1に設定します。接続情報の絶対パスを指定してください。結果JSONは`/build/reports/remote-databases.json`です。

終了後は、この検証環境だけを停止・削除できます。

```powershell
docker compose -p yydbmanager-validation -f apps/yydbmanager/tests/docker/compose.yml down -v
```

Dockerイメージはタグに加えて、実行時のdigestとサーバー版を結果レポートに記録します。WindowsのSQL Serverドライバー経由の試験と、Linux検証ランナーの試験結果は区別して扱います。

実行済みの結果は[2026-10-20の検証記録](results/2026-10-20.md)を参照してください。DBイメージの取得・初回ビルド・初期化時間と、テスト本体の実行時間は別です。

ERの複合主キー・複合外部キーの列順、一意制約、NOT NULLも検証します。各DBの`.yyer`と画像をランナーの結果フォルダーへ出力します。[2026-10-21のER検証](results/2026-10-21-er.json)は5DBすべてで成功しました。

生成DDLを専用DBへ適用し、表・FK・索引と既定値、FK違反が拒否されることも確認します。整数・VARCHAR・複合FKの設計を対象とし、`*-design.sql`を出力します。[2026-10-21のDDL検証](results/2026-10-21-design.json)は外部5DBで成功しました。

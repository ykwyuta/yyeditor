# 外部DBの実接続検証

このフォルダーはyydbmanager専用の一時的な検証環境です。既存のDB・コンテナーには接続せず、`yydbmanager-validation`プロジェクト内だけでテーブルを作成します。接続設定のパスワードはこの使い捨て環境用の固定値です。

```powershell
docker compose -p yydbmanager-validation -f apps/yydbmanager/tests/docker/compose.yml up -d
docker build -t yydbmanager-validation-runner -f apps/yydbmanager/tests/docker/runner.Dockerfile .
docker volume create yydbmanager-validation-build
docker run --rm --network yydbmanager-validation_default --name yydbmanager-validation-runner `
  -v "${PWD}:/workspace" -v yydbmanager-validation-build:/build `
  -v yydbmanager-validation-registry:/usr/local/cargo/registry `
  -e YYDB_CONNECTIONS=/workspace/apps/yydbmanager/tests/docker/connections.json `
  -e YYDB_REPORT_DIR=/build/reports -e CARGO_BUILD_JOBS=4 -e CXXFLAGS=-g0 `
  yydbmanager-validation-runner `
  cargo test --manifest-path apps/yydbmanager/Cargo.toml -p yy-db-core --test docker_live --locked -- --ignored --nocapture
```

Oracle Free、Db2 Community、SQL Server Developerの利用条件に従う開発・検証用途です。Oracle／Db2の初期化には時間がかかります。Db2は公式の起動手順に従い専用コンテナーにprivilegedを付けています。ホストディレクトリやDockerソケットをDBへマウントしません。

ホストへの公開ポートは127.0.0.1に限定しています。検証ランナーは製品と同じRust直接接続を使います。各DBの追加クライアント・ドライバー・Db2インスタンス設定は不要です。

試験は接続・DDL/DML・DECIMAL(31,8)の精度・日本語・NULL・メタデータ・型付きバインド投入・CSV／Parquet出力を検証します。5DBすべてで推定計画、PostgreSQL／SQL Serverでは実測計画も検証します。Db2のEXPLAIN表は試験側が検証DBに明示的に作成し、通常のアプリ操作は自動作成しません。

`YYDB_EXPECT_DATABASES`は既定5です。DBごとのジョブでは、設定ファイルを1DB分にし、同変数を1に設定します。接続情報の絶対パスを指定してください。結果JSONは`/build/reports/remote-databases.json`です。

Db2のTLS・バイナリNULLを含むバインド投入・実行中取消は、専用Db2にテスト証明書とTLSリスナーを設定し、`YYDB_DB2_CONFIG`に`yy_db_core::native::Config`形式のJSONファイルを指定して検証します。`tls`は`require`、`ca_file`はテストCAのPEMファイル、`port`はTLSポートです。実行コマンドは`cargo test --manifest-path apps/yydbmanager/Cargo.toml -p yy-db-core --test db2_transport --locked -- --ignored --nocapture`です。サーバーの`MON_GET_APPL_LOCKWAIT`で実際のロック待機を確認してから取消し、証明書の不信頼時に接続が拒否されることも確認します。

終了後は、この検証環境だけを停止・削除できます。

```powershell
docker compose -p yydbmanager-validation -f apps/yydbmanager/tests/docker/compose.yml down -v
```

Dockerイメージはタグに加えて、実行時のdigestとサーバー版を結果レポートに記録します。WindowsとLinuxの試験結果は区別して扱います。過去の検証記録は当時の接続経路の結果であり、現行ドライバーの検証結果ではありません。

実行済みの結果は[2026-10-20の検証記録](../../../../docs/history/verification/yydbmanager-docker-2026-10-20.md)を参照してください。DBイメージの取得・初回ビルド・初期化時間と、テスト本体の実行時間は別です。

ERの複合主キー・複合外部キーの列順、一意制約、NOT NULLも検証します。各DBの`.yyer`と画像をランナーの結果フォルダーへ出力します。[2026-10-21のER検証](results/2026-10-21-er.json)は5DBすべてで成功しました。

生成DDLを専用DBへ適用し、表・FK・索引と既定値、FK違反が拒否されることも確認します。整数・VARCHAR・複合FKの設計を対象とし、`*-design.sql`を出力します。[2026-10-21のDDL検証](results/2026-10-21-design.json)は外部5DBで成功しました。

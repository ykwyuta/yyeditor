# yydbmanager のビルド・検証

コマンドはリポジトリルートから実行します。

## ParquetのZstdバックエンド

Parquetはローカル修正版の56.2.1を使い、`arrow,snap,zstd-rust`を有効にします。変更範囲は[依存元の記録](../../apps/yydbmanager/vendor/parquet/YYEDITOR-PATCH.md)を参照。`yy-parquet-zstd`が`zstandard = 0.1.9`へ圧縮レベル1〜22をそのまま渡します。レベルの読み替えや上限の引き下げは行いません。ParquetのZstdLevel APIが元から受け付けない0・負数・23以上はエラーです。

この依存にはRust 1.96以上が必要です。通常のアプリ依存から`zstd`・`zstd-safe`・`zstd-sys`は外れますが、DuckDB本体に含まれるC++と圧縮処理、SQLite、TLS等のネイティブ実装は別に残ります。既定のParquet書き出し設定と画面の操作は変更していません。

展開時はParquetページの展開後サイズを上限として使い、実際の結果とも一致することを確認します。チェックサム検証と128 MiBの既定ウィンドウ上限を維持し、連結フレーム・skippable frameも処理します。全体の検証が成功してから呼び出し元の出力へ追記するため、失敗時に途中のデータを残しません。

通常のワークスペース試験には、libzstd 1.5.7から作った22レベルの固定フレーム・Parquetファイルと、型・NULL・辞書エンコーディング・小数精度・バイナリ・壊れた入力・ブロック境界の回帰試験を含みます。ネットワークやDuckDBの拡張取得は使いません。

参照実装を使った再生成・相互検証と簡易測定は、製品とは別のCargoワークスペースで実行します。再生成は既存の試験用ファイルを上書きします。

```sh
cargo run --manifest-path apps/yydbmanager/tests/zstd-reference/Cargo.toml --release --locked
cargo run --manifest-path apps/yydbmanager/tests/zstd-reference/Cargo.toml --release --locked -- --benchmark
```

この参照ツールだけはC版libzstdを比較用にビルドします。アプリの開発依存にも入りません。修正版とレジストリー版のParquetを別ソースとして同時に参照し、全22レベル・両WriterVersionで相互に読み込みます。測定結果は`target/zstd-benchmark.json`、相互検証結果は`target/zstd-compatibility.json`へ出力します。

## 通常のビルドと検証

```powershell
cargo build --manifest-path apps/yydbmanager/Cargo.toml --release -p yydbmanager --locked
./apps/yydbmanager/target/release/yydbmanager.exe
```

```powershell
cargo test --manifest-path apps/yydbmanager/Cargo.toml --workspace --locked
cargo clippy --manifest-path apps/yydbmanager/Cargo.toml --workspace --all-targets --locked -- -D warnings
./apps/yydbmanager/target/release/yydbmanager.exe --smoke-test target/yydbmanager-smoke
./apps/yydbmanager/target/release/yydbmanager.exe --ui-smoke-test target/yydbmanager-ui-smoke
./apps/yydbmanager/tests/windows-shell.ps1 -Executable ./apps/yydbmanager/target/release/yydbmanager.exe
```

## 起動とビルド

リポジトリ直下から実行します。既存yyシリーズとは別のCargoワークスペース・lockfileです。

WindowsのビルドにはVisual Studio 2022のC++ビルドツールを使用します。固定しているDuckDB 1.4.1の同梱fmtは、Visual Studio 2026で削除された`stdext::checked_array_iterator`を使用するため、共通CI内のDBジョブも`windows-2022`を指定しています。DuckDB側の[修正](https://github.com/duckdb/duckdb/pull/23261)を含む版への更新と検証後に、新しいツールチェーンへ移行します。

SQLiteとDuckDBは静的リンクしており、別途DBサーバー・CLIを導入せず使えます。

外部DBはRust製ドライバーを使用し、ODBC依存・機能フラグ・接続経路は廃止しました。Db2は `rust-db2-driver` を完全なコミットSHAで固定したGit依存です。TLSはドライバーのrustlsを使い、公開CAと `ca_file` の追加CAを検証します。取消は `cancel_handle()` でソケットを閉じるため再接続が必要です。Db2インポートは別接続で `execute_params` を使い、成功時にCOMMIT、失敗時にROLLBACKします。バイナリ列のNULLはドライバーの未型付けVARCHAR NULLでは変換できないため、SQL NULLを使って格納先に型を決定させます。実測計画のOUT引数はサーバー側のSQLブロックで受け、接続内一時表から取得した完全なステートメントキーで計画を検索します。一時表はSQL実行前に準備し、成否にかかわらず削除します。USER TEMPORARY表スペースとUSE権限が必要です。最新時刻だけの検索で代替しません。旧接続文字列は接続前に拒否し、設定の再入力を案内します。
同梱フォントは他のyyシリーズと同じUDEV Gothicです。
アプリアイコンも共通のYYデザインを使い、青緑の背景とデータベースの記号で識別できます。

## 検証

`--smoke-test`は同梱エンジン、SQL、精度、Parquet、計画を試験します。
`--ui-smoke-test`は非表示のネイティブウィンドウを起動し、接続・SQLタブ・結果・計画・ER・ツリー・列展開・3サイズでの再配置／再描画を検証します。`windows-shell.ps1`はメニュー、接続の保存／削除ダイアログ、暗号化設定の再読込、最大化／復元を操作して画像と結果を保存します。接続設定は試験用フォルダーに分離します。GUI試験はWindows専用です。

`YYDBMANAGER_PLAN_FIXTURE`へ`tests/fixtures/sqlserver-hash-sort.sqlplan`の絶対パスを設定すると、UI試験で複合計画のズーム・選択・プロパティ・ツールチップ・パン・ミニマップ・検索も確認します。シェル試験は`-PlanFile`で同じファイルを指定できます。共通CIにも組み込んでいます。

通常のUI試験ではERの選択・関連線を伴うドラッグ・保存再読込・表示切替・整列も検証し、`edited-er.yyer.yaml`を出力します。シェル試験へ`-PlanFile target/yydbmanager-ui-smoke/edited-er.yyer.yaml`を渡すと、表選択と論理名編集の実ダイアログも検証します。500表・2,000関連の描画測定は明示的に`cargo test --manifest-path apps/yydbmanager/Cargo.toml --release -p yy-db-core --lib er_viewport_performance -- --ignored --nocapture`で実行します。

外部DBの実接続試験は[Docker検証](../../apps/yydbmanager/tests/docker/README.md)を参照してください。通常テストではこの試験を明示的にignoreとし、未実行を接続成功と扱いません。


## データを再生成する（開発者向け）

通常利用では不要です。テキストSQL・CSV等はPython標準ライブラリーだけで再生成できます。

```powershell
# リポジトリーのルートで実行
python demo/yydbmanager/tools/generate.py
cargo run --manifest-path apps/yydbmanager/Cargo.toml -p yy-db-core --example prepare_demo -- demo/yydbmanager demo/yydbmanager/generated
```

後者はアプリと同じSQLite／DuckDB・入出力ライブラリーを使い、件数、精度、計画、Parquet、固定長、ER YAMLを検証してバイナリを生成します。既存ファイルは上書きしません。`generated/`の出力を確認してから`data/`の配布サンプルを更新してください。JSONの期待件数は`data/expected-counts.json`です。

`verify.ps1 -Services ...`は起動中の指定DBの全テーブル件数、共通サンプルSQLとDB別の性能分析SQLを読み取り検証し、使用イメージID・件数・実行ファイル一覧を`reports/verification.json`へ記録します。Oracleは日本語の投入結果も検証します。プロファイルを指定しただけで未起動のDBへ接続することはありません。Db2 CLPの「0行取得」による終了コード1は成功として扱い、SQLエラーは失敗にします。

検証ジョブは[共通CI](../../.github/workflows/ci.yml)に統合しています。Cargoワークスペースとlockfileは独立したままです。

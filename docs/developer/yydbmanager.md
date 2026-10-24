# yydbmanager のビルド・検証

コマンドはリポジトリルートから実行します。

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

WindowsのビルドにはVisual Studio 2022のC++ビルドツールを使用します。固定しているDuckDB 1.4.1の同梱fmtは、Visual Studio 2026で削除された`stdext::checked_array_iterator`を使用するため、専用CIも`windows-2022`を指定しています。DuckDB側の[修正](https://github.com/duckdb/duckdb/pull/23261)を含む版への更新と検証後に、新しいツールチェーンへ移行します。

SQLiteとDuckDBは静的リンクしており、別途DBサーバー・CLIを導入せず使えます。
同梱フォントは他のyyシリーズと同じUDEV Gothicです。
アプリアイコンも共通のYYデザインを使い、青緑の背景とデータベースの記号で識別できます。

## 検証

`--smoke-test`は同梱エンジン、SQL、精度、Parquet、計画を試験します。
`--ui-smoke-test`は非表示のネイティブウィンドウを起動し、接続・SQLタブ・結果・計画・ER・ツリー・列展開・3サイズでの再配置／再描画を検証します。`windows-shell.ps1`はメニュー、接続の保存／削除ダイアログ、暗号化設定の再読込、最大化／復元を操作して画像と結果を保存します。接続設定は試験用フォルダーに分離します。GUI試験はWindows専用です。

`YYDBMANAGER_PLAN_FIXTURE`へ`tests/fixtures/sqlserver-hash-sort.sqlplan`の絶対パスを設定すると、UI試験で複合計画のズーム・選択・プロパティ・ツールチップ・パン・ミニマップ・検索も確認します。シェル試験は`-PlanFile`で同じファイルを指定できます。専用CIにも組み込んでいます。

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

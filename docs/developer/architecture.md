# 現行の内部構成

## リポジトリとワークスペース

| 単位 | 定義 | 責務 |
| --- | --- | --- |
| ルート | [Cargo.toml](../../Cargo.toml) | yyeditor、yyterm、yysftp、yysheet、yyclip、yyfilemanager、yybrowser、yydraw、yy-agentと共通クレート |
| yydbmanager | [独立Cargo.toml](../../apps/yydbmanager/Cargo.toml) | DBドライバー・結果・実行計画・ERモデルとWindows UI |
| yymerge | [独立Cargo.toml](../../apps/yymerge/Cargo.toml) | 比較・マージ・同期・各形式のモデル、CLI、Windows UI |
| YY3Designer | [独立Cargo.toml](../../apps/yy3designer/Cargo.toml) | ドット絵・画像変換・3D・保存のコアとWindows UI |
| YYLibrary | [外部リポジトリ](https://github.com/ykwyuta/YYLibrary) | yy-encodingと対応表生成。本体の複製を置かずGitコミットを固定 |
| YYSort | [外部リポジトリ](https://github.com/ykwyuta/YYSort) | 固定長/CSVのCLI外部ソート。YYLibraryを共用 |

独立ワークスペースは個別のCargo.lockを持ちます。yydbmanagerとyymergeの検証・配布は共通CIに統合し、YY3Designerは専用CIを維持します。共有クレートを利用するアプリは複数の
ワークスペースにまたがるため、ルートの検査だけで全アプリの検証済みとは扱いません。

## モジュール境界

- [yy-buffer](../../crates/yy-buffer/src/lib.rs)、[yy-core](../../crates/yy-core/src/lib.rs)、[yy-io](../../crates/yy-io/src/lib.rs): バッファ・編集モデル・ファイル読み書き。文字コードはYYLibraryを利用。
- [yy-win](../../crates/yy-win/src/lib.rs)、[yy-ui-win](../../crates/yy-ui-win/src/lib.rs): Windows画面、描画・フォント・共通ヘルプ。プラットフォーム非依存の処理はコアクレートに置く。
- [yy-sheet](../../crates/yy-sheet/src/lib.rs)、[yy-formula](../../crates/yy-formula/src/lib.rs)、[yy-cobol](../../crates/yy-cobol/src/lib.rs): 列データ・数式・固定長レイアウト。
- [yy-ssh](../../crates/yy-ssh/src/lib.rs)、[yy-remote](../../crates/yy-remote/src/lib.rs)、[yy-proto](../../crates/yy-proto/src/lib.rs): SSH転送、リモート操作、エージェント通信。
- [yy-draw](../../crates/yy-draw/src/lib.rs): 構成図のモデル・経路・描画・保存/出力。
- [yy-db-core](../../apps/yydbmanager/crates/yy-db-core/src/lib.rs): 接続、SQL、カタログ、計画・ER、型付きデータ入出力。UIからもデモ生成からも利用。
- [yy-merge-core](../../apps/yymerge/crates/yy-merge-core/src/lib.rs): 比較と保存・同期の保護。CLIとUIで判定・モデルを共有。
- [yy3-core](../../apps/yy3designer/crates/yy3-core/src/lib.rs): ドット絵編集、6方向からの立体化、丸み、PNG/OBJとプロジェクト保存。

現在のアルゴリズム・境界条件は各モジュールのRustdocとテストを参照します。
要件からの到達経緯・実装途中の判断は[提案履歴](../history/proposals/README.md)に保存します。

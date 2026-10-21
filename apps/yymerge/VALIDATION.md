# yymerge 初期実装の検証

2026-10-21、Rust 1.99.0。WindowsローカルおよびDockerの`rust:1.99.0-bookworm`で実施しました。GitHub Actions上の実行結果ではなく、同じコマンドのローカル検証です。

| 検証 | 結果 |
|---|---|
| 専用ワークスペース fmt / Windows Clippy `-D warnings` | 成功 |
| Windows コア・CLI試験 | 14試験成功 |
| Linux コア・CLI試験、リンク循環の拒否 | 15試験成功 |
| Linux GUIなし Clippy `-D warnings` | 成功 |
| Windows GUI / CLI releaseビルド | 成功 |
| 共通yy-ui-winの既存試験（フォント・描画） | 7試験成功 |
| GUI競合選択・取消・編集画面の描画・安全保存 | 成功 |
| GUIリサイズ 680×420 / 1920×1080 / 1000×700 / 1280×820 | 全ペインがクライアント領域内にあることを確認 |
| 共通ヘルプ、テキストフォールバック | 両モードのGUIスモーク成功 |
| 既存出力保護、入力変更検知、CP932 / Unicode往復 | 成功 |
| Unified出力をGitで適用して対象を復元 | 成功 |

小規模入力の4,096組について、独立したLCS計算と編集数を照合し、左右の復元と片側変更マージの性質を確認しています。改行・終端改行の相違、同位置への異なる挿入、削除対変更、隣接した独立変更、比較上限と取消も対象です。

## 初期性能測定

`tests/benchmark.py`でUTF-8の7,800,000 bytes、100,000行を作り、release CLIを各5回実行しました。ファイルの読み込み・デコード・比較・必要な入力再検査を含むプロセス全体の時間です。

| 条件 | 中央値 | 最大 | 終了コード |
|---|---:|---:|---:|
| 完全一致 | 52.13 ms | 160.13 ms | 0 |
| 離れた10行の変更 | 52.31 ms | 61.06 ms | 1 |
| 全行変更で追跡予算を超過 | 71.01 ms | 94.37 ms | 2（比較未完了） |

ローカルのキャッシュされたディスク上での観測値です。ピークメモリー、コールドディスク、GUIのp95、混在DPI、巨大入力の目標値は未評価です。提案書の性能目標を達成したという判定には使いません。

再実行:

```powershell
cargo test --manifest-path apps/yymerge/Cargo.toml --workspace --locked
cargo clippy --manifest-path apps/yymerge/Cargo.toml --workspace --all-targets --locked -- -D warnings
cargo build --manifest-path apps/yymerge/Cargo.toml --release --workspace --locked
python apps/yymerge/tests/benchmark.py apps/yymerge/target/release/yymerge.exe --workdir apps/yymerge/target/benchmark
apps/yymerge/target/release/yymerge.exe gui --ui-smoke-test apps/yymerge/target/ui-smoke --wait
```

`YY_HELP_TEXT_ONLY=1`を指定すると、同じスモークをテキストヘルプで確認できます。Windowsの一時ディレクトリーが制限された環境では、書き込める専用ディレクトリーをTEMP / TMPに指定してテストしてください。

提案書のP0全体は未完了です。整列表示・同期スクロール、GUIフォルダー比較・3方向フォルダー・同期計画、Gitアダプター、多形式比較、ディスク索引は[README](README.md)に残件として整理しています。

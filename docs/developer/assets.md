# 素材・ヘルプ・描画の保守

コマンドはリポジトリルートから実行します。

| 対象 | 生成・検査 | 出力・原稿 |
| --- | --- | --- |
| 業務・ITアイコン | `python tools/business-icons/generate.py` / `--check` | SVGカタログと[利用者向け一覧](../user/yydraw-business-icons.md) |
| イラストアイコン | `python tools/friendly-icons/generate.py` / `--check` | SVGカタログと[利用者向け一覧](../user/yydraw-friendly-icons.md) |
| クラウド素材 | `python tools/cloud-icons/fetch.py` | [出典・利用条件](../../crates/yy-draw/assets/NOTICE.md)、取得URL/SHA/収録数 |
| アプリアイコン | `tools/gen-icon/gen_icon.py` | 各`apps/*/res/` |
| プレビュー | `tools/fetch-preview-assets/fetch.py` | [同梱アセット](../../crates/yy-preview/assets/)とライセンス |
| 同梱ヘルプ | 各アプリの埋め込みMarkdown原稿 | [利用者向け索引](../user/README.md)で原稿を案内 |

生成物だけを手で変更すると`--check`で差分になるため、説明文や一覧の移動時も生成スクリプトを更新します。
通常のyyeditor共通CIはアイコン生成物の整合性も検査します。
フォントはUDEV Gothicをプロセス内に登録し、[フォントライセンス](../../crates/yy-win/fonts/LICENSE-UDEVGothic.txt)を配布します。

## 描画を調査する

```text
cargo run --release -p yy-draw --example perf
cargo run -p yy-draw --example sample -- target/yydraw-sample
```

描画・経路計算は[yy-draw](../../crates/yy-draw/src/lib.rs)を参照します。
測定では要素・接続線数、画像サイズ、キャッシュ状態、OS・CPU・ビルドを記録します。
日付付きの結果は[性能測定履歴](../history/benchmarks/yydraw.md)へ保存し、現行の速度保証と混在させません。

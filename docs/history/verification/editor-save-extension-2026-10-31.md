# yyeditor 保存時の拡張子推定（2026-10-31）

## 挙動

- アウトライン表示で新規保存・名前を付けて保存すると、JSON形式の内容に合わせて`.json`を提案する。
- 通常文書の新規・拡張子なしのローカル保存では、編集モード、手動選択したハイライト、先頭64KiBの内容を利用する。
- JSON/JSONL、Markdown、HTML/XML/SVG、CSV/TSV、YAML/TOML/INI、特徴が明確な主要言語のソース等を推定する。判定できない場合は`.txt`。
- shebang・モードラインの判定には既存のシンタックスレジストリ、CSV/TSVには既存の区切り文字パーサーを利用する。
- 提案は名前・既定拡張子・先頭のファイル種類フィルターに反映する。利用者は任意の別名・拡張子を指定できる。内容は変換しない。
- 通常の名前を付けて保存は既存の拡張子を維持する。Makefile・Dockerfile・ドットファイルの名前も維持する。アウトラインは`.json`の提案が優先。
- 上書き保存の既存パスは変更しない。既存`.yyoutline`を読み込む機能・文書形式も維持する。リモートの独自保存画面のファイル名は従来どおり利用者が指定する。

## 検証

Windows Rust 1.99.0：

- ルートの `cargo fmt --all -- --check` と `cargo clippy --workspace --all-targets --locked -- -D warnings` 成功。
- `cargo test -p yy-win --locked app:: -- --nocapture` 成功（8件）。
- 形式判定の表形式試験でJSON・未完成JSON・JSONL・BOM・日本語・絵文字・各種テキストとソースを確認。Markdownのコードフェンス内の言語を文書形式と取り違えないこと、列数不一致の文章をCSVとしないこと、64KiBより後ろの内容が推定に使われないことを検証。
- アウトラインの既定名、`.txt`/`.yyoutline`からの名前を付けて保存の提案、既存の大文字拡張子、拡張子なしの名前、編集モード・手動ハイライトの優先、Makefile等の維持を検証。
- ネイティブのIFileSaveDialogを生成し、設定後のGetFileName/GetFileTypeIndexで提案が反映されていることを確認。ダイアログを表示して保存ボタンを押す自動化試験ではない。
- アウトラインの保存・再読込試験を`.json`で実施し、編集・Undo/Redo・ID・参照情報と保存バイト列を確認。Markdownやインライン展開の既存保存試験も成功。
- `cargo build -p yyeditor --locked` 成功。動作確認用は `target/debug/yyeditor.exe`。

Linux Rust stable 1.99.1（Docker）：

- 最終差分のルート書式・全ターゲットClippy成功。保存画面・形式推定はWindows専用モジュールであり、実行試験はWindows側で実施した。
- 共有シンタックス・区切り文字ライブラリの実装は変更していない。独立ワークスペースのyymerge・yydbmanager・YY3Designerはyy-winを参照していないことを確認し、検証済みとは扱わない。
- 新しいパッケージの追加はなく、既存serde_jsonへのWindows限定依存をyy-winに追加した（root Cargo.lockの依存辺1件）。

ローカルログ：`target/saveformat-{app-tests,clippy-windows,linux-final,build-final}.log`。公開前のローカル検証記録であり、CI・リリース公開の結果ではない。

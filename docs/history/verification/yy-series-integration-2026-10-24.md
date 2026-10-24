# YYシリーズCI・比較連携の検証（2026-10-24）

## 変更範囲

- yymerge・yydbmanagerの独立ワークフローを共通`CI`へ移管。既存の検証ステップとランナーを維持。
- Windows配布を統合し、yyeditorの隣にyymergew・yymerge・yydbmanagerを配置。各部品の説明・ライセンスを保持。
- yyeditorのタブ・Git比較をyymergeへ変更。現在の編集内容を不変スナップショットから出力し、左右の名前を別引数で渡す。

## 実施した検証

Windows / Rust 1.92.0:

- ルートとyymergeの`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`。
- yymerge全ワークスペース試験: CLI 11、コア72、UI 9、コマンド32、Gitプロセス2件成功。性能測定・Office実描画の明示ignoreは未実行。
- yyeditor比較連携4件成功。日本語、CRLF/LF、終端改行なし、先頭のU+FEFF、複数ピースの編集、空のGit入力、引用符・先頭ハイフンを含む名前、起動失敗時と通常終了時の一時ファイル削除を確認。
- 実際のyymergewを起動する連携試験は、通常ignoreのまま放置せず明示実行。左右の見出しと描画用エディタの内容を検証。共通CIにもこの明示実行を追加。
- yymerge標準のGUIスモークを通常ヘルプ・テキストヘルプで実行。サイズ変更、4ペイン、同期スクロール、競合解決、保存保護等が成功。

Linux / Docker:

- Rust 1.99.1: yymerge全ワークスペースを`--no-default-features --locked --offline`でテスト。CLI 11、コア76、コマンド33、Gitプロセス2件成功（性能測定・Office実描画はignore）。
- Rust 1.99.0: 同構成の全ターゲットClippyを`-D warnings`付きで実行し成功。

ワークフロー・文書:

- YAML構文、ジョブの参照・循環、DB手動実接続オプション、除外パスを検証。
- 移管前のyymerge・yydbmanagerの全ステップとランナーをGit上の内容と構造比較し、一致を確認。
- 文書の相対リンク・アンカーの欠落なし。

## 検証の境界

GitHub上の統合CIは未実行。今回はyydbmanagerのソースと検証コマンドを変更せず、DB実接続・Office実描画の再実行はしていない。
統合配布成果物のアップロードはGitHub Actions実行時に確認する。
比較入力にはyymergeの上限（UTF-8のBOM 3バイト込みで片側32 MiB）が適用される。
比較中にyyeditorを強制終了すると一時ファイルが残る場合がある。操作上の説明は[ヘルプ](../../../crates/yy-win/help/help.md)を参照。

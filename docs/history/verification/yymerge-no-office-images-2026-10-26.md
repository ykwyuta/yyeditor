# Excel・PowerPoint画像比較の廃止とCI所要時間の調査

2026-10-26。基点はyyeditor `f74497d6d122a7eb0c2869396eb1e5c1d60a4c91`。利用者の指示によるExcel・PowerPoint画像比較の廃止を記録する。

## 確認したCI所要時間

[実行40284650502](https://github.com/ykwyuta/yyeditor/actions/runs/40284650502)では、CI全体が約73分、yymerge Linuxが約72分だった。ステップの開始・終了時刻と完了ログを確認した。

| 処理 | 所要時間 | 根拠と解釈 |
| --- | --- | --- |
| LibreOffice・日本語フォント導入 | 約68分 | aptログで160 MBを1時間7分41秒、39.3 kB/sで取得。Ubuntuミラーからの取得待ちが主因と確認済み |
| 実Excel・PowerPoint画像比較試験 | 約36秒 | 試験実行自体は全体の長時間化の主因ではない |
| yydbmanager Windowsキャッシュ保存 | 約6.8分 | 約2.6 GB。圧縮開始から最初の送信まで約6分18秒、送信は約10秒。圧縮・読み取りが主因 |
| ルートWindows releaseビルド | 約6.9分 | yymerge Windows・agent完了後に開始する依存関係も、全体の待ち時間を増やす |

[実行40200759308](https://github.com/ykwyuta/yyeditor/actions/runs/40200759308)では同じOffice導入が約24秒、画像比較試験が約30秒、CI全体が約26分だった。依存廃止の短縮効果はネットワーク状態に左右され、常に68分短縮するという意味ではない。

## 変更

- Office→PDFの外部変換、実行ファイル検出、専用プロファイル、子プロセス取消・回収、変換専用のOOXML事前検査を削除する。
- `office_render`を`pdf_render`へ整理する。PDFのページ描画・欠落検出・原本保護を維持する。
- CLIの`image-diff`はExcel・PowerPointを拒否し、`diff`による内容比較を案内する。GUIの画像比較メニューからOfficeの表記を除く。
- Office変換専用試験と、CIのLibreOffice・Noto CJK導入を削除する。廃止した機能の試験であり、残る機能の試験をスキップする変更ではない。
- Office内容比較、通常画像比較、PDF全ページ・選択ページ比較は継続する。Office内容比較用のzip/XML依存とライセンス表示も維持する。
- JSONレポートの`office_rendered`を廃止する。PDFの`pdf_rendered`と各ページの結果は維持する。
- 既存の検証履歴は当時の記録として残す。現行ヘルプ・開発手順・デモ案内を更新する。

## 検証

- Windows Rust 1.99.0: `cargo fmt --manifest-path apps/yymerge/Cargo.toml --all -- --check`、`cargo clippy --manifest-path apps/yymerge/Cargo.toml --workspace --all-targets --locked -- -D warnings`が成功。ワークスペース試験133件が成功。
- Linux Rust 1.99.1: `rust:1.99-bookworm`コンテナーへSFTP試験用の`openssh-sftp-server`だけを追加し、`soffice`が存在しないことを確認。書式検査、`--workspace --all-targets --no-default-features --locked`のClippy、`--workspace --no-default-features --locked`の試験129件が成功。
- CLI回帰にはOfficeの同形式・異形式・Office/PDF混在の画像比較拒否、失敗時にレポート・差分PNGを生成しないことを含む。既存のOffice内容比較、PDF全ページ・欠落ページ・原本保護、画像比較試験も成功。
- Windowsの初回試験はサンドボックス内の一時ファイル永続化がアクセス拒否となった。同じ全ワークスペース試験を通常権限で再実行して成功。試験内容や検査条件は変更していない。
- Windowsデバッグ実行ファイルのUIスモーク6構成が成功。通常ヘルプ／テキストヘルプそれぞれで、内容比較（Office内容比較を含む）、WebP画像、PDF自動判定・ページ往復移動・欠落比較・680×420／1180×760へのリサイズを確認した。記録は`target/yymerge-no-office-ui-{normal,text}-{content,image,pdf}/`。releaseビルドとGitHub CIは今回未実行。
- ローカルログは`target/yymerge-no-office-windows-{clippy,tests,build}.log`、`target/yymerge-no-office-linux.log`。

変更後のGitHub Actions所要時間は未測定。

## 残る短縮候補

キャッシュ保存を主ブランチだけに限定する案はPRの保存コストを減らせるが、masterの保存時間は残る。キャッシュ対象・依存更新頻度を測定して調整する必要がある。

ルートWindowsのビルド・検査を先に開始し、yymerge実機連携と配布収集だけを成果物待ちへ分離すれば、yymerge Windowsの約6～9分との重なりを増やせる。ただしランナー待機とジョブ数の影響があり、短縮量は未検証。

ルートの実GUI連携試験だけ`cargo test -p yy-win`で起動すると、直前の`--workspace`と機能解決が異なり、依存を再コンパイルする形跡がある。`--workspace`のまま対象試験名を指定する案を次の候補とする。試験範囲を保って再ビルドを抑えられるか実測が必要。

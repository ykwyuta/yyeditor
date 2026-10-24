# yyeditor / YYアプリケーション

[English](README.md) | 日本語

<img src="apps/yyeditor/res/yyeditor-256.png" alt="yyeditor" width="96">

Windows向けの編集・端末・転送・表計算・作図・DB・比較ツール群です。

| アプリ | 現在の主な機能 | 操作・詳細 |
| --- | --- | --- |
| **yyeditor** | 大容量テキスト、日本語/EBCDIC、CSV・固定長・バイナリ、ライブMarkdown編集 | [ガイド](crates/yy-win/help/help.md) |
| **yyterm** | ローカル・SSH・3270、tmux/screen、マクロ、タブ別ログ | [ガイド](crates/yy-win/help/help.md) |
| **yysftp** | SFTP/SCP、再開転送、SSHゲートウェイ、ホスト別一覧コマンド | [ガイド](demo/yysftp/README.md) |
| **yysheet** | 大量CSV、数式、COBOL固定長、レコード長・レイアウト推定 | [ガイド](crates/yy-win/help/sheet.md) |
| **yyclip** | クリップボード・定型文・お気に入りとユーザーのファイル履歴 | [ガイド](apps/yyclip/README.md) |
| **yyfilemanager** | フォルダー同期、検索、重複・世代整理、復元 | [ガイド](crates/yy-win/help/filemanager.md) |
| **yybrowser** | タブブラウザ、プロファイル、プロキシ、広告ブロック | [ガイド](crates/yy-win/help/browser.md) |
| **yydraw** | 構成図、公式クラウド・業務アイコン、グループ・整列、SVG/PNG | [ガイド](docs/user/yydraw.md) |
| **yydbmanager** | 7DBのSQL・実行計画・ER図、データ入出力、保存計画の表示 | [ガイド](docs/user/yydbmanager.md) |
| **yymerge** | 2/3方向マージ、巨大ファイル・SSH入力・フォルダー・Office・画像・PDFの比較 | [ガイド](docs/user/yymerge.md) |
| **YY3Designer** | ドット絵、画像変換、最大6方向の3D化、厚み・丸み、PNG/OBJ | [ガイド](docs/user/yy3designer.md) |
| **yy-agent** | SSH接続先で使うLinux補助プログラム | [ガイド](docs/developer/guide.ja.md) |

## 文書を探す

- **利用者**: [使い始め方・同梱ヘルプ・作例](docs/user/README.md)、[機能一覧](docs/user/applications.ja.md)。
- **開発者**: [内部仕様・ビルド・テスト・CI](docs/developer/README.md)、[開発ガイド](docs/developer/guide.ja.md)。
- **検討経緯・進捗**: [提案・レビュー・検証・性能記録](docs/history/README.md)。

## 動作環境と使い始め方

Windows x64向けの配布資材を展開し、目的のアプリの実行ファイルを起動します。
共通CIの`yyeditor-windows-x64`にはyydbmanager・yymergeも同梱します。YY3Designerは専用CI・配布を維持します。
yyeditorのタブ・Git比較には、同じフォルダーの`yymergew.exe`を使います。使い方はF1またはヘルプメニューで開けます。
GUIのプレビュー・ブラウザ等はWebView2を利用します。yymergeはWindows/Linux向けCLIも提供します。
SSH・外部DBは接続先の設定が必要です。接続方法・形式ごとの制限は各ヘルプを確認してください。

## 別リポジトリ

- [YYSort](https://github.com/ykwyuta/YYSort): Windows/Linux CLI。巨大な固定長・RFC形式CSVの外部ソート、DFSORT制御文の対応範囲、CPU/メモリ制限。
- [YYLibrary](https://github.com/ykwyuta/YYLibrary): 共用の文字コード処理・対応表生成。Cargoの固定コミット参照で利用します。
- [DuckDB-FileIndex](https://github.com/ykwyuta/DuckDB-FileIndex): DuckDBでCSV・固定長ファイルに永続索引を付ける独立拡張。

yysqlfileは廃止しています。現行のファイル索引機能はDuckDB-FileIndexを参照してください。

## ライセンス

[GPL-3.0-or-later](COPYING)と[WebView2 Loaderの追加許可](COPYING.EXCEPTION)。
同梱素材・ライブラリには個別の条件が適用されます。[利用機能一覧のライセンス案内](docs/user/applications.ja.md#ライセンス)を参照してください。

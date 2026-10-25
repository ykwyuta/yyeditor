# 開発者向け文書

[文書全体](../README.md)。ここには現行実装を変更・保守するための仕様と再現手順を置きます。

| 文書 | 内容 |
| --- | --- |
| [開発ガイド](guide.ja.md) / [English](guide.md) | 環境、ビルド、テスト、各ワークスペース、CI、配布 |
| [内部構成](architecture.md) | モジュール境界、独立ワークスペース、現行仕様の参照先 |
| [バッファ・I/O・ジョブ](buffer-io-jobs.md) | ピースツリー、保存、取消と進捗 |
| [リモート・データモデル](remote-and-data-models.md) | 通信、シート、図、比較、DBの内部境界 |
| [SSH接続設定](ssh-connections.md) | 接続先GUI、SFTP/SCP、エージェントなし編集、ゲートウェイ |
| [データ形式とAPI](formats.md) | 保存形式・プロトコル・文字コードの実装元と互換性の扱い |
| [素材と描画](assets.md) | アイコン生成、フォント、プレビュー、描画性能の調査方法 |
| [yydbmanager](yydbmanager.md) | ネイティブDB、DuckDBのツールチェーン、CLI/UI/実DB試験 |
| [yymerge](yymerge.md) | GUI/CLIのビルドとヘッドレス構成 |
| [yyproject（独立リポジトリ）](https://github.com/ykwyuta/YYproject/blob/master/docs/developer/yyproject.md) | SQLite・MCP・ネイティブGUIの構成と検証 |
| [YY3Designer](yy3designer.md) | 画像・3DコアとWindows UIの検証 |
| [yydraw](yydraw.md) | アプリ・作例のビルド |

作業ルールは[AGENTS.md](../../AGENTS.md)。提案・過去のレビュー・測定結果は
[履歴](../history/README.md)へ置き、現在の内部仕様と混在させません。

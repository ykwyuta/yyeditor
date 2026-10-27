# 利用者向けガイド

[文書全体](../README.md) / [機能一覧](applications.ja.md) / [Feature guide](applications.md)

## 操作マニュアル

| アプリ | 使い方・設定 | 作例・体験用データ |
| --- | --- | --- |
| yyeditor | [同梱ヘルプ](../../crates/yy-win/help/help.md) / [アウトライン編集](outline.md) | [アウトライン作例](../../demo/outline/example.yyoutline) |
| yyterm | [同梱ヘルプのターミナル項目](../../crates/yy-win/help/help.md)（SSH、3270、tmux/screen、マクロ、タブログ） | [SSHマクロ](../../demo/yyterm/README.md) |
| yysftp | [同梱ヘルプ](../../crates/yy-win/help/help.md#sftp-gateway) | [ゲートウェイ設定例](../../demo/yysftp/README.md) |
| yysheet | [同梱ヘルプ](../../crates/yy-win/help/sheet.md) | CSV・固定長ファイルを開く |
| yyclip | [履歴・テンプレート・ファイル履歴](../../apps/yyclip/README.md) / [期限・定期作業](yyclip-deadlines.md) | 通知領域から起動 |
| yyfilemanager | [同梱ヘルプ](../../crates/yy-win/help/filemanager.md) | 同期計画を作って確認する |
| yybrowser | [同梱ヘルプ](../../crates/yy-win/help/browser.md) | プロファイル・接続経路を設定 |
| yydraw | [使い方](yydraw.md) / [同梱ヘルプ](../../crates/yy-win/help/draw.md) | [業務・ITアイコン](yydraw-business-icons.md) / [イラスト](yydraw-friendly-icons.md) |
| yydbmanager | [使い方](yydbmanager.md) / [同梱ヘルプ](../../apps/yydbmanager/help/help.md) | [7DBデモ](../../demo/yydbmanager/README.md) / [実行計画](../../demo/yydbmanager/plans/README.md) |
| yyproject | [独立リポジトリの使い方](https://github.com/ykwyuta/YYproject/blob/master/docs/user/yyproject.md) / [ヘルプ・MCP](https://github.com/ykwyuta/YYproject/blob/master/help/help.md) | `yyproject --init-demo NEW_DATABASE` |
| yymerge | [使い方](yymerge.md) / [同梱ヘルプ](../../apps/yymerge/help/help.md) | [テキスト・Office・画像・PDF](../../demo/yymerge/README.md) |
| YY3Designer | [使い方](yy3designer.md) / [同梱ヘルプ](../../apps/yy3designer/help/help.md) | [椅子・車・猫・時計塔など](../../demo/yy3designer/README.md) |

## 使い始める

yyeditor・yyterm・yysftpの接続先は共通の[SSH接続先管理GUI](../../crates/yy-win/help/help.md#ssh-sites)から登録できます。
SFTP／SCP、ProxyJump、L/R/D転送、tmux／screenもホスト別に設定します。

yyeditor・yytermは各アプリの「ヘルプ → エクスプローラーの右クリックに登録」から、
ファイルやフォルダーの右クリックメニューに追加できます。[登録・解除の説明](../../crates/yy-win/help/help.md#explorer)を参照してください。

Windowsの配布資材を展開して使うアプリの実行ファイルを起動します。yymergeはGUI用の`yymergew.exe`と
CLI用の`yymerge.exe`を分けています。yydbmanager・yymerge・YY3Designerは個別の配布資材です。
ヘルプはF1またはヘルプメニューから開けます。Markdownプレビュー・ブラウザ・ヘルプの表示には
WebView2を利用し、対応するアプリのヘルプは利用できない場合にテキストへ切り替わります。
SSHや外部DBは接続先の権限・設定が必要です。設定値・保存場所・現在の制限は各ヘルプを参照してください。

構築・データ再生成の手順は[開発者向け](../developer/README.md)、過去の実機確認の範囲は
[作業記録](../history/README.md)に分けています。

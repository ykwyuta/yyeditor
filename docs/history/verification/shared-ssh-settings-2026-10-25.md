# YYシリーズの共通SSH設定適用の検証（2026-10-25）

## 対象

yyeditor・yyterm・yysftp・yysheet・yymergeで、接続先検索・Resolver生成・ホスト別転送方式・エージェント利用判定・一覧コマンドの設定解釈をyy-remote::policyへ集約した。共有ファイルは従来と同じconnections.tomlとconfig.toml。明示的なホスト別protocolが最優先で、未指定時のみ既存のアプリ別既定値を使う。

yysheetに共通の接続先管理とSSH診断メニューを追加。シートの読書きとワークスペースのダウンロード／コピーを共通のNative転送に変更した。yymergeにもSCP入力とホスト別のエージェント指定を適用。zip生成などエージェント専用の操作は、ホスト別設定がエージェントを禁止する場合に起動せず通常のyysftp転送へ案内する。

## 検証結果

- Windows: yy-config／yy-remote／yy-ssh全試験が成功。yy-winのホスト別設定3試験、yysheetのWin32メニュー試験が成功。
- Windows: ルートおよびyymerge／yydbmanager／YY3Designerのfmtと全対象Clippy成功。
- Windows: yyeditor・yyterm・yysftp・yysheet・yymergeのdebugビルド成功。yymergeの共通プロファイル回帰試験成功。
- Linux Docker（Rust 1.99.0、OpenSSH client／sftp-server必須）: 共有設定／SSH部品の全試験とClippy成功。yymergeのSSH入力3試験とno-default-features全対象Clippy成功。
- 新しいNative転送試験では、選択した方式以外を呼ぶと失敗するTransportを使用。SCPでSFTPを開始しないこと、SFTPでshell／エージェントを要求しないことを検証した。
- 実SCP／SFTPによるCSV相当バイト列の取得・保存・ローカル／リモートコピー・再帰コピー、ls -laによる再帰一覧、中止時の元ファイル保持、転送中に別の書込み元が作成した最終ファイルの保持を確認した。
- yymergeでは実SCPによる日本語・空白を含む名前の入力取得、拡張子保持、一時ファイルの片付け、中止を確認した。

## 制限

SecureCube Access Check実機での接続は未検証。SCPに必要なシェル・属性取得・一時ファイル操作の許可は接続先側で必要。現在接続中の端末へ設定を強制適用せず、他の起動中アプリは共通設定変更後に再起動する。診断の有効化はプロセス単位。yymergeは比較用の入力を取得するだけで原本へ書き戻さない。

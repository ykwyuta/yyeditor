# SSHゲートウェイ経由の転送

[設定例](gateway-examples.toml)から必要な項目を、ヘルプ → 設定ファイルを開くで開いた`config.toml`へ統合してください。実際のホスト名・ユーザー名・許可された接続方式へ置き換え、yysftpを再起動します。既存の`[transfer]`等がある場合、同名のセクションを重複させず項目を追加・変更します。

接続先に`internal`、`sftp-proxy`または`sftp-command`を指定します。方式の選び方と制約は[同梱ヘルプ](../../crates/yy-win/help/help.md#sftp-gateway)を参照してください。

接続先ごとに一覧コマンドを変える場合は、該当する`[remote.host.名前]`に`list_command = "ls -1A"`と`list_format = "lines"`を指定します。任意のスクリプトも使用できますが、標準出力は直下のファイル名だけにしてください。実行条件・NUL区切り・属性取得の制約は[ディレクトリ一覧コマンド](../../crates/yy-win/help/help.md#sftp-list-command)を参照してください。

SecureCube Access Check／Access Check Essential向けの確認点は[専用メモ](securecube-access-check.md)に記載しています。Essentialの公開仕様に合わせたSCPモードでは、一覧にもSFTPを要求しないよう改善しています。

実製品との接続は未検証です。確認済みの模擬ゲートウェイの範囲は[検証記録](../../docs/history/verification/yysftp-gateway-2026-10-23.md)を参照してください。

参考にした公式仕様: [WinSCPのトンネル](https://winscp.net/eng/docs/ui_login_tunnel)、[SFTPの起動設定](https://winscp.net/eng/docs/ui_login_sftp)、[プロキシ](https://winscp.net/eng/docs/ui_login_proxy)。

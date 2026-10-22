# SSHゲートウェイ経由の転送

[設定例](gateway-examples.toml)から必要な項目を、ヘルプ → 設定ファイルを開くで開いた`config.toml`へ統合してください。実際のホスト名・ユーザー名・許可された接続方式へ置き換え、yysftpを再起動します。既存の`[transfer]`等がある場合、同名のセクションを重複させず項目を追加・変更します。

接続先に`internal`、`sftp-proxy`または`sftp-command`を指定します。方式の選び方と制約は[同梱ヘルプ](../../crates/yy-win/help/help.md#sftp-gateway)を参照してください。

SecureCube Access Check／Access Check Essential向けの確認点は[専用メモ](securecube-access-check.md)に記載しています。Essentialの公開仕様に合わせたSCPモードでは、一覧にもSFTPを要求しないよう改善しています。

製品の実機には接続せず、`crates/yy-ssh/tests/gateway.rs`の模擬SSHゲートウェイを使用しました。Windows上の実際のループバックSSH通信で、次を確認しています。

- direct-tcpipトンネル、PTYなし中継コマンド、複合ユーザー名のSFTPプロキシ、既定SFTPサブシステムの4経路。
- ゲートウェイと接続先それぞれのホスト鍵確認、パスワード・OTPの2段階keyboard-interactive認証。
- ホームディレクトリーの解決、フォルダー一覧、64 KiBの全バイト値を含むデータの並列書き込みと読み出しの一致。
- SFTP起動要求の拒否時に、PermissionDeniedエラーとして復帰すること。

実行コマンドは`cargo +1.99.0 test -p yy-ssh --test gateway --locked`です。外部SSH/SFTPプログラムや製品の認証情報は不要です。実製品ごとのメニューや独自プロトコルとの互換性は未確認です。

参考にした公式仕様: [WinSCPのトンネル](https://winscp.net/eng/docs/ui_login_tunnel)、[SFTPの起動設定](https://winscp.net/eng/docs/ui_login_sftp)、[プロキシ](https://winscp.net/eng/docs/ui_login_proxy)。

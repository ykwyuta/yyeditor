# SSHマクロの検証

2026-10-23、Windows、Rust 1.99.0で実施。

- `cargo +1.99.0 test -p yy-3270-macro -p yy-term -p yyterm --locked`: マクロ6件、端末34件が成功。
- `cargo +1.99.0 test -p yy-win 'term::' --locked`: 14件が成功。
- `cargo +1.99.0 test -p yy-win help::tests --locked`: 目次リンク・ヘルプ表示を含む7件が成功。
- `cargo +1.99.0 clippy -p yyterm -p yy-win -p yy-3270-macro --all-targets --locked -- -D warnings`: 成功。
- `cargo +1.99.0 build -p yyterm --release --locked`: 成功（SSH・TLSの既定機能を含む）。

マクロ試験では変数・定数・配列・マップ・関数・分岐・繰り返し・例外、UTF-8の記録と制御文字のエスケープを確認しています。

GUI試験では実際のRhai実行スレッドとWin32メッセージ処理を使用し、SSHタブの送受信チャネルを模擬しています。日本語コマンドとEnter・Ctrl+Cの送信、応答による待機解除、時間切れ、停止、切断、実行中の利用者入力の抑止を確認しています。画面読み取りでは全角・結合文字・非表示属性と端末モードに応じたキーの符号化も確認しています。

実接続先へのSSH認証・ネットワーク通信を伴う試験は実施していません。SSHトランスポートは既存実装を使用しています。実環境では[サンプル](ssh-diagnostics.rhai)の`run_id`を変更し、接続済みタブから実行できます。

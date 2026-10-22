# yyterm SSHマクロ

接続済みのSSHタブでCtrl+Shift+F5を押し、[ssh-diagnostics.rhai](ssh-diagnostics.rhai)を選びます。`pwd`と`uname -s`を実行し、完了マーカーを待って現在の画面をマクロ出力フォルダーの`ssh-diagnostics.txt`へ保存します。POSIX互換シェル用の例です。

`run_id`は実行ごとに変更してください。入力エコーや前回のマーカーを完了と取り違えないよう、文字列を分割して出力し、実行前にも既存表示を確認しています。

Ctrl+Shift+F6でSSHの操作記録を開始・終了すると、UTF-8の`.rhai`ファイルとして保存できます。記録後はプロンプトや完了マーカーの`wait_text`を加えてください。SSHでは記録中の秘密値も保存されるため、手入力のパスワードは記録せず、`password("名前")`を使ってください。

変数と全命令は[ヘルプ](../../crates/yy-win/help/help.md#terminal-macros)を参照してください。`[tn3270.macro]`の`folder` / `output` / `timeout`設定はSSHと3270で共通です。

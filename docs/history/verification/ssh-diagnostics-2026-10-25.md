# SSH診断・ls長形式一覧の検証（2026-10-25）

## 実装範囲

- yyeditor・yyterm・yysftp共通のヘルプメニューから構造化SSH診断を開始・停止・UTF-8テキストへ書き出す。
- 任意のユーザー名、ホスト名、ファイル／端末本文、認証プロンプト、コマンド、Error本文を記録しない。エラーは種類とOSコード、ユーザー名は@の数を記録する。
- プロセス単位で既定無効。8192イベント／4 MiBで停止し、ディスク書込み失敗と上限到達をレポートに表示する。
- 独自一覧にlist_format="ls"を追加。ls -laの通常日時・ISO日時、空白・日本語名、シンボリックリンクを扱う。引用・エスケープされた名前など曖昧な形式は失敗させる。
- 独自一覧をyysftpだけでなくyyeditor・yytermでも適用する。
- 接続先ユーザー名operator@target-user@target-hostを模擬SSHサーバーがそのまま受信することを確認。

## 実施結果

- Windows: yy-config／yy-remote／yy-sshの全試験、診断メニューのWin32試験、接続先別設定の3試験が成功。
- Windows: Git for Windowsの実GNUツールでls -laによる一覧と属性を確認。空白・引用符・日本語を含む名前を保持。
- Linux Docker（Rust 1.99.0、OpenSSH clientとsftp-server必須）: yy-config／yy-remote／yy-ssh全試験が成功。実ls -la、SCP／SFTPのファイル編集往復、転送再開、模擬ゲートウェイ認証とチャネル処理を含む。
- 診断の試験: Error本文に埋めた秘密文字列がファイル／レポートへ出ないこと、停止、再開始時の別ファイル、上限停止を確認。
- ルートとyymerge／yydbmanager／YY3Designerのfmt検査、Windows全対象Clippy成功。Linuxの共有SSH部品Clippyとyymergeのno-default-features全対象Clippy成功。
- yyeditor・yyterm・yysftpのWindows debugビルド成功。

## 検証の限界

SecureCube Access Check実機への接続はできていない。製品側でGNU stat等の属性・操作コマンドやPTYなしexec／shellが許可されているかは未確認。ls長形式はファイル名抽出に使用し、属性取得や保存操作の要件をなくすものではない。診断レポートのヒントはクライアントの失敗段階からの推定で、製品側ログを取得した結果ではない。

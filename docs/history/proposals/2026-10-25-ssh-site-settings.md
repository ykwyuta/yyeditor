# SSH接続先GUI・SecureCube対応とWinSCP設定の対応表

2026-10-25。対象はyyeditor、yyterm、yysftp。ユーザー指定によりFTP・WebDAV・S3は対象外。
WinSCPのSSH関連全設定との同等性を目標とし、未実装の項目を設定欄だけ作って対応済みにはしない。
現行の使い方は[同梱ヘルプ](../../../crates/yy-win/help/help.md#ssh-sites)、内部仕様は[開発者向け](../../developer/ssh-connections.md)。

## 製品仕様からの判断

[NRI公式FAQ](https://www.nri-secure.co.jp/service/solution/access-check-essential)はEssentialの中継対象をSSH・RDP・SCPとする。
[標準版公式カタログ](https://www.nri-secure.co.jp/hubfs/NRIS/PDF/ac_pamphlet_jp.pdf)はSFTPも掲載する。
したがって両製品でSFTPが使えるとは仮定せず、ホストごとの方式を明示する。
RHEL上の製品だから一般的なOpenSSH踏み台と同じだとは判断しない。
製品固有のユーザー名、接続口、許可チャネル、コマンド制限は実環境の手順で確認する必要がある。
今回の失敗原因がSFTP要求・exec制限・エージェント配置のどれだったかは、ログ・実機なしには断定できない。

## 対応状況と次の実装単位

WinSCPの[詳細設定の分類](https://winscp.net/eng/docs/ui_login_advanced)と
[raw設定一覧](https://winscp.net/eng/docs/rawsettings)を比較の入口とした。

| 設定群 | 現在の対応 | 残る設定／機能 |
| --- | --- | --- |
| 接続先 | 共通GUI、新規・編集・複製・削除、独自TOML、ホスト・ユーザー・ポート・鍵 | 注記、接続色、WinSCP／PuTTY設定のインポート |
| 認証 | 公開鍵、password、複数段keyboard-interactive、方法の有効／無効、資格情報保存 | GSSAPI／SSPI／Kerberos、ssh-agent／Pageant、PPK、証明書認証、方式順序の編集 |
| 接続維持 | SSH接続確立タイムアウト、keepalive間隔・未応答回数 | IP family／送信元IP、TCP keepalive、ダミーコマンドによるkeepalive、プロキシ交渉・転送I/Oごとのタイムアウト |
| Proxy | HTTP CONNECT、SOCKS4／4a／5、認証、ホスト別none | Telnetプロキシ、任意ローカルProxyCommand、プロキシDNSの選択、localhost除外 |
| Tunnel | 複数段ProxyJump、踏み台ごとの認証、direct-tcpip／中継コマンド | 他アプリと共有する固定トンネルローカルポート |
| SSH暗号 | cipher／KEX／host key／MACの優先順、圧縮 | rekey閾値・時間、旧式アルゴリズムの警告境界、サーバーバグ回避の個別スイッチ |
| SFTP | v3、独自subsystem、サーバーコマンド、POSIX rename検出 | v4～6、canonicalize切替、自動SCPフォールバック、リンク引数順・1970年以前の時刻バグ回避 |
| SCP/Shell | 標準SCP、exec／PTYなしshell、バナー分離、独自一覧コマンド | shellパス・終了コード変数、ls長形式解析、groups照会、alias／locale初期化、scp1互換フラグ |
| Environment | 開始リモートフォルダー、バイト列のファイル名保持 | 開始ローカルフォルダー、ファイル名文字コード、時差／DST補正、ディレクトリキャッシュ方針 |
| ファイル保護 | 編集の競合確認、一時ファイル、キャンセル時の元ファイル保持 | リモートごみ箱、所有者・ACLの完全継承、透過ファイル暗号化 |
| 独自設定 | agent配置、一覧コマンド、L/R/D転送、tmux／screenと必須prefix | その他のグローバルSSH設定のGUI化 |

[SCP/Shell公式仕様](https://winscp.net/eng/docs/ui_login_scp)と
[SFTP公式仕様](https://winscp.net/eng/docs/ui_login_sftp)も参照した。
WinSCPの設定キーや設定値をそのまま解釈する互換レイヤーは実装していない。

## 次の検証

1. SecureCubeで成功しているクライアントの秘密を除いた接続手順と、失敗したチャネル種別を取得する。
2. Unix以外の中継先・GNUツールを制限する構成に対し、ls形式解析・SCP/Shellオプションを追加する。
3. 上表の未対応を独立した回帰テスト付きで実装し、GSSAPI等は必要なOS／ライブラリ境界を設計する。

今回の自動テストは一般的なSSHの制限を再現するもので、製品実機の互換認定ではない。

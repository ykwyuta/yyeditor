# SecureCube Access Check／Access Check Essential

転送用の接続口・ログイン名・許可されるプロトコルは管理者の案内を確認してください。

- [SecureCube Access Check公式カタログ](https://www.nri-secure.co.jp/hubfs/NRIS/PDF/ac_pamphlet_jp.pdf)は、OSにRHEL、管理対象プロトコルにSFTP・SCPを掲載。
- [Access Check Essential公式FAQ](https://www.nri-secure.co.jp/service/solution/accesscheck/access-check-essential)は中継プロトコルをSSH・RDP・SCPと記載。SFTPは掲載されていないため、対応を仮定しません。
- [WinSCPのSCP要件](https://winscp.net/eng/docs/requirements#scp_requirements)も、一覧などの操作にUnixシェルコマンドを必要とします。

## 設定

管理者から案内された転送用ゲートウェイのアドレス・ポート・SSHログイン名を設定します。公開資料では製品固有の接続先指定構文を確認できなかったため、ログイン名の書式は例示しません。既存のWinSCP利用手順があれば、プロトコル・ホスト・ポート・ユーザー名を確認してください。

```toml
[transfer]
protocol = "scp"       # SFTPが許可される標準版環境ならsftpを選ぶ
use_agent = false

[remote.host.access-check]
hostname = "gateway.example.com" # 管理者指定の転送用接続口
port = 22                        # 実際のポートに置換
user = "provided-login-name"     # 接続先指定を含む場合も、案内された値をそのまま設定
```

yysftpを再起動し、`access-check`へ接続します。パスワードやOTPは接続時の質問に答えます。`proxy_jump`、`jump_command`、`sftp_server`、`sftp_subsystem`は、製品側から明示的な指定がない限り設定しません。RHEL上の製品であることから、ゲートウェイ上のOpenSSHプログラムを直接起動できると推定することはできません。

SCPモード・エージェント無効時の一覧は、接続先のPOSIXシェルとGNU find/stat/realpath等を使い、SFTPサブシステムを要求しません。転送には既存のSCP転送機構を使います。中継先がGNUコマンドのないOSの場合、または製品が一覧用のexec要求を拒否する場合は、この一覧方式は利用できません。対話シェルの接続先選択メニューを自動操作する機能はありません。

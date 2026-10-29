# SSH接続設定とエージェントなしファイル編集

## 暗号バックエンド

`yy-ssh`はRustCryptoを使うローカル修正版のrussh 0.63.1を利用する。上流の固定版ソースと変更範囲は[`vendor/russh/YYEDITOR-PATCH.md`](../../vendor/russh/YYEDITOR-PATCH.md)を参照。依存を更新する際はローカル変更を再確認し、ルートとyymergeが同じソース・機能（`rustcrypto,rsa,flate2`）を解決することを確認する。

`yy-ssh-crypto`はAES-128/256-GCMとOpenSSH形式のChaCha20-Poly1305を担当する。AES-GCMは暗号化しない4バイトの長さをAADに含め、固定32ビット＋64ビットの呼出カウンターを使用する。カウンターの周回後は拒否する。ChaCha20-Poly1305はヘッダーと本文に別の鍵を使い、64ビットのnonce、本文のブロックカウンター1、暗号文全体へのPoly1305認証を使う。IETFの同名AEADとは形式が異なる。再鍵交換とSSH連番の管理はrusshに残す。

通常のSSH依存はring/AWS-LCを使わない。暗号互換性の比較試験のみringを開発依存として使う。3270のTLSも[共有RustCryptoバックエンド](tls-crypto.md)を使う。独立yydbmanagerのTLSは別途移行が必要。

```sh
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test -p yy-ssh-crypto -p yy-ssh -p yy-agent -p yy-remote --locked
```

Linuxの`yy-ssh/tests/openssh.rs`は外部OpenSSHクライアントを相手に、各AEAD暗号のバイナリ往復と再鍵交換を検証する。`YY_REQUIRE_OPENSSH=1`でOpenSSHを必須にする。CIはこの設定を有効にし、既存のOpenSSH導入手順を利用する。SFTP試験は別途sftp-serverも必要。通常の製品動作にOpenSSHの実行ファイルを追加したわけではない。

## 設定の所有と優先順位

操作方法・保存例は[同梱ヘルプ](../../crates/yy-win/help/help.md#ssh-sites)。
`yy-config::connections::Profiles` がversion 1の `connections.toml` を管理する。
`Config::load_from` は同じディレクトリーのprofilesを読み、同名のlegacy host定義全体を置換する。
`removed` はGUIで削除したlegacy定義の復活を抑止する。OpenSSH設定ファイルは変更しない。
プロファイル更新は固定のsidecarロックを取得して再読込し、同一ディレクトリーの一時ファイルをfsyncして置換する。
ロックファイル自体は残るが、ロックはOSハンドルの寿命で解放される。

`yy-win::sites` は4アプリ共通の接続先管理・カテゴリ別編集を提供する。
新規／複製時は既存名への上書きを拒否する。変更を保存したアプリはRemoteStateの接続キャッシュを破棄し、
すでに端末・転送・保存処理が所有しているArcは維持する。他プロセスは再起動時に取り込む。
秘密情報の保存は既存のWindows資格情報マネージャー経路を使う。GUIのプロキシURLに平文パスワードは許可しない。

## 転送方式

ホストの `protocol` が指定された場合はグローバル `use_agent` より優先し、エージェントを使用しない。
SCPではShellFsを使い、SFTP要求・エージェント配置・転送後のエージェントhash検証を行わない。
yysftpは一覧対象・アップロード先・ダウンロード元のTargetから方式を解決し、ジョブに確定値を保持する。
転送メニューで接続中の方式を変更した場合はホストのprofileへ保存する。未接続時だけ従来の今回の既定値を変更する。

エディタの `EditorConnection` は専用エージェントと `yy_remote::file_access::FileAccess` を分岐する。
SFTPは読み書きに32KiB単位のパケット、SCPは標準SCPのバイナリ転送を使う。
FileAccessは前後のidentity／サイズ確認、キャンセル、同一フォルダーへのステージング、変更競合、基本mode保持を担当する。
SFTPの既存ファイルの保存ではPOSIX rename拡張を必須にし、remove＋renameによる擬似置換を拒否する。
SCPではnoclobberで一時パスを予約し、GNU `mv -T -f` で置換する。失敗時は所有する一時ファイルだけを削除する。
サーバー側CAS操作ではないので、最終statとrenameの間の競合は排除できない。秒単位mtimeとサイズが同じ変更も検出できない場合がある。
所有者・ACL等の完全な継承は保証しない。

## SSH設定とL7ゲートウェイ

HostOverride／HostSpecは共有のSshSettingsを保持し、踏み台ごとにrusshのConfigを生成する。
タイムアウト、keepalive、認証方式、暗号・鍵交換・ホスト鍵・MAC・圧縮を反映する。
公開鍵→keyboard-interactive→passwordの既存順序は維持し、無効な方法を試さない。
不明な暗号等は入力エラーにし、既定値へ黙って戻さない。OpenSSH方式の正式名を使い、WinSCP raw名は解釈しない。

`exec_via_shell` はPTYなしshellを要求し、Success後に境界マーカーと `exec sh -c <quoted command>` を送る。
STDOUTは開始境界まで上限64KiB／接続タイムアウト以内で走査する。境界より後のバイトは失わず後段へ渡す。
SFTPサーバーのカスタムコマンドにもこの入口を使える。通常のSFTP subsystemは独立した要求のまま。
端末のPTYつきshell、tmux/screen、forwardingは既存経路を維持する。
PTY必須の接続先選択メニューの自動化やSecureCube固有プロトコルの推定は行わない。

## 回帰検証

- yy-config: 別ファイル保存、legacy上書き・削除、同時更新、失敗時の設定保持。
- yy-win: 実ネイティブダイアログで6カテゴリを巡回し、L/R/D転送・tmux・ProxyJumpを保持。
- yy-ssh: 実SSHソケット、多段認証、トンネル、カスタムサブシステム、バナー付きshell／全バイト値。
- yy-remote（Unix）: 実scp／sftp-serverでエディタ読み書き、競合・キャンセル・リンク拒否・一時ファイル回収。

SecureCube Access Check／Essential実機への接続は未検証。仕様差と残作業は
[WinSCPとの対応表](../history/proposals/2026-10-25-ssh-site-settings.md)で管理する。


## 構造化SSH診断

`yy-remote::diagnostics`はプロセス単位のopt-in記録。`Context`は匿名の接続ID、`Span`は操作IDで開始・結果・途中終了を関連付ける。値は静的コード、数値、真偽値に限定し、任意文字列やErrorのDisplayを保存しない。記録対象はSSH経路・認証方式／回数・チャネル種別／終了コード、SFTP状態コード、SCPと一覧処理の成否。サーバーの出力、MFA質問、実際のユーザー名・ホスト名・ファイル名・コマンドは対象外。

`yy-win::diagnosticui`から4アプリ共通で開始・停止・テキスト書き出しを行う。上限8192イベント／4 MiB、上限時は停止。元ファイルへ逐次flushし、現在の記録はメモリから安全なレポートを生成する。書き出しは一時ファイルを同期して置換し、元の記録への上書きは拒否する。既存の自由記述ログを転載しない。解析ヒントは記録された失敗段階からの推定であり、SecureCube側の設定を観測した結果ではない。

`ListCommand`の`ls`形式はUnixの長形式から名前を抽出する。属性は元バックエンドで取得するためSCPではGNU stat等が引き続き必要。引用・エスケープ形式や曖昧なシンボリックリンク表示はエラーとして扱う。独自一覧は転送方式と独立して3アプリから利用する。


## アプリ間でのポリシー共通化

`yy-remote::policy`に接続名の検索（大小文字を区別しない）、ホスト別転送方式、エージェント利用判定、一覧設定、Resolver生成を集約する。yy-winの4アプリ（yyeditor・yyterm・yysftp・yysheet）とyymergeが同じ処理を利用する。明示的なprotocolはuse_agentより優先し、SCP設定でSFTPやエージェントへ暗黙に切り替えない。ホスト別指定がない場合だけ既存のアプリごとの既定値を使う。

yy-winの`remote::file_location`はシートの取得・保存、ワークスペースのダウンロード・コピーを`transfer::Loc::Native`へ接続する。NativeはFileAccessを使い、同じ転送方式と一覧を再帰処理にも引き継ぐ。ファイル保存は同一ディレクトリの一時ファイルへ書いて変更を検査してから置き換える。Nativeの単一ファイル転送の失敗時はFileAccessが自分の一時ファイルを消すため、汎用コピー処理から最終パスを削除しない（競合して作成された他者のファイルを保護する）。

zip生成はエージェント専用なので、ホスト別設定がエージェントを禁止する場合は開始せず、yysftpの通常のフォルダー転送へ案内する。端末専用のtmux／screenとポート転送の用途は従来通り。yymergeは読取入力だけを取得し、原本へ書き戻さない。各アプリの接続先設定ファイルは共通だが、他プロセスが既にロードしている設定は再起動後に反映する。

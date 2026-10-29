# SSH暗号のPure Rust化（2026-10-29）

変更前コミット: `7ec737683560f5e36b696d9108aade86dacc58ae`。BLAKE3・証明書署名の未コミット変更に追加した。Windows x64 / Linux x64、Rust 1.99.0で検証。現在の変更は未プッシュでCIは未実行。

## 変更と範囲

`russh 0.63.1`の取得済み公開ソースを`vendor/russh`に保持し、`rustcrypto`機能を追加した。元のコミットは`d3ae702a43a163946f258297e398dc216339d5ce`。元ソースの変更はCargo.tomlと4つのRustファイルに限定し、AES-GCM用のアダプターを追加。原著作者・Apache-2.0ライセンス・変更一覧を保持した。新しい公開上流版として扱わない。

共有`yy-ssh-crypto`でRustCryptoのaes-gcm 0.11.1、chacha20 0.10.2、poly1305 0.9.1を使用する。独自の暗号プリミティブは作らず、SSHパケット形式への接続部分を実装した。RSA・鍵交換・認証・圧縮・暗号の選択肢・SFTP・ポート転送を削除していない。ルートとyymergeのlockfileを同じローカルrusshへ更新した。yydbmanager・YY3Designerの依存グラフにはrussh/yy-sshがなく、今回の影響対象ではない。

AES-GCMのnonceは固定4バイト＋64ビットの呼出カウンター。下位64ビットが周回した後は使用を拒否し、固定部分へ繰り上げない。認証失敗で平文を公開しない。ChaCha20-Poly1305はOpenSSHの64バイト鍵・別ヘッダー鍵・64ビットnonce・ブロック0からのPoly1305鍵導出・ブロック1からの本文暗号化を実装し、復号前にタグを定数時間比較する。SSHの再鍵交換・連番管理は元のrusshを使う。

形式の根拠: [OpenSSHの暗号実装](https://github.com/openssh/openssh-portable/blob/master/cipher-chachapoly.c)、[RFC 5647](https://www.rfc-editor.org/rfc/rfc5647.html)、使用中ring 0.17.14とrussh 0.63.1の実ソース。

## 検証

Windows/Linuxでルートの全対象Clippy（`--workspace --all-targets --locked -- -D warnings`）と書式検査が成功。Windowsでは増分キャッシュのハードリンクが使えずコピーする環境警告が出たが、コードのClippy警告はない。Linux限定の新規試験には初回に構造体初期化のClippy指摘があり、初期化式へ直して再実行した。

修正した依存本体にも`cargo clippy -p russh --lib --locked -- -D warnings`を明示適用してWindows/Linuxで成功し、両方のCIジョブに同じ検査を追加した。

- 暗号部品: Windows/Linux各3試験が成功。ChaCha20は3種類の鍵×7つの連番（0、境界、u32最大）×11入力長の231条件で、ringの暗号文・タグ・長さ復号と一致。AES-128/256-GCMも11入力長で一致。両実装で復号でき、タグ・ヘッダー・本文・連番の改変、短すぎる入力を拒否する。AES-GCMカウンターの桁上がりと周回拒否も確認。
- Windows: yy-sshの5単体試験、ゲートウェイ・トンネル・SFTPの2結合試験が成功。
- Linux: yy-agent、yy-remote、yy-ssh、暗号部品の選択試験129件が成功。認証、ホスト鍵、エージェント配置、SFTP/SCP、ポート転送、プロキシ、tmux/screen等の実試験を含む。
- OpenSSH: 9.2p1の実クライアントと、ChaCha20-Poly1305・AES-128-GCM・AES-256-GCMそれぞれで512 KiBのバイナリを往復してバイト一致。64 KiBでの再鍵交換を要求し、完了を示すNEWKEYS送信ログが複数あることを確認。固定sleepは使わず、子プロセス終了と転送結果を期限付きで待ち、終了を回収する。
- yymerge Windows: 独立ワークスペースの書式・全対象Clippy・通常試験133件が成功。

yymergeのLinux構成も`--no-default-features`付きの書式・全対象Clippy・通常試験129件が成功。openssh-sftp-serverを導入し、`YYMERGE_REQUIRE_SFTP=1`で実転送を必須にして確認した。元のエラーを意図的に発生させるCLI引数試験の出力はあるが、試験全体の終了コードは0。

SSHのみを参照する独立検証用パッケージで依存の機能統合を分離した。解決されたrusshの機能は`flate2,rsa,rustcrypto`だけで、ring・AWS-LC・OpenSSLは含まれない。Windows/Linuxとも新しいビルド先で`CC`・`CXX`・`AR`を存在しないパスにし、SSH依存全体のビルドとAES-GCMの暗号化・復号が成功した。BLAKE3のRust製ビルド依存`cc`は残るが、その存在をネイティブ実装の使用とは扱わない。

作業ログは`target/ssh-pure-root-clippy.log`、`ssh-pure-windows-tests.log`、`ssh-pure-linux.log`、`ssh-pure-merge-clippy.log`、`ssh-pure-merge-windows-tests.log`、`ssh-pure-merge-linux.log`、`ssh-pure-probe-windows.log`、`ssh-pure-probe-linux.log`に保存した。Linuxは既存のRust検証用Dockerイメージを使い、ソースとCargoキャッシュを読み取り専用でマウントした。

## 残る依存と制限

ringは暗号部品の比較用開発依存と、3270のTLS等の別経路に残る。通常のSSHにringを使わないことと、ルート全体からringがなくなったことは区別する。Cコンパイラー不要の確認でも、リンカー・OS APIを不要にしたとは扱わない。ARM64/muslの実行・LinuxからWindowsへのクロス検査と、現変更のCIは今回未実行。速度比較は実施していない。

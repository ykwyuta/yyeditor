# 証明書署名のPure Rust化（2026-10-29）

変更前コミット: `7ec737683560f5e36b696d9108aade86dacc58ae`。先行する未コミットのBLAKE3変更に追加した。Windows x64とLinux x64、Rust 1.99.0で検証した。現在の変更は未プッシュで、CIは未実行。

## 変更

- rcgen 0.14.7の`crypto`・`ring`を外し、`pem`のみ有効化。
- 既存の依存グラフにあるp256 0.14.0を使う共有`yy-cert`を追加。鍵生成、DER形式のECDSA署名、PKCS#8保存を担当する。
- yybrowser・3270模擬ホスト・3270 TLS試験を同じアダプターへ移行。SAN、用途、DN、有効期間、PEMファイルの扱いは利用側で維持。
- rcgenの暗号バックエンドが補っていたシリアル番号を、OS乱数から明示生成。鍵識別子もSHA-256で指定。
- 解決済み依存を確認し、yymerge・yydbmanager・YY3Designerには今回の証明書生成部品の利用がない。独立ワークスペースの依存・lockは変更しない。

## 検証

Windows/Linuxとも、ルートワークスペースの`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`が成功した。Windowsの一部実行には増分キャッシュのハードリンクが使えないためコピーする旨の環境警告が出たが、Clippyのコード警告はなかった。

`cargo test -p yy-cert -p yy-browser -p yy-3270-tls -p yy-3270-mock --features yy-browser/devcert --locked`で、Windowsは63件、Linuxは62件が成功した。差は既存のWindows専用試験。最終更新後、yy-cert・yy-browserの試験を再実行してWindows37件・Linux36件の成功を確認した。

- PKCS#8 DER/PEMを読み戻し、公開鍵と署名の互換性を確認。
- ASN.1 DER署名を検証し、改変したメッセージ・別鍵での検証は拒否。
- 証明書を別のX.509パーサーで読み、ECDSA/SHA-256、SAN（DNS・IPv4・IPv6）、ServerAuth、有効期間、20オクテットの正のシリアルを確認。証明書本体の署名も検証。
- yybrowserの生成した証明書・鍵をrustls/ringで読み込み、TLS 1.2と1.3をそれぞれ明示したハンドシェイクと暗号化データ転送が成功。
- 実ソケットでの3270 TLS・STARTTLS・クライアント証明書・TOFU・証明書拒否等の回帰試験が成功。

Windowsの通常サンドボックスではソケットと一部一時ファイルへのアクセスが拒否されたため、必要なアクセスを許可した環境で再実行して成功した。試験のスキップ・無効化は行っていない。

別の検証用パッケージからyy-certとyy-browser/devcertだけを参照して依存の機能統合を分離した。解決されたrcgenの機能は`pem`のみ。通常・ビルド依存にring、AWS-LC、ccおよびネイティブ`links`指定がないことを確認した。既存lockの版を使い、Windows/Linuxとも`CC`・`CXX`・`AR`を存在しないパスにした新しいビルド先でコンパイル・実行が成功し、鍵とブラウザー用証明書を生成できた。

実行ログは作業用の`target/cert-tests-windows.log`、`cert-final-tests-windows.log`、`cert-clippy-windows.log`、`cert-linux.log`、`cert-final-linux.log`、`cert-pure-probe.log`に保存した。Linuxは既存のRust検証用Dockerイメージを用い、ソースとCargoキャッシュを読み取り専用でマウントし、成果物をDockerボリュームへ分離した。

## 範囲

証明書の生成・署名が対象。TLSやSSHの暗号バックエンドは引き続きringを使うため、全体のringビルド時間がなくなったとは扱わない。既存の証明書・鍵ファイルの再生成は不要。実行速度の比較は今回実施していない。

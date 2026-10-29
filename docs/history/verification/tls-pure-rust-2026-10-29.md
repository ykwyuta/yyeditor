# TLS暗号バックエンドのPure Rust化（2026-10-29）

対象は`3bfb7b1`後の作業ツリー。WindowsとLinux Docker、Rust 1.99.0で検証。CIへのプッシュ・実行はしていない。既存のyytoolbox録画変更は保持した。

## 変更範囲

共通`yy-tls-crypto`を追加し、3270クライアントと模擬ホストで明示的に使う。ルートのrustls既定依存からringを外した。ブラウザーの証明書試験ではPure Rust TLSとの接続を確認し、実WebView2用の試験サーバーにも適用した。

上流RustCryptoの完全SHA `70f76c039e587192688af18a80d5d6435dedaf22`を基にしたローカル修正版を保持する。取り込んだままのalpha版ではない。X25519の非寄与秘密値拒否、EC鍵共有の形式検査、AES-GCMの回数上限、短いレコードと16 KiB超の平文の拒否、通信鍵の抽出、鍵と証明書の一致確認、RSA署名のブラインディング等を補強した。詳細は[現行仕様](../../developer/tls-crypto.md)と[ソース・ライセンス・差分の説明](../../../vendor/rustls-rustcrypto/YYEDITOR-PATCH.md)を参照。

隔離ビルドの依存解決で既存`crypto-bigint 0.7.5`の公開取り下げを検出した。[上流変更履歴](https://github.com/RustCrypto/crypto-bigint/blob/v0.7.10/CHANGELOG.md)は0.7.6の安全性修正と後続のAPI修正を理由として記載している。ルートとyymergeのlockfileを0.7.10、ctutilsを0.4.7へ更新した。TLS部品はRust 1.87以上を明示する。

yydbmanager・YY3Designerの依存には今回のTLS部品やcrypto-bigint経路がなく、これらの独立ワークスペースは変更・検証対象にしていない。YYLibraryの参照は変更していない。

## 結果

| 確認 | Windows | Linux |
| --- | --- | --- |
| ルート `cargo fmt --all -- --check` | 成功 | 成功 |
| ルート全ワークスペース・全ターゲットClippy、locked、警告をエラー化 | 成功 | 成功 |
| 修正したrustls-rustcrypto本体のClippy、locked、警告をエラー化 | 成功 | 成功 |
| TLS・証明書・3270・ブラウザー・SSH関連試験 | 78件成功 | 78件成功 |
| yymerge書式・全ワークスペース・全ターゲットClippy | 成功 | 成功、no-default-features |
| yymerge全ワークスペース試験 | 133件成功 | 129件成功、no-default-features、実SFTP必須 |
| 同じ暗号依存版のCコンパイラーなし隔離ビルド・通信 | 成功 | 成功 |

関連試験のコマンド:

```text
cargo +1.99.0 test -p yy-tls-crypto -p yy-cert -p yy-3270-tls -p yy-3270-mock -p yy-browser -p yy-ssh -p yy-ssh-crypto --features yy-browser/devcert --locked
```

- TLSの216条件: 9スイート、3鍵交換、RSA/P-256/P-384/Ed25519の証明書、Pure Rustとringの両方向およびPure Rust同士。全条件でmTLSと双方向各64 KiBの転送を確認。TLS 1.3では鍵更新、全条件で通信鍵抽出も確認した。
- 暗号レコードの192条件: 公開APIから作れる32-byte鍵で6スイート×4連番×8入力長。ringと暗号文・タグが一致。改変、連番違い、短すぎる入力、上限超過を拒否。AES-128はハンドシェイク内で導出された16-byte鍵で検証した。
- native OpenSSL 3.1.1で独立生成した固定証明書・PKCS#8/PKCS#1/SEC1鍵を使用。署名の相互検証、間違った名前、不一致の秘密鍵、不正DER、RSA-1024の拒否を確認。通常試験はOpenSSLコマンドを必要としない。
- 3270の実TCP試験でTLS 1.2、暗黙TLS、STARTTLS、CA、TOFU・変更された証明書の拒否、クライアント証明書、転送・プリンターを確認。
- `YY_REQUIRE_WEBVIEW2=1`で実WebView2の開発者用証明書・HTTPS試験が1件成功。
- Linuxでは`YY_REQUIRE_OPENSSH=1`のOpenSSH相互運用を実施。yymergeはSFTPサーバーを導入し、`YYMERGE_REQUIRE_SFTP=1`で実行した。

Windowsの初回ソケット試験はサンドボックスの`10013`で失敗したため、権限を得て実接続で再検証した。増分キャッシュのハードリンク失敗からコピーへ切り替える環境警告は出たが、最終Clippyは成功している。検査の除外や広いallowは追加していない。

## ネイティブ依存の確認

`cargo tree -p yyterm --edges normal,build --locked --offline`と模擬ホストの通常依存にはring/AWS-LC/OpenSSLはない。ringはTLS/SSHの比較試験の開発依存に残る。

隔離probeはルートのlockfileを基に暗号部品版をそろえ、yy-tls-cryptoとyy-3270-tlsを通常依存で組み込んだ。新しい成果物フォルダーで`CC/CXX/AR/NASM`を存在しないプログラムに設定し、TLS 1.2と1.3のハンドシェイク・アプリケーションデータ交換がWindows/Linuxとも成功。OSのリンカー・SDKやOS乱数APIまで除去したという意味ではない。

ログは作業用`target/tls-pure-windows-clippy.log`、`tls-pure-windows-tests.log`、`tls-pure-linux-final.log`、`tls-pure-merge-*.log`、`tls-pure-webview-windows.log`、`tls-pure-no-cc-locked-windows.log`、`tls-pure-no-cc-locked-linux.log`に置いた。

## 残る範囲

独立yydbmanagerのPostgreSQL/MySQL/SQL Serverのnative-tls経路、Oracle/Db2の暗号providerは未移行。Linux版のOpenSSL依存はまだ残る。SQLite/DuckDBのC/C++、WebView2本体も別の対象。今回の試験は形式的な暗号監査・FIPS認証ではなく、上流alpha実装に対する独立した安全性監査は未実施。

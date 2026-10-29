# 証明書の生成と署名

`crates/yy-cert`はrcgen向けのECDSA P-256/SHA-256署名アダプター。yybrowserの開発者用証明書、yy-3270-mockの試験用PKI、yy-3270-tlsの試験で共有する。X.509の構築はrcgen、鍵生成・署名・PKCS#8はRustCryptoのp256が担当する。rcgenの`crypto`・`ring`・`aws_lc_rs`は有効にせず、`pem`だけを使う。

`KeyPair::generate()`はOSの暗号学的乱数生成器を使い、失敗を呼び出し側へ返す。`certificate_params()`はSANを設定し、正の20オクテットのランダムなシリアル番号と、公開鍵情報のSHA-256による鍵識別子を設定する。CA・用途・DN・有効期間は利用側が指定する。rcgenの`CertificateParams::new()`を直接使う場合は、暗号バックエンドが自動生成していたシリアル番号等を明示する必要がある。

`PublicKeyData`には非圧縮SEC1公開鍵を渡し、`SigningKey`はSHA-256によるECDSA署名をASN.1 DER形式で返す。秘密鍵の出力は従来と同じPKCS#8のDER/PEM。既存の保存済み証明書・鍵を変換する必要はない。

確認コマンド:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test -p yy-cert -p yy-browser -p yy-3270-tls -p yy-3270-mock --features yy-browser/devcert --locked
```

yy-certの試験は鍵の保存・復元、署名の改変拒否、証明書のSAN・用途・期間・署名を確認する。yy-browserは生成した証明書でTLS 1.2/1.3のハンドシェイクとデータ転送をメモリ上で実行する。3270は実ソケットでTLS・STARTTLS・クライアント証明書・TOFU等を確認するため、ループバック接続を許可した環境が必要。

TLS通信そのものは現在もrustls/ringを利用する。SSHは別の共有部品でRustCryptoを利用する。証明書生成からringを外したことは、ルートの依存全体からringがなくなったことを意味しない。

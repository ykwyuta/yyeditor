# TLSの暗号バックエンド

`crates/yy-tls-crypto::provider()`が共通のRustCryptoバックエンドを返す。3270クライアントと模擬ホストの通常ビルド、およびブラウザーの証明書相互運用試験で明示的に渡す。プロセス全体のデフォルトproviderは変更しない。TLSの状態機械はrustls、証明書チェーン・用途・名前・有効期間の検証はwebpki、3270のTOFUは既存の検証層が担当する。

## 対応と境界

- TLS 1.3のAES-128/256-GCM・ChaCha20-Poly1305、およびTLS 1.2のECDHE-ECDSA/ECDHE-RSAと同じ3暗号の計9スイート。
- X25519、P-256、P-384の鍵交換。X25519の非寄与鍵、ECの不正な点や圧縮形式を拒否する。
- RSA-PSS/PKCS#1のSHA-256/384/512、ECDSA P-256/P-384、Ed25519。RSAは2048〜8192 bit。PKCS#8、RSAのPKCS#1、ECのSEC1秘密鍵を読み、証明書との一致を確認する。
- AES-GCMの1鍵あたりの暗号化回数上限はrustls/ringと同じ`2^24`レコード。短い暗号レコード、認証タグ・連番の改変、TLS 1.2の16 KiB超の平文を拒否する。
- QUIC、FIPS認証、DBドライバへの組み込みはこの部品の現在の範囲に含めない。

通常の3270/yyterm依存からring/AWS-LCを外している。ringは暗号文・署名・接続の比較対象として開発依存に残る。`yydbmanager`は別ワークスペースで、TLSとOpenSSLの移行は未実施。WebView2自体のTLSやOSの証明書ストアをRustへ置き換えたものではない。

## 実装元と保守

`vendor/rustls-rustcrypto`はRustCryptoの完全SHA `70f76c039e587192688af18a80d5d6435dedaf22`を基にしたローカル修正版。取得したままの公開alpha版として扱わない。原ライセンス、原READMEと[変更点](../../vendor/rustls-rustcrypto/YYEDITOR-PATCH.md)を保持する。上流READMEは本番利用を推奨しておらず、本リポジトリの相互運用試験が形式的な安全性監査や認証を意味するものでもない。更新時にはローカルの検証・制限を再確認する。

暗号プリミティブはRustCryptoを使い、独自のAES/RSA/TLSプロトコルは実装しない。RSA-PSS署名にはブラインディングを使用し、秘密鍵のDebug表示を抑止する。OS乱数取得の失敗はエラーにする。

既定の`zeroize`機能はAEAD・HMAC・SHA-2にも転送し、保持する鍵・ハッシュ状態を破棄時に消去する。主要な保持型の`ZeroizeOnDrop`契約はコンパイル時に検査する。コンパイラーの一時コピーやレジスターまで物理的な消去を保証するものではない。rustlsはセッションのDebug表示から秘密情報が漏れる問題の修正版0.23.46以上を要求する。

3270で新しく承認したTOFU証明書は、CertificateVerifyとFinishedを含むハンドシェイク成功後に保存する。署名不正・改変・途中切断時は承認済みでも保存しない。保存失敗は接続エラーとして通知する。

TLS部品の最低Rustは1.87。暗号用`crypto-bigint`は旧版の公開取り下げと修正内容を確認し、ルート・yymergeのlockfileを0.7.10へそろえた。詳細は[上流変更履歴](https://github.com/RustCrypto/crypto-bigint/blob/v0.7.10/CHANGELOG.md)を参照。

## 検証

```powershell
cargo fmt --all -- --check
cargo fmt --manifest-path vendor/rustls-rustcrypto/Cargo.toml -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p rustls-rustcrypto --lib --locked -- -D warnings
cargo test -p yy-tls-crypto -p yy-cert -p yy-3270-tls -p yy-3270-mock -p yy-browser -p yy-ssh -p yy-ssh-crypto --features yy-browser/devcert --locked
```

`interop.rs`はOpenSSLで独立生成した固定証明書を使い、9スイート・3鍵交換・4種の証明書・双方の接続方向とPure Rust同士の接続を組み合わせ、216条件のmTLS・双方向データ・TLS 1.3鍵更新・通信鍵抽出を確認する。各署名方式と鍵形式、名前の不一致・弱いRSA鍵・不正な秘密鍵も検査する。

`boundaries.rs`は公開APIで作れる32-byte鍵を使い、6スイート×4連番×8入力長の192条件で暗号文とタグをringと比較する。AES-128は上記ハンドシェイクで導出される16-byte鍵で検証する。短い入力、改変、最大長、不正鍵共有、暗号化回数上限も確認する。通常試験にOpenSSLコマンドは不要。固定試験鍵の再生成手順は[fixtures](../../crates/yy-tls-crypto/tests/fixtures/README.md)に置く。

`adversarial.rs`は全9スイートの実レコードの各バイト改変・全切り詰め位置・誤連番・TLS 1.2ヘッダー改変、TLS 1.3パディング、非正規署名・低位点・証明書の信頼境界を検査する。3270の`adversarial_tests.rs`は不正署名・最終レコード改変・EOFでTOFU記録を作成・追記しないことを実TCP接続で検証する。評価範囲と残る課題は[敵対的レビュー記録](../history/verification/tls-adversarial-2026-10-29.md)を参照。

実機ではWebView2の証明書・HTTPS試験を必須フラグ付きで実行する。

```powershell
$env:YY_REQUIRE_WEBVIEW2 = '1'
cargo test -p yy-win --locked rules_host_maps_and_dev_certificates_in_webview2
```

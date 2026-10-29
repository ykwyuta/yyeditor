# Pure Rust TLSの敵対的レビューと検証

2026-10-29。基準HEAD `3bfb7b1`に加え、未コミットの`yy-tls-crypto`、ローカルRustCrypto providerと3270・ブラウザーの統合差分を対象とした。利用者の依頼により3エージェントを起動した。

| 担当 | 実施内容 |
| --- | --- |
| `tls_adversarial_review` | 暗号アダプター、鍵共有・署名・AEAD・乱数・鍵の破棄・依存版のコードレビュー |
| `tls_adversarial_tests` | 独立した攻撃入力の試験作成と実行。`tests/adversarial.rs`の6試験 |
| `tls_integration_review` | 信頼判断、TOFU、ネイティブ依存・既知アドバイザリー、実TCP攻撃の再現 |

3者から結果を受領した後、追加作業の途中でエージェントの利用上限に達した。以降の修正・回帰試験と最終検証は親エージェントが担当した。暗号専門機関による監査、FIPS認証、形式検証、継続的ファジングを実施したという意味ではない。

## 確認した問題と対応

| 問題 | 影響・条件 | 対応 |
| --- | --- | --- |
| rustls 0.23.45のセッションDebug表示に秘密情報が含まれる | カスタムセッションストア等がそのDebugをログ出力する場合。現在の製品で該当ログ経路は確認していない | root/helper/vendorの最低版とroot lockを0.23.46へ更新 |
| providerのzeroize機能がAEAD・HMAC・SHA-2に転送されない | オブジェクト破棄後のメモリー取得に対する防御不足。リモート認証突破の実証ではない | feature転送と保持型のコンパイル時契約検査を追加 |
| TOFUがCertificateVerify・Finishedより前に記録される | 未知の証明書を利用者が承認した後、相手が不正署名等で失敗しても記録だけ残り、後続の正しい接続を拒否し得る。移行前から存在 | 成功するまで記録を保留。TLS 1.3不正署名、TLS 1.2最終暗号レコード改変・EOFの6条件で新規作成も既存ファイル追記も行わない回帰試験。保存エラーの通知、成功時の既存保存試験も確認 |

rustlsの根拠は[GHSA-c8m5-c952-6cm4](https://github.com/rustls/rustls/security/advisories/GHSA-c8m5-c952-6cm4)。RSA依存には[Marvinのアドバイザリー](https://rustsec.org/advisories/RUSTSEC-2023-0071.html)があるが、このTLS構成はECDHEとRSA署名を使い、RSA鍵交換の復号を公開しない。署名はブラインディングを使う。これをもって全経路の時間差耐性を証明したとは扱わない。

## 攻撃入力と検査の限界

`crates/yy-tls-crypto/tests/adversarial.rs`は全9スイートの実レコードについて、各バイトの反転、全prefix切り詰め、余剰、誤連番、TLS 1.2の型・バージョン改変を検査する。TLS 1.3のパディング・inner content type、RFC 8032のEd25519既知ベクトル、非正規署名・方式違い・低位X25519点・不正SEC1、失効期間外・未来・EKU違い・非信頼・改変・名前違い・同曲線の別秘密鍵も対象とする。

Ed25519の特定のidentity鍵/R・S=0入力ではringが受理し、本providerの`verify_strict`は拒否した。比較対象が常に正しいという仮定を避け、Pure Rust側が拒否することを試験する。これだけからring全体や証明書認証の突破を結論しない。

コードレビュー担当は別の一時probeで12,000件の不正秘密鍵DERと、14 verifier合計56,000件のランダム公開鍵・署名入力をreleaseで実行し、全拒否・panicなしと報告した。これは担当者の補助実験であり、コミット対象の恒久試験や親による最終再実行の件数には含めない。

残る課題: 3270の`Options.timeout`はソケットの各read待ちに適用され、TLS全体の絶対期限ではない。期限未満で断片を送る相手による接続保持が可能な設計をコードレビューで確認した。今回の暗号移行以前からの問題で、専用の低速相手による再現試験と総期限対応は未実施。RSA-PSSの塩長別のOpenSSL固定負例、網羅的なサイドチャネル測定、外部プロトコル実装による悪性ハンドシェイク検査、cargo-audit全件の実行も今回の完了範囲には含めない。

## 再現

現行の検証コマンドは[TLS開発者文書](../../developer/tls-crypto.md)。Windowsではローカルソケット制限による10013を確認し、実ソケット試験は権限を付けて再実行する。LinuxはRust 1.99のDockerでルート全体の書式・Clippyと対象試験を実施する。独立ワークスペースのTLS依存は今回変更していない。

結果: Windows・Linuxのルート全体の書式・全target Clippy（`--locked -- -D warnings`）、vendor providerのClippyが成功した。TLS・証明書・3270・ブラウザー・SSHの関連試験はWindows 86件（85件の一括実行に追加したTOFU保存失敗試験1件を含む）、Linux 86件成功。Linuxではさらにyytoolboxのコア・CLI 21件が成功し、一括合計107件。OpenSSH必須フラグを付けた実接続試験も成功した。

最終ログ: `target/review-final-clippy-windows.log`、`target/review-provider-clippy-windows.log`、`target/review-final-clippy-linux.log`、`target/review-final-tests-windows.log`、`target/tls-review-tofu-write.log`、`target/review-final-linux.log`。Windowsのインクリメンタルキャッシュでハードリンク失敗からコピーへ切り替える環境警告があるが、Clippyのコード警告はない。関連試験と全ワークスペースのClippyを区別し、ルート全試験を実行済みとはしない。

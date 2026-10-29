# 検討経緯・作業記録

[文書全体](../README.md)。ここは日付・対象コミット・検証環境に依存する記録です。
現行の操作は[利用者向け](../user/README.md)、内部仕様と再現手順は[開発者向け](../developer/README.md)を参照してください。

- [提案書・要求・ロードマップ](proposals/README.md): 旧提案を保存。将来計画と実装途中の説明を含みます。
- [レビュー](reviews/): セキュリティ・UI比較の指摘、画像、調査用コードとログ。調査用コードは製品のビルド対象ではありません。
- [検証記録](verification/): DB別・アプリ別の動作確認、未検証範囲。
- [描画性能の測定](benchmarks/yydraw.md): 日付・条件付きの実測と改善経緯。
- [CI失敗の分析](ci-failure-analysis.md): 過去の原因と対策の根拠。継続的な作業ルールは[AGENTS.md](../../AGENTS.md)。
- [yydbmanagerの検討中の項目](yydbmanager-roadmap.md)。
- [Pure Rust化できる依存関係の調査（2026-10-29）](pure-rust-dependency-audit-2026-10-29.md): 4ワークスペースのネイティブ依存、既存実装への置換、独自アダプターの候補。
- [BLAKE3のPure Rust化検証（2026-10-29）](verification/blake3-pure-2026-10-29.md): バックエンド間のハッシュ互換性とCコンパイラーなしのビルド確認。
- [証明書署名のPure Rust化（2026-10-29）](verification/certificate-pure-rust-2026-10-29.md): rcgenの署名をRustCryptoへ移行し、ブラウザー・3270の証明書を共通化。
- [SSH暗号のPure Rust化（2026-10-29）](verification/ssh-pure-rust-2026-10-29.md): RustCryptoによるパケット暗号、ringとの照合、OpenSSHとの再鍵交換・転送試験。
- [Parquet ZstdのPure Rust化（2026-10-29）](verification/parquet-zstd-pure-rust-2026-10-29.md): 圧縮レベル1〜22の相互運用、Cコンパイラーなしの検証、圧縮率と速度の比較。
- [yytoolboxの録画途中保存・終了キー（2026-10-29）](verification/yytoolbox-recording-stop-2026-10-29.md): 実Esc入力、キーの競合・変更、MP4と音声の途中保存、実マイク未検証の区別。
- [TLS暗号のPure Rust化（2026-10-29）](verification/tls-pure-rust-2026-10-29.md): 3270の通常依存からringを除去、TLS 1.2/1.3・RSA/mTLSの相互運用、Cコンパイラーなしの検証。
- [TLSの敵対的レビュー（2026-10-29）](verification/tls-adversarial-2026-10-29.md): 3エージェントによる攻撃入力・信頼境界の検査と修正、未検証範囲。
- [yytoolbox録画の上下反転修正（2026-10-29）](verification/yytoolbox-recording-orientation-2026-10-29.md): DIBの行順統一、実デスクトップMP4の全30フレーム確認。

- [録画操作バー・容量整理・アプリ起動承認（2026-10-29）](verification/toolbox-filemanager-controls-2026-10-29.md): ボタンによる途中保存とEsc廃止、録画からのバー除外、大容量候補と未署名アプリ承認。

- [yytoolboxの簡易動画編集（2026-10-29）](verification/toolbox-video-edit-2026-10-29.md): 複数範囲のカット・クリップ、実MFによる映像・音声の検証、元動画保護。

記録を追加するときは日時・コミット・環境・成功/失敗/未実行の区別を記載します。
新しい機能の使い方や現行の仕様説明は、この場所だけに追加しないでください。

# Pure Rust化できる依存関係の調査（2026-10-29）

対象コミット: `7ec737683560f5e36b696d9108aade86dacc58ae`。Windows上のRust 1.99.0で、ルート・yymerge・yydbmanager・YY3Designerの4ワークスペースを調査した。依存変更・置換実装・動作試験は行っていない。

## 調査方法と判定基準

各ワークスペースの`cargo metadata --format-version 1 --locked`、Windows/Linuxそれぞれの`cargo tree --locked --offline --target <target> --edges normal,build`、Cargo.toml・build.rs・利用コード・依存クレートの実ソースを確認した。比較先の公開文書は調査日時点のもの。置換候補について、この製品に組み込んだ状態の互換性や速度は未検証。

ここでいうPure Rust化は、第三者のC/C++実装・外部アセンブラー・外部CLIへの依存をRustコードへ移すこと。OSのファイル、ソケット、ウィンドウ、音声などのAPI呼び出しは別に扱う。`windows`、`libc`という名前だけで置換対象とはしない。

`cargo metadata`には他OS・開発用の依存も含まれるため、その一覧をそのまま製品のネイティブ依存件数とはしていない。`links`属性もネイティブコードの証明ではない。例えばrayon-coreには`links`があるが、今回のC/C++置換対象ではない。

## 現在入っている主な依存

下表は通常の依存とビルド依存。ベンチマーク等の開発依存は別記する。WebView2 Loaderはビルド時にCをコンパイルするのではなく、同梱されたネイティブライブラリを利用する。

| ワークスペース | Windows x64 | Linux x64 |
| --- | --- | --- |
| ルート | BLAKE3、ring、WebView2 Loader | BLAKE3、ring |
| yymerge | BLAKE3、ring、WebView2 Loader | BLAKE3、ring |
| yydbmanager | BLAKE3、ring、AWS-LC、SQLite、DuckDB、Zstd、WebView2 Loader | BLAKE3、ring、AWS-LC、SQLite、DuckDB、Zstd、OpenSSL |
| YY3Designer | WebView2 Loader | 上記に該当するネイティブ依存なし |

Windowsの`native-tls`はSchannelを利用する。OpenSSLがWindowsでもビルドされる、という意味ではない。

## 優先して検討できる候補

### 1. BLAKE3: 機能フラグで対応できる

- 現在: `blake3 1.8.7`の既定構成。ルートのyy-files・yy-draw・yy-win・yytoolbox、yymerge、yydbmanagerの共有部品に入る。
- 方法: `features = ["pure"]`を各ワークスペースに反映する。`crates/yy-files/Cargo.toml`には既に`blake3-pure`という入口があるが、既定では有効ではない。
- `pure`はRustのSIMD intrinsicsを利用する。AVX-512等の最適化構成が変わるので、ファイルハッシュの互換性と大きなファイルでの性能を確認する。
- `cc`がビルド依存のツリーに残る場合がある。ツリーから`cc`が消えることと、C/アセンブリを実際にビルドしないことは区別する。
- 規模: 小。最初に着手しやすい。

根拠: [BLAKE3の機能フラグ](https://docs.rs/crate/blake3/1.8.7/features)、取得済み同版の`build.rs`。

### 2. 開発用証明書: rcgenを残して署名だけRustへ移せる

- 現在: `yy-browser -> rcgen 0.14.7 -> ring`。`crates/yy-browser/src/rules.rs`はP-256鍵生成・自己署名証明書作成を行う。3270の試験用証明書にもrcgenを利用する。
- 方法: rcgenの`crypto`・`ring`を外し、`pem`等の必要な形式処理を残す。RustCryptoのP-256鍵に`rcgen::SigningKey`・`PublicKeyData`を実装する小さいアダプターを作る。
- 同版の実ソースで、これらのtraitと`CertificateParams::self_signed`が独自の署名鍵を受け取れることを確認した。X.509のシリアライズ全体を独自実装する必要はない。
- 確認項目: DER署名形式、PKCS#8/PEM保存、SAN・用途・有効期間、生成した証明書での実TLS接続。共有アダプターにまとめ、試験ごとに鍵処理を複製しない。
- 規模: 小〜中。この経路のringを外せても、SSH/TLS経路のringは別途残る。

根拠: [SigningKey](https://docs.rs/rcgen/latest/rcgen/trait.SigningKey.html)、[PublicKeyData](https://docs.rs/rcgen/latest/rcgen/trait.PublicKeyData.html)。公開最新版だけでなく、使用中の0.14.7のソースでも確認した。

### 3. SSH: russhの暗号バックエンドを追加する

- 現在: `yy-ssh -> russh 0.63.1 -> ring`。ルートとyymergeに影響する。
- 方法: russhにRustCryptoバックエンドを追加する。直接のring/AWS-LC利用は主に`cipher/gcm.rs`、`cipher/chacha20poly1305.rs`、`cipher/mod.rs`に集中している。既にRustCryptoのAES・CTR・曲線暗号等も利用しているため、SSH全体を再実装する必要はない。
- 現状は`ring`と`aws-lc-rs`の両方を外すと`compile_error!`になる。フラグを消すだけでは対応できず、バックエンドの追加と条件分岐の変更が必要。
- OpenSSHの`chacha20-poly1305@openssh.com`は通常のIETF ChaCha20-Poly1305と同じAPIで単純に置換できない。パケット長の暗号化、連番、nonce、タグ、再鍵交換まで合わせる。
- 確認項目: OpenSSHとのAES-GCM/ChaCha20接続、破損タグ拒否、鍵交換・再接続、SFTP、ポート転送、LinuxのSSH結合試験。
- 規模: 中。上流PRまたは修正した依存を完全SHAで固定する方式が考えられる。

根拠: 使用中russhの`src/lib.rs`と上記cipher実装、[russh API](https://docs.rs/russh/0.63.1/russh/)。

### 4. ParquetのZstd: Rustコーデックへ移せる

- 現在: `yy-db-core -> parquet 56.2.1 -> zstd -> zstd-safe -> zstd-sys`。`parquet`では`arrow, snap, zstd`を有効化している。
- 方法: `ruzstd`等を使うZstdコーデックへ移す。使用中Parquetの`create_codec`は内部で実装を選ぶため、依存名の差し替えだけでは足りない。Parquet側の限定的な改修または対応版への更新が必要。
- `ruzstd 0.9.0`には展開だけでなく圧縮APIもある。ただし公式文書は圧縮速度・圧縮率・設定範囲の差を明記している。
- 確認項目: 外部で作ったZstd圧縮Parquetの読み込み、出力の相互運用、辞書・ウィンドウサイズ・圧縮レベル、壊れた入力、メモリ使用量。
- 規模: 中。`zstd`機能を削除するだけではZstd圧縮の読み込み能力を失うため、置換完了とはしない。またDuckDB本体に含まれる圧縮実装は別に残る。

根拠: [ruzstd](https://docs.rs/ruzstd/0.9.0/ruzstd/)、使用中Parquetの`src/compression.rs`。

### 5. DBと3270のTLS: 共通のPure Rust暗号基盤を検討できる

現在、接続プロトコル本体は既にRustで実装されているが、TLSの下層にネイティブ実装が残る。

| 経路 | 残っている依存 | 変更点 |
| --- | --- | --- |
| PostgreSQL | postgres-native-tls / native-tls | tokio-postgres-rustls等と共通providerへ移す。接続だけでなくクエリ中止のTLS経路も対象 |
| MySQL | native-tls | mysqlの`rustls`入口と共通providerを利用。`rustls-tls`はAWS-LC、`rustls-tls-ring`はringを選ぶため、それだけではPure Rustにならない |
| SQL Server | tiberius-ngの既定native-tls | `default-features = false`とrustls経路を検討。既定のtds80・Windows統合認証を落とさないよう個別に維持 |
| Oracle | oracledbがrustlsの既定機能を有効化し、AWS-LCが入る | 依存側の機能制御を改修し、共通providerを渡す。Oracleの接続処理全体の再実装は不要 |
| Db2 | rust-db2-driverがrustls/ringを有効化。利用側にもring providerの明示あり | 共有ドライバのTLS機能と`native/mod.rs`のprovider指定の双方を変更 |
| 3270 | yy-3270-tls -> rustls/ring | TLS 1.2、STARTTLS、TOFU、クライアント証明書を維持して共通providerへ移す |

`rustls`へ統一するだけでは、ring/AWS-LCが残る。RustCryptoベースのproviderを選ぶ、または互換性を満たすproviderアダプターを作る工程が必要。

既存候補の制限も確認した。[rustls-rustcrypto](https://github.com/RustCrypto/rustls-rustcrypto)は公開READMEで本番利用を推奨していない。[rustls-pure](https://docs.rs/rustls-pure/latest/rustls_pure/)はTLS 1.3のみでRSA証明書検証に対応しない。現在のDB・3270のTLS 1.2/RSA対応を保つ置換として、そのまま採用できるとは判断しない。独自実装では既存の暗号プリミティブを組み合わせる部分に範囲を絞り、TLSや暗号方式そのものを一から書かない。

規模: 中〜大。TLS 1.2/1.3、RSA/ECDSAの証明書チェーン、CA指定、ホスト名検証、mTLS、各DBの実接続が受入条件。暗号providerの相互運用・実装検証と、DBドライバの検証を分けて進める。

## 外部プログラムを減らせる候補

| 対象 | 現状 | Rust化の方法・注意点 |
| --- | --- | --- |
| Git操作 | `crates/yy-git/src/runner.rs`とyymergeのgit_toolがGit CLIを起動 | [gix/gitoxide](https://github.com/GitoxideLabs/gitoxide)でオブジェクト・履歴・status/diff等から段階的に実装。commit/push/checkout/パッチ適用、Git設定、属性、改行、認証ヘルパー、リモート側Gitまであるため、一括置換の規模は大きい。gixの機能選択・推移依存も再監査する |
| Docker/Podman容量探索 | `crates/yy-win/src/fm/cleanup.rs`がCLIを呼ぶ | [Docker Engine API](https://docs.docker.com/reference/api/engine/)・[Podman API](https://docs.podman.io/en/latest/_static/api.html)のRustクライアントへ移す。Windows named pipe、Unix socket、context、Podman machine/SSH接続を対象にする。デーモンへの依存は残るがCLIは外せる。探索機能は読取専用を維持 |
| GNU向けアイコン埋め込み | `apps/yyeditor/build.rs`がGNUターゲットでwindresを呼ぶ | RustでCOFFリソースオブジェクトを生成する。既にRustで生成している`.res`を生かせる。通常のMSVCビルドではwindresを使わないため、そのCI短縮には直結しない |

## 大きな効果が見込めるが、互換性を別に調べる候補

### SQLite

`rusqlite 0.40.2 -> libsqlite3-sys 0.38.2`のbundled SQLiteはC実装。Rust再実装の[Turso](https://github.com/tursodatabase/turso)が候補になるが、[互換表](https://github.com/tursodatabase/turso/blob/main/COMPAT.md)に未対応・部分対応があり、任意のSQLiteファイルとSQLを扱うyydbmanagerの単純な差し替えとは判断しない。

さらにTursoも「エンジン本体がRust」と「選んだ依存の全体がPure Rust」は別である。調査時点のmainの[core/Cargo.toml](https://github.com/tursodatabase/turso/blob/main/core/Cargo.toml)は既定の`simd`でsimsimdを使い、暗号用aegisには`pure-rust-crypto`の制御がある。[Rust binding](https://github.com/tursodatabase/turso/blob/main/bindings/rust/Cargo.toml)の既定にはmimallocも含まれる。採用する公開版を固定し、これらを含む推移依存を検証する必要がある。

確認項目は既存ファイルの読み書き・WAL・ロック・トランザクション・PRAGMA・型情報・EXPLAIN等。規模は大きい。

### DuckDB

`duckdb 1.4.1 -> libduckdb-sys 1.4.1`のbundledエンジンはC++実装で、Windows CIがVS 2022固定となる理由でもある。ただしこれはDb2のようなサーバー接続用ドライバではなく、SQL実行と`.duckdb`ファイルの読み書きを行うエンジンそのもの。

別のRust製分析エンジンを選んでも、既存DuckDBファイル・SQL方言・EXPLAIN・プロファイリング・スキーマ関数の互換性は得られない。同等機能を維持するにはエンジンと形式の再実装または十分な互換層が必要で、今回の候補中では特に大きい。外部DuckDBプロセスへ逃がすだけならネイティブビルドを分離できるが、Pure Rust化には数えない。

### ベンチマークのalloca

`criterion 0.8 -> alloca 0.4 -> cc`が開発用の依存にある。`criterion/src/routine.rs`ではスタック配置をずらして測定バイアスを抑えるために利用している。通常の製品依存ツリーには入らない。

Rustによるスタック配置の実装を上流へ追加する、または同等の測定条件を保つベンチマーク基盤へ移す余地はある。ヒープへ移すだけでは元の測定上の目的が変わる。製品の配布物より、開発・ベンチマーク環境の改善候補として扱う。

## 既にRustであるもの・対象を混同しやすいもの

- `serde_yaml_ng -> unsafe-libyaml 0.2.11`はLibYAMLをRustへ移植したコード。CライブラリへのFFIという意味ではない。メモリ安全性を改善する目的の別パーサーへの変更は、今回のC依存解消とは分ける。
- yymergeのPDF処理は`hayro 0.8.0`、画像は`image`のPNG/JPEG/BMP/WebPを選択しており、今回調べた通常依存経路で新たなPDFium/ImageMagick等は見つからなかった。
- flate2/miniz_oxide、lzma-rs、PNG/JPEG/WebP処理、BiomeのJavaScript整形は、今回調べた構成では追加のC/C++置換対象にならない。
- yytoolboxの録画はWindows Media Foundation/WASAPIを利用する。FFmpeg/ImageMagick依存はない。これらのOS標準機能を完全に除くならH.264/AACエンコーダー等まで別途必要で、現在の録画機能への小さい置換ではない。
- WebView2のRust bindingは同梱Loader静的ライブラリをリンクするが、C++の大規模ビルドは行っていない。Loader部分の独自化とChromiumエンジン自体のRust化は別の規模の話である。ブラウザー機能を保ったままWebView2全体を小さく置換できる候補とはしない。
- GnuCOBOL/x3270/OpenSSH/Python/tmux/screen等のCI用外部プログラムには、相互運用の比較対象や試験環境として必要なものもある。製品のRust化だけを理由に試験から削除しない。

## 推奨順と完了条件

1. BLAKE3のpure化と証明書署名アダプター。
2. SSHのRustCryptoバックエンドとParquetのZstdバックエンド。
3. DB/3270のTLS基盤の統一・Pure Rust providerの互換性検証。
4. Docker/Podman API、Gitの読取処理からの段階的な内蔵化。
5. SQLite互換エンジンの評価。DuckDB互換エンジンは独立した大きな開発として扱う。

変更したワークスペースごとに書式・全対象Clippy・回帰試験を実施し、共有部品の利用側、Windows/Linux、機能フラグの差も確認する。C/C++コンパイラーを利用できない隔離ビルドで、実際にネイティブコンパイルが発生しないことを確認する。OS ABI・リンカーまで不要になったとは解釈しない。

依存数が減ることとCI全体が速くなることも別である。ルートのリリース設定にはfat LTO・codegen-units=1があり、Rust側の最適化・リンク時間も残る。ビルド時間と実行性能は変更前後を同条件で測定して判断する。

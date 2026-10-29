# Parquet ZstdのPure Rust化と全圧縮レベルの検証（2026-10-29）

変更前コミット: `7ec737683560f5e36b696d9108aade86dacc58ae`。先行するBLAKE3・証明書・SSHの未コミット変更に追加した。Windows x64 / Linux x64、Rust 1.99.0で実施。現在の変更は未プッシュでCIは未実行。

## 採用した構成

当初調べたruzstd 0.9.1はFastest以外の圧縮モードが未実装だった。圧縮レベルを制限しないという要求に合わせ、全レベルの探索戦略を実装する[zstandard 0.1.9](https://docs.rs/zstandard/0.1.9/zstandard/)を取得し、実ソースと相互運用を検証して採用した。このクレートの通常依存はなく、独自のZstdアルゴリズムを新しく作る必要はなかった。

`apps/yydbmanager/vendor/parquet`に56.2.1のソースとApacheライセンス・NOTICEを保持し、Cargo.tomlとcompression.rsだけを変更した。`zstd-rust`機能と`yy-parquet-zstd`アダプターを追加。既存の1〜22をそのまま渡し、別のレベルへの読み替えはしない。既定の書き出し設定は維持した。zstandardのMITライセンスと参照したZstdのBSD表示も同梱通知へ追加した。

アプリのCargo metadataとlockfileから、`zstd`・`zstd-safe`・`zstd-sys`がなくなったことを確認した。Parquetの有効機能はArrow、Snappy、`zstd-rust`。yydbmanager以外の3ワークスペースにはParquetの利用がなく、今回のコード変更はyydbmanagerに限定した。

## 結果

- Windows: 独立ワークスペースのfmt・全対象Clippy（`--workspace --all-targets --locked -- -D warnings`）と通常試験67件が成功。
- Linux: 同じfmt・全対象Clippy・通常試験57件が成功。差はWindows専用UI試験10件。
- 比較用の独立ワークスペースもfmtと全対象Clippyが成功。アプリのワークスペースにネイティブ参照実装を開発依存として追加していない。
- CSV、日本語、疑似乱数バイナリ、長距離の繰り返し、空入力を使い、全22レベルでPure Rust出力をlibzstd 1.5.7が読み、libzstdの出力をPure Rustが読めることを確認。空入力を含む4種×22レベルを両方向で検証した。
- 元のParquet 56.2.1＋libzstdで作った22個のファイルを修正版で読めることを確認。Pure Rust出力44条件（22レベル×WriterVersion 1/2）は元のネイティブZstd版Parquetで読めた。2,048行、複数行グループ、辞書エンコーディング、NULL、日本語、高精度小数、バイナリを比較した。
- チェックサム付きの22参照フレーム、128 KiBのブロック境界、連結・skippable frame、改変・切断・余分な末尾、展開後サイズの過小・過大、無効レベルを回帰試験で確認。失敗時に既存の出力バッファが変わらないことも検証した。
- Windows/Linuxの新規ビルド先で`CC`・`CXX`・`AR`を存在しないパスにし、製品と同じ機能のParquet・Arrow・Zstdをビルドして、レベル22で書き込み・読み込みが成功した。標準ランタイム・リンカー・OS ABIまでなくしたという意味ではない。

全対象Clippyの終了コードは0。ローカルソースになった上流Parquetの同期Arrow構成には未使用のキャッシュAPI等の警告8件があり、検証ログにも残る。今回の変更を通すためのallow・検査除外は追加していない。Windowsでは増分キャッシュのハードリンクが使えずコピーする環境警告も出た。

最初の相互試験ではDuckDBがParquet拡張をユーザーディレクトリへ自動取得し、並列取得が競合した。取得された4ファイルは作成時刻と完全パスを確認して削除した。最終版ではDuckDBの拡張を使う試験を置き換え、参照生成を独立ツールへ分離し、通常試験は固定ファイルだけを使う。外部DB接続を必要とする既存のignored試験は対象外で、接続成功とは扱っていない。

## 圧縮率・速度の簡易比較

Windows 11 / Ryzen 7 5700X、Rust 1.99.0のrelease構成。ほかのビルド終了後、メモリ上の決定的な1,400行・132,667バイトの日本語CSVで測定した。初回をウォームアップとし、順序を交互にして各実装5回を採取した中央値。Pure Rustはアダプター、C版はlibzstd 1.5.7のbulk圧縮。両方とも既知の入力長・チェックサムなし。

| レベル | Rust出力バイト | C出力バイト | Rust圧縮ms | C圧縮ms |
| --- | ---: | ---: | ---: | ---: |
| 1 | 14,610 | 14,610 | 0.237 | 0.200 |
| 3 | 16,756 | 16,756 | 0.498 | 0.301 |
| 9 | 14,155 | 15,551 | 2.348 | 2.523 |
| 15 | 14,325 | 14,325 | 25.330 | 22.394 |
| 19 | 11,527 | 11,361 | 100.099 | 107.096 |
| 22 | 11,527 | 11,361 | 101.251 | 106.502 |

この入力ではレベル22のRust出力はC版より約1.5%大きく、圧縮時間は約5%短かった。低レベルではRust側が遅い場合もあり、同じ数字でも圧縮率・速度・出力バイト一致を一般保証するものではない。C版でも、この短い入力では高いレベルが常に小さくなるわけではない。アプリ全体・CI時間・他データ・展開速度の測定結果とは区別する。

生データは`target/zstd-benchmark.json`、相互検証は`target/zstd-compatibility.json`。通常試験・Clippyのログは`target/zstd-db-tests-windows.log`、`zstd-db-clippy-windows.log`、`zstd-db-linux.log`、Cコンパイラーなしの結果は`zstd-pure-windows.log`・`zstd-pure-linux.log`。再現用の参照ツールと固定ファイルはリポジトリに保持した。

## 残るネイティブ実装

DuckDB本体と内部のZstd、SQLite、TLSの暗号基盤等は残る。今回外したのはRust Parquet経由のネイティブZstdビルド。zstandardの宣言MSRVは1.96で、今回実行したツールチェーンは1.99.0。現変更のリリース・CI・MSRVでの試験は未実行。

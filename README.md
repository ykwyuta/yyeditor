# yyeditor

Rust で実装する、Windows 向けの軽量テキストエディタです。数 GB のファイルを即座に開ける巨大ファイル対応、日本語の幅広い文字コード、区切り文字（CSV/TSV）編集などを目指しています。

設計は [docs/proposal/](docs/proposal/README.md) の提案書を参照してください。

## 現在の状態

ロードマップ（[08 章](docs/proposal/08-roadmap-testing.md)）の **M0（基盤）と M1（巨大ファイルビューア）** を実装済みです。現時点では閲覧専用で、編集は M2 以降で対応します。

| 機能 | 状態 |
|------|------|
| ファイルを開く（メニュー / Ctrl+O / ドラッグ＆ドロップ / コマンドライン引数） | ✅ |
| メモリマップ＋永続ピースツリーによる巨大ファイル表示 | ✅ |
| 行数のバックグラウンドカウント（進捗をステータスバーに表示） | ✅ |
| 行番号表示（未確定範囲は推定値を薄く表示） | ✅ |
| スクロール（キー・ホイール・スクロールバー。巨大ファイルはバイト位置比例） | ✅ |
| 行へ移動（Ctrl+G） | ✅ |
| 長大行（改行のない数 GB の行など）の分割表示 | ✅ |
| 不正な UTF-8 バイト（`\xNN`）・制御文字の可視化、UTF-8 BOM の判別 | ✅ |
| 拡大・縮小（Ctrl+ホイール / Ctrl++ / Ctrl+-） | ✅ |
| 編集・Undo・保存、文字コード変換、検索、CSV など | M2 以降 |

### 性能（M1 の完了条件の確認）

`tools/gen-bigfile` の `open-bench` による計測値（Linux、ページキャッシュに載った状態、4 コア）:

| 項目 | 10 GB のログ（1.57 億行） | 2 GB・改行なしの 1 行 |
|------|-----------------------|---------------------|
| ファイルを開いて最初の画面を用意 | 98 ms | 7 ms |
| 行数未確定のまま中央へジャンプ | 0.06 ms | 1.7 ms |
| 全行数のカウント（バックグラウンド） | 0.62 秒 | 0.08 秒 |
| 行番号での移動（カウント後） | 0.14 ms | — |

## プロジェクト構成

```
crates/
  yy-buffer/   永続ピースツリー（スナップショット、行 ⇔ オフセット変換）
  yy-jobs/     バックグラウンドジョブ（進捗・キャンセル）
  yy-config/   設定ファイル（%APPDATA%\yyeditor\config.toml）
  yy-io/       メモリマップによるファイルオープン（書き込み共有を拒否）
  yy-core/     文書モデル、改行数のバックグラウンドカウント
  yy-layout/   表示行の分割（長大行のセグメント化）、スクロール位置
  yy-win/      Win32 + Direct2D / DirectWrite の UI（Windows のみ）
apps/yyeditor/ 実行ファイル（マニフェスト埋め込み）
tools/gen-bigfile/  巨大テストファイル生成（gen-bigfile）と性能計測（open-bench）
```

`yy-win` 以外は OS に依存しないため、Linux でもテストできます。

## ビルド

Windows（MSVC）:

```sh
cargo build --release -p yyeditor
target\release\yyeditor.exe [開くファイル]
```

テスト・静的解析:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
# Linux から Windows UI 層の型検査のみ行う場合
rustup target add x86_64-pc-windows-msvc
cargo check -p yy-win -p yyeditor --target x86_64-pc-windows-msvc
```

巨大ファイルでの性能確認:

```sh
cargo run --release -p gen-bigfile --bin gen-bigfile -- big.log 10G log   # log|japanese|long|single|csv
cargo run --release -p gen-bigfile --bin open-bench -- big.log
```

描画の確認・不具合調査用に、ファイルの先頭 1 画面を画面外に描画して BMP に保存できます:

```sh
yyeditor.exe --render-bmp input.txt out.bmp
```

## 設定

`%APPDATA%\yyeditor\config.toml`（任意）。書いた項目だけが既定値を上書きします。

```toml
[editor]
font_family = "BIZ UDゴシック"   # 見つからない場合は Consolas → BIZ UDゴシック → MS Gothic の順に代替
font_size = 11.0                 # ポイント
tab_width = 4

[view]
line_numbers = true
max_row_bytes = 8192             # 長大行を分割して表示する単位（バイト）
wheel_lines = 3

[colors]
background = "#FFFFFF"
foreground = "#1E1E1E"
```

## 既知の制限（M1 時点）

- 文字コードは UTF-8 として表示します（Shift_JIS 等は M3 で対応。現在は不正バイトとして `\xNN` 表示）。
- 描画には `ID2D1HwndRenderTarget` を使っています。提案書 07 章のスワップチェーン＋デバイスコンテキスト方式への移行は IME のインライン描画と合わせて M2 で行います。

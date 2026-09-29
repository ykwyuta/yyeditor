# yyeditor

Rust で実装する、Windows 向けの軽量テキストエディタです。数 GB のファイルを即座に開ける巨大ファイル対応、日本語の幅広い文字コード、区切り文字（CSV/TSV）編集などを目指しています。

設計は [docs/proposal/](docs/proposal/README.md) の提案書を参照してください。

## 現在の状態

ロードマップ（[08 章](docs/proposal/08-roadmap-testing.md)）の **M0（基盤）・M1（巨大ファイルビューア）・M2（エディタ基本）・M2.5（矩形選択・マルチカーソル）** を実装済みです。

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
| 文字入力・削除・改行（自動インデント）、上書きモード（Insert） | ✅ |
| 日本語入力（IME の変換中文字列をインライン表示） | ✅ |
| カーソル移動（文字＝書記素単位・単語・行頭／行末・スマートホーム・ページ） | ✅ |
| 選択（Shift＋移動・マウスドラッグ・ダブルクリックで単語・すべて選択） | ✅ |
| マルチカーソル（Ctrl+クリック / Ctrl+Alt+↑↓）と全カーソル同時編集、行ごとの貼り付け振り分け | ✅ |
| 切り取り・コピー・貼り付け（改行コードを文書に合わせる） | ✅ |
| Undo / Redo（連続入力を 1 単位にまとめる。保存後も元に戻せる） | ✅ |
| 新規作成・上書き保存・名前を付けて保存（UTF-8、BOM と改行コードは元のまま） | ✅ |
| 未保存の変更の確認（終了・開く・新規作成時）、タイトルの `*` 表示 | ✅ |
| 矩形選択（Alt+ドラッグ / Alt+Shift+矢印 / 矩形選択モード）。全角・タブを考慮した表示桁で範囲を決める | ✅ |
| 矩形への入力・削除（行末より右は空白で埋める）、矩形のコピー・切り取り・貼り付け | ✅ |
| 矩形データのクリップボード形式（Visual Studio・EmEditor と共通の `MSDEVColumnSelect`） | ✅ |
| 次の出現箇所を選択（Ctrl+D）、すべての出現箇所を選択（Ctrl+Shift+L）、各行末にカーソル（Alt+Shift+I）、矩形をカーソルに変換 | ✅ |
| IME の変換中文字列を全カーソル位置にプレビュー | ✅ |
| 文字コード変換（M3）、検索・置換（M4）、CSV（M5） | 今後 |

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
  yy-io/       メモリマップによるファイルオープン（書き込み共有を拒否）、一時ファイル経由の保存
  yy-core/     文書モデル、選択（マルチカーソル）、一括編集、Undo/Redo、カーソル移動、行数カウント
  yy-layout/   表示行の分割（長大行のセグメント化）、表示テキスト ⇔ オフセット変換、スクロール位置
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
ambiguous_wide = true            # ①・○ などの曖昧幅文字を全角（2 桁）として数える（矩形選択の桁）

[view]
line_numbers = true
max_row_bytes = 8192             # 長大行を分割して表示する単位（バイト）
wheel_lines = 3

[colors]
background = "#FFFFFF"
foreground = "#1E1E1E"
```

## 保存の仕組み

保存は、保存先と同じフォルダの一時ファイルに書き出して永続化してから置き換えます（途中で失敗しても元のファイルは残ります）。
開いているファイル自体に上書き保存する場合、元のファイルは隠しファイル名（`.名前.yyorig-…`）に退避され、Undo 履歴が参照しなくなった時点（通常はファイルを閉じたとき）で削除されます。そのため保存後も Undo で保存前の内容に戻せます。

## 既知の制限（M2.5 時点）

- 矩形選択の桁は、半角 1・全角 2 の等幅を前提に数えます。全角文字が半角のちょうど 2 倍の幅でないフォント（Consolas と日本語フォントの組み合わせなど）では、矩形が見た目上わずかにずれます。MS ゴシックや BIZ UDゴシックなど、全角・半角の幅がそろったフォントの利用をおすすめします。
- 矩形選択で一度に編集できるのは 100 万行までです（超える場合はメッセージを表示します）。
- 矩形内の連番挿入・大文字小文字変換・並べ替え・検索、Ctrl+K Ctrl+D（出現箇所のスキップ）は未実装です。

- 文字コードは UTF-8 として扱います（Shift_JIS 等は M3 で対応。現在は不正バイトとして `\xNN` 表示し、保存時は元のバイトのまま書き戻します）。
- 保存は UI スレッドで行うため、数 GB のファイルの保存中は応答しません（バックグラウンド保存は今後対応）。
- 別のアプリが書き込み中のファイル（出力中のログなど）は開けません。
- 描画には `ID2D1HwndRenderTarget` を使っています（M2 の時点で IME のインライン表示も含め問題がないため、提案書 07 章のスワップチェーン方式への移行は必要になった時点で行います）。

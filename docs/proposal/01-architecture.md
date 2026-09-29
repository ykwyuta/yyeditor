# 01. 全体アーキテクチャとプロジェクト構成

## 1. 基本方針

- **コア（OS 非依存）と UI（Windows 固有）を分離する。**
  テキストバッファ、文字コード、検索、CSV 解析、レイアウト計算は純粋な Rust ライブラリとし、`cargo test` を Linux / Windows の両方で実行できるようにする。
- **UI スレッドをブロックしない。** ファイルサイズに比例する処理（行数カウント、文字コード変換、検索、全置換、保存、CSV 列幅計算、バックアップ）はすべてバックグラウンドで実行し、進捗表示とキャンセルを可能にする。
- **ファイルサイズに比例するメモリを確保しない。** 元ファイルは mmap で参照し、編集差分と小さな集約情報だけをメモリに持つ。
- **軽量さ。** 外部ランタイム（.NET、WebView2、VC++ 再頒布パッケージ等）に依存しない単一 exe とする。

## 2. 技術選定

### 2.1 GUI 方式の比較

| 方式 | 利点 | 欠点 | 評価 |
|------|------|------|------|
| **Win32 + Direct2D/DirectWrite（`windows` クレート）** | 最軽量・最速起動。IME（IMM32/TSF）、DPI、ファイルダイアログ、D&D を完全制御。独自テキストビューを自由に実装できる | 実装量が多い。Windows 専用 | **採用** |
| egui / iced 等（即時/宣言的 GUI） | 実装が速い。クロスプラットフォーム | 日本語 IME のインライン変換が不完全になりがち。独自の仮想スクロールビューはどのみち自作が必要。GPU 初期化で起動が遅い場合がある | 不採用 |
| Tauri / WebView2 | UI 実装が容易 | WebView2 ランタイム依存で「軽量」に反する。数 GB のテキスト表示には結局ネイティブ側の仮想化が必要 | 不採用 |
| Slint / gtk-rs 等 | ネイティブ風 UI | テキストエディタ用コントロールの自由度・IME 対応に懸念 | 不採用 |

エディタの価値の大半は「独自テキストビュー」にあり、どの GUI フレームワークを選んでもそこは自作になります。
それならば周辺（メニュー、ダイアログ、ステータスバー）も Win32 標準コントロールで作るのが最も軽量かつ Windows ユーザーにとって自然です。

### 2.2 主要依存クレート

| 用途 | クレート | 備考 |
|------|---------|------|
| Win32 / D2D / DirectWrite / COM | `windows` | Microsoft 公式バインディング |
| メモリマップ | `memmap2` | Windows では `CreateFileMappingW` を使用 |
| 高速バイト検索 | `memchr` | 改行カウント・区切り文字探索（SIMD） |
| 文字コード | `encoding_rs`, `chardetng` | 自前テーブルで拡張（[03-encoding](03-encoding.md)） |
| 正規表現 | `regex`, `regex-automata`, `fancy-regex` | [05-search-replace](05-search-replace.md) |
| Unicode | `unicode-width`, `unicode-segmentation` | 東アジア文字幅・書記素クラスタ |
| 並行処理 | `crossbeam-channel`, `parking_lot`, `rayon`（限定使用） | |
| 設定 | `serde`, `toml` | `%APPDATA%\yyeditor\config.toml` |
| エラー / ログ | `thiserror`, `anyhow`, `tracing` | |
| テスト | `proptest`, `criterion`, `tempfile` | |

`unsafe` は `yy-win`（FFI）と `yy-io`（mmap）に限定し、コアクレートは `#![forbid(unsafe_code)]` とします。

## 3. プロジェクト構成（Cargo ワークスペース）

```
yyeditor/
├─ Cargo.toml                 # [workspace] 定義、共通 profile
├─ rust-toolchain.toml        # stable 固定
├─ crates/
│  ├─ yy-buffer/              # 永続ピースツリー、スナップショット、行/オフセット変換
│  │  └─ src/{lib.rs, tree.rs, piece.rs, snapshot.rs, cursor.rs, metrics.rs}
│  ├─ yy-encoding/            # コーデック trait、各文字コード実装、自動判別
│  │  ├─ src/{lib.rs, detect.rs, registry.rs, codecs/...}
│  │  └─ tables/              # 生成済みマッピングテーブル（build 時に埋め込み）
│  ├─ yy-io/                  # mmap、ファイルオープン、変換ローダー、保存、ロック、一時ファイル
│  │  └─ src/{lib.rs, source.rs, loader.rs, saver.rs, tempdir.rs, lock.rs}
│  ├─ yy-core/                # Document（バッファ＋メタ情報）、マルチカーソル・矩形選択、一括編集、Undo/Redo
│  │  └─ src/{lib.rs, document.rs, selection.rs, rect.rs, edit.rs, history.rs, commands/...}
│  ├─ yy-search/              # 正規表現検索・置換（チャンク検索、全置換ストリーミング）
│  ├─ yy-delimited/           # 区切り文字/CSV 解析、レコードインデックス、列幅計算、列操作
│  ├─ yy-syntax/              # シンタックスハイライト（定義ファイル、状態機械、状態チェックポイント）
│  │  └─ syntax/              # 組み込みのハイライト定義（TOML、exe に埋め込み）
│  ├─ yy-layout/              # 表示用レイアウト計算（折り返し、表示列、仮想行）。描画 API 非依存
│  ├─ yy-config/              # 設定、キーマップ、ファイルタイプ（拡張子→モード/区切り文字/ハイライト定義）、カラーテーマ
│  ├─ yy-jobs/                # バックグラウンドジョブ基盤（進捗、キャンセルトークン）
│  └─ yy-win/                 # Win32 アプリ層：ウィンドウ、描画、IME、メニュー、ダイアログ
│     └─ src/{app.rs, frame.rs, editor_view.rs, render/, ime.rs, dialogs/, statusbar.rs}
├─ apps/
│  └─ yyeditor/               # バイナリクレート（main.rs、リソース .rc、manifest、アイコン）
│     ├─ build.rs             # embed-resource で manifest（PerMonitorV2 DPI, longPathAware, UTF-8）埋め込み
│     └─ res/
├─ tools/
│  ├─ gen-tables/             # Unicode/JIS/IBM の対応表からコーデックテーブルを生成
│  └─ gen-bigfile/            # 性能試験用の巨大ファイル生成
├─ tests/
│  ├─ fixtures/               # 各文字コード・CSV のテストデータ
│  └─ integration/
├─ benches/
├─ docs/
│  └─ proposal/               # 本提案書
└─ .github/workflows/ci.yml   # windows-latest でビルド＋全テスト、ubuntu でコアテスト
```

### 3.1 クレート依存関係

```
                 apps/yyeditor
                       │
                    yy-win ─────────────┐
                       │                │
      ┌────────┬───────┼────────┬───────┴──┐
  yy-layout yy-search yy-delimited yy-config yy-syntax
      └────────┴───────┼────────┴──────────┘
                    yy-core
                  ┌────┴─────┐
               yy-io      yy-jobs
             ┌───┴────┐
        yy-buffer  yy-encoding
```

- 下位クレートは上位を知らない。`yy-win` 以外は Windows API に依存しない（`yy-io` のファイル差し替えのみ `cfg(windows)` で `ReplaceFileW` を使い、他 OS では `rename` にフォールバック）。
- `yy-layout` は「どの行のどの範囲を、どの x 座標に描くか」までを計算し、実際のグリフ描画は `yy-win` の DirectWrite レンダラが行う。文字幅計測は trait で注入するため、テストではダミー計測器を使える。

### 3.2 ビルド設定

```toml
# Cargo.toml（抜粋）
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
strip = true
panic = "unwind"   # パニック時にも Drop（一時ファイルの削除など）が走るよう unwind を維持
```

- ターゲット: `x86_64-pc-windows-msvc`（主）、`aarch64-pc-windows-msvc`（副）。
- CRT は静的リンク（`-C target-feature=+crt-static`）し、再頒布パッケージ不要にする。
- マニフェストで `PerMonitorV2` DPI、`longPathAware`、Common Controls v6、`activeCodePage=UTF-8` を宣言。

## 4. 実行時アーキテクチャ

### 4.1 レイヤー

```
┌───────────────────────────────────────────────────────────┐
│ yy-win  : Window / Menu / Dialog / EditorView / IME / D2D  │  UI スレッド
├───────────────────────────────────────────────────────────┤
│ Command dispatcher（キー入力 → Command → Document 操作）      │  UI スレッド
├───────────────────────────────────────────────────────────┤
│ Document = { Snapshot(root), History, Selections,          │
│              Encoding, LineEnding, Mode(Text|Delimited) }  │
├───────────────────────────────────────────────────────────┤
│ Background Jobs: Indexer / Transcoder / Search / Replace / │  ワーカースレッド
│                  Save / Backup / CSV Scanner / ColumnWidth │
├───────────────────────────────────────────────────────────┤
│ Buffer Sources: OriginalMmap / TranscodedMmap / AddBuffer  │
└───────────────────────────────────────────────────────────┘
```

### 4.2 スレッドモデル

- **UI スレッド**（1 本）: Win32 メッセージループ。`Document` の唯一の書き手。
- **ジョブワーカー**（CPU コア数に応じたプール）: `yy-jobs` が管理。各ジョブは開始時に `Document` の **スナップショット（`Arc` のルート）** を受け取り、それを読むだけなので UI スレッドの編集とロック競合しない。
- **結果の通知**: ワーカーは `crossbeam-channel` に結果を送り、`PostMessageW(hwnd, WM_APP_JOB, ...)` で UI スレッドを起こす。UI スレッドは受け取った結果が「現在のスナップショット世代」に対して有効かを検証してから反映する（古い結果は破棄、または差分補正）。
- **キャンセル**: 各ジョブは `CancelToken`（`Arc<AtomicBool>`）を持ち、チャンク処理ごとに確認する。

```rust
pub trait Job: Send + 'static {
    type Output: Send;
    fn run(self, ctx: &JobContext) -> Result<Self::Output, JobError>;
}

pub struct JobContext {
    pub cancel: CancelToken,
    pub progress: ProgressSink,   // 0..=total バイト、UI は 100ms 間隔で反映
}
```

### 4.3 ドキュメントとスナップショット

```rust
pub struct Document {
    snapshot: Snapshot,            // 現在の内容（永続ピースツリーのルート）
    history: History,              // Undo/Redo（スナップショットのスタック）
    selections: SelectionSet,      // マルチカーソル / 矩形選択（09 章）
    source: SourceInfo,            // パス、元の文字コード、BOM、改行コード、ファイル識別子
    mode: EditMode,                // Text | Delimited(DelimitedConfig)
    generation: u64,               // 編集ごとに増加。ジョブ結果の有効性判定に使う
    save_point: Option<u64>,       // 保存時点の generation（変更フラグ判定）
}
```

スナップショットを中心に据えることで、以下がすべて同じ仕組みで実現できます。

| 機能 | スナップショットの使い方 |
|------|------------------------|
| Undo/Redo | 編集前のスナップショットを保存するだけ |
| バックグラウンド保存 | 保存開始時点のスナップショットを書き出す（保存中も編集可能） |
| 検索・全置換 | 検索開始時点のスナップショットを走査 |
| CSV 再解析 | 解析対象スナップショットと generation を紐付け |

### 4.4 設定とファイルタイプ

`config.toml` で拡張子ごとのモードを定義します。

```toml
[filetype.csv]
extensions = ["csv"]
mode = "delimited"
delimiter = ","
quote = '"'
rfc4180 = true
header = "auto"

[filetype.rust]
extensions = ["rs"]
syntax = "rust"          # ハイライト定義（10 章）

[filetype.tsv]
extensions = ["tsv", "tab"]
mode = "delimited"
delimiter = "\t"
rfc4180 = false

[filetype.mainframe]
extensions = ["ebc"]
encoding = "IBM-930"
record_length = 80     # 改行のない固定長レコード
```

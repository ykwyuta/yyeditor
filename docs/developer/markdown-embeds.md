# Markdown埋め込みの実装と検証

利用説明は[Markdown埋め込み](../user/markdown-embeds.md)。対象ワークスペースはルートです。
yymerge・yydbmanager・YY3Designerの独立ワークスペースはyy-preview／yy-winを利用していません。

## 境界

- `yy-preview::transclusion::expand` が `pulldown-cmark` のWiki画像イベントだけを展開します。描画と同じ構文オプションを使い、コード・HTML・メタデータ中の文字列を走査置換しません。
- ルートは元文書の保存フォルダと専用snippetsフォルダ。候補は正規化して範囲を確認し、再帰探索ではシンボリックリンクを除外し、ディレクトリの同一性・範囲も再確認します。
- 参照内容は1回の展開内でキャッシュします。現在のノートは編集中のスナップショット、それ以外は保存済みUTF-8を使います。
- ファイル＋フラグメントのスタックで循環を検出します。深さ16、個数256、読み込み／出力各8 MiB、探索20000項目／深さ32で負荷を制限します。
- 元の行対応を保持し、`markdown_with_source_lines` に渡してスクロール同期を維持します。
- 相対画像・通常リンクを参照元を基準に解決します。プレビューのsnippets資源は専用のパス接頭辞を持ち、`DocBase::Embedded` が2つの許可フォルダを分けて扱います。資源要求は一度だけURLデコードし、既存の正規化・読み込み制限・取消を使います。
- 展開保存はUTF-8の別ファイルへの原子的置換です。未解決の展開や参照元の上書きを拒否し、アクティブ文書を保存・変更しません。画像本体はコピーせず、存在する許可範囲内のローカルリンクをfile URLへ変換します。
- ローカルMarkdownの右側プレビューが対象です。ライブ編集の段落表示とSSH文書は対象外です。

## 検証

変更前にルートの `.github/workflows/ci.yml` と [AGENTS.md](../../AGENTS.md) を確認します。

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test -p yy-preview --locked
cargo test -p yy-win --locked expanded_save_preserves_sources_and_refuses_incomplete_exports
$env:YY_REQUIRE_WEBVIEW2='1'
cargo test -p yy-win --locked preview::tests:: -- --nocapture
cargo build -p yyeditor --locked
```

Linux固有のシンボリックリンク試験は `cargo test -p yy-preview --locked` のUnix構成で実行します。
WebView2試験は既存の直列化を使用し、一時フォルダ・専用画面を破棄します。表示済み本文・見出し・画像の読み込み完了を待ち、単なるウィンドウ生成や固定時間経過を成功条件にしません。

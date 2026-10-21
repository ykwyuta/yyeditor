# yymerge

YYシリーズの比較・マージツール。独立したCargoワークスペースと専用CIを持ちます。Windows GUIは共通のエディター、UDEV Gothic、Markdownヘルプを再利用します。

現在は最初の実装段階です。厳密な2方向テキスト比較、祖先付き3方向マージ、競合の選択・手動編集・結果保存、CLIのJSON / Unified出力、読み取り専用フォルダー比較を提供します。WinMerge / Beyond Compare / Araxis Merge相当の全機能を実装済みという意味ではありません。

```powershell
cargo build --manifest-path apps/yymerge/Cargo.toml --release --locked
apps/yymerge/target/release/yymergew.exe
apps/yymerge/target/release/yymerge.exe diff demo/yymerge/left.txt demo/yymerge/right.txt
```

GUI不要の場合:

```sh
cargo build --manifest-path apps/yymerge/Cargo.toml -p yymerge --no-default-features --bin yymerge --release --locked
cargo test --manifest-path apps/yymerge/Cargo.toml --workspace --locked
```

操作・CLI・終了コード・保存方式・文字コード・上限は[ヘルプ](help/help.md)、長期計画は[提案書](../../docs/proposal/23-merge.md)を参照してください。デモは[demo/yymerge](../../demo/yymerge/README.md)にあります。

未実装: 同期スクロールと挿入空白行による整列、行内色分け、ドラッグ選択、GUIフォルダー比較、3方向フォルダー、同期計画、Git専用連携、比較規則、構造化データ、画像・バイナリー、セッション保存、巨大入力のディスク索引。これらを含む提案書のP0全体は未完了です。

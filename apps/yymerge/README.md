# yymerge

YYシリーズの比較・マージツール。独立したCargoワークスペースと専用CIを持ちます。Windows GUIは共通のエディター、UDEV Gothic、Markdownヘルプを再利用します。

現在は最初の実装段階です。厳密な2方向テキスト比較、祖先付き3方向マージ、競合の選択・手動編集・結果保存、差分行の仮想整列・4ペインの祖先基準整列と同期スクロール・行内強調表示、CLIのJSON / Unified出力、読み取り専用フォルダー比較、確認付きの同期実行・記録からの復元を提供します。WinMerge / Beyond Compare / Araxis Merge相当の全機能を実装済みという意味ではありません。

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

GUIの[ファイル] → [フォルダー比較を開く]で、子フォルダーを含めて読み取り専用で比較できます。結果は2,000件ずつ表示し、両側にあるファイルをダブルクリックまたはEnterで別ウィンドウのファイル比較に開きます。既存のファイル比較や未保存のマージ結果は保持します。詳細は[ヘルプ](help/help.md#folders)を参照してください。

[3方向フォルダー比較を開く]ではBASEを指定し、左右の変更・追加・削除・同じ変更・削除対変更などを区別します。3側とも通常ファイルなら祖先付きのファイルマージへ進めます。CLIは`folders LEFT RIGHT --base BASE --recursive`です。フォルダーの同期・書き換えは行いません。

[同期計画]メニューから追加・更新、ミラー、祖先付き統合の操作候補を一覧化し、JSONへ保存できます。CLIは`sync plan --left LEFT --right RIGHT --mode update-right`です。計画の作成・保存では入力フォルダーを変更しません。要確認項目のない計画は[計画を確認して実行]、CLIの`sync apply PLAN --execute --run-dir NEW_DIRECTORY`で適用できます。置換・削除前の元ファイルを退避し、`sync rollback RUN_DIRECTORY --execute`またはGUIから復元できます。中断した同期は[実行記録から同期を再開]、または`sync resume RUN_DIRECTORY --execute`で入力・退避状態を再検証して続けられます。復元を始めた記録は再開できません。退避先は全入力の外、左右と同じファイルシステムが必須です。変更検出時は停止し、実行後のユーザー編集は復元時に上書きしません。詳しい制限と手順はヘルプを参照してください。

未実装: Git専用連携、比較規則、構造化データ、画像・バイナリー、セッション保存、巨大入力のディスク索引。これらを含む提案書のP0全体は未完了です。

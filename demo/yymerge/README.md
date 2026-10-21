# yymerge 動作確認

リポジトリーのルートから実行します。Dockerや外部サーバーは不要です。

```powershell
cargo build --manifest-path apps/yymerge/Cargo.toml --locked
apps/yymerge/target/debug/yymerge.exe diff demo/yymerge/left.txt demo/yymerge/right.txt
apps/yymerge/target/debug/yymerge.exe gui --base demo/yymerge/base.txt --left demo/yymerge/left.txt --right demo/yymerge/right.txt --output demo/yymerge/result.txt --wait
```

serverとtimeoutの変更は自動統合され、themeだけが競合します。右側の競合を選んで[マージ] → [LEFTを採用]または[RIGHTを採用]を選び、[保存]で結果を書きます。元ファイルは変わりません。生成したresult.txtは動作確認後に削除できます。

CLIで競合付き出力を試す場合（終了コード1が正しい結果です）:

```powershell
apps/yymerge/target/debug/yymerge.exe merge --base demo/yymerge/base.txt --left demo/yymerge/left.txt --right demo/yymerge/right.txt --output demo/yymerge/result.txt --conflict-markers
```

`--conflict-markers`なしでは競合があるため出力しません。既存のresult.txtを置換するときだけ`--overwrite`を加えてください。

フォルダー比較は、GUIの[ファイル] → [フォルダー比較を開く]で`demo/yymerge/folders/left`と`demo/yymerge/folders/right`を選択します。`settings.txt`は相違、`nested/共通.txt`は一致、左右それぞれの専用ファイルは片側のみと表示されます。`settings.txt`をダブルクリックすると、別ウィンドウで左右のテキストを比較できます。フォルダーの内容は変更しません。

```powershell
apps/yymerge/target/debug/yymerge.exe folders demo/yymerge/folders/left demo/yymerge/folders/right --recursive --format json
```

3方向では[ファイル] → [3方向フォルダー比較を開く]で`folders/base`、`folders/left`、`folders/right`をこの順に選択します。`settings.txt`は「両側変更」で、開くとtimeoutの競合を解決できます。`削除と変更.txt`は左で削除・右で変更した衝突として表示します。`nested/共通.txt`は祖先から変更なしです。

```powershell
apps/yymerge/target/debug/yymerge.exe folders demo/yymerge/folders/left demo/yymerge/folders/right --base demo/yymerge/folders/base --recursive --format json
```

同期計画はフォルダー比較の後に[同期計画]メニューから開きます。2方向では追加・更新またはミラーを選択し、作成・置換・削除候補を確認できます。3方向では[祖先付き: 左右の統合計画]を選びます。`settings.txt`は内容マージ待ち、`削除と変更.txt`は要確認として残ります。JSON保存も入力フォルダーの外を指定します。コピーや削除は実行しません。

```powershell
apps/yymerge/target/debug/yymerge.exe sync plan --left demo/yymerge/folders/left --right demo/yymerge/folders/right --mode update-right
apps/yymerge/target/debug/yymerge.exe sync plan --left demo/yymerge/folders/left --right demo/yymerge/folders/right --base demo/yymerge/folders/base --mode merge
```

前者は計画作成成功の0、後者は要確認を含む1を返します。`sync apply`は未実装です。


同期実行・復元を試すときは、この配下の入力を作業用フォルダーへコピーしてから使ってください。リポジトリー内のデモ原本へ直接適用しないでください。

```powershell
# C:/work/yymerge-demo に demo/yymerge/folders のコピーを作成した場合
yymerge sync plan --left C:/work/yymerge-demo/left --right C:/work/yymerge-demo/right --mode mirror-right --output C:/work/plan.json
yymerge sync apply C:/work/plan.json
yymerge sync apply C:/work/plan.json --execute --run-dir C:/work/yymerge-run-001
# 停止した実行を続ける場合（入力・退避データが変わっていないこと）
yymerge sync resume C:/work/yymerge-run-001 --execute
yymerge sync rollback C:/work/yymerge-run-001 --execute
```

`run-dir`は存在しない新規フォルダーで、入力の外・同じファイルシステムに指定します。祖先付き統合のデモには未解決項目があるため、そのままでは実行できません。GUIでも[計画を確認して実行]と[実行記録から復元]を利用できます。


Git連携を試す場合は、作業用Gitリポジトリーを用意し、[Git連携ヘルプ](../../apps/yymerge/help/help.md#git-tool)に沿って`git-tool config`の設定例を登録します。普段使うリポジトリーやglobal設定を書き換える必要はありません。`git-tool config --headless`ならGUIを使わず、解決できる3方向の変更だけを反映し、競合は失敗として残します。自動検証は`apps/yymerge/app/tests/git_tools.rs`の一時リポジトリーで行います。


比較規則は、次のように原文が異なる2ファイルで確認できます。

```powershell
yymerge diff before.txt after.txt --ignore-case --ignore-trailing-space --ignore-eol --format json
yymerge gui --left before.txt --right after.txt --ignore-case --ignore-trailing-space --ignore-eol --wait
```

例えば左を`Alpha`＋行末空白＋CRLF、右を`alpha`＋LFとすると、原本を変えず「規則上は同値」となり、無視した元行の対も確認できます。[厳密比較に戻す]で差分が再び表示されます。パッチ・自動マージ・同期へ無視規則は適用しません。

同じ規則を再利用する場合はJSONへ保存できます。GUIでも[比較規則]から保存・読み込みできます。

```powershell
yymerge rules-profile save review.json --ignore-case --ignore-trailing-space --ignore-eol
yymerge rules-profile show review.json
yymerge diff before.txt after.txt --rules-profile review.json --format json
yymerge gui --left before.txt --right after.txt --rules-profile review.json --wait
```

個別の無視フラグとの併用はできません。既存JSONの置換には`--overwrite`を指定します。新しいウィンドウに自動適用される設定ではなく、明示指定した規則を使います。

ブラウザーで共有・印刷する比較結果は、単一HTMLで保存できます。

```powershell
yymerge diff before.txt after.txt --rules-profile review.json --format html --report review.html
```

GUIは[ファイル] → [2方向比較をHTMLレポートに保存…]を使います。左右の原文と行番号、改行種別、適用規則を表示し、青の「≈」は規則で無視した差分です。全文とフルパスを含みます。上限32 MiBを超える場合はエラーになり、既存のHTMLは保持します。

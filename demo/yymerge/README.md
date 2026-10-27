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


Git連携を試す場合は、作業用Gitリポジトリーを用意し、[Git連携ヘルプ](../../apps/yymerge/help/help.md#git-tool)に沿って`git-tool config`の設定例を登録します。普段使うリポジトリーやglobal設定を書き換える必要はありません。`git-tool config --headless`ならGUIを使わず、解決できる3方向の変更だけを反映し、競合は失敗として残します。


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

3方向マージを途中保存する場合は、GUIの[ファイル] → [マージ作業セッションを保存…]を使います。[開く…]で未解決の競合・採用状態・手動編集を復元できます。Undo履歴と以前のRESULT保存先は復元しません。

```powershell
yymerge session create --base demo/yymerge/base.txt --left demo/yymerge/left.txt --right demo/yymerge/right.txt --output work.yymerge-session.json
yymerge session inspect work.yymerge-session.json
yymerge gui --session work.yymerge-session.json --wait
```

`session create`の0は作業ファイルの作成成功で、マージ結果の保存ではありません。入力を変更・削除してから`inspect`すると2になり、その作業ファイルからのGUI復元も停止します。保存時の入力を保持してください。作業セッションだけを保存してもRESULTは未保存として扱われます。

フォルダー結果を絞り込む場合は、GUIの一覧上部でパスの部分一致と状態を指定します。CLIでも同じ条件を利用できます。

```powershell
yymerge folders LEFT RIGHT --recursive --path-contains .csv --status different --format json
yymerge folders LEFT RIGHT --base BASE --recursive --status review --format json
```

有効な条件があるJSONは`entries`と全体／表示件数を持つオブジェクトです。表示が0件でも全体に差異・未比較があれば終了コードは1・2になります。同期計画はこの表示条件で絞らず、全入力から作ります。パスは文字そのものの部分一致で、`*.csv`のglobは使いません。

本文検索はファイル比較・マージ画面でCtrl+Fを押します。対象ペインを選び、文字列を入力してEnter、F3／Shift+F3で前後へ移動してください。正規表現・大小区別・単語単位を指定できます。RESULTを編集中に検索した場合も、文書が変わる前の検索結果で新しい選択範囲を上書きしません。

3方向マージではCtrl+HでRESULTの置換欄を開けます。一致を検索して[選択を置換]、またはRESULT全体への[全置換]を使います。置換後は入力どおりで、`$1`や`\n`を展開しません。Ctrl+Zで1操作をまとめて戻せます。置換しても未解決の競合は残るため、内容を確認して[手動編集の完了を確定]とRESULT保存を行ってください。

2方向比較の再開は、[ファイル] → [2方向比較の作業セッションを保存…]／[開く…]を使います。左右の選択範囲・対象ペイン・差分位置・比較規則を残し、入力の生バイト指紋を検証して再比較します。

```powershell
yymerge comparison-session create --left demo/yymerge/left.txt --right demo/yymerge/right.txt --ignore-case --output compare.json
yymerge comparison-session inspect compare.json
yymerge gui --comparison-session compare.json --wait
```

作業ファイルは本文を含まず、元ファイルが変更・欠落すると再開を停止します。CLIの作成・検証成功は0で、左右が同じことを意味しません。3方向マージ用の`session`／`--session`とは別形式です。

フォルダー比較は[ファイル] → [フォルダー比較の作業セッションを保存…]／[開く…]で設定を再利用できます。

```powershell
yymerge folder-session create --left LEFT --right RIGHT --base BASE --recursive --path-contains .csv --output folders.json
yymerge folder-session inspect folders.json
yymerge gui --folder-session folders.json --wait
```

保存先は全比較対象の外に指定します。再開時は現在の内容を再走査し、表示条件と選択項目を復元します。過去の比較結果・同期計画・実行記録は復元せず、背後の未保存RESULTは保持します。CLIの`create`／`inspect`は設定の保存・検証だけです。


## Excel・PowerPoint（読み取り専用）

`office/left.xlsx`と`office/right.xlsx`は数量・書式・非表示シートの値とシート追加、`office/left.pptx`と`office/right.pptx`は文章・ノート・画像・スライド順が異なります。Excel／PowerPoint本体は不要です。2方向比較から選ぶか、次を実行してください。

```powershell
apps/yymerge/target/debug/yymerge.exe gui --left demo/yymerge/office/left.xlsx --right demo/yymerge/office/right.xlsx --wait
apps/yymerge/target/debug/yymerge.exe diff demo/yymerge/office/left.pptx demo/yymerge/office/right.pptx --format html --report slides.html
```

差異ありの終了コードは1です。見た目の再現ではなく、セル・スライドの文章と内部データを比較します。原本の編集・マージは行いません。例はこのリポジトリー用に作成したものです。比較の実行にPythonは不要です。


## 画像比較

`images/left.png`と`images/right.png`は、四角の色・円の位置・赤い小さい領域が異なります。背景には透明部分があります。

```powershell
apps/yymerge/target/debug/yymerge.exe gui --image --left demo/yymerge/images/left.png --right demo/yymerge/images/right.png --wait
apps/yymerge/target/debug/yymerge.exe image-diff demo/yymerge/images/left.png demo/yymerge/images/right.png --diff-image difference.png --report images.json
```

差異ありは終了コード1です。`left.webp`・`right.webp`はPNGと同じ画素の可逆WebPです。`sample.jpg`・`sample.bmp`・`sample-lossy.webp`は別形式の読み込み確認用です。`oriented.jpg`はEXIFの向きを持つ2×3画像で、比較時には3×2になります。`animated.png`・`animated.webp`・`sixteen-bit.png`は未対応形式の拒否確認用で、通常の比較作例ではありません。画像比較は原本を書き換えず、出力も原本と異なる場所を指定してください。


## PDF比較

`pdf/left.pdf`と`pdf/right.pdf`を通常の2方向比較で開くと、外部ツールなしでページごとの見た目を比較できます。CLIは`yymerge pdf-diff demo/yymerge/pdf/left.pdf demo/yymerge/pdf/right.pdf --format html --report pdf.html`で全ページを出力します。差異ありは終了コード1です。`pdf/later.pdf`は先頭が同じで2ページ目だけ異なる確認用です。`pdf/password.pdf`（パスワード`yymerge-demo`）はパスワード入力未対応の拒否確認用です。yymergeの実行時にPythonは不要です。

作例の再生成と必要なPythonライブラリは[開発者向け](../../docs/developer/yymerge.md)を参照してください。

# yymerge

YYシリーズの比較・マージツール。独立したCargoワークスペースと専用CIを持ちます。Windows GUIは共通のエディター、UDEV Gothic、Markdownヘルプを再利用します。

現在は最初の実装段階です。厳密な2方向テキスト比較、祖先付き3方向マージ、競合の選択・手動編集・結果保存、差分行の仮想整列・4ペインの祖先基準整列と同期スクロール・行内強調表示、CLIのJSON / Unified / HTML出力、読み取り専用フォルダー比較、確認付きの同期実行・記録からの復元を提供します。WinMerge / Beyond Compare / Araxis Merge相当の全機能を実装済みという意味ではありません。

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

未実装: 高度な比較規則、構造化データ、汎用バイナリー、巨大入力のディスク索引。これらを含む提案書のP0全体は未完了です。

`git-tool diff`／`git-tool merge`をGitの外部ツールとして登録できます。`git-tool config`は登録用設定例を表示するだけで、自動でGit設定を変更しません。GUIは終了まで待機し、保存・解決済みの結果だけをMERGEDへ反映します。`--headless`も用意します。祖先なし・削除・リンク・submoduleのマージ競合は未対応として返します。登録・解除手順と終了コードは[ヘルプ](help/help.md#git-tool)を参照してください。

通常の2方向比較は[比較規則]メニューとCLIの`--ignore-space`／`--ignore-trailing-space`／`--ignore-case`／`--ignore-eol`に対応します。元データの一致と規則上の同値を区別し、無視した行対も原文のまま確認できます。マージ・同期・Git専用画面は厳密比較を継続し、無視フラグ付きunifiedパッチは生成しません。詳細は[比較規則のヘルプ](help/help.md#comparison-rules)を参照してください。

規則は[比較規則]メニューからJSONプロファイルへ保存・読み込みできます。CLIでは`rules-profile save review.json --ignore-case`で作成し、`diff LEFT RIGHT --rules-profile review.json`や`gui --rules-profile review.json`で再利用します。設定は明示指定したときだけ適用します。形式と手順は[プロファイルのヘルプ](help/help.md#comparison-profiles)を参照してください。

[ファイル] → [2方向比較をHTMLレポートに保存…]またはCLIの`diff LEFT RIGHT --format html --report review.html`で、原文・行番号・改行種別・適用規則を含む単一HTMLを保存できます。GUIはバックグラウンドで生成し、保存前の入力検証と32 MiBの出力上限を設けています。詳しくは[HTMLレポートのヘルプ](help/help.md#html-report)を参照してください。

通常の3方向マージは[ファイル] → [マージ作業セッションを保存…]／[開く…]で途中保存・再開できます。未解決の競合と手動編集中のRESULTを残し、再開時は入力の指紋を検証します。CLIは`session create`／`session inspect`／`gui --session FILE`に対応します。RESULTの保存とUndo履歴の復元は別扱いです。[作業セッションのヘルプ](help/help.md#merge-session)を参照してください。

2方向比較も[ファイル] → [2方向比較の作業セッションを保存…]／[開く…]で再開できます。左右の入力パス・指紋・文字コード、比較規則、両ペインの選択と対象ペイン・差分位置を保存します。CLIは`comparison-session create`／`inspect`／`gui --comparison-session FILE`です。ファイル本文を内包せず、変更・欠落した入力では再開を止めます。[2方向作業セッションのヘルプ](help/help.md#comparison-session)を参照してください。

フォルダー比較の一覧は、パスの部分一致と状態で絞り込めます。CLIは`folders LEFT RIGHT --path-contains .csv --status different`です。絞り込みは表示専用で、全体の差異・未比較判定と同期計画は変わりません。[フォルダー絞り込みのヘルプ](help/help.md#folder-filter)を参照してください。

フォルダー比較の設定は[ファイル] → [フォルダー比較の作業セッションを保存…]／[開く…]で再利用できます。2方向・BASE付き3方向、再帰の有無、表示フィルター、選択項目を保存します。再開時には最新の内容を再走査し、過去の比較結果や同期計画は復元しません。CLIは`folder-session create`／`inspect`／`gui --folder-session FILE`です。[フォルダー作業セッションのヘルプ](help/help.md#folder-session)を参照してください。

ファイル画面ではCtrl+Fで検索バーを開き、LEFT / RIGHT / BASE / RESULTを指定して検索できます。正規表現・大小文字の区別・単語単位を選べ、F3／Shift+F3で前後へ移動します。既存のYY検索エンジンをバックグラウンドで実行し、編集・選択・比較対象が変わった場合は古い検索結果を破棄します。[本文検索のヘルプ](help/help.md#pane-search)を参照してください。

3方向マージではCtrl+HでRESULT専用の単一置換・全置換を使えます。1操作を1回のUndoで戻せ、変更は手動編集・未保存として扱います。出力上限・取消・編集中の変更を検査してから反映します。既定は置換後の文字列をそのまま使用し、[正規表現]と[置換構文]を有効にすると番号・名前付きキャプチャ、改行・タブ、Unicodeの大小文字変換を使えます。展開もチャンク単位で容量・中止を検査し、不正な参照や計算中の条件変更では反映しません。[RESULT置換のヘルプ](help/help.md#result-replace)を参照してください。

フォルダー比較の表示パスは部分一致のほか、ワイルドカードでも絞れます。GUIの方式欄で切り替え、CLIでは`folders LEFT RIGHT --recursive --path-glob "*.csv"`を指定します。`*`は区切りを含む任意の文字列、`?`は1文字です。条件はフォルダー作業セッションにも保存されます。表示だけの条件で、比較全体の終了コード・未比較件数・同期対象は変わりません。

Excel（`.xlsx` / `.xlsm`）・PowerPoint（`.pptx` / `.pptm`）を2方向の読み取り専用比較で開けます。Excelはシート・セル値・数式、PowerPointはスライド順・図形の文章・ノートを抽出し、書式等の内部XMLと画像等の指紋も比較します。GUI・CLI・HTMLレポート・比較作業セッションに対応します。Office本体は不要です。この内容比較では画面描画・数式再計算・マージ・旧形式（`.xls` / `.xlsb` / `.ppt`）・暗号化ファイルは未対応です。[範囲と制限](help/help.md#office-compare)を参照してください。

PNG・JPEG・BMP・WebPの読み取り専用画像比較にも対応します。[ファイル] → [画像・PDF・Officeの見た目比較を開く…]で、左右・差分を並べ、重ね合わせ・点滅・連動ズーム／移動・色差閾値・整数位置合わせを利用できます。CLIは`image-diff LEFT RIGHT --threshold 8 --diff-image difference.png --report result.json`です。差分PNG・HTML・JSONを出力でき、原本は変更しません。[形式・規則・上限](help/help.md#image-compare)を参照してください。

Excel・PowerPointの見た目もページ単位で比較できます。別途LibreOfficeを導入し、上記の見た目比較メニューまたは`gui --image`から開きます。PageUp / PageDownで印刷ページ・スライドを切り替えます。CLIは`image-diff before.pptx after.pptx --page 2 --diff-image slide-2.png`です。通常の内容比較は引き続き外部ツール不要です。[準備・制限](help/help.md#office-image-compare)を参照してください。

PDFの画像比較は外部ツール不要です。通常の[開く]でPDFを選び、PageUp / PageDownでページを移動できます。CLIの`pdf-diff before.pdf after.pdf`と通常の`diff`は全ページを比較し、JSON・テキスト・全ページ入りHTMLを出力します。選択ページのPNG出力は`image-diff --page 2 --diff-image page-2.png`を使います。[PDFの比較範囲と上限](help/help.md#pdf-compare)を参照してください。

左右別の文字コードは GUIの「比較規則 → 入力の文字コード…」または `diff --left-encoding UTF-8 --right-encoding CP932 --ignore-encoding` で指定できます。パック十進数などの非テキスト部分を含む読み取り専用比較に対応し、不正バイト・制御文字を可視化します。数値への自動解釈は行いません。固定長EBCDICの指定・制約・比較作業ファイルについてはヘルプの「文字コードと制限」を参照してください。

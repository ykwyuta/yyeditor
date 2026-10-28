# yytoolbox

Windows向けのローカル開発ツール集です。正規表現、Base64、ハッシュ、JWT、JSON／SQL整形、
パスワード・ダミーCSV生成、複利・返済、和暦・年齢・学年・時差を扱います。
QRコード、URL変換、XML/YAML/CSV/INIの相互変換、HTML/CSS/JavaScript整形、画面録画、一括画像リサイズにも対応します。
録画はWindows標準API、画像処理はRustライブラリを使い、FFmpeg・ImageMagickには依存しません。

[利用ガイド](../../docs/user/yytoolbox.md) / [同梱ヘルプ](help/help.md) / [ダミーCSV定義例](../../demo/yytoolbox/README.md)

ルートのCargoワークスペースに含まれます。ビルドは`cargo build -p yytoolbox --locked`、
試験は`cargo test -p yytoolbox --locked`。計算・変換ライブラリはOS非依存、GUIはWindows専用です。

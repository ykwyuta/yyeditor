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

# テキストバッファ・I/O・ジョブの内部仕様

## バッファとスナップショット

[yy-buffer](../../crates/yy-buffer/src/lib.rs)は元ファイルや追記領域を参照するPieceを永続B+木へ並べます。
[Piece](../../crates/yy-buffer/src/piece.rs)がソースと範囲、[tree](../../crates/yy-buffer/src/tree.rs)が
Snapshot・Slice・Summary・行位置検索を扱います。編集で全文を複製せず、参照を共有したスナップショットを
検索や保存側へ渡せる構成です。UTF-8境界・改行数の扱いはPieceと木の試験を回帰条件とします。

[yy-core](../../crates/yy-core/src/lib.rs)は文書・選択・編集・Undo/Redoをまとめます。
[yy-layout](../../crates/yy-layout/src/lib.rs)は表示位置・折り返し等を担当し、描画APIの呼び出しと文書編集を分離します。
文字コード本体は[YYLibrary](https://github.com/ykwyuta/YYLibrary)にあります。

## ファイルと保存

[yy-io](../../crates/yy-io/src/lib.rs)は読み取りマップとファイルハンドルをMmapSourceで保持します。
UTF-8以外の変換、保存用一時ファイル、置換、古いマップの寿命を管理します。
マップ参照を使う処理が残っている間はソースを破棄せず、元ファイルの退避が必要な場合は
参照がなくなった時点で片付けます。上書き保存のためのパス変更と作業ファイルのunlinkを区別します。

編集済みの内容は一時ファイルへ書き出してから置き換えます。エラー・取消で既存ファイルを
失わないこと、別文字コードに保存するときの変換不能範囲の扱いを検証します。
変換の不正バイト・非正規符号・EBCDICシフト状態の契約はYYLibraryのRustdocを参照してください。

## ジョブと完了通知

[yy-jobs](../../crates/yy-jobs/src/lib.rs)はワーカースレッド、CancelToken、Progress、完了通知を提供します。
CancelTokenはチャンク処理ごとに確認する協調的な取消です。Progressのdone/totalの単位は
各ジョブで定義し、通常はバイト数を使います。大きさに比例する検索・変換・行数集計・保存をUI処理から分けます。

UIの検証ではスレッド開始やウィンドウの有無を完了とみなさず、結果モデル・表示状態を期限付きで待ちます。
詳細な作業ルールは[AGENTS.md](../../AGENTS.md)、実行コマンドは[開発ガイド](guide.ja.md)を参照します。

# yyfilemanagerのフォルダサイズ・WSL検証

2026-10-30、基点`35b06c62af82ec9bedf46b8d77875b5717c94caf`への未コミットの変更。利用説明は[フォルダサイズ・WSL](../../user/yyfilemanager-folders-wsl.md)、内部仕様は[サイズ・WSLの実装](../../developer/filemanager-size-wsl.md)を参照。

## 内容

詳細表示の名前の隣に合計サイズと占有率バーを追加。一覧の列挙中から表示し、別のワーカーで子孫を集計する。途中値・キャッシュ・読めなかった部分を区別し、サイズ順のソート、履歴移動、変更通知後の再計算に対応。メモリ内キャッシュ・結果キューを制限し、通常の名前順では集計結果ごとに全件ソートしない。

登録済みのWSLをPCとツリーへ追加し、UNC経由で既存のフォルダ操作を利用する。Linuxの大文字小文字を区別。ネイティブのプロパティ画面でオーナー・グループ・rwx・特殊ビット・8進数を指定し、sudoが必要なパスワードはWindows資格情報マネージャーへ保存・削除する。

`/mnt`は除外しない。利用者の指示により、WSLプロバイダーが拒否した箇所をWindowsパスへ読み替える処理は追加しない。未取得・不完全なサイズと割合を許容する。

## 成功した検証

- Windows、Rust 1.99.0：ルートの`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`。
- Linux、Docker上のstable Rust 1.99.1：ルートの書式・全ターゲットlocked Clippy、yy-filesの90試験。実アーカイブ等の2件は既定どおり未実行。
- Windows：yy-filesの88試験成功、既定の2件は未実行。階層・隠しファイル・空フォルダ、リンク・取消、除外・走査途中の消失、不正確な結果の表現、WSLパス・モードの入力を確認。
- Windowsのファイル管理UI：11試験成功、環境依存3件は通常実行では未実行。640/1100pxで名前・合計サイズ・占有率が収まること、実GDIでバーの塗られる部分と背景、割合・サイズ順、履歴、古い世代の排除、子孫変更後の更新を確認。所有者画面は文字幅・コントロールの収まり、チェックと8進数の連動、パスワードのマスクを検査。
- 環境依存のうち、実Ubuntu WSLとWindows資格情報マネージャーの2試験を別途実行し成功。試験専用の一時データでWindowsからの作成・読み書き・コピー・リネーム・削除、WSLのモード・所有者変更、差し替えられたinodeの拒否、リンクの拒否、資格情報の保存・再読込・削除を確認。
- `/mnt`の試験は、Windowsに作った37バイトの専用一時データへWSL UNC経由でアクセス。この環境のプロバイダーはアクセス拒否を返した。その事実をエラーとして保持し、Windowsパスへのフォールバックがないことを確認。読める環境では同じ試験が37バイトを検査する。
- `cargo build -p yyfilemanager --locked`に成功。実行ファイルは`target/debug/yyfilemanager.exe`。

ログ：`target/fm-size-wsl-clippy-windows.log`、`target/fm-size-wsl-linux.log`、`target/fm-size-wsl-files-tests.log`、`target/fm-size-wsl-native-tests.log`、`target/fm-wsl-real-tests.log`、`target/fm-size-wsl-build.log`。

## 範囲・環境

実sudoの特権変更試験は、その試験だけが所有する一時ファイルに対しrootユーザーで行った。利用者の実パスワードを取得・使用しておらず、GUIでの既定ユーザーのパスワード認証の全ポリシーまでは検証していない。製品は読み取った既定ユーザーを固定してsudoを実行し、オーナー入力を`wsl --user root`へ読み替えない。

全ドライブやWSLの`/mnt`全体の容量測定、共有先ごとの変更通知の対応、すべてのWSLディストリビューションのcoreutils構成は未検証。変更通知がない場所はF5で更新する。実Docker CLIの容量整理試験はこの変更では実行していない。

yymerge・yydbmanager・YY3Designerはyy-files/yy-winを参照しておらず、独立ワークスペースは変更・検証対象外。Windowsのincrementalキャッシュのハードリンク警告はコピーへのフォールバックによる環境警告で、Clippyは成功した。Dockerが停止していたためLinux検証用に起動した。コミット・プッシュ・リリースは未実施。

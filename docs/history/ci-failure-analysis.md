# CI失敗の分析と再発防止

> 検討・作業記録です。現在の操作・内部仕様は[文書案内](../README.md)を参照してください。

調査日: 2026-10-23。2026-10-19〜23の通常CI・yymerge・yydbmanagerから、失敗した13実行のログと関係する修正を確認した。以下は確認した範囲の分類であり、全期間の失敗を網羅するものではない。ルートの[AGENTS.md](../../AGENTS.md)を今後の作業ルールとする。

## 確認した失敗と原因

| 分類 | 根拠となる実行 | 確認した内容と対応 |
| --- | --- | --- |
| Linuxの実行ファイル競合（2回） | [39854093445](https://github.com/ykwyuta/yyeditor/actions/runs/39854093445)、[39870809817](https://github.com/ykwyuta/yyeditor/actions/runs/39870809817) | `git_tools::setup`でコピーしたバイナリを直後に起動し、`ExecutableFileBusy / Text file busy`。書き込みFDが他の並列子プロセスに継承される競合が原因として考えられる。コピーを避け、バイナリと同一ファイルシステムの一時フォルダーにハードリンクを作る方式へ修正。引用符・空白・日本語を含む実行パスの検証は残す。FD継承そのものをトレースしたわけではないため、機序は推定とする。 |
| ダイアログ終了とモデル反映の競合 | [39863141760](https://github.com/ykwyuta/yyeditor/actions/runs/39863141760) | yydbmanagerの空ER作成後、古いステータスを読んで失敗。コード上、親のEnableWindowはモデル反映前に行われる。ダイアログ破棄・期待ステータス・SQL欄と結果欄の非表示まで期限付きで待つよう`4ca4c4a`で修正。[修正後の実行](https://github.com/ykwyuta/yyeditor/actions/runs/39870549669)は成功。 |
| C++依存とランナー更新の非互換（3回） | [39561826514](https://github.com/ykwyuta/yyeditor/actions/runs/39561826514)、[39571971646](https://github.com/ykwyuta/yyeditor/actions/runs/39571971646)、[39588987697](https://github.com/ykwyuta/yyeditor/actions/runs/39588987697) | DuckDB 1.4.1同梱fmtが`stdext::checked_array_iterator`を参照し、VS 2026/MSVC 14.51でコンパイル失敗。現行workflowはVS 2022の`windows-2022`へ固定し、キャッシュキーにも反映済み。対応理由と上流修正へのリンクを維持する。 |
| 新しいClippy lint（2回） | [39341455214](https://github.com/ykwyuta/yyeditor/actions/runs/39341455214)、[39342671301](https://github.com/ykwyuta/yyeditor/actions/runs/39342671301) | Rust 1.99の`manual_checked_ops`が描画の除算を検出し、`-D warnings`で失敗。CIと同じツールチェーンで全ターゲットを検査し、計算を修正する。警告抑止を対策にしない。 |
| 小さいCI画面での描画座標（2回） | [39359636143](https://github.com/ykwyuta/yyeditor/actions/runs/39359636143)、[39361113888](https://github.com/ykwyuta/yyeditor/actions/runs/39361113888) | yydrawのリサイズハンドル試験が`Drag::Resize`へ入らない。修正`9954858`はハンドルを小さいビューポート内へ収めている。ウィンドウサイズ・DPI・ズームに従う座標を使う。前者はWebView2が利用できない失敗も併発しており、その起動失敗の詳細原因まではログだけでは断定しない。 |
| Git設定による改行変換 | [39588987713](https://github.com/ykwyuta/yyeditor/actions/runs/39588987713) | Windowsのパッチ適用結果がCRLF、期待値がLFで不一致。現在の試験は一時Git設定を隔離し改行を固定する。正確なバイト比較を弱めない。 |
| Windowsの短縮パス | [39247604422](https://github.com/ykwyuta/yyeditor/actions/runs/39247604422) | yyclip履歴で`runneradmin`と`RUNNER~1`を表示文字列として比較し不一致。同じファイルの異なるパス表記を正規化する修正`ef321ca`を確認。 |
| 非対話端末と永続セッション | [39246090686](https://github.com/ykwyuta/yyeditor/actions/runs/39246090686) | tmuxで新規作成の衝突を期待して成功扱いになり、screenで作成済みセッションを確認できず失敗。修正`f76c9e3`でCI依存と端末試験条件を見直している。現在はPTYと入力エコーを避けた完了マーカーを使い、実セッションの状態を待つ。 |

## 再発の共通点

「ローカルで一度通る」「親画面が操作可能」「ファイルコピーが返った」という中間状態を、CI上でも処理が完了している根拠にしていたケースがある。また、OS固有の改行・パス、ランナーのC++環境、Rust stableのlint更新が検証条件から抜けていた。

対策は成功条件そのものの改善と環境の明示である。UIは完成した状態、Gitは結果バイト、プロセスは安全に公開済みの実行ファイルを確認する。単なるsleep延長、無条件リトライ、失敗試験のスキップでは原因が残る。

## 今回のローカル検証

- yydbmanager: `windows-shell.ps1`をCIと同じ編集済みER YAMLで実行し、ER新規作成、7 DBの型選択、列の一括編集、Excelコピー、Undo、サイズ変更、ヘルプ、接続設定の保存・再読込まで成功。
- yymerge: WindowsのGit連携2試験が成功。LinuxはRust 1.99.0の既存Dockerイメージ、ソースとCargoレジストリを読み取り専用、ビルド用ボリュームを分離、外部ネットワークなしでヘッドレス全91試験が成功。
- yymergeのWindows・Linux両方でClippyが成功。Linuxの並列Git連携2試験は10回連続実行も成功した。Linux Clippyのコンポーネント取得時だけネットワークを有効にし、依存解決はキャッシュを使用した。
- WindowsのGit Bash試験を通常のサンドボックスで実行した際は`NtCreateDirectoryObject ... Access denied`となったが、制限外の同じ試験は成功した。これはGit連携の期待終了コードを変更する理由にはしない。

最新コミットのGitHub Actions完了結果は、修正の報告時に実行URLとともに確認する。ローカル成功だけから全CI成功とは記載しない。

## 2026-10-23 デモ接続の追加と保存試験の固定件数

[実行39917757769](https://github.com/ykwyuta/yyeditor/actions/runs/39917757769)のWindowsシェル試験で、保存した接続先の再読込時に「件数が1、対象は先頭」と仮定して失敗した。原因は確認済み。初期デモ接続7件の追加後も試験側が固定件数と固定インデックスを使っていた。削除後0件という仮定と、初回保存前に設定ファイルが存在しないという待機条件も同じ理由で無効になった。

保存・再読込・削除の対象を接続名の完全一致で検索し、保存完了はダイアログ終了と接続名のモデル反映で待つ。再読込時には7種類のデモ接続もそれぞれ名前で確認する。初期データを追加する変更では、空の一覧・固定件数・先頭項目への依存を持つUI回帰試験を併せて見直す。

## 2026-10-25 共有SSH型の変更とLinux専用結合試験

[実行40108059549](https://github.com/ykwyuta/yyeditor/actions/runs/40108059549)と[実行40132807221のLinuxジョブ](https://github.com/ykwyuta/yyeditor/actions/runs/40132807221/job/120987941981)で、`apps/yy-agent/tests/ssh.rs:358`の`HostSpec`初期化に`ssh`フィールドがなく、E0063で失敗した。最新SHA `f1318b7b85dec19763c7e467fdcb925eefad7525`のログでも同じ本文を確認した。Armのエージェント結合試験も同じ原因で失敗している。

原因は確認済み。共有型へのSSH詳細設定の追加時に、`#![cfg(unix)]`の利用側試験を更新し忘れていた。Windows全対象Clippyではこのソースが除外される。Linuxでも`-p yy-remote -p yy-ssh -p yy-config`だけでは`yy-agent`の結合試験はコンパイルされず、部分的な成功からルート全体の成功を見込んでいた。

試験用HostSpecに`ssh: Default::default()`を追加する。試験除外や`allow`は追加しない。AGENTS.mdには、共有型の変更時にLinux専用の利用側初期化を確認し、Linuxルート全ワークスペースの全対象Clippyを実施することを明記した。

## 2026-10-26 ODBC廃止時のWindowsリンクとDb2接続識別子

今回はローカル検証で確認した失敗であり、該当するCI失敗実行URLはない。ODBC削除後、DuckDBの `RmStartSession` 等がLNK2019となったため、yy-db-coreのbuild.rsからWindows Restart Managerを明示的にリンクした。Windowsワークスペース全体のClippy・試験・実行ファイルビルドで検証した。

Db2の新ドライバーはCRRTKNが固定で、異なる接続のAPPLICATION_IDとUOW/activityキーが衝突したことをサーバーログで確認した。繰り返し実測取得時にはEXPLAIN_FROM_ACTIVITY内でサーバーがSIGSEGVとなったが、衝突からトラップに至る内部経路の全容は未確定。上流へ一意な接続識別子と並列・再接続の回帰試験を追加した。[修正PR](https://github.com/ykwyuta/rust-db2-driver/pull/1)、[修正SHAのCI成功](https://github.com/ykwyuta/rust-db2-driver/actions/runs/40299413772)。過去のモニターレコードを残した同じ専用Db2を含む5DB回帰が成功した。詳細と検証範囲は[移行の検証記録](verification/yydbmanager-db2-driver-2026-10-26.md)を参照。

## 2026-10-26: Office変換ツール導入による長時間化

[実行40284650502](https://github.com/ykwyuta/yyeditor/actions/runs/40284650502)ではLibreOffice・日本語フォントの導入に約68分を要した。aptログの160 MB取得に1時間7分41秒という記録から、Ubuntuミラーの転送待ちが主因と確認した。利用者の指示によりExcel・PowerPointの画像比較を廃止し、変換コードとCIの導入・専用試験を削除する。Office内容比較・通常画像・PDF比較の試験は維持する。[変更と検証記録](verification/yymerge-no-office-images-2026-10-26.md)を参照。変更後CIの所要時間は未測定。

## 2026-10-29: 画面試験の閉鎖待ちとキャプチャ対象の診断

[実行40792413651のyydbmanager Windowsジョブ](https://github.com/ykwyuta/yyeditor/actions/runs/40792413651/job/123180588392)（SHA `30df3fa0ac4f0cd521907b52616ba884f9738bfd`）で、`windows-shell.ps1` の `Snapshot` がBitmap生成時に `Parameter is not valid` で失敗した。直前の同じyydbmanagerソースに対する[実行40788137025](https://github.com/ykwyuta/yyeditor/actions/runs/40788137025)は成功している。

取得先の矩形・画面名がログにないため、今回無効になったウィンドウと機序は未確定。コード上は `GetWindowRect` の失敗を無視しており、無効な対象では0サイズのBitmapを生成してしまう。また、各ヘルプを `PostMessage(WM_CLOSE)` で閉じた後、固定40msだけで次を開いており、古いヘルプを次の検索で拾う競合がある。これは確認した試験上の問題であり、今回の直接原因という断定はしない。

ヘルプの実際の破棄を期限付きで待ってから次へ進むよう変更する。キャプチャは取得対象名を記録し、矩形取得失敗・0サイズでは対象ハンドル・矩形・同一プロセス内の画面情報を付けて失敗させる。PrintWindowや保存の失敗時もGDIオブジェクトをfinallyで解放する。試験のスキップや失敗の無視、固定待機時間の延長は行わない。

ローカルの独立ワークスペースで書式・全対象Clippy・通常試験61件が成功した。最新ビルドを用いたSQL実行計画・編集済みER・WebView2なしのヘルプの画面回帰も成功した。修正後SHAのCI完了結果も確認する。

## 2026-10-29: Zstd参照CSVのWindows改行変換

[実行40858184952のyydbmanager Windowsジョブ](https://github.com/ykwyuta/yyeditor/actions/runs/40858184952/job/123394390752)（SHA `f076538a6c433870092ac1b7983ec7fa7a79dbe2`）の`Test embedded engines, plans and transfer`で、`yy-parquet-zstd/tests/codec.rs`の参照フレーム読み込み・連結フレーム読み込みの2件が`Zstd decoded size does not match Parquet page size`で失敗した。Parquetの22レベル相互運用と、同じコーデックの圧縮・展開往復は成功していた。

原因は確認済み。固定Zstdフレームの元データはLFの132,667バイトだが、期待値を`include_bytes!`する`corpus.csv`に改行属性がなかった。実際のGitで`core.autocrlf=true`・`core.eol=crlf`の`checkout-index`を使うと、1,401個のLFにCRが加わり134,068バイトになる。手元の`core.autocrlf=input`とLinuxでは元のLFを保つため、従来のローカル試験では検出できなかった。

当該CSVだけを`.gitattributes`の`text eol=lf`で固定し、参照データの説明にバイト一致の要件を明記する。同じGitチェックアウト条件で修正後のCSVが132,667バイト・CRなしとなり、元データとバイト一致することを確認する。参照圧縮ファイルの再生成、サイズ・バイト比較の緩和、失敗試験の除外は行わない。独立yydbmanagerワークスペースの書式・全対象Clippy・通常試験をWindows/Linuxで検証し、修正SHAのCI完了まで確認する。

ローカルではRust 1.99.0で独立ワークスペース全体の書式・全対象ClippyがWindows/Linuxとも成功し、通常試験はWindows 67件・Linux 57件成功した。Windowsの一時ファイル置換はサンドボックスで拒否されたため、実ファイル操作の権限を付けて成功を確認した。外部DB等を必要とする既存のignored試験は実行していない。ログは`target/ci-zstd-db-clippy-windows.log`、`target/ci-zstd-db-tests-windows.log`、`target/ci-zstd-db-linux.log`。

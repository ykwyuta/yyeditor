# 録画操作バー・容量整理・アプリ起動承認の検証

2026-10-29、Windows実機とDockerのLinux、Rust 1.99.0。基点は `2b28a0266d8778d2bc4e40e5520dafcc85f289b2`、本記録を含む未コミットの変更を検証。現行の使い方は[toolboxヘルプ](../../../apps/yytoolbox/help/help.md)と[容量整理](../../user/yyfilemanager-cleanup.md)を参照。

## 変更

- 録画時は通常画面を隠し、約240×104論理pxの操作バーを表示。バーはWindowsのキャプチャ除外属性を設定・確認してから表示する。停止ボタンは途中までのMP4を保存し、通常画面を復元する。
- MP4の保存先は開始時に一度選択する。保存済みの結果に対する二度目のテキスト保存ボタンを表示しない。
- Escによる録画停止を廃止。終了キーの初期値は「なし（ボタンのみ）」。任意のCtrl+Shift/Altとファンクションキーの組合せは競合検査付きで選択可能。矩形選択中のEsc取り消しは残す。
- 容量整理は18分類へ拡張。VM、携帯バックアップ、DB・メール、モデル・データセット、メディア、診断データ等を検出し、所有アプリでの圧縮・世代管理を案内する。管理データを不要と断定せず、探索は読み取りのみ。回収可能量は測定していない。
- EXE/COMはWinVerifyTrustで署名を検証し、未署名・確認不能なら既定Noの承認を求める。検証から承認・プロセス作成までファイルの書き換え・削除を拒否する読み取りハンドルを保持する。OSの実行制御は維持し、起動エラーを表示する。

## 検証結果

- Windows/Linuxでルートの書式検査、全ターゲットのClippy（locked、warnings拒否）に成功。Windowsではファイルシステムのincrementalキャッシュのハードリンクに関する環境警告が出たが、検査は成功。Linuxのyy-files 85件、yytoolboxのコア・結合21件も成功（実アーカイブ試験2件は既定どおり未実行）。
- Windowsのtoolbox通常試験はライブラリ14件、バイナリー6件。22ツールの小画面・DPI・倍率・非同期処理に加え、録画ガード保持中のEscが停止フラグを立てず、明示停止は立てることを確認。
- 操作バーの既定設定と5種類の任意キーの表示を96/120/144/192 DPIで実GDI計測し、文字の収まりと停止通知の重複防止を確認。
- 実デスクトップでバーが表示される正の対照を取得し、除外後のMP4全20フレームにバーが映らないことをデコード確認。別の実画面試験で停止ボタンによる10秒未満の途中保存、既存ファイル置換、通常画面復元を確認した。
- 容量整理の既存・新規分類、管理データの読み取り維持、共通フォルダーとDocker仮想ディスクの検出、重複除去を試験。実ネイティブUIで非同期完了と640/1100pxの配置を確認。
- テスト専用の未署名実行ファイルを日本語・空白付きパスへ生成。承認拒否では起動せず、承認後は起動して終了を回収。承認中の書き換え・削除の拒否も確認した。

ログは `target/toolbox-bar-regression-tests.log`、`target/recording-bar-hardware-final.log`、`target/recording-stop-button-hardware-final.log`、`target/fm-cleanup-ui-final.log`、`target/fm-unsigned-launch-tests.log`、`target/toolbox-fm-clippy-windows.log`、`target/toolbox-fm-linux-final.log` に保存する。

yyfilemanagerの通常ビルドは成功。yytoolboxはバイナリー生成後、起動中の `target/debug/yytoolbox.exe` の置換でアクセス拒否になったため、その新しい生成物 `target/debug/deps/yytoolbox.exe` を `target/feature-preview/yytoolbox.exe` へコピーした。yyfilemanagerも同じ確認用フォルダーへ配置した。起動中のアプリは終了していない。

## 範囲

今回の実機試験は録画操作・映像除外が対象。マイク・PC再生音の実デバイス収録、複数モニター間の移動、UAC操作、組織のアプリ制御ポリシーによる拒否は再試験していない。Docker/Podmanの実エンジン調査試験はこの変更では実行していない。独立したyymerge/yydbmanager/YY3Designerワークスペースは変更せず、yy-files/yy-winの参照もないため対象外。プッシュ・CI実行・リリースは未実施。

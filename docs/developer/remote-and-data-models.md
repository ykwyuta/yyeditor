# リモート通信・データモデルの内部仕様

## SSHとエージェント

[yy-ssh](../../crates/yy-ssh/src/lib.rs)がSSHの接続・転送を担当し、
[yy-remote](../../crates/yy-remote/src/lib.rs)がリモートファイル操作とエージェント連携を扱います。
エージェント不要のSFTP/SCP経路と、配置したyy-agent経由の操作を区別します。
ゲートウェイの利用者設定は[転送例](../../demo/yysftp/README.md)へ分けています。

[yy-proto](../../crates/yy-proto/src/lib.rs)は標準入出力を通るフレームを定義します。
フレームはu32 LEの長さ、u32 LEの要求ID、postcardの本体で、長さはIDと本体の合計です。
応答は同じ要求IDを返し、複数要求を並行させます。起動時のMAGICでシェル出力との同期を取り、
VERSIONが異なるエージェントは配置し直します。上限・チャンク長・FileIdの各定義はソースの定数と型を正とします。
メッセージ形状を変えるときはVERSIONと利用側・エージェントの組を更新します。

## スプレッドシート

[yy-sheet](../../crates/yy-sheet/src/lib.rs)は値、型別の列チャンク、列の編集差分、シートとUndoを分離します。
Contextがメモリ予算・チャンクキャッシュ・作業用Storeを文書間で共有します。
CSV取り込み・固定長レイアウト・クエリ・数式・`.yys`保存は各モジュールで扱います。
固定長の推定結果も編集可能なレイアウトとして扱い、COBOLの符号・桁数の処理はyy-cobolを利用します。
操作と保存項目は[シートヘルプ](../../crates/yy-win/help/sheet.md)を参照します。

## 構成図と立体モデル

[yy-draw](../../crates/yy-draw/src/lib.rs)はDPIから独立した座標、ノード、ポート、接続・経路、描画、素材と保存を持ちます。
[format](../../crates/yy-draw/src/format.rs)が文書の読込・保存、[router](../../crates/yy-draw/src/router.rs)が経路、
[render](../../crates/yy-draw/src/render.rs)が描画を担当します。文書には利用した素材を含め、別環境で再編集できるようにします。
性能測定の結果は[測定履歴](../history/benchmarks/yydraw.md)へ分離します。

[yy3-core](../../apps/yy3designer/crates/yy3-core/src/lib.rs)はmodel・paint・image・sculpt・mesh・storageを公開します。
2Dの編集モデル、6方向の画像と立体化、ドットごとの厚み、丸み、プロジェクト保存と出力をUIから切り離します。
保存・往復・隣接ドットの接続の検証はyy3-coreの試験を参照します。

## 比較・DBのモデル

[yy-merge-core](../../apps/yymerge/crates/yy-merge-core/src/lib.rs)の厳密比較と、表示上の規則による同値を区別します。
正規化した比較用テキストを自動マージへ流用しません。sync_planが候補を作り、sync_runが明示的な実行・復旧を扱います。
画像・Office・PDFの比較モデルはCLIとUIで共有し、対応外のマージを成功扱いにしません。

[yy-db-core](../../apps/yydbmanager/crates/yy-db-core/src/lib.rs)はsession/native、values、catalog、
plan/plan_file/plan_visual、schema、transfer、diagnosticsを分離します。
DB別の生計画を保持した共通計画モデル、ER定義と配置・表示、精度を保つ値をUIへ渡します。
利用者のER YAML仕様・取得権限は[DBヘルプ](../../apps/yydbmanager/help/help.md)を正とします。

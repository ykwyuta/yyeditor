# Db2ドライバー移行・ODBC廃止の検証（2026-10-26）

yyeditorはリモートmaster `603179d` へ早送りしてから変更した。Db2は `rust-db2-driver` の `14999cd983a666b209f0446b398005b141bebb08` をGit依存として固定した。yyeditorの変更はローカルで検証し、リモートCIは未実行。

## 実装と互換性

- ODBC依存・feature・接続経路・ドライバー列挙・設定UI・検証イメージの導入処理を削除。旧接続文字列は接続前に拒否して再入力を案内する。
- Db2のTLSは公開CAと追加PEM CAを検証する。TLS接続も取消ハンドルでソケットを閉じるため、取消後は再接続が必要。
- インポートは別接続の値バインドを使用する。バイナリNULLは、未型付けVARCHARのNULLから変換できないため、SQL NULLとして格納先に型を決めさせる。
- 実測計画のOUT引数はSQLブロック内で受け、一時表から返された正確なステートメントキーを検索する。対象SQLの実行前に一時表を準備し、終了時に削除する。USER TEMPORARY表スペースとUSE権限が必要。
- DRDAのトランザクションは明示的。新規イベントモニターはCOMMIT後に起動する。通常の実測取得で利用者の変更を自動コミットしない。
- ドライバーが列名を返さない場合はCOLUMN1等を表示する。LOB・DECFLOAT、z/OS・IBM iは対応を宣言しない。

## 接続識別子の修正

初回のDb2単独検証とTLS・取消検証は成功したが、その後の5DB回帰でDb2 12.1.5がEXPLAIN_FROM_ACTIVITY内でSIGSEGVとなった。診断スタックは `sqlnxFormatOBJECTMETRICS → sqlnxFormatExplain → explainFromActivity`。クライアントのI/O切断だけを成功・スキップとして扱わなかった。

ドライバーはCRRTKNを `RUSTDB2.CONN` に固定しており、サーバーログ上も異なるAPPHDL（0-24、0-42、0-60、0-66等）が同じAPPID `RUSTDB.F24BC3D6D5D5` を持っていたことを確認した。接続とUOW/activityの識別が衝突することは確認済み。これがサーバートラップに至る内部経路の全容までは断定しない。

利用者の許可を受け、上流にOS乱数による19バイトのCRRTKN生成と、並列接続・再接続の通信試験、実サーバーのAPPLICATION_ID一意性試験を追加した。[修正PR](https://github.com/ykwyuta/rust-db2-driver/pull/1)。修正後は過去レコードが残る同じ検証DBで5DB回帰が成功した。ドライバーの[対象SHAのCI](https://github.com/ykwyuta/rust-db2-driver/actions/runs/40299413772)はWindows・Linux・macOS・Rust 1.89の全ジョブが成功した。

ODBC削除後のWindowsリンクではDuckDBの `RmStartSession` 等が未解決となった。DuckDBが使うRestart Managerをyy-db-coreのbuild.rsで明示的にリンクし、Windowsのワークスペース試験を通した。

## 検証範囲

Rust 1.99.0、Windows（VS 2022 Build Tools）とLinuxコンテナーで検証した。ローカルの既定stable 1.92.0の結果だけでは判断していない。

- yydbmanager独立ワークスペース全体のfmt、Clippy（全ターゲット、locked、警告をエラー化）、単体試験。
- 専用コンテナーのPostgreSQL 17.11、MySQL 8.4.11、SQL Server 2022 16.0.4305.5、Oracle 23.26.3、Db2 12.1.5で、接続・精度・日本語・NULL・入出力・推定／実測計画・複雑な計画・ER・DDLを検証。[5DB結果](../../../apps/yydbmanager/tests/docker/results/2026-10-26-native-drivers.json)。利用者の既存DBは使用していない。
- Db2の専用自己署名CAを使い、TLS接続、不信頼CAの拒否、DECIMAL・日本語・バイナリ・NULLの投入、サーバーで確認したロック待機中の取消を検証。
- Windowsの既存UI試験で、メニュー・ヘルプ・接続設定・ODBCコントロールの不存在・暗号化保存と再読込・サイズ変更等を検証。

過去のODBC検証記録は当時の結果として残し、現行接続の説明は利用者文書と同梱ヘルプへ反映した。

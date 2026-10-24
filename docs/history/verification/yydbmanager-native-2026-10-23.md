# yydbmanager Rust直接接続の検証（2026-10-23）

> 検討・作業記録です。現在の操作・内部仕様は[文書案内](../../README.md)を参照してください。

## 実装

接続先の設定でRust直接接続／ODBCを選択する。新しい外部接続はRust直接接続が初期値。既存のODBCプロファイルは変換せず維持する。SQLite・DuckDBは同梱エンジンを継続利用する。

| DB | 使用する実装 |
| --- | --- |
| PostgreSQL | postgres 0.19.14、postgres-native-tls |
| MySQL | mysql 28.0.4 |
| SQL Server | tiberius-ng 0.13.1（TDS） |
| Oracle | Oracle公式oracledb 26.0.0-beta.4（Thin） |
| Db2 LUW | Zeusのzeus-drdaを移植した同期DRDAクライアント |

ODBCドライバー、Java/JDBC、Oracle Instant Client、IBM CLIを直接接続に使用しない。TLSバックエンドはOSのTLS基盤を利用するため、「依存ライブラリーが一切ない」という意味ではない。ODBC方式は従来どおり別途選択できる。

## Zeusからの移植

移植元は https://github.com/ykwyuta/Zeus 、コミット`99a30e6e0748357eb2aeecb7a584785d023e0935`。`crates/zeus-drda/src/{client,ddm,sql}.rs`と`zeus-cobol`のCP037表を移植し、アプリ内で独立したモジュールにした。ライセンスはGPL-3.0-or-later。SQLDARD解析の参考実装と新ドライバーのライセンスは`apps/yydbmanager/THIRD-PARTY-NOTICES.md`へ記載する。

移植時の変更は以下のとおり。

- DRDA制御情報と認証の文字コードをCCSID 500へ補正。CP037との記号差により、公開デモパスワードの`!`が誤変換されることを実DBで確認した。[IBMの説明](https://www.ibm.com/support/pages/drda-user-id-and-password-not-being-transmitted-correctly-when-containing-characters-%C2%AC-%C2%A2)に基づき、7文字の差を回帰テストにした。
- TLS、接続／読み書きタイムアウト、受信サイズ制限、不正フレームの検査を追加。
- SQLAM 7のSQLDARDから列名・ラベルを取得する。実応答の固定フィクスチャと途中切断の試験を追加。
- 結果取得後とデコード失敗時にカーソルを閉じる。未対応CLOBの問い合わせが失敗した後でも、同じ接続で後続SQLとテーブル削除が成功することを試験する。
- バイナリ値はDb2の`BX'...'`を使用する。インポートは別接続・別トランザクションとし、SQLタブのトランザクションを確定しない。

## 実接続試験

Windows x64／Rust 1.99.0から、`demo/yydbmanager/compose.yaml`の稼働中のローカルDocker DBへ接続した。テストはデモ専用ポートと一意な試験テーブルを使い、既存のデモデータは更新しない。

```powershell
cargo +1.99.0 test --manifest-path apps/yydbmanager/Cargo.toml -p yy-db-core --test native_live --locked -- --ignored --nocapture
```

対象を絞る場合は`YY_NATIVE_FILTER`に`PostgreSQL`、`MySQL`、`SQL Server`、`Oracle`、`IBM Db2`のいずれかを指定する。この試験は通常CIではignoreされる。未実行を実接続成功として扱わない。

5DBすべてで接続、符号付き64bit整数の最大値、20桁DECIMALの小数精度、日本語・引用符・バックスラッシュ、NULLとバイナリの保持、別接続でのインポート、ロールバック、列情報、推定計画、テーブル一覧取得を確認した。PostgreSQL／MySQL／SQL Server／Oracleは実測計画も確認した。Db2は未対応型の失敗後にカーソルが解放され、後続DDLが実行できることも確認した。

ローカル検証DBはTLS無効の明示設定で接続した。TLSの証明書チェーン・企業内認証・クラウド接続・Windows統合認証は、この実接続試験では未検証。

## 制限

Oracleドライバーはベータ版。Db2移植版の認証情報／DB名はASCII、SQLは30,000バイト、受信結果は64MiB以内。BLOB/CLOB/XML等の型は現在未対応。推定計画の述語は1件32,672バイト以内とし、それを超えた場合は切り捨てずエラーにする。OUT引数を使うDb2実測収集はODBC方式で利用する。z/OS・IBM iは未検証。

PostgreSQLはサーバーへ取消要求を送る。Db2は停止時にソケットを閉じるため再接続が必要。他の外部DBは行取得時にも停止要求を確認する。読み取り専用の強制はDB側の権限を使う。詳細はアプリ内の「ODBC不要のRust直接接続」を参照。

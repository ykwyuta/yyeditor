# yydraw 業務・ITアイコン306種

システム構成図、業務フロー、プロジェクト計画、戦略・分析資料向けの独自SVGアイコンです。
左のカテゴリで「業務・IT / 01 …」～「業務・IT / 31 …」を選び、名前または検索語で絞り込めます。
名前と画像を表示し、ドラッグして配置します。配置した図柄だけが文書に保存されます。
ラベルは他のyyシリーズと同じフォントを使用します。図柄内の製品名も省略せずパスで描き、外部フォントに依存しません。

既存クラウドアイコン1,834種と合わせて2,140種です。公式クラウド素材とは別の独自図柄で、
このリポジトリと同じGPL-3.0-or-laterで提供します。製品名のアイコンも公式ロゴではなく、用途を表す独自イラストです。

再生成: `python tools/business-icons/generate.py`。整合性確認: `python tools/business-icons/generate.py --check`。

## 01 コンピューティング

| 名前 | 検索語 |
| --- | --- |
| サーバー | server サーバ |
| ラック | rack datacenter データセンター |
| デスクトップPC | desktop workstation client 端末 |
| ノートPC | laptop notebook mobile パソコン |
| 仮想マシン | virtual machine vm 仮想化 |
| コンテナ | container docker pod kubernetes k8s |
| アプリケーション | application app software system ソフトウェア |
| バッチ処理 | batch job bulk 一括処理 |
| メインフレーム | mainframe host legacy 汎用機 ホスト |
| エッジ機器 | edge iot device sensor センサー |

## 02 ネットワーク

| 名前 | 検索語 |
| --- | --- |
| ルーター | router routing ルータ |
| スイッチ | switch switching hub ハブ |
| ファイアウォール | firewall fw 境界防御 |
| ロードバランサー | load balancer lb 負荷分散 |
| VPN | vpn tunnel encrypted 専用接続 トンネル |
| ゲートウェイ | gateway gw proxy プロキシ |
| インターネット | internet wan globe 外部ネットワーク |
| 無線LAN | wifi wireless wlan access point AP |
| DNS | dns domain name resolver 名前解決 |
| サブネット | subnet vlan segment lan セグメント |

## 03 データ・連携

| 名前 | 検索語 |
| --- | --- |
| データベース | database db sql rdb データ管理 |
| データウェアハウス | data warehouse dwh bi 集約 |
| データレイク | data lake raw unstructured 非構造化 |
| ファイルストレージ | file storage nas folder 共有フォルダー |
| バックアップ | backup restore recovery 復元 |
| オブジェクトストレージ | object blob bucket オブジェクト |
| イベントストリーム | event stream queue message kafka メッセージ キュー |
| ETL・データ変換 | etl elt integration transform データ連携 変換 |
| データマート | data mart department analytics 部門データ |
| データカタログ | data catalog metadata lineage メタデータ |

## 04 セキュリティ

| 名前 | 検索語 |
| --- | --- |
| ユーザー認証 | identity authentication login idp 認証 sso |
| 暗号鍵 | key encryption kms secret 暗号 秘密 |
| セキュリティ保護 | shield protection secure 防御 |
| アクセス制限 | lock access control restriction acl ロック |
| 電子証明書 | certificate tls ssl pki 証明書 |
| 多要素認証 | mfa otp two factor 2fa 二要素 |
| 監査 | audit compliance evidence コンプライアンス 証跡 |
| 脆弱性 | vulnerability bug cve security risk 脆弱性診断 |
| SOC・脅威監視 | soc siem threat security operations 脅威 |
| 権限・ロール | authorization permission role iam rbac アクセス権 |

## 05 開発・運用

| 名前 | 検索語 |
| --- | --- |
| 稼働監視 | monitoring observability health ヘルスチェック |
| アラート | alert notification alarm 通知 警報 |
| ログ | logs logging log 記録 |
| メトリクス | metrics performance gauge 性能 指標 |
| 分散トレーシング | trace tracing span apm 分散追跡 |
| ジョブスケジューラー | scheduler cron job schedule 定期実行 |
| CI・CD | ci cd cicd pipeline deployment devops デプロイ |
| テスト・品質保証 | test qa quality verification 品質 |
| 障害対応 | incident outage problem 障害 インシデント |
| ヘルプデスク | helpdesk support service desk サポート |

## 06 組織・拠点

| 名前 | 検索語 |
| --- | --- |
| 担当者 | person employee staff user 担当 社員 |
| チーム | team group organization 部署 組織 |
| 本社 | headquarters office company enterprise 会社 オフィス |
| 支店・営業所 | branch office site location 拠点 |
| 工場 | factory manufacturing plant 製造 |
| 店舗 | store shop retail 小売 店 |
| 在宅勤務 | home office remote work 在宅 テレワーク |
| 遠隔拠点 | remote site geographic location リモート 地域 |
| パートナー企業 | partner vendor supplier alliance 取引先 ベンダー |
| 顧客 | customer client consumer 顧客 利用者 |

## 07 業務プロセス

| 名前 | 検索語 |
| --- | --- |
| 作業・タスク | task work activity 作業 |
| ワークフロー | workflow process flow 業務フロー |
| 承認 | approval signoff permission 決裁 稟議 |
| 判断・分岐 | decision branch condition 判断 分岐 条件 |
| 業務文書 | document paper specification 文書 仕様書 |
| 申請フォーム | form request input 申請 入力 |
| 契約 | contract agreement legal 契約書 合意 |
| チェックリスト | checklist todo completion 点検 完了 |
| 業務自動化 | automation rpa robot 自動化 |
| 業務引き継ぎ | handover transfer handoff 引継ぎ 移管 |

## 08 プロジェクト

| 名前 | 検索語 |
| --- | --- |
| ロードマップ | roadmap planning plan 計画 |
| ガントチャート | gantt schedule timeline 工程表 |
| マイルストーン | milestone checkpoint flag 節目 |
| カンバン | kanban agile scrum board アジャイル |
| リソース配分 | resource allocation staffing capacity 要員 工数 |
| 予算・コスト | budget cost finance money 費用 予算 |
| リスク管理 | risk issue mitigation リスク 課題 |
| 会議 | meeting workshop discussion ワークショップ 打合せ |
| 依存関係 | dependency dependencies critical path wbs 依存 |
| 成果物 | deliverable release delivery 成果 納品 |

## 09 戦略・ビジネス

| 名前 | 検索語 |
| --- | --- |
| ビジョン | vision future direction 将来像 方針 |
| 目標・KPI | target goal kpi okr 目標 |
| SWOT分析 | swot strength weakness opportunity threat 強み 弱み 機会 脅威 |
| カスタマージャーニー | customer journey experience cx touchpoint 顧客体験 |
| バリューチェーン | value chain supply chain 価値連鎖 サプライチェーン |
| ポートフォリオ | portfolio investment product business 事業 製品 |
| 変革・DX | transformation dx change digital デジタル変革 |
| ビジネスモデル | business model canvas revenue model 収益モデル |
| 競合比較 | competitive competitor benchmark positioning 競合 ベンチマーク |
| ステークホルダー | stakeholder influence interest relationship 利害関係者 |

## 10 分析・報告

| 名前 | 検索語 |
| --- | --- |
| レポート | report reporting presentation 資料 報告書 |
| ダッシュボード | dashboard analytics cockpit 分析画面 |
| 棒グラフ | bar chart histogram 棒グラフ 比較 |
| 折れ線グラフ | line chart trend time series 傾向 推移 |
| 円グラフ | pie chart share ratio 構成比 |
| 散布図 | scatter plot correlation distribution 相関 分布 |
| ファネル | funnel conversion pipeline sales コンバージョン |
| 評価マトリクス | matrix evaluation prioritization priority 評価 優先順位 |
| インサイト・仮説 | insight hypothesis idea innovation 知見 仮説 アイデア |
| アンケート・調査 | survey research questionnaire interview 調査 ヒアリング |

## 11 モバイル端末

| 名前 | 検索語 |
| --- | --- |
| スマートフォン | smartphone mobile phone スマホ 携帯電話 |
| Androidスマートフォン | android アンドロイド smartphone スマートフォン スマホ |
| iPhone | iphone ios apple アイフォン アイフォーン スマートフォン スマホ |
| タブレット | tablet slate タブレット端末 |
| Androidタブレット | android tablet アンドロイド タブレット |
| iPad | ipad ipados apple アイパッド タブレット |
| スマートウォッチ | smartwatch wearable watch 腕時計 ウェアラブル |
| フィーチャーフォン | feature phone keypad ガラケー 携帯電話 |
| 業務用ハンディ端末 | rugged handheld barcode scanner PDA ハンディ 倉庫 |
| 折りたたみスマートフォン | foldable folding dual screen 折りたたみ 二画面 スマホ |

## 12 モバイル機能・通信

| 名前 | 検索語 |
| --- | --- |
| モバイルアプリ | mobile app application スマホアプリ |
| アプリ配信 | app store distribution install download アプリストア インストール |
| プッシュ通知 | push notification alert 通知 お知らせ |
| スマホ決済 | mobile payment wallet cashless 電子マネー キャッシュレス |
| QRコード読み取り | qr code scan scanner 二次元コード QRコード |
| 生体認証端末 | biometric fingerprint face id 指紋 顔認証 スマホ |
| NFC・近距離通信 | nfc contactless tap 非接触 近距離通信 タッチ |
| スマホ位置情報 | mobile location map geolocation 位置情報 地図 |
| 5G通信 | 5g cellular wireless mobile 第五世代 移動体通信 |
| モバイル端末管理 | mdm emm mobile device management 端末管理 |

## 13 開発言語

| 名前 | 検索語 |
| --- | --- |
| Java | java jvm openjdk ジャバ 開発言語 |
| Rust | rust rustlang cargo ラスト 開発言語 |
| Python | python py pip パイソン 開発言語 |
| JavaScript | javascript js ecmascript node nodejs ジャバスクリプト |
| TypeScript | typescript ts deno タイプスクリプト 開発言語 |
| C# | c# csharp dotnet .net シーシャープ 開発言語 |
| Go | go golang gopher ゴー 開発言語 |
| Kotlin | kotlin kt android コトリン 開発言語 |
| Swift | swift ios apple スウィフト 開発言語 |
| COBOL | cobol mainframe legacy コボル 汎用機 開発言語 |

## 14 データベース製品

| 名前 | 検索語 |
| --- | --- |
| PostgreSQL | postgresql postgres pgsql ポスグレ ポストグレス データベース |
| MySQL | mysql マイエスキューエル データベース |
| MariaDB | mariadb maria マリアデービー データベース |
| SQLite | sqlite embedded local エスキューライト 組込 データベース |
| SQL Server | sql server mssql microsoft マイクロソフト データベース |
| Oracle Database | oracle database db オラクル データベース |
| Redis | redis cache key value レディス キャッシュ データベース |
| MongoDB | mongodb mongo nosql document モンゴ データベース |
| Elasticsearch | elasticsearch elastic search エラスティック 全文検索 データベース |
| データベースレプリケーション | database replication replica primary standby 同期 複製 レプリカ |

## 15 AI・機械学習

| 名前 | 検索語 |
| --- | --- |
| AI・人工知能 | ai artificial intelligence 人工知能 エーアイ |
| 生成AI | generative ai genai 生成 人工知能 |
| 大規模言語モデル | llm large language model 生成ai 言語モデル |
| AIチャットボット | ai chatbot chat bot 対話 チャットボット |
| AIエージェント | ai agent autonomous assistant 自律 エージェント |
| ニューラルネットワーク | neural network deep learning ニューラル 深層学習 ai |
| 機械学習・モデル学習 | ml machine learning training model 機械学習 学習 ai |
| 画像認識 | computer vision image recognition ai 画像認識 検出 |
| 音声認識 | speech voice recognition stt asr ai 音声 文字起こし |
| RAG・検索拡張生成 | rag retrieval augmented generation vector embedding ai 検索拡張生成 ベクトル |

## 16 顧客対応・メール

| 名前 | 検索語 |
| --- | --- |
| コールセンター | call center contact centre コールセンター コンタクトセンター 電話 |
| 電話オペレーター | operator agent receptionist オペレーター 電話受付 |
| 自動音声応答・IVR | ivr interactive voice response auto attendant 自動音声 応答 |
| 着信キュー | call queue acd waiting routing 着信待ち 呼分配 |
| CTI・電話連携 | cti computer telephony integration 電話連携 |
| チャット窓口 | chat messaging live support チャット メッセージ |
| メール | email e-mail mail メール 電子メール Eメール |
| メールサーバー | mail server smtp imap pop3 メールサーバ 配信 |
| メルマガ・一斉配信 | newsletter bulk mail mailing campaign メルマガ 一斉送信 |
| ビデオ会議 | video conference meeting webinar ビデオ会議 ウェビナー |

## 17 人物・ライフステージ

| 名前 | 検索語 |
| --- | --- |
| 赤ちゃん | baby infant newborn 赤ちゃん 乳児 ベビー |
| 子供 | child children kid 子供 子ども こども 児童 |
| 学生 | student school pupil 学生 生徒 児童 学校 |
| 若者・青少年 | teenager youth young 青少年 若者 ティーン |
| 成人 | adult person grownup 大人 成人 |
| 高齢者 | senior elderly older person aged 老人 高齢者 お年寄り シニア |
| 高齢者夫婦 | senior elderly couple 老夫婦 高齢者 世帯 老人 |
| 家族 | family household parents children 家族 世帯 親子 子供 |
| 介護・医療スタッフ | caregiver nurse medical care 介護 看護師 介護士 |
| 車椅子利用者 | wheelchair accessibility disability 車椅子 車いす バリアフリー |

## 18 ファイル形式・帳票

| 名前 | 検索語 |
| --- | --- |
| 固定長ファイル | fixed length file fixed-width record flat file 固定長 固定幅 レコード |
| CSVファイル | csv comma separated values delimited カンマ区切り CSV ファイル |
| TSVファイル | tsv tab separated values delimited タブ区切り TSV ファイル |
| JSONファイル | json javascript object notation ジェイソン ファイル |
| XMLファイル | xml markup structured エックスエムエル ファイル |
| PDFファイル | pdf portable document 電子文書 ファイル |
| 表計算ファイル | xlsx xls excel spreadsheet workbook エクセル 表計算 ファイル |
| 帳票 | report form business statement 帳票 定型帳票 報告 |
| 請求書 | invoice billing bill receipt 請求書 明細 帳票 |
| 帳票印刷 | printer print spool report 帳票 印刷 プリンター |

## 19 広告・メディア

| 名前 | 検索語 |
| --- | --- |
| Web広告 | web advertising advertisement ads online internet WEB広告 ウェブ広告 ネット広告 |
| ディスプレイ広告 | display banner advertising ad バナー ディスプレイ広告 |
| 検索広告 | search advertising paid sem ppc listing 検索広告 リスティング |
| 動画広告 | video advertising preroll commercial 動画広告 CM |
| SNS広告 | social media sns advertising influencer SNS広告 ソーシャル |
| テレビ | television tv broadcast テレビ 受像機 放送 |
| ラジオ | radio audio broadcast am fm ラジオ 音声放送 |
| 新聞 | newspaper news press print 新聞 ニュース 紙媒体 |
| デジタルサイネージ | digital signage outdoor ooh dooh 電子看板 サイネージ |
| 広告効果測定 | advertising analytics conversion ctr roas web 広告効果 計測 |

## 20 衛星・通信インフラ

| 名前 | 検索語 |
| --- | --- |
| 人工衛星 | satellite space orbital 人工衛星 衛星 宇宙 |
| 衛星アンテナ | satellite dish antenna parabolic パラボラ 衛星アンテナ |
| 衛星地上局 | satellite ground station earth station teleport 衛星 地上局 |
| GPS・測位衛星 | gps gnss navigation satellite 位置 測位衛星 衛星 |
| 地球観測 | earth observation remote sensing satellite リモートセンシング 衛星 地球観測 |
| 気象衛星 | weather satellite meteorology forecast 気象衛星 天気 予報 |
| ロケット | rocket launch space vehicle ロケット 打ち上げ 宇宙 |
| 放送塔 | broadcast tower tv television transmitter テレビ 放送塔 電波塔 |
| 海底ケーブル | submarine subsea underwater fiber cable 海底ケーブル 光ケーブル |
| 携帯基地局 | cellular mobile base station 4g 5g antenna 携帯 基地局 |

## 21 メッセージング製品

| 名前 | 検索語 |
| --- | --- |
| IBM MQ | ibm mq message queue messaging websphere メッセージキュー |
| Apache Kafka | apache kafka broker event streaming カフカ メッセージ |
| RabbitMQ | rabbitmq rabbit mq amqp ラビット メッセージブローカー |
| Apache ActiveMQ Artemis | apache activemq artemis jms amqp メッセージブローカー |
| NATS | nats messaging jetstream publish subscribe メッセージング |
| Apache Pulsar | apache pulsar pubsub messaging パルサー メッセージ |

## 22 キュー・トピック

| 名前 | 検索語 |
| --- | --- |
| メッセージキュー | queue message mq キュー 待ち行列 メッセージ |
| メッセージトピック | topic message トピック メッセージ 配信 |
| メッセージ送信者 | producer publisher sender メッセージ 送信 プロデューサー |
| メッセージ受信者 | consumer subscriber receiver メッセージ 受信 コンシューマー |
| 発行・購読 | publish subscribe pubsub fanout ファンアウト 発行 購読 |
| 一対一メッセージ通信 | point to point direct messaging 一対一 メッセージ |
| デッドレターキュー | dead letter queue dlq error poison message デッドレター エラーキュー |
| 先入れ先出しキュー | fifo first in first out ordering キュー 先入れ先出し |
| 遅延キュー | delay delayed scheduled message queue 遅延 キュー |
| メッセージブローカー | broker exchange message router メッセージ ブローカー 中継 |

## 23 メッセージ配送・信頼性

| 名前 | 検索語 |
| --- | --- |
| 受信確認 | ack acknowledgement receipt message メッセージ 受信確認 応答 |
| メッセージ再送 | retry redelivery resend メッセージ 再送 リトライ |
| 再送間隔の調整 | exponential backoff retry 再送 間隔 バックオフ |
| メッセージ順序制御 | ordering sequence message 順序 制御 メッセージ |
| 優先キュー | priority queue message 優先 キュー メッセージ |
| 重複メッセージ除去 | deduplication duplicate message 重複 排除 メッセージ |
| 冪等な受信処理 | idempotent idempotency consumer message 冪等 べき等 受信 |
| トランザクショナルアウトボックス | transactional outbox database message アウトボックス 送信保証 |
| イベント再生 | event replay reprocess イベント 再生 再処理 |
| メッセージ保持 | message retention durable persistence メッセージ 保持 永続化 |

## 24 バックアップ方式

| 名前 | 検索語 |
| --- | --- |
| フルバックアップ | full backup complete バックアップ 完全 全体 |
| 増分バックアップ | incremental backup delta 増分 バックアップ |
| 差分バックアップ | differential backup difference 差分 バックアップ |
| スナップショット | snapshot point in time storage バックアップ スナップショット |
| 継続的バックアップ | continuous backup cdp data protection 継続 バックアップ |
| オフラインバックアップ | offline air gap backup removable オフライン バックアップ エアギャップ |
| 変更不可バックアップ | immutable backup worm object lock イミュータブル 改ざん防止 バックアップ |
| 暗号化バックアップ | encrypted encryption backup 暗号化 バックアップ |
| 遠隔バックアップ | remote offsite backup 遠隔 オフサイト バックアップ |
| 複数リージョンバックアップ | multi region geographic backup geo 複数リージョン 地理分散 バックアップ |

## 25 復旧・バックアップ運用

| 名前 | 検索語 |
| --- | --- |
| バックアップから復元 | restore backup recovery バックアップ 復元 リストア |
| 指定時点への復旧 | point in time recovery pitr rollback 時点 復旧 |
| 災害復旧 | disaster recovery dr bcp 災害 復旧 事業継続 |
| フェイルオーバー | failover standby switchover 切替 障害 待機 |
| フェイルバック | failback return primary 復帰 切戻し フェイルバック |
| 復旧訓練 | recovery drill rehearsal disaster test 復旧 訓練 演習 |
| バックアップスケジュール | backup schedule calendar scheduled バックアップ 計画 定期 |
| バックアップ保存期限 | backup retention policy lifecycle expiry バックアップ 保存期限 世代管理 |
| バックアップ検証 | backup verification integrity checksum バックアップ 検証 整合性 |
| アーカイブ復元 | archive retrieval cold storage restore アーカイブ 復元 取り出し |

## 26 暗号化・鍵管理

| 名前 | 検索語 |
| --- | --- |
| 保存データの暗号化 | encryption at rest storage disk 暗号化 保存 データ |
| 通信データの暗号化 | encryption in transit tls transport 暗号化 通信 経路 |
| エンドツーエンド暗号化 | end to end encryption e2ee 暗号化 端末間 |
| 公開鍵暗号 | public key asymmetric encryption rsa ecc 公開鍵 暗号化 |
| 共通鍵暗号 | symmetric shared key encryption aes 共通鍵 暗号化 |
| 暗号鍵のローテーション | key rotation renewal encryption 暗号鍵 更新 ローテーション |
| 暗号鍵管理 | key management kms keystore 暗号鍵 管理 |
| ハードウェアセキュリティモジュール | hardware security module hsm cryptographic ハードウェア 暗号装置 |
| デジタル署名 | digital signature signing verification 電子署名 デジタル署名 |
| データのトークン化 | tokenization token sensitive data トークン化 置換 個人情報 |

## 27 データ保護・認証

| 名前 | 検索語 |
| --- | --- |
| データマスキング | masking redaction privacy データ マスキング 秘匿 |
| データ匿名化 | anonymization anonymous privacy 個人情報 匿名化 |
| シークレット保管庫 | secret vault credentials credential store シークレット 保管庫 資格情報 |
| パスワードハッシュ化 | password hashing hash bcrypt argon パスワード ハッシュ化 |
| 証明書更新 | certificate rotation renewal tls ssl 証明書 更新 |
| 情報漏洩防止 | data loss prevention dlp exfiltration 情報漏洩 防止 |
| データ完全消去 | secure erase erasure sanitization wipe データ 消去 完全消去 |
| ゼロトラストアクセス | zero trust ztna verify access ゼロトラスト アクセス |
| 相互認証通信 | mutual tls mtls authentication certificate 相互認証 通信 |
| データアクセス監査 | data access audit trail database アクセス 監査 証跡 |

## 28 クラスタ構成

| 名前 | 検索語 |
| --- | --- |
| サーバークラスタ | server cluster node compute サーバ クラスタ クラスター |
| データベースクラスタ | database cluster distributed db データベース クラスタ |
| Kubernetesクラスタ | kubernetes k8s cluster pod node クバネティス クラスタ |
| アクティブ・アクティブ構成 | active active cluster multi master クラスタ 同時稼働 |
| 稼働系・待機系構成 | active passive standby cluster クラスタ 稼働系 待機系 |
| クラスタの多数決 | cluster quorum majority consensus クラスタ クォーラム 多数決 |
| リーダー選出 | leader election consensus cluster クラスタ リーダー 選出 |
| データベースシャーディング | sharding shard partition cluster データベース シャード 分割 クラスタ |
| クラスタ間の複製 | cluster replication replica sync クラスタ レプリケーション 複製 |
| クラスタ自動拡張 | cluster autoscaling scale out クラスタ 自動拡張 オートスケール |

## 29 可用性・負荷制御

| 名前 | 検索語 |
| --- | --- |
| 水平スケーリング | horizontal scaling scale out in 水平 拡張 スケールアウト |
| 垂直スケーリング | vertical scaling scale up down 垂直 拡張 スケールアップ |
| サービス正常性確認 | health check probe liveness readiness サービス ヘルスチェック 正常性 |
| クラスタ死活監視 | heartbeat keepalive cluster failure クラスタ ハートビート 死活監視 |
| サーキットブレーカー | circuit breaker resilience isolation サーキットブレーカー 遮断 |
| 障害の隔離 | bulkhead isolation fault containment 障害 隔離 分離 |
| 流量制限 | rate limit limiting throttle quota 流量 制限 スロットリング |
| バックプレッシャー | backpressure flow control consumer バックプレッシャー 流量制御 |
| 分散キャッシュクラスタ | cache cluster distributed memory キャッシュ クラスタ 分散 |
| セッション情報の複製 | session replication state cluster セッション 複製 クラスタ |

## 30 イベント・ストリーム連携

| 名前 | 検索語 |
| --- | --- |
| イベント駆動構成 | event driven architecture eda イベント 駆動 非同期 |
| データ変更の取得 | change data capture cdc database データ変更 変更データ キャプチャ |
| スキーマレジストリ | schema registry avro protobuf compatibility スキーマ レジストリ |
| イベント履歴による状態管理 | event sourcing append log state イベントソーシング 履歴 状態 |
| コマンド・参照分離 | cqrs command query responsibility segregation コマンド 参照 分離 |
| ストリーム処理 | stream processing real time flink spark ストリーム リアルタイム 処理 |
| コンシューマーグループ | kafka consumer group subscribers コンシューマー グループ 受信 |
| トピックのパーティション | kafka topic partition shard トピック パーティション 分割 |
| 読み取り位置 | kafka consumer offset cursor checkpoint オフセット 読み取り位置 |
| メッセージ暗号化 | message encryption payload secure メッセージ 暗号化 ペイロード |

## 31 AIサービス・開発支援

| 名前 | 検索語 |
| --- | --- |
| OpenAI | openai オープンエーアイ ai service 人工知能 |
| ChatGPT | chatgpt openai chat assistant チャットジーピーティー 対話 ai |
| OpenAI API | openai api developer integration オープンエーアイ 開発 api ai |
| OpenAI Codex | openai codex coding agent コーデックス コード生成 開発支援 ai |
| Gemini | gemini google ジェミニ ジェミナイ ai service |
| Google AI Studio | google ai studio gemini developer グーグル エーアイ スタジオ 開発 |
| Claude | claude anthropic assistant クロード アンソロピック 対話 ai |
| Claude Code | claude code anthropic coding agent クロードコード コード生成 開発支援 ai |
| GitHub Copilot | github copilot coding assistant ギットハブ コパイロット 開発支援 ai |
| Microsoft Copilot | microsoft copilot office assistant マイクロソフト コパイロット ai |

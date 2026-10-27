//! Read-only diagnostic templates shared with the verified performance examples.
use crate::{DbKind, Result};

pub struct Template {
    pub title: &'static str,
    pub requirements: &'static str,
    pub sql: String,
}

pub fn templates(kind: DbKind) -> Result<Vec<Template>> {
    let (source, titles, requirements, skip, count): (&str, &[&str], &[&str], usize, usize) =
        match kind {
            DbKind::SqlServer => (
                include_str!("../../../../../demo/yydbmanager/sql/performance/sqlserver.sql"),
                &[
                    "実行中の要求とブロック元",
                    "重いSQL（累積CPU・読み取り）",
                    "サーバー全体の待機統計",
                ],
                &[
                    "SQL Server 2022以降はVIEW SERVER PERFORMANCE STATE、以前の版はVIEW SERVER STATE等が必要です。",
                ],
                0,
                3,
            ),
            DbKind::PostgreSql => (
                include_str!("../../../../../demo/yydbmanager/sql/performance/postgres.sql"),
                &[
                    "現在の処理・トランザクションとブロック元",
                    "重いSQL（pg_stat_statements）",
                    "DB全体の読み取り・一時領域・デッドロック",
                ],
                &[
                    "他ユーザーのSQL本文等はpg_read_all_stats／pg_monitor等の権限が必要です。",
                    "pg_stat_statementsのプリロード・再起動・対象DBでの拡張作成が必要です。時間の単位はmsです。",
                    "累積値とstats_resetを記録し、観測期間の差分を比較してください。",
                ],
                0,
                3,
            ),
            DbKind::MySql => (
                include_str!("../../../../../demo/yydbmanager/sql/performance/mysql.sql"),
                &[
                    "重いSQL（時間・走査行数・一時テーブル）",
                    "InnoDBのロック待ちとブロック元",
                    "現在実行中の接続",
                ],
                &[
                    "Performance Schemaの対象計測とSELECT権限が必要です。時間は秒へ換算しています。",
                    "sysと参照先のSELECT、内部関数のEXECUTE、PROCESS等の権限が必要です。",
                    "performance_schema.threadsのSELECTが必要です。PROCESSLIST_TIMEは現在の状態の秒数です。",
                ],
                0,
                3,
            ),
            DbKind::Oracle => (
                include_str!("../../../../../demo/yydbmanager/sql/performance/oracle.sql"),
                &[
                    "アクティブなセッションと待機・ブロック元",
                    "重いSQL（子カーソル別の累積統計）",
                ],
                &[
                    "SYS.V_$SESSIONのSELECT権限が必要です。自分の調査セッションも表示されます。",
                    "SYS.V_$SQLのSELECT権限が必要です。解析スキーマは接続ユーザーに絞っています。時間は秒です。",
                ],
                0,
                2,
            ),
            DbKind::Db2 => (
                include_str!("../../../../../demo/yydbmanager/sql/performance/db2.sql"),
                &[
                    "接続ごとのCPU・活動時間・待機時間",
                    "重いSQL（パッケージキャッシュ）",
                    "ロック待ちとブロック元",
                ],
                &[
                    "Db2 LUW用です。各MON_GET関数のEXECUTE等と収集設定を確認してください。CPUはus、活動・待機はmsです。",
                    "Db2 LUW用です。MON_GET_PKG_CACHE_STMTのEXECUTE等が必要です。MEMBERとEXECUTABLE_IDを併せて比較します。",
                    "Db2 LUW用です。SYSIBMADM.MON_LOCKWAITSの参照権限が必要です。待機がなければ0行です。",
                ],
                0,
                3,
            ),
            DbKind::DuckDb => (
                include_str!("../../../../../demo/yydbmanager/sql/performance/duckdb.sql"),
                &[
                    "メモリーと一時領域の現在の使用量",
                    "現在の一時ファイル",
                    "スレッド数・メモリー上限・一時フォルダー",
                ],
                &[
                    "同じDuckDBエンジン内の現在値です。OS全体の使用量や実行中のピークではありません。",
                ],
                0,
                3,
            ),
            DbKind::Sqlite => (
                include_str!("../../../../../demo/yydbmanager/sql/performance/sqlite.sql"),
                &[
                    "DBのページ数",
                    "ページサイズ（バイト）",
                    "再利用可能な空きページ数",
                    "他の接続による変更の検知",
                    "ジャーナルモードの確認",
                ],
                &[
                    "ページ数×ページサイズは論理容量の目安で、WAL・一時ファイルを含みません。",
                    "ページサイズの参照です。設定値の変更は行いません。",
                    "多いだけで異常とは限りません。VACUUMは実行しません。",
                    "同じ接続で時点を変えて比較してください。グローバルな更新回数ではありません。",
                    "モードの参照です。変更やチェックポイントは実行しません。",
                ],
                1,
                5,
            ),
        };
    let statements = crate::sql::statements(source, kind)?;
    if statements.len() < skip + count
        || titles.len() != count
        || !(requirements.len() == 1 || requirements.len() == count)
    {
        return Err("性能分析テンプレートの定義が不正です".into());
    }
    Ok(statements
        .into_iter()
        .skip(skip)
        .take(count)
        .enumerate()
        .map(|(i, sql)| Template {
            title: titles[i],
            requirements: requirements[if requirements.len() == 1 { 0 } else { i }],
            sql: format!("{};\n", sql.trim().trim_end_matches(';')),
        })
        .collect())
}

impl Template {
    pub fn editor_text(&self, kind: DbKind, connection_name: &str) -> String {
        let name = connection_name.replace(['\r', '\n'], " ");
        format!(
            "-- {}: {}\n-- 対象接続先: {}\n-- 表示しただけでは実行しません。内容を確認し、対象接続先に接続してF5で実行します。\n-- {}\n-- 詳細: ヘルプ → DB別の性能分析ビュー…\n\n{}",
            kind.name(),
            self.title,
            name,
            self.requirements,
            self.sql
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Database, Options};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn templates_need_no_application_tables_and_only_read_metadata() {
        for kind in DbKind::ALL {
            let templates = templates(kind).unwrap();
            assert!(templates.len() >= 2);
            for template in &templates {
                assert!(!template.requirements.is_empty());
                let statements = crate::sql::statements(&template.sql, kind).unwrap();
                assert_eq!(statements.len(), 1);
                assert!(
                    statements[0].starts_with("SELECT ") || statements[0].starts_with("PRAGMA ")
                );
                assert!(!template.sql.contains("FROM orders"));
                let text = template.editor_text(kind, "検証\nSELECT 42;");
                assert!(text.contains("-- 対象接続先: 検証 SELECT 42;"));
                if matches!(kind, DbKind::Sqlite | DbKind::DuckDb) {
                    let mut db = Database::open(&Options::memory(kind)).unwrap();
                    assert!(
                        !db.execute(&text, &AtomicBool::new(false))
                            .unwrap()
                            .is_empty()
                    );
                }
            }
        }
    }
}

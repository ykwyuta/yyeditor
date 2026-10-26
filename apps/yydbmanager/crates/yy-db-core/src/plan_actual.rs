//! Capture one execution in the current connection, restoring its collection settings.
use super::*;

fn marker() -> String {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "YYDBACT{:X}{:X}{:X}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    )
}
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
fn first(sets: &[ResultSet]) -> Result<Vec<Value>> {
    sets.first()
        .ok_or("統計の結果がありません")?
        .page(0, 1)?
        .into_iter()
        .next()
        .ok_or("統計が見つかりません".into())
}
fn restored<T>(result: Result<T>, cleanup: Result<Vec<ResultSet>>) -> Result<T> {
    match (result, cleanup) {
        (r, Ok(_)) => r,
        (Ok(_), Err(e)) => Err(format!(
            "実行は完了しましたが収集設定を復元できません。接続を閉じてください: {e}"
        )),
        (Err(e), Err(c)) => Err(format!(
            "{e}\n収集設定も復元できません。接続を閉じてください: {c}"
        )),
    }
}

impl Database {
    pub(super) fn oracle_actual(
        &mut self,
        statement: &str,
        cancel: &AtomicBool,
    ) -> Result<crate::plan::Plan> {
        // Preflight privileges before executing user SQL (which may modify data).
        self.query("SELECT SQL_ID,CHILD_NUMBER,LAST_OUTPUT_ROWS,LAST_ELAPSED_TIME FROM V$SQL_PLAN_STATISTICS_ALL WHERE 1=0",cancel)?;
        let before = first(&self.query(
            "SELECT VALUE FROM V$PARAMETER WHERE NAME='statistics_level'",
            cancel,
        )?)?[0]
            .display()
            .trim()
            .to_ascii_uppercase();
        if !matches!(before.as_str(), "ALL" | "TYPICAL" | "BASIC") {
            return Err("statistics_levelの復元値が不正です".into());
        }
        self.query(
            "SELECT PREV_SQL_ID,PREV_CHILD_NUMBER FROM V$SESSION WHERE 1=0",
            cancel,
        )?;
        self.query("ALTER SESSION SET STATISTICS_LEVEL=ALL", cancel)?;
        let mut executed = false;
        let result = (|| {
            if cancel.load(Ordering::Relaxed) {
                return Err("取り消されました".into());
            }
            // Keep the tracking token in the hint alongside the statistics hint.
            // Isolate shared-cursor LAST_* counters from simultaneous identical user SQL.
            let sql = oracle_tag(statement, &marker())?;
            executed = true;
            self.query(&sql, cancel)?;
            let cursor=first(&self.query("SELECT PREV_SQL_ID,PREV_CHILD_NUMBER FROM V$SESSION WHERE SID=TO_NUMBER(SYS_CONTEXT('USERENV','SID'))",cancel)?).map_err(|e|format!("実行済みOracleカーソルの識別: {e}"))?;
            if cursor[0] == Value::Null || cursor[1] == Value::Null {
                return Err("実行済みカーソルのSQL_IDを取得できません".into());
            }
            let sql_id = literal(&cursor[0].display());
            let child = cursor[1]
                .display()
                .parse::<u64>()
                .map_err(|e| e.to_string())?;
            let sets=self.query(&format!("SELECT ID,PARENT_ID,OPERATION || ' ' || OPTIONS AS OPERATOR,CASE WHEN OBJECT_NAME IS NULL THEN NULL ELSE OBJECT_OWNER || '.' || OBJECT_NAME END AS OBJECT_NAME,CARDINALITY,COST,ACCESS_PREDICATES,FILTER_PREDICATES,BYTES,TIME,PARTITION_START,PARTITION_STOP,DISTRIBUTION,OBJECT_TYPE,LAST_STARTS,LAST_OUTPUT_ROWS,LAST_ELAPSED_TIME,LAST_CR_BUFFER_GETS,LAST_CU_BUFFER_GETS,LAST_DISK_READS,LAST_DISK_WRITES,LAST_MEMORY_USED,LAST_EXECUTION,LAST_TEMPSEG_SIZE FROM V$SQL_PLAN_STATISTICS_ALL WHERE SQL_ID={sql_id} AND CHILD_NUMBER={child} ORDER BY ID"),cancel)?;
            let mut plan = crate::plan::Plan::tabular(
                DbKind::Oracle,
                sets.first().ok_or("カーソル計画がありません")?,
            )?;
            for node in &mut plan.nodes {
                node.actual_rows = property(node, "LAST_OUTPUT_ROWS");
                if let Some(starts) = property(node, "LAST_STARTS") {
                    node.properties
                        .insert("Actual Starts".into(), starts.to_string());
                }
                if let Some(us) = property(node, "LAST_ELAPSED_TIME") {
                    node.properties
                        .insert("Actual Elapsed ms".into(), (us / 1000.).to_string());
                }
                if property(node, "LAST_TEMPSEG_SIZE").is_some_and(|n| n > 0.)
                    || node
                        .properties
                        .get("LAST_EXECUTION")
                        .is_some_and(|s| matches!(s.as_str(), "ONE PASS" | "MULTI-PASS"))
                {
                    node.properties.insert(
                        "Warnings".into(),
                        "ディスク退避（Oracleカーソル統計）".into(),
                    );
                }
                node.properties
                    .insert("Cursor.SQL_ID".into(), cursor[0].display());
                node.properties
                    .insert("Cursor.CHILD_NUMBER".into(), child.to_string());
            }
            if !plan.nodes.iter().any(|n| {
                n.actual_rows.is_some() && property(n, "LAST_STARTS").is_some_and(|v| v > 0.)
            }) {
                return Err("実行は完了しましたがカーソルの実測統計を取得できません。再実行は自動で行いません".into());
            }
            plan.actual = true;
            crate::plan_visual::validate(&plan)?;
            Ok(plan)
        })();
        let cleanup = self.query(
            &format!("ALTER SESSION SET STATISTICS_LEVEL={before}"),
            &AtomicBool::new(false),
        );
        restored(result, cleanup).map_err(|e| {
            if executed {
                format!("SQL実行を要求した後に失敗しました。再実行する前に結果を確認してください。自動再実行は行いません。\n{e}")
            } else {
                e
            }
        })
    }

    pub(super) fn db2_actual(
        &mut self,
        statement: &str,
        cancel: &AtomicBool,
    ) -> Result<crate::plan::Plan> {
        let table = format!("SESSION.{}", marker());
        // Fail before executing user SQL when the DB lacks a usable user temporary tablespace.
        self.query(&format!("DECLARE GLOBAL TEMPORARY TABLE {table} (R VARCHAR(128), T TIMESTAMP, N VARCHAR(128), S VARCHAR(128), V VARCHAR(64)) ON COMMIT PRESERVE ROWS NOT LOGGED"), cancel)
            .map_err(|e| format!("Db2実測収集の一時表を準備できません。USER TEMPORARY表スペースとUSE権限を確認してください。SQLは実行していません。\n{e}"))?;
        let result = self.db2_actual_with_table(statement, cancel, &table);
        let cleanup = self.query(&format!("DROP TABLE {table}"), &AtomicBool::new(false));
        restored(result, cleanup)
    }
    fn db2_actual_with_table(
        &mut self,
        statement: &str,
        cancel: &AtomicBool,
        key_table: &str,
    ) -> Result<crate::plan::Plan> {
        let targets=self.query("SELECT TABSCHEMA,TABNAME FROM SYSCAT.EVENTTABLES WHERE EVMONNAME='YYDBMANAGER_ACTIVITY' AND LOGICAL_GROUP='ACTIVITYSTMT'",cancel)?;
        let table=first(&targets).map_err(|_|"Db2の実測取得には稼働中のYYDBMANAGER_ACTIVITYモニターが必要です。実行計画メニューの『Db2実測収集の準備SQL』をDB管理者が実行してください".to_owned())?;
        let state = first(&self.query("VALUES EVENT_MON_STATE('YYDBMANAGER_ACTIVITY')", cancel)?)?;
        if state[0].display().trim() != "1" {
            return Err(
                "YYDBMANAGER_ACTIVITYモニターが停止しています。準備SQLで起動してください".into(),
            );
        }
        self.query(
            "SELECT OPERATOR_ID,ACTUAL_TYPE,ACTUAL_VALUE FROM EXPLAIN_ACTUALS WHERE 1=0",
            cancel,
        )?;
        let before=first(&self.query("SELECT XMLSERIALIZE(XMLPARSE(DOCUMENT DETAILS) AS VARCHAR(8192)) FROM TABLE(SYSPROC.WLM_GET_CONN_ENV(CAST(NULL AS BIGINT))) AS E",cancel)?).map_err(|e|format!("WLM設定取得: {e}"))?[0].display();
        let restore = connection_environment(&before)?;
        let appl = first(&self.query("VALUES APPLICATION_ID()", cancel)?)
            .map_err(|e| format!("Application ID取得: {e}"))?[0]
            .display();
        let token = marker();
        let activity_table = format!(
            "{}.{}",
            crate::sql::identifier(&table[0].display())?,
            crate::sql::identifier(&table[1].display())?
        );
        // Also preflight read permission on the target before executing the statement.
        self.query(
            &format!("SELECT APPL_ID,UOW_ID,ACTIVITY_ID,STMT_TEXT FROM {activity_table} WHERE 1=0"),
            cancel,
        )?;
        self.query("CALL SYSPROC.WLM_SET_CONN_ENV(NULL,'<collectactdata>WITH DETAILS, SECTION</collectactdata><collectactpartition>ALL</collectactpartition><collectsectionactuals>BASE</collectsectionactuals>')",cancel)?;
        let execution = if cancel.load(Ordering::Relaxed) {
            Err("取り消されました".into())
        } else {
            self.query(
                &format!(
                    "{statement}
/*{token}*/"
                ),
                cancel,
            )
            .map_err(|e| {
                format!(
                    "SQL実行を要求した後に失敗しました。再実行する前に結果を確認してください。\n{e}"
                )
            })
        };
        let cleanup = self.query(
            &format!("CALL SYSPROC.WLM_SET_CONN_ENV(NULL,{})", literal(&restore)),
            &AtomicBool::new(false),
        );
        restored(execution, cleanup)?;
        let result = (|| {
            // A completed event can still be buffered; flush the already configured monitor only.
            self.query("FLUSH EVENT MONITOR YYDBMANAGER_ACTIVITY", cancel)?;
            let lookup = format!(
                "SELECT UOW_ID,ACTIVITY_ID FROM {activity_table} WHERE APPL_ID={} AND LOCATE('{token}',STMT_TEXT)>0 ORDER BY UOW_ID DESC,ACTIVITY_ID DESC FETCH FIRST 1 ROW ONLY",
                literal(appl.trim())
            );
            let started = std::time::Instant::now();
            let activity = loop {
                let sets = self.query(&lookup, cancel)?;
                if let Ok(row) = first(&sets) {
                    break row;
                }
                if cancel.load(Ordering::Relaxed) {
                    return Err("SQLは実行済みです。統計取得の待機を取り消しました".into());
                }
                if started.elapsed() > std::time::Duration::from_secs(10) {
                    return Err("SQLは実行済みですがactivityモニターへの記録が10秒以内に完了しませんでした。再実行は自動で行いません".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
                self.query("FLUSH EVENT MONITOR YYDBMANAGER_ACTIVITY", cancel)?;
            };
            let uow = activity[0]
                .display()
                .parse::<u64>()
                .map_err(|e| e.to_string())?;
            let id = activity[1]
                .display()
                .parse::<u64>()
                .map_err(|e| e.to_string())?;
            let schema = first(&self.query("VALUES CURRENT SCHEMA", cancel)?)?[0].display();
            let stmt = explain_activity_key(self, &appl, uow, id, &schema, key_table)?;
            self.db2_captured_plan(&stmt, cancel)
        })();
        result.map_err(|e| {
            format!("SQL実行を要求した後に失敗しました。再実行する前に結果を確認してください。自動再実行は行いません。\n{e}")
        })
    }
    fn db2_captured_plan(&mut self, stmt: &str, cancel: &AtomicBool) -> Result<crate::plan::Plan> {
        let key = |a: &str| {
            [
                "EXPLAIN_REQUESTER",
                "EXPLAIN_TIME",
                "SOURCE_NAME",
                "SOURCE_SCHEMA",
                "SOURCE_VERSION",
                "EXPLAIN_LEVEL",
                "STMTNO",
                "SECTNO",
            ]
            .iter()
            .map(|k| format!("{a}.{k}=t.{k}"))
            .collect::<Vec<_>>()
            .join(" AND ")
        };
        let sets=self.query(&format!("WITH stmt AS ({stmt}), ops AS (SELECT o.* FROM EXPLAIN_OPERATOR o JOIN stmt t ON {}), streams AS (SELECT s.* FROM EXPLAIN_STREAM s JOIN stmt t ON {}) SELECT o.OPERATOR_ID,(SELECT MIN(s.TARGET_ID) FROM streams s WHERE s.SOURCE_TYPE='O' AND s.TARGET_TYPE='O' AND s.SOURCE_ID=o.OPERATOR_ID) AS PARENT_ID,o.OPERATOR_TYPE,(SELECT MIN(s.OBJECT_SCHEMA || '.' || s.OBJECT_NAME) FROM streams s WHERE s.TARGET_ID=o.OPERATOR_ID AND s.SOURCE_TYPE='D') AS OBJECT_NAME,(SELECT MAX(s.STREAM_COUNT) FROM streams s WHERE s.SOURCE_ID=o.OPERATOR_ID AND s.SOURCE_TYPE='O') AS ESTIMATED_ROWS,o.TOTAL_COST,o.IO_COST,o.CPU_COST FROM ops o ORDER BY o.OPERATOR_ID",key("o"),key("s")),cancel)?;
        let mut plan = crate::plan::Plan::tabular(
            DbKind::Db2,
            sets.first().ok_or("section計画がありません")?,
        )?;
        let sets=self.query(&format!("WITH stmt AS ({stmt}) SELECT a.OPERATOR_ID,a.DBPARTITIONNUM,a.PREDICATE_ID,a.ACTUAL_TYPE,a.ACTUAL_VALUE FROM EXPLAIN_ACTUALS a JOIN stmt t ON {} ORDER BY a.OPERATOR_ID,a.DBPARTITIONNUM",key("a")),cancel)?;
        let rows = sets
            .first()
            .ok_or("section actualsがありません")?
            .page(0, 10_000)?;
        for row in &rows {
            let id = row[0].display().parse::<u64>().map_err(|e| e.to_string())?;
            let Some(node) = plan.nodes.iter_mut().find(|n| n.id == id) else {
                continue;
            };
            let kind = row[3].display().trim().to_owned();
            node.properties.insert(
                format!("Actual.{}.{}", row[1].display(), kind),
                row[4].display(),
            );
            if row[2] == Value::Null
                && kind == "CARDINALITY"
                && let Ok(n) = row[4].display().trim().parse::<f64>()
                && n.is_finite()
                && n >= 0.
            {
                node.actual_rows = Some(node.actual_rows.unwrap_or(0.) + n);
            }
        }
        let predicates=self.query(&format!("WITH stmt AS ({stmt}) SELECT p.OPERATOR_ID,p.PREDICATE_ID,p.HOW_APPLIED,CAST(p.PREDICATE_TEXT AS VARCHAR(32672)),OCTET_LENGTH(p.PREDICATE_TEXT) FROM EXPLAIN_PREDICATE p JOIN stmt t ON {} ORDER BY p.OPERATOR_ID,p.PREDICATE_ID",key("p")),cancel)?;
        let predicates = predicates
            .first()
            .ok_or("述語の結果がありません")?
            .page(0, 10_000)?;
        for row in &predicates {
            if row
                .get(4)
                .is_some_and(|v| v.display().parse::<u64>().is_ok_and(|n| n > 32672))
            {
                return Err(
                    "Db2の述語サイズ上限（32672バイト）を超えています。保存計画を使用してください"
                        .into(),
                );
            }
            if let Ok(id) = row[0].display().parse::<u64>()
                && let Some(node) = plan.nodes.iter_mut().find(|n| n.id == id)
            {
                node.properties.insert(
                    format!(
                        "Predicate.{}.{}",
                        row[2].display().trim(),
                        row[1].display().trim()
                    ),
                    row[3].display(),
                );
            }
        }
        plan.raw=serde_json::to_string(&serde_json::json!({"operators":serde_json::from_str::<serde_json::Value>(&plan.raw).map_err(|e|e.to_string())?,"actuals":rows,"predicates":predicates})).map_err(|e|e.to_string())?;
        if !plan.nodes.iter().any(|n| n.actual_rows.is_some()) {
            return Err("SQLは実行しましたが演算子のCARDINALITYを取得できません。収集モニターと行編成表のsection actualsを確認してください。再実行は自動で行いません".into());
        }
        plan.actual = true;
        crate::plan_visual::validate(&plan)?;
        Ok(plan)
    }
}
fn property(node: &crate::plan::Node, key: &str) -> Option<f64> {
    node.properties
        .get(key)?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n >= 0.)
}
fn connection_environment(xml: &str) -> Result<String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
    let mut out = String::new();
    for (key, values) in [
        (
            "collectactdata",
            &[
                "NONE",
                "WITHOUT DETAILS",
                "WITH DETAILS",
                "WITH DETAILS, SECTION",
                "WITH DETAILS, SECTION AND VALUES",
                "WITH DETAILS AND VALUES",
            ][..],
        ),
        ("collectactpartition", &["COORDINATOR", "ALL"][..]),
        ("collectsectionactuals", &["NONE", "BASE"][..]),
    ] {
        let value = doc
            .descendants()
            .find(|n| n.has_tag_name(key))
            .and_then(|n| n.text())
            .ok_or("WLM収集設定を復元できません")?
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !values.contains(&value.as_str()) {
            return Err("WLM収集設定の値が不正です".into());
        }
        out.push_str(&format!("<{key}>{value}</{key}>"));
    }
    Ok(out)
}

// The SQL block receives procedure OUT values on the server. The exact returned
// statement key crosses the wire through a connection-local temporary table.
fn explain_activity_key(
    db: &mut Database,
    appl: &str,
    uow: u64,
    id: u64,
    schema: &str,
    table: &str,
) -> Result<String> {
    let cancel = AtomicBool::new(false);
    let block = format!(
        "BEGIN ATOMIC
            DECLARE e VARCHAR(128) DEFAULT {};
            DECLARE r VARCHAR(128);
            DECLARE t TIMESTAMP;
            DECLARE n VARCHAR(128);
            DECLARE s VARCHAR(128);
            DECLARE v VARCHAR(64);
            CALL SYSPROC.EXPLAIN_FROM_ACTIVITY({}, {uow}, {id}, {}, e, r, t, n, s, v);
            INSERT INTO {table} VALUES (r,t,n,s,v);
            END",
        literal(schema),
        literal(appl),
        literal("YYDBMANAGER_ACTIVITY")
    );
    // query sends a single compound statement without the SQL editor splitter.
    db.query(&block, &cancel)?;
    let row = first(&db.query(&format!("SELECT R,CHAR(T),N,S,V FROM {table}"), &cancel)?)?;
    if row.len() != 5 || row.contains(&Value::Null) {
        return Err("EXPLAIN_FROM_ACTIVITYが完全なステートメントキーを返しませんでした".into());
    }
    Ok(format!(
        "SELECT * FROM EXPLAIN_STATEMENT WHERE EXPLAIN_REQUESTER={} AND EXPLAIN_TIME=TIMESTAMP({}) AND SOURCE_NAME={} AND SOURCE_SCHEMA={} AND SOURCE_VERSION={}",
        literal(&row[0].display()),
        literal(&row[1].display()),
        literal(&row[2].display()),
        literal(&row[3].display()),
        literal(&row[4].display())
    ))
}

fn oracle_tag(sql: &str, token: &str) -> Result<String> {
    let bytes = sql.as_bytes();
    let mut i = 0;
    loop {
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if sql[i..].starts_with("--") {
            i = sql[i..].find('\n').map(|n| i + n + 1).unwrap_or(sql.len());
            continue;
        }
        if sql[i..].starts_with("/*") {
            i = sql[i + 2..]
                .find("*/")
                .map(|n| i + n + 4)
                .ok_or("SQLコメントが閉じていません")?;
            continue;
        }
        break;
    }
    let start = i;
    while bytes.get(i).is_some_and(u8::is_ascii_alphabetic) {
        i += 1;
    }
    if start == i {
        return Err("Oracle実測計画のSQL先頭キーワードを識別できません".into());
    }
    let mut next = i;
    while bytes.get(next).is_some_and(u8::is_ascii_whitespace) {
        next += 1;
    }
    if sql[next..].starts_with("/*+") {
        let close = sql[next + 3..]
            .find("*/")
            .map(|n| next + n + 3)
            .ok_or("SQLヒントが閉じていません")?;
        return Ok(format!("{} {token} {}", &sql[..close], &sql[close..]));
    }
    if sql[next..].starts_with("--+") {
        let end = sql[next..]
            .find('\n')
            .map(|n| next + n)
            .unwrap_or(sql.len());
        return Ok(format!("{} {token}{}", &sql[..end], &sql[end..]));
    }
    Ok(format!("{} /*+ {token} */ {}", &sql[..i], &sql[i..]))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oracle_cursor_tag_preserves_hint_and_unicode_comments() {
        assert_eq!(
            oracle_tag("-- 日本語\nSELECT /*+ FULL(t) */ * FROM t", "YYDB1").unwrap(),
            "-- 日本語\nSELECT /*+ FULL(t)  YYDB1 */ * FROM t"
        );
        assert!(
            oracle_tag(
                "/*説明*/ WITH q AS(SELECT 1 FROM dual) SELECT * FROM q",
                "YYDB1"
            )
            .unwrap()
            .contains("WITH /*+ YYDB1 */")
        );
        assert!(oracle_tag("-- no SQL", "YYDB1").is_err());
    }
    #[test]
    fn restore_whitelists_xml_and_reports_both_errors() {
        let xml = "<wlm_conn_env xmlns='urn:db2'><collectactdata>WITH DETAILS, SECTION</collectactdata><collectactpartition>ALL</collectactpartition><collectsectionactuals>BASE</collectsectionactuals></wlm_conn_env>";
        assert!(
            connection_environment(xml)
                .unwrap()
                .contains("<collectsectionactuals>BASE</collectsectionactuals>")
        );
        assert!(connection_environment(&xml.replace("BASE", "UNKNOWN")).is_err());
        let error = restored::<()>(Err("target".into()), Err("cleanup".into())).unwrap_err();
        assert!(error.contains("target") && error.contains("cleanup"));
    }
}

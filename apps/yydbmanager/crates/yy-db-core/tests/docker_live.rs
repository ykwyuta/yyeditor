use serde::Deserialize;
use std::{
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
use yy_db_core::{Database, DbKind, Options, Value};

#[derive(Deserialize)]
struct Connection {
    kind: DbKind,
    connection: String,
}

/// Explicitly enabled so missing database services are never reported as a pass.
#[test]
#[ignore = "requires the isolated Docker database stack and YYDB_CONNECTIONS"]
fn all_five_remote_databases() {
    let config = std::env::var("YYDB_CONNECTIONS").expect("YYDB_CONNECTIONS is required");
    let mut connections: Vec<Connection> =
        serde_json::from_str(&std::fs::read_to_string(config).unwrap()).unwrap();
    if let Ok(filter) = std::env::var("YYDB_DATABASE_FILTER") {
        connections.retain(|c| c.kind.name() == filter);
    }
    let expected = std::env::var("YYDB_EXPECT_DATABASES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(5);
    assert_eq!(connections.len(), expected);
    let directory =
        PathBuf::from(std::env::var("YYDB_REPORT_DIR").unwrap_or_else(|_| "/build/reports".into()));
    std::fs::create_dir_all(&directory).unwrap();
    let mut reports = vec![];
    let mut failures = vec![];
    for connection in connections {
        let kind = connection.kind;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            validate(&connection, &directory)
        }))
        .unwrap_or_else(|_| {
            Err("ODBCドライバーで例外が発生しました。64bit SQLLENのABIを確認してください".into())
        });
        match result {
            Ok(summary) => {
                println!("{}: {summary}", kind.name());
                reports.push(
                    serde_json::json!({"database":kind.name(),"status":"passed","summary":summary}),
                );
            }
            Err(error) => {
                println!("{}: FAILED {error}", kind.name());
                failures.push(format!("{}: {error}", kind.name()));
                reports.push(
                    serde_json::json!({"database":kind.name(),"status":"failed","error":error}),
                );
            }
        }
    }
    std::fs::write(
        directory.join("remote-databases.json"),
        serde_json::to_vec_pretty(&reports).unwrap(),
    )
    .unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
fn complex_portable_plan(
    db: &mut Database,
    kind: DbKind,
    directory: &std::path::Path,
    cancel: &AtomicBool,
) -> yy_db_core::Result<()> {
    for table in ["YYDB_PLAN_A", "YYDB_PLAN_B"] {
        let _ = db.execute(&format!("DROP TABLE {table}"), cancel);
        db.execute(
            &format!(
                "CREATE TABLE {table}(ID INTEGER NOT NULL PRIMARY KEY, GRP INTEGER, AMOUNT INTEGER)"
            ),
            cancel,
        )?;
        let source = match kind {
            DbKind::Oracle => " FROM DUAL",
            DbKind::Db2 => " FROM SYSIBM.SYSDUMMY1",
            _ => "",
        };
        let rows = (1..=80)
            .map(|n| format!("SELECT {n},{},{}{source}", n % 5, n * 7))
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        db.execute(&format!("INSERT INTO {table} {rows}"), cancel)?;
    }
    let sql = "SELECT a.GRP, COUNT(*) AS N, SUM(b.AMOUNT) AS TOTAL FROM YYDB_PLAN_A a JOIN YYDB_PLAN_B b ON a.ID=b.ID WHERE b.AMOUNT>10 GROUP BY a.GRP ORDER BY SUM(b.AMOUNT) DESC";
    for actual in [false, true] {
        let plan = db.explain(sql, actual, cancel)?;
        yy_db_core::plan_visual::validate(&plan)?;
        if plan.nodes.len() < 4 || !plan.nodes.iter().any(|n| n.parent.is_some()) {
            return Err(format!("{}: 複合計画の構造が不足しています", kind.name()));
        }
        if actual && !plan.nodes.iter().any(|n| n.actual_rows.is_some()) {
            return Err("実測行数がありません".into());
        }
        let name = format!(
            "{}-complex-{}",
            kind.name(),
            if actual { "actual" } else { "estimated" }
        );
        std::fs::write(
            directory.join(format!("{name}.yyplan")),
            serde_json::to_vec_pretty(&plan).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        let doc = plan.drawing();
        std::fs::write(
            directory.join(format!("{name}.png")),
            yy_db_core::plan::drawing_renderer().png(
                &doc,
                &yy_db_core::plan::drawing_export(&doc),
                1.,
            )?,
        )
        .map_err(|e| e.to_string())?;
    }
    if kind == DbKind::MySql {
        db.execute("SET SESSION explain_json_format_version=2", cancel)?;
        let plan = db.explain(sql, false, cancel)?;
        db.execute("SET SESSION explain_json_format_version=1", cancel)?;
        if plan.nodes.len() < 4 || !plan.nodes.iter().any(|n| n.estimated_rows.is_some()) {
            return Err("MySQL JSON v2の構造・統計がありません".into());
        }
        std::fs::write(
            directory.join("MySQL-complex-json-v2.yyplan"),
            serde_json::to_vec_pretty(&plan).unwrap(),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn validate(config: &Connection, directory: &std::path::Path) -> yy_db_core::Result<String> {
    let options = Options {
        kind: config.kind,
        path: None,
        connection_string: config.connection.clone(),
        read_only: false,
    };
    let started = Instant::now();
    let mut db = loop {
        match Database::open(&options) {
            Ok(db) => break db,
            Err(error) => {
                if error.contains("SQLAllocHandle")
                    || error.contains("Can't open lib")
                    || error.contains("IM002")
                {
                    return Err(error);
                }
                if started.elapsed() > Duration::from_secs(300) {
                    return Err(error);
                }
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    };
    let cancel = AtomicBool::new(false);
    let version_sql = match config.kind {
        DbKind::SqlServer => "SELECT @@VERSION",
        DbKind::Oracle => "SELECT banner FROM v$version WHERE rownum=1",
        DbKind::Db2 => "SELECT service_level FROM TABLE(sysproc.env_get_inst_info()) AS t",
        _ => "SELECT version()",
    };
    let version = db
        .execute(version_sql, &cancel)?
        .first()
        .ok_or("サーバー版を取得できません")?
        .page(0, 1)?
        .first()
        .and_then(|r| r.first())
        .map(|v| v.display())
        .ok_or("サーバー版がありません")?;
    let table = "YYDB_VALIDATION";
    // The table exists only in this disposable database stack.
    let _ = db.execute(&format!("DROP TABLE {table}"), &cancel);
    let varchar = if config.kind == DbKind::SqlServer {
        "NVARCHAR(100)"
    } else if config.kind == DbKind::Oracle {
        "VARCHAR2(100 CHAR)"
    } else {
        "VARCHAR(100)"
    };
    db.execute(
        &format!(
            "CREATE TABLE {table} (ID INTEGER NOT NULL PRIMARY KEY, AMOUNT DECIMAL(31,8), LABEL {varchar})"
        ),
        &cancel,
    )?;
    let label = if config.kind == DbKind::SqlServer {
        "N'日本語'"
    } else {
        "'日本語'"
    };
    db.execute(
        &format!("INSERT INTO {table} VALUES (1,12345678901234567890123.12345678,{label})"),
        &cancel,
    )?;
    db.execute(&format!("INSERT INTO {table} VALUES (2,NULL,'')"), &cancel)?;
    let results = db.execute(
        &format!("SELECT ID, AMOUNT, LABEL FROM {table} ORDER BY ID"),
        &cancel,
    )?;
    let set = results.first().ok_or("SELECT結果がありません")?;
    let rows = set.page(0, 10)?;
    if rows.len() != 2 {
        return Err(format!("結果の行数が不正です: {}", rows.len()));
    }
    if rows[0][1] != Value::Decimal("12345678901234567890123.12345678".into()) {
        return Err(format!("十進精度の不一致: {:?}", rows[0][1]));
    }
    if rows[0][2].display() != "日本語" {
        return Err(format!("日本語の不一致: {:?}", rows[0][2]));
    }
    if rows[1][1] != Value::Null {
        return Err("SQL NULLが保持されていません".into());
    }
    db.tables()?;
    let objects = db.catalog()?;
    let object = objects
        .iter()
        .find(|o| o.kind == "TABLE" && o.name.eq_ignore_ascii_case(table))
        .ok_or("オブジェクト一覧に検証表がありません")?;
    let columns = db.object_columns(&object.schema, &object.name)?;
    if columns.len() != 3
        || !columns
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case("AMOUNT"))
    {
        return Err("列メタデータが一致しません".into());
    }
    if config.kind == DbKind::Db2 {
        let counts=db.execute("SELECT COUNT(*) FROM SYSCAT.TABLES WHERE TABSCHEMA=CURRENT SCHEMA AND TABNAME='EXPLAIN_OPERATOR'",&cancel)?;
        if counts[0].page(0, 1)?[0][0].display().trim() == "0" {
            db.execute("CALL SYSPROC.SYSINSTALLOBJECTS('EXPLAIN','C',CAST(NULL AS VARCHAR(128)),CURRENT SCHEMA)",&cancel)?;
        }
        let monitor = db.execute(
            "SELECT COUNT(*) FROM SYSCAT.EVENTMONITORS WHERE EVMONNAME='YYDBMANAGER_ACTIVITY'",
            &cancel,
        )?;
        if monitor[0].page(0, 1)?[0][0].display().trim() == "0" {
            db.execute("CREATE EVENT MONITOR YYDBMANAGER_ACTIVITY FOR ACTIVITIES WRITE TO TABLE MANUALSTART",&cancel)?;
        }
        db.execute("SET EVENT MONITOR YYDBMANAGER_ACTIVITY STATE 1", &cancel)?;
    }
    let imported_table = yy_db_core::sql::identifier_for(config.kind, "YYDB_IMPORTED")?;
    let _ = db.execute(&format!("DROP TABLE {imported_table}"), &cancel);
    db.import_into(set, "YYDB_IMPORTED", true)?;
    let imported = db.execute(
        &format!("SELECT * FROM {imported_table} ORDER BY 1"),
        &cancel,
    )?;
    let imported_rows = imported
        .first()
        .ok_or("投入後の結果がありません")?
        .page(0, 10)?;
    if imported_rows.len() != 2
        || imported_rows[0][1] != rows[0][1]
        || imported_rows[0][2].display() != "日本語"
    {
        return Err(format!("バインド投入の不一致: {imported_rows:?}"));
    }
    let csv = directory.join(format!("{}-result.csv", config.kind.name()));
    yy_db_core::transfer::export_delimited(set, &csv, &Default::default())?;
    let csv_rows = yy_db_core::transfer::read_delimited(&csv, &Default::default())?.page(0, 10)?;
    if csv_rows.len() != 2
        || csv_rows[0][1].display() != rows[0][1].display()
        || csv_rows[0][2].display() != "日本語"
        || csv_rows[1][1] != Value::Null
    {
        return Err("CSV往復の値が一致しません".into());
    }
    let parquet = directory.join(format!("{}-result.parquet", config.kind.name()));
    yy_db_core::transfer::export_parquet(set, &parquet)?;
    let parquet_rows = yy_db_core::transfer::read_parquet(&parquet)?.page(0, 10)?;
    // Integer columns are stored as DECIMAL(38,0), preserving their exact values.
    if parquet_rows.len() != rows.len()
        || !parquet_rows.iter().zip(&rows).all(|(actual, expected)| {
            actual.len() == expected.len()
                && actual.iter().zip(expected).all(|(a, e)| {
                    (a == &Value::Null) == (e == &Value::Null) && a.display() == e.display()
                })
        })
    {
        return Err("Parquet往復の値が一致しません".into());
    }
    let mut summary =
        "connect, DDL/DML, exact DECIMAL(31,8), Unicode, NULL, catalog and columns, bound import, CSV and Parquet OK"
            .to_owned();
    summary = format!("{version}: {summary}");
    {
        let plan = db.explain(&format!("SELECT * FROM {table} WHERE ID=1"), false, &cancel)?;
        if plan.nodes.is_empty() {
            return Err("推定計画にノードがありません".into());
        }
        std::fs::write(
            directory.join(format!("{}-plan.json", config.kind.name())),
            serde_json::to_vec_pretty(&plan).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        summary.push_str(", estimated plan OK");
        if matches!(
            config.kind,
            DbKind::PostgreSql | DbKind::SqlServer | DbKind::MySql | DbKind::Oracle | DbKind::Db2
        ) {
            let before = collection_setting(&mut db, config.kind, &cancel)?;
            let actual = db.explain(&format!("SELECT * FROM {table} WHERE ID=1"), true, &cancel)?;
            if collection_setting(&mut db, config.kind, &cancel)? != before {
                return Err("実測取得後に収集設定が戻っていません".into());
            }
            if matches!(config.kind, DbKind::Oracle | DbKind::Db2) {
                if config.kind == DbKind::Oracle {
                    let again =
                        db.explain(&format!("SELECT * FROM {table} WHERE ID=1"), true, &cancel)?;
                    if actual.nodes[0].properties.get("Cursor.SQL_ID")
                        == again.nodes[0].properties.get("Cursor.SQL_ID")
                    {
                        return Err("Oracleの実測カーソルが実行ごとに分離されていません".into());
                    }
                    if !actual
                        .nodes
                        .iter()
                        .any(|n| n.object.contains("YYDB_VALIDATION"))
                    {
                        return Err("Oracle実測カーソルの対象が異なります".into());
                    }
                }
                if db
                    .explain("SELECT * FROM YYDB_MISSING_ACTUAL_TABLE", true, &cancel)
                    .is_ok()
                {
                    return Err("不正SQLの実測が成功しました".into());
                }
                if collection_setting(&mut db, config.kind, &cancel)? != before {
                    return Err("失敗後に収集設定が戻っていません".into());
                }
                if db
                    .explain(
                        "SELECT * FROM YYDB_VALIDATION",
                        true,
                        &AtomicBool::new(true),
                    )
                    .is_ok()
                {
                    return Err("取消済みの実測が成功しました".into());
                }
                if collection_setting(&mut db, config.kind, &cancel)? != before {
                    return Err("取消後に収集設定が戻っていません".into());
                }
                let _ = db.execute("DROP TABLE YYDB_EXECUTION_ONCE", &cancel);
                db.execute("CREATE TABLE YYDB_EXECUTION_ONCE(N INTEGER)", &cancel)?;
                db.execute("INSERT INTO YYDB_EXECUTION_ONCE VALUES(0)", &cancel)?;
                let _ = db.explain("UPDATE YYDB_EXECUTION_ONCE SET N=N+1", true, &cancel);
                let n = db.execute("SELECT N FROM YYDB_EXECUTION_ONCE", &cancel)?;
                if n[0].page(0, 1)?[0][0].display().trim() != "1" {
                    return Err("実測SQLの実行回数が1回ではありません".into());
                }
                if collection_setting(&mut db, config.kind, &cancel)? != before {
                    return Err("DML後に収集設定が戻っていません".into());
                }
            }
            if !actual.nodes.iter().any(|n| n.actual_rows.is_some()) {
                return Err("実測計画に実測行数がありません".into());
            }
            std::fs::write(
                directory.join(format!("{}-actual-plan.json", config.kind.name())),
                serde_json::to_vec_pretty(&actual).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            summary.push_str(", actual plan OK");
        }
    }
    if std::env::var_os("YYDB_COMPLEX_PLANS").is_some() {
        complex_portable_plan(&mut db, config.kind, directory, &cancel)?;
        summary.push_str(", complex join/aggregate/sort plan OK");
    }
    if config.kind == DbKind::SqlServer && std::env::var_os("YYDB_COMPLEX_PLANS").is_some() {
        db.execute("DROP TABLE IF EXISTS YYDB_PLAN_ORDER; DROP TABLE IF EXISTS YYDB_PLAN_CUSTOMER; CREATE TABLE YYDB_PLAN_CUSTOMER(id int NOT NULL PRIMARY KEY, region varchar(10)); CREATE TABLE YYDB_PLAN_ORDER(id int NOT NULL PRIMARY KEY, customer_id int, amount decimal(12,2)); WITH n AS (SELECT TOP (2000) ROW_NUMBER() OVER (ORDER BY (SELECT NULL)) AS id FROM sys.all_objects) INSERT INTO YYDB_PLAN_CUSTOMER SELECT id, CASE WHEN id%2=0 THEN 'East' ELSE 'West' END FROM n; WITH n AS (SELECT TOP (10000) ROW_NUMBER() OVER (ORDER BY (SELECT NULL)) AS id FROM sys.all_objects a CROSS JOIN sys.all_objects b) INSERT INTO YYDB_PLAN_ORDER SELECT id,1+id%2000,id%10000 FROM n; CREATE INDEX IX_YYDB_PLAN_ORDER_CUSTOMER ON YYDB_PLAN_ORDER(customer_id)",&cancel)?;
        for (name, sql) in [
            (
                "hash-sort",
                "SELECT c.region, COUNT_BIG(*) AS orders, SUM(o.amount) AS amount FROM YYDB_PLAN_CUSTOMER c JOIN YYDB_PLAN_ORDER o ON o.customer_id=c.id WHERE o.amount>100 GROUP BY c.region ORDER BY SUM(o.amount) DESC OPTION(HASH JOIN,MAXDOP 2)",
            ),
            (
                "seek-loops",
                "SELECT c.id,o.amount FROM YYDB_PLAN_CUSTOMER c JOIN YYDB_PLAN_ORDER o ON o.customer_id=c.id WHERE c.id<10 ORDER BY c.id OPTION(LOOP JOIN,MAXDOP 1)",
            ),
        ] {
            let plan = db.explain(sql, true, &cancel)?;
            if plan.nodes.len() < 4 {
                return Err("複合計画の演算子が不足しています".into());
            }
            std::fs::write(
                directory.join(format!("sqlserver-{name}.sqlplan")),
                &plan.raw,
            )
            .map_err(|e| e.to_string())?;
            std::fs::write(
                directory.join(format!("sqlserver-{name}.yyplan")),
                serde_json::to_vec_pretty(&plan).unwrap(),
            )
            .map_err(|e| e.to_string())?;
            let doc = plan.drawing();
            let renderer = yy_db_core::plan::drawing_renderer();
            std::fs::write(
                directory.join(format!("sqlserver-{name}.png")),
                renderer.png(&doc, &yy_db_core::plan::drawing_export(&doc), 1.)?,
            )
            .map_err(|e| e.to_string())?;
        }
        summary.push_str(", complex hash/sort and seek/loops actual plans OK");
    }
    for table in ["YYDB_ER_CHILD", "YYDB_ER_SINGLE", "YYDB_ER_PARENT"] {
        let _ = db.execute(&format!("DROP TABLE {table}"), &cancel);
    }
    db.execute(
        "CREATE TABLE YYDB_ER_PARENT (A INTEGER NOT NULL, B INTEGER NOT NULL, PRIMARY KEY(A,B))",
        &cancel,
    )?;
    db.execute("CREATE TABLE YYDB_ER_CHILD (X INTEGER, Y INTEGER, CONSTRAINT FK_ER_CHILD FOREIGN KEY(X,Y) REFERENCES YYDB_ER_PARENT(A,B))",&cancel)?;
    db.execute("CREATE TABLE YYDB_ER_SINGLE (X INTEGER NOT NULL, Y INTEGER NOT NULL, UNIQUE(X,Y), CONSTRAINT FK_ER_SINGLE FOREIGN KEY(X,Y) REFERENCES YYDB_ER_PARENT(A,B))",&cancel)?;
    let model = db.er_model()?;
    let relation = model
        .relations
        .iter()
        .find(|r| r.child.name.eq_ignore_ascii_case("YYDB_ER_CHILD"))
        .ok_or("ERの複合外部キーがありません")?;
    if relation.columns.len() != 2
        || !relation.columns[0].eq_ignore_ascii_case("X")
        || !relation.referenced_columns[1].eq_ignore_ascii_case("B")
    {
        return Err("ERの複合外部キーの列対応が不正です".into());
    }
    let parent = model
        .entities
        .iter()
        .find(|e| e.id == relation.parent)
        .ok_or("ERの親テーブルがありません")?;
    if !parent
        .keys
        .iter()
        .any(|k| k.primary && k.columns.len() == 2)
    {
        return Err("ERの複合主キーがありません".into());
    }
    let single = model
        .entities
        .iter()
        .find(|e| e.id.name.eq_ignore_ascii_case("YYDB_ER_SINGLE"))
        .ok_or("ERの一意制約の表がありません")?;
    if !single
        .keys
        .iter()
        .any(|k| !k.primary && k.columns.len() == 2)
        || single.fields.iter().any(|f| f.nullable)
    {
        return Err("ERのUNIQUE/NOT NULLが不正です".into());
    }
    model.save(&directory.join(format!("{}-er.yyer", config.kind.name())))?;
    let doc = model.drawing();
    let renderer = yy_db_core::plan::drawing_renderer();
    std::fs::write(
        directory.join(format!("{}-er.png", config.kind.name())),
        renderer.png(&doc, &yy_db_core::plan::drawing_export(&doc), 0.6)?,
    )
    .map_err(|e| e.to_string())?;
    summary.push_str(", ER composite PK/FK, UNIQUE and NOT NULL OK");
    let mut design = model.clone();
    design
        .entities
        .retain(|e| e.id.name.to_ascii_uppercase().starts_with("YYDB_ER_"));
    design.relations.retain(|r| {
        design.entities.iter().any(|e| e.id == r.child)
            && design.entities.iter().any(|e| e.id == r.parent)
    });
    for i in 0..design.entities.len() {
        let mut e = design.entities[i].clone();
        e.id.name =
            e.id.name
                .to_ascii_uppercase()
                .replace("YYDB_ER_", "YYDB_DDL_");
        e.catalog_definition = false;
        for (k, key) in e.keys.iter_mut().enumerate() {
            key.name = format!("{}_{}", e.id.name, k);
        }
        for f in &mut e.fields {
            f.data_type = "INTEGER".into();
        }
        if e.id.name == "YYDB_DDL_CHILD" {
            e.fields.push(yy_db_core::schema::Field {
                important: false,
                name: "NOTE".into(),
                logical_name: "備考".into(),
                data_type: "VARCHAR(40)".into(),
                nullable: true,
                default_sql: Some("'a;b'".into()),
            });
            e.indexes.push(yy_db_core::schema::Index {
                name: "IX_YYDB_DDL_NOTE".into(),
                columns: vec!["NOTE".into()],
                unique: false,
            });
        }
        design.replace_entity(i, e)?;
    }
    for r in &mut design.relations {
        r.name.push_str("_DDL");
    }
    for name in ["YYDB_DDL_CHILD", "YYDB_DDL_SINGLE", "YYDB_DDL_PARENT"] {
        let _ = db.execute(
            &format!(
                "DROP TABLE {}",
                yy_db_core::sql::identifier_for(config.kind, name)?
            ),
            &cancel,
        );
    }
    let ddl = design.ddl()?;
    std::fs::write(
        directory.join(format!("{}-design.sql", config.kind.name())),
        &ddl.sql,
    )
    .map_err(|e| e.to_string())?;
    db.execute(&ddl.sql, &cancel)?;
    let child = yy_db_core::sql::identifier_for(config.kind, "YYDB_DDL_CHILD")?;
    db.execute(
        &format!("INSERT INTO {child}(X,Y) VALUES(NULL,NULL)"),
        &cancel,
    )?;
    let defaults = db.execute(
        &format!(
            "SELECT {} FROM {child}",
            yy_db_core::sql::identifier_for(config.kind, "NOTE")?
        ),
        &cancel,
    )?;
    if defaults[0].page(0, 1)?[0][0].display() != "a;b" {
        return Err("生成DDLの既定値が一致しません".into());
    }
    if db
        .execute(
            &format!("INSERT INTO {child}(X,Y) VALUES(999,999)"),
            &cancel,
        )
        .is_ok()
    {
        return Err("生成DDLの外部キーが機能していません".into());
    }
    if !db
        .catalog()?
        .iter()
        .any(|o| o.kind == "INDEX" && o.name.to_ascii_uppercase().ends_with("IX_YYDB_DDL_NOTE"))
    {
        return Err("生成DDLの索引がカタログにありません".into());
    }
    let parent = yy_db_core::sql::identifier_for(config.kind, "YYDB_DDL_PARENT")?;
    let single = yy_db_core::sql::identifier_for(config.kind, "YYDB_DDL_SINGLE")?;
    db.execute(
        &format!("INSERT INTO {parent}(A,B) VALUES(1,1); INSERT INTO {single}(X,Y) VALUES(1,1)"),
        &cancel,
    )?;
    if db
        .execute(&format!("INSERT INTO {single}(X,Y) VALUES(1,1)"), &cancel)
        .is_ok()
        || db
            .execute(
                &format!("INSERT INTO {single}(X,Y) VALUES(NULL,NULL)"),
                &cancel,
            )
            .is_ok()
    {
        return Err("生成DDLのUNIQUE/NOT NULLが機能していません".into());
    }
    summary.push_str(", generated DDL tables/FKs/index/default verified OK");
    Ok(summary)
}

fn collection_setting(
    db: &mut Database,
    kind: DbKind,
    cancel: &AtomicBool,
) -> yy_db_core::Result<String> {
    let query = match kind {
        DbKind::Oracle => "SELECT VALUE FROM V$PARAMETER WHERE NAME='statistics_level'",
        DbKind::Db2 => {
            "SELECT XMLSERIALIZE(XMLPARSE(DOCUMENT DETAILS) AS CLOB(8192)) FROM TABLE(SYSPROC.WLM_GET_CONN_ENV(CAST(NULL AS BIGINT))) AS E"
        }
        _ => return Ok(String::new()),
    };
    Ok(db.execute(query, cancel)?[0].page(0, 1)?[0][0].display())
}

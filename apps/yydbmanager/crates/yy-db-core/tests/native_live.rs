//! Uses the explicitly started, loopback-only demo databases. Never runs implicitly.
use std::sync::atomic::AtomicBool;
use yy_db_core::{Column, Database, DbKind, Options, ResultSet, Value, native::Config};

#[test]
#[ignore = "requires demo/yydbmanager/compose.yaml databases on loopback"]
fn native_connections_exact_values_transactions_import_and_plans() {
    let filter = std::env::var("YY_NATIVE_FILTER").unwrap_or_default();
    for (kind, port, database, user) in [
        (DbKind::PostgreSql, 25432, "yydemo", "yy_demo"),
        (DbKind::MySql, 23306, "yydemo", "yy_demo"),
        (DbKind::SqlServer, 21433, "yydemo", "sa"),
        (DbKind::Oracle, 21521, "FREEPDB1", "yy_demo"),
        (DbKind::Db2, 25000, "YYDEMO", "db2inst1"),
    ] {
        if !filter.is_empty() && !kind.name().eq_ignore_ascii_case(&filter) {
            continue;
        }
        eprintln!("Native connection: {}", kind.name());
        let config = Config {
            host: "127.0.0.1".into(),
            port,
            database: database.into(),
            username: user.into(),
            password: "YyDemo_2026!".into(),
            tls: "disable".into(),
            ca_file: None,
            trust_server_certificate: false,
        };
        let mut options = Options::memory(kind);
        options.connection_string = config.encode().unwrap();
        let mut db = Database::open(&options).unwrap_or_else(|e| panic!("{}: {e}", kind.name()));
        assert!(matches!(db, Database::Native(..)));
        let cancel = AtomicBool::new(false);
        let mut source = ResultSet::new(vec![
            Column {
                name: "id".into(),
                type_name: "BIGINT".into(),
            },
            Column {
                name: "amount".into(),
                type_name: "DECIMAL(20,4)".into(),
            },
            Column {
                name: "label".into(),
                type_name: "VARCHAR".into(),
            },
            Column {
                name: "payload".into(),
                type_name: "VARBINARY".into(),
            },
        ])
        .unwrap();
        let expected = vec![
            Value::Integer("9223372036854775807".into()),
            Value::Decimal("1234567890123456.1234".into()),
            Value::Text("日本語'引用\\path".into()),
            Value::Binary(vec![0, 255, 65, 127]),
        ];
        source.push(&expected).unwrap();
        source
            .push(&[Value::Null, Value::Null, Value::Null, Value::Null])
            .unwrap();
        source.finish().unwrap();
        let table = format!("yynative_probe_{}", std::process::id());
        assert_eq!(db.import_into(&source, &table, true).unwrap(), 2);
        let table = yy_db_core::sql::identifier_for(kind, &table).unwrap();
        let columns = ["id", "amount", "label", "payload"]
            .iter()
            .map(|s| yy_db_core::sql::identifier_for(kind, s).unwrap())
            .collect::<Vec<_>>()
            .join(",");
        let query = format!(
            "SELECT {columns} FROM {table} WHERE {} IS NOT NULL",
            yy_db_core::sql::identifier_for(kind, "id").unwrap()
        );
        let sets = db.execute(&query, &cancel).unwrap();
        let row = sets[0].page(0, 1).unwrap().remove(0);
        assert_eq!(
            row.iter().map(Value::display).collect::<Vec<_>>(),
            expected.iter().map(Value::display).collect::<Vec<_>>(),
            "{}",
            kind.name()
        );
        let sets = db
            .execute(
                &format!(
                    "SELECT {columns} FROM {table} WHERE {} IS NULL",
                    yy_db_core::sql::identifier_for(kind, "id").unwrap()
                ),
                &cancel,
            )
            .unwrap();
        assert_eq!(sets[0].page(0, 1).unwrap()[0], vec![Value::Null; 4]);
        if !matches!(kind, DbKind::Oracle | DbKind::Db2) {
            db.execute(
                if kind == DbKind::SqlServer {
                    "BEGIN TRANSACTION"
                } else {
                    "BEGIN"
                },
                &cancel,
            )
            .unwrap();
        }
        db.execute(&format!("DELETE FROM {table}"), &cancel)
            .unwrap();
        db.execute("ROLLBACK", &cancel).unwrap();
        assert_eq!(
            db.execute(&format!("SELECT COUNT(*) FROM {table}"), &cancel)
                .unwrap()[0]
                .page(0, 1)
                .unwrap()[0][0]
                .display(),
            "2"
        );
        let plan = db.explain(&query, false, &cancel).unwrap();
        assert!(!plan.nodes.is_empty(), "{}", kind.name());
        if !matches!(kind, DbKind::Db2) {
            let plan = db.explain(&query, true, &cancel).unwrap();
            assert!(plan.actual);
        } else {
            // An unsupported field must still close its cursor, allowing later
            // SQL/DDL to proceed without an implicit commit or reconnect.
            let label = yy_db_core::sql::identifier_for(kind, "label").unwrap();
            assert!(
                db.execute(
                    &format!("SELECT CAST({label} AS CLOB(128)) FROM {table}"),
                    &cancel
                )
                .is_err()
            );
        }
        db.execute(&format!("DROP TABLE {table}"), &cancel).unwrap();
        if matches!(kind, DbKind::Oracle | DbKind::Db2) {
            db.execute("COMMIT", &cancel).unwrap();
        }
        assert!(!db.tables().unwrap().is_empty());
        eprintln!(
            "PASS native {}: exact values, NULL, Unicode, rollback, import, metadata and plans",
            kind.name()
        );
    }
}

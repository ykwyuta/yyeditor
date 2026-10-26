//! Opt-in regression on a disposable Db2 instance, using the application's TLS and cancellation paths.
use std::{
    sync::{atomic::AtomicBool, mpsc},
    thread,
    time::{Duration, Instant},
};
use yy_db_core::{Column, Database, DbKind, Options, ResultSet, Value, native::Config};

#[test]
#[ignore = "requires YYDB_DB2_CONFIG containing a disposable Db2 TLS Config JSON"]
fn tls_bound_import_and_running_cancellation() {
    let path = std::env::var("YYDB_DB2_CONFIG").expect("YYDB_DB2_CONFIG is required");
    let config: Config = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(config.tls, "require");
    assert!(config.ca_file.is_some());
    let mut options = Options::memory(DbKind::Db2);
    options.connection_string = config.encode().unwrap();
    let flag = AtomicBool::new(false);
    let mut db = Database::open(&options).unwrap();
    let mut invalid = config.clone();
    invalid.ca_file = None;
    let mut invalid_options = options.clone();
    invalid_options.connection_string = invalid.encode().unwrap();
    assert!(
        Database::open(&invalid_options).is_err(),
        "self-signed certificate accepted without its CA"
    );

    let table = format!("YYDB_TLS_{}", std::process::id());
    let mut data = ResultSet::new(vec![
        Column {
            name: "N".into(),
            type_name: "DECIMAL(31,8)".into(),
        },
        Column {
            name: "S".into(),
            type_name: "VARCHAR".into(),
        },
        Column {
            name: "B".into(),
            type_name: "VARBINARY".into(),
        },
    ])
    .unwrap();
    let expected = vec![
        Value::Decimal("12345678901234567890123.12345678".into()),
        Value::Text("日本語'); DELETE FROM t; -- ?".into()),
        Value::Binary(vec![0, 255, 39]),
    ];
    data.push(&expected).unwrap();
    data.push(&vec![Value::Null; 3]).unwrap();
    data.finish().unwrap();
    db.import_into(&data, &table, true).unwrap();
    let result = (|| -> yy_db_core::Result<()> {
        let sets = db.execute(&format!("SELECT * FROM {table} WHERE N IS NOT NULL"), &flag)?;
        assert_eq!(sets[0].page(0, 2)?[0], expected);
        let sets = db.execute(&format!("SELECT * FROM {table} WHERE N IS NULL"), &flag)?;
        assert_eq!(sets[0].page(0, 2)?[0], vec![Value::Null; 3]);
        db.execute("COMMIT", &flag)?;
        db.execute(&format!("LOCK TABLE {table} IN EXCLUSIVE MODE"), &flag)?;
        let mut monitor = Database::open(&options)?;
        let (ready_tx, ready_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let sql = format!("SELECT * FROM {table}");
        let worker = thread::spawn(move || {
            let mut connection = Database::open(&options).unwrap();
            let flag = AtomicBool::new(false);
            let sets = connection
                .execute("VALUES MON_GET_APPLICATION_HANDLE()", &flag)
                .unwrap();
            let handle = sets[0].page(0, 1).unwrap()[0][0]
                .display()
                .parse::<u64>()
                .unwrap();
            connection.execute("COMMIT", &flag).unwrap();
            let cancellation = connection.cancellation();
            let cancelled_flag = cancellation.flag();
            ready_tx.send((cancellation, handle)).unwrap();
            let result = connection.execute(&sql, &cancelled_flag);
            result_tx.send(result.map(|_| ())).unwrap();
        });
        let (cancel, handle) = ready_rx.recv_timeout(Duration::from_secs(15)).unwrap();
        let wait = (|| -> yy_db_core::Result<bool> {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                let sets = monitor.execute(
                    &format!(
                        "SELECT COUNT(*) FROM TABLE(MON_GET_APPL_LOCKWAIT({handle}, -2)) AS W"
                    ),
                    &flag,
                )?;
                if sets[0].page(0, 1)?[0][0].display() != "0" {
                    return Ok(true);
                }
                thread::sleep(Duration::from_millis(20));
            }
            Ok(false)
        })();
        cancel.cancel();
        let cancelled = result_rx.recv_timeout(Duration::from_secs(3));
        // Release the lock and reap the worker even when the bounded wait failed.
        let rollback = db.execute("ROLLBACK", &flag);
        worker.join().unwrap();
        rollback?;
        assert!(wait?, "Db2 never reported the worker's lock wait");
        assert!(
            cancelled
                .expect("cancellation failed to interrupt TLS I/O")
                .is_err()
        );
        Ok(())
    })();
    db.execute("ROLLBACK", &flag).unwrap();
    db.execute(&format!("DROP TABLE {table}"), &flag).unwrap();
    db.execute("COMMIT", &flag).unwrap();
    result.unwrap();
}

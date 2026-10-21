use crate::{Column, Result, ResultSet, Value};
use odbc_api::{Cursor, ResultSetMetadata};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[path = "plan_actual.rs"]
mod plan_actual;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbKind {
    Sqlite,
    DuckDb,
    Oracle,
    SqlServer,
    PostgreSql,
    MySql,
    Db2,
}
impl DbKind {
    pub const ALL: [Self; 7] = [
        Self::Sqlite,
        Self::DuckDb,
        Self::Oracle,
        Self::SqlServer,
        Self::PostgreSql,
        Self::MySql,
        Self::Db2,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Sqlite => "SQLite",
            Self::DuckDb => "DuckDB",
            Self::Oracle => "Oracle",
            Self::SqlServer => "SQL Server",
            Self::PostgreSql => "PostgreSQL",
            Self::MySql => "MySQL",
            Self::Db2 => "IBM Db2",
        }
    }
}
pub fn odbc_drivers() -> Result<Vec<String>> {
    Ok(odbc_api::environment()
        .map_err(|e| e.to_string())?
        .drivers()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|d| d.description)
        .collect())
}

#[derive(Clone, Debug)]
pub struct Options {
    pub kind: DbKind,
    pub path: Option<PathBuf>,
    pub connection_string: String,
    pub read_only: bool,
}
impl Options {
    pub fn memory(kind: DbKind) -> Self {
        Self {
            kind,
            path: None,
            connection_string: String::new(),
            read_only: false,
        }
    }
}

pub enum Database {
    Sqlite(rusqlite::Connection),
    Duck(duckdb::Connection),
    Odbc(DbKind, odbc_api::Connection<'static>, String),
}
pub struct Cancellation {
    flag: Arc<AtomicBool>,
    interrupt: Box<dyn Fn() + Send + Sync>,
}
impl Cancellation {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
        (self.interrupt)();
    }
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.flag.clone()
    }
}
impl Database {
    pub fn open(o: &Options) -> Result<Self> {
        match o.kind {
            DbKind::Sqlite => {
                let flags = if o.read_only {
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                } else {
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                        | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
                };
                let c = rusqlite::Connection::open_with_flags(
                    o.path
                        .as_deref()
                        .unwrap_or_else(|| std::path::Path::new(":memory:")),
                    flags,
                )
                .map_err(|e| e.to_string())?;
                c.busy_timeout(std::time::Duration::from_secs(5))
                    .map_err(|e| e.to_string())?;
                Ok(Self::Sqlite(c))
            }
            DbKind::DuckDb => {
                let mode = if o.read_only {
                    duckdb::AccessMode::ReadOnly
                } else {
                    duckdb::AccessMode::ReadWrite
                };
                let config = duckdb::Config::default()
                    .access_mode(mode)
                    .map_err(|e| e.to_string())?;
                let c = duckdb::Connection::open_with_flags(
                    o.path
                        .as_deref()
                        .unwrap_or_else(|| std::path::Path::new(":memory:")),
                    config,
                )
                .map_err(|e| e.to_string())?;
                c.execute_batch(
                    "SET autoinstall_known_extensions=false; SET autoload_known_extensions=false;",
                )
                .map_err(|e| e.to_string())?;
                Ok(Self::Duck(c))
            }
            kind => {
                if o.connection_string.trim().is_empty() {
                    return Err(
                        "ODBC接続文字列を指定してください（対応する64bitドライバーが必要です）"
                            .into(),
                    );
                }
                let c = odbc_api::environment()
                    .map_err(|e| e.to_string())?
                    .connect_with_connection_string(
                        &o.connection_string,
                        odbc_api::ConnectionOptions {
                            login_timeout_sec: Some(30),
                            ..Default::default()
                        },
                    )
                    .map_err(|e| e.to_string())?;
                if o.read_only {
                    return Err("ODBC接続の読み取り専用強制は未実装です。DB側の読み取り専用権限を使用してください".into());
                }
                Ok(Self::Odbc(kind, c, o.connection_string.clone()))
            }
        }
    }
    pub fn kind(&self) -> DbKind {
        match self {
            Self::Sqlite(_) => DbKind::Sqlite,
            Self::Duck(_) => DbKind::DuckDb,
            Self::Odbc(k, ..) => *k,
        }
    }
    pub fn cancellation(&self) -> Cancellation {
        let interrupt: Box<dyn Fn() + Send + Sync> = match self {
            Self::Sqlite(c) => {
                let h = c.get_interrupt_handle();
                Box::new(move || h.interrupt())
            }
            Self::Duck(c) => {
                let h = c.interrupt_handle();
                Box::new(move || h.interrupt())
            }
            Self::Odbc(..) => Box::new(|| {}),
        };
        Cancellation {
            flag: Arc::new(AtomicBool::new(false)),
            interrupt,
        }
    }
    pub fn execute(&mut self, sql: &str, cancel: &AtomicBool) -> Result<Vec<ResultSet>> {
        let mut results = vec![];
        for statement in crate::sql::statements(sql, self.kind())? {
            if cancel.load(Ordering::Relaxed) {
                return Err("取り消されました".into());
            }
            results.extend(self.query(&statement, cancel)?);
        }
        Ok(results)
    }
    fn query(&mut self, sql: &str, cancel: &AtomicBool) -> Result<Vec<ResultSet>> {
        let mut results = vec![];
        match self {
            Self::Sqlite(c) => {
                let mut s = c.prepare(sql).map_err(|e| e.to_string())?;
                let columns = s
                    .columns()
                    .iter()
                    .map(|col| Column {
                        name: col.name().to_string(),
                        type_name: col.decl_type().unwrap_or("dynamic").to_string(),
                    })
                    .collect();
                let mut set = ResultSet::new(columns)?;
                if s.column_count() == 0 {
                    let n = s.execute([]).map_err(|e| e.to_string())?;
                    set.columns = vec![Column {
                        name: "更新件数".into(),
                        type_name: "INTEGER".into(),
                    }];
                    set.push(&[Value::Integer(n.to_string())])?;
                } else {
                    let n = s.column_count();
                    let mut rows = s.query([]).map_err(|e| e.to_string())?;
                    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                        if cancel.load(Ordering::Relaxed) {
                            return Err("取り消されました".into());
                        }
                        let mut values = vec![];
                        for i in 0..n {
                            use rusqlite::types::ValueRef;
                            values.push(match row.get_ref(i).map_err(|e| e.to_string())? {
                                ValueRef::Null => Value::Null,
                                ValueRef::Integer(v) => Value::Integer(v.to_string()),
                                ValueRef::Real(v) => Value::Float(v),
                                ValueRef::Text(v) => Value::Text(
                                    String::from_utf8(v.to_vec()).map_err(|e| e.to_string())?,
                                ),
                                ValueRef::Blob(v) => Value::Binary(v.to_vec()),
                            });
                        }
                        set.push(&values)?;
                    }
                }
                set.finish()?;
                results.push(set);
            }
            Self::Duck(c) => {
                use duckdb::arrow::{
                    array::{Array, BinaryArray, LargeBinaryArray},
                    datatypes::DataType,
                    util::display::array_value_to_string,
                };
                let mut s = c.prepare(sql).map_err(|e| e.to_string())?;
                let batches = s.query_arrow([]).map_err(|e| e.to_string())?;
                let schema = batches.get_schema();
                let mut set = ResultSet::new(
                    schema
                        .fields()
                        .iter()
                        .map(|f| Column {
                            name: f.name().clone(),
                            type_name: f.data_type().to_string(),
                        })
                        .collect(),
                )?;
                for batch in batches {
                    for row in 0..batch.num_rows() {
                        if cancel.load(Ordering::Relaxed) {
                            return Err("取り消されました".into());
                        }
                        let mut values = vec![];
                        for array in batch.columns() {
                            let text = || {
                                array_value_to_string(array.as_ref(), row)
                                    .map_err(|e| e.to_string())
                            };
                            let v = if array.is_null(row) {
                                Value::Null
                            } else {
                                match array.data_type() {
                                    DataType::Binary => Value::Binary(
                                        array
                                            .as_any()
                                            .downcast_ref::<BinaryArray>()
                                            .unwrap()
                                            .value(row)
                                            .to_vec(),
                                    ),
                                    DataType::LargeBinary => Value::Binary(
                                        array
                                            .as_any()
                                            .downcast_ref::<LargeBinaryArray>()
                                            .unwrap()
                                            .value(row)
                                            .to_vec(),
                                    ),
                                    DataType::Int8
                                    | DataType::Int16
                                    | DataType::Int32
                                    | DataType::Int64
                                    | DataType::UInt8
                                    | DataType::UInt16
                                    | DataType::UInt32
                                    | DataType::UInt64 => Value::Integer(text()?),
                                    DataType::Decimal128(_, _) | DataType::Decimal256(_, _) => {
                                        Value::Decimal(text()?)
                                    }
                                    DataType::Float32 | DataType::Float64 => Value::Float(
                                        text()?.parse::<f64>().map_err(|e| e.to_string())?,
                                    ),
                                    DataType::Boolean => Value::Bool(text()? == "true"),
                                    DataType::Date32
                                    | DataType::Date64
                                    | DataType::Time32(_)
                                    | DataType::Time64(_)
                                    | DataType::Timestamp(_, _) => Value::Temporal(text()?),
                                    _ => Value::Text(text()?),
                                }
                            };
                            values.push(v);
                        }
                        set.push(&values)?;
                    }
                }
                set.finish()?;
                results.push(set);
            }
            Self::Odbc(_, c, _) => {
                let mut next = c.execute(sql, (), Some(60)).map_err(|e| e.to_string())?;
                while let Some(mut cursor) = next {
                    let count = cursor.num_result_cols().map_err(|e| e.to_string())? as usize;
                    let names = cursor
                        .column_names()
                        .map_err(|e| e.to_string())?
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map_err(|e| e.to_string())?;
                    let mut types = vec![];
                    for i in 1..=count {
                        types.push(cursor.col_data_type(i as u16).map_err(|e| e.to_string())?);
                    }
                    let mut set = ResultSet::new(
                        names
                            .into_iter()
                            .zip(&types)
                            .map(|(name, t)| Column {
                                name,
                                type_name: format!("{t:?}"),
                            })
                            .collect(),
                    )?;
                    while let Some(mut row) = cursor.next_row().map_err(|e| e.to_string())? {
                        if cancel.load(Ordering::Relaxed) {
                            return Err(
                                "取消要求を受け付けました。ODBCの実行中取消には未対応です".into()
                            );
                        }
                        let mut values = vec![];
                        for (i, t) in types.iter().enumerate() {
                            let mut text = vec![];
                            let mut binary = vec![];
                            let v = if matches!(
                                t,
                                odbc_api::DataType::Binary { .. }
                                    | odbc_api::DataType::Varbinary { .. }
                                    | odbc_api::DataType::LongVarbinary { .. }
                            ) {
                                if row
                                    .get_binary((i + 1) as u16, &mut binary)
                                    .map_err(|e| e.to_string())?
                                {
                                    Value::Binary(binary)
                                } else {
                                    Value::Null
                                }
                            } else if row
                                .get_wide_text((i + 1) as u16, &mut text)
                                .map_err(|e| e.to_string())?
                            {
                                let text = String::from_utf16(&text).map_err(|e| e.to_string())?;
                                match t {
                                    odbc_api::DataType::Numeric { .. }
                                    | odbc_api::DataType::Decimal { .. } => Value::Decimal(text),
                                    odbc_api::DataType::Integer
                                    | odbc_api::DataType::SmallInt
                                    | odbc_api::DataType::BigInt
                                    | odbc_api::DataType::TinyInt => Value::Integer(text),
                                    _ => Value::Text(text),
                                }
                            } else {
                                Value::Null
                            };
                            values.push(v);
                        }
                        set.push(&values)?;
                    }
                    set.finish()?;
                    results.push(set);
                    next = cursor.more_results().map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(results)
    }
    pub fn tables(&mut self) -> Result<Vec<ResultSet>> {
        let sql = match self.kind() {
            DbKind::Sqlite => {
                "SELECT name, type FROM sqlite_schema WHERE type IN ('table','view') ORDER BY name"
            }
            DbKind::DuckDb | DbKind::PostgreSql | DbKind::SqlServer | DbKind::MySql => {
                "SELECT table_schema, table_name, table_type FROM information_schema.tables ORDER BY table_schema, table_name"
            }
            DbKind::Oracle => "SELECT owner, table_name FROM all_tables ORDER BY owner, table_name",
            DbKind::Db2 => {
                "SELECT tabschema, tabname, type FROM syscat.tables ORDER BY tabschema, tabname"
            }
        };
        self.execute(sql, &AtomicBool::new(false))
    }
    pub fn explain(
        &mut self,
        sql: &str,
        actual: bool,
        cancel: &AtomicBool,
    ) -> Result<crate::plan::Plan> {
        let mut plan = self.explain_native(sql, actual, cancel)?;
        for node in plan.nodes.iter_mut().filter(|n| n.parent.is_none()) {
            node.properties.insert("Query.Text".into(), sql.into());
        }
        Ok(plan)
    }
    fn explain_native(
        &mut self,
        sql: &str,
        actual: bool,
        cancel: &AtomicBool,
    ) -> Result<crate::plan::Plan> {
        let statements = crate::sql::statements(sql, self.kind())?;
        if statements.len() != 1 {
            return Err("計画取得は1文を選択してください".into());
        }
        let statement = &statements[0];
        match self.kind() {
            DbKind::Sqlite => {
                if actual {
                    return Err("SQLiteではノード単位の実測計画は未対応です".into());
                }
                let r = self.execute(&format!("EXPLAIN QUERY PLAN {statement}"), cancel)?;
                crate::plan::Plan::sqlite(r.first().ok_or("計画がありません")?)
            }
            DbKind::DuckDb | DbKind::PostgreSql | DbKind::MySql => {
                let prefix = match self.kind() {
                    DbKind::DuckDb => {
                        if actual {
                            "EXPLAIN (ANALYZE, FORMAT JSON)"
                        } else {
                            "EXPLAIN (FORMAT JSON)"
                        }
                    }
                    DbKind::PostgreSql => {
                        if actual {
                            "EXPLAIN (ANALYZE, VERBOSE, BUFFERS, FORMAT JSON)"
                        } else {
                            "EXPLAIN (VERBOSE, FORMAT JSON)"
                        }
                    }
                    _ => {
                        if actual {
                            "EXPLAIN ANALYZE FORMAT=TREE"
                        } else {
                            "EXPLAIN FORMAT=JSON"
                        }
                    }
                };
                let r = self.execute(&format!("{prefix} {statement}"), cancel)?;
                let row = r
                    .first()
                    .ok_or("計画がありません")?
                    .page(0, 1)?
                    .into_iter()
                    .next()
                    .ok_or("計画がありません")?;
                let raw = row.last().ok_or("計画がありません")?.display();
                crate::plan::Plan::parse(self.kind(), &raw, actual)
            }
            DbKind::SqlServer => {
                let setting = if actual {
                    "STATISTICS XML"
                } else {
                    "SHOWPLAN_XML"
                };
                self.query(&format!("SET {setting} ON"), cancel)?;
                let r = self.query(statement, cancel);
                let restored = self.query(&format!("SET {setting} OFF"), &AtomicBool::new(false));
                restored?;
                let sets = r?;
                for set in sets {
                    for row in set.page(0, 10)? {
                        for value in row {
                            let text = value.display();
                            if text.contains("ShowPlanXML") {
                                return crate::plan::Plan::parse(DbKind::SqlServer, &text, actual);
                            }
                        }
                    }
                }
                Err("SQL ServerのXML計画を取得できませんでした".into())
            }
            DbKind::Oracle => {
                if actual {
                    return self.oracle_actual(statement, cancel);
                }
                let token = format!(
                    "YYDB{:08X}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .subsec_nanos()
                );
                self.query(
                    &format!("EXPLAIN PLAN SET STATEMENT_ID='{token}' FOR {statement}"),
                    cancel,
                )?;
                let sets=self.query(&format!("SELECT ID,PARENT_ID,OPERATION || ' ' || OPTIONS AS OPERATOR,CASE WHEN OBJECT_NAME IS NULL THEN NULL ELSE OBJECT_OWNER || '.' || OBJECT_NAME END AS OBJECT_NAME,CARDINALITY,COST,ACCESS_PREDICATES,FILTER_PREDICATES,IO_COST,CPU_COST,BYTES,TIME,PARTITION_START,PARTITION_STOP,DISTRIBUTION,OBJECT_TYPE FROM PLAN_TABLE WHERE STATEMENT_ID='{token}' ORDER BY ID"),cancel);
                let cleanup = self.query(
                    &format!("DELETE FROM PLAN_TABLE WHERE STATEMENT_ID='{token}'"),
                    &AtomicBool::new(false),
                );
                cleanup?;
                let sets = sets?;
                crate::plan::Plan::tabular(
                    DbKind::Oracle,
                    sets.first().ok_or("計画表がありません")?,
                )
            }
            DbKind::Db2 => {
                if actual {
                    return self.db2_actual(statement, cancel);
                }
                let queryno = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos();
                self.query(
                    &format!("EXPLAIN PLAN SET QUERYNO={queryno} FOR {statement}"),
                    cancel,
                )
                .map_err(|e| {
                    format!(
                        "{e}（EXPLAIN表と権限を確認してください。アプリは表を自動作成しません）"
                    )
                })?;
                let key = |alias: &str| {
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
                    .map(|k| format!("{alias}.{k}=t.{k}"))
                    .collect::<Vec<_>>()
                    .join(" AND ")
                };
                let sql = format!(
                    "WITH stmt AS (SELECT * FROM EXPLAIN_STATEMENT WHERE QUERYNO={queryno} AND EXPLAIN_REQUESTER=CURRENT USER AND EXPLAIN_LEVEL='P' ORDER BY EXPLAIN_TIME DESC FETCH FIRST 1 ROW ONLY), ops AS (SELECT o.* FROM EXPLAIN_OPERATOR o JOIN stmt t ON {}), streams AS (SELECT s.* FROM EXPLAIN_STREAM s JOIN stmt t ON {}) SELECT o.OPERATOR_ID,(SELECT MIN(s.TARGET_ID) FROM streams s WHERE s.SOURCE_TYPE='O' AND s.TARGET_TYPE='O' AND s.SOURCE_ID=o.OPERATOR_ID) AS PARENT_ID,o.OPERATOR_TYPE,(SELECT MIN(s.OBJECT_SCHEMA || '.' || s.OBJECT_NAME) FROM streams s WHERE s.TARGET_ID=o.OPERATOR_ID AND s.SOURCE_TYPE='D') AS OBJECT_NAME,(SELECT MAX(s.STREAM_COUNT) FROM streams s WHERE s.SOURCE_ID=o.OPERATOR_ID AND s.SOURCE_TYPE='O') AS ESTIMATED_ROWS,o.TOTAL_COST,o.IO_COST,o.CPU_COST FROM ops o ORDER BY o.OPERATOR_ID",
                    key("o"),
                    key("s")
                );
                let sets = self.query(&sql, cancel)?;
                let mut plan = crate::plan::Plan::tabular(
                    DbKind::Db2,
                    sets.first().ok_or("計画がありません")?,
                )?;
                let predicates=self.query(&format!("WITH stmt AS (SELECT * FROM EXPLAIN_STATEMENT WHERE QUERYNO={queryno} AND EXPLAIN_REQUESTER=CURRENT USER AND EXPLAIN_LEVEL='P' ORDER BY EXPLAIN_TIME DESC FETCH FIRST 1 ROW ONLY) SELECT p.OPERATOR_ID,p.PREDICATE_ID,p.HOW_APPLIED,p.PREDICATE_TEXT FROM EXPLAIN_PREDICATE p JOIN stmt t ON {} ORDER BY p.OPERATOR_ID,p.PREDICATE_ID",key("p")),cancel)?;
                if let Some(set) = predicates.first() {
                    let rows = set.page(0, 10_000)?;
                    for row in &rows {
                        if let Some(node) = row
                            .first()
                            .and_then(|v| v.display().trim().parse::<u64>().ok())
                            .and_then(|id| plan.nodes.iter_mut().find(|n| n.id == id))
                            && row.len() >= 4
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
                    plan.raw=serde_json::to_string(&serde_json::json!({"operators":serde_json::from_str::<serde_json::Value>(&plan.raw).map_err(|e|e.to_string())?,"predicates":rows})).map_err(|e|e.to_string())?;
                }
                Ok(plan)
            }
        }
    }
    pub fn import_into(&mut self, set: &ResultSet, table: &str, create: bool) -> Result<u64> {
        if let Self::Odbc(kind, _, connection) = self {
            return import_odbc(*kind, connection, set, table, create);
        }
        let table = crate::sql::identifier(table)?;
        let fields = set
            .columns
            .iter()
            .map(|c| crate::sql::identifier(&c.name))
            .collect::<Result<Vec<_>>>()?;
        let begin = if self.kind() == DbKind::SqlServer {
            "BEGIN TRANSACTION"
        } else {
            "BEGIN"
        };
        self.execute(begin, &AtomicBool::new(false))?;
        let run = (|| -> Result<u64> {
            if create {
                self.execute(
                    &format!(
                        "CREATE TABLE {table} ({})",
                        fields
                            .iter()
                            .map(|c| format!("{c} VARCHAR"))
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                    &AtomicBool::new(false),
                )?;
            }
            let marks = vec!["?"; fields.len()].join(",");
            let sql = format!(
                "INSERT INTO {table} ({}) VALUES ({marks})",
                fields.join(",")
            );
            let mut start = 0;
            while start < set.row_count {
                for row in set.page(start, 512)? {
                    match self {
                        Self::Sqlite(c) => {
                            let values = row
                                .into_iter()
                                .map(|v| match v {
                                    Value::Null => rusqlite::types::Value::Null,
                                    Value::Binary(b) => rusqlite::types::Value::Blob(b),
                                    v => rusqlite::types::Value::Text(v.display()),
                                })
                                .collect::<Vec<_>>();
                            c.execute(&sql, rusqlite::params_from_iter(values))
                                .map_err(|e| e.to_string())?;
                        }
                        Self::Duck(c) => {
                            let values = row
                                .into_iter()
                                .map(|v| match v {
                                    Value::Null => duckdb::types::Value::Null,
                                    Value::Binary(b) => duckdb::types::Value::Blob(b),
                                    v => duckdb::types::Value::Text(v.display()),
                                })
                                .collect::<Vec<_>>();
                            c.execute(&sql, duckdb::params_from_iter(values))
                                .map_err(|e| e.to_string())?;
                        }
                        Self::Odbc(..) => return Err("ODBCの型付きインポートは未実装です".into()),
                    }
                }
                start += 512;
            }
            Ok(set.row_count)
        })();
        match run {
            Ok(n) => {
                self.execute("COMMIT", &AtomicBool::new(false))?;
                Ok(n)
            }
            Err(e) => {
                let rollback = self.execute("ROLLBACK", &AtomicBool::new(false));
                Err(if let Err(r) = rollback {
                    format!("{e}; ロールバックも失敗: {r}")
                } else {
                    e
                })
            }
        }
    }
}

// Imports use a separate connection so they cannot commit a SQL tab's transaction.
fn import_odbc(
    kind: DbKind,
    connection: &str,
    set: &ResultSet,
    table: &str,
    create: bool,
) -> Result<u64> {
    use odbc_api::IntoParameter;
    use odbc_api::parameter::{InputParameter, VarWCharBox};
    let c = odbc_api::environment()
        .map_err(|e| e.to_string())?
        .connect_with_connection_string(
            connection,
            odbc_api::ConnectionOptions {
                login_timeout_sec: Some(30),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
    let table = crate::sql::identifier_for(kind, table)?;
    let fields = set
        .columns
        .iter()
        .map(|c| crate::sql::identifier_for(kind, &c.name))
        .collect::<Result<Vec<_>>>()?;
    if create {
        let definitions = set
            .columns
            .iter()
            .zip(&fields)
            .map(|(column, name)| {
                let t = column.type_name.to_ascii_lowercase();
                let ty = if t.contains("decimal") || t.contains("numeric") {
                    let nums = t
                        .split(|c: char| !c.is_ascii_digit())
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>();
                    if nums.len() >= 2 {
                        format!("DECIMAL({},{})", nums[0], nums[1])
                    } else {
                        "DECIMAL(31,8)".into()
                    }
                } else if t.contains("int") {
                    if kind == DbKind::Oracle {
                        "NUMBER(19)".into()
                    } else {
                        "BIGINT".into()
                    }
                } else {
                    match kind {
                        DbKind::SqlServer => "NVARCHAR(1024)",
                        DbKind::Oracle => "VARCHAR2(1024 CHAR)",
                        _ => "VARCHAR(1024)",
                    }
                    .into()
                };
                format!("{name} {ty}")
            })
            .collect::<Vec<_>>();
        c.execute(
            &format!("CREATE TABLE {table} ({})", definitions.join(",")),
            (),
            Some(60),
        )
        .map_err(|e| e.to_string())?;
    }
    c.set_autocommit(false).map_err(|e| e.to_string())?;
    let sql = format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        fields.join(","),
        vec!["?"; fields.len()].join(",")
    );
    let run = (|| -> Result<()> {
        let mut start = 0;
        while start < set.row_count {
            for row in set.page(start, 512)? {
                let params = row
                    .into_iter()
                    .map(|v| -> Box<dyn InputParameter> {
                        match v {
                            Value::Null => Box::new(VarWCharBox::null()),
                            Value::Binary(b) => Box::new(b.into_parameter()),
                            v => Box::new(VarWCharBox::from_str_slice(&v.display())),
                        }
                    })
                    .collect::<Vec<_>>();
                c.execute(&sql, params.as_slice(), Some(60))
                    .map_err(|e| e.to_string())?;
            }
            start += 512;
        }
        Ok(())
    })();
    match run {
        Ok(()) => {
            c.commit().map_err(|e| e.to_string())?;
            Ok(set.row_count)
        }
        Err(e) => {
            let rollback = c.rollback();
            Err(if let Err(r) = rollback {
                format!("{e}; rollback failed: {r}")
            } else {
                e
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_import_rolls_back_partial_data() {
        let mut db = Database::open(&Options::memory(DbKind::Sqlite)).unwrap();
        let cancel = AtomicBool::new(false);
        db.execute("CREATE TABLE t(n INTEGER UNIQUE)", &cancel)
            .unwrap();
        let mut rows = ResultSet::new(vec![Column {
            name: "n".into(),
            type_name: "INTEGER".into(),
        }])
        .unwrap();
        rows.push(&[Value::Integer("1".into())]).unwrap();
        rows.push(&[Value::Integer("1".into())]).unwrap();
        rows.finish().unwrap();
        assert!(db.import_into(&rows, "t", false).is_err());
        let result = db.execute("SELECT COUNT(*) FROM t", &cancel).unwrap();
        assert_eq!(
            result[0].page(0, 1).unwrap()[0][0],
            Value::Integer("0".into())
        );
    }
    #[test]
    fn embedded_engines_transactions_exact_values_and_plan() {
        for kind in [DbKind::Sqlite, DbKind::DuckDb] {
            let mut db = Database::open(&Options::memory(kind)).unwrap();
            let cancel = AtomicBool::new(false);
            db.execute("CREATE TABLE t (n BIGINT, s VARCHAR); BEGIN; INSERT INTO t VALUES (9223372036854775807, '日本語'); ROLLBACK; INSERT INTO t VALUES (9223372036854775807, '日本語')",&cancel).unwrap();
            let r = db.execute("SELECT * FROM t", &cancel).unwrap();
            assert_eq!(r[0].row_count, 1);
            assert_eq!(
                r[0].page(0, 1).unwrap()[0][0],
                Value::Integer("9223372036854775807".into())
            );
            let plan = db.explain("SELECT * FROM t", false, &cancel).unwrap();
            assert!(!plan.nodes.is_empty());
        }
    }
    #[test]
    fn embedded_complex_plan_has_usable_operators_properties_and_flow() {
        for kind in [DbKind::Sqlite, DbKind::DuckDb] {
            let mut db = Database::open(&Options::memory(kind)).unwrap();
            let cancel = AtomicBool::new(false);
            db.execute("CREATE TABLE a(id INTEGER, grp INTEGER); CREATE TABLE b(id INTEGER, amount INTEGER); INSERT INTO a VALUES(1,1),(2,1),(3,2); INSERT INTO b VALUES(1,30),(1,40),(2,20),(3,50)",&cancel).unwrap();
            for actual in [false, true] {
                if actual && kind == DbKind::Sqlite {
                    continue;
                }
                let p=db.explain("SELECT a.grp, SUM(b.amount) AS total FROM a JOIN b ON a.id=b.id GROUP BY a.grp ORDER BY total DESC",actual,&cancel).unwrap();
                crate::plan_visual::validate(&p).unwrap();
                assert!(p.nodes.len() >= 4);
                if actual {
                    assert!(
                        p.nodes
                            .iter()
                            .any(|n| n.properties.contains_key("operator_timing"))
                    );
                    assert!(p.nodes.iter().any(|n| n.actual_rows.is_some()));
                }
                if let Ok(directory) = std::env::var("YYDB_REPORT_DIR") {
                    std::fs::create_dir_all(&directory).unwrap();
                    let name = format!(
                        "{}-complex-{}",
                        kind.name(),
                        if actual { "actual" } else { "estimated" }
                    );
                    let path = std::path::Path::new(&directory);
                    std::fs::write(
                        path.join(format!("{name}.yyplan")),
                        serde_json::to_vec_pretty(&p).unwrap(),
                    )
                    .unwrap();
                    let doc = p.drawing();
                    std::fs::write(
                        path.join(format!("{name}.png")),
                        crate::plan::drawing_renderer()
                            .png(&doc, &crate::plan::drawing_export(&doc), 1.)
                            .unwrap(),
                    )
                    .unwrap();
                }
            }
        }
    }
    #[test]
    fn duckdb_decimal38_does_not_pass_through_rust_decimal() {
        let mut db = Database::open(&Options::memory(DbKind::DuckDb)).unwrap();
        let r = db
            .execute(
                "SELECT 123456789012345678901234567890.12345678::DECIMAL(38,8)",
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(
            r[0].page(0, 1).unwrap()[0][0],
            Value::Decimal("123456789012345678901234567890.12345678".into())
        );
    }
}

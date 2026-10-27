//! Offline, DB-specific built-in type suggestions. Parameters are editable examples.
use super::*;
use yy_db_core::{DbKind, schema::Model};
pub(super) fn builtins(kind: DbKind) -> &'static [&'static str] {
    match kind {
        DbKind::Sqlite => &[
            "INTEGER",
            "TEXT",
            "REAL",
            "BLOB",
            "NUMERIC",
            "INT",
            "VARCHAR(255)",
            "CHAR(1)",
            "DECIMAL(18,2)",
            "BOOLEAN",
            "DATE",
            "DATETIME",
        ],
        DbKind::DuckDb => &[
            "INTEGER",
            "BIGINT",
            "SMALLINT",
            "TINYINT",
            "HUGEINT",
            "UTINYINT",
            "USMALLINT",
            "UINTEGER",
            "UBIGINT",
            "UHUGEINT",
            "BOOLEAN",
            "DECIMAL(18,2)",
            "FLOAT",
            "DOUBLE",
            "VARCHAR",
            "BLOB",
            "DATE",
            "TIME",
            "TIMESTAMP",
            "TIMESTAMP WITH TIME ZONE",
            "INTERVAL",
            "UUID",
            "BIT",
            "INTEGER[]",
            "STRUCT(name VARCHAR, age INTEGER)",
            "MAP(VARCHAR, INTEGER)",
        ],
        DbKind::PostgreSql => &[
            "INTEGER",
            "BIGINT",
            "SMALLINT",
            "NUMERIC(18,2)",
            "DECIMAL(18,2)",
            "REAL",
            "DOUBLE PRECISION",
            "BOOLEAN",
            "VARCHAR(255)",
            "CHAR(1)",
            "TEXT",
            "BYTEA",
            "DATE",
            "TIME",
            "TIME WITH TIME ZONE",
            "TIMESTAMP",
            "TIMESTAMP WITH TIME ZONE",
            "INTERVAL",
            "UUID",
            "JSON",
            "JSONB",
            "XML",
            "MONEY",
            "BIT(8)",
            "BIT VARYING(64)",
            "INET",
            "CIDR",
            "MACADDR",
            "TSVECTOR",
            "TSQUERY",
            "INTEGER[]",
            "TEXT[]",
        ],
        DbKind::SqlServer => &[
            "INT",
            "BIGINT",
            "SMALLINT",
            "TINYINT",
            "BIT",
            "DECIMAL(18,2)",
            "NUMERIC(18,2)",
            "MONEY",
            "SMALLMONEY",
            "REAL",
            "FLOAT",
            "CHAR(1)",
            "VARCHAR(255)",
            "VARCHAR(MAX)",
            "NCHAR(1)",
            "NVARCHAR(255)",
            "NVARCHAR(MAX)",
            "BINARY(16)",
            "VARBINARY(255)",
            "VARBINARY(MAX)",
            "DATE",
            "TIME(7)",
            "DATETIME",
            "DATETIME2(7)",
            "SMALLDATETIME",
            "DATETIMEOFFSET(7)",
            "UNIQUEIDENTIFIER",
            "XML",
            "ROWVERSION",
            "SQL_VARIANT",
            "GEOMETRY",
            "GEOGRAPHY",
            "HIERARCHYID",
        ],
        DbKind::Oracle => &[
            "NUMBER",
            "NUMBER(18,2)",
            "INTEGER",
            "FLOAT",
            "BINARY_FLOAT",
            "BINARY_DOUBLE",
            "CHAR(1)",
            "VARCHAR2(255 CHAR)",
            "NCHAR(1)",
            "NVARCHAR2(255)",
            "DATE",
            "TIMESTAMP(6)",
            "TIMESTAMP(6) WITH TIME ZONE",
            "TIMESTAMP(6) WITH LOCAL TIME ZONE",
            "INTERVAL YEAR(2) TO MONTH",
            "INTERVAL DAY(2) TO SECOND(6)",
            "RAW(16)",
            "BLOB",
            "CLOB",
            "NCLOB",
            "BFILE",
            "ROWID",
            "UROWID",
        ],
        DbKind::MySql => &[
            "INT",
            "BIGINT",
            "SMALLINT",
            "MEDIUMINT",
            "TINYINT",
            "INT UNSIGNED",
            "BIGINT UNSIGNED",
            "DECIMAL(18,2)",
            "FLOAT",
            "DOUBLE",
            "BIT(8)",
            "BOOLEAN",
            "CHAR(1)",
            "VARCHAR(255)",
            "TINYTEXT",
            "TEXT",
            "MEDIUMTEXT",
            "LONGTEXT",
            "BINARY(16)",
            "VARBINARY(255)",
            "TINYBLOB",
            "BLOB",
            "MEDIUMBLOB",
            "LONGBLOB",
            "DATE",
            "TIME",
            "DATETIME",
            "TIMESTAMP",
            "YEAR",
            "JSON",
            "ENUM('value1','value2')",
            "SET('value1','value2')",
            "GEOMETRY",
            "POINT",
            "LINESTRING",
            "POLYGON",
        ],
        DbKind::Db2 => &[
            "SMALLINT",
            "INTEGER",
            "BIGINT",
            "DECIMAL(18,2)",
            "DECFLOAT(16)",
            "DECFLOAT(34)",
            "REAL",
            "DOUBLE",
            "BOOLEAN",
            "CHAR(1)",
            "VARCHAR(255)",
            "CLOB(1M)",
            "GRAPHIC(1)",
            "VARGRAPHIC(255)",
            "DBCLOB(1M)",
            "BINARY(16)",
            "VARBINARY(255)",
            "BLOB(1M)",
            "DATE",
            "TIME",
            "TIMESTAMP",
            "XML",
        ],
    }
}
pub(super) fn choices(model: &Model) -> Vec<String> {
    let mut types = builtins(model.kind)
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    for field in model.entities.iter().flat_map(|e| &e.fields) {
        let value = field.data_type.trim();
        if !value.is_empty() && !types.iter().any(|s| s.eq_ignore_ascii_case(value)) {
            types.push(value.into());
        }
    }
    types
}
pub(super) fn populate(combo: HWND, values: &[String]) {
    unsafe {
        for value in values {
            send_message(
                combo,
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(wide(value).as_ptr() as isize),
            );
        }
        send_message(combo, CB_LIMITTEXT, WPARAM(32768), LPARAM(0));
        send_message(combo, CB_SETMINVISIBLE, WPARAM(12), LPARAM(0));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kinds_have_distinct_native_types_and_keep_custom_definitions() {
        for kind in DbKind::ALL {
            let values = builtins(kind);
            assert!(values.len() >= 5);
            assert_eq!(
                values
                    .iter()
                    .map(|v| v.to_uppercase())
                    .collect::<std::collections::HashSet<_>>()
                    .len(),
                values.len()
            );
        }
        assert!(builtins(DbKind::Oracle).contains(&"VARCHAR2(255 CHAR)"));
        assert!(!builtins(DbKind::Oracle).contains(&"JSONB"));
        assert!(builtins(DbKind::PostgreSql).contains(&"JSONB"));
        assert!(builtins(DbKind::SqlServer).contains(&"NVARCHAR(MAX)"));
        assert!(builtins(DbKind::MySql).contains(&"INT UNSIGNED"));
        assert!(builtins(DbKind::Db2).contains(&"DECFLOAT(34)"));
        let mut model = Model {
            version: 4,
            kind: DbKind::Sqlite,
            entities: vec![],
            relations: vec![],
            names: yy_db_core::schema::Names::Physical,
            level: yy_db_core::schema::Level::Types,
        };
        model.entities.push(yy_db_core::schema::Entity {
            id: yy_db_core::schema::TableId {
                schema: "public".into(),
                name: "sample".into(),
            },
            logical_name: String::new(),
            view: false,
            keys: vec![],
            indexes: vec![],
            size: None,
            position: yy_draw::Point::default(),
            visible: true,
            catalog_definition: false,
            fields: ["custom.money", "custom.money", "text"]
                .iter()
                .enumerate()
                .map(|(i, t)| yy_db_core::schema::Field {
                    name: format!("c{i}"),
                    logical_name: String::new(),
                    data_type: t.to_string(),
                    nullable: true,
                    default_sql: None,
                    important: false,
                })
                .collect(),
        });
        for kind in DbKind::ALL {
            let mut proposed = model.clone();
            proposed.kind = kind;
            proposed.entities[0].id.schema = if matches!(kind, DbKind::Sqlite | DbKind::DuckDb) {
                "main"
            } else {
                "public"
            }
            .into();
            let rows = builtins(kind)
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let mut f = proposed.entities[0].fields[0].clone();
                    f.name = format!("c{i}");
                    f.data_type = t.to_string();
                    (None, f)
                })
                .collect::<Vec<_>>();
            proposed.replace_fields(0, &rows).unwrap();
            let ddl = proposed.ddl().unwrap();
            if matches!(kind, DbKind::Sqlite | DbKind::DuckDb) {
                let mut db =
                    yy_db_core::Database::open(&yy_db_core::Options::memory(kind)).unwrap();
                db.execute(&ddl.sql, &std::sync::atomic::AtomicBool::new(false))
                    .unwrap();
            }
        }
        let values = choices(&model);
        assert_eq!(
            values
                .iter()
                .filter(|v| v.as_str() == "custom.money")
                .count(),
            1
        );
        assert_eq!(
            values
                .iter()
                .filter(|v| v.eq_ignore_ascii_case("text"))
                .count(),
            1
        );
    }
}

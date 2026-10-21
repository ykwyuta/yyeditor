//! Offline import of explicitly supported native plans. Never opens a DB connection.
use crate::{
    DbKind, Result,
    plan::{Node, Plan},
};
use serde_json::Value;
use std::{collections::BTreeMap, io::Read, path::Path};

const LIMIT: usize = 32 * 1024 * 1024;
const FORMATS: &str = "対応形式: .yyplan、SQL Server Showplan XML、PostgreSQL EXPLAIN JSON、MySQL EXPLAIN JSON/TREE、DuckDB JSON、Oracle/SQLiteの見出し付きCSV、Db2の指定8列DEL（.db2plan）。取得方法はヘルプ「実行計画ファイル」を参照してください";

pub fn load(path: &Path) -> Result<Plan> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > LIMIT {
        return Err("計画ファイルは32MiBまでです".into());
    }
    let raw = decode(&bytes)?;
    parse(
        &raw,
        path.extension().and_then(|s| s.to_str()).unwrap_or(""),
    )
}

fn decode(bytes: &[u8]) -> Result<String> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        let (pairs, remainder) = bytes[2..].as_chunks::<2>();
        if !remainder.is_empty() {
            return Err("UTF-16計画ファイルのバイト数が不正です".into());
        }
        let le = bytes[0] == 0xff;
        let words: Vec<_> = pairs
            .iter()
            .map(|b| {
                if le {
                    u16::from_le_bytes([b[0], b[1]])
                } else {
                    u16::from_be_bytes([b[0], b[1]])
                }
            })
            .collect();
        String::from_utf16(&words).map_err(|e| format!("不正なUTF-16: {e}"))
    } else {
        String::from_utf8(
            bytes
                .strip_prefix(&[0xef, 0xbb, 0xbf])
                .unwrap_or(bytes)
                .to_vec(),
        )
        .map_err(|e| format!("UTF-8またはBOM付きUTF-16で保存してください: {e}"))
    }
}

pub fn parse(raw: &str, extension: &str) -> Result<Plan> {
    if raw.len() > LIMIT {
        return Err("実行計画は32MiBまでです".into());
    }
    let text = raw.trim_start_matches('\u{feff}').trim();
    let plan = if text.starts_with('<') {
        Plan::parse(DbKind::SqlServer, text, false)?
    } else if text.starts_with('{') || text.starts_with('[') {
        let value: Value =
            serde_json::from_str(text).map_err(|e| format!("計画JSONが不正です: {e}"))?;
        if value.get("version").is_some() && value.get("nodes").is_some() {
            serde_json::from_value(value).map_err(|e| format!("yyplanが不正です: {e}"))?
        } else {
            let head = value.as_array().and_then(|a| a.first()).unwrap_or(&value);
            if head.get("Plan").is_some() || head.get("Node Type").is_some() {
                Plan::parse(DbKind::PostgreSql, text, false)?
            } else if value.get("query_block").is_some()
                || value.get("operation").and_then(Value::as_str).is_some()
            {
                Plan::parse(DbKind::MySql, text, false)?
            } else if head.get("operator_name").is_some()
                || head.get("children").is_some()
                    && (head.get("name").is_some()
                        || head.get("latency").is_some()
                        || head.get("query_name").is_some())
            {
                Plan::parse(DbKind::DuckDb, text, false)?
            } else if head.get("detail").is_some()
                && head.get("id").is_some()
                && head.get("parent").is_some()
            {
                sqlite_json(raw, &value)?
            } else {
                return Err(format!("識別できない計画JSONです。{FORMATS}"));
            }
        }
    } else if text.starts_with("->") {
        Plan::parse(DbKind::MySql, text, false)?
    } else {
        csv_plan(raw, extension.eq_ignore_ascii_case("db2plan"))?
    };
    if plan.version != 1 {
        return Err("未対応の計画ファイルの版です".into());
    }
    crate::plan_visual::validate(&plan)?;
    Ok(plan)
}

fn sqlite_node(id: u64, parent: u64, detail: &str) -> Node {
    Node {
        id,
        parent: (parent != 0).then_some(parent),
        operator: detail.into(),
        object: if detail.starts_with("SEARCH ") || detail.starts_with("SCAN ") {
            detail.split_whitespace().nth(1).unwrap_or("").into()
        } else {
            String::new()
        },
        estimated_rows: None,
        actual_rows: None,
        cost: None,
        properties: BTreeMap::from([("Detail".into(), detail.into())]),
    }
}
fn sqlite_json(raw: &str, value: &Value) -> Result<Plan> {
    let rows = value
        .as_array()
        .ok_or("SQLite JSONは行オブジェクトの配列で保存してください")?;
    if rows.len() > 10_000 {
        return Err("計画ノードは1万件までです".into());
    }
    let mut nodes = Vec::new();
    for row in rows {
        nodes.push(sqlite_node(
            row.get("id")
                .and_then(Value::as_u64)
                .ok_or("不正なSQLite id")?,
            row.get("parent")
                .and_then(Value::as_u64)
                .ok_or("不正なSQLite parent")?,
            row.get("detail")
                .and_then(Value::as_str)
                .ok_or("不正なSQLite detail")?,
        ));
    }
    Ok(Plan {
        version: 1,
        kind: DbKind::Sqlite,
        actual: false,
        raw: raw.into(),
        nodes,
    })
}

fn csv_plan(raw: &str, db2_del: bool) -> Result<Plan> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(!db2_del)
        .from_reader(raw.as_bytes());
    let headers: Vec<String> = if db2_del {
        [
            "OPERATOR_ID",
            "PARENT_ID",
            "OPERATOR_TYPE",
            "OBJECT_NAME",
            "ESTIMATED_ROWS",
            "TOTAL_COST",
            "IO_COST",
            "CPU_COST",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    } else {
        reader
            .headers()
            .map_err(|e| e.to_string())?
            .iter()
            .map(|s| s.trim().to_ascii_uppercase())
            .collect()
    };
    let has = |name: &str| headers.iter().any(|h| h == name);
    let kind =
        if has("ID") && has("PARENT_ID") && has("OPERATION") && has("CARDINALITY") && has("COST") {
            DbKind::Oracle
        } else if has("OPERATOR_ID")
            && has("PARENT_ID")
            && has("OPERATOR_TYPE")
            && has("ESTIMATED_ROWS")
            && has("TOTAL_COST")
        {
            DbKind::Db2
        } else if has("ID") && has("PARENT") && has("DETAIL") {
            DbKind::Sqlite
        } else {
            return Err(format!("計画CSVの列見出しを識別できません。{FORMATS}"));
        };
    if headers
        .iter()
        .enumerate()
        .any(|(i, h)| headers[..i].contains(h))
    {
        return Err("計画CSVの列名が重複しています".into());
    }
    let mut plan = Plan {
        version: 1,
        kind,
        actual: false,
        raw: raw.into(),
        nodes: vec![],
    };
    for (i, row) in reader.records().enumerate() {
        if i >= 10_000 {
            return Err("計画ノードは1万件までです".into());
        }
        let row = row.map_err(|e| format!("計画CSVの{}行目: {e}", i + 2))?;
        if row.len() != headers.len() {
            return Err("計画CSVの列数が一致しません".into());
        }
        let get = |name: &str| {
            headers
                .iter()
                .position(|h| h == name)
                .and_then(|j| row.get(j))
                .unwrap_or("")
                .trim()
        };
        let number = |name: &str| -> Result<Option<f64>> {
            let value = get(name);
            if value.is_empty() || value.eq_ignore_ascii_case("null") {
                return Ok(None);
            }
            let n: f64 = value
                .parse()
                .map_err(|_| format!("{}行目の{name}が数値ではありません", i + 2))?;
            if !n.is_finite() || n < 0. {
                return Err(format!("{}行目の{name}は0以上の有限値が必要です", i + 2));
            }
            Ok(Some(n))
        };
        let id: u64 = get(if kind == DbKind::Db2 {
            "OPERATOR_ID"
        } else {
            "ID"
        })
        .parse()
        .map_err(|_| "不正な計画ノードID")?;
        if kind == DbKind::Sqlite {
            plan.nodes.push(sqlite_node(
                id,
                get("PARENT").parse().map_err(|_| "不正なSQLite parent")?,
                get("DETAIL"),
            ));
            continue;
        }
        let parent = get("PARENT_ID");
        let parent = if parent.is_empty() || parent.eq_ignore_ascii_case("null") {
            None
        } else {
            Some(parent.parse().map_err(|_| "不正な親ノードID")?)
        };
        let operator = if kind == DbKind::Oracle {
            format!("{} {}", get("OPERATION"), get("OPTIONS"))
                .trim()
                .to_owned()
        } else {
            get("OPERATOR_TYPE").to_owned()
        };
        if operator.is_empty() {
            return Err("計画演算子が空です".into());
        }
        let object = if !get("OBJECT_OWNER").is_empty() && !get("OBJECT_NAME").is_empty() {
            format!("{}.{}", get("OBJECT_OWNER"), get("OBJECT_NAME"))
        } else {
            get("OBJECT_NAME").to_owned()
        };
        let mut properties: BTreeMap<_, _> = headers
            .iter()
            .zip(row.iter())
            .map(|(h, v)| (h.clone(), v.to_owned()))
            .collect();
        let actual = if kind == DbKind::Oracle {
            number("LAST_OUTPUT_ROWS")?
        } else {
            None
        };
        if actual.is_some() {
            let starts =
                number("LAST_STARTS")?.ok_or("Oracleの実測CSVにはLAST_STARTSが必要です")?;
            properties.insert("Actual Starts".into(), starts.to_string());
            if let Some(us) = number("LAST_ELAPSED_TIME")? {
                properties.insert("Actual Elapsed ms".into(), (us / 1000.).to_string());
            }
            plan.actual |= starts > 0.;
        }
        plan.nodes.push(Node {
            id,
            parent,
            operator,
            object,
            estimated_rows: number(if kind == DbKind::Oracle {
                "CARDINALITY"
            } else {
                "ESTIMATED_ROWS"
            })?,
            cost: number(if kind == DbKind::Oracle {
                "COST"
            } else {
                "TOTAL_COST"
            })?,
            actual_rows: actual,
            properties,
        });
    }
    if plan.nodes.iter().any(|n| n.actual_rows.is_some()) && !plan.actual {
        return Err("Oracleの実測統計が未収集です。LAST_STARTSが0の行しかありません".into());
    }
    Ok(plan)
}

/// File-open dispatch, including command-line startup, does not depend on the current DB.
pub fn is_plan_path(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| {
            ["sqlplan", "yyplan", "json", "xml", "csv", "db2plan", "plan"]
                .iter()
                .any(|e| ext.eq_ignore_ascii_case(e))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_seven_native_demo_files_import_and_save_without_a_connection() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../demo/yydbmanager/plans");
        for (name, kind, actual) in [
            ("sqlserver-plan.sqlplan", DbKind::SqlServer, true),
            ("postgresql-plan.json", DbKind::PostgreSql, true),
            ("mysql-plan.json", DbKind::MySql, false),
            ("mysql-actual.txt", DbKind::MySql, true),
            ("oracle-plan.csv", DbKind::Oracle, false),
            ("db2-plan.db2plan", DbKind::Db2, false),
            ("duckdb-plan.json", DbKind::DuckDb, true),
            ("sqlite-plan.csv", DbKind::Sqlite, false),
        ] {
            let plan = load(&root.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(plan.kind, kind, "{name}");
            assert_eq!(plan.actual, actual, "{name}");
            assert!(!plan.drawing().nodes.is_empty(), "{name}");
            let json = serde_json::to_string(&plan).unwrap();
            assert_eq!(
                serde_json::to_value(parse(&json, "yyplan").unwrap()).unwrap(),
                serde_json::to_value(plan).unwrap()
            );
        }
    }

    #[test]
    fn existing_plans_and_native_json_keep_database_metrics_and_drawing() {
        for source in [
            include_str!("../../../tests/fixtures/postgresql-complex.yyplan"),
            include_str!("../../../tests/fixtures/mysql-complex.yyplan"),
            include_str!("../../../tests/fixtures/duckdb-complex.yyplan"),
            include_str!("../../../tests/fixtures/oracle-complex.yyplan"),
            include_str!("../../../tests/fixtures/db2-complex.yyplan"),
            include_str!("../../../tests/fixtures/sqlite-complex.yyplan"),
        ] {
            let saved: Plan = serde_json::from_str(source).unwrap();
            let loaded = parse(source, "yyplan").unwrap();
            assert_eq!(
                serde_json::to_value(&saved).unwrap(),
                serde_json::to_value(&loaded).unwrap()
            );
            if matches!(
                saved.kind,
                DbKind::PostgreSql | DbKind::MySql | DbKind::DuckDb
            ) {
                let native = parse(&saved.raw, "json").unwrap();
                assert_eq!(native.kind, saved.kind);
                assert_eq!(native.nodes.len(), saved.nodes.len());
                assert_eq!(native.actual, saved.actual);
                assert!(!native.drawing().nodes.is_empty());
            }
        }
        let v2 = parse(
            r#"{"operation":"Table scan on t","estimated_rows":10,"estimated_total_cost":2}"#,
            "json",
        )
        .unwrap();
        assert_eq!(v2.kind, DbKind::MySql);
        let duck = parse(r#"[{"name":"SEQ_SCAN","extra_info":{"Table":"t","Estimated Cardinality":"3"},"children":[]}]"#, "json").unwrap();
        assert_eq!(duck.kind, DbKind::DuckDb);
        assert!(!duck.actual);
    }

    #[test]
    fn tabular_formats_keep_quotes_unicode_nulls_and_actual_units() {
        let oracle = "ID,PARENT_ID,OPERATION,OPTIONS,OBJECT_OWNER,OBJECT_NAME,CARDINALITY,COST,LAST_OUTPUT_ROWS,LAST_STARTS,LAST_ELAPSED_TIME,ACCESS_PREDICATES\r\n0,,SELECT STATEMENT,,,,3,4,6,2,1500,\r\n1,0,TABLE ACCESS,FULL,APP,表,3,4,6,2,1000,\"x='a,b'\nAND y=1\"\r\n";
        let plan = parse(oracle, "csv").unwrap();
        assert_eq!(plan.kind, DbKind::Oracle);
        assert_eq!(plan.nodes[1].parent, Some(0));
        assert_eq!(plan.nodes[1].object, "APP.表");
        assert_eq!(plan.nodes[1].properties["Actual Elapsed ms"], "1");
        assert_eq!(plan.nodes[1].properties["Actual Starts"], "2");
        assert_eq!(
            plan.nodes[1].properties["ACCESS_PREDICATES"],
            "x='a,b'\nAND y=1"
        );
        assert!(plan.actual);
        assert!(parse(&oracle.replace(",6,2,", ",6,0,"), "csv").is_err());
        assert!(parse(&oracle.replace("LAST_STARTS", "UNKNOWN_STARTS"), "csv").is_err());
        let del = "1,,\"RETURN\",,3,4.5,1,2\n2,1,\"TBSCAN\",\"APP.T\",3,4.5,1,2\n";
        let plan = parse(del, "db2plan").unwrap();
        assert_eq!(plan.kind, DbKind::Db2);
        assert!(!plan.actual);
        assert_eq!(plan.nodes[1].properties["CPU_COST"], "2");
        assert_eq!(plan.nodes[1].parent, Some(1));
        assert!(parse(del, "csv").is_err());
        let sqlite = parse(
            "id,parent,notused,detail\n3,0,0,SEARCH t USING INDEX idx (id=?)\n",
            "csv",
        )
        .unwrap();
        let json = parse(
            r#"[{"id":3,"parent":0,"notused":0,"detail":"SEARCH t USING INDEX idx (id=?)"}]"#,
            "json",
        )
        .unwrap();
        assert_eq!(sqlite.kind, DbKind::Sqlite);
        assert_eq!(sqlite.nodes[0].object, "t");
        assert_eq!(sqlite.nodes[0].operator, json.nodes[0].operator);
        assert_eq!(json.nodes[0].cost, None);
        assert_eq!(json.nodes[0].actual_rows, None);
        let tree = parse(
            "-> Table scan on t  (cost=1 rows=3) (actual time=0.1..0.2 rows=3 loops=1)\n",
            "txt",
        )
        .unwrap();
        assert_eq!(tree.kind, DbKind::MySql);
        assert!(tree.actual);
        assert_eq!(tree.nodes[0].actual_rows, Some(3.));
    }

    #[test]
    fn files_accept_bom_encodings_and_reject_invalid_or_unsupported_input() {
        let xml = include_str!("../../../tests/fixtures/sqlserver-hash-sort.sqlplan");
        let tmp = tempfile::tempdir().unwrap();
        for le in [true, false] {
            let mut bytes = if le {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for word in xml.encode_utf16() {
                bytes.extend(if le {
                    word.to_le_bytes()
                } else {
                    word.to_be_bytes()
                });
            }
            let path = tmp.path().join("plan.sqlplan");
            std::fs::write(&path, bytes).unwrap();
            let loaded = load(&path).unwrap();
            assert_eq!(loaded.kind, DbKind::SqlServer);
            assert!(loaded.actual);
        }
        let path = tmp.path().join("plan.csv");
        std::fs::write(
            &path,
            b"\xef\xbb\xbfid,parent,notused,detail\r\n1,0,0,SCAN t\r\n",
        )
        .unwrap();
        assert_eq!(load(&path).unwrap().kind, DbKind::Sqlite);
        for raw in [
            "",
            "{}",
            "[]",
            "<html/>",
            "QUERY PLAN\nSeq Scan on t",
            "id,parent,detail\n1,2,SCAN t\n2,1,SCAN t",
            "ID,PARENT_ID,OPERATION,CARDINALITY,COST\n0,,SELECT,NaN,3",
            "ID,PARENT_ID,OPERATION,CARDINALITY,COST\n0,,SELECT,3,-1",
        ] {
            assert!(
                parse(raw, "csv").is_err(),
                "accepted unsupported input: {raw}"
            );
        }
        assert!(decode(&[0xff, 0xfe, 0]).is_err());
        assert!(decode(&[0xff, 0xfe, 0x00, 0xd8]).is_err());
        assert!(decode(&[0x80]).is_err());
        let mut saved: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/sqlite-complex.yyplan"
        ))
        .unwrap();
        saved["version"] = 99.into();
        assert!(parse(&saved.to_string(), "yyplan").is_err());
        let file = std::fs::File::create(&path).unwrap();
        file.set_len((LIMIT + 1) as u64).unwrap();
        assert!(load(&path).unwrap_err().contains("32MiB"));
    }
}

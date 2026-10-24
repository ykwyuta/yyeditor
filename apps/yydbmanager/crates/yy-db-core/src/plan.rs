use crate::{DbKind, Result, ResultSet};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[path = "plan_json.rs"]
mod json;

/// Reviewed in a SQL tab; callers must never execute this automatically.
pub const DB2_ACTUAL_SETUP_SQL: &str = "-- DB管理者が必要な文を選択して実行してください。既存オブジェクトの再作成は不要です。\n-- EXPLAIN表がない場合のみ：\nCALL SYSPROC.SYSINSTALLOBJECTS('EXPLAIN','C',CAST(NULL AS VARCHAR(128)),CURRENT SCHEMA);\n-- 行編成のactivity収集用。既存ワークロードの収集にも影響するため運用方針を確認してください。\nCREATE EVENT MONITOR YYDBMANAGER_ACTIVITY FOR ACTIVITIES WRITE TO TABLE MANUALSTART;\nSET EVENT MONITOR YYDBMANAGER_ACTIVITY STATE 1;\n-- DB全体のSECTION_ACTUALS設定変更は不要です。接続単位で有効化し、取得後に復元します。\n";

pub fn drawing_renderer() -> yy_draw::render::Renderer {
    yy_draw::render::Renderer::new(Some(include_bytes!(
        "../../../../../crates/yy-win/fonts/UDEVGothic-Regular.ttf"
    )))
}
pub fn drawing_export(doc: &yy_draw::Document) -> yy_draw::render::Export {
    yy_draw::render::Export {
        background: Some(0xffffff),
        ..yy_draw::render::Export::document(doc)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: u64,
    pub parent: Option<u64>,
    pub operator: String,
    pub object: String,
    pub estimated_rows: Option<f64>,
    pub actual_rows: Option<f64>,
    pub cost: Option<f64>,
    pub properties: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub version: u32,
    pub kind: DbKind,
    pub actual: bool,
    pub raw: String,
    pub nodes: Vec<Node>,
}
impl Plan {
    pub fn tabular(kind: DbKind, set: &ResultSet) -> Result<Self> {
        if set.row_count > 10_000 {
            return Err("計画ノードは1万件までです".into());
        }
        let rows = set.page(0, 10_000)?;
        let mut nodes = vec![];
        for row in &rows {
            if row.len() < 6 {
                return Err("不正な計画表です".into());
            }
            let id = row[0]
                .display()
                .trim()
                .parse::<u64>()
                .map_err(|e| e.to_string())?;
            let parent = if row[1] == crate::Value::Null {
                None
            } else {
                Some(
                    row[1]
                        .display()
                        .trim()
                        .parse::<u64>()
                        .map_err(|e| e.to_string())?,
                )
            };
            nodes.push(Node {
                id,
                parent,
                operator: row[2].display().trim().into(),
                object: if row[3] == crate::Value::Null {
                    String::new()
                } else {
                    row[3].display()
                },
                estimated_rows: row[4].display().trim().parse().ok(),
                actual_rows: None,
                cost: row[5].display().trim().parse().ok(),
                properties: set
                    .columns
                    .iter()
                    .zip(row)
                    .skip(6)
                    .map(|(c, v)| (c.name.clone(), v.display()))
                    .collect(),
            });
        }
        if nodes.is_empty() {
            return Err("計画がありません".into());
        }
        Ok(Self {
            version: 1,
            kind,
            actual: false,
            raw: serde_json::to_string(&rows).map_err(|e| e.to_string())?,
            nodes,
        })
    }
    pub fn parse(kind: DbKind, raw: &str, actual: bool) -> Result<Self> {
        if raw.len() > 32 * 1024 * 1024 {
            return Err("実行計画は32MiBまでです".into());
        }
        let mut plan = Self {
            version: 1,
            kind,
            actual,
            raw: raw.into(),
            nodes: vec![],
        };
        if kind == DbKind::SqlServer {
            let xml = roxmltree::Document::parse(raw).map_err(|e| e.to_string())?;
            let mut ids = std::collections::HashMap::new();
            for element in xml.descendants().filter(|n| {
                n.has_tag_name("RelOp")
                    || n.has_tag_name("StmtSimple")
                        && n.children().any(|c| c.has_tag_name("QueryPlan"))
            }) {
                if plan.nodes.len() >= 10_000 {
                    return Err("計画ノードは1万件までです".into());
                }
                let id = plan.nodes.len() as u64 + 1;
                ids.insert(element.id(), id);
                if element.has_tag_name("StmtSimple") {
                    let mut properties = element
                        .attributes()
                        .map(|a| (a.name().into(), a.value().into()))
                        .collect::<BTreeMap<String, String>>();
                    for child in element
                        .children()
                        .filter(|n| n.has_tag_name("QueryPlan"))
                        .flat_map(|n| n.children())
                        .filter(|n| {
                            matches!(
                                n.tag_name().name(),
                                "MemoryGrantInfo"
                                    | "QueryTimeStats"
                                    | "MissingIndexes"
                                    | "Warnings"
                            )
                        })
                    {
                        properties
                            .insert(child.tag_name().name().into(), raw[child.range()].into());
                    }
                    plan.nodes.push(Node {
                        id,
                        parent: None,
                        operator: element
                            .attribute("StatementType")
                            .unwrap_or("SELECT")
                            .into(),
                        object: String::new(),
                        estimated_rows: element
                            .attribute("StatementEstRows")
                            .and_then(|v| v.parse().ok()),
                        actual_rows: None,
                        cost: element
                            .attribute("StatementSubTreeCost")
                            .and_then(|v| v.parse().ok()),
                        properties,
                    });
                    continue;
                }
                let parent = element
                    .ancestors()
                    .skip(1)
                    .find(|n| n.has_tag_name("RelOp") || n.has_tag_name("StmtSimple"))
                    .and_then(|p| ids.get(&p.id()).copied());
                let mut props = element
                    .attributes()
                    .map(|a| (a.name().to_owned(), a.value().to_owned()))
                    .collect::<BTreeMap<_, _>>();
                let belongs = |n: roxmltree::Node<'_, '_>| {
                    n.ancestors().find(|p| p.has_tag_name("RelOp")) == Some(element)
                };
                let object_element = element
                    .descendants()
                    .find(|n| n.has_tag_name("Object") && belongs(*n));
                let object = object_element
                    .map(|n| {
                        ["Database", "Schema", "Table", "Index"]
                            .iter()
                            .filter_map(|name| n.attribute(*name))
                            .collect::<Vec<_>>()
                            .join(".")
                    })
                    .unwrap_or_default();
                if let Some(object) = object_element {
                    for attr in object.attributes() {
                        props.insert(format!("Object.{}", attr.name()), attr.value().into());
                    }
                }
                let counters = element
                    .descendants()
                    .filter(|n| n.has_tag_name("RunTimeCountersPerThread") && belongs(*n))
                    .collect::<Vec<_>>();
                let row_counts = counters
                    .iter()
                    .filter_map(|n| {
                        n.attribute("ActualRows")
                            .and_then(|v| v.parse::<f64>().ok())
                    })
                    .collect::<Vec<_>>();
                let actual_rows = if row_counts.is_empty() {
                    None
                } else {
                    Some(row_counts.iter().sum())
                };
                for (index, counter) in counters.iter().enumerate() {
                    for attr in counter.attributes() {
                        props.insert(
                            format!("Thread{index}.{}", attr.name()),
                            attr.value().to_owned(),
                        );
                    }
                }
                let predicates = element
                    .descendants()
                    .filter(|n| n.has_tag_name("ScalarOperator") && belongs(*n))
                    .filter_map(|n| n.attribute("ScalarString"))
                    .collect::<Vec<_>>();
                if !predicates.is_empty() {
                    props.insert("Expressions".into(), predicates.join("\n"));
                }
                for item in element.descendants().filter(|n| {
                    belongs(*n)
                        && matches!(
                            n.tag_name().name(),
                            "OutputList"
                                | "SeekPredicates"
                                | "Predicate"
                                | "OrderBy"
                                | "HashKeysBuild"
                                | "HashKeysProbe"
                        )
                }) {
                    props.insert(item.tag_name().name().into(), raw[item.range()].into());
                }
                for warning in element
                    .descendants()
                    .filter(|n| n.has_tag_name("Warnings") && belongs(*n))
                {
                    props.insert(
                        "Warnings".into(),
                        warning
                            .descendants()
                            .skip(1)
                            .filter(|n| n.is_element())
                            .map(|n| {
                                format!(
                                    "{} {}",
                                    n.tag_name().name(),
                                    n.attributes()
                                        .map(|a| format!("{}={}", a.name(), a.value()))
                                        .collect::<Vec<_>>()
                                        .join(" ")
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                }
                plan.nodes.push(Node {
                    id,
                    parent,
                    operator: element.attribute("PhysicalOp").unwrap_or("Unknown").into(),
                    object,
                    estimated_rows: element
                        .attribute("EstimateRows")
                        .and_then(|s| s.parse().ok()),
                    actual_rows,
                    cost: element
                        .attribute("EstimatedTotalSubtreeCost")
                        .and_then(|s| s.parse().ok()),
                    properties: props,
                });
            }
        } else if kind == DbKind::MySql && !raw.trim_start().starts_with('{') {
            json::mysql_tree(&mut plan, raw)?;
        } else {
            let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
            json::visit(&mut plan, &value, None, 0)?;
        }
        if plan.nodes.is_empty() {
            return Err("対応する計画ノードが見つかりません。原文を確認してください".into());
        }
        if plan.nodes.iter().any(|n| n.actual_rows.is_some()) {
            plan.actual = true;
        }
        crate::plan_visual::validate(&plan)?;
        Ok(plan)
    }
    pub fn sqlite(set: &ResultSet) -> Result<Self> {
        let mut plan = Self {
            version: 1,
            kind: DbKind::Sqlite,
            actual: false,
            raw: String::new(),
            nodes: vec![],
        };
        for row in set.page(0, set.row_count as usize)? {
            if row.len() < 4 {
                return Err("不正なSQLite計画です".into());
            }
            let id = row[0].display().parse::<u64>().map_err(|e| e.to_string())?;
            let parent = row[1].display().parse::<u64>().map_err(|e| e.to_string())?;
            plan.nodes.push(Node {
                id,
                parent: if parent == 0 { None } else { Some(parent) },
                operator: row[3].display(),
                object: if row[3].display().starts_with("SCAN ")
                    || row[3].display().starts_with("SEARCH ")
                {
                    row[3]
                        .display()
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("")
                        .into()
                } else {
                    String::new()
                },
                estimated_rows: None,
                actual_rows: None,
                cost: None,
                properties: BTreeMap::from([("Detail".into(), row[3].display())]),
            });
        }
        plan.raw = serde_json::to_string(&plan.nodes).map_err(|e| e.to_string())?;
        Ok(plan)
    }
    /// SSMS-style horizontal operator scene, shared with yydraw export.
    pub fn drawing(&self) -> yy_draw::Document {
        crate::plan_visual::drawing(self)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_six_non_mssql_saved_plans_keep_native_details_and_draw() {
        for raw in [
            include_str!("../../../tests/fixtures/postgresql-complex.yyplan"),
            include_str!("../../../tests/fixtures/mysql-complex.yyplan"),
            include_str!("../../../tests/fixtures/oracle-complex.yyplan"),
            include_str!("../../../tests/fixtures/db2-complex.yyplan"),
            include_str!("../../../tests/fixtures/duckdb-complex.yyplan"),
            include_str!("../../../tests/fixtures/sqlite-complex.yyplan"),
        ] {
            let p: Plan = serde_json::from_str(raw).unwrap();
            crate::plan_visual::validate(&p).unwrap();
            assert!(p.nodes.len() >= 4);
            assert_eq!(
                p.drawing().connectors.len(),
                p.nodes.iter().filter(|n| n.parent.is_some()).count()
            );
            assert!(p.nodes.iter().any(|n| !n.properties.is_empty()));
            if matches!(p.kind, DbKind::PostgreSql | DbKind::MySql | DbKind::DuckDb) {
                let parsed = Plan::parse(p.kind, &p.raw, p.actual).unwrap();
                assert_eq!(parsed.nodes.len(), p.nodes.len());
                assert!(
                    crate::plan_visual::labels(&parsed, 0)
                        .iter()
                        .any(|s| s.contains("ms"))
                );
            }
            if p.kind == DbKind::Db2 {
                assert!(
                    p.nodes
                        .iter()
                        .any(|n| n.properties.keys().any(|k| k.starts_with("Predicate.")))
                );
            }
            if p.kind == DbKind::Oracle {
                assert!(p.nodes.iter().any(|n| n.properties.contains_key("TIME")));
            }
        }
    }
    #[test]
    fn postgres_runtime_properties_keep_arrays_units_and_loop_averages() {
        let p=Plan::parse(DbKind::PostgreSql,r#"[{"Plan":{"Node Type":"Sort","Plan Rows":2,"Actual Rows":40,"Actual Loops":3,"Actual Total Time":1.25,"Sort Space Type":"Disk","Sort Key":["t.id"],"Plans":[{"Node Type":"Index Scan","Relation Name":"t","Schema":"public","Index Name":"idx","Plan Rows":2,"Output":["id"],"Index Cond":"id > 0","Parallel Aware":true}]},"Execution Time":4.2}]"#,true).unwrap();
        assert_eq!(p.nodes[0].properties["Query.Execution Time"], "4.2");
        assert_eq!(p.nodes[1].properties["Output"], r#"["id"]"#);
        assert_eq!(p.nodes[1].object, "public.t [idx]");
        assert!(crate::plan_visual::parallel(&p.nodes[1]));
        assert!(
            crate::plan_visual::warning(&p.nodes[0])
                .unwrap()
                .contains("ディスク")
        );
        assert_eq!(crate::plan_visual::flow_rows(&p.nodes[0]), Some(120.));
        assert!(
            crate::plan_visual::labels(&p, 0)
                .iter()
                .any(|s| s == "1.25 ms / loop")
        );
        assert_eq!(crate::plan_visual::operator_cost(&p, 0), None);
    }
    #[test]
    fn mysql_nested_join_order_and_wrapper_nodes_are_preserved() {
        let p=Plan::parse(DbKind::MySql,r#"{"query_block":{"select_id":1,"cost_info":{"query_cost":"20.5"},"ordering_operation":{"using_filesort":true,"nested_loop":[{"table":{"table_name":"a","access_type":"ALL","rows_examined_per_scan":100,"filtered":"10","used_columns":["id"]}},{"table":{"table_name":"b","access_type":"ref","key":"idx","rows_produced_per_join":20,"cost_info":{"prefix_cost":"10"}}},{"table":{"table_name":"c","access_type":"eq_ref","rows_produced_per_join":20,"cost_info":{"prefix_cost":"20.5"}}}]}}}"#,false).unwrap();
        assert_eq!(p.nodes.len(), 7);
        assert_eq!(p.nodes[0].cost, Some(20.5));
        assert_eq!(p.nodes[1].operator, "Sort");
        assert_eq!(p.nodes[2].operator, "Nested Loop");
        assert_eq!(p.nodes[3].parent, Some(p.nodes[2].id));
        assert_eq!(p.nodes[4].parent, Some(p.nodes[3].id));
        assert_eq!(p.nodes[5].parent, p.nodes[4].parent);
        assert_eq!(p.nodes[6].parent, Some(p.nodes[2].id));
        assert_eq!(p.nodes[4].estimated_rows, Some(10.));
        assert_eq!(p.nodes[5].object, "b [idx]");
        assert_eq!(crate::plan_visual::cost_percent(&p, 0), None);
    }
    #[test]
    fn mysql_json_v2_keeps_access_path_tree_and_index() {
        let p=Plan::parse(DbKind::MySql,r#"{"operation":"Filter: id > 0","estimated_rows":17,"estimated_total_cost":3.668,"condition":"id > 0","inputs":[{"operation":"Index range scan on t using PRIMARY","table_name":"t","index_name":"PRIMARY","estimated_rows":17,"estimated_total_cost":3.668}]}"#,false).unwrap();
        assert_eq!(p.nodes.len(), 2);
        assert_eq!(p.nodes[1].parent, Some(1));
        assert_eq!(p.nodes[1].object, "t [PRIMARY]");
        assert_eq!(p.nodes[0].cost, Some(3.668));
        assert_eq!(p.nodes[0].estimated_rows, Some(17.));
    }
    #[test]
    fn mysql_analyze_tree_retains_rescans_and_never_executed_nodes() {
        let raw = "-> Nested loop inner join  (cost=3.2 rows=2) (actual time=0.03..0.04 rows=4 loops=1)\n    -> Table scan on a  (cost=1 rows=2) (actual time=0.01..0.02 rows=2 loops=1)\n    -> Index lookup on b using idx (id=a.id)  (cost=0.3 rows=1) (actual time=0.003..0.004 rows=2 loops=2)\n        -> Filter: false  (cost=0 rows=1) (never executed)";
        let p = Plan::parse(DbKind::MySql, raw, true).unwrap();
        assert_eq!(p.nodes.len(), 4);
        assert_eq!(p.nodes[2].parent, Some(1));
        assert_eq!(p.nodes[2].cost, Some(0.3));
        assert_eq!(crate::plan_visual::flow_rows(&p.nodes[2]), Some(4.));
        assert_eq!(p.nodes[3].properties["Actual Loops"], "0");
        assert_eq!(p.raw, raw);
    }
    #[test]
    fn duckdb_promotes_object_filters_estimates_and_seconds() {
        let p=Plan::parse(DbKind::DuckDb,r#"{"latency":0.2,"children":[{"operator_name":"SEQ_SCAN","operator_cardinality":10,"operator_timing":0.005,"extra_info":{"Table":"t","Estimated Cardinality":"1","Filters":"id>0"},"children":[]}]}"#,true).unwrap();
        assert_eq!(p.nodes[0].object, "t");
        assert_eq!(p.nodes[0].estimated_rows, Some(1.));
        assert_eq!(p.nodes[0].properties["extra_info.Filters"], "id>0");
        assert!(crate::plan_visual::warning(&p.nodes[0]).is_some());
        assert!(
            crate::plan_visual::labels(&p, 0)
                .iter()
                .any(|s| s == "5 ms")
        );
    }
    #[test]
    fn statements_have_independent_costs_and_repeated_rows() {
        let p = Plan::parse(DbKind::SqlServer, r#"<ShowPlanXML><StmtSimple StatementType="SELECT" StatementText="SELECT 1" StatementSubTreeCost="10"><QueryPlan><RelOp NodeId="0" PhysicalOp="Index Seek" EstimateRows="1" EstimateRebinds="40" EstimateRewinds="4" EstimateCPU="2" EstimateIO="0" EstimatedTotalSubtreeCost="10"><RunTimeInformation><RunTimeCountersPerThread ActualRows="20" ActualElapsedms="7"/><RunTimeCountersPerThread ActualRows="25" ActualElapsedms="9"/></RunTimeInformation><Warnings><SpillToTempDb SpillLevel="1"/></Warnings></RelOp></QueryPlan></StmtSimple><StmtSimple StatementType="SELECT" StatementText="SELECT 2" StatementSubTreeCost="100"><QueryPlan><RelOp NodeId="0" PhysicalOp="Table Scan" EstimateCPU="30" EstimateIO="0" EstimatedTotalSubtreeCost="100"/></QueryPlan></StmtSimple></ShowPlanXML>"#, true).unwrap();
        use crate::plan_visual as visual;
        assert_eq!(p.nodes.len(), 4);
        assert_eq!(p.nodes[1].parent, Some(p.nodes[0].id));
        assert_eq!(p.nodes[3].parent, Some(p.nodes[2].id));
        assert_eq!(visual::cost_percent(&p, 1), Some(20.));
        assert_eq!(visual::cost_percent(&p, 3), Some(30.));
        assert_eq!(visual::estimated_flow_rows(&p.nodes[1]), Some(45.));
        assert_eq!(visual::flow_rows(&p.nodes[1]), Some(45.));
        assert!(
            visual::warning(&p.nodes[1])
                .unwrap()
                .contains("SpillLevel=1")
        );
        assert!(
            visual::details(&p, 1, false)
                .contains(&("Actual Elapsed Time (max thread, ms)".into(), "9".into()))
        );
        assert_eq!(p.drawing().connectors.len(), 2);
    }
    #[test]
    fn real_sqlserver_plan_fixture_is_renderable() {
        let p = Plan::parse(
            DbKind::SqlServer,
            include_str!("../../../tests/fixtures/sqlserver-hash-sort.sqlplan"),
            true,
        )
        .unwrap();
        assert!(p.nodes.iter().any(|n| n.operator == "Sort"));
        assert!(p.nodes.iter().any(|n| n.operator == "Hash Match"));
        assert!(p.nodes.iter().any(|n| n.actual_rows.is_some()));
        assert_eq!(p.drawing().connectors.len(), p.nodes.len() - 1);
    }
    #[test]
    fn showplan_keeps_thread_counts_and_missing_measurements() {
        let p=Plan::parse(DbKind::SqlServer,r#"<ShowPlanXML><RelOp NodeId="0" PhysicalOp="Select" EstimateRows="2"><RelOp NodeId="1" PhysicalOp="Index Seek"><RunTimeInformation><RunTimeCountersPerThread ActualRows="3"/><RunTimeCountersPerThread ActualRows="4"/></RunTimeInformation></RelOp></RelOp></ShowPlanXML>"#,true).unwrap();
        assert_eq!(p.nodes.len(), 2);
        assert_eq!(p.nodes[0].actual_rows, None);
        assert_eq!(p.nodes[1].actual_rows, Some(7.));
        assert_eq!(p.nodes[1].parent, Some(1));
        assert_eq!(p.drawing().connectors.len(), 1);
    }
    #[test]
    fn postgres_preserves_loops_instead_of_fabricating_total_time() {
        let p=Plan::parse(DbKind::PostgreSql,r#"[{"Plan":{"Node Type":"Nested Loop","Plan Rows":10,"Plans":[{"Node Type":"Seq Scan","Actual Rows":3,"Actual Loops":4}]}}]"#,true).unwrap();
        assert_eq!(p.nodes[1].actual_rows, Some(3.));
        assert_eq!(p.nodes[1].properties["Actual Loops"], "4");
    }
}

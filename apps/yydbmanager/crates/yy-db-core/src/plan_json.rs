//! Native plan adapters. Keep native metrics; do not equate DB cost units or times.
use super::{Node, Plan};
use crate::{DbKind, Result};
use serde_json::Value;
use std::collections::BTreeMap;

fn text(v: &Value) -> String {
    v.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| v.to_string())
}
fn num(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str()?.parse().ok())
        .filter(|n| n.is_finite())
}
fn properties(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| {
            !matches!(
                k.as_str(),
                "Plans"
                    | "Plan"
                    | "children"
                    | "nested_loop"
                    | "query_block"
                    | "table"
                    | "ordering_operation"
                    | "grouping_operation"
                    | "duplicates_removal"
                    | "materialized_from_subquery"
                    | "attached_subqueries"
                    | "union_result"
                    | "query_specifications"
                    | "windowing"
                    | "inputs"
            )
        })
        .map(|(k, v)| (k.clone(), text(v)))
        .collect()
}
fn push(plan: &mut Plan, v: &Value, parent: Option<u64>, op: &str) -> Result<u64> {
    if plan.nodes.len() >= 10_000 {
        return Err("計画ノードは1万件までです".into());
    }
    let id = plan.nodes.len() as u64 + 1;
    let mut props = properties(v);
    if let Some(extra) = v.get("extra_info").and_then(Value::as_object) {
        for (k, value) in extra {
            props.insert(format!("extra_info.{k}"), text(value));
        }
    }
    if let Some(cost) = v.get("cost_info").and_then(Value::as_object) {
        for (k, value) in cost {
            props.insert(format!("cost_info.{k}"), text(value));
        }
    }
    if v.get("Parallel Aware").and_then(Value::as_bool) == Some(true)
        || v.get("Workers Planned")
            .and_then(num)
            .is_some_and(|n| n > 0.)
    {
        props.insert("Parallel".into(), "true".into());
    }
    let mut warnings = vec![];
    if v.get("Sort Space Type").and_then(Value::as_str) == Some("Disk")
        || v.get("Disk Usage").and_then(num).is_some_and(|n| n > 0.)
        || v.get("Hash Batches").and_then(num).is_some_and(|n| n > 1.)
    {
        warnings.push("ディスク退避／複数バッチ（DB統計）");
    }
    if !warnings.is_empty() {
        props.insert("Warnings".into(), warnings.join("; "));
    }
    let object = ["Schema", "Relation Name"]
        .iter()
        .filter_map(|k| v.get(k).and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(".");
    let object = if object.is_empty() {
        v.get("table_name")
            .or_else(|| v.pointer("/extra_info/Table"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    } else {
        object
    };
    let object = if let Some(index) = v
        .get("Index Name")
        .or_else(|| v.get("key"))
        .or_else(|| v.get("index_name"))
        .and_then(Value::as_str)
    {
        format!("{object} [{index}]")
    } else {
        object
    };
    let estimated = v
        .get("Plan Rows")
        .or_else(|| v.get("estimated_rows"))
        .and_then(num)
        .or_else(|| v.pointer("/extra_info/Estimated Cardinality").and_then(num))
        .or_else(|| {
            v.get("rows_examined_per_scan")
                .and_then(num)
                .map(|n| n * v.get("filtered").and_then(num).unwrap_or(100.) / 100.)
        });
    plan.nodes.push(Node {
        id,
        parent,
        operator: op.trim().into(),
        object,
        estimated_rows: estimated,
        actual_rows: v
            .get("Actual Rows")
            .or_else(|| v.get("operator_cardinality"))
            .and_then(num),
        cost: v
            .get("Total Cost")
            .or_else(|| v.get("estimated_total_cost"))
            .and_then(num),
        properties: props,
    });
    Ok(id)
}
pub(super) fn visit(plan: &mut Plan, v: &Value, parent: Option<u64>, depth: usize) -> Result<()> {
    if depth > 128 {
        return Err("計画の深さが上限を超えました".into());
    }
    if let Some(a) = v.as_array() {
        for item in a {
            visit(plan, item, parent, depth + 1)?;
        }
        return Ok(());
    }
    let Some(o) = v.as_object() else {
        return Ok(());
    };
    if plan.kind == DbKind::MySql {
        return mysql(plan, v, parent, depth);
    }
    let op = ["Node Type", "operator_name", "name"]
        .iter()
        .find_map(|k| o.get(*k).and_then(Value::as_str));
    let first = plan.nodes.len();
    let next = match op {
        Some(op) => Some(push(plan, v, parent, op)?),
        None => parent,
    };
    for key in ["Plan", "Plans", "children"] {
        if let Some(child) = o.get(key) {
            visit(plan, child, next, depth + 1)?;
        }
    }
    if op.is_none()
        && let Some(root) = plan.nodes.get_mut(first)
    {
        // PostgreSQL planning/execution totals and DuckDB profile summary belong to the query.
        for (k, v) in properties(v) {
            root.properties.insert(format!("Query.{k}"), v);
        }
    }
    Ok(())
}
fn mysql(plan: &mut Plan, v: &Value, parent: Option<u64>, depth: usize) -> Result<()> {
    if let Some(op) = v.get("operation").and_then(Value::as_str) {
        let id = push(plan, v, parent, op)?;
        if let Some(inputs) = v.get("inputs") {
            visit(plan, inputs, Some(id), depth + 1)?;
        }
        return Ok(());
    }
    let mut next = parent;
    if v.get("select_id").is_some() {
        let id = push(plan, v, parent, "SELECT")?;
        plan.nodes.last_mut().unwrap().cost = v.pointer("/cost_info/query_cost").and_then(num);
        next = Some(id);
    }
    if let Some(access) = v.get("access_type").and_then(Value::as_str) {
        let id = push(plan, v, next, access)?;
        next = Some(id);
    }
    if let Some(items) = v.get("nested_loop").and_then(Value::as_array) {
        mysql_join(plan, items, next, depth + 1)?;
    }
    for key in [
        "query_block",
        "table",
        "ordering_operation",
        "grouping_operation",
        "duplicates_removal",
        "materialized_from_subquery",
        "attached_subqueries",
        "union_result",
        "query_specifications",
        "windowing",
    ] {
        if let Some(child) = v.get(key) {
            let op = match key {
                "ordering_operation" => Some("Sort"),
                "grouping_operation" => Some("Aggregate"),
                "duplicates_removal" => Some("Distinct"),
                "materialized_from_subquery" => Some("Materialize"),
                "union_result" => Some("Union"),
                "windowing" => Some("Window"),
                _ => None,
            };
            let parent = if let Some(op) = op {
                Some(push(plan, child, next, op)?)
            } else {
                next
            };
            visit(plan, child, parent, depth + 1)?;
        }
    }
    Ok(())
}
fn mysql_join(plan: &mut Plan, items: &[Value], parent: Option<u64>, depth: usize) -> Result<()> {
    if depth > 128 {
        return Err("計画の深さが上限を超えました".into());
    }
    let Some((last, previous)) = items.split_last() else {
        return Ok(());
    };
    if previous.is_empty() {
        return visit(plan, last, parent, depth + 1);
    }
    let table = last.get("table").unwrap_or(last);
    let op = if table
        .get("using_join_buffer")
        .and_then(Value::as_str)
        .is_some_and(|s| s.to_ascii_lowercase().contains("hash"))
    {
        "Hash Join"
    } else {
        "Nested Loop"
    };
    let id = push(plan, &Value::Null, parent, op)?;
    let node = plan.nodes.last_mut().unwrap();
    node.estimated_rows = table.get("rows_produced_per_join").and_then(num);
    node.cost = table.pointer("/cost_info/prefix_cost").and_then(num);
    node.properties.insert(
        "Join order".into(),
        "JSON nested_loop order; outer input first".into(),
    );
    mysql_join(plan, previous, Some(id), depth + 1)?;
    visit(plan, last, Some(id), depth + 1)
}

/// MySQL 8 EXPLAIN ANALYZE FORMAT=TREE (JSON ANALYZE is unavailable on 8.4).
pub(super) fn mysql_tree(plan: &mut Plan, raw: &str) -> Result<()> {
    let mut stack: Vec<(usize, u64)> = vec![];
    let mut lines: Vec<String> = vec![];
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        if line.contains("->") {
            lines.push(line.into());
        } else if let Some(last) = lines.last_mut() {
            last.push(' ');
            last.push_str(line.trim());
        }
    }
    for line in &lines {
        let Some((indent, body)) = line.find("->").map(|i| (i, line[i + 2..].trim())) else {
            continue;
        };
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        if stack.len() > 128 {
            return Err("計画の深さが上限を超えました".into());
        }
        let operator = body
            .split(" (cost=")
            .next()
            .unwrap_or(body)
            .split(" (actual ")
            .next()
            .unwrap_or(body)
            .trim();
        let id = push(
            plan,
            &Value::Null,
            stack.last().map(|(_, id)| *id),
            operator,
        )?;
        let node = plan.nodes.last_mut().unwrap();
        node.properties.insert("Native TREE".into(), body.into());
        if let Some((_, target)) = operator.split_once(" on ") {
            let table = target.split_whitespace().next().unwrap_or("");
            let index = target
                .split_once(" using ")
                .and_then(|(_, s)| s.split_whitespace().next());
            node.object = index
                .map(|i| format!("{table} [{i}]"))
                .unwrap_or_else(|| table.into());
        }
        if let Some(cost) = body
            .split("(cost=")
            .nth(1)
            .and_then(|s| s.split(')').next())
        {
            let (cost, rest) = cost.split_once(' ').unwrap_or((cost, ""));
            node.cost = cost.split("..").last().and_then(|s| s.parse().ok());
            node.estimated_rows = rest
                .split("rows=")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse().ok());
        }
        if let Some(actual) = body
            .split("(actual time=")
            .nth(1)
            .and_then(|s| s.split(')').next())
        {
            let mut parts = actual.split_whitespace();
            if let Some(time) = parts.next() {
                let mut times = time.split("..");
                if let Some(start) = times.next() {
                    node.properties
                        .insert("Actual Startup Time".into(), start.into());
                }
                if let Some(end) = times.next() {
                    node.properties
                        .insert("Actual Total Time".into(), end.into());
                }
            }
            for item in parts {
                if let Some(n) = item.strip_prefix("rows=") {
                    node.actual_rows = n.parse().ok();
                }
                if let Some(n) = item.strip_prefix("loops=") {
                    node.properties.insert("Actual Loops".into(), n.into());
                }
            }
        } else if body.contains("(never executed)") {
            node.actual_rows = Some(0.);
            node.properties.insert("Actual Loops".into(), "0".into());
        }
        stack.push((indent, id));
    }
    Ok(())
}

//! SSMS-style presentation. Symbols are drawn locally; no SSMS artwork is bundled.
use crate::{
    DbKind,
    plan::{Node, Plan},
};
use std::collections::{HashMap, VecDeque};
use yy_draw::{Document, Point};

pub const CARD_WIDTH: f64 = 208.;
pub const CARD_HEIGHT: f64 = 164.;
const COLUMN: f64 = 272.;
const ROW: f64 = 180.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Node(usize),
    Edge(usize),
}

pub fn number(value: Option<f64>) -> String {
    let Some(value) = value.filter(|v| v.is_finite()) else {
        return "—".into();
    };
    if value.abs() >= 1_000_000_000. {
        format!("{value:.3e}")
    } else if value.fract().abs() < 0.000001 {
        format!("{value:.0}")
    } else {
        format!("{value:.3}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}
fn metric(node: &Node, key: &str) -> Option<f64> {
    node.properties
        .get(key)?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}
pub fn warning(node: &Node) -> Option<&str> {
    node.properties
        .get("Warnings")
        .filter(|v| !v.is_empty())
        .map(String::as_str)
        .or_else(|| {
            row_error(node)
                .filter(|r| *r >= 10.)
                .map(|_| "行数推定が実測と10倍以上乖離（yydbmanagerによる比較）")
        })
}
fn row_error(node: &Node) -> Option<f64> {
    let (estimate, actual) = node.estimated_rows.zip(node.actual_rows)?;
    let actual = actual
        / metric(node, "Actual Starts")
            .filter(|v| *v > 0.)
            .unwrap_or(1.);
    if estimate < 0. || actual < 0. || metric(node, "Actual Loops") == Some(0.) {
        return None;
    }
    // A zero versus nonzero is still useful, but is not an infinite measured ratio.
    (estimate > 0. && actual > 0.).then(|| estimate.max(actual) / estimate.min(actual))
}
fn semantics(kind: DbKind) -> &'static str {
    match kind {
        DbKind::SqlServer => {
            "コストはオプティマイザー推定値。Cost %は文内の演算子比率。実測時間とは異なります"
        }
        DbKind::PostgreSql => {
            "コストは相対単位（子の処理を含む）。実測行数・時間はループ平均、BUFFERSは子の処理を含みます"
        }
        DbKind::MySql => {
            "JSONのprefix_costは結合順序の累積値。TREEの行数・時間はループ平均。コストは時間ではありません"
        }
        DbKind::Oracle => {
            "コストとTIMEは推定値。LAST_OUTPUT_ROWS/経過時間は最後の実行の全starts合計。経過時間は子を含むms"
        }
        DbKind::Db2 => {
            "TOTAL_COSTは累積timeron、IO_COSTはページI/O、CPU_COSTは命令数。実測CARDINALITYは全パーティション合計。演算子時間ではありません"
        }
        DbKind::Sqlite => {
            "EXPLAIN QUERY PLANのアクセス方法。推定コスト・行数・実測時間はDBから提供されません"
        }
        DbKind::DuckDb => {
            "operator_timingは演算子の秒数。並列処理のため合計はクエリの壁時計時間と一致しません"
        }
    }
}
pub fn display_operator(kind: DbKind, node: &Node) -> String {
    let native = node.operator.trim();
    if kind == DbKind::MySql && node.properties.contains_key("Native TREE") {
        let lower = native.to_ascii_lowercase();
        for (needle, title) in [
            ("nested loop", "Nested Loop"),
            ("hash join", "Hash Join"),
            ("aggregate", "Aggregate"),
            ("table scan", "Table Scan"),
            ("index lookup", "Index Lookup"),
            ("index range scan", "Index Range Search"),
            ("index scan", "Index Scan"),
            ("sort:", "Sort"),
            ("filter:", "Filter"),
            ("materialize", "Materialize"),
            ("limit", "Limit"),
        ] {
            if lower.contains(needle) {
                return title.into();
            }
        }
    }
    let name = match (kind, native) {
        (DbKind::Db2, "TBSCAN") => "Table Scan",
        (DbKind::Db2, "IXSCAN" | "EISCAN" | "XISCAN") => "Index Scan",
        (DbKind::Db2, "RIDSCN") => "Row Identifier Scan",
        (DbKind::Db2, "FETCH") => "Row Lookup",
        (DbKind::Db2, "NLJOIN") => "Nested Loop Join",
        (DbKind::Db2, "HSJOIN") => "Hash Join",
        (DbKind::Db2, "MSJOIN") => "Merge Join",
        (DbKind::Db2, "GRPBY") => "Aggregate",
        (DbKind::Db2, "RETURN") => "SELECT",
        (DbKind::Db2, "TEMP") => "Materialize",
        (DbKind::Db2, "TQ" | "REBAL") => "Parallel Exchange",
        (DbKind::Db2, "UNIQUE") => "Distinct Aggregate",
        (DbKind::MySql, "ALL") => "Table Scan",
        (DbKind::MySql, "index") => "Index Scan",
        (DbKind::MySql, "range") => "Index Range Search",
        (
            DbKind::MySql,
            "ref" | "eq_ref" | "ref_or_null" | "const" | "system" | "unique_subquery"
            | "index_subquery",
        ) => "Index Lookup",
        (DbKind::MySql, "index_merge") => "Index Merge",
        _ => native,
    };
    if kind == DbKind::Oracle && native.starts_with("TABLE ACCESS") {
        return if native.contains("ROWID") {
            "Row Lookup".into()
        } else {
            "Table Scan".into()
        };
    }
    if kind == DbKind::Oracle && native.contains("GROUP BY") {
        return "Aggregate".into();
    }
    if kind == DbKind::DuckDb && (native.contains("GROUP_BY") || native == "UNGROUPED_AGGREGATE") {
        return "Aggregate".into();
    }
    if kind == DbKind::Sqlite {
        if native.starts_with("SEARCH ") {
            return "Index Search".into();
        }
        if native.starts_with("SCAN ") {
            return "Table / Index Scan".into();
        }
        if native.starts_with("USE TEMP B-TREE") {
            return "Sort / Temporary B-tree".into();
        }
    }
    name.into()
}
pub fn parallel(node: &Node) -> bool {
    node.properties
        .get("Parallel")
        .is_some_and(|s| s.eq_ignore_ascii_case("true") || s == "1")
}
pub fn flow_rows(node: &Node) -> Option<f64> {
    node.actual_rows
        .map(|v| v * metric(node, "Actual Loops").unwrap_or(1.))
        .or_else(|| estimated_flow_rows(node))
        .filter(|n| n.is_finite() && *n >= 0.)
}
pub fn estimated_flow_rows(node: &Node) -> Option<f64> {
    node.estimated_rows.map(|v| {
        v * metric(node, "Actual Starts").unwrap_or(1.)
            * (1.
                + metric(node, "EstimateRebinds").unwrap_or(0.)
                + metric(node, "EstimateRewinds").unwrap_or(0.))
    })
}
fn elapsed(node: &Node) -> Option<f64> {
    if let Some(ms) = metric(node, "Actual Elapsed ms") {
        return Some(ms);
    }
    if let Some(ms) = metric(node, "Actual Total Time") {
        return Some(ms);
    }
    if let Some(seconds) = metric(node, "operator_timing") {
        return Some(seconds * 1000.);
    }
    node.properties
        .iter()
        .filter(|(key, _)| key.starts_with("Thread") && key.ends_with(".ActualElapsedms"))
        .filter_map(|(_, v)| v.parse::<f64>().ok())
        .reduce(f64::max)
}
pub fn flow_width(node: &Node) -> f64 {
    flow_rows(node)
        .map(|n| 1. + n.max(1.).log10() * 1.25)
        .unwrap_or(1.)
        .clamp(1., 9.)
}

/// Cost percentages are estimates within one statement, never measured elapsed time.
pub fn operator_cost(plan: &Plan, index: usize) -> Option<f64> {
    let node = plan.nodes.get(index)?;
    if node.properties.contains_key("StatementText") {
        return Some(0.);
    }
    if plan.kind == DbKind::SqlServer
        && let (Some(cpu), Some(io)) = (metric(node, "EstimateCPU"), metric(node, "EstimateIO"))
    {
        return Some(cpu + io);
    }
    // Inclusive costs, rescans and parallel branches cannot be subtracted reliably.
    if plan.kind != DbKind::SqlServer {
        return None;
    }
    let cost = node.cost?;
    let mut children = 0.;
    for child in plan.nodes.iter().filter(|n| n.parent == Some(node.id)) {
        children += child.cost?;
    }
    let cost = cost - children;
    (cost >= -0.000001 && cost.is_finite()).then_some(cost.max(0.))
}
pub fn cost_percent(plan: &Plan, index: usize) -> Option<f64> {
    if plan.kind != DbKind::SqlServer {
        return None;
    }
    let map = plan
        .nodes
        .iter()
        .map(|n| (n.id, n))
        .collect::<HashMap<_, _>>();
    let mut root = plan.nodes.get(index)?;
    for _ in 0..plan.nodes.len() {
        match root.parent.and_then(|id| map.get(&id)) {
            Some(parent) => root = parent,
            None => break,
        }
    }
    let total = root.cost.filter(|c| c.is_finite() && *c > 0.)?;
    Some(operator_cost(plan, index)? / total * 100.)
}

pub fn details(plan: &Plan, index: usize, edge: bool) -> Vec<(String, String)> {
    let Some(node) = plan.nodes.get(index) else {
        return vec![];
    };
    let mut rows: Vec<(String, String)> = vec![];
    let mut push = |name: &str, value: String| rows.push((name.into(), value));
    push(
        "表示対象",
        if edge {
            "データフロー".into()
        } else {
            node.operator.clone()
        },
    );
    push(
        "Node ID",
        node.properties
            .get("NodeId")
            .cloned()
            .unwrap_or_else(|| node.id.to_string()),
    );
    push("Physical Operation", node.operator.clone());
    push("処理内容", display_operator(plan.kind, node));
    push("統計の意味", semantics(plan.kind).into());
    if let Some(logical) = node.properties.get("LogicalOp") {
        push("Logical Operation", logical.clone());
    }
    if !node.object.is_empty() {
        push("Object", node.object.clone());
    }
    if edge {
        push(
            "行数の基準",
            if node.actual_rows.is_some() {
                "実測".into()
            } else {
                "推定（実測未取得）".into()
            },
        );
        if let Some(size) = metric(node, "AvgRowSize")
            .zip(flow_rows(node))
            .map(|(size, rows)| size * rows)
        {
            push("データ量の目安 (bytes)", number(Some(size)));
        }
        if let Some(bytes) = metric(node, "BYTES") {
            push("推定データ量 (bytes)", number(Some(bytes)));
        }
    }
    push("Estimated Number of Rows", number(node.estimated_rows));
    if node.properties.contains_key("EstimateRebinds")
        || node.properties.contains_key("Actual Starts")
    {
        push(
            "Estimated Rows (all executions)",
            number(estimated_flow_rows(node)),
        );
    }
    if node.actual_rows.is_some() {
        push("Actual Number of Rows", number(node.actual_rows));
        if node.properties.contains_key("Actual Loops") {
            push("Actual Loops", number(metric(node, "Actual Loops")));
            push("Actual Rows (all executions)", number(flow_rows(node)));
            push("行数・時間の基準", "Actual Rows / Actual Total Time は1ループ平均。全実行行数は行数×loops。時間の合計は壁時計時間ではありません".into());
        }
        if let Some(ratio) = row_error(node) {
            push(
                "行数推定の乖離（大きい側／小さい側）",
                format!("{ratio:.1}倍（1実行あたり）"),
            );
        }
    }
    if elapsed(node).is_some() {
        push(
            if node.properties.contains_key("Actual Total Time") {
                "Actual Total Time (per loop, inclusive, ms)"
            } else if node.properties.contains_key("Actual Elapsed ms") {
                "Actual Elapsed Time (all starts, inclusive, ms)"
            } else if node.properties.contains_key("operator_timing") {
                "Operator Timing (ms; not query wall time)"
            } else {
                "Actual Elapsed Time (max thread, ms)"
            },
            number(elapsed(node)),
        );
    }
    if let Some(cost) = operator_cost(plan, index) {
        push("Estimated Operator Cost", number(Some(cost)));
    }
    if let Some(cost) = cost_percent(plan, index) {
        push("Estimated Cost %", format!("{cost:.1}%"));
    }
    if node.cost.is_some() {
        push(
            if plan.kind == DbKind::SqlServer {
                "Estimated Subtree Cost"
            } else {
                "Native Estimated Cost"
            },
            number(node.cost),
        );
    }
    if let Some(warnings) = warning(node) {
        push("Warnings", warnings.into());
    }
    if parallel(node) {
        push("Parallel", "True".into());
    }
    // Promote actionable attributes before the complete native property list.
    for key in [
        "Filter",
        "Index Cond",
        "Recheck Cond",
        "Hash Cond",
        "Merge Cond",
        "Join Filter",
        "ACCESS_PREDICATES",
        "FILTER_PREDICATES",
        "attached_condition",
        "extra_info.Filters",
        "extra_info.Conditions",
        "Sort Key",
        "Group Key",
        "Output",
        "used_columns",
        "used_key_parts",
        "possible_keys",
        "Sort Method",
        "Sort Space Used",
        "Sort Space Type",
        "Hash Batches",
        "Peak Memory Usage",
        "Shared Hit Blocks",
        "Shared Read Blocks",
        "Temp Read Blocks",
        "Temp Written Blocks",
        "Query.Execution Time",
        "Query.Planning Time",
    ] {
        if let Some(value) = node.properties.get(key) {
            push(key, value.clone());
        }
    }
    for (key, value) in &node.properties {
        if !matches!(
            key.as_str(),
            "Warnings" | "LogicalOp" | "NodeId" | "Parallel"
        ) {
            push(key, value.clone());
        }
    }
    let mut seen = std::collections::HashSet::new();
    rows.retain(|(key, _)| seen.insert(key.clone()));
    rows
}
pub fn tooltip(plan: &Plan, hit: Hit) -> String {
    let (index, edge) = match hit {
        Hit::Node(i) => (i, false),
        Hit::Edge(i) => (i, true),
    };
    details(plan, index, edge)
        .into_iter()
        .take(22)
        .map(|(k, v)| format!("{k}: {}", v.chars().take(350).collect::<String>()))
        .collect::<Vec<_>>()
        .join("\r\n")
}

pub fn validate(plan: &Plan) -> crate::Result<()> {
    if plan.nodes.is_empty() || plan.nodes.len() > 10_000 {
        return Err("計画のノード数が不正です".into());
    }
    let map = plan
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id, i))
        .collect::<HashMap<_, _>>();
    if map.len() != plan.nodes.len() {
        return Err("計画のNode IDが重複しています".into());
    }
    let mut children = vec![vec![]; plan.nodes.len()];
    let mut queue = VecDeque::new();
    for (i, node) in plan.nodes.iter().enumerate() {
        if let Some(parent) = node.parent {
            let &p = map.get(&parent).ok_or("計画の親ノードがありません")?;
            children[p].push(i);
        } else {
            queue.push_back(i);
        }
    }
    let mut count = 0;
    while let Some(i) = queue.pop_front() {
        count += 1;
        queue.extend(&children[i]);
    }
    if count != plan.nodes.len() {
        return Err("計画の親子関係に循環があります".into());
    }
    Ok(())
}

pub fn drawing(plan: &Plan) -> Document {
    use yy_draw::*;
    let mut doc = Document {
        show_grid: false,
        snap: false,
        jumps: false,
        ..Default::default()
    };
    if validate(plan).is_err() {
        return doc;
    }
    let map = plan
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id, i))
        .collect::<HashMap<_, _>>();
    let mut children = vec![vec![]; plan.nodes.len()];
    let mut roots = vec![];
    for (i, n) in plan.nodes.iter().enumerate() {
        if let Some(parent) = n.parent {
            children[map[&parent]].push(i);
        } else {
            roots.push(i);
        }
    }
    let mut sizes = vec![1usize; plan.nodes.len()];
    let mut order = vec![];
    let mut stack = roots.clone();
    while let Some(i) = stack.pop() {
        order.push(i);
        stack.extend(&children[i]);
    }
    for &i in order.iter().rev() {
        if !children[i].is_empty() {
            sizes[i] = children[i].iter().map(|&c| sizes[c]).sum();
        }
    }
    let mut positions = vec![(0usize, 0usize); plan.nodes.len()];
    let mut root_of = vec![0; plan.nodes.len()];
    let mut lane = 0;
    for &root in &roots {
        let mut stack = vec![(root, 0, lane)];
        while let Some((i, depth, row)) = stack.pop() {
            positions[i] = (depth, row);
            root_of[i] = root;
            let mut next = row;
            for &child in &children[i] {
                stack.push((child, depth + 1, next));
                next += sizes[child];
            }
        }
        lane += sizes[root] + 1;
    }
    for (i, node) in plan.nodes.iter().enumerate() {
        let (depth, row) = positions[i];
        let asset_id = doc.id();
        let own = if node.properties.contains_key("StatementText") {
            Some(0.)
        } else if let (Some(cpu), Some(io)) =
            (metric(node, "EstimateCPU"), metric(node, "EstimateIO"))
        {
            Some(cpu + io)
        } else if plan.kind == DbKind::SqlServer {
            node.cost
                .zip(
                    children[i]
                        .iter()
                        .try_fold(0., |sum, &c| Some(sum + plan.nodes[c].cost?)),
                )
                .and_then(|(cost, children)| {
                    ((cost - children) >= -0.000001).then_some((cost - children).max(0.))
                })
        } else {
            None
        };
        let percent = if plan.kind == DbKind::SqlServer {
            own.zip(plan.nodes[root_of[i]].cost)
                .filter(|(_, total)| *total > 0. && total.is_finite())
                .map(|(cost, total)| cost / total * 100.)
        } else {
            None
        };
        doc.assets.push(Asset {
            id: asset_id,
            name: node.operator.clone(),
            source: "yydbmanager/operator-symbols".into(),
            svg: card(plan, i, percent),
        });
        doc.add_node(
            Shape::Icon,
            Point::new(24. + depth as f64 * COLUMN, 72. + row as f64 * ROW),
            Some(asset_id),
        );
        let n = doc.nodes.last_mut().unwrap();
        n.rect.w = CARD_WIDTH;
        n.rect.h = CARD_HEIGHT;
        n.label.clear();
    }
    for (i, node) in plan.nodes.iter().enumerate() {
        if let Some(parent) = node.parent {
            let source = doc.nodes[i].rect;
            let target = doc.nodes[map[&parent]].rect;
            let start = Point::new(source.x + CARD_WIDTH / 2. - 21., source.y + 21.);
            let end = Point::new(target.x + CARD_WIDTH / 2. + 21., target.y + 21.);
            doc.connect(Endpoint::Free(start), Endpoint::Free(end), false);
            let c = doc.connectors.last_mut().unwrap();
            c.color = 0x606060;
            c.width = flow_width(node);
            c.jump = Jump::Never;
            if (start.y - end.y).abs() > 1. {
                let mid = (start.x + end.x) / 2.;
                c.waypoints = vec![Point::new(mid, start.y), Point::new(mid, end.y)];
            }
        }
    }
    let batch_cost = roots
        .iter()
        .filter_map(|&i| plan.nodes[i].cost)
        .filter(|c| c.is_finite())
        .sum::<f64>();
    for (ordinal, &root) in roots.iter().enumerate() {
        let node = &plan.nodes[root];
        let y = doc.nodes[root].rect.y - 60.;
        let relative = if roots.len() > 1 && batch_cost > 0. {
            node.cost
                .map(|v| format!(" / バッチ内の推定コスト {:.1}%", v / batch_cost * 100.))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let text = node
            .properties
            .get("StatementText")
            .or_else(|| node.properties.get("Query.Text"))
            .map(|v| v.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        let header = format!(
            "Query {} · {} · {}{}\n{}",
            ordinal + 1,
            plan.kind.name(),
            if plan.actual {
                "実測計画（コストは推定値）"
            } else {
                "推定実行計画"
            },
            relative,
            shorten(&text, 110)
        );
        let asset_id = doc.id();
        let mut svg =
            "<svg xmlns='http://www.w3.org/2000/svg' width='800' height='40' viewBox='0 0 800 40'>"
                .to_owned();
        for (i, line) in header.lines().enumerate() {
            svg.push_str(&format!("<text x='0' y='{}' font-family='UDEV Gothic' font-size='12' fill='#202020'>{}</text>",14+i*17,yy_draw::render::escape(line)));
        }
        svg.push_str("</svg>");
        doc.assets.push(Asset {
            id: asset_id,
            name: "Query header".into(),
            source: "yydbmanager/plan-header".into(),
            svg,
        });
        doc.add_node(Shape::Icon, Point::new(24., y), Some(asset_id));
        let n = doc.nodes.last_mut().unwrap();
        n.label.clear();
        n.rect.w = 800.;
        n.rect.h = 64.;
        n.font_size = 12.;
    }
    doc
}

fn shorten(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let value = chars.by_ref().take(max).collect::<String>();
    if chars.next().is_some() {
        format!("{value}…")
    } else {
        value
    }
}
fn lines(text: &str, width: usize, max: usize) -> Vec<String> {
    let mut out = vec![];
    let mut line = String::new();
    let mut count = 0;
    for c in text.chars() {
        if count >= width {
            out.push(std::mem::take(&mut line));
            count = 0;
            if out.len() == max {
                break;
            }
        }
        line.push(c);
        count += if c.is_ascii() { 1 } else { 2 };
    }
    if !line.is_empty() && out.len() < max {
        out.push(line);
    }
    if text.chars().count() > width * max
        && let Some(last) = out.last_mut()
    {
        last.push('…');
    }
    out
}
fn card(plan: &Plan, index: usize, percent: Option<f64>) -> String {
    use yy_draw::render::escape;
    let node = &plan.nodes[index];
    let mut svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='208' height='140' viewBox='0 0 208 140'><g transform='translate(88 4)'>{}</g>",
        symbol(&display_operator(plan.kind, node))
    );
    if warning(node).is_some() {
        svg.push_str("<g transform='translate(114 0)'><path d='M8 0L16 15H0Z' fill='#ffe179' stroke='#9c6b00'/><path d='M8 4V9M8 11V13' stroke='#302000' stroke-width='2'/></g>");
    }
    if parallel(node) {
        svg.push_str("<g transform='translate(82 24)' fill='#f3b43e' stroke='#846016'><path d='M0 0H12L8 4H12L8 8H0L4 4H0Z'/></g>");
    }
    for (i, label) in labels_with_cost(plan, index, percent)
        .into_iter()
        .enumerate()
    {
        svg.push_str(&format!("<text x='104' y='{}' text-anchor='middle' font-family='UDEV Gothic' font-size='11.5' fill='#202020'>{}</text>",51.+i as f64*14.,escape(&label)));
    }
    svg.push_str("</svg>");
    svg
}
pub fn labels(plan: &Plan, index: usize) -> Vec<String> {
    labels_with_cost(plan, index, cost_percent(plan, index))
}
fn labels_with_cost(plan: &Plan, index: usize, percent: Option<f64>) -> Vec<String> {
    let node = &plan.nodes[index];
    let display = display_operator(plan.kind, node);
    let mut labels = lines(&display, 30, 1);
    if display != node.operator
        && plan.kind != DbKind::Sqlite
        && !node.properties.contains_key("Native TREE")
    {
        labels.push(format!("({})", shorten(&node.operator, 28)));
    }
    if let Some(logical) = node
        .properties
        .get("LogicalOp")
        .filter(|s| s.as_str() != node.operator)
    {
        labels.push(format!("({})", shorten(logical, 26)));
    }
    if !node.object.is_empty() {
        let compact = ["Object.Table", "Object.Index"]
            .iter()
            .filter_map(|key| node.properties.get(*key))
            .cloned()
            .collect::<Vec<_>>()
            .join(".");
        labels.extend(lines(
            if compact.is_empty() {
                &node.object
            } else {
                &compact
            },
            30,
            2,
        ));
    }
    if let Some(cost) = percent {
        labels.push(format!("Cost: {cost:.0}%"));
    } else if node.cost.is_some() {
        labels.push(format!(
            "推定コスト: {}{}",
            number(node.cost),
            if plan.kind == DbKind::Db2 {
                " timeron"
            } else {
                ""
            }
        ));
    }
    if node.actual_rows.is_some() {
        if let Some(ms) = elapsed(node) {
            labels.push(format!(
                "{} ms{}",
                number(Some(ms)),
                if node.properties.contains_key("Actual Total Time") {
                    " / loop"
                } else {
                    ""
                }
            ));
        }
        labels.push(format!(
            "行数 {} / 推定 {}{}",
            number(if node.properties.contains_key("Actual Loops") {
                node.actual_rows
            } else {
                flow_rows(node)
            }),
            number(estimated_flow_rows(node)),
            if node.properties.contains_key("Actual Loops") {
                " / loop"
            } else {
                ""
            }
        ));
    } else if node.estimated_rows.is_some() {
        labels.push(format!("推定行数: {}", number(node.estimated_rows)));
    }
    labels.truncate(7);
    labels
}

/// Recognizable relational-operation symbols, drawn at 32 DIP.
fn symbol(operator: &str) -> String {
    let op = operator.to_ascii_lowercase();
    let body = if op == "select" {
        "<path d='M5 2H21L27 8V29H5Z' fill='#eef5ff'/><path d='M21 2V8H27M9 12H21M9 17H21M9 22H16'/><path d='M0 20H12M5 15L0 20L5 25' stroke='#c08b14' stroke-width='2.5'/>"
    } else if op.contains("seek")
        || op.contains("lookup")
        || op.contains("index")
        || op.contains("scan")
        || op.starts_with("search")
    {
        if op.contains("seek") || op.contains("lookup") || op.starts_with("search") {
            "<path d='M2 3H25V25H2Z' fill='#fbe6a0'/><path d='M2 10H25M2 17H25M9 3V25M17 3V25'/><circle cx='21' cy='20' r='7' fill='#e7f3ff'/><path d='M26 25L31 31' stroke-width='3'/>"
        } else {
            "<path d='M3 4H28V28H3Z' fill='#fbe6a0'/><path d='M3 11H28M3 19H28M11 4V28M20 4V28'/><path d='M0 15H15M11 11L15 15L11 19' stroke='#2771ac' stroke-width='2.5'/>"
        }
    } else if op.contains("nested") || op.contains("loop") {
        "<path d='M2 8H21V23H11V15H30M25 10L30 15L25 20M7 3L2 8L7 13' fill='none' stroke='#a47b18' stroke-width='3'/><circle cx='11' cy='23' r='3' fill='#4e90bf'/>"
    } else if op.contains("hash") {
        "<path d='M3 6H29V26H3Z' fill='#e8f3ff'/><path d='M12 8L9 24M22 8L19 24M7 13H26M6 20H25' stroke-width='2.5'/>"
    } else if op.contains("merge") || op.contains("concat") || op.contains("union") {
        "<path d='M30 3H22L12 15H1M30 27H22L12 15M6 10L1 15L6 20' fill='none' stroke='#347fb4' stroke-width='3'/><path d='M22 2V8M22 24V30' stroke='#bc901c' stroke-width='4'/>"
    } else if op.contains("sort") || op.contains("order") {
        "<path d='M7 4V29M2 9L7 4L12 9M24 4V29M19 24L24 29L29 24' fill='none' stroke-width='2.5'/><path d='M15 7H18M13 15H18M11 23H18' stroke='#bc901c' stroke-width='2'/>"
    } else if op.contains("filter") {
        "<path d='M2 4H30L19 17V28L13 25V17Z' fill='#fbe6a0'/><path d='M8 9H24'/>"
    } else if op.contains("aggregate") || op.contains("group") || op.contains("aggr") {
        "<path d='M26 4H6L17 16L6 28H27' fill='none' stroke-width='3'/><path d='M26 4V9M27 23V28' stroke-width='3'/>"
    } else if op.contains("compute") || op.contains("projection") || op.contains("scalar") {
        "<rect x='5' y='2' width='23' height='28' rx='2' fill='#e3effa'/><path d='M9 7H24V12H9ZM10 17H14M12 15V19M19 17H24M10 24H14M19 22L24 26M24 22L19 26'/>"
    } else if op.contains("parallel") || op.contains("exchange") || op.contains("stream") {
        "<path d='M30 4H23L15 16H2M30 16H2M30 28H23L15 16M7 11L2 16L7 21' fill='none' stroke-width='2.5'/><circle cx='24' cy='4' r='3' fill='#f3bd43'/><circle cx='24' cy='28' r='3' fill='#f3bd43'/>"
    } else if op.contains("spool") || op.contains("material") {
        "<path d='M5 7V26C5 32 27 32 27 26V7' fill='#fbe6a0'/><ellipse cx='16' cy='7' rx='11' ry='5' fill='#fff1c9'/><path d='M5 16C5 22 27 22 27 16' fill='none'/>"
    } else if op.contains("top") || op.contains("limit") {
        "<path d='M3 4H29M16 28V9M9 16L16 9L23 16' fill='none' stroke-width='3'/>"
    } else if op.contains("insert") || op.contains("update") || op.contains("delete") {
        "<path d='M4 3H27V28H4Z' fill='#fbe6a0'/><path d='M4 11H27M4 20H27M12 3V28'/><path d='M17 26L29 14L32 17L20 29L16 30Z' fill='#e9f3fd'/>"
    } else {
        "<rect x='4' y='4' width='24' height='24' rx='3' fill='#e8f1fa'/><path d='M9 10H23M9 16H23M9 22H23'/>"
    };
    format!("<g stroke='#326895' stroke-width='1.4' stroke-linejoin='round'>{body}</g>")
}

pub fn hit(plan: &Plan, doc: &Document, p: Point, tolerance: f64) -> Option<Hit> {
    if let Some(i) = doc
        .nodes
        .iter()
        .take(plan.nodes.len())
        .position(|n| n.rect.contains(p))
    {
        return Some(Hit::Node(i));
    }
    let sources = plan
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.parent.is_some())
        .map(|(i, _)| i);
    for (edge, index) in doc.connectors.iter().zip(sources) {
        let mut points = vec![doc.endpoint(edge.start)];
        points.extend(&edge.waypoints);
        points.push(doc.endpoint(edge.end));
        if points
            .windows(2)
            .any(|pair| distance(p, pair[0], pair[1]) <= tolerance.max(edge.width / 2. + 2.))
        {
            return Some(Hit::Edge(index));
        }
    }
    None
}
fn distance(p: Point, a: Point, b: Point) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length = dx * dx + dy * dy;
    if length == 0. {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / length).clamp(0., 1.);
    p.distance(Point::new(a.x + t * dx, a.y + t * dy))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Plan {
        Plan::parse(DbKind::SqlServer,r#"<ShowPlanXML><RelOp NodeId="0" PhysicalOp="Hash Match" LogicalOp="Inner Join" EstimateRows="1000" EstimatedTotalSubtreeCost="10" EstimateCPU="1" EstimateIO="0"><RelOp NodeId="1" PhysicalOp="Index Seek" EstimateRows="100" EstimatedTotalSubtreeCost="2" EstimateCPU="1" EstimateIO="1"/><RelOp NodeId="2" PhysicalOp="Table Scan" EstimateRows="100000" EstimatedTotalSubtreeCost="7" EstimateCPU="2" EstimateIO="5"><Warnings><SpillToTempDb SpillLevel="1"/></Warnings></RelOp></RelOp></ShowPlanXML>"#,false).unwrap()
    }
    #[test]
    fn horizontal_flow_costs_symbols_and_hit_testing() {
        let plan = fixture();
        let doc = drawing(&plan);
        assert!(doc.nodes[1].rect.x > doc.nodes[0].rect.x);
        assert!(doc.nodes[2].rect.y > doc.nodes[1].rect.y);
        assert_eq!(cost_percent(&plan, 1), Some(20.));
        assert!(doc.connectors[1].width > doc.connectors[0].width);
        assert!(doc.endpoint(doc.connectors[0].start).x > doc.endpoint(doc.connectors[0].end).x);
        assert!(doc.assets[2].svg.contains("#ffe179"));
        let source = doc.endpoint(doc.connectors[0].start);
        let target = doc.endpoint(doc.connectors[0].end);
        assert_eq!(
            hit(
                &plan,
                &doc,
                Point::new((source.x + target.x) / 2., source.y),
                3.
            ),
            Some(Hit::Edge(1))
        );
        assert!(tooltip(&plan, Hit::Node(1)).contains("Estimated Cost %: 20.0%"));
    }
    #[test]
    fn cycles_duplicates_and_unknown_parents_are_rejected() {
        let mut plan = fixture();
        plan.nodes[0].parent = Some(2);
        assert!(validate(&plan).is_err());
        plan.nodes[0].parent = None;
        plan.nodes[1].id = plan.nodes[0].id;
        assert!(validate(&plan).is_err());
    }
}

use super::*;
use std::collections::HashMap;
use std::fmt::Write as _;

#[derive(Clone, Debug)]
pub struct RelationSuggestion {
    pub relation: Relation,
    pub reason: String,
}
fn normalized(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn datatype(s: &str) -> String {
    match s
        .split(['(', ' '])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "int" | "integer" | "int4" => "integer".into(),
        "bigint" | "int8" => "bigint".into(),
        "varchar" | "nvarchar" | "varchar2" | "text" | "char" | "nchar" => "text".into(),
        "decimal" | "numeric" | "number" => "decimal".into(),
        value => value.into(),
    }
}
pub(super) fn unique(e: &Entity, columns: &[String]) -> bool {
    e.keys
        .iter()
        .any(|k| k.columns.iter().all(|c| columns.contains(c)))
        || e.indexes
            .iter()
            .any(|k| k.unique && k.columns.iter().all(|c| columns.contains(c)))
}
impl Model {
    /// Relationship endpoints are retained so semantic associations remain readable.
    pub fn important_field(&self, e: &Entity, f: &Field) -> bool {
        f.important
            || e.keys
                .iter()
                .any(|k| k.primary && k.columns.contains(&f.name))
            || self.relations.iter().any(|r| {
                (r.child == e.id && r.columns.contains(&f.name))
                    || (r.parent == e.id && r.referenced_columns.contains(&f.name))
            })
    }
    /// Suggestions do not mutate the model. Generic bare ID/name columns are not
    /// sufficient evidence; table-qualified names and logical names also match.
    pub fn recommend_logical_relations(&self, limit: usize) -> Vec<RelationSuggestion> {
        let mut targets: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        for (ei, e) in self.entities.iter().enumerate().filter(|(_, e)| e.visible) {
            for (fi, f) in e.fields.iter().enumerate() {
                if !f.important && !e.keys.iter().any(|k| k.columns.contains(&f.name)) {
                    continue;
                }
                let mut aliases = vec![normalized(&f.name), normalized(&f.logical_name)];
                aliases.push(normalized(&format!("{}_{}", e.id.name, f.name)));
                if !e.logical_name.is_empty() && !f.logical_name.is_empty() {
                    aliases.push(normalized(&format!("{}{}", e.logical_name, f.logical_name)));
                }
                aliases.sort();
                aliases.dedup();
                for alias in aliases.into_iter().filter(|s| {
                    !s.is_empty() && !["id", "name", "code", "番号", "名称"].contains(&s.as_str())
                }) {
                    targets.entry(alias).or_default().push((ei, fi));
                }
            }
        }
        let mut output = vec![];
        let mut seen = HashSet::new();
        for (ci, child) in self.entities.iter().enumerate().filter(|(_, e)| e.visible) {
            for (cfi, field) in child.fields.iter().enumerate() {
                for alias in [normalized(&field.name), normalized(&field.logical_name)] {
                    for &(pi, pfi) in targets.get(&alias).into_iter().flatten() {
                        if output.len() >= limit {
                            return output;
                        }
                        if (ci == pi && cfi == pfi) || !seen.insert((ci, cfi, pi, pfi)) {
                            continue;
                        }
                        let parent = &self.entities[pi];
                        let pf = &parent.fields[pfi];
                        if datatype(&field.data_type) != datatype(&pf.data_type)
                            || self.relations.iter().any(|r| {
                                r.child == child.id
                                    && r.parent == parent.id
                                    && r.columns
                                        .iter()
                                        .zip(&r.referenced_columns)
                                        .any(|(c, p)| c == &field.name && p == &pf.name)
                            })
                        {
                            continue;
                        }
                        let mut suffix = output.len() + 1;
                        let name = loop {
                            let name = format!("LR_recommended_{suffix}");
                            if !self
                                .relations
                                .iter()
                                .any(|r| r.child == child.id && r.name == name)
                                && !output.iter().any(|r: &RelationSuggestion| {
                                    r.relation.child == child.id && r.relation.name == name
                                })
                            {
                                break name;
                            }
                            suffix += 1;
                        };
                        output.push(RelationSuggestion {
                            relation: Relation {
                                logical: true,
                                name,
                                child: child.id.clone(),
                                parent: parent.id.clone(),
                                columns: vec![field.name.clone()],
                                referenced_columns: vec![pf.name.clone()],
                            },
                            reason: "物理名・論理名・表名付き列名の一致、型の整合（意味は要確認）"
                                .into(),
                        });
                    }
                }
            }
        }
        output
    }
    pub fn markdown(&self) -> Result<String> {
        self.validate()?;
        let tables = self
            .entities
            .iter()
            .enumerate()
            .filter(|(_, e)| e.visible)
            .collect::<Vec<_>>();
        if tables.is_empty() {
            return Err("出力する表示対象テーブルがありません".into());
        }
        let ids = tables
            .iter()
            .map(|(i, e)| (&e.id, *i))
            .collect::<HashMap<_, _>>();
        let mut out = format!(
            "# テーブル設計書\n\n対象DB: {}\n\n表示対象テーブルを出力しています。設計書は全列、ER図は主キー・外部キー・主要項目・論理関係の接続列を表示します。\n\n<a id=\"table-list\"></a>\n\n## テーブル一覧\n\n| テーブル | 論理名 | 種別 |\n| --- | --- | --- |\n",
            self.kind.name()
        );
        for (i, e) in &tables {
            writeln!(
                out,
                "| [{}](#table-{i}) | {} | {} |",
                md(&e.id.display()),
                md(&e.logical_name),
                if e.view { "VIEW" } else { "TABLE" }
            )
            .unwrap();
        }
        out.push_str("\n## ER図\n\n実線は外部キー制約、破線は論理リレーションです。論理関係の存在・件数はDBで保証されません。LFは論理関係の接続列、★は主要項目です。\n\n```mermaid\nerDiagram\n");
        for (i, e) in &tables {
            writeln!(
                out,
                "    T{i}[\"{}\"] {{",
                mm(&format!("{} {}", e.id.display(), e.logical_name))
            )
            .unwrap();
            let mut attribute_names = HashSet::new();
            for (fi, f) in e
                .fields
                .iter()
                .enumerate()
                .filter(|(_, f)| self.important_field(e, f))
            {
                let pk = e
                    .keys
                    .iter()
                    .any(|k| k.primary && k.columns.contains(&f.name));
                let fk = self
                    .relations
                    .iter()
                    .any(|r| !r.logical && r.child == e.id && r.columns.contains(&f.name));
                let key = match (pk, fk) {
                    (true, true) => " PK, FK",
                    (true, false) => " PK",
                    (false, true) => " FK",
                    _ => "",
                };
                let mut token = token(&f.name, &format!("column_{fi}"));
                while !attribute_names.insert(token.clone()) {
                    token.push('_');
                }
                let flags = self.field_flags(e, f);
                writeln!(
                    out,
                    "        {} {}{} \"{}\"",
                    token_type(&f.data_type),
                    token,
                    key,
                    mm(&format!(
                        "{} {} / {} {}",
                        f.name, f.logical_name, f.data_type, flags
                    ))
                )
                .unwrap();
            }
            out.push_str("    }\n");
        }
        for r in &self.relations {
            let (Some(ci), Some(pi)) = (ids.get(&r.child), ids.get(&r.parent)) else {
                continue;
            };
            let c = &self.entities[*ci];
            let p = &self.entities[*pi];
            let left = if r.logical && !unique(p, &r.referenced_columns) {
                "}o"
            } else if !r.logical
                && r.columns
                    .iter()
                    .all(|col| c.fields.iter().any(|f| &f.name == col && !f.nullable))
            {
                "||"
            } else {
                "|o"
            };
            let right = if unique(c, &r.columns) { "o|" } else { "o{" };
            writeln!(
                out,
                "    T{pi} {left}{}{right} T{ci} : \"{}\"",
                if r.logical { ".." } else { "--" },
                mm(&format!(
                    "{}{}",
                    if r.logical { "論理: " } else { "" },
                    r.name
                ))
            )
            .unwrap();
        }
        out.push_str("```\n");
        for (i, e) in tables {
            writeln!(out, "\n<a id=\"table-{i}\"></a>\n\n## {}\n\n論理名: {}\n\n[テーブル一覧へ戻る](#table-list)\n\n### 列定義\n\n| 物理名 | 論理名 | 型 | NULL許可 | キー・主要項目 | 既定値 |\n| --- | --- | --- | --- | --- | --- |", md(&e.id.display()), md(&e.logical_name)).unwrap();
            for f in &e.fields {
                writeln!(
                    out,
                    "| {} | {} | {} | {} | {} | {} |",
                    md(&f.name),
                    md(&f.logical_name),
                    md(&f.data_type),
                    if f.nullable { "可" } else { "不可" },
                    self.field_flags(e, f),
                    md(f.default_sql.as_deref().unwrap_or(""))
                )
                .unwrap();
            }
            out.push_str(
                "\n### キー・索引\n\n| 名前 | 種別 | 構成列（順序） |\n| --- | --- | --- |\n",
            );
            for k in &e.keys {
                writeln!(
                    out,
                    "| {} | {} | {} |",
                    md(&k.name),
                    if k.primary {
                        "主キー"
                    } else {
                        "一意キー"
                    },
                    md(&k.columns.join(", "))
                )
                .unwrap();
            }
            for k in &e.indexes {
                writeln!(
                    out,
                    "| {} | {} | {} |",
                    md(&k.name),
                    if k.unique { "一意索引" } else { "索引" },
                    md(&k.columns.join(", "))
                )
                .unwrap();
            }
            out.push_str("\n### リレーション（参照元・参照先）\n\n| 名前 | 種別 | 参照元 | 参照先 | 列の対応 |\n| --- | --- | --- | --- | --- |\n");
            for r in self
                .relations
                .iter()
                .filter(|r| r.child == e.id || r.parent == e.id)
            {
                let link = |id: &TableId| {
                    ids.get(id)
                        .map(|i| format!("[{}](#table-{i})", md(&id.display())))
                        .unwrap_or_else(|| md(&id.display()))
                };
                let pairs = r
                    .columns
                    .iter()
                    .zip(&r.referenced_columns)
                    .map(|(c, p)| format!("{c} → {p}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(
                    out,
                    "| {} | {} | {} | {} | {} |",
                    md(&r.name),
                    if r.logical {
                        "論理（制約なし）"
                    } else {
                        "外部キー制約"
                    },
                    link(&r.child),
                    link(&r.parent),
                    md(&pairs)
                )
                .unwrap();
            }
        }
        Ok(out)
    }
    fn field_flags(&self, e: &Entity, f: &Field) -> String {
        let mut flags = vec![];
        if e.keys
            .iter()
            .any(|k| k.primary && k.columns.contains(&f.name))
        {
            flags.push("PK");
        }
        if self
            .relations
            .iter()
            .any(|r| !r.logical && r.child == e.id && r.columns.contains(&f.name))
        {
            flags.push("FK");
        }
        if self.relations.iter().any(|r| {
            r.logical
                && ((r.child == e.id && r.columns.contains(&f.name))
                    || (r.parent == e.id && r.referenced_columns.contains(&f.name)))
        }) {
            flags.push("LF");
        }
        if f.important {
            flags.push("★主要");
        }
        flags.join(", ")
    }
    pub fn export_markdown(&self, path: &std::path::Path) -> Result<()> {
        let markdown = self.markdown()?;
        let mut temp = tempfile::NamedTempFile::new_in(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new(".")),
        )
        .map_err(|e| e.to_string())?;
        use std::io::Write;
        temp.write_all(markdown.as_bytes())
            .map_err(|e| e.to_string())?;
        temp.as_file().sync_all().map_err(|e| e.to_string())?;
        temp.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
}
fn md(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace('[', "&#91;")
        .replace(']', "&#93;")
        .replace('*', "&#42;")
        .replace('_', "&#95;")
        .replace('`', "&#96;")
        .replace('\\', "&#92;")
        .replace(['\r', '\n'], " ")
}
fn mm(s: &str) -> String {
    s.replace('"', "＂")
        .replace('`', "｀")
        .replace('\\', "＼")
        .replace('<', "＜")
        .replace('>', "＞")
        .replace('&', "＆")
        .replace(['\r', '\n'], " ")
}
fn token(s: &str, fallback: &str) -> String {
    if s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && !["PK", "FK", "UK"].contains(&s.to_ascii_uppercase().as_str())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        s.into()
    } else {
        fallback.into()
    }
}
fn token_type(s: &str) -> String {
    token(s.split(['(', ' ']).next().unwrap_or(""), "value")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model() -> Model {
        let mut db = Database::open(&crate::Options::memory(DbKind::Sqlite)).unwrap();
        db.execute("CREATE TABLE customer(id INTEGER PRIMARY KEY, display_name TEXT, internal_note TEXT); CREATE TABLE department(id INTEGER PRIMARY KEY); CREATE TABLE orders(id INTEGER PRIMARY KEY, customer_id INTEGER, customer_name TEXT, department_id INTEGER REFERENCES department(id), detail TEXT); CREATE TABLE wrong_type(id INTEGER PRIMARY KEY, customer_id TEXT); CREATE TABLE keyless(payload TEXT)", &AtomicBool::new(false)).unwrap();
        let mut m = db.er_model().unwrap();
        let customer = m
            .entities
            .iter_mut()
            .find(|e| e.id.name == "customer")
            .unwrap();
        customer.logical_name = "顧客|<>&[x]".into();
        customer
            .fields
            .iter_mut()
            .find(|f| f.name == "display_name")
            .unwrap()
            .important = true;
        m
    }
    #[test]
    fn recommendations_require_confirmation_skip_generic_ids_types_and_existing_pairs() {
        let mut m = model();
        let before = serde_json::to_string(&m).unwrap();
        let proposed = m.recommend_logical_relations(200);
        assert_eq!(serde_json::to_string(&m).unwrap(), before);
        assert_eq!(proposed.len(), 1);
        let r = &proposed[0].relation;
        assert!(r.logical);
        assert_eq!(r.child.name, "orders");
        assert_eq!(r.parent.name, "customer");
        assert_eq!(r.columns, ["customer_id"]);
        assert_eq!(r.referenced_columns, ["id"]);
        assert!(m.recommend_logical_relations(0).is_empty());
        m.replace_relation(None, r.clone()).unwrap();
        assert!(m.recommend_logical_relations(200).is_empty());
    }
    #[test]
    fn logical_non_unique_relations_filter_diagrams_roundtrip_and_never_change_ddl() {
        let mut m = model();
        let customer = m
            .entities
            .iter()
            .find(|e| e.id.name == "customer")
            .unwrap()
            .id
            .clone();
        let orders = m
            .entities
            .iter()
            .find(|e| e.id.name == "orders")
            .unwrap()
            .id
            .clone();
        let physical = m.clone();
        m.replace_relation(
            None,
            Relation {
                logical: true,
                name: "LR_customer_name".into(),
                child: orders.clone(),
                parent: customer.clone(),
                columns: vec!["customer_name".into()],
                referenced_columns: vec!["display_name".into()],
            },
        )
        .unwrap();
        let mut as_constraint = m.relations.last().unwrap().clone();
        as_constraint.logical = false;
        assert!(
            m.replace_relation(Some(m.relations.len() - 1), as_constraint)
                .is_err()
        );
        m.replace_relation(
            None,
            Relation {
                logical: true,
                name: "LR_reverse".into(),
                child: customer,
                parent: orders,
                columns: vec!["display_name".into()],
                referenced_columns: vec!["customer_name".into()],
            },
        )
        .unwrap();
        for kind in DbKind::ALL {
            m.kind = kind;
            let mut expected = physical.clone();
            expected.kind = kind;
            assert_eq!(
                m.ddl().unwrap().sql,
                expected.ddl().unwrap().sql,
                "{kind:?}"
            );
        }
        m.level = Level::Important;
        let mut drawing = m.drawing();
        assert_eq!(drawing.connectors.iter().filter(|c| c.dashed).count(), 2);
        let cards = drawing
            .assets
            .iter()
            .filter(|a| a.name != "cardinality")
            .map(|a| a.svg.as_str())
            .collect::<String>();
        assert!(!cards.contains("internal_note") && !cards.contains("detail"));
        assert!(
            cards.contains("display_name")
                && cards.contains("customer_name")
                && cards.contains("department_id")
        );
        m.move_entity(0, yy_draw::Point::new(1000., 250.), &mut drawing);
        assert_eq!(drawing.nodes, m.drawing().nodes);
        assert_eq!(drawing.connectors, m.drawing().connectors);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("semantic.yyer");
        m.save(&path).unwrap();
        let loaded = Model::load(&path).unwrap();
        assert_eq!(loaded.version, 4);
        assert_eq!(
            serde_json::to_value(&loaded).unwrap()["relations"],
            serde_json::to_value(&m).unwrap()["relations"]
        );
        assert!(
            loaded
                .entities
                .iter()
                .flat_map(|e| &e.fields)
                .any(|f| f.important)
        );
        assert_eq!(loaded.level, Level::Important);
    }
    #[test]
    fn markdown_has_full_definitions_safe_links_and_a_reduced_mermaid_diagram() {
        let mut m = model();
        m.relations
            .push(m.recommend_logical_relations(1).remove(0).relation);
        let text = m.markdown().unwrap();
        let diagram = text
            .split("```mermaid\n")
            .nth(1)
            .unwrap()
            .split("```")
            .next()
            .unwrap();
        assert!(diagram.contains("erDiagram") && diagram.contains("..") && diagram.contains("--"));
        assert!(diagram.contains("display_name") && diagram.contains("customer_id"));
        assert!(!diagram.contains("internal_note") && !diagram.contains("detail"));
        assert!(text.contains("internal&#95;note") && text.contains("detail"));
        assert!(text.contains("顧客&#124;&lt;&gt;&amp;&#91;x&#93;"));
        assert!(text.contains("論理（制約なし）"));
        for i in 0..m.entities.len() {
            assert!(text.contains(&format!("(#table-{i})")));
            assert!(text.contains(&format!("id=\"table-{i}\"")));
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tables.md");
        m.export_markdown(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        if let Some(output) = std::env::var_os("YYDBMANAGER_MARKDOWN_FIXTURE") {
            std::fs::write(output, &text).unwrap();
        }
        m.entities.iter_mut().for_each(|e| e.visible = false);
        assert!(m.export_markdown(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
}

use super::*;
#[derive(Debug)]
pub struct Ddl {
    pub sql: String,
    pub notes: Vec<String>,
}
fn identifier(value: &str) -> Result<()> {
    crate::sql::identifier(value).map(|_| ())
}
pub(super) fn validate_entity(e: &Entity) -> Result<()> {
    identifier(&e.id.name)?;
    if !e.id.schema.is_empty() {
        identifier(&e.id.schema)?;
    }
    if e.keys.iter().filter(|k| k.primary).count() > 1 {
        return Err("主キーは表ごとに1つです".into());
    }
    let mut names = HashSet::new();
    for f in &e.fields {
        identifier(&f.name)?;
        if f.data_type.contains('\0') {
            return Err("列型に不正な文字があります".into());
        }
    }
    for k in &e.keys {
        identifier(&k.name)?;
        if !names.insert(&k.name)
            || k.columns.iter().collect::<HashSet<_>>().len() != k.columns.len()
        {
            return Err("キー名またはキー列が重複しています".into());
        }
    }
    let mut names = HashSet::new();
    for i in &e.indexes {
        identifier(&i.name)?;
        if !names.insert(&i.name)
            || i.columns.is_empty()
            || i.columns.iter().collect::<HashSet<_>>().len() != i.columns.len()
            || i.columns
                .iter()
                .any(|c| !e.fields.iter().any(|f| &f.name == c))
        {
            return Err("索引名または索引列が不正です".into());
        }
    }
    Ok(())
}
impl Model {
    /// The proposed replacement is validated before committing any mutation.
    pub fn replace_entity(&mut self, index: usize, entity: Entity) -> Result<()> {
        let old = self
            .entities
            .get(index)
            .ok_or("テーブルがありません")?
            .id
            .clone();
        let mut proposal = self.clone();
        proposal.entities[index] = entity;
        for r in &mut proposal.relations {
            if r.child == old {
                r.child = proposal.entities[index].id.clone();
            }
            if r.parent == old {
                r.parent = proposal.entities[index].id.clone();
            }
        }
        for r in proposal
            .relations
            .iter()
            .filter(|r| !r.logical && r.parent == proposal.entities[index].id)
        {
            if self.entities[index]
                .keys
                .iter()
                .any(|k| k.columns == r.referenced_columns)
                && !proposal.entities[index]
                    .keys
                    .iter()
                    .any(|k| k.columns == r.referenced_columns)
            {
                return Err("リレーションが参照するキーは削除できません".into());
            }
        }
        proposal.validate()?;
        *self = proposal;
        Ok(())
    }
    /// Replace a table's column grid atomically. Original indices retain identity
    /// across renames, swaps and moves; None denotes a newly inserted column.
    pub fn replace_fields(&mut self, entity: usize, rows: &[(Option<usize>, Field)]) -> Result<()> {
        let original = self.entities.get(entity).ok_or("テーブルがありません")?;
        if rows.len() > 4096 {
            return Err("列は4096個までです".into());
        }
        let mut mapping = std::collections::HashMap::new();
        let mut names = HashSet::new();
        for (row, (index, field)) in rows.iter().enumerate() {
            let mut check = || -> Result<()> {
                identifier(&field.name)?;
                if !names.insert(&field.name) {
                    return Err(format!("物理名「{}」が重複しています", field.name));
                }
                if field.data_type.trim().is_empty() {
                    return Err("データ型を指定してください".into());
                }
                fragment(&field.data_type, true, self.kind)?;
                if let Some(default) = &field.default_sql {
                    fragment(default, false, self.kind)?;
                }
                Ok(())
            };
            check().map_err(|e| format!("{}行目: {e}", row + 1))?;
            if let Some(index) = index {
                let old = original.fields.get(*index).ok_or("元の列がありません")?;
                if mapping
                    .insert(old.name.clone(), field.name.clone())
                    .is_some()
                {
                    return Err("元の列が重複しています".into());
                }
            }
        }
        let rename = |columns: &mut Vec<String>| -> Result<()> {
            for name in columns {
                *name = mapping
                    .get(name)
                    .ok_or_else(|| {
                        format!(
                            "列「{name}」はキー・索引・リレーションで使用中のため削除できません"
                        )
                    })?
                    .clone();
            }
            Ok(())
        };
        let mut proposal = self.clone();
        let e = &mut proposal.entities[entity];
        e.fields = rows.iter().map(|(_, field)| field.clone()).collect();
        for key in &mut e.keys {
            rename(&mut key.columns)?;
        }
        for index in &mut e.indexes {
            rename(&mut index.columns)?;
        }
        if e.keys
            .iter()
            .filter(|k| k.primary)
            .flat_map(|k| &k.columns)
            .any(|name| e.fields.iter().any(|f| &f.name == name && f.nullable))
        {
            return Err("主キーの列はNULL許可に変更できません".into());
        }
        for r in &mut proposal.relations {
            if r.child == original.id {
                rename(&mut r.columns)?;
            }
            if r.parent == original.id {
                rename(&mut r.referenced_columns)?;
            }
        }
        proposal.validate()?;
        *self = proposal;
        Ok(())
    }
    pub fn replace_field(
        &mut self,
        entity: usize,
        index: Option<usize>,
        field: Field,
    ) -> Result<()> {
        let mut proposal = self.clone();
        let e = proposal
            .entities
            .get_mut(entity)
            .ok_or("テーブルがありません")?;
        if field.nullable
            && index.and_then(|i| e.fields.get(i)).is_some_and(|f| {
                e.keys
                    .iter()
                    .any(|k| k.primary && k.columns.contains(&f.name))
            })
        {
            return Err("主キーの列はNULL許可に変更できません".into());
        }
        fragment(&field.data_type, true, self.kind)?;
        if let Some(default) = &field.default_sql {
            fragment(default, false, self.kind)?;
        }
        if let Some(i) = index {
            let old = e.fields.get(i).ok_or("列がありません")?.name.clone();
            let new = field.name.clone();
            e.fields[i] = field;
            for k in &mut e.keys {
                for c in &mut k.columns {
                    if *c == old {
                        *c = new.clone();
                    }
                }
            }
            for k in &mut e.indexes {
                for c in &mut k.columns {
                    if *c == old {
                        *c = new.clone();
                    }
                }
            }
            for r in &mut proposal.relations {
                if r.child == e.id {
                    for c in &mut r.columns {
                        if *c == old {
                            *c = new.clone();
                        }
                    }
                }
                if r.parent == e.id {
                    for c in &mut r.referenced_columns {
                        if *c == old {
                            *c = new.clone();
                        }
                    }
                }
            }
        } else {
            e.fields.push(field);
        }
        proposal.validate()?;
        *self = proposal;
        Ok(())
    }
    pub fn delete_field(&mut self, entity: usize, index: usize) -> Result<()> {
        let mut proposal = self.clone();
        let e = proposal
            .entities
            .get_mut(entity)
            .ok_or("テーブルがありません")?;
        if index >= e.fields.len() {
            return Err("列がありません".into());
        }
        e.fields.remove(index);
        proposal.validate().map_err(|_|"キー・索引・リレーションで使用中の列は削除できません。先に対応する定義を編集してください".to_string())?;
        *self = proposal;
        Ok(())
    }
    pub fn replace_relation(&mut self, index: Option<usize>, relation: Relation) -> Result<()> {
        identifier(&relation.name)?;
        let mut proposal = self.clone();
        let parent = proposal
            .entities
            .iter()
            .find(|e| e.id == relation.parent)
            .ok_or("参照先テーブルがありません")?;
        if !relation.logical
            && !parent
                .keys
                .iter()
                .any(|k| k.columns == relation.referenced_columns)
        {
            return Err("参照先の列と順序は主キーまたは一意キーに合わせてください".into());
        }
        if proposal
            .relations
            .iter()
            .enumerate()
            .any(|(i, r)| Some(i) != index && r.child == relation.child && r.name == relation.name)
        {
            return Err("外部キー名が重複しています".into());
        }
        if let Some(i) = index {
            let slot = proposal
                .relations
                .get_mut(i)
                .ok_or("リレーションがありません")?;
            *slot = relation;
        } else {
            proposal.relations.push(relation);
        }
        proposal.validate()?;
        *self = proposal;
        Ok(())
    }
    /// Create SQL only. The caller chooses whether, where and when to execute it.
    pub fn ddl(&self) -> Result<Ddl> {
        self.validate()?;
        let kind = self.kind;
        let quote = |s: &str| crate::sql::identifier_for(kind, s);
        let table = |id: &TableId| -> Result<String> {
            if id.schema.is_empty() {
                return quote(&id.name);
            }
            Ok(format!("{}.{}", quote(&id.schema)?, quote(&id.name)?))
        };
        let columns = |cols: &[String]| -> Result<String> {
            Ok(cols
                .iter()
                .map(|c| quote(c))
                .collect::<Result<Vec<_>>>()?
                .join(", "))
        };
        let mut included = self
            .entities
            .iter()
            .enumerate()
            .filter(|(_, e)| e.visible && !e.view)
            .map(|(i, _)| i)
            .collect::<HashSet<_>>();
        if included.is_empty() {
            return Err("表示中のテーブルがありません".into());
        }
        // Include dependencies even if their table card is hidden.
        loop {
            let before = included.len();
            for r in self.relations.iter().filter(|r| !r.logical) {
                if self
                    .entities
                    .iter()
                    .enumerate()
                    .any(|(i, e)| included.contains(&i) && e.id == r.child)
                {
                    let (i, e) = self
                        .entities
                        .iter()
                        .enumerate()
                        .find(|(_, e)| e.id == r.parent)
                        .ok_or("参照先がモデル外です。依存先を取得してからDDLを生成してください")?;
                    if e.view {
                        return Err("ビューを参照する外部キーはDDL生成できません".into());
                    }
                    included.insert(i);
                }
            }
            if included.len() == before {
                break;
            }
        }
        let inline = matches!(kind, DbKind::Sqlite | DbKind::DuckDb);
        for r in self.relations.iter().filter(|r| !r.logical) {
            if included.iter().any(|i| self.entities[*i].id == r.child) {
                let parent = self
                    .entities
                    .iter()
                    .find(|e| e.id == r.parent)
                    .ok_or("参照先がありません")?;
                if !parent
                    .keys
                    .iter()
                    .any(|k| k.columns == r.referenced_columns)
                {
                    return Err(format!(
                        "{} の参照先を主キー／一意キーとして定義してください",
                        r.name
                    ));
                }
            }
        }
        let mut order = vec![];
        let mut pending = included.clone();
        while !pending.is_empty() {
            let next = pending
                .iter()
                .copied()
                .filter(|i| {
                    kind != DbKind::DuckDb
                        || !self.relations.iter().any(|r| {
                            !r.logical
                                && r.child == self.entities[*i].id
                                && self
                                    .entities
                                    .iter()
                                    .enumerate()
                                    .any(|(p, e)| pending.contains(&p) && e.id == r.parent)
                        })
                })
                .min();
            let i = next.ok_or("DuckDBでは自己参照・循環外部キーを生成できません")?;
            pending.remove(&i);
            order.push(i);
        }
        let mut sql = String::new();
        let mut notes = vec![];
        if included
            .iter()
            .any(|i| self.entities[*i].catalog_definition)
        {
            notes.push("逆生成したモデルのDDLは設計用です。型の桁数、既定値、生成列、照合順序、CHECK、索引等を完全には復元していません。列定義を確認してください。".into());
        }
        if self.entities.iter().any(|e| e.visible && e.view) {
            notes.push("ビューは定義SQLを保持していないため出力しません。".into());
        }
        if included.iter().any(|i| !self.entities[*i].visible) {
            notes.push("非表示の参照先テーブルも依存関係のため出力します。".into());
        }
        for note in &notes {
            sql.push_str(&format!("-- {note}\n"));
        }
        if kind == DbKind::Sqlite {
            sql.push_str("PRAGMA foreign_keys = ON;\n");
        }
        for i in &order {
            let e = &self.entities[*i];
            if e.fields.is_empty() {
                return Err(format!("{} に列がありません", e.id.display()));
            }
            let mut items = vec![];
            for f in &e.fields {
                fragment(&f.data_type, true, kind)?;
                let mut line = format!("  {} {}", quote(&f.name)?, f.data_type.trim());
                if let Some(default) = &f.default_sql {
                    fragment(default, false, kind)?;
                    line.push_str(&format!(" DEFAULT {}", default.trim()));
                }
                if !f.nullable {
                    line.push_str(" NOT NULL");
                }
                items.push(line);
            }
            for k in &e.keys {
                items.push(format!(
                    "  CONSTRAINT {} {} ({})",
                    quote(&k.name)?,
                    if k.primary { "PRIMARY KEY" } else { "UNIQUE" },
                    columns(&k.columns)?
                ));
            }
            if inline {
                for r in self
                    .relations
                    .iter()
                    .filter(|r| !r.logical && r.child == e.id)
                {
                    let parent = self
                        .entities
                        .iter()
                        .find(|e| e.id == r.parent)
                        .ok_or("参照先がありません")?;
                    if !parent
                        .keys
                        .iter()
                        .any(|k| k.columns == r.referenced_columns)
                    {
                        return Err(format!(
                            "{} の参照先は主キー／一意キーではありません",
                            r.name
                        ));
                    }
                    let target = if kind == DbKind::Sqlite {
                        if r.child.schema != r.parent.schema {
                            return Err("SQLiteの外部キーは同じスキーマ内に限ります".into());
                        }
                        quote(&r.parent.name)?
                    } else {
                        table(&r.parent)?
                    };
                    items.push(format!(
                        "  CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
                        quote(&r.name)?,
                        columns(&r.columns)?,
                        target,
                        columns(&r.referenced_columns)?
                    ));
                }
            }
            sql.push_str(&format!(
                "\nCREATE TABLE {} (\n{}\n);\n",
                table(&e.id)?,
                items.join(",\n")
            ));
        }
        if !inline {
            for r in self.relations.iter().filter(|r| !r.logical) {
                if included.iter().any(|i| self.entities[*i].id == r.child) {
                    sql.push_str(&format!(
                        "\nALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({});\n",
                        table(&r.child)?,
                        quote(&r.name)?,
                        columns(&r.columns)?,
                        table(&r.parent)?,
                        columns(&r.referenced_columns)?
                    ));
                }
            }
        }
        for i in order {
            let e = &self.entities[i];
            for index in &e.indexes {
                let index_name = if kind == DbKind::Sqlite && !e.id.schema.is_empty() {
                    format!("{}.{}", quote(&e.id.schema)?, quote(&index.name)?)
                } else {
                    quote(&index.name)?
                };
                let target = if kind == DbKind::Sqlite {
                    quote(&e.id.name)?
                } else {
                    table(&e.id)?
                };
                sql.push_str(&format!(
                    "\nCREATE {}INDEX {} ON {} ({});\n",
                    if index.unique { "UNIQUE " } else { "" },
                    index_name,
                    target,
                    columns(&index.columns)?
                ));
            }
        }
        Ok(Ddl { sql, notes })
    }
}
// A type/default is a single SQL fragment, not a second statement or column clause.
fn fragment(text: &str, data_type: bool, kind: DbKind) -> Result<()> {
    if text.trim().is_empty() || text.contains('\0') {
        return Err("列型／既定値を入力してください".into());
    }
    let mut quote = None;
    let mut depth = 0_i32;
    let chars = text.chars().collect::<Vec<_>>();
    let mut i = 0;
    let mut token = String::new();
    let forbidden = |word: &str| {
        matches!(
            word,
            "CREATE"
                | "ALTER"
                | "DROP"
                | "INSERT"
                | "DELETE"
                | "UPDATE"
                | "SELECT"
                | "REFERENCES"
                | "CONSTRAINT"
                | "DEFAULT"
                | "PRIMARY"
                | "UNIQUE"
                | "CHECK"
        ) || data_type && matches!(word, "NOT" | "NULL")
    };
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            if c == q {
                if chars.get(i + 1) == Some(&q) {
                    i += 2;
                    continue;
                }
                quote = None;
            } else if c == '\\' && kind == DbKind::MySql {
                i += 1;
            }
            i += 1;
            continue;
        }
        if c.is_alphanumeric() || c == '_' {
            token.push(c.to_ascii_uppercase());
        } else {
            if forbidden(&token) {
                return Err("列型／既定値に列定義や別のSQLは指定できません".into());
            }
            token.clear();
        }
        match c {
            ',' if depth == 0 => return Err("列型／既定値の外側にカンマは指定できません".into()),
            '\'' | '"' | '`' => quote = Some(c),
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return Err("括弧が対応していません".into());
                }
            }
            ';' => return Err("列型／既定値に文区切りは指定できません".into()),
            '-' if chars.get(i + 1) == Some(&'-') => {
                return Err("列型／既定値にコメントは指定できません".into());
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                return Err("列型／既定値にコメントは指定できません".into());
            }
            _ => {}
        }
        i += 1;
    }
    if quote.is_some() || depth != 0 || forbidden(&token) {
        return Err("列型／既定値の引用符・括弧・キーワードが不正です".into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    pub(crate) fn model(kind: DbKind) -> Model {
        let id = |name: &str| TableId {
            schema: if matches!(kind, DbKind::Sqlite | DbKind::DuckDb) {
                "main".into()
            } else {
                String::new()
            },
            name: name.into(),
        };
        let entity = |name: &str| Entity {
            size: None,
            id: id(name),
            logical_name: String::new(),
            view: false,
            fields: vec![
                Field {
                    important: false,
                    name: "id".into(),
                    logical_name: String::new(),
                    data_type: "INTEGER".into(),
                    nullable: false,
                    default_sql: None,
                },
                Field {
                    important: false,
                    name: "label".into(),
                    logical_name: String::new(),
                    data_type: "VARCHAR(40)".into(),
                    nullable: true,
                    default_sql: Some("'a;b'".into()),
                },
            ],
            keys: vec![Key {
                name: format!("PK_{name}"),
                primary: true,
                columns: vec!["id".into()],
            }],
            position: yy_draw::Point::default(),
            visible: true,
            indexes: vec![],
            catalog_definition: false,
        };
        Model {
            version: 1,
            kind,
            entities: vec![entity("child"), entity("parent")],
            relations: vec![Relation {
                logical: false,
                name: "FK_child_parent".into(),
                child: id("child"),
                parent: id("parent"),
                columns: vec!["id".into()],
                referenced_columns: vec!["id".into()],
            }],
            names: Names::Physical,
            level: Level::Types,
        }
    }
    #[test]
    fn design_renames_follow_dependencies_and_failed_edits_are_atomic() {
        let mut m = model(DbKind::Sqlite);
        m.entities[1].indexes.push(Index {
            name: "IX_label".into(),
            columns: vec!["label".into()],
            unique: false,
        });
        let before = serde_json::to_string(&m).unwrap();
        assert!(m.delete_field(1, 0).is_err());
        assert_eq!(serde_json::to_string(&m).unwrap(), before);
        let mut parent = m.entities[1].clone();
        parent.keys.clear();
        assert!(m.replace_entity(1, parent).is_err());
        let mut field = m.entities[1].fields[0].clone();
        field.name = "key id".into();
        m.replace_field(1, Some(0), field).unwrap();
        assert_eq!(m.relations[0].referenced_columns, vec!["key id"]);
        let mut parent = m.entities[1].clone();
        parent.id.name = "renamed".into();
        m.replace_entity(1, parent).unwrap();
        assert_eq!(m.relations[0].parent.name, "renamed");
        let mut duplicate = m.entities[1].fields[1].clone();
        duplicate.name = "key id".into();
        assert!(m.replace_field(1, Some(1), duplicate).is_err());
        let mut relation = m.relations[0].clone();
        relation.referenced_columns = vec!["label".into()];
        assert!(m.replace_relation(Some(0), relation).is_err());
    }
    #[test]
    fn batch_columns_swap_move_rename_dependencies_and_reject_deletion_atomically() {
        for kind in DbKind::ALL {
            let mut m = model(kind);
            m.entities[1].indexes.push(Index {
                name: "IX_label".into(),
                columns: vec!["label".into()],
                unique: false,
            });
            let mut rows = m.entities[1]
                .fields
                .iter()
                .cloned()
                .enumerate()
                .map(|(i, f)| (Some(i), f))
                .collect::<Vec<_>>();
            rows[0].1.name = "label".into();
            rows[1].1.name = "id".into();
            rows.swap(0, 1);
            let mut added = rows[0].1.clone();
            added.name = "new_field".into();
            rows.push((None, added));
            m.replace_fields(1, &rows).unwrap();
            assert_eq!(m.entities[1].fields[0].name, "id");
            assert_eq!(m.entities[1].keys[0].columns, vec!["label"]);
            assert_eq!(m.entities[1].indexes[0].columns, vec!["id"]);
            assert_eq!(m.relations[0].referenced_columns, vec!["label"]);
            // Removing a referenced column cannot accidentally bind to a newly added namesake.
            let before = serde_json::to_string(&m).unwrap();
            let mut changed = m.entities[1]
                .fields
                .iter()
                .cloned()
                .enumerate()
                .map(|(i, f)| (Some(i), f))
                .collect::<Vec<_>>();
            changed[1].0 = None;
            assert!(m.replace_fields(1, &changed).is_err());
            assert_eq!(before, serde_json::to_string(&m).unwrap());
            changed[1].0 = Some(1);
            changed[1].1.nullable = true;
            assert!(m.replace_fields(1, &changed).is_err());
            assert_eq!(before, serde_json::to_string(&m).unwrap());
            changed[1].1.nullable = false;
            changed[2].1.name = changed[0].1.name.clone();
            assert!(m.replace_fields(1, &changed).is_err());
            assert_eq!(before, serde_json::to_string(&m).unwrap());
            changed.pop();
            m.replace_fields(1, &changed).unwrap();
            assert_eq!(m.entities[1].fields.len(), 2);
        }
        // Both ends, including self references and logical relations, follow original identity.
        let mut m = model(DbKind::Sqlite);
        let mut r = m.relations[0].clone();
        r.child = r.parent.clone();
        r.logical = true;
        r.name = "self".into();
        m.relations.push(r);
        let mut rows = m.entities[1]
            .fields
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, f)| (Some(i), f))
            .collect::<Vec<_>>();
        rows[0].1.name = "new_id".into();
        m.replace_fields(1, &rows).unwrap();
        assert_eq!(m.relations[1].columns, vec!["new_id"]);
        assert_eq!(m.relations[1].referenced_columns, vec!["new_id"]);
    }
    #[test]
    fn ddl_roundtrips_embedded_engines_and_includes_hidden_dependencies() {
        for kind in [DbKind::Sqlite, DbKind::DuckDb] {
            let mut m = model(kind);
            m.entities[1].visible = false;
            m.entities[0].indexes.push(Index {
                name: "IX_label".into(),
                columns: vec!["label".into()],
                unique: false,
            });
            let ddl = m.ddl().unwrap();
            let mut db = Database::open(&crate::Options::memory(kind)).unwrap();
            db.execute(&ddl.sql, &AtomicBool::new(false)).unwrap();
            db.execute(
                "INSERT INTO parent(id) VALUES(1); INSERT INTO child(id) VALUES(1)",
                &AtomicBool::new(false),
            )
            .unwrap();
            let result = db
                .execute("SELECT label FROM child", &AtomicBool::new(false))
                .unwrap();
            assert_eq!(result[0].page(0, 1).unwrap()[0][0].display(), "a;b");
            assert!(
                db.execute("INSERT INTO child(id) VALUES(2)", &AtomicBool::new(false))
                    .is_err()
            );
            assert!(db.catalog().unwrap().iter().any(|o| o.kind == "INDEX"));
        }
    }
    #[test]
    fn ddl_dialects_cycles_and_fragments() {
        for kind in DbKind::ALL {
            let mut m = model(kind);
            let ddl = m.ddl().unwrap();
            assert!(ddl.sql.contains("FOREIGN KEY"));
            if kind == DbKind::MySql {
                assert!(ddl.sql.contains("`child`"));
            }
            if kind == DbKind::DuckDb {
                assert!(
                    ddl.sql.find("CREATE TABLE \"main\".\"parent\"")
                        < ddl.sql.find("CREATE TABLE \"main\".\"child\"")
                );
            }
            m.entities[1].fields[1].default_sql = Some("0); DROP TABLE parent; --".into());
            assert!(m.ddl().is_err());
        }
        let mut m = model(DbKind::DuckDb);
        m.relations.push(Relation {
            logical: false,
            name: "cycle".into(),
            child: m.entities[1].id.clone(),
            parent: m.entities[0].id.clone(),
            columns: vec!["id".into()],
            referenced_columns: vec!["id".into()],
        });
        assert!(m.ddl().is_err());
        for bad in [
            "INT, injected INT",
            "INT) REFERENCES x(id)",
            "VARCHAR(20) -- comment",
            "INT DEFAULT 0",
        ] {
            assert!(fragment(bad, true, DbKind::Sqlite).is_err());
        }
        assert!(fragment("DECIMAL(31,8)", true, DbKind::Db2).is_ok());
        assert!(fragment("'x;y,z'", false, DbKind::Sqlite).is_ok());
    }
    #[test]
    fn saved_design_keeps_defaults_indexes_and_reads_version_one() {
        let mut m = model(DbKind::Sqlite);
        m.entities[0].indexes.push(Index {
            name: "IX_label".into(),
            columns: vec!["label".into()],
            unique: false,
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("design.yyer");
        m.save(&path).unwrap();
        let restored = Model::load(&path).unwrap();
        assert_eq!(restored.version, 4);
        assert_eq!(
            restored.entities[0].fields[1].default_sql,
            Some("'a;b'".into())
        );
        assert_eq!(restored.entities[0].indexes[0].name, "IX_label");
        assert_eq!(restored.ddl().unwrap().sql, m.ddl().unwrap().sql);
        std::fs::write(
            &path,
            include_str!("../../../tests/fixtures/sqlite-er.yyer"),
        )
        .unwrap();
        let legacy = Model::load(&path).unwrap();
        assert_eq!(legacy.version, 4);
        assert!(legacy.entities[0].fields[0].default_sql.is_none());
    }
}

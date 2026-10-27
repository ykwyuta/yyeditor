//! ER semantic model, separate from its yydraw presentation.
use crate::{Database, DbKind, Result, Value};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    sync::atomic::AtomicBool,
};
#[path = "schema_design.rs"]
mod design;
#[path = "schema_document.rs"]
mod document;
#[path = "schema_file.rs"]
mod file;
pub use file::{ErDocument, ErViewState};
#[path = "schema_metadata.rs"]
mod metadata;
#[path = "schema_visual.rs"]
mod visual;
pub use design::Ddl;
pub use document::RelationSuggestion;
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableId {
    pub schema: String,
    pub name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    #[serde(default)]
    pub important: bool,
    pub name: String,
    pub logical_name: String,
    pub data_type: String,
    pub nullable: bool,
    #[serde(default)]
    pub default_sql: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
    pub name: String,
    pub primary: bool,
    pub columns: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entity {
    /// None follows content; Some preserves an explicitly chosen card size.
    #[serde(default)]
    pub size: Option<TableSize>,
    pub id: TableId,
    pub logical_name: String,
    pub view: bool,
    pub fields: Vec<Field>,
    pub keys: Vec<Key>,
    #[serde(with = "file::point")]
    pub position: yy_draw::Point,
    pub visible: bool,
    #[serde(default)]
    pub indexes: Vec<Index>,
    #[serde(default = "catalog_definition_default")]
    pub catalog_definition: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableSize {
    pub width: f64,
    pub height: f64,
}
fn catalog_definition_default() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    /// A semantic association, never emitted as a database constraint.
    #[serde(default)]
    pub logical: bool,
    pub name: String,
    pub child: TableId,
    pub parent: TableId,
    pub columns: Vec<String>,
    pub referenced_columns: Vec<String>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Names {
    #[default]
    Physical,
    Logical,
    Both,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Level {
    Entity,
    Keys,
    Attributes,
    #[default]
    Types,
    Important,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub version: u32,
    pub kind: DbKind,
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
    pub names: Names,
    pub level: Level,
}
impl TableId {
    pub fn display(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }
}
impl Model {
    pub fn validate(&self) -> Result<()> {
        if !(1..=4).contains(&self.version)
            || self.entities.len() > 500
            || self.relations.len() > 10_000
        {
            return Err("未対応のER形式、またはERの上限を超えています".into());
        }
        let mut ids = HashSet::new();
        for e in &self.entities {
            design::validate_entity(e)?;
            if e.size.is_some_and(|s| {
                !s.width.is_finite()
                    || !s.height.is_finite()
                    || !(160. ..=10000.).contains(&s.width)
                    || !(48. ..=120000.).contains(&s.height)
            }) {
                return Err("テーブルの幅は160～10000、高さは48～120000で指定してください".into());
            }
            if !ids.insert(&e.id)
                || !e.position.x.is_finite()
                || !e.position.y.is_finite()
                || e.position.x.abs() > 1e7
                || e.position.y.abs() > 1e7
                || e.fields.len() > 4096
            {
                return Err("ERの表・座標・列が不正です".into());
            }
            let columns = e.fields.iter().map(|f| &f.name).collect::<HashSet<_>>();
            if columns.len() != e.fields.len()
                || e.keys
                    .iter()
                    .any(|k| k.columns.is_empty() || k.columns.iter().any(|c| !columns.contains(c)))
            {
                return Err("ERの列・キーが不正です".into());
            }
        }
        for r in &self.relations {
            if r.logical && !self.entities.iter().any(|e| e.id == r.parent) {
                return Err("論理リレーションの参照先テーブルがありません".into());
            }
            if r.columns.iter().collect::<HashSet<_>>().len() != r.columns.len()
                || r.referenced_columns.iter().collect::<HashSet<_>>().len()
                    != r.referenced_columns.len()
            {
                return Err("リレーションの列が重複しています".into());
            }
            let child = self
                .entities
                .iter()
                .find(|e| e.id == r.child)
                .ok_or("ERの参照元がありません")?;
            if r.columns.is_empty()
                || r.columns.len() != r.referenced_columns.len()
                || r.columns
                    .iter()
                    .any(|c| !child.fields.iter().any(|f| &f.name == c))
            {
                return Err("ERのリレーション列が不正です".into());
            }
            // Referenced tables may be outside this user's catalog permissions.
            if let Some(parent) = self.entities.iter().find(|e| e.id == r.parent)
                && r.referenced_columns
                    .iter()
                    .any(|c| !parent.fields.iter().any(|f| &f.name == c))
            {
                return Err("ERの参照先列が不正です".into());
            }
        }
        Ok(())
    }
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        self.validate()?;
        ErDocument::new(self.clone(), None).save(path)
    }
    pub fn load(path: &std::path::Path) -> Result<Self> {
        Ok(ErDocument::load(path)?.model)
    }
    pub fn drawing(&self) -> yy_draw::Document {
        visual::drawing(self)
    }
    pub fn table_size(&self, index: usize) -> Option<TableSize> {
        self.entities.get(index).map(|e| visual::size(self, e))
    }
    /// Move geometry without rebuilding table SVGs or invalidating their raster cache.
    pub fn move_entity(
        &mut self,
        index: usize,
        position: yy_draw::Point,
        doc: &mut yy_draw::Document,
    ) {
        visual::move_entity(self, index, position, doc);
    }
    pub fn arrange(&mut self) {
        let mut y = 32.;
        let visible = self
            .entities
            .iter()
            .enumerate()
            .filter(|(_, e)| e.visible)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        for row in visible.chunks(3) {
            let mut height = 0_f64;
            let mut x = 40.;
            for index in row {
                self.entities[*index].position = yy_draw::Point::new(x, y);
                x += visual::size(self, &self.entities[*index]).width + 100.;
                height = height.max(visual::height(self, &self.entities[*index]));
            }
            y += height + 100.;
        }
    }
    pub fn entity_details(&self, index: usize) -> Vec<(String, String)> {
        let Some(e) = self.entities.get(index) else {
            return vec![];
        };
        let mut d = vec![
            ("テーブル".into(), e.id.display()),
            ("論理名".into(), e.logical_name.clone()),
            ("種類".into(), if e.view { "VIEW" } else { "TABLE" }.into()),
        ];
        for f in &e.fields {
            d.push((
                f.name.clone(),
                format!(
                    "{} {}{}{}{}",
                    f.logical_name,
                    f.data_type,
                    if f.nullable { "" } else { " NOT NULL" },
                    f.default_sql
                        .as_ref()
                        .map(|v| format!(" DEFAULT {v}"))
                        .unwrap_or_default(),
                    if f.important { " ★主要項目" } else { "" }
                ),
            ));
        }
        for k in &e.keys {
            d.push((
                format!(
                    "{} {}",
                    if k.primary { "PRIMARY KEY" } else { "UNIQUE" },
                    k.name
                ),
                k.columns.join(", "),
            ));
        }
        for r in self.relations.iter().filter(|r| r.child == e.id) {
            d.push((
                format!(
                    "{} {}",
                    if r.logical {
                        "論理リレーション"
                    } else {
                        "FOREIGN KEY"
                    },
                    r.name
                ),
                format!(
                    "({}) → {} ({})",
                    r.columns.join(", "),
                    r.parent.display(),
                    r.referenced_columns.join(", ")
                ),
            ));
        }
        for i in &e.indexes {
            d.push((
                format!("{}INDEX {}", if i.unique { "UNIQUE " } else { "" }, i.name),
                i.columns.join(", "),
            ));
        }
        d
    }
}
impl Database {
    pub fn er_model(&mut self) -> Result<Model> {
        let kind = self.kind();
        let objects = self
            .catalog()?
            .into_iter()
            .filter(|o| matches!(o.kind.as_str(), "TABLE" | "VIEW"))
            .collect::<Vec<_>>();
        if objects.len() > 500 {
            return Err("ER図は一度に500テーブルまでです".into());
        }
        let mut m = Model {
            version: 1,
            kind,
            entities: vec![],
            relations: vec![],
            names: Names::Physical,
            level: Level::Types,
        };
        for o in objects {
            let fields = self
                .object_columns(&o.schema, &o.name)?
                .into_iter()
                .map(|c| Field {
                    important: false,
                    name: c.name,
                    logical_name: String::new(),
                    data_type: c.data_type,
                    nullable: !matches!(c.nullable.as_str(), "NO" | "N"),
                    default_sql: None,
                })
                .collect();
            m.entities.push(Entity {
                size: None,
                id: TableId {
                    schema: o.schema,
                    name: o.name,
                },
                logical_name: String::new(),
                view: o.kind == "VIEW",
                fields,
                keys: vec![],
                position: yy_draw::Point::default(),
                visible: true,
                indexes: vec![],
                catalog_definition: true,
            });
        }
        let rows = if kind == DbKind::Sqlite {
            self.sqlite_er_keys(&m)?
        } else {
            self.er_key_rows(metadata::key_sql(kind))?
        };
        type ConstraintId = (TableId, String, String, TableId);
        let mut groups = BTreeMap::<ConstraintId, Vec<(usize, String, String)>>::new();
        for r in rows {
            if r.len() != 9 {
                return Err("ERのキー情報の列数が不正です".into());
            }
            let child = TableId {
                schema: r[0].clone(),
                name: r[1].clone(),
            };
            if !m.entities.iter().any(|e| e.id == child) {
                continue;
            }
            let parent = TableId {
                schema: r[6].clone(),
                name: r[7].clone(),
            };
            groups
                .entry((child, r[2].clone(), r[3].clone(), parent))
                .or_default()
                .push((
                    r[4].parse().map_err(|_| "ERのキー順序が不正です")?,
                    r[5].clone(),
                    r[8].clone(),
                ));
        }
        for ((child, name, kind, parent), mut cols) in groups {
            cols.sort_by_key(|c| c.0);
            if kind == "F" {
                m.relations.push(Relation {
                    logical: false,
                    name,
                    child,
                    parent,
                    columns: cols.iter().map(|c| c.1.clone()).collect(),
                    referenced_columns: cols.into_iter().map(|c| c.2).collect(),
                });
            } else if let Some(e) = m.entities.iter_mut().find(|e| e.id == child) {
                let columns = cols.into_iter().map(|c| c.1).collect::<Vec<_>>();
                if kind == "P" && self.kind() != DbKind::Sqlite {
                    for f in &mut e.fields {
                        if columns.contains(&f.name) {
                            f.nullable = false;
                        }
                    }
                }
                e.keys.push(Key {
                    name,
                    primary: kind == "P",
                    columns,
                });
            }
        }
        m.validate()?;
        m.arrange();
        Ok(m)
    }
    pub fn er_diagram(&mut self) -> Result<yy_draw::Document> {
        Ok(self.er_model()?.drawing())
    }
    fn er_key_rows(&mut self, sql: &str) -> Result<Vec<Vec<String>>> {
        let sets = self.execute(sql, &AtomicBool::new(false))?;
        let mut rows = vec![];
        if let Some(set) = sets.first() {
            if set.row_count > 100_000 {
                return Err("ERのキー列情報が10万件を超えました".into());
            }
            let mut start = 0;
            while start < set.row_count {
                for row in set.page(start, 512)? {
                    rows.push(
                        row.into_iter()
                            .map(|v| {
                                if v == Value::Null {
                                    String::new()
                                } else {
                                    v.display().trim_end().to_owned()
                                }
                            })
                            .collect(),
                    );
                }
                start += 512;
            }
        }
        Ok(rows)
    }
    fn sqlite_er_keys(&mut self, m: &Model) -> Result<Vec<Vec<String>>> {
        let mut rows = vec![];
        for e in &m.entities {
            let name = format!("'{}'", e.id.name.replace('\'', "''"));
            rows.extend(self.er_key_rows(&format!("SELECT 'main',{name},'PRIMARY','P',pk,name,'','','' FROM pragma_table_info({name}) WHERE pk>0"))?);
            for index in self.er_key_rows(&format!("SELECT name FROM pragma_index_list({name}) WHERE \"unique\"=1 AND origin<>'pk' AND partial=0"))? {
                let idx=format!("'{}'",index[0].replace('\'',"''"));
                let cols=self.er_key_rows(&format!("SELECT seqno,name FROM pragma_index_info({idx}) ORDER BY seqno"))?;
                if cols.iter().all(|c|!c[1].is_empty()) { for c in cols {rows.push(vec!["main".into(),e.id.name.clone(),index[0].clone(),"U".into(),c[0].clone(),c[1].clone(),String::new(),String::new(),String::new()]);} }
            }
            for fk in self.er_key_rows(&format!("SELECT id,seq,\"table\",\"from\",\"to\" FROM pragma_foreign_key_list({name}) ORDER BY id,seq"))? {
                let parent=m.entities.iter().find(|p|p.id.name.eq_ignore_ascii_case(&fk[2])).map(|p|p.id.name.clone()).unwrap_or_else(||fk[2].clone());
                let to=if fk[4].is_empty() {
                    let target=format!("'{}'",parent.replace('\'',"''"));
                    self.er_key_rows(&format!("SELECT name FROM pragma_table_info({target}) WHERE pk={}+1",fk[1]))?.first().map(|r|r[0].clone()).ok_or("SQLiteの省略された参照列を解決できません")?
                }else{m.entities.iter().find(|p|p.id.name==parent).and_then(|p|p.fields.iter().find(|f|f.name.eq_ignore_ascii_case(&fk[4]))).map(|f|f.name.clone()).unwrap_or_else(||fk[4].clone())};
                let from=e.fields.iter().find(|f|f.name.eq_ignore_ascii_case(&fk[3])).map(|f|f.name.clone()).unwrap_or_else(||fk[3].clone());
                rows.push(vec!["main".into(),e.id.name.clone(),format!("FK_{}",fk[0]),"F".into(),fk[1].clone(),from,"main".into(),parent,to]);
            }
        }
        Ok(rows)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_er_composite_keys_and_roundtrip() {
        for kind in [DbKind::Sqlite, DbKind::DuckDb] {
            let mut db = Database::open(&crate::Options::memory(kind)).unwrap();
            db.execute("CREATE TABLE parent(a INTEGER,b INTEGER,PRIMARY KEY(a,b)); CREATE TABLE child(x INTEGER,y INTEGER,FOREIGN KEY(x,y) REFERENCES parent(a,b)); CREATE TABLE one_child(x INTEGER NOT NULL,y INTEGER NOT NULL,UNIQUE(x,y),FOREIGN KEY(x,y) REFERENCES parent(a,b))",&AtomicBool::new(false)).unwrap();
            let mut m = db.er_model().unwrap();
            assert_eq!(m.relations.len(), 2);
            assert_eq!(m.relations[0].columns, vec!["x", "y"]);
            assert_eq!(m.drawing().connectors.len(), 2);
            let mut drawing = m.drawing();
            for (i, position) in [
                (0, yy_draw::Point::new(75.5, 60.2)),
                (1, yy_draw::Point::new(1800., 220.)),
                (2, yy_draw::Point::new(-10., 15.)),
            ] {
                m.move_entity(i, position, &mut drawing);
                let fresh = m.drawing();
                assert_eq!(drawing.nodes, fresh.nodes);
                assert_eq!(drawing.connectors, fresh.connectors);
                assert_eq!(drawing.assets, fresh.assets);
            }
            assert!(m.drawing().assets.iter().any(|a| a.svg.contains("PK")));
            m.entities[0].logical_name = "論理名<&>".into();
            m.names = Names::Both;
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("test.yyer");
            m.save(&path).unwrap();
            let loaded = Model::load(&path).unwrap();
            assert_eq!(loaded.entities[0].logical_name, m.entities[0].logical_name);
            assert!(
                loaded
                    .drawing()
                    .assets
                    .iter()
                    .any(|a| a.svg.contains("&lt;&amp;&gt;"))
            );
            m.entities[0].position.x = f64::NAN;
            assert!(m.validate().is_err());
        }
    }
    #[test]
    fn sqlite_implicit_composite_key_and_case_are_resolved() {
        let mut db = Database::open(&crate::Options::memory(DbKind::Sqlite)).unwrap();
        db.execute("CREATE TABLE Parent(a INTEGER,b INTEGER,PRIMARY KEY(b,a)); CREATE TABLE Child(x INTEGER,y INTEGER,FOREIGN KEY(x,y) REFERENCES parent)",&AtomicBool::new(false)).unwrap();
        let m = db.er_model().unwrap();
        assert_eq!(m.relations.len(), 1);
        assert_eq!(m.relations[0].referenced_columns, vec!["b", "a"]);
        assert_eq!(m.relations[0].parent.name, "Parent");
        db.execute(
            "CREATE TABLE explicit(x INTEGER,y INTEGER,FOREIGN KEY(X,Y) REFERENCES PARENT(B,A))",
            &AtomicBool::new(false),
        )
        .unwrap();
        let m = db.er_model().unwrap();
        assert!(m.relations.iter().any(|r| r.child.name == "explicit"
            && r.columns == vec!["x", "y"]
            && r.referenced_columns == vec!["b", "a"]));
    }
    #[test]
    fn self_reference_and_tall_entities_have_valid_geometry() {
        let mut db = Database::open(&crate::Options::memory(DbKind::Sqlite)).unwrap();
        let columns = (0..30).map(|i| format!(",c{i} TEXT")).collect::<String>();
        db.execute(&format!("CREATE TABLE tall(id INTEGER PRIMARY KEY,parent INTEGER REFERENCES tall(id){columns}); CREATE TABLE b(id INTEGER); CREATE TABLE c(id INTEGER); CREATE TABLE d(id INTEGER)"),&AtomicBool::new(false)).unwrap();
        let mut m = db.er_model().unwrap();
        m.entities.sort_by_key(|e| e.id.name != "tall");
        m.arrange();
        let doc = m.drawing();
        doc.validate().unwrap();
        let start = doc.endpoint(doc.connectors[0].start);
        let end = doc.endpoint(doc.connectors[0].end);
        assert_eq!(start.x, end.x);
        assert_ne!(start.y, end.y);
        assert!(doc.connectors[0].waypoints[0].x > start.x);
        assert!(
            m.entities[3].position.y
                > m.entities[0].position.y + visual::height(&m, &m.entities[0])
        );
        m.entities[1].visible = false;
        let hidden = m.drawing();
        assert!(!hidden.nodes.iter().any(|n| n.id == 1_000_001));
    }
    #[test]
    #[ignore = "explicit release-mode drawing performance measurement"]
    fn er_viewport_performance() {
        use std::time::Instant;
        let mut m = Model {
            version: 1,
            kind: DbKind::Sqlite,
            entities: vec![],
            relations: vec![],
            names: Names::Physical,
            level: Level::Types,
        };
        for i in 0..500 {
            m.entities.push(Entity {
                size: None,
                id: TableId {
                    schema: "main".into(),
                    name: format!("table_{i}"),
                },
                logical_name: String::new(),
                view: false,
                fields: (0..8)
                    .map(|n| Field {
                        important: false,
                        name: format!("c{n}"),
                        logical_name: String::new(),
                        data_type: "INTEGER".into(),
                        nullable: n != 0,
                        default_sql: None,
                    })
                    .collect(),
                keys: vec![Key {
                    name: "PK".into(),
                    primary: true,
                    columns: vec!["c0".into()],
                }],
                position: yy_draw::Point::default(),
                visible: true,
                indexes: vec![],
                catalog_definition: false,
            });
        }
        for i in 0..500 {
            for j in 1..=4 {
                m.relations.push(Relation {
                    logical: false,
                    name: format!("fk_{i}_{j}"),
                    child: m.entities[i].id.clone(),
                    parent: m.entities[(i + j) % 500].id.clone(),
                    columns: vec![format!("c{j}")],
                    referenced_columns: vec!["c0".into()],
                });
            }
        }
        m.arrange();
        let start = Instant::now();
        let mut doc = m.drawing();
        let layout = start.elapsed().as_secs_f64() * 1000.;
        let renderer = crate::plan::drawing_renderer();
        let options = yy_draw::render::Export {
            bounds: yy_draw::Rect {
                x: 0.,
                y: 0.,
                w: 1400.,
                h: 800.,
            },
            background: Some(0xffffff),
            ..crate::plan::drawing_export(&doc)
        };
        let start = Instant::now();
        renderer.raster_flat_icons(&doc, &options, 1.).unwrap();
        let first = start.elapsed().as_secs_f64() * 1000.;
        let mut frames = vec![];
        let mut moves = vec![];
        for frame in 0..20 {
            let start = Instant::now();
            m.move_entity(
                0,
                yy_draw::Point::new(40. + frame as f64, 32. + frame as f64),
                &mut doc,
            );
            moves.push(start.elapsed().as_secs_f64() * 1000.);
            let start = Instant::now();
            renderer.raster_flat_icons(&doc, &options, 1.).unwrap();
            frames.push(start.elapsed().as_secs_f64() * 1000.);
        }
        moves.sort_by(f64::total_cmp);
        frames.sort_by(f64::total_cmp);
        let report = serde_json::json!({"tables":500,"relations":2000,"viewport":"1400x800 @ 100%","layout_ms":layout,"first_raster_ms":first,"move_p95_ms":moves[18],"cached_raster_p95_ms":frames[18],"marker_assets":doc.assets.len()-500});
        println!("{report}");
        if let Ok(path) = std::env::var("YYDB_PERF_OUTPUT") {
            std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        }
    }
}

use super::*;
const FORMAT: &str = "yydbmanager-er";
const MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErViewState {
    /// Scale at 96 DPI. Device DPI is applied by the UI on restoration.
    pub zoom: f64,
    #[serde(with = "point")]
    pub origin: yy_draw::Point,
    pub selected: Option<TableId>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErDocument {
    pub format: String,
    pub format_version: u32,
    pub model: Model,
    pub view: Option<ErViewState>,
}
impl ErDocument {
    pub fn new(mut model: Model, view: Option<ErViewState>) -> Self {
        model.version = 4;
        Self {
            format: FORMAT.into(),
            format_version: 1,
            model,
            view,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.format != FORMAT || self.format_version != 1 || self.model.version != 4 {
            return Err(
                    "未対応のER YAML形式です（format: yydbmanager-er / format_version: 1 / model.version: 4 が必要です）"
                    .into(),
            );
        }
        self.model.validate()?;
        if let Some(v) = &self.view
            && (!v.zoom.is_finite()
                || v.zoom <= 0.
                || v.zoom > 4.
                || !v.origin.x.is_finite()
                || !v.origin.y.is_finite()
                || v.origin.x.abs() > 1e7
                || v.origin.y.abs() > 1e7
                || v.selected
                    .as_ref()
                    .is_some_and(|id| !self.model.entities.iter().any(|e| &e.id == id)))
        {
            return Err("ER YAMLの拡大率・表示位置・選択中の表が不正です".into());
        }
        Ok(())
    }
    pub fn from_yaml(text: &str) -> Result<Self> {
        if text.len() > MAX_BYTES {
            return Err("ERファイルは32MiBまでです".into());
        }
        let options = serde_saphyr::options! {
            duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
            merge_keys: serde_saphyr::MergeKeyPolicy::Error,
            strict_booleans: true,
            reject_unsupported_tags: true,
            budget: serde_saphyr::budget! { max_documents: 1, max_aliases: 0, max_anchors: 0, max_depth: 32 },
        };
        let document: Self =
            serde_saphyr::from_str_with_options(text.trim_start_matches('\u{feff}'), options)
                .map_err(|e| format!("ER YAMLを読み込めません: {e}"))?;
        document.validate()?;
        Ok(document)
    }
    pub fn to_yaml(&self) -> Result<String> {
        self.validate()?;
        let text = format!(
            "# yydbmanager ER model — UTF-8 YAML\n{}",
            serde_saphyr::to_string(self).map_err(|e| e.to_string())?
        );
        // Do not write a document our own bounded reader cannot open.
        Self::from_yaml(&text)?;
        Ok(text)
    }
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        let text = self.to_yaml()?;
        let mut temp = tempfile::NamedTempFile::new_in(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new(".")),
        )
        .map_err(|e| e.to_string())?;
        use std::io::Write;
        temp.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        temp.as_file().sync_all().map_err(|e| e.to_string())?;
        temp.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn load(path: &std::path::Path) -> Result<Self> {
        use std::io::Read;
        let mut text = String::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take(MAX_BYTES as u64 + 1)
            .read_to_string(&mut text)
            .map_err(|e| e.to_string())?;
        if text.len() > MAX_BYTES {
            return Err("ERファイルは32MiBまでです".into());
        }
        let text = text.trim_start_matches('\u{feff}');
        if text.trim_start().starts_with('{') {
            // Legacy .yyer JSON, format versions 1–3. New saves always use YAML.
            let model: Model = serde_json::from_str(text).map_err(|e| e.to_string())?;
            model.validate()?;
            Ok(Self::new(model, None))
        } else {
            Self::from_yaml(text)
        }
    }
}
pub(super) mod point {
    use super::*;
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Point {
        x: f64,
        y: f64,
    }
    pub fn serialize<S: serde::Serializer>(
        p: &yy_draw::Point,
        s: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        Point { x: p.x, y: p.y }.serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<yy_draw::Point, D::Error> {
        let p = Point::deserialize(d)?;
        Ok(yy_draw::Point::new(p.x, p.y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> ErDocument {
        let mut model: Model =
            serde_json::from_str(include_str!("../../../tests/fixtures/sqlite-er.yyer")).unwrap();
        model.names = Names::Both;
        model.level = Level::Important;
        for (i, e) in model.entities.iter_mut().enumerate() {
            e.logical_name = format!("000{i}: 日本語\n改行\r\n引用\" # : [yes] 'null'");
            e.position = yy_draw::Point::new(0.10000000000000002 + i as f64, -123.12345678912345);
            e.visible = i != 0;
            e.catalog_definition = i == 0;
            e.size = (i == 0).then_some(TableSize {
                width: 512.3333333333334,
                height: 300.25,
            });
            for (fi, f) in e.fields.iter_mut().enumerate() {
                f.logical_name = format!("列{fi}: true\nnull");
                f.important = fi == 0;
                f.default_sql = (fi == 0).then(|| "'0001' || '\n日本語'".into());
            }
            e.indexes.push(Index {
                name: format!("IX_{i}"),
                unique: true,
                columns: e.fields.iter().rev().map(|f| f.name.clone()).collect(),
            });
        }
        let mut logical = model.relations[0].clone();
        logical.logical = true;
        logical.name = "LR_semantic".into();
        model.relations.push(logical);
        let selected = Some(model.entities[0].id.clone());
        ErDocument::new(
            model,
            Some(ErViewState {
                zoom: 0.7543210987654321,
                origin: yy_draw::Point::new(-0., 123.00000000000003),
                selected,
            }),
        )
    }
    #[test]
    fn yaml_roundtrip_preserves_every_field_view_and_exact_floating_point_coordinates() {
        let mut doc = sample();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("all.yyer.yaml");
        for kind in DbKind::ALL {
            doc.model.kind = kind;
            doc.save(&path).unwrap();
            let read = ErDocument::load(&path).unwrap();
            assert_eq!(
                serde_json::to_value(&read).unwrap(),
                serde_json::to_value(&doc).unwrap()
            );
            assert_eq!(
                read.model.entities[0].position.x.to_bits(),
                doc.model.entities[0].position.x.to_bits()
            );
            assert_eq!(
                read.view.as_ref().unwrap().origin.x.to_bits(),
                doc.view.as_ref().unwrap().origin.x.to_bits()
            );
            assert_eq!(
                read.view.unwrap().origin.y.to_bits(),
                doc.view.as_ref().unwrap().origin.y.to_bits()
            );
        }
        let yaml = std::fs::read_to_string(&path).unwrap();
        assert!(
            yaml.contains("format: yydbmanager-er")
                && yaml.contains("size:")
                && yaml.contains("logical: true")
        );
        if let Some(output) = std::env::var_os("YYDBMANAGER_YAML_FIXTURE") {
            doc.save(std::path::Path::new(&output)).unwrap();
        }
    }
    #[test]
    fn yaml_rejects_unrecognized_information_duplicates_invalid_views_and_preserves_existing_file()
    {
        let doc = sample();
        let yaml = doc.to_yaml().unwrap();
        for invalid in [
            yaml.replace("format_version: 1", "format_version: 99"),
            yaml.replace("format: yydbmanager-er", "format: other-tool"),
            format!("{yaml}\nunknown_data: please_preserve\n"),
            yaml.replacen("model:", "format_version: 1\nmodel:", 1),
            yaml.replacen("  version:", "  typo: value\n  version:", 1),
            yaml.replacen("position:", "unknown_column: omitted\n    position:", 1),
            format!("{yaml}\n---\nformat: yydbmanager-er\n"),
            yaml.replacen(
                "format: yydbmanager-er",
                "format: &anchor yydbmanager-er",
                1,
            ),
        ] {
            assert!(
                ErDocument::from_yaml(&invalid).is_err(),
                "accepted invalid YAML: {invalid}"
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keep.yaml");
        doc.save(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        for invalid in [f64::NAN, -1., 5.] {
            let mut bad = doc.clone();
            bad.view.as_mut().unwrap().zoom = invalid;
            assert!(bad.save(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
        let mut bad = doc.clone();
        bad.model.entities[0].size.as_mut().unwrap().width = f64::INFINITY;
        assert!(bad.save(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
    #[test]
    fn legacy_json_versions_migrate_to_yaml_and_embedded_help_example_loads() {
        let base = sample();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.yyer");
        for version in [1, 2, 3] {
            let mut model = base.model.clone();
            model.version = version;
            let mut json = serde_json::to_value(model).unwrap();
            for entity in json["entities"].as_array_mut().unwrap() {
                entity.as_object_mut().unwrap().remove("size");
            }
            std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
            let doc = ErDocument::load(&path).unwrap();
            assert_eq!(doc.model.version, 4);
            assert!(doc.view.is_none());
            assert!(doc.model.entities.iter().all(|e| e.size.is_none()));
            doc.save(&path).unwrap();
            assert!(
                std::fs::read_to_string(&path)
                    .unwrap()
                    .starts_with("# yydbmanager ER model")
            );
        }
        let help = include_str!("../../../help/help.md").replace("\r\n", "\n");
        let example = help
            .split("```yaml\n")
            .nth(1)
            .unwrap()
            .split("```")
            .next()
            .unwrap();
        let doc = ErDocument::from_yaml(example).unwrap();
        assert_eq!(doc.model.entities.len(), 2);
        assert_eq!(doc.model.relations.len(), 2);
        assert!(doc.model.relations[1].logical);
        assert!(doc.view.is_some());
    }
}

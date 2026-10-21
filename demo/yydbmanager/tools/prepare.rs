//! Rebuild demo binary files with the same engines/importers as yydbmanager.
use std::{io::Write, path::Path, sync::atomic::AtomicBool};
use yy_db_core::{
    Database, DbKind, Options,
    schema::{ErDocument, ErViewState, Level, Names, Relation},
    transfer,
};
fn install(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    file.write_all(bytes)?;
    file.as_file_mut().sync_all()?;
    file.persist_noclobber(path)?;
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    let root = Path::new(
        args.get(1)
            .ok_or("usage: prepare_demo <demo-root> <new-output-directory>")?,
    );
    let output = Path::new(args.get(2).ok_or("output directory is required")?);
    std::fs::create_dir_all(output)?;
    for name in [
        "demo.sqlite",
        "demo.duckdb",
        "import_sample.parquet",
        "exact_values.parquet",
        "postgres-demo.yyer.yaml",
        "sqlite-demo.yyer.yaml",
    ] {
        if output.join(name).exists() {
            return Err(format!(
                "{} already exists; use a new output directory",
                output.join(name).display()
            )
            .into());
        }
    }
    let stage = tempfile::tempdir_in(output)?;
    let cancel = AtomicBool::new(false);
    for (kind, sql_file, filename) in [
        (DbKind::Sqlite, "sqlite.sql", "demo.sqlite"),
        (DbKind::DuckDb, "duckdb.sql", "demo.duckdb"),
    ] {
        let path = stage.path().join(filename);
        let mut options = Options::memory(kind);
        options.path = Some(path.clone());
        let mut db = Database::open(&options)?;
        db.execute(
            &std::fs::read_to_string(root.join("init").join(sql_file))?,
            &cancel,
        )?;
        let expected: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("data/expected-counts.json"))?)?;
        for (table, count) in expected.as_object().unwrap() {
            let rows = db.execute(&format!("SELECT COUNT(*) FROM {table}"), &cancel)?;
            assert_eq!(
                rows[0].page(0, 1)?[0][0].display(),
                count.to_string(),
                "{table}"
            );
        }
        let values = db.execute("SELECT * FROM exact_values ORDER BY value_id", &cancel)?;
        assert_eq!(values[0].page(0, 1)?[0][1].display(), "9223372036854775807");
        assert_eq!(
            values[0].page(0, 1)?[0][2].display(),
            "12345678901234567890123.12345678"
        );
        db.explain("SELECT c.name, SUM(o.total_amount) FROM customers c JOIN orders o ON o.customer_id=c.customer_id GROUP BY c.name",false,&cancel)?;
        let performance = if kind == DbKind::DuckDb {
            "duckdb.sql"
        } else {
            "sqlite.sql"
        };
        let results = db.execute(
            &std::fs::read_to_string(root.join("sql/performance").join(performance))?,
            &cancel,
        )?;
        assert!(
            !results.is_empty(),
            "performance examples returned no results"
        );
        if kind == DbKind::DuckDb {
            transfer::export_parquet(&values[0], &stage.path().join("exact_values.parquet"))?;
            let reloaded = transfer::read_parquet(&stage.path().join("exact_values.parquet"))?;
            assert_eq!(
                reloaded.page(0, 1)?[0][2].display(),
                "12345678901234567890123.12345678"
            );
        } else {
            let mut model = db.er_model()?;
            for e in &mut model.entities {
                e.visible = !e.view && e.id.name != "demo_ready";
                e.logical_name = match e.id.name.as_str() {
                    "departments" => "部門",
                    "employees" => "社員",
                    "customers" => "顧客",
                    "products" => "商品",
                    "orders" => "受注",
                    "order_items" => "受注明細",
                    "support_tickets" => "問い合わせ",
                    "exact_values" => "精度検証",
                    _ => "",
                }
                .into();
                for f in &mut e.fields {
                    f.important = matches!(
                        f.name.as_str(),
                        "name" | "status" | "total_amount" | "subject"
                    );
                }
            }
            let child = model
                .entities
                .iter()
                .find(|e| e.id.name == "support_tickets")
                .unwrap()
                .id
                .clone();
            let parent = model
                .entities
                .iter()
                .find(|e| e.id.name == "customers")
                .unwrap()
                .id
                .clone();
            model.replace_relation(
                None,
                Relation {
                    logical: true,
                    name: "lr_ticket_customer".into(),
                    child,
                    parent,
                    columns: vec!["customer_code".into()],
                    referenced_columns: vec!["customer_code".into()],
                },
            )?;
            model.names = Names::Both;
            model.level = Level::Important;
            model.arrange();
            for (kind, name) in [
                (DbKind::Sqlite, "sqlite-demo.yyer.yaml"),
                (DbKind::PostgreSql, "postgres-demo.yyer.yaml"),
            ] {
                let mut m = model.clone();
                m.kind = kind;
                if kind == DbKind::PostgreSql {
                    for e in &mut m.entities {
                        e.id.schema = "public".into();
                        if e.id.name == "exact_values" {
                            e.fields
                                .iter_mut()
                                .find(|f| f.name == "amount")
                                .unwrap()
                                .data_type = "DECIMAL(31,8)".into();
                        }
                    }
                    for r in &mut m.relations {
                        r.child.schema = "public".into();
                        r.parent.schema = "public".into();
                    }
                }
                let selected = m
                    .entities
                    .iter()
                    .find(|e| e.id.name == "customers")
                    .unwrap()
                    .id
                    .clone();
                ErDocument::new(
                    m,
                    Some(ErViewState {
                        zoom: 0.8,
                        origin: yy_draw::Point::new(0., 0.),
                        selected: Some(selected),
                    }),
                )
                .save(&stage.path().join(name))?;
                ErDocument::load(&stage.path().join(name))?;
            }
        }
        drop(db);
        println!(
            "{}: seeded counts, exact values, foreign keys and plan OK",
            kind.name()
        );
    }
    let set = transfer::read_delimited(
        &root.join("data/import_sample.csv"),
        &transfer::Delimited::default(),
    )?;
    assert_eq!(set.row_count, 5);
    transfer::export_parquet(&set, &stage.path().join("import_sample.parquet"))?;
    assert_eq!(
        transfer::read_parquet(&stage.path().join("import_sample.parquet"))?.row_count,
        5
    );
    let layout = yy_cobol::parse(&std::fs::read_to_string(
        root.join("data/fixed_sample.cbl"),
    )?)?;
    let fixed = transfer::read_fixed(
        &root.join("data/fixed_sample.dat"),
        &layout,
        yy_cobol::Codec::new(yy_cobol::Charset::Ms932),
        b"",
    )?;
    assert_eq!(fixed.row_count, 3);
    for entry in std::fs::read_dir(stage.path())? {
        let entry = entry?;
        install(
            &output.join(entry.file_name()),
            &std::fs::read(entry.path())?,
        )?;
    }
    println!(
        "Parquet/CSV/fixed-length/YAML roundtrips OK; fixtures saved to {}",
        output.display()
    );
    Ok(())
}

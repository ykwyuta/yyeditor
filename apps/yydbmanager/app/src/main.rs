#![cfg_attr(windows, windows_subsystem = "windows")]
use std::{path::PathBuf, sync::atomic::AtomicBool};
use yy_db_core::{Database, DbKind, Options};

fn execute() -> yy_db_core::Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    #[cfg(windows)]
    if args.first().is_some_and(|a| a == "--ui-smoke-test") {
        let dir = PathBuf::from(args.get(1).ok_or("出力フォルダーを指定してください")?);
        yy_db_ui::smoke(dir.clone()).map_err(|e| e.to_string())?;
        if !dir.join("ui-smoke.txt").exists() {
            return Err("UI試験が完了しませんでした".into());
        }
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "--smoke-test") {
        let dir = PathBuf::from(
            args.get(1)
                .ok_or("--smoke-test に出力フォルダーを指定してください")?,
        );
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut report = vec![];
        for kind in [DbKind::Sqlite, DbKind::DuckDb] {
            let mut db = Database::open(&Options::memory(kind))?;
            let sets=db.execute("CREATE TABLE t (id BIGINT, name VARCHAR); INSERT INTO t VALUES (9223372036854775807, '日本語'); SELECT * FROM t",&AtomicBool::new(false))?;
            let set = sets.last().ok_or("結果がありません")?;
            let path = dir.join(format!("{}.parquet", kind.name()));
            yy_db_core::transfer::export_parquet(set, &path)?;
            if yy_db_core::transfer::read_parquet(&path)?.row_count != 1 {
                return Err("Parquet検証に失敗しました".into());
            }
            let plan = db.explain("SELECT * FROM t", false, &AtomicBool::new(false))?;
            let doc = plan.drawing();
            let renderer = yy_db_core::plan::drawing_renderer();
            std::fs::write(
                dir.join(format!("{}-plan.svg", kind.name())),
                renderer.svg(&doc, &yy_db_core::plan::drawing_export(&doc))?,
            )
            .map_err(|e| e.to_string())?;
            report.push(format!(
                "{}: SQL, exact integer, Parquet and plan OK",
                kind.name()
            ));
        }
        std::fs::write(dir.join("smoke-test.txt"), report.join("\n")).map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[cfg(windows)]
    yy_db_ui::run(args.first().map(PathBuf::from)).map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    return Err("GUIはWindows専用です。--smoke-test <dir> は他のOSでも使用できます".into());
    #[cfg(windows)]
    Ok(())
}
fn main() {
    if let Err(e) = execute() {
        eprintln!("yydbmanager: {e}");
        if let Ok(path) = std::env::var("YYDBMANAGER_ERROR_FILE") {
            let _ = std::fs::write(path, &e);
        }
        std::process::exit(1);
    }
}

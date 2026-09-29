//! 区切り文字モードの性能計測（M5 の完了条件の確認用）。
//!
//! ```text
//! csv-bench <ファイル>
//! ```

use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use yy_core::Document;
use yy_delimited::{Dialect, RecordIndex};
use yy_layout::cells::measure_widths;
use yy_layout::{CellLayout, ColumnConfig, RowConfig, Viewport, rows_from};

fn main() -> ExitCode {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("usage: csv-bench <file>");
        return ExitCode::FAILURE;
    };
    let doc = match Document::open(Path::new(&path)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("open failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let snap = doc.snapshot().clone();
    let len = snap.len();
    println!(
        "file size          : {:.2} GB",
        len as f64 / (1u64 << 30) as f64
    );
    let d = Dialect::csv();

    // 開いた直後（インデックスなし）の最初の画面
    let index = Arc::new(Mutex::new(RecordIndex::new(d)));
    let t = Instant::now();
    let cl = CellLayout::new(d, Vec::new(), ColumnConfig::default(), index.clone());
    let mut widths = Vec::new();
    measure_widths(&snap, &cl, 0, 1000, 8192, 60, &mut widths);
    let cl = Arc::new(CellLayout::new(
        d,
        widths.clone(),
        ColumnConfig::default(),
        index.clone(),
    ));
    let cfg = RowConfig::default().with_cells(cl);
    let rows = rows_from(&snap, &cfg, 0, 60);
    println!(
        "first screen       : {:?} ({} rows, {} columns)",
        t.elapsed(),
        rows.len(),
        widths.len()
    );

    // バックグラウンドで行うインデックスの作成
    let t = Instant::now();
    {
        let mut idx = index.lock().unwrap();
        while !idx.extend(&snap, 4 << 20) {}
    }
    let secs = t.elapsed().as_secs_f64();
    let records = index.lock().unwrap().record_count(&snap).unwrap_or(0);
    println!(
        "record index       : {secs:.2}s ({:.2} GB/s, {records} records)",
        len as f64 / secs / 1e9
    );

    // 中央へジャンプして 1 画面を表示
    let cl = Arc::new(CellLayout::new(
        d,
        widths,
        ColumnConfig::default(),
        index.clone(),
    ));
    let cfg = RowConfig::default().with_cells(cl);
    let mut vp = Viewport::default();
    let t = Instant::now();
    vp.scroll_to_fraction(&snap, &cfg, 0.5, 60);
    let rows = rows_from(&snap, &cfg, vp.top, 60);
    println!(
        "jump to 50% + draw : {:?} ({} rows)",
        t.elapsed(),
        rows.len()
    );
    ExitCode::SUCCESS
}

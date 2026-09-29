//! ファイルを開いて最初の画面を用意するまでの時間と、行数カウントの時間を計測する。
//!
//! M1 の完了条件（10 GB のファイルを 0.5 秒以内に表示、任意位置へジャンプ）の確認用。
//!
//! ```text
//! open-bench <ファイル>
//! ```

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use yy_buffer::LineLookup;
use yy_core::Document;
use yy_jobs::JobPool;
use yy_layout::{RowConfig, Viewport, rows_from};

const PAGE: usize = 60;

fn main() -> ExitCode {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("usage: open-bench <file>");
        return ExitCode::FAILURE;
    };
    let cfg = RowConfig::default();
    let pool = JobPool::new(0);

    let t = Instant::now();
    let mut doc = match Document::open(Path::new(&path)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("open failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let rows = rows_from(doc.snapshot(), cfg, 0, PAGE);
    let first_screen = t.elapsed();
    println!(
        "file size            : {:.2} GB",
        doc.file_len() as f64 / (1u64 << 30) as f64
    );
    println!("pieces               : {}", doc.snapshot().summary().pieces);
    println!(
        "open + first screen  : {first_screen:?} ({} rows)",
        rows.len()
    );

    // 行数が未確定のまま任意位置（中央・末尾）へジャンプ
    let snap = doc.snapshot().clone();
    let mut vp = Viewport::default();
    let t = Instant::now();
    vp.scroll_to_fraction(&snap, cfg, 0.5, PAGE);
    let rows = rows_from(&snap, cfg, vp.top, PAGE);
    println!(
        "jump to 50% (unindexed): {:?} (estimated line {})",
        t.elapsed(),
        snap.line_of_offset(vp.top).line + 1
    );
    let t = Instant::now();
    vp.scroll_to_offset(&snap, cfg, u64::MAX, PAGE);
    println!("jump to end          : {:?}", t.elapsed());
    let t = Instant::now();
    vp.scroll_rows(&snap, cfg, -(PAGE as i64) * 10, PAGE);
    println!("scroll up 10 pages   : {:?}", t.elapsed());
    drop(rows);

    // バックグラウンドの行数カウント
    let t = Instant::now();
    doc.start_indexing(&pool, Arc::new(|| {}));
    doc.wait_indexing();
    let secs = t.elapsed().as_secs_f64();
    let lines = doc.snapshot().line_count().unwrap_or(0);
    println!(
        "index all lines      : {secs:.2}s ({:.2} GB/s, {lines} lines)",
        doc.file_len() as f64 / secs / 1e9
    );

    // 行番号でのジャンプ
    let snap = doc.snapshot().clone();
    let target = lines * 3 / 4;
    let t = Instant::now();
    let lookup = snap.line_start(target, false);
    let el = t.elapsed();
    match lookup {
        LineLookup::Found(off) => println!("goto line {target}  : {el:?} (offset {off})"),
        other => println!("goto line {target}  : {other:?}"),
    }
    let t = Instant::now();
    let pos = snap.line_of_offset(snap.len() / 3);
    println!(
        "line of offset 33%   : {:?} (line {})",
        t.elapsed(),
        pos.line + 1
    );
    ExitCode::SUCCESS
}

//! 検索・すべて置換の時間とメモリ使用量を計測する（M4 の完了条件の確認用）。
//!
//! ```text
//! replace-bench <ファイル> <検索文字列> <置換文字列> [regex]
//! ```

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use yy_core::{Document, Query, Replacement, Searcher};
use yy_jobs::JobPool;

/// `/proc/self/status` の項目（MB。Linux のみ。取れなければ 0）。
fn status_mb(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines().find(|l| l.starts_with(key)).and_then(|l| {
                l.split_whitespace()
                    .nth(1)
                    .and_then(|n| n.parse::<u64>().ok())
            })
        })
        .map_or(0, |kb| kb / 1024)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: replace-bench <file> <pattern> <replacement> [regex]");
        return ExitCode::FAILURE;
    }
    let regex = args.get(4).is_some_and(|a| a == "regex");
    let pool = JobPool::new(0);
    let mut doc = match Document::open(Path::new(&args[1])) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("open failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    if doc.is_loading() {
        doc.start_indexing(&pool, Arc::new(|| {}));
        doc.wait_loading();
    }
    let q = Query {
        pattern: args[2].clone(),
        regex,
        case_sensitive: true,
        whole_word: false,
    };
    let searcher = match Searcher::new(&q) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let snap = doc.snapshot().clone();
    let len = snap.len();
    println!(
        "file size        : {:.2} GB",
        len as f64 / (1u64 << 30) as f64
    );

    let t = Instant::now();
    let n = searcher
        .count(&snap, 0..len, u64::MAX, &mut |_| true)
        .unwrap();
    let s = t.elapsed().as_secs_f64();
    println!(
        "count matches    : {n} in {s:.2}s ({:.2} GB/s)",
        len as f64 / s / 1e9
    );

    let t = Instant::now();
    let found = searcher
        .find_prev(&snap, 0..len, len, &mut |_| true)
        .unwrap();
    println!("find last match  : {:?} {:?}", t.elapsed(), found);

    let repl = if regex {
        match Replacement::parse(&args[3], &searcher) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        Replacement::literal(&args[3])
    };
    let t = Instant::now();
    let r = doc.replace_all(searcher, repl, 0..len, &pool, Arc::new(|| {}));
    let count = match r {
        Ok(Some(n)) => n,
        Ok(None) => {
            while doc.is_busy() {
                doc.poll_indexing();
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            match doc.take_replace_result() {
                Some(Ok(n)) => n,
                other => {
                    eprintln!("replace failed: {other:?}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Err(e) => {
            eprintln!("replace failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let s = t.elapsed().as_secs_f64();
    println!(
        "replace all      : {count} replacements in {s:.2}s ({:.2} GB/s), new size {:.2} GB",
        len as f64 / s / 1e9,
        doc.snapshot().len() as f64 / (1u64 << 30) as f64
    );
    let t = Instant::now();
    doc.undo();
    println!("undo             : {:?}", t.elapsed());
    // RSS にはメモリマップしたファイルのページも含まれるので、ヒープなどの匿名メモリを別に表示する
    println!(
        "memory           : anonymous {} MB (RSS incl. mapped files {} MB)",
        status_mb("RssAnon:"),
        status_mb("VmRSS:")
    );
    ExitCode::SUCCESS
}

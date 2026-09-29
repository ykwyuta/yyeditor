use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use yy_buffer::{Snapshot, SourceRef, count_lf};

const SIZE: usize = 64 << 20;

fn sample() -> SourceRef {
    let line = b"2026-09-28 12:34:56.789 INFO  yyeditor: benchmark line with some payload\n";
    let mut v = Vec::with_capacity(SIZE);
    while v.len() + line.len() <= SIZE {
        v.extend_from_slice(line);
    }
    Arc::new(v)
}

fn bench(c: &mut Criterion) {
    let src = sample();
    let len = src.bytes().len() as u64;

    c.bench_function("open_64mib_unindexed", |b| {
        b.iter(|| Snapshot::from_source(black_box(src.clone()), 0..len, false))
    });

    c.bench_function("index_64mib", |b| {
        let snap = Snapshot::from_source(src.clone(), 0..len, false);
        b.iter(|| snap.fill_line_counts(&|p| Some(count_lf(p.bytes()))))
    });

    let snap = Snapshot::from_source(src.clone(), 0..len, true);
    let lines = snap.line_count().unwrap();
    c.bench_function("line_start_random", |b| {
        let mut i = 0u64;
        b.iter(|| {
            i = (i + 7_919_993) % lines;
            snap.line_start(black_box(i), false)
        })
    });

    c.bench_function("line_of_offset_random", |b| {
        let mut i = 0u64;
        b.iter(|| {
            i = (i + 7_919_993) % len;
            snap.line_of_offset(black_box(i))
        })
    });

    c.bench_function("insert_char_random", |b| {
        let mut i = 0u64;
        b.iter(|| {
            i = (i + 7_919_993) % len;
            snap.insert(black_box(i), b"x")
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);

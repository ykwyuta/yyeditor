use std::io::Write;
use std::time::Instant;

fn corpus() -> Vec<u8> {
    let mut s = String::from("id,category,title,amount,date\n");
    for n in 0..1400 {
        s.push_str(&format!(
            "{n},category-{},\"日本語テスト{}・圧縮レベル互換性の確認\",{}.{:02},2026-10-{:02}\n",
            n % 23,
            n % 137,
            n * 37,
            n % 100,
            n % 28 + 1
        ));
    }
    s.into_bytes()
}
fn random_bytes(size: usize) -> Vec<u8> {
    let mut state = 0x123456789abcdef0u64;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}
fn native(input: &[u8], level: i32) -> Vec<u8> {
    zstd::bulk::compress(input, level).unwrap()
}
fn native_checksummed(input: &[u8], level: i32) -> Vec<u8> {
    let mut encoder = zstd::Encoder::new(Vec::new(), level).unwrap();
    encoder.include_checksum(true).unwrap();
    encoder.write_all(input).unwrap();
    encoder.finish().unwrap()
}
fn main() {
    if std::env::args().any(|arg| arg == "--benchmark") {
        benchmark();
        return;
    }
    parquet_interop();
    let dir = std::path::Path::new("apps/yydbmanager/crates/yy-parquet-zstd/tests/fixtures");
    std::fs::create_dir_all(dir).unwrap();
    let csv = corpus();
    std::fs::write(dir.join("corpus.csv"), &csv).unwrap();
    let version = zstd::zstd_safe::version_string();
    println!("reference libzstd {version}");
    let mut long_range = random_bytes(131072);
    let prefix = long_range.clone();
    long_range.extend(random_bytes(65536));
    long_range.extend(prefix);
    let mut rows = Vec::new();
    for level in 1..=22 {
        std::fs::write(
            dir.join(format!("level-{level:02}.zst")),
            native_checksummed(&csv, level),
        )
        .unwrap();
        for (name, input) in [
            ("csv", csv.clone()),
            ("random", random_bytes(131073)),
            ("long-range", long_range.clone()),
            ("empty", vec![]),
        ] {
            let start = Instant::now();
            let mut encoded = Vec::new();
            yy_parquet_zstd::compress(&input, &mut encoded, level).unwrap();
            let pure_ns = start.elapsed().as_nanos();
            assert_eq!(
                zstd::decode_all(&*encoded).unwrap(),
                input,
                "native decode of pure level {level} {name}"
            );
            let start = Instant::now();
            let reference = native(&input, level);
            let native_ns = start.elapsed().as_nanos();
            let mut decoded = Vec::new();
            yy_parquet_zstd::decompress(&reference, &mut decoded, Some(input.len())).unwrap();
            assert_eq!(decoded, input, "pure decode of native level {level} {name}");
            rows.push(serde_json::json!({"level":level,"corpus":name,"input":input.len(),"pure_size":encoded.len(),"native_size":reference.len(),"pure_ns":pure_ns.to_string(),"native_ns":native_ns.to_string()}));
        }
        println!("level {level}: 4 corpora, both directions passed");
    }
    let data = serde_json::json!({"reference":version,"samples":rows});
    std::fs::write(
        "target/zstd-compatibility.json",
        serde_json::to_vec_pretty(&data).unwrap(),
    )
    .unwrap();
}

fn parquet_interop() {
    use arrow_array::{
        ArrayRef, BinaryArray, Decimal128Array, Int64Array, RecordBatch, StringArray,
    };
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("title", DataType::Utf8, true),
        Field::new("amount", DataType::Decimal128(38, 8), false),
        Field::new("payload", DataType::Binary, false),
    ]));
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from_iter_values(0..2048)),
        Arc::new(StringArray::from_iter(
            (0..2048).map(|n| (n % 11 != 0).then_some("日本語の辞書値")),
        )),
        Arc::new(
            Decimal128Array::from_iter_values(std::iter::repeat_n(
                12345678901234567890123456789012345678i128,
                2048,
            ))
            .with_precision_and_scale(38, 8)
            .unwrap(),
        ),
        Arc::new(BinaryArray::from_iter_values(std::iter::repeat_n(
            &[0u8, 255, 127][..],
            2048,
        ))),
    ];
    let batch = RecordBatch::try_new(schema.clone(), arrays).unwrap();
    let dir = std::path::Path::new("apps/yydbmanager/tests/fixtures/zstd-parquet");
    std::fs::create_dir_all(dir).unwrap();
    let temp = tempfile::tempdir().unwrap();
    macro_rules! write_parquet {
        ($codec:ident,$path:expr,$level:expr,$v2:expr) => {{
            use $codec::file::properties::{WriterProperties, WriterVersion};
            let props = WriterProperties::builder()
                .set_compression($codec::basic::Compression::ZSTD(
                    $codec::basic::ZstdLevel::try_new($level).unwrap(),
                ))
                .set_writer_version(if $v2 {
                    WriterVersion::PARQUET_2_0
                } else {
                    WriterVersion::PARQUET_1_0
                })
                .set_dictionary_enabled(true)
                .set_max_row_group_size(512)
                .set_data_page_size_limit(4096)
                .build();
            let mut writer = $codec::arrow::ArrowWriter::try_new(
                std::fs::File::create($path).unwrap(),
                schema.clone(),
                Some(props),
            )
            .unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
        }};
    }
    macro_rules! verify {
        ($codec:ident,$path:expr) => {{
            let reader = $codec::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
                std::fs::File::open($path).unwrap(),
            )
            .unwrap()
            .with_batch_size(2048)
            .build()
            .unwrap();
            let batches: Vec<_> = reader.map(Result::unwrap).collect();
            assert_eq!(batches, vec![batch.clone()]);
        }};
    }
    for level in 1..=22 {
        let path = dir.join(format!("native-{level}.parquet"));
        write_parquet!(native_parquet, &path, level, false);
        verify!(rust_parquet, &path);
        for v2 in [false, true] {
            let path = temp.path().join("pure.parquet");
            write_parquet!(rust_parquet, &path, level, v2);
            verify!(native_parquet, &path);
        }
        println!("Parquet level {level}: native/pure and both writer versions passed");
    }
}

fn benchmark() {
    let data = corpus();
    let mut rows = Vec::new();
    for level in 1..=22 {
        let mut pure_times = Vec::new();
        let mut native_times = Vec::new();
        let mut pure_size = 0;
        let mut native_size = 0;
        // Warm each implementation before collecting five alternated samples.
        for sample in 0..6 {
            for pure_first in if sample % 2 == 0 {
                [true, false]
            } else {
                [false, true]
            } {
                let start = Instant::now();
                let encoded = if pure_first {
                    let mut output = Vec::new();
                    yy_parquet_zstd::compress(std::hint::black_box(&data), &mut output, level)
                        .unwrap();
                    output
                } else {
                    native(std::hint::black_box(&data), level)
                };
                let elapsed = start.elapsed().as_nanos() as u64;
                std::hint::black_box(&encoded);
                if pure_first {
                    pure_size = encoded.len();
                    if sample > 0 {
                        pure_times.push(elapsed);
                    }
                } else {
                    native_size = encoded.len();
                    if sample > 0 {
                        native_times.push(elapsed);
                    }
                }
            }
        }
        pure_times.sort();
        native_times.sort();
        rows.push(serde_json::json!({"level":level,"input_bytes":data.len(),"pure_bytes":pure_size,"native_bytes":native_size,"pure_median_ns":pure_times[2],"native_median_ns":native_times[2],"pure_samples_ns":pure_times,"native_samples_ns":native_times}));
    }
    let result = serde_json::json!({"reference":zstd::zstd_safe::version_string(),"corpus":"synthetic CSV with UTF-8, 1400 rows","release":true,"samples_per_implementation":5,"rows":rows});
    std::fs::write(
        "target/zstd-benchmark.json",
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
    println!(
        "All 22 levels measured; five samples per backend, median in target/zstd-benchmark.json"
    );
}

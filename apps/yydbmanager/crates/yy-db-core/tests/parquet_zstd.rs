//! Offline Parquet fixtures from the independent native libzstd reference tool.
use std::sync::Arc;

use duckdb::arrow::{
    array::{ArrayRef, BinaryArray, Decimal128Array, Int64Array, StringArray},
    datatypes::{DataType, Field, Schema},
    record_batch::RecordBatch,
};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::{WriterProperties, WriterVersion};
use parquet::file::reader::{FileReader, SerializedFileReader};
use yy_db_core::{Value, transfer};

#[test]
fn reads_native_zstd_parquet_at_all_22_levels() {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/zstd-parquet");
    for level in 1..=22 {
        let path = dir.join(format!("native-{level}.parquet"));
        let set = transfer::read_parquet(&path).unwrap();
        assert_eq!(set.row_count, 2048, "native level {level}");
        for start in [0, 1024, 2047] {
            let row = set.page(start, 1).unwrap().remove(0);
            assert_eq!(row[0], Value::Integer(start.to_string()));
            assert_eq!(
                row[1],
                if start % 11 == 0 {
                    Value::Null
                } else {
                    Value::Text("日本語の辞書値".into())
                }
            );
            assert_eq!(
                row[2],
                Value::Decimal("123456789012345678901234567890.12345678".into())
            );
            assert_eq!(row[3], Value::Binary(vec![0, 255, 127]));
        }
    }
}

#[test]
fn writes_and_reads_rust_zstd_parquet_at_all_22_levels() {
    let dir = tempfile::tempdir().unwrap();
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("title", DataType::Utf8, true),
        Field::new("amount", DataType::Decimal128(38, 8), false),
        Field::new("payload", DataType::Binary, false),
    ]));
    let bytes = [0, 255, 127];
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
            bytes.as_slice(),
            2048,
        ))),
    ];
    let batch = RecordBatch::try_new(schema.clone(), arrays).unwrap();
    for level in 1..=22 {
        for (name, version) in [
            ("v1", WriterVersion::PARQUET_1_0),
            ("v2", WriterVersion::PARQUET_2_0),
        ] {
            let path = dir.path().join(format!("rust-{name}-{level}.parquet"));
            let properties = WriterProperties::builder()
                .set_compression(Compression::ZSTD(ZstdLevel::try_new(level).unwrap()))
                .set_writer_version(version)
                .set_dictionary_enabled(true)
                .set_max_row_group_size(512)
                .set_data_page_size_limit(4096)
                .build();
            let mut writer = ArrowWriter::try_new(
                std::fs::File::create(&path).unwrap(),
                schema.clone(),
                Some(properties),
            )
            .unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
            let reader = SerializedFileReader::new(std::fs::File::open(&path).unwrap()).unwrap();
            for group in reader.metadata().row_groups() {
                for column in group.columns() {
                    assert!(matches!(column.compression(), Compression::ZSTD(_)));
                }
            }
            let set = transfer::read_parquet(&path).unwrap();
            assert_eq!(set.row_count, 2048);
            let row = set.page(2047, 1).unwrap().remove(0);
            assert_eq!(
                row,
                vec![
                    Value::Integer("2047".into()),
                    Value::Text("日本語の辞書値".into()),
                    Value::Decimal("123456789012345678901234567890.12345678".into()),
                    Value::Binary(bytes.to_vec())
                ],
                "level {level} {name}"
            );
        }
    }
}

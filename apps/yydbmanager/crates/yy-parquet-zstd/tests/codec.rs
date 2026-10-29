use yy_parquet_zstd::{compress, decompress};

const SOURCE: &[u8] = include_bytes!("fixtures/corpus.csv");

fn fixture(level: i32) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/level-{level:02}.zst")),
    )
    .unwrap()
}

#[test]
fn reads_reference_frames_at_every_level() {
    for level in 1..=22 {
        let mut output = b"prefix".to_vec();
        let size = decompress(&fixture(level), &mut output, Some(SOURCE.len())).unwrap();
        assert_eq!(size, SOURCE.len(), "level {level}");
        assert_eq!(&output[..6], b"prefix");
        assert_eq!(&output[6..], SOURCE, "level {level}");
    }
}

#[test]
fn round_trips_all_levels_and_block_boundaries() {
    for level in 1..=22 {
        for length in [0, 1, 7, 255, 1024, 131_071, 131_072, 131_073] {
            let input: Vec<_> = (0..length).map(|n| (n % 251) as u8).collect();
            let mut encoded = b"prefix".to_vec();
            compress(&input, &mut encoded, level).unwrap();
            assert_eq!(&encoded[..6], b"prefix");
            let mut output = Vec::new();
            decompress(&encoded[6..], &mut output, Some(length)).unwrap();
            assert_eq!(output, input, "level {level}, length {length}");
        }
    }
}

#[test]
fn errors_do_not_append_partial_output() {
    for level in 1..=22 {
        let valid = fixture(level);
        let mut invalid_checksum = valid.clone();
        *invalid_checksum.last_mut().unwrap() ^= 1;
        let mut trailing = valid.clone();
        trailing.extend_from_slice(b"invalid trailing frame");
        for input in [
            &invalid_checksum[..],
            &trailing[..],
            &valid[..1],
            &valid[..valid.len() / 2],
            &valid[..valid.len() - 1],
        ] {
            let mut output = b"preserved".to_vec();
            assert!(decompress(input, &mut output, None).is_err());
            assert_eq!(output, b"preserved");
        }
        for size in [0, SOURCE.len() - 1, SOURCE.len() + 1] {
            let mut output = b"preserved".to_vec();
            assert!(decompress(&valid, &mut output, Some(size)).is_err());
            assert_eq!(output, b"preserved");
        }
    }
    for level in [i32::MIN, -1, 0, 23, i32::MAX] {
        let mut output = b"preserved".to_vec();
        assert!(compress(SOURCE, &mut output, level).is_err());
        assert_eq!(output, b"preserved");
    }
}

#[test]
fn supports_concatenated_and_skippable_frames() {
    let mut frames = vec![0x50, 0x2a, 0x4d, 0x18, 3, 0, 0, 0, b'a', b'b', b'c'];
    frames.extend(fixture(1));
    frames.extend(fixture(22));
    let mut output = Vec::new();
    decompress(&frames, &mut output, Some(SOURCE.len() * 2)).unwrap();
    assert_eq!(&output[..SOURCE.len()], SOURCE);
    assert_eq!(&output[SOURCE.len()..], SOURCE);
}

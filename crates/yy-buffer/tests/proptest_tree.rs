//! ピースツリーを素朴な `Vec<u8>` 実装（オラクル）と比較するプロパティテスト。

use std::sync::Arc;

use proptest::prelude::*;
use yy_buffer::{LineLookup, Snapshot, SourceRef};

#[derive(Debug, Clone)]
enum Op {
    Insert { at: f64, text: Vec<u8> },
    Delete { at: f64, len: usize },
    Fill,
}

fn text_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(
        prop_oneof![
            4 => b'a'..=b'z',
            2 => Just(b'\n'),
            1 => Just(0xE3u8), // UTF-8 の先頭バイト
            1 => Just(0x81u8), // 継続バイト
        ],
        0..40,
    )
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        5 => (0.0..=1.0f64, text_strategy()).prop_map(|(at, text)| Op::Insert { at, text }),
        4 => (0.0..=1.0f64, 0usize..200).prop_map(|(at, len)| Op::Delete { at, len }),
        1 => Just(Op::Fill),
    ]
}

fn line_starts(v: &[u8]) -> Vec<u64> {
    let mut starts = vec![0u64];
    starts.extend(
        v.iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| i as u64 + 1),
    );
    starts
}

fn check_equal(snap: &Snapshot, oracle: &[u8]) {
    snap.check_invariants();
    assert_eq!(snap.len(), oracle.len() as u64);
    assert_eq!(snap.read(0..snap.len()), oracle);

    let starts = line_starts(oracle);
    // 行 → オフセット（未確定ピースをその場で数える）
    for (line, &start) in starts.iter().enumerate() {
        assert_eq!(
            snap.line_start(line as u64, true),
            LineLookup::Found(start),
            "line {line}"
        );
    }
    assert_eq!(
        snap.line_start(starts.len() as u64, true),
        LineLookup::OutOfRange
    );

    if snap.is_fully_indexed() {
        assert_eq!(snap.line_count(), Some(starts.len() as u64));
        for (line, &start) in starts.iter().enumerate() {
            assert_eq!(
                snap.line_start(line as u64, false),
                LineLookup::Found(start)
            );
        }
        // オフセット → 行
        for off in 0..=oracle.len() as u64 {
            let expected = oracle[..off as usize]
                .iter()
                .filter(|b| **b == b'\n')
                .count() as u64;
            let pos = snap.line_of_offset(off);
            assert!(pos.exact);
            assert_eq!(pos.line, expected, "offset {off}");
        }
    }

    // 前方・後方探索
    let n = oracle.len() as u64;
    for off in (0..=n).step_by(7) {
        let next = oracle[off as usize..]
            .iter()
            .position(|b| *b == b'\n')
            .map(|i| off + i as u64);
        assert_eq!(snap.find_next(off..n, b'\n'), next);
        let prev = oracle[..off as usize]
            .iter()
            .rposition(|b| *b == b'\n')
            .map(|i| i as u64);
        assert_eq!(snap.find_prev(0..off, b'\n'), prev);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn matches_oracle(initial in prop::collection::vec(prop_oneof![8 => b'a'..=b'z', 2 => Just(b'\n')], 0..3000),
                      chunk in 5u32..64,
                      ops in prop::collection::vec(op_strategy(), 0..60)) {
        let source: SourceRef = Arc::new(initial.clone());
        let mut snap = Snapshot::from_source_with_chunk(source, 0..initial.len() as u64, chunk, false);
        let mut oracle = initial;
        // 古いスナップショットが後の編集の影響を受けないことも確認する
        let mut history: Vec<(Snapshot, Vec<u8>)> = Vec::new();
        check_equal(&snap, &oracle);
        for op in ops {
            history.push((snap.clone(), oracle.clone()));
            match op {
                Op::Insert { at, text } => {
                    let pos = (at * oracle.len() as f64) as usize;
                    snap = snap.insert(pos as u64, &text);
                    oracle.splice(pos..pos, text);
                }
                Op::Delete { at, len } => {
                    let pos = (at * oracle.len() as f64) as usize;
                    let end = (pos + len).min(oracle.len());
                    snap = snap.delete(pos as u64..end as u64);
                    oracle.drain(pos..end);
                }
                Op::Fill => {
                    snap = snap.fill_line_counts(&|p| Some(yy_buffer::count_lf(p.bytes())));
                    prop_assert!(snap.is_fully_indexed());
                }
            }
            check_equal(&snap, &oracle);
        }
        for (s, o) in &history {
            prop_assert_eq!(&s.read(0..s.len()), o);
        }
    }
}

#[test]
fn large_tree_is_balanced_and_shallow() {
    let data: Vec<u8> = (0..200_000u32)
        .map(|i| if i % 50 == 49 { b'\n' } else { b'x' })
        .collect();
    let len = data.len() as u64;
    let snap = Snapshot::from_source_with_chunk(Arc::new(data), 0..len, 16, true);
    let depth = snap.check_invariants();
    assert!(depth <= 4, "depth {depth}");
    assert_eq!(snap.line_count(), Some(4001));
    assert_eq!(snap.line_start(4000, false), LineLookup::Found(200_000));
    assert_eq!(snap.line_start(123, false), LineLookup::Found(123 * 50));

    // 大量の小さな編集の後も木は健全
    let mut s = snap.clone();
    for i in 0..2000u64 {
        let at = (i * 7919) % s.len();
        s = if i % 2 == 0 {
            s.insert(at, b"ab\n")
        } else {
            s.delete(at..(at + 3).min(s.len()))
        };
    }
    s.check_invariants();
}

#[test]
fn unindexed_lookup_reports_not_indexed_and_estimates() {
    let data: Vec<u8> = (0..10_000u32)
        .map(|i| if i % 10 == 9 { b'\n' } else { b'y' })
        .collect();
    let len = data.len() as u64;
    let snap = Snapshot::from_source_with_chunk(Arc::new(data), 0..len, 100, false);
    assert_eq!(snap.line_count(), None);
    assert_eq!(snap.line_start(5, false), LineLookup::NotIndexed);
    assert_eq!(snap.line_start(5, true), LineLookup::Found(50));
    let pos = snap.line_of_offset(5000);
    assert!(!pos.exact);

    // 前半だけ数えると、前半では正確・後半では推定になる
    let half: Vec<_> = snap.unindexed_pieces(50);
    let keys: std::collections::HashMap<_, _> = half
        .iter()
        .map(|p| (p.key(), yy_buffer::count_lf(p.bytes())))
        .collect();
    let snap = snap.fill_line_counts(&|p| keys.get(&p.key()).copied());
    assert_eq!(snap.line_start(100, false), LineLookup::Found(1000));
    let early = snap.line_of_offset(1234);
    assert!(early.exact);
    assert_eq!(early.line, 123);
    let late = snap.line_of_offset(9000);
    assert!(!late.exact);
    assert!(
        (late.line as i64 - 900).abs() <= 10,
        "estimate {}",
        late.line
    );
    assert!((snap.estimated_line_count() as i64 - 1001).abs() <= 10);
}

#[test]
fn piece_split_respects_utf8_boundaries() {
    let text = "あいうえおかきくけこ\n".repeat(100);
    let len = text.len() as u64;
    let src: SourceRef = Arc::new(text.clone().into_bytes());
    let pieces = yy_buffer::split_into_pieces(&src, 0, len, 16, true);
    for p in &pieces {
        assert!(std::str::from_utf8(p.bytes()).is_ok());
    }
    let snap = Snapshot::from_source_with_chunk(src, 0..len, 16, true);
    assert_eq!(snap.read(0..len), text.as_bytes());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// ピースの境界をまたぐ一致も含めて、素朴な検索と同じ位置を返す。
    #[test]
    fn find_bytes_matches_naive_search(
        data in prop::collection::vec(prop_oneof![Just(b'a'), Just(b'b'), Just(b'c')], 0..400),
        needle in prop::collection::vec(prop_oneof![Just(b'a'), Just(b'b')], 1..6),
        chunk in 5u32..20,
        from in 0.0..=1.0f64,
    ) {
        let len = data.len() as u64;
        let snap = Snapshot::from_source_with_chunk(Arc::new(data.clone()), 0..len, chunk, true);
        let start = (from * len as f64) as usize;
        let naive = data[start..]
            .windows(needle.len())
            .position(|w| w == needle.as_slice())
            .map(|i| (start + i) as u64);
        prop_assert_eq!(snap.find_bytes(start as u64..len, &needle), naive);
    }

    /// 切り出したピース列を並べると元の範囲の内容になる。
    #[test]
    fn pieces_in_reproduces_range(
        data in prop::collection::vec(any::<u8>(), 0..400),
        chunk in 5u32..20,
        a in 0.0..=1.0f64,
        b in 0.0..=1.0f64,
    ) {
        let len = data.len() as u64;
        let snap = Snapshot::from_source_with_chunk(Arc::new(data.clone()), 0..len, chunk, false);
        let (x, y) = ((a * len as f64) as u64, (b * len as f64) as u64);
        let (lo, hi) = (x.min(y), x.max(y));
        let bytes: Vec<u8> = snap.pieces_in(lo..hi).iter().flat_map(|p| p.bytes().to_vec()).collect();
        prop_assert_eq!(bytes, data[lo as usize..hi as usize].to_vec());
    }
}

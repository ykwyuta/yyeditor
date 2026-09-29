use std::sync::Arc;

use proptest::prelude::*;
use yy_buffer::Snapshot;
use yy_layout::{
    RowConfig, Viewport, is_line_start, next_row_start, prev_row_start, row_containing, rows_from,
};

fn snapshot(data: &[u8], chunk: u32) -> Snapshot {
    let len = data.len() as u64;
    Snapshot::from_source_with_chunk(Arc::new(data.to_vec()), 0..len, chunk, true)
}

fn forward_rows(snap: &Snapshot, cfg: RowConfig) -> Vec<u64> {
    let mut rows = vec![0];
    while let Some(n) = next_row_start(snap, cfg, *rows.last().unwrap()) {
        assert!(n > *rows.last().unwrap());
        rows.push(n);
    }
    rows
}

fn backward_rows(snap: &Snapshot, cfg: RowConfig) -> Vec<u64> {
    let len = snap.len();
    let mut rows = vec![row_containing(snap, cfg, len)];
    while let Some(p) = prev_row_start(snap, cfg, *rows.last().unwrap()) {
        rows.push(p);
    }
    rows.reverse();
    rows
}

fn text() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(
        prop_oneof![
            10 => b'a'..=b'z',
            1 => Just(b'\n'),
            1 => Just(b'\r'),
            // 3 バイト文字「あ」
            1 => Just(0xE3u8),
        ],
        0..600,
    )
    .prop_map(|v| {
        // 0xE3 を「あ」(E3 81 82) に展開して有効な UTF-8 にする
        let mut out = Vec::with_capacity(v.len() * 2);
        for b in v {
            if b == 0xE3 {
                out.extend_from_slice("あ".as_bytes());
            } else {
                out.push(b);
            }
        }
        out
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn forward_and_backward_agree(data in text(), s in 8u64..40, chunk in 5u32..50) {
        let snap = snapshot(&data, chunk);
        let cfg = RowConfig::new(s);
        let fwd = forward_rows(&snap, cfg);
        let bwd = backward_rows(&snap, cfg);
        prop_assert_eq!(&fwd, &bwd);

        let len = data.len() as u64;
        for (i, &r) in fwd.iter().enumerate() {
            let next = fwd.get(i + 1).copied().unwrap_or(len);
            // 表示行の長さの上限
            prop_assert!(next - r <= 2 * s + 3, "row {}..{} too long", r, next);
            // 文字境界で分割されている
            prop_assert!(std::str::from_utf8(&data[r as usize..next as usize]).is_ok());
            // row_containing はその行を返す
            for off in r..next {
                prop_assert_eq!(row_containing(&snap, cfg, off), r);
            }
        }
        // すべての論理行の先頭は表示行の先頭
        for (i, _) in data.iter().enumerate().filter(|(_, b)| **b == b'\n') {
            prop_assert!(fwd.contains(&(i as u64 + 1)));
        }
        // S 以下の長さの論理行は分割されない
        let mut ls = 0usize;
        for (i, b) in data.iter().enumerate() {
            if *b == b'\n' {
                if (i + 1 - ls) as u64 <= s {
                    for r in &fwd {
                        prop_assert!(!(*r > ls as u64 && *r <= i as u64), "short line split");
                    }
                }
                ls = i + 1;
            }
        }
        // rows_from で取り出した行数が一致する
        let rows = rows_from(&snap, cfg, 0, usize::MAX);
        prop_assert_eq!(rows.len(), fwd.len());
    }
}

#[test]
fn very_long_single_line_is_segmented() {
    let data = vec![b'x'; 100_000];
    let snap = snapshot(&data, 4096);
    let cfg = RowConfig::new(1000);
    let rows = forward_rows(&snap, cfg);
    // 論理行の先頭から S 以上離れた格子点ごとに分割される
    assert_eq!(&rows[..3], &[0, 1000, 2000]);
    assert_eq!(rows.len(), 100);
    // 末尾付近からでも後方に辿れる
    assert_eq!(prev_row_start(&snap, cfg, 55_500), Some(55_000));
}

#[test]
fn viewport_scrolls_and_clamps() {
    let mut data = Vec::new();
    for i in 0..100 {
        data.extend_from_slice(format!("line {i}\n").as_bytes());
    }
    let snap = snapshot(&data, 64);
    let cfg = RowConfig::new(64);
    let mut vp = Viewport::default();
    assert!(vp.scroll_rows(&snap, cfg, 10, 20));
    assert_eq!(snap.line_of_offset(vp.top).line, 10);
    vp.scroll_rows(&snap, cfg, 1000, 20);
    // 最終行（空行 = 行 100）が最下段に来る位置で止まる
    assert_eq!(snap.line_of_offset(vp.top).line, 81);
    vp.scroll_rows(&snap, cfg, -5, 20);
    assert_eq!(snap.line_of_offset(vp.top).line, 76);
    vp.scroll_to_fraction(&snap, cfg, 0.0, 20);
    assert_eq!(vp.top, 0);
    vp.scroll_to_fraction(&snap, cfg, 0.5, 20);
    assert!(is_line_start(&snap, vp.top));
    assert!(!vp.scroll_rows(&snap, cfg, -1000, 20) || vp.top == 0);
    assert_eq!(vp.top, 0);
}

#[test]
fn rows_have_decoded_text() {
    let snap = snapshot(b"abc\r\n\xFFdef\n", 64);
    let rows = rows_from(&snap, RowConfig::default(), 0, 10);
    let texts: Vec<_> = rows.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts, vec!["abc", "\\xFFdef", ""]);
    assert!(rows[0].line_start && rows[1].line_start && rows[2].line_start);
}

#[test]
fn ensure_visible_scrolls_minimally() {
    let mut data = Vec::new();
    for i in 0..100 {
        data.extend_from_slice(format!("line {i}\n").as_bytes());
    }
    let snap = snapshot(&data, 64);
    let cfg = RowConfig::new(64);
    let line = |n: u64| match snap.line_start(n, false) {
        yy_buffer::LineLookup::Found(o) => o,
        other => panic!("{other:?}"),
    };
    let mut vp = Viewport::default();
    assert!(!vp.ensure_visible(&snap, cfg, line(9), 10));
    assert!(vp.ensure_visible(&snap, cfg, line(10), 10));
    assert_eq!(vp.top, line(1));
    assert!(vp.ensure_visible(&snap, cfg, line(50) + 2, 10));
    assert_eq!(vp.top, line(41));
    assert!(vp.ensure_visible(&snap, cfg, line(5), 10));
    assert_eq!(vp.top, line(5));
}

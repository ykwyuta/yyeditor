//! 表示桁と矩形選択のテスト（全角・タブ・不正バイトの混在）。

use yy_buffer::Snapshot;
use yy_layout::columns::{Edge, col_of, content_cols, offset_at_col, units};
use yy_layout::rect::{self, RectSelection};
use yy_layout::{ColumnConfig, RowConfig, row_at, rows_from};

fn snap(s: &str) -> Snapshot {
    Snapshot::from_bytes(s.as_bytes().to_vec())
}

fn apply(s: &Snapshot, changes: &[(std::ops::Range<u64>, Vec<u8>)]) -> String {
    let mut out = s.read(0..s.len());
    for (r, b) in changes.iter().rev() {
        out.splice(r.start as usize..r.end as usize, b.iter().copied());
    }
    String::from_utf8(out).unwrap()
}

#[test]
fn columns_count_wide_tab_invalid_and_combining() {
    let s = Snapshot::from_bytes(b"a\xE3\x81\x82\tb\xFFc\xE3\x81\x8B\xE3\x82\x9A\n".to_vec());
    let row = row_at(&s, RowConfig::default(), 0);
    let cfg = ColumnConfig::default();
    let us = units(&row, &cfg);
    let cols: Vec<(u64, u32, u32)> = us
        .iter()
        .map(|u| (u.start, u.col_start, u.col_end))
        .collect();
    // a(1) あ(2) tab(→4) b(1) \xFF(4) c(1) か+゜(2、結合文字を含めて 1 単位)
    assert_eq!(
        cols,
        vec![
            (0, 0, 1),
            (1, 1, 3),
            (4, 3, 4),
            (5, 4, 5),
            (6, 5, 9),
            (7, 9, 10),
            (8, 10, 12)
        ]
    );
    assert_eq!(us.last().unwrap().end, 14);
    assert_eq!(content_cols(&us), 12);
    assert_eq!(col_of(&us, &row, 4), 3);
    // 全角文字の途中（桁 2）
    assert_eq!(offset_at_col(&us, &row, 2, Edge::Left), (1, 1));
    assert_eq!(offset_at_col(&us, &row, 2, Edge::Right), (4, 3));
    // 行末より右
    assert_eq!(offset_at_col(&us, &row, 20, Edge::Left), (14, 12));
}

#[test]
fn rectangle_over_japanese_and_short_lines() {
    let text = "abcdef\nあいう\nxy\n\tz\n";
    let s = snap(text);
    let rc = RowConfig::default();
    let cc = ColumnConfig::default();
    let rows: Vec<u64> = rows_from(&s, rc, 0, 10).iter().map(|r| r.start).collect();
    // 桁 2〜4 の矩形（4 行）
    let r = RectSelection {
        anchor_row: rows[0],
        anchor_col: 2,
        head_row: rows[3],
        head_col: 4,
    };
    let rr = rect::rect_rows(&s, rc, &cc, &r, 100).unwrap();
    let texts: Vec<String> = rect::row_texts(&s, &rr)
        .into_iter()
        .map(|b| String::from_utf8(b).unwrap())
        .collect();
    // "cd", "い", "" (xy は 2 桁しかない), タブ（0〜4 桁）は左端にかかるので含める
    assert_eq!(texts, vec!["cd", "い", "", "\t"]);
    assert_eq!(rr[2].pad, 0);

    // 矩形の削除
    let e = rect::delete_backward(&rr, &r);
    assert_eq!(apply(&s, &e.changes), "abef\nあう\nxy\nz\n");
    assert_eq!(e.new_col, 2);
}

#[test]
fn zero_width_rectangle_typing_pads_virtual_space() {
    let s = snap("abcdef\nxy\nあいうえ\n");
    let rc = RowConfig::default();
    let cc = ColumnConfig::default();
    let rows: Vec<u64> = rows_from(&s, rc, 0, 10).iter().map(|r| r.start).collect();
    let r = RectSelection {
        anchor_row: rows[0],
        anchor_col: 4,
        head_row: rows[2],
        head_col: 4,
    };
    let rr = rect::rect_rows(&s, rc, &cc, &r, 100).unwrap();
    assert_eq!(rr[1].pad, 2);
    let e = rect::replace_rows(&rr, &["|"], &cc);
    assert_eq!(apply(&s, &e.changes), "abcd|ef\nxy  |\nあい|うえ\n");
    assert_eq!(e.new_col, 5);

    // 行ごとに異なる文字列（貼り付けの振り分け）
    let e = rect::replace_rows(&rr, &["1", "2", "3"], &cc);
    assert_eq!(apply(&s, &e.changes), "abcd1ef\nxy  2\nあい3うえ\n");

    // BackSpace: 仮想空白の行は消さない
    let e = rect::delete_backward(&rr, &r);
    assert_eq!(apply(&s, &e.changes), "abcef\nxy\nあうえ\n");
    // 新しい桁は先頭行に合わせる
    assert_eq!(e.new_col, 3);
    let e = rect::delete_forward(&rr, &r);
    assert_eq!(apply(&s, &e.changes), "abcdf\nxy\nあいえ\n");
}

#[test]
fn rect_rows_limit() {
    let text: String = (0..100).map(|i| format!("{i}\n")).collect();
    let s = snap(&text);
    let rc = RowConfig::default();
    let rows: Vec<u64> = rows_from(&s, rc, 0, 200).iter().map(|r| r.start).collect();
    let r = RectSelection {
        anchor_row: rows[0],
        anchor_col: 0,
        head_row: rows[99],
        head_col: 1,
    };
    assert!(rect::rect_rows(&s, rc, &ColumnConfig::default(), &r, 50).is_err());
    assert_eq!(
        rect::rect_rows(&s, rc, &ColumnConfig::default(), &r, 100)
            .unwrap()
            .len(),
        100
    );
}

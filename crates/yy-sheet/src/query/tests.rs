use std::collections::HashSet;

use super::*;
use crate::chunk::Chunk;
use crate::column::Piece;
use crate::value::CellError;

/// 列（値の並びを `chunk` 行ずつのチャンクに）。
fn col(ctx: &Context, name: &str, vals: &[Value], chunk: usize) -> Column {
    let mut pieces = Vec::new();
    for part in vals.chunks(chunk) {
        let ch = Chunk::create(ctx, Data::from_values(part.iter().map(CellRef::of))).unwrap();
        pieces.push(Piece {
            len: ch.rows,
            chunk: ch,
            start: 0,
        });
    }
    Column::from_pieces(name, pieces)
}

fn table(ctx: &Context, cols: Vec<Vec<Value>>) -> Table {
    let rows = cols[0].len() as u64;
    Table {
        columns: Arc::new(
            cols.iter()
                .enumerate()
                .map(|(i, v)| col(ctx, &format!("c{i}"), v, 3))
                .collect(),
        ),
        rows,
        header: true,
    }
}

fn n(x: f64) -> Value {
    Value::Number(x)
}
fn t(s: &str) -> Value {
    Value::text(s)
}

fn sample(ctx: &Context) -> Table {
    table(
        ctx,
        vec![
            vec![
                t("東京"),
                t("大阪"),
                t("tokyo"),
                t("名古屋"),
                Value::Empty,
                t("Tokyo"),
                t("大阪"),
                t("札幌"),
            ],
            vec![
                n(10.0),
                n(5.0),
                n(30.0),
                Value::Empty,
                n(20.0),
                t("x"),
                n(5.0),
                n(-1.0),
            ],
        ],
    )
}

fn rows(b: &Bitmap) -> Vec<usize> {
    b.ones().collect()
}

#[test]
fn wildcards_and_text() {
    assert!(wildcard_match("東*", "東京"));
    assert!(wildcard_match("??店", "新宿店"));
    assert!(!wildcard_match("??店", "新宿駅前店"));
    assert!(wildcard_match("*~**", "a*b"));
    assert!(!wildcard_match("*~**", "ab"));
    assert!(wildcard_match("A*", "abc"));
    assert_eq!(cmp_text("abc", "ABD"), Ordering::Less);
    assert!(eq_text("Tokyo", "TOKYO"));
}

#[test]
fn filters_with_stages() {
    let ctx = Context::for_tests();
    let tb = sample(&ctx);
    let f = |col, cond| ColFilter { col, cond };
    // 値の一覧（大文字・小文字を区別しない）
    let mut set = HashSet::new();
    set.insert(Key::Text("tokyo".into()));
    set.insert(Key::Text("大阪".into()));
    let (b, counts) = filter(
        &ctx,
        &tb,
        &[f(
            0,
            Cond::Values {
                values: set,
                blanks: false,
            },
        )],
    )
    .unwrap();
    assert_eq!(rows(&b), vec![1, 2, 5, 6]);
    assert_eq!(counts, vec![4]);
    // 2 段階: 数値 >= 5 かつ 文字列が「大*」
    let (b, counts) = filter(
        &ctx,
        &tb,
        &[
            f(
                1,
                Cond::Number {
                    op: Cmp::Ge,
                    value: 5.0,
                },
            ),
            f(
                0,
                Cond::Text {
                    op: TextOp::Wildcard,
                    pattern: "大*".into(),
                    negate: false,
                },
            ),
        ],
    )
    .unwrap();
    assert_eq!(rows(&b), vec![1, 6]);
    assert_eq!(counts, vec![5, 2]);
    // 空・範囲・上位・平均
    assert_eq!(
        rows(&filter_column(&ctx, &tb, &f(1, Cond::Blank)).unwrap()),
        vec![3]
    );
    assert_eq!(
        rows(&filter_column(&ctx, &tb, &f(0, Cond::Blank)).unwrap()),
        vec![4]
    );
    assert_eq!(
        rows(&filter_column(&ctx, &tb, &f(1, Cond::Between(5.0, 10.0))).unwrap()),
        vec![0, 1, 6]
    );
    assert_eq!(
        rows(
            &filter_column(
                &ctx,
                &tb,
                &f(
                    1,
                    Cond::Top {
                        n: 2,
                        bottom: false,
                        percent: false
                    }
                )
            )
            .unwrap()
        ),
        vec![2, 4]
    );
    assert_eq!(
        rows(
            &filter_column(
                &ctx,
                &tb,
                &f(
                    1,
                    Cond::Top {
                        n: 1,
                        bottom: true,
                        percent: false
                    }
                )
            )
            .unwrap()
        ),
        vec![7]
    );
    // 平均 (10+5+30+20+5-1)/6 = 11.5
    assert_eq!(
        rows(&filter_column(&ctx, &tb, &f(1, Cond::Average { above: true })).unwrap()),
        vec![2, 4]
    );
    assert_eq!(
        rows(
            &filter_column(
                &ctx,
                &tb,
                &f(
                    0,
                    Cond::Text {
                        op: TextOp::Contains,
                        pattern: "OK".into(),
                        negate: false
                    }
                )
            )
            .unwrap()
        ),
        vec![2, 5]
    );
    let or = Cond::Or(
        Box::new(Cond::Number {
            op: Cmp::Lt,
            value: 0.0,
        }),
        Box::new(Cond::Number {
            op: Cmp::Gt,
            value: 25.0,
        }),
    );
    assert_eq!(
        rows(&filter_column(&ctx, &tb, &f(1, or)).unwrap()),
        vec![2, 7]
    );
}

#[test]
fn filter_sees_edits() {
    let ctx = Context::for_tests();
    let mut tb = sample(&ctx);
    Arc::make_mut(&mut tb.columns)[1]
        .set(&ctx, 3, n(99.0))
        .unwrap();
    let b = filter_column(
        &ctx,
        &tb,
        &ColFilter {
            col: 1,
            cond: Cond::Number {
                op: Cmp::Gt,
                value: 50.0,
            },
        },
    )
    .unwrap();
    assert_eq!(rows(&b), vec![3]);
}

#[test]
fn value_lists() {
    let ctx = Context::for_tests();
    let tb = sample(&ctx);
    let (vals, blanks, cut) = value_counts(&ctx, &tb.columns[0], None, 100).unwrap();
    assert_eq!(blanks, 1);
    assert!(!cut);
    // tokyo と Tokyo は同じ値
    let names: Vec<(String, u64)> = vals
        .iter()
        .map(|v| (v.value.general_text(), v.count))
        .collect();
    assert_eq!(names.len(), 5);
    assert!(
        names
            .iter()
            .any(|(s, c)| s.eq_ignore_ascii_case("tokyo") && *c == 2)
    );
    assert!(names.iter().any(|(s, c)| s == "大阪" && *c == 2));
    // 絞り込みの結果だけ
    let mut mask = Bitmap::new(8, false);
    mask.set(0, true);
    mask.set(1, true);
    let (vals, blanks, _) = value_counts(&ctx, &tb.columns[0], Some(&mask), 100).unwrap();
    assert_eq!((vals.len(), blanks), (2, 0));
    let (_, _, cut) = value_counts(&ctx, &tb.columns[0], None, 2).unwrap();
    assert!(cut);
}

#[test]
fn sorts_like_excel() {
    let ctx = Context::for_tests();
    let tb = table(
        &ctx,
        vec![
            vec![
                t("b"),
                n(2.0),
                Value::Empty,
                t("A"),
                Value::Bool(true),
                n(-1.0),
                Value::Error(CellError::NA),
                t("a"),
            ],
            vec![
                n(1.0),
                n(2.0),
                n(3.0),
                n(4.0),
                n(5.0),
                n(6.0),
                n(7.0),
                n(8.0),
            ],
        ],
    );
    let asc = sort(
        &ctx,
        &tb,
        &[SortKey {
            col: 0,
            desc: false,
        }],
        None,
    )
    .unwrap();
    // 数値 < 文字列（A と a は同じ順位なら元の順）< 真偽値 < エラー < 空
    assert_eq!(asc, vec![5, 1, 3, 7, 0, 4, 6, 2]);
    let desc = sort(&ctx, &tb, &[SortKey { col: 0, desc: true }], None).unwrap();
    // エラー > 真偽値 > 文字列 > 数値、空は最後
    assert_eq!(desc, vec![6, 4, 0, 7, 3, 1, 5, 2]);
    // 複数のキー: 1 列目の昇順、同じなら 2 列目の降順
    let tb2 = table(
        &ctx,
        vec![
            vec![t("x"), t("y"), t("x"), t("y"), t("x")],
            vec![n(1.0), n(5.0), n(3.0), n(2.0), n(2.0)],
        ],
    );
    let o = sort(
        &ctx,
        &tb2,
        &[
            SortKey {
                col: 0,
                desc: false,
            },
            SortKey { col: 1, desc: true },
        ],
        None,
    )
    .unwrap();
    assert_eq!(o, vec![2, 4, 0, 1, 3]);
    // 一部の行だけ
    let o = sort(
        &ctx,
        &tb2,
        &[SortKey {
            col: 1,
            desc: false,
        }],
        Some(&[1, 3, 4]),
    )
    .unwrap();
    assert_eq!(o, vec![3, 4, 1]);
}

#[test]
fn sort_matches_simple_sort_on_random_data() {
    let ctx = Context::for_tests();
    let mut x: u64 = 12345;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let vals: Vec<Value> = (0..3000)
        .map(|_| match next() % 5 {
            0 => Value::Empty,
            1 => t(&format!("s{}", next() % 50)),
            2 => Value::Bool(next() % 2 == 0),
            _ => n((next() % 1000) as f64 - 500.0),
        })
        .collect();
    let tb = Table {
        columns: Arc::new(vec![col(&ctx, "v", &vals, 700)]),
        rows: vals.len() as u64,
        header: false,
    };
    for desc in [false, true] {
        let got = sort(&ctx, &tb, &[SortKey { col: 0, desc }], None).unwrap();
        let mut want: Vec<u32> = (0..vals.len() as u32).collect();
        want.sort_by(|&a, &b| {
            let (va, vb) = (&vals[a as usize], &vals[b as usize]);
            match (va.is_empty(), vb.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                _ => {
                    let o = cmp_values(va, vb);
                    if desc { o.reverse() } else { o }
                }
            }
        });
        let gv: Vec<&Value> = got.iter().map(|&i| &vals[i as usize]).collect();
        let wv: Vec<&Value> = want.iter().map(|&i| &vals[i as usize]).collect();
        assert_eq!(gv, wv, "desc {desc}");
        assert_eq!(got, want, "安定であること desc {desc}");
    }
}

#[test]
fn view_maps_rows_and_free_cells() {
    let ctx = Context::for_tests();
    let mut s = crate::sheet::Sheet::new("s");
    s.table = sample(&ctx);
    // 表の下（格子の 10 行目）と表の横の自由なセル
    s.set(&ctx, 10, 0, t("下")).unwrap();
    s.set(&ctx, 2, 5, t("横")).unwrap();
    assert_eq!(s.extent(), (11, 6));
    s.view.filters.push(ColFilter {
        col: 1,
        cond: Cond::Number {
            op: Cmp::Ge,
            value: 10.0,
        },
    });
    s.view.sort.push(SortKey { col: 1, desc: true });
    let tb = s.table.clone();
    apply(&ctx, &tb, &mut s.view).unwrap();
    // 10・30・20 の行（表の 0・2・4 行目）を降順に
    assert_eq!(s.view.rows.as_deref().unwrap(), &vec![2, 4, 0]);
    assert_eq!(s.view.counts, vec![3]);
    assert_eq!(s.visible_rows(), 3);
    assert_eq!(s.get(&ctx, 0, 1).unwrap(), t("c1"));
    assert_eq!(s.get(&ctx, 1, 1).unwrap(), n(30.0));
    assert_eq!(s.get(&ctx, 3, 0).unwrap(), t("東京"));
    assert_eq!(s.place(4, 0), crate::sheet::Place::Free);
    // 表の下のセルは隠れた 5 行の分だけ上に見える
    assert_eq!(s.extent(), (6, 6));
    assert_eq!(s.get(&ctx, 5, 0).unwrap(), t("下"));
    // 見えている行への書き込みは元の行へ
    s.set(&ctx, 1, 0, t("書いた")).unwrap();
    assert_eq!(s.table.columns[0].get(&ctx, 2).unwrap(), t("書いた"));
    s.set(&ctx, 5, 1, t("下2")).unwrap();
    assert_eq!(s.cells.get(&(10, 1)), Some(&t("下2")));
    // 解除すれば元どおり
    s.view = crate::sheet::View::default();
    assert_eq!(s.get(&ctx, 10, 1).unwrap(), t("下2"));
    assert_eq!(s.extent(), (11, 6));
}

#[test]
fn view_follows_column_edits() {
    let ctx = Context::for_tests();
    let mut s = crate::sheet::Sheet::new("s");
    s.table = sample(&ctx);
    s.view.filters.push(ColFilter {
        col: 1,
        cond: Cond::NonBlank,
    });
    s.view.sort.push(SortKey {
        col: 0,
        desc: false,
    });
    s.insert_cols(&ctx, 0, 2).unwrap();
    assert_eq!(s.view.filters[0].col, 3);
    assert_eq!(s.view.sort[0].col, 2);
    s.delete_cols(3, 1);
    assert!(s.view.filters.is_empty());
    assert_eq!(s.view.sort[0].col, 2);
}

#[test]
fn permute_rebuilds_columns() {
    let ctx = Context::for_tests();
    let tb = sample(&ctx);
    let order = sort(
        &ctx,
        &tb,
        &[SortKey {
            col: 1,
            desc: false,
        }],
        None,
    )
    .unwrap();
    let p = permute(&ctx, &tb, &order).unwrap();
    assert_eq!(p.rows, tb.rows);
    for (i, &r) in order.iter().enumerate() {
        for c in 0..2 {
            assert_eq!(
                p.columns[c].get(&ctx, i as u64).unwrap(),
                tb.columns[c].get(&ctx, r as u64).unwrap()
            );
        }
    }
    // 並べ替えたあとは元の順のまま並ぶ
    let again = sort(
        &ctx,
        &p,
        &[SortKey {
            col: 1,
            desc: false,
        }],
        None,
    )
    .unwrap();
    assert_eq!(again, (0..tb.rows as u32).collect::<Vec<_>>());
}

#[test]
fn multi_key_sort_matches_naive_stable_sort() {
    let ctx = Context::for_tests();
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move |n: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % n
    };
    let words = ["b", "A", "a", "B", "東京", "大阪", "c"];
    let rows = 3000;
    let cols: Vec<Vec<Value>> = (0..5)
        .map(|c| {
            (0..rows)
                .map(|_| match (c, next(10)) {
                    (_, 0) => Value::Empty,
                    (0 | 3, _) => n(next(7) as f64 - 3.0),
                    (1, k) if k < 8 => t(words[next(words.len() as u64) as usize]),
                    (1, _) => Value::Bool(next(2) == 0),
                    (2, k) if k < 5 => n(next(4) as f64 * 0.5),
                    (2, _) => t(words[next(3) as usize]),
                    _ => n(next(1_000_000) as f64 / 7.0),
                })
                .collect()
        })
        .collect();
    let tb = table(&ctx, cols.clone());
    let cmp = |a: &Value, b: &Value, desc: bool| match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => {
            let o = cmp_values(a, b);
            if desc { o.reverse() } else { o }
        }
    };
    for trial in 0..6 {
        let keys: Vec<SortKey> = (0..5)
            .map(|i| SortKey {
                col: ((i + trial) % 5) as u32,
                desc: next(2) == 0,
            })
            .take(1 + trial % 5)
            .collect();
        let subset: Option<Vec<u32>> =
            (trial % 2 == 1).then(|| (0..rows as u32).filter(|r| r % 3 != 0).collect());
        let got = sort(&ctx, &tb, &keys, subset.as_deref()).unwrap();
        let mut want: Vec<u32> = subset.unwrap_or_else(|| (0..rows as u32).collect());
        want.sort_by(|&a, &b| {
            for k in &keys {
                let col = &cols[k.col as usize];
                let o = cmp(&col[a as usize], &col[b as usize], k.desc);
                if o != Ordering::Equal {
                    return o;
                }
            }
            Ordering::Equal
        });
        assert_eq!(got, want, "keys {keys:?}");
    }
}

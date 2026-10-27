//! Display alignment only. Missing rows never become bytes in a document.
use crate::{Cancellation, Diff, Kind, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Equal,
    Changed,
    LeftOnly,
    RightOnly,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlignedRow {
    pub left: Option<usize>,
    pub right: Option<usize>,
    pub kind: RowKind,
}
/// BASE, LEFT, RIGHT, RESULT. Ordering in each original document is preserved.
#[derive(Clone, Copy, Debug)]
pub struct AncestorRow {
    pub lines: [Option<usize>; 4],
}
const MAX_ANCESTOR_ROWS: usize = 1_000_000;

pub fn ancestor_rows(
    base: &str,
    left: &str,
    right: &str,
    result: Option<&str>,
    cancel: &Cancellation,
) -> Result<Vec<AncestorRow>> {
    cancel.check()?;
    let mut columns = Vec::new();
    for side in [Some(left), Some(right), result].into_iter().flatten() {
        columns.push(align(&crate::compare(base, side, cancel)?, cancel)?);
    }
    let mut positions = [0; 3];
    let count = crate::lines(base).len();
    let mut out = Vec::new();
    for line in 0..=count {
        if line & 1023 == 0 {
            cancel.check()?;
        }
        let mut inserted = [0; 3];
        for (i, column) in columns.iter().enumerate() {
            inserted[i] = column[positions[i]..]
                .iter()
                .take_while(|r| r.left.is_none())
                .count();
        }
        for j in 0..inserted.iter().copied().max().unwrap_or(0) {
            if j & 1023 == 0 {
                cancel.check()?;
            }
            if out.len() >= MAX_ANCESTOR_ROWS {
                return Err("ancestor alignment incomplete: display row limit".into());
            }
            let mut lines = [None; 4];
            for (i, column) in columns.iter().enumerate() {
                if j < inserted[i] {
                    lines[i + 1] = column[positions[i] + j].right;
                }
            }
            out.push(AncestorRow { lines });
        }
        for i in 0..columns.len() {
            positions[i] += inserted[i];
        }
        if line == count {
            break;
        }
        if out.len() >= MAX_ANCESTOR_ROWS {
            return Err("ancestor alignment incomplete: display row limit".into());
        }
        let mut lines = [Some(line), None, None, None];
        for (i, column) in columns.iter().enumerate() {
            let row = &column[positions[i]];
            debug_assert_eq!(row.left, Some(line));
            lines[i + 1] = row.right;
            positions[i] += 1;
        }
        out.push(AncestorRow { lines });
    }
    Ok(out)
}

pub fn align(diff: &Diff, cancel: &Cancellation) -> Result<Vec<AlignedRow>> {
    cancel.check()?;
    let mut out = Vec::with_capacity(diff.rows.len());
    let mut at = 0;
    while at < diff.rows.len() {
        cancel.check()?;
        if diff.rows[at].kind == Kind::Equal {
            let row = &diff.rows[at];
            out.push(AlignedRow {
                left: row.left,
                right: row.right,
                kind: RowKind::Equal,
            });
            at += 1;
        } else {
            let start = at;
            while at < diff.rows.len() && diff.rows[at].kind != Kind::Equal {
                at += 1;
            }
            let left: Vec<_> = diff.rows[start..at].iter().filter_map(|r| r.left).collect();
            let right: Vec<_> = diff.rows[start..at]
                .iter()
                .filter_map(|r| r.right)
                .collect();
            for i in 0..left.len().max(right.len()) {
                if i & 1023 == 0 {
                    cancel.check()?;
                }
                let (left, right) = (left.get(i).copied(), right.get(i).copied());
                let kind = match (left, right) {
                    (Some(_), Some(_)) => RowKind::Changed,
                    (Some(_), None) => RowKind::LeftOnly,
                    (None, Some(_)) => RowKind::RightOnly,
                    _ => unreachable!(),
                };
                out.push(AlignedRow { left, right, kind });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ancestor_alignment_preserves_every_column_with_independent_insertions() {
        for texts in [
            ["a\nb\nc\n", "new\na\nb\nC\n", "a\nB\nc\n", "new\na\nB\nC\n"],
            [
                "",
                "left\n",
                "right\n",
                "<<<<<<<\nleft\n=======\nright\n>>>>>>>\n",
            ],
            ["a\nb\n", "b\n", "a\n", ""],
        ] {
            let rows = ancestor_rows(
                texts[0],
                texts[1],
                texts[2],
                Some(texts[3]),
                &Cancellation::default(),
            )
            .unwrap();
            for (column, text) in texts.iter().enumerate() {
                assert_eq!(
                    rows.iter()
                        .filter_map(|r| r.lines[column])
                        .collect::<Vec<_>>(),
                    (0..crate::lines(text).len()).collect::<Vec<_>>()
                );
            }
        }
    }
    #[test]
    fn pairs_replacements_and_keeps_source_rows_exactly_once() {
        for (a, b) in [
            ("a\nb\nc\nd\n", "a\nB\nX\nc\nd\n"),
            ("", "one\ntwo"),
            ("same\n", "same\n"),
            ("x\ny\n", "z\n"),
        ] {
            let c = Cancellation::default();
            let d = crate::compare(a, b, &c).unwrap();
            let aligned = align(&d, &c).unwrap();
            assert_eq!(
                aligned.iter().filter_map(|r| r.left).collect::<Vec<_>>(),
                (0..crate::lines(a).len()).collect::<Vec<_>>()
            );
            assert_eq!(
                aligned.iter().filter_map(|r| r.right).collect::<Vec<_>>(),
                (0..crate::lines(b).len()).collect::<Vec<_>>()
            );
            for row in aligned.iter().filter(|r| r.kind == RowKind::Equal) {
                assert_eq!(
                    crate::lines(a)[row.left.unwrap()],
                    crate::lines(b)[row.right.unwrap()]
                );
            }
        }
        let c = Cancellation::default();
        let rows = align(
            &crate::compare("a\nb\nc\n", "a\nB\nX\nc\n", &c).unwrap(),
            &c,
        )
        .unwrap();
        assert_eq!(rows[1].kind, RowKind::Changed);
        assert_eq!(rows[2].left, None);
        assert_eq!((rows[3].left, rows[3].right), (Some(2), Some(3)));
    }
}

//! 2 文書の行を左右に揃えるための差分。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Equal,
    Changed,
    LeftOnly,
    RightOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffRow {
    pub left: Option<usize>,
    pub right: Option<usize>,
    pub kind: DiffKind,
}

#[derive(Clone, Copy)]
enum Op {
    Equal(usize, usize),
    Left(usize),
    Right(usize),
}

fn same(a: &str, b: &str) -> bool {
    a.trim_end_matches('\r') == b.trim_end_matches('\r')
}

/// 行単位の比較。小さな範囲は最長共通部分列で合わせ、大きな範囲は近傍の
/// 同一行をアンカーにしてメモリ使用量を抑える。
pub fn compare_lines(left: &[&str], right: &[&str]) -> Vec<DiffRow> {
    let (n, m) = (left.len(), right.len());
    let mut ops = Vec::new();
    if n.saturating_mul(m) <= 4_000_000 {
        let stride = m + 1;
        let mut lcs = vec![0u32; (n + 1) * stride];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * stride + j] = if same(left[i], right[j]) {
                    1 + lcs[(i + 1) * stride + j + 1]
                } else {
                    lcs[(i + 1) * stride + j].max(lcs[i * stride + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && same(left[i], right[j]) {
                ops.push(Op::Equal(i, j));
                i += 1;
                j += 1;
            } else if i < n && (j == m || lcs[(i + 1) * stride + j] >= lcs[i * stride + j + 1]) {
                ops.push(Op::Left(i));
                i += 1;
            } else {
                ops.push(Op::Right(j));
                j += 1;
            }
        }
    } else {
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && same(left[i], right[j]) {
                ops.push(Op::Equal(i, j));
                i += 1;
                j += 1;
                continue;
            }
            // 近くに同一行があれば挿入・削除として揃える。
            let a = (i + 1..(i + 33).min(n)).find(|&k| j < m && same(left[k], right[j]));
            let b = (j + 1..(j + 33).min(m)).find(|&k| i < n && same(left[i], right[k]));
            match (a, b) {
                (Some(k), Some(l)) if k - i <= l - j => {
                    ops.push(Op::Left(i));
                    i += 1;
                }
                (Some(_), Some(_)) | (None, Some(_)) => {
                    ops.push(Op::Right(j));
                    j += 1;
                }
                (Some(_), None) => {
                    ops.push(Op::Left(i));
                    i += 1;
                }
                (None, None) if i < n && j < m => {
                    ops.push(Op::Left(i));
                    ops.push(Op::Right(j));
                    i += 1;
                    j += 1;
                }
                (None, None) if i < n => {
                    ops.push(Op::Left(i));
                    i += 1;
                }
                _ => {
                    ops.push(Op::Right(j));
                    j += 1;
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut deleted = Vec::new();
    let mut inserted = Vec::new();
    let flush = |out: &mut Vec<DiffRow>, deleted: &mut Vec<usize>, inserted: &mut Vec<usize>| {
        let len = deleted.len().max(inserted.len());
        for k in 0..len {
            let l = deleted.get(k).copied();
            let r = inserted.get(k).copied();
            out.push(DiffRow {
                left: l,
                right: r,
                kind: match (l, r) {
                    (Some(_), Some(_)) => DiffKind::Changed,
                    (Some(_), None) => DiffKind::LeftOnly,
                    _ => DiffKind::RightOnly,
                },
            });
        }
        deleted.clear();
        inserted.clear();
    };
    for op in ops {
        match op {
            Op::Equal(i, j) => {
                flush(&mut out, &mut deleted, &mut inserted);
                out.push(DiffRow {
                    left: Some(i),
                    right: Some(j),
                    kind: DiffKind::Equal,
                });
            }
            Op::Left(i) => deleted.push(i),
            Op::Right(j) => inserted.push(j),
        }
    }
    flush(&mut out, &mut deleted, &mut inserted);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_insertions_and_changes() {
        let l = ["a", "old", "c"];
        let r = ["a", "new", "extra", "c"];
        let rows = compare_lines(&l, &r);
        assert_eq!(
            rows.iter().map(|x| x.kind).collect::<Vec<_>>(),
            [
                DiffKind::Equal,
                DiffKind::Changed,
                DiffKind::RightOnly,
                DiffKind::Equal
            ]
        );
        assert_eq!(rows[3].left, Some(2));
        assert_eq!(rows[3].right, Some(3));
    }

    #[test]
    fn ignores_line_ending_style() {
        let rows = compare_lines(&["a\r", "b"], &["a", "b"]);
        assert!(rows.iter().all(|x| x.kind == DiffKind::Equal));
    }
}

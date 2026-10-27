//! Prepare immutable source maps on the comparison worker, not on the UI thread.
use std::ops::Range;
use yy_config::Color;
use yy_merge_core::{
    Cancellation, Diff, Result,
    alignment::{RowKind, align},
};
use yy_ui_win::{
    aligned::{AlignedView, DisplayRow},
    render::RowDecoration,
};

pub struct Prepared {
    pub left: AlignedView,
    pub right: AlignedView,
    pub left_marks: Vec<Range<u64>>,
    pub right_marks: Vec<Range<u64>>,
}
pub struct MergePrepared {
    /// Native order: LEFT, RIGHT, BASE, RESULT.
    pub panes: [Option<AlignedView>; 4],
    pub marks: [Vec<Range<u64>>; 4],
    pub warning: Option<String>,
}
fn merge_marks(marks: &mut Vec<Range<u64>>) {
    marks.sort_by_key(|r| r.start);
    let mut out: Vec<Range<u64>> = Vec::with_capacity(marks.len());
    for range in marks.drain(..) {
        if let Some(last) = out.last_mut()
            && last.end >= range.start
        {
            last.end = last.end.max(range.end);
        } else {
            out.push(range);
        }
    }
    *marks = out;
}
pub fn prepare_merge(
    base: &str,
    left: &str,
    right: &str,
    result: &str,
    cancel: &Cancellation,
) -> Result<MergePrepared> {
    let (aligned, active, warning) =
        match yy_merge_core::alignment::ancestor_rows(base, left, right, Some(result), cancel) {
            Ok(rows) => (rows, 4, None),
            Err(e) => {
                cancel.check()?;
                match yy_merge_core::alignment::ancestor_rows(base, left, right, None, cancel) {
                    Ok(rows) => (
                        rows,
                        3,
                        Some(format!("RESULTの整列は未完了（編集・保存は継続可能）: {e}")),
                    ),
                    Err(e) => {
                        cancel.check()?;
                        return Ok(MergePrepared {
                            panes: std::array::from_fn(|_| None),
                            marks: std::array::from_fn(|_| Vec::new()),
                            warning: Some(format!("整列は未完了（各文書を個別表示）: {e}")),
                        });
                    }
                }
            }
        };
    let texts = [base, left, right, result];
    let starts: [Vec<usize>; 4] = std::array::from_fn(|i| {
        if i < active {
            offsets(texts[i])
        } else {
            Vec::new()
        }
    });
    let lines: [Vec<&str>; 4] = std::array::from_fn(|i| {
        if i < active {
            yy_merge_core::lines(texts[i])
        } else {
            Vec::new()
        }
    });
    let mut maps: [Vec<DisplayRow>; 4] = std::array::from_fn(|_| Vec::with_capacity(aligned.len()));
    let mut marks: [Vec<Range<u64>>; 4] = std::array::from_fn(|_| Vec::new());
    for (index, row) in aligned.iter().enumerate() {
        if index & 1023 == 0 {
            cancel.check()?;
        }
        let value = |i: usize| row.lines[i].map(|line| lines[i][line]).unwrap_or("");
        let changed = (1..active).any(|i| value(i) != value(0));
        let chunks: [Vec<u64>; 4] = std::array::from_fn(|i| {
            row.lines[i]
                .map(|line| {
                    segments(
                        texts[i],
                        starts[i][line],
                        starts[i].get(line + 1).copied().unwrap_or(texts[i].len()),
                    )
                })
                .unwrap_or_default()
        });
        for segment in 0..chunks[..active].iter().map(Vec::len).max().unwrap_or(0) {
            for i in 0..active {
                let kind = if i == 0 {
                    if changed {
                        RowKind::Changed
                    } else {
                        RowKind::Equal
                    }
                } else if row.lines[0].is_none() {
                    RowKind::RightOnly
                } else if value(i) != value(0) {
                    RowKind::Changed
                } else {
                    RowKind::Equal
                };
                append(
                    &mut maps[i],
                    chunks[i].get(segment).copied(),
                    if segment == 0 {
                        row.lines[i].map(|n| n as u64 + 1)
                    } else {
                        None
                    },
                    kind,
                );
            }
        }
        for i in 1..active {
            let (a, b) = yy_merge_core::changed_span(value(0), value(i));
            if !a.is_empty()
                && let Some(line) = row.lines[0]
            {
                marks[0].push((starts[0][line] + a.start) as u64..(starts[0][line] + a.end) as u64);
            }
            if !b.is_empty()
                && let Some(line) = row.lines[i]
            {
                marks[i].push((starts[i][line] + b.start) as u64..(starts[i][line] + b.end) as u64);
            }
        }
    }
    if texts[..active]
        .iter()
        .any(|s| s.is_empty() || s.ends_with('\n'))
    {
        for i in 0..active {
            let eof = texts[i].is_empty() || texts[i].ends_with('\n');
            append(
                &mut maps[i],
                eof.then_some(texts[i].len() as u64),
                eof.then_some(starts[i].len() as u64),
                RowKind::Equal,
            );
        }
    }
    for ranges in &mut marks {
        merge_marks(ranges);
    }
    let mut views = maps
        .into_iter()
        .enumerate()
        .map(|(i, rows)| {
            if i < active {
                let mut view = AlignedView::new(rows);
                view.pad_bottom = true;
                Some(view)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let mut ranges = marks.into_iter().collect::<Vec<_>>();
    let result = views.pop().unwrap();
    let base = views.remove(0);
    let left = views.remove(0);
    let right = views.remove(0);
    let result_marks = ranges.pop().unwrap();
    let base_marks = ranges.remove(0);
    let left_marks = ranges.remove(0);
    let right_marks = ranges.remove(0);
    Ok(MergePrepared {
        panes: [left, right, base, result],
        marks: [left_marks, right_marks, base_marks, result_marks],
        warning,
    })
}
fn offsets(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(
            text.bytes()
                .enumerate()
                .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
        )
        .collect()
}
fn segments(text: &str, start: usize, end: usize) -> Vec<u64> {
    let mut out = vec![start as u64];
    let mut at = start;
    let size = yy_layout::RowConfig::default().max_row_bytes as usize;
    loop {
        let mut next = if at == start {
            (at + size).div_ceil(size) * size
        } else {
            (at / size + 1) * size
        };
        if next >= end {
            break;
        }
        while next < text.len() && !text.is_char_boundary(next) {
            next += 1;
        }
        if next >= end {
            break;
        }
        out.push(next as u64);
        at = next;
    }
    out
}
fn row_color(kind: RowKind, source: bool) -> Option<Color> {
    if !source {
        return Some(Color::rgb(238, 240, 243));
    }
    match kind {
        RowKind::Equal => None,
        RowKind::Changed => Some(Color::rgb(255, 248, 222)),
        RowKind::LeftOnly => Some(Color::rgb(255, 232, 232)),
        RowKind::RightOnly => Some(Color::rgb(229, 247, 233)),
    }
}
fn append(out: &mut Vec<DisplayRow>, source: Option<u64>, number: Option<u64>, kind: RowKind) {
    out.push(DisplayRow {
        source,
        decoration: RowDecoration {
            line_number: number,
            background: row_color(kind, source.is_some()),
        },
    });
}
pub fn prepare(left: &str, right: &str, diff: &Diff, cancel: &Cancellation) -> Result<Prepared> {
    let aligned = align(diff, cancel)?;
    let ls = offsets(left);
    let rs = offsets(right);
    let ll = yy_merge_core::lines(left);
    let rr = yy_merge_core::lines(right);
    let mut l = Vec::with_capacity(aligned.len());
    let mut r = Vec::with_capacity(aligned.len());
    let mut lm = Vec::new();
    let mut rm = Vec::new();
    for (i, row) in aligned.iter().enumerate() {
        if i & 1023 == 0 {
            cancel.check()?;
        }
        let la = row
            .left
            .map(|n| segments(left, ls[n], ls.get(n + 1).copied().unwrap_or(left.len())))
            .unwrap_or_default();
        let ra = row
            .right
            .map(|n| segments(right, rs[n], rs.get(n + 1).copied().unwrap_or(right.len())))
            .unwrap_or_default();
        let ignored = row.kind == RowKind::Equal
            && row.left.zip(row.right).is_some_and(|(a, b)| ll[a] != rr[b]);
        for index in 0..la.len().max(ra.len()) {
            append(
                &mut l,
                la.get(index).copied(),
                if index == 0 {
                    row.left.map(|n| n as u64 + 1)
                } else {
                    None
                },
                row.kind,
            );
            append(
                &mut r,
                ra.get(index).copied(),
                if index == 0 {
                    row.right.map(|n| n as u64 + 1)
                } else {
                    None
                },
                row.kind,
            );
            if ignored {
                for rows in [&mut l, &mut r] {
                    if let Some(last) = rows.last_mut()
                        && last.source.is_some()
                    {
                        last.decoration.background = Some(Color::rgb(232, 241, 253));
                    }
                }
            }
        }
        if row.kind != RowKind::Equal {
            let a = row.left.map(|n| ll[n]).unwrap_or("");
            let b = row.right.map(|n| rr[n]).unwrap_or("");
            let (a, b) = yy_merge_core::changed_span(a, b);
            if !a.is_empty()
                && let Some(n) = row.left
            {
                lm.push((ls[n] + a.start) as u64..(ls[n] + a.end) as u64);
            }
            if !b.is_empty()
                && let Some(n) = row.right
            {
                rm.push((rs[n] + b.start) as u64..(rs[n] + b.end) as u64);
            }
        }
    }
    // A trailing empty editor line is a view convention, not an additional diff token.
    let le = left.is_empty() || left.ends_with('\n');
    let re = right.is_empty() || right.ends_with('\n');
    if le || re {
        append(
            &mut l,
            le.then_some(left.len() as u64),
            le.then_some(ls.len() as u64),
            RowKind::Equal,
        );
        append(
            &mut r,
            re.then_some(right.len() as u64),
            re.then_some(rs.len() as u64),
            RowKind::Equal,
        );
    }
    Ok(Prepared {
        left: AlignedView::new(l),
        right: AlignedView::new(r),
        left_marks: lm,
        right_marks: rm,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn four_pane_projection_keeps_each_original_row_and_merged_result() {
        let base = "header\nbase\nend\n";
        let left = "left-added\nheader\nLEFT\nend\n";
        let right = "header\nRIGHT\nend\nright-added\n";
        let c = Cancellation::default();
        let merged = yy_merge_core::merge(base, left, right, &c).unwrap();
        let result = merged.render(true).unwrap();
        let p = prepare_merge(base, left, right, &result, &c).unwrap();
        let count = p.panes[0].as_ref().unwrap().rows.len();
        for (i, text) in [left, right, base, result.as_str()].into_iter().enumerate() {
            let view = p.panes[i].as_ref().unwrap();
            assert_eq!(view.rows.len(), count);
            assert!(view.pad_bottom);
            let document = yy_core::Document::from_text(text);
            let expected = yy_layout::rows_from(
                document.snapshot(),
                &yy_layout::RowConfig::default(),
                0,
                100,
            )
            .iter()
            .map(|r| r.start)
            .collect::<Vec<_>>();
            assert_eq!(
                view.rows
                    .iter()
                    .filter_map(|r| r.source)
                    .collect::<Vec<_>>(),
                expected
            );
            assert!(p.marks[i].windows(2).all(|r| r[0].end < r[1].start));
        }
    }
    #[test]
    fn result_projection_budget_failure_preserves_three_source_panes() {
        let result = (0..2500)
            .map(|i| format!("different{i}\n"))
            .collect::<String>();
        let p = prepare_merge(
            "base\n",
            "base\n",
            "base\n",
            &result,
            &Cancellation::default(),
        )
        .unwrap();
        assert!(p.warning.as_ref().unwrap().contains("未完了"));
        assert!(p.panes[..3].iter().all(Option::is_some));
        assert!(p.panes[3].is_none());
        assert_eq!(result.lines().count(), 2500);
        let p = prepare_merge(
            "base\n",
            &result,
            "base\n",
            &result,
            &Cancellation::default(),
        )
        .unwrap();
        assert!(p.panes.iter().all(Option::is_none));
        assert!(p.warning.is_some());
    }
    #[test]
    fn aligned_slots_keep_original_offsets_and_unicode_marks() {
        let a = "共通\nvalue=日本\nend\n";
        let b = "共通\nvalue=日本語\nadded\nend\n";
        let c = Cancellation::default();
        let diff = yy_merge_core::compare(a, b, &c).unwrap();
        let p = prepare(a, b, &diff, &c).unwrap();
        assert_eq!(p.left.rows.len(), p.right.rows.len());
        assert!(p.left.rows[2].source.is_none());
        assert_eq!(p.left.rows[3].decoration.line_number, Some(3));
        assert_eq!(p.right.rows[3].decoration.line_number, Some(4));
        for mark in p.left_marks {
            assert!(
                a.is_char_boundary(mark.start as usize) && a.is_char_boundary(mark.end as usize)
            );
        }
        assert!(
            p.right_marks
                .iter()
                .any(|r| &b[r.start as usize..r.end as usize] == "語")
        );
    }
    #[test]
    fn long_lines_are_split_at_layout_boundaries_without_losing_bytes() {
        let a = "日".repeat(10000) + "\nend\n";
        let b = "short\nend\n";
        let c = Cancellation::default();
        let d = yy_merge_core::compare(&a, b, &c).unwrap();
        let p = prepare(&a, b, &d, &c).unwrap();
        assert_eq!(p.left.rows.len(), p.right.rows.len());
        let offsets = p
            .left
            .rows
            .iter()
            .filter_map(|r| r.source)
            .collect::<Vec<_>>();
        assert!(offsets.len() > 4);
        for offset in offsets {
            assert!(a.is_char_boundary(offset as usize));
        }
        let document = yy_core::Document::from_text(&a);
        let expected = yy_layout::rows_from(
            document.snapshot(),
            &yy_layout::RowConfig::default(),
            0,
            100,
        )
        .iter()
        .map(|r| r.start)
        .collect::<Vec<_>>();
        assert_eq!(
            p.left
                .rows
                .iter()
                .filter_map(|r| r.source)
                .collect::<Vec<_>>(),
            expected
        );
        assert!(p.right.rows[1].source.is_none());
    }
}

#[cfg(test)]
mod rule_display_tests {
    use super::*;
    #[test]
    fn normalized_equal_rows_keep_original_offsets_and_visible_ignored_decoration() {
        let left = "ALPHA \t\r\nkeep\n";
        let right = "alpha\nkeep\n";
        let c = Cancellation::new();
        let result = yy_merge_core::rules::compare(
            left,
            right,
            yy_merge_core::rules::Rules {
                ignore_case: true,
                ignore_trailing_space: true,
                ignore_eol: true,
                ..Default::default()
            },
            &c,
        )
        .unwrap();
        let p = prepare(left, right, &result.diff, &c).unwrap();
        assert_eq!(p.left.rows[0].source, Some(0));
        assert_eq!(p.right.rows[0].source, Some(0));
        assert_eq!(p.left.rows[1].source, Some(9));
        assert_eq!(p.right.rows[1].source, Some(6));
        assert_eq!(
            p.left.rows[0].decoration.background,
            Some(Color::rgb(232, 241, 253))
        );
        assert_eq!(p.left.rows[1].decoration.background, None);
        assert!(p.left_marks.is_empty());
        assert!(p.right_marks.is_empty());
    }
}

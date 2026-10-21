//! Exact, bounded comparisons. Neither heuristics nor normalized text feed auto-merge.
pub mod files;
pub mod folders;

use serde::Serialize;
use std::{
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

pub type Result<T> = std::result::Result<T, String>;
pub const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_LINES: usize = 1_000_000;
const MAX_TRACE: usize = 4_000_000;
const MAX_WORK: usize = 50_000_000;

#[derive(Default)]
pub struct Cancellation(AtomicBool);
impl Cancellation {
    pub const fn new() -> Self {
        Self(AtomicBool::new(false))
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn check(&self) -> Result<()> {
        if self.0.load(Ordering::Relaxed) {
            Err("cancelled".into())
        } else {
            Ok(())
        }
    }
}

/// Terminators are part of each token, including a missing final newline.
pub fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Kind {
    Equal,
    Delete,
    Insert,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Row {
    pub kind: Kind,
    pub left: Option<usize>,
    pub right: Option<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Edit {
    pub base: Range<usize>,
    pub replacement: Range<usize>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Diff {
    pub rows: Vec<Row>,
    pub edits: Vec<Edit>,
}

fn diagonal(v: &[isize], depth: usize, k: isize) -> isize {
    let i = k + depth as isize;
    if i < 0 || i as usize >= v.len() {
        -1
    } else {
        v[i as usize]
    }
}

/// Myers' shortest edit script with a bounded trace and explicit work budget.
pub fn compare(left: &str, right: &str, cancel: &Cancellation) -> Result<Diff> {
    cancel.check()?;
    if left.len() as u64 > MAX_BYTES || right.len() as u64 > MAX_BYTES {
        return Err("comparison incomplete: 32 MiB per text limit".into());
    }
    let a = lines(left);
    let b = lines(right);
    if a.len() + b.len() > MAX_LINES {
        return Err("comparison incomplete: line limit".into());
    }
    let mut prefix = 0;
    while prefix < a.len().min(b.len()) && a[prefix] == b[prefix] {
        if prefix & 1023 == 0 {
            cancel.check()?;
        }
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < (a.len() - prefix).min(b.len() - prefix)
        && a[a.len() - suffix - 1] == b[b.len() - suffix - 1]
    {
        if suffix & 1023 == 0 {
            cancel.check()?;
        }
        suffix += 1;
    }
    let n = a.len() - prefix - suffix;
    let m = b.len() - prefix - suffix;
    let mut middle = Vec::new();
    if n == 0 {
        middle.resize(m, Kind::Insert);
    } else if m == 0 {
        middle.resize(n, Kind::Delete);
    } else {
        let mut trace: Vec<Vec<isize>> = Vec::new();
        let mut cells = 0;
        let mut work = 0;
        'search: for d in 0..=n + m {
            cancel.check()?;
            cells += 2 * d + 1;
            if cells > MAX_TRACE {
                return Err("comparison incomplete: exact diff trace limit".into());
            }
            let mut v = vec![-1; 2 * d + 1];
            for k in (-(d as isize)..=d as isize).step_by(2) {
                let mut x = if d == 0 {
                    0
                } else {
                    let prev = &trace[d - 1];
                    if k == -(d as isize)
                        || (k != d as isize
                            && diagonal(prev, d - 1, k - 1) < diagonal(prev, d - 1, k + 1))
                    {
                        diagonal(prev, d - 1, k + 1)
                    } else {
                        diagonal(prev, d - 1, k - 1) + 1
                    }
                };
                let mut y = x - k;
                while x < n as isize
                    && y < m as isize
                    && x >= 0
                    && y >= 0
                    && a[prefix + x as usize] == b[prefix + y as usize]
                {
                    x += 1;
                    y += 1;
                    work += 1;
                    if work & 1023 == 0 {
                        cancel.check()?;
                    }
                    if work > MAX_WORK {
                        return Err("comparison incomplete: work limit".into());
                    }
                }
                work += 1;
                v[(k + d as isize) as usize] = x;
                if x >= n as isize && y >= m as isize {
                    trace.push(v);
                    let (mut x, mut y) = (n as isize, m as isize);
                    for depth in (1..=d).rev() {
                        let k = x - y;
                        let prev = &trace[depth - 1];
                        let pk = if k == -(depth as isize)
                            || (k != depth as isize
                                && diagonal(prev, depth - 1, k - 1)
                                    < diagonal(prev, depth - 1, k + 1))
                        {
                            k + 1
                        } else {
                            k - 1
                        };
                        let px = diagonal(prev, depth - 1, pk);
                        let py = px - pk;
                        while x > px && y > py {
                            middle.push(Kind::Equal);
                            x -= 1;
                            y -= 1;
                        }
                        if x == px {
                            middle.push(Kind::Insert);
                            y -= 1;
                        } else {
                            middle.push(Kind::Delete);
                            x -= 1;
                        }
                    }
                    while x > 0 && y > 0 {
                        middle.push(Kind::Equal);
                        x -= 1;
                        y -= 1;
                    }
                    middle.reverse();
                    break 'search;
                }
            }
            trace.push(v);
        }
    }
    let mut rows = Vec::with_capacity(prefix + middle.len() + suffix);
    let (mut x, mut y) = (0, 0);
    for kind in std::iter::repeat_n(Kind::Equal, prefix)
        .chain(middle)
        .chain(std::iter::repeat_n(Kind::Equal, suffix))
    {
        let left = (kind != Kind::Insert).then_some(x);
        let right = (kind != Kind::Delete).then_some(y);
        if left.is_some() {
            x += 1;
        }
        if right.is_some() {
            y += 1;
        }
        rows.push(Row { kind, left, right });
    }
    let mut edits = Vec::new();
    let (mut x, mut y) = (0, 0);
    let mut start = None;
    for row in &rows {
        if row.kind == Kind::Equal {
            if let Some((sx, sy)) = start.take() {
                edits.push(Edit {
                    base: sx..x,
                    replacement: sy..y,
                });
            }
        } else if start.is_none() {
            start = Some((x, y));
        }
        x += usize::from(row.left.is_some());
        y += usize::from(row.right.is_some());
    }
    if let Some((sx, sy)) = start {
        edits.push(Edit {
            base: sx..x,
            replacement: sy..y,
        });
    }
    Ok(Diff { rows, edits })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Choice {
    Left,
    Right,
    Base,
    Both,
    Manual(String),
}
#[derive(Clone, Debug, Serialize)]
pub struct Conflict {
    pub id: usize,
    pub base_lines: Range<usize>,
    pub left_lines: Range<usize>,
    pub right_lines: Range<usize>,
    pub base: String,
    pub left: String,
    pub right: String,
    pub resolution: Option<Choice>,
}
#[derive(Clone, Debug, Serialize)]
pub enum Part {
    Text(String),
    Conflict(Conflict),
}
#[derive(Clone, Debug, Serialize)]
pub struct Merge {
    pub parts: Vec<Part>,
}
pub type ConflictRanges = Vec<(usize, Range<usize>)>;
impl Merge {
    pub fn unresolved(&self) -> usize {
        self.parts
            .iter()
            .filter(|p| matches!(p,Part::Conflict(c) if c.resolution.is_none()))
            .count()
    }
    pub fn resolve(&mut self, id: usize, choice: Option<Choice>) -> Result<()> {
        for part in &mut self.parts {
            if let Part::Conflict(c) = part
                && c.id == id
            {
                c.resolution = choice;
                return Ok(());
            }
        }
        Err("unknown conflict".into())
    }
    pub fn render(&self, markers: bool) -> Result<String> {
        self.render_indexed(markers).map(|(text, _)| text)
    }
    /// Result byte ranges for conflict navigation, independent of marker content.
    pub fn render_indexed(&self, markers: bool) -> Result<(String, ConflictRanges)> {
        let mut out = String::new();
        let mut ranges = Vec::new();
        for part in &self.parts {
            let start = out.len();
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Conflict(c) => match &c.resolution {
                    Some(Choice::Left) => out.push_str(&c.left),
                    Some(Choice::Right) => out.push_str(&c.right),
                    Some(Choice::Base) => out.push_str(&c.base),
                    Some(Choice::Both) => {
                        out.push_str(&c.left);
                        out.push_str(&c.right);
                    }
                    Some(Choice::Manual(t)) => out.push_str(t),
                    None if markers => {
                        if !out.is_empty() && !out.ends_with('\n') {
                            out.push('\n');
                        }
                        out.push_str("<<<<<<< LEFT\n");
                        marker_text(&mut out, &c.left);
                        out.push_str("||||||| BASE\n");
                        marker_text(&mut out, &c.base);
                        out.push_str("=======\n");
                        marker_text(&mut out, &c.right);
                        out.push_str(">>>>>>> RIGHT\n");
                    }
                    None => return Err("unresolved merge conflicts".into()),
                },
            }
            if let Part::Conflict(c) = part {
                ranges.push((c.id, start..out.len()));
            }
        }
        Ok((out, ranges))
    }
}
fn marker_text(out: &mut String, text: &str) {
    out.push_str(text);
    if !text.is_empty() && !text.ends_with('\n') {
        out.push('\n');
    }
}
fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    if a.is_empty() || b.is_empty() {
        a.start <= b.end && b.start <= a.end
    } else {
        a.start < b.end && b.start < a.end
    }
}
fn apply(base: &[&str], side: &[&str], range: Range<usize>, edits: &[&Edit]) -> String {
    let mut out = String::new();
    let mut at = range.start;
    for edit in edits {
        out.push_str(&base[at..edit.base.start].concat());
        out.push_str(&side[edit.replacement.clone()].concat());
        at = edit.base.end;
    }
    out.push_str(&base[at..range.end].concat());
    out
}
fn project_start(line: usize, edits: &[Edit]) -> usize {
    let mut at = line as isize;
    for edit in edits {
        if edit.base.end < line || edit.base.end == line && !edit.base.is_empty() {
            at += edit.replacement.len() as isize - edit.base.len() as isize;
        } else {
            break;
        }
    }
    at.max(0) as usize
}
pub fn merge(base: &str, left: &str, right: &str, cancel: &Cancellation) -> Result<Merge> {
    let dl = compare(base, left, cancel)?;
    let dr = compare(base, right, cancel)?;
    let (b, l, r) = (lines(base), lines(left), lines(right));
    let mut edits: Vec<_> = dl
        .edits
        .iter()
        .map(|e| (false, e))
        .chain(dr.edits.iter().map(|e| (true, e)))
        .collect();
    edits.sort_by_key(|(_, e)| (e.base.start, e.base.end));
    let mut parts = Vec::new();
    let mut at = 0;
    let mut id = 0;
    let mut i = 0;
    while i < edits.len() {
        cancel.check()?;
        let mut range = edits[i].1.base.clone();
        let start = i;
        i += 1;
        while i < edits.len() && overlaps(&range, &edits[i].1.base) {
            range.end = range.end.max(edits[i].1.base.end);
            i += 1;
        }
        parts.push(Part::Text(b[at..range.start].concat()));
        let le: Vec<_> = edits[start..i]
            .iter()
            .filter(|(right, _)| !right)
            .map(|(_, e)| *e)
            .collect();
        let re: Vec<_> = edits[start..i]
            .iter()
            .filter(|(right, _)| *right)
            .map(|(_, e)| *e)
            .collect();
        let lt = apply(&b, &l, range.clone(), &le);
        let rt = apply(&b, &r, range.clone(), &re);
        if le.is_empty() {
            parts.push(Part::Text(rt));
        } else if re.is_empty() || lt == rt {
            parts.push(Part::Text(lt));
        } else {
            id += 1;
            parts.push(Part::Conflict(Conflict {
                id,
                base_lines: range.clone(),
                left_lines: {
                    let start = project_start(range.start, &dl.edits);
                    start..start + lines(&lt).len()
                },
                right_lines: {
                    let start = project_start(range.start, &dr.edits);
                    start..start + lines(&rt).len()
                },
                base: b[range.clone()].concat(),
                left: lt,
                right: rt,
                resolution: None,
            }));
        }
        at = range.end;
    }
    parts.push(Part::Text(b[at..].concat()));
    Ok(Merge { parts })
}

/// A Unicode scalar range shared at the beginning and end; never splits UTF-8.
pub fn changed_span(a: &str, b: &str) -> (Range<usize>, Range<usize>) {
    let prefix = a
        .chars()
        .zip(b.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    let suffix = a[prefix..]
        .chars()
        .rev()
        .zip(b[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    (prefix..a.len() - suffix, prefix..b.len() - suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c() -> Cancellation {
        Cancellation::default()
    }
    #[test]
    fn merge_independent_and_identical() {
        assert_eq!(
            merge("a\nb\nc\n", "A\nb\nc\n", "a\nb\nC\n", &c())
                .unwrap()
                .render(false)
                .unwrap(),
            "A\nb\nC\n"
        );
        assert_eq!(
            merge("a\n", "b\n", "b\n", &c())
                .unwrap()
                .render(false)
                .unwrap(),
            "b\n"
        );
    }
    #[test]
    fn conflicts_have_explicit_resolution() {
        for (b, l, r) in [("", "a", "b"), ("a\n", "", "b\n"), ("a\n", "x\n", "y\n")] {
            let mut m = merge(b, l, r, &c()).unwrap();
            assert_eq!(m.unresolved(), 1);
            assert!(m.render(false).is_err());
            m.resolve(1, Some(Choice::Right)).unwrap();
            assert_eq!(m.render(false).unwrap(), r);
            m.resolve(1, None).unwrap();
            assert_eq!(m.unresolved(), 1);
        }
    }
    #[test]
    fn adjacent_edits_are_independent() {
        assert_eq!(
            merge("a\nb\n", "A\nb\n", "a\nB\n", &c())
                .unwrap()
                .render(false)
                .unwrap(),
            "A\nB\n"
        );
    }
    #[test]
    fn exact_newlines_and_unicode() {
        for (a, b) in [
            ("a\r\n", "a\n"),
            ("a", "a\n"),
            ("日本\n猫\n", "日本\n犬\n"),
            ("", "x"),
            ("x", ""),
        ] {
            let diff = compare(a, b, &c()).unwrap();
            assert!(!diff.edits.is_empty());
            let aa = lines(a);
            let bb = lines(b);
            let mut pos = 0;
            let mut out = String::new();
            for e in diff.edits {
                out.push_str(&aa[pos..e.base.start].concat());
                out.push_str(&bb[e.replacement].concat());
                pos = e.base.end;
            }
            out.push_str(&aa[pos..].concat());
            assert_eq!(out, b);
        }
        assert_eq!(changed_span("日本語", "日本人"), (6..9, 6..9));
    }
    #[test]
    fn exhaustive_small_scripts_reconstruct_and_are_minimal() {
        let texts: Vec<String> = (0..64)
            .map(|n| {
                (0..6)
                    .filter(|i| n & (1 << i) != 0)
                    .map(|i| if i & 1 == 0 { "a\n" } else { "b\n" })
                    .collect()
            })
            .collect();
        for a in &texts {
            for b in &texts {
                let aa = lines(a);
                let bb = lines(b);
                let d = compare(a, b, &c()).unwrap();
                let mut table = vec![vec![0; bb.len() + 1]; aa.len() + 1];
                for i in 0..aa.len() {
                    for j in 0..bb.len() {
                        table[i + 1][j + 1] = if aa[i] == bb[j] {
                            table[i][j] + 1
                        } else {
                            table[i][j + 1].max(table[i + 1][j])
                        };
                    }
                }
                assert_eq!(
                    d.rows.iter().filter(|r| r.kind != Kind::Equal).count(),
                    aa.len() + bb.len() - 2 * table[aa.len()][bb.len()]
                );
                assert_eq!(
                    d.rows
                        .iter()
                        .filter_map(|r| r.left.map(|i| aa[i]))
                        .collect::<String>(),
                    *a
                );
                assert_eq!(
                    d.rows
                        .iter()
                        .filter_map(|r| r.right.map(|i| bb[i]))
                        .collect::<String>(),
                    *b
                );
                assert_eq!(merge(a, a, b, &c()).unwrap().render(false).unwrap(), *b);
            }
        }
    }
    #[test]
    fn conflict_indices_track_sources_and_result() {
        let m = merge("a\nb\nc\n", "new\na\nB\nc\n", "a\nOTHER\nc\n", &c()).unwrap();
        let conflict = m
            .parts
            .iter()
            .find_map(|p| {
                if let Part::Conflict(c) = p {
                    Some(c)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(conflict.base_lines, 1..2);
        assert_eq!(conflict.left_lines, 2..3);
        assert_eq!(conflict.right_lines, 1..2);
        let (text, index) = m.render_indexed(true).unwrap();
        assert_eq!(index.len(), 1);
        assert!(text[index[0].1.clone()].contains("OTHER"));
    }
    #[test]
    fn cancellation_and_budget_are_errors() {
        let token = c();
        token.cancel();
        assert!(compare("a", "b", &token).is_err());
        let a = (0..2500).map(|i| format!("a{i}\n")).collect::<String>();
        let b = a.replace('a', "b");
        assert!(compare(&a, &b, &c()).unwrap_err().contains("incomplete"));
    }
}

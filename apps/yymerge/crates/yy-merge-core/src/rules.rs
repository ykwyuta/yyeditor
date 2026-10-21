//! Display-only comparison rules. Keys never replace source text or feed merge/sync.
use crate::{Cancellation, Diff, Kind, MAX_BYTES, MAX_LINES, Result};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    pub ignore_space: bool,
    pub ignore_trailing_space: bool,
    pub ignore_case: bool,
    pub ignore_eol: bool,
}
impl Rules {
    pub fn active(self) -> bool {
        self != Self::default()
    }
    pub fn label(self) -> String {
        let mut labels = Vec::new();
        if self.ignore_space {
            labels.push("空白・タブ無視")
        } else if self.ignore_trailing_space {
            labels.push("行末空白無視")
        }
        if self.ignore_case {
            labels.push("大小文字無視")
        }
        if self.ignore_eol {
            labels.push("LF/CRLF無視")
        }
        if labels.is_empty() {
            "厳密比較".into()
        } else {
            labels.join(" / ")
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct IgnoredLine {
    /// Original zero-based line indices. Original bytes remain independently available.
    pub left: usize,
    pub right: usize,
}
pub struct Comparison {
    pub diff: Diff,
    pub ignored: Vec<IgnoredLine>,
}
fn keys(lines: &[&str], rules: Rules, cancel: &Cancellation, limit: usize) -> Result<Vec<String>> {
    let mut used = 0;
    let mut out = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if i & 1023 == 0 {
            cancel.check()?;
        }
        let (body, eol) = if let Some(body) = line.strip_suffix("\r\n") {
            (body, if rules.ignore_eol { "\n" } else { "\r\n" })
        } else if let Some(body) = line.strip_suffix('\n') {
            (body, "\n")
        } else {
            (*line, "")
        };
        let body = if rules.ignore_trailing_space {
            body.trim_end_matches([' ', '\t'])
        } else {
            body
        };
        let mut key = String::with_capacity(body.len().min(4096));
        let mut push = |c: char| -> Result<()> {
            if used + key.len() + c.len_utf8() > limit {
                return Err("comparison incomplete: normalized keys exceed byte budget".into());
            }
            key.push(c);
            Ok(())
        };
        for (j, c) in body.chars().enumerate() {
            if j & 4095 == 0 {
                cancel.check()?;
            }
            if rules.ignore_space && matches!(c, ' ' | '\t') {
                continue;
            }
            if rules.ignore_case {
                for lower in c.to_lowercase() {
                    push(lower)?;
                }
            } else {
                push(c)?;
            }
        }
        for c in eol.chars() {
            push(c)?;
        }
        used += key.len();
        out.push(key);
    }
    Ok(out)
}
pub fn compare(left: &str, right: &str, rules: Rules, cancel: &Cancellation) -> Result<Comparison> {
    cancel.check()?;
    if !rules.active() {
        return Ok(Comparison {
            diff: crate::compare(left, right, cancel)?,
            ignored: Vec::new(),
        });
    }
    if left.len() as u64 > MAX_BYTES || right.len() as u64 > MAX_BYTES {
        return Err("comparison incomplete: 32 MiB per text limit".into());
    }
    let a = crate::lines(left);
    let b = crate::lines(right);
    if a.len() + b.len() > MAX_LINES {
        return Err("comparison incomplete: line limit".into());
    }
    let ak = keys(&a, rules, cancel, MAX_BYTES as usize)?;
    let bk = keys(&b, rules, cancel, MAX_BYTES as usize)?;
    let aa: Vec<&str> = ak.iter().map(String::as_str).collect();
    let bb: Vec<&str> = bk.iter().map(String::as_str).collect();
    let diff = crate::compare_tokens(&aa, &bb, cancel)?;
    let mut ignored = Vec::new();
    for (i, row) in diff.rows.iter().enumerate() {
        if i & 1023 == 0 {
            cancel.check()?;
        }
        if row.kind == Kind::Equal {
            let (l, r) = (
                row.left.ok_or("missing rule comparison row")?,
                row.right.ok_or("missing rule comparison row")?,
            );
            if a[l] != b[r] {
                ignored.push(IgnoredLine { left: l, right: r });
            }
        }
    }
    Ok(Comparison { diff, ignored })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rules_preserve_original_indices_and_distinguish_real_changes_from_ignored_pairs() {
        let a = "Alpha  \r\nkeep\r\nold\r\n";
        let b = "alpha\ninserted\nkeep\nnew\n";
        let result = compare(
            a,
            b,
            Rules {
                ignore_case: true,
                ignore_trailing_space: true,
                ignore_eol: true,
                ..Rules::default()
            },
            &Cancellation::new(),
        )
        .unwrap();
        assert_eq!(
            result.ignored,
            vec![
                IgnoredLine { left: 0, right: 0 },
                IgnoredLine { left: 1, right: 2 }
            ]
        );
        assert!(!result.diff.edits.is_empty());
        let mut left = Vec::new();
        let mut right = Vec::new();
        for row in result.diff.rows {
            if let Some(l) = row.left {
                left.push(l)
            }
            if let Some(r) = row.right {
                right.push(r)
            }
        }
        assert_eq!(left, vec![0, 1, 2]);
        assert_eq!(right, vec![0, 1, 2, 3]);
        assert!(
            !crate::compare(a, b, &Cancellation::new())
                .unwrap()
                .edits
                .is_empty()
        );
    }
    #[test]
    fn whitespace_newline_presence_unicode_and_case_boundaries_are_explicit() {
        let c = Cancellation::new();
        for (a, b, rules, equal) in [
            (
                "a \tb\n",
                "ab\n",
                Rules {
                    ignore_space: true,
                    ..Rules::default()
                },
                true,
            ),
            (
                "a b\n",
                "ab\n",
                Rules {
                    ignore_trailing_space: true,
                    ..Rules::default()
                },
                false,
            ),
            (
                "a　b\n",
                "ab\n",
                Rules {
                    ignore_space: true,
                    ..Rules::default()
                },
                false,
            ),
            (
                "A\r\n",
                "a\n",
                Rules {
                    ignore_case: true,
                    ..Rules::default()
                },
                false,
            ),
            (
                "a\r\n",
                "a\n",
                Rules {
                    ignore_eol: true,
                    ..Rules::default()
                },
                true,
            ),
            (
                "a\n",
                "a",
                Rules {
                    ignore_eol: true,
                    ..Rules::default()
                },
                false,
            ),
            (
                "a\rb",
                "ab",
                Rules {
                    ignore_eol: true,
                    ..Rules::default()
                },
                false,
            ),
            (
                " ",
                "",
                Rules {
                    ignore_space: true,
                    ..Rules::default()
                },
                false,
            ),
            (
                " \n",
                "\n",
                Rules {
                    ignore_space: true,
                    ..Rules::default()
                },
                true,
            ),
            (
                "Ä\n",
                "ä\n",
                Rules {
                    ignore_case: true,
                    ..Rules::default()
                },
                true,
            ),
            (
                "İ\n",
                "i\u{307}\n",
                Rules {
                    ignore_case: true,
                    ..Rules::default()
                },
                true,
            ),
            (
                "ß\n",
                "ss\n",
                Rules {
                    ignore_case: true,
                    ..Rules::default()
                },
                false,
            ),
        ] {
            assert_eq!(
                compare(a, b, rules, &c).unwrap().diff.edits.is_empty(),
                equal,
                "{a:?} / {b:?}"
            );
        }
    }
    #[test]
    fn unicode_key_expansion_and_cumulative_key_bytes_are_bounded() {
        let c = Cancellation::new();
        let text = "İ".repeat(30);
        let rules = Rules {
            ignore_case: true,
            ..Rules::default()
        };
        assert!(keys(&[&text], rules, &c, 64).is_err());
        assert!(
            keys(
                &["abcdefghijklmnop", "abcdefghijklmnop", "abcdefghijklmnop"],
                rules,
                &c,
                40
            )
            .is_err()
        );
        assert_eq!(keys(&["İ\n"], rules, &c, 4).unwrap(), vec!["i\u{307}\n"]);
    }
    #[test]
    fn default_is_exact_and_rules_never_suppress_merge_conflicts_or_cancellation() {
        let c = Cancellation::new();
        let r = compare("A\n", "a\n", Rules::default(), &c).unwrap();
        assert!(r.ignored.is_empty());
        assert_eq!(r.diff.rows, crate::compare("A\n", "a\n", &c).unwrap().rows);
        assert!(
            compare(
                "A\n",
                "a\n",
                Rules {
                    ignore_case: true,
                    ..Rules::default()
                },
                &c
            )
            .unwrap()
            .diff
            .edits
            .is_empty()
        );
        assert_eq!(
            crate::merge("base\n", "A\n", "a\n", &c)
                .unwrap()
                .unresolved(),
            1
        );
        c.cancel();
        assert!(
            compare(
                "A\n",
                "a\n",
                Rules {
                    ignore_case: true,
                    ..Rules::default()
                },
                &c
            )
            .is_err()
        );
    }
}

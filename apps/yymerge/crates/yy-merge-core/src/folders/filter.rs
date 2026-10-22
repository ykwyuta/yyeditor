//! Display filters never exclude scan inputs or change the overall comparison result.
use super::{Status, three};
use crate::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Selection {
    #[default]
    All,
    Different,
    Equal,
    LeftOnly,
    RightOnly,
    Review,
    Uncompared,
}
pub const SELECTIONS: [Selection; 7] = [
    Selection::All,
    Selection::Different,
    Selection::Equal,
    Selection::LeftOnly,
    Selection::RightOnly,
    Selection::Review,
    Selection::Uncompared,
];
impl Selection {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "すべて",
            Self::Different => "変更・差異あり",
            Self::Equal => "変更なし・同一",
            Self::LeftOnly => "左側にのみ存在",
            Self::RightOnly => "右側にのみ存在",
            Self::Review => "要確認（競合・両側変更）",
            Self::Uncompared => "未比較・エラー",
        }
    }
}
#[derive(Clone, Copy)]
pub struct Facts {
    pub equal: bool,
    pub incomplete: bool,
    left_only: bool,
    right_only: bool,
    review: bool,
}
impl Facts {
    pub fn two(status: &Status) -> Self {
        Self {
            equal: *status == Status::Equal,
            incomplete: matches!(status, Status::Error | Status::SkippedLink),
            left_only: *status == Status::LeftOnly,
            right_only: *status == Status::RightOnly,
            review: *status == Status::TypeMismatch,
        }
    }
    pub fn three(status: three::Status, left_present: bool, right_present: bool) -> Self {
        use three::Status as S;
        let incomplete = matches!(status, S::Error | S::SkippedLink);
        Self {
            equal: status == S::Unchanged,
            incomplete,
            left_only: !incomplete && left_present && !right_present,
            right_only: !incomplete && right_present && !left_present,
            review: matches!(
                status,
                S::BothChanged | S::AdditionConflict | S::DeleteModifyConflict | S::TypeConflict
            ),
        }
    }
}
pub fn path_key(path: &Path) -> String {
    normalize(&path.to_string_lossy())
}
fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| if c == '\\' { '/' } else { c })
        .flat_map(char::to_lowercase)
        .collect()
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct Filter {
    path_contains: String,
    status: Selection,
    #[serde(skip)]
    needle: String,
}
impl Filter {
    pub fn new(path_contains: String, status: Selection) -> Result<Self> {
        if path_contains.len() > 2048 || path_contains.contains('\0') {
            return Err(
                "folder filter: literal path query must be at most 2048 UTF-8 bytes without NUL"
                    .into(),
            );
        }
        let needle = normalize(&path_contains);
        Ok(Self {
            path_contains,
            status,
            needle,
        })
    }
    pub fn query(&self) -> &str {
        &self.path_contains
    }
    pub fn status(&self) -> Selection {
        self.status
    }
    pub fn active(&self) -> bool {
        !self.path_contains.is_empty() || self.status != Selection::All
    }
    pub fn matches(&self, path_key: &str, facts: Facts) -> bool {
        let status = match self.status {
            Selection::All => true,
            Selection::Different => !facts.equal && !facts.incomplete,
            Selection::Equal => facts.equal,
            Selection::LeftOnly => facts.left_only,
            Selection::RightOnly => facts.right_only,
            Selection::Review => facts.review,
            Selection::Uncompared => facts.incomplete,
        };
        status && path_key.contains(&self.needle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_unicode_path_filter_preserves_spaces_metacharacters_and_boundaries() {
        let facts = Facts::two(&Status::Different);
        let key = path_key(Path::new("Src/日本語/Ä-data.CSV"));
        assert!(
            Filter::new("SRC\\日本語\\ä-".into(), Selection::Different)
                .unwrap()
                .matches(&key, facts)
        );
        assert!(
            !Filter::new("*.csv".into(), Selection::All)
                .unwrap()
                .matches(&key, facts)
        );
        assert!(
            !Filter::new(" data".into(), Selection::All)
                .unwrap()
                .matches(&key, facts)
        );
        assert!(
            !Filter::new("ss".into(), Selection::All)
                .unwrap()
                .matches(&normalize("ß"), facts)
        );
        assert!(Filter::new(" ".into(), Selection::All).unwrap().active());
        assert!(Filter::new("x".repeat(2049), Selection::All).is_err());
        assert!(!Filter::default().active());
    }
    #[test]
    fn two_and_three_way_status_groups_keep_incomplete_and_review_distinct() {
        use three::Status as S;
        let filter = |s| Filter::new(String::new(), s).unwrap();
        assert!(!filter(Selection::Different).matches("", Facts::two(&Status::Error)));
        assert!(filter(Selection::Uncompared).matches("", Facts::two(&Status::SkippedLink)));
        assert!(filter(Selection::Review).matches("", Facts::two(&Status::TypeMismatch)));
        for (status, review, changed) in [
            (S::Unchanged, false, false),
            (S::SameChange, false, true),
            (S::BothChanged, true, true),
            (S::AdditionConflict, true, true),
            (S::DeleteModifyConflict, true, true),
            (S::TypeConflict, true, true),
            (S::Error, false, false),
            (S::SkippedLink, false, false),
        ] {
            let facts = Facts::three(status, true, false);
            assert_eq!(filter(Selection::Different).matches("", facts), changed);
            assert_eq!(filter(Selection::Review).matches("", facts), review);
            assert_eq!(
                filter(Selection::Equal).matches("", facts),
                status == S::Unchanged
            );
            if facts.incomplete {
                assert!(!filter(Selection::LeftOnly).matches("", facts));
            }
        }
        assert!(
            filter(Selection::LeftOnly).matches("", Facts::three(S::RightDeleted, true, false))
        );
        assert!(
            filter(Selection::RightOnly).matches("", Facts::three(S::LeftDeleted, false, true))
        );
    }
}

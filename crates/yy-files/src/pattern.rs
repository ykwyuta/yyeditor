//! 名前のパターン（`*` と `?`。大文字・小文字は区別しない）。

/// 既定で除くファイル。
pub const DEFAULT_EXCLUDE_FILES: &[&str] = &[
    "Thumbs.db",
    "desktop.ini",
    ".DS_Store",
    "~$*",
    "*.tmp",
    "*.yypart",
];

/// 既定で除くフォルダ。
pub const DEFAULT_EXCLUDE_DIRS: &[&str] = &[
    "$RECYCLE.BIN",
    "System Volume Information",
    ".git",
    ".svn",
    crate::purge::TRASH_DIR,
];

/// 名前のパターンの集まり。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Patterns(Vec<String>);

impl Patterns {
    pub fn new<S: AsRef<str>>(pats: &[S]) -> Patterns {
        Patterns(
            pats.iter()
                .map(|p| p.as_ref().trim().to_owned())
                .filter(|p| !p.is_empty())
                .collect(),
        )
    }

    /// `;` 区切りの文字列から。
    pub fn parse(text: &str) -> Patterns {
        Patterns::new(&text.split(';').collect::<Vec<_>>())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// どれかに一致するか。
    pub fn matches(&self, name: &str) -> bool {
        self.0
            .iter()
            .any(|p| yy_core::grep::wildcard_match(p, name))
    }

    pub fn items(&self) -> &[String] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        let p = Patterns::new(DEFAULT_EXCLUDE_FILES);
        assert!(p.matches("THUMBS.DB"));
        assert!(p.matches("~$報告書.docx"));
        assert!(p.matches("a.yypart"));
        assert!(!p.matches("報告書.docx"));
        assert!(Patterns::parse("*.xlsx; *.csv").matches("a.CSV"));
        assert!(Patterns::parse("").is_empty());
    }
}

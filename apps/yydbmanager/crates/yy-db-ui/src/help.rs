//! Embedded Markdown guide, rendered by the common yy help host.
use super::*;
pub(super) fn section(id: u16) -> Option<&'static str> {
    match id {
        HELP_USAGE => Some("usage"),
        HELP_PLANS => Some("plans"),
        HELP_PERFORMANCE => Some("performance"),
        HELP_PLAN_FILES => Some("plan-files"),
        HELP_ORACLE => Some("oracle"),
        HELP_DB2 => Some("db2"),
        HELP_TROUBLE => Some("troubleshooting"),
        HELP_YAML => Some("er-yaml"),
        _ => None,
    }
}
pub(super) fn markdown() -> String {
    include_str!("../../../help/help.md")
        .replace("{{DB2_SETUP_SQL}}", yy_db_core::plan::DB2_ACTUAL_SETUP_SQL)
        .replace(
            "{{ORACLE_PLAN_EXPORT}}",
            include_str!("../../../../../demo/yydbmanager/sql/plan-files/export-oracle.sql"),
        )
        .replace(
            "{{DB2_PLAN_EXPORT}}",
            include_str!("../../../../../demo/yydbmanager/sql/plan-files/export-db2.sql"),
        )
}
pub(super) fn show(section: &str) -> windows::core::Result<()> {
    yy_ui_win::help::show(
        &markdown(),
        "yydbmanager ヘルプ",
        Some(section),
        "yydbmanager",
    )
}

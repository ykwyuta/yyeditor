//! Database sessions, exact values, execution plans and file transfer.
pub mod plan;
pub mod plan_file;
pub mod plan_visual;
pub mod schema;
pub mod session;
pub mod sql;
pub mod transfer;
pub mod values;
pub use session::{Database, DbKind, Options};
pub use values::{Column, ResultSet, Value};

pub type Result<T> = std::result::Result<T, String>;
pub mod catalog;
pub mod diagnostics;

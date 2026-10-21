//! Native yydbmanager shell.
#[cfg(windows)]
mod native;
#[cfg(windows)]
mod profiles;
#[cfg(windows)]
pub use native::run;
#[cfg(windows)]
pub use native::smoke;

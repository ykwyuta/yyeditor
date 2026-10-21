//! Windows host with the yy series' shared editor, font and offline help.
#[cfg(windows)]
mod comparison;
#[cfg(windows)]
mod native;
#[cfg(windows)]
pub use native::run;

//! Portable pixel editor, six orthographic views, voxel reconstruction and exports.
pub mod image;
pub mod mesh;
pub mod model;
pub mod paint;
pub mod sculpt;
pub mod storage;
pub use model::*;
pub use paint::*;
pub use storage::{load, save};

pub(crate) fn invalid(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}

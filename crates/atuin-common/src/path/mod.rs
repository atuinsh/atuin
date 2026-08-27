//! Filesystem path utilities and extension traits.

pub mod display_rich;

use std::path::Path;

pub use display_rich::{DisplayRichExt, RichDisplay};

/// Utility extensions for paths in atuin.
pub trait PathExt {
    /// Check whether the given path is a symlink, and a dangling one at that.
    fn is_dangling_symlink(&self) -> bool;
}

impl<P: AsRef<Path>> PathExt for P {
    fn is_dangling_symlink(&self) -> bool {
        let path: &Path = self.as_ref();
        path.is_symlink() && !path.exists()
    }
}

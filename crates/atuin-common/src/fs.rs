use std::path::{Path, PathBuf};

/// A path that is removed when this type is dropped.
pub struct RemoveOnDropPath<P: AsRef<Path> = PathBuf>(
    /// The path to the file.
    pub P,
);

impl<P: AsRef<Path>> Drop for RemoveOnDropPath<P> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// Not using `derive_more` as it causes Clippy to emit an incorrect "this trait bound is already
// specified in the where clause" warning.
impl<P: AsRef<Path>> AsRef<Path> for RemoveOnDropPath<P> {
    fn as_ref(&self) -> &Path {
        self
    }
}

// Not using `derive_more` for this as it would require adding a `P: Deref<Target = Path>` bound to
// the struct.
impl<P: AsRef<Path>> std::ops::Deref for RemoveOnDropPath<P> {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

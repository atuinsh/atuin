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

impl<P: AsRef<Path>> std::ops::Deref for RemoveOnDropPath<P> {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl<P: AsRef<Path>> AsRef<Path> for RemoveOnDropPath<P> {
    fn as_ref(&self) -> &Path {
        self
    }
}

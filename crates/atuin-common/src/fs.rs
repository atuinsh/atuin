use std::path::{Path, PathBuf};

/// A path that is removed when this type is dropped.
#[derive(derive_more::AsRef)]
pub struct RemoveOnDropPath<P: AsRef<Path> = PathBuf>(
    /// The path to the file.
    #[as_ref(Path)]
    pub P,
);

impl<P: AsRef<Path>> Drop for RemoveOnDropPath<P> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
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

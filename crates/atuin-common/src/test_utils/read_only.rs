//! Directory trees locked against writes, as a read-only filesystem holds them.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// A directory tree stripped of write permission until this is dropped.
#[must_use = "dropping the guard gives write permission back at once"]
pub struct ReadOnlyTree {
    /// Each locked path with the mode it had before.
    modes: Vec<(PathBuf, u32)>,
}

impl ReadOnlyTree {
    /// Strip write permission from `root` and everything under it, leaving symlinks and what they
    /// point to alone.
    ///
    /// Returns `None` when this process can write there anyway, as root can. Every mode is given
    /// back then. A test of storage that refuses writes should skip, since it would prove nothing.
    #[must_use]
    pub fn lock(root: &Path) -> Option<Self> {
        let mut tree = Self { modes: Vec::new() };
        tree.lock_path(root);

        let probe = root.join(".atuin-write-probe");
        if fs::File::create_new(&probe).is_ok() {
            let _ = fs::remove_file(&probe);
            eprintln!("skipping: this process can write {} without permission", root.display());
            return None;
        }

        Some(tree)
    }

    fn lock_path(&mut self, path: &Path) {
        let meta = fs::symlink_metadata(path).expect("a path to lock exists");
        if meta.is_symlink() {
            return;
        }

        if meta.is_dir() {
            for entry in fs::read_dir(path).expect("a directory to lock is readable") {
                self.lock_path(&entry.expect("a directory to lock lists its entries").path());
            }
        }

        let mode = meta.permissions().mode();
        fs::set_permissions(path, fs::Permissions::from_mode(mode & !0o222))
            .expect("the owner can change a path's mode");
        self.modes.push((path.to_owned(), mode));
    }
}

impl Drop for ReadOnlyTree {
    fn drop(&mut self) {
        for (path, mode) in self.modes.drain(..).rev() {
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode));
        }
    }
}

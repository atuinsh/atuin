//! Utilities for working with directories
//!
//! Right now just a wrapper around WalkDirs that does some utf8 conversions and strip_prefixing,
//! since we always end up doing that.

use crate::error::*;
use camino::{Utf8Path, Utf8PathBuf};

/// Walk through this dir's descendants with `walkdirs`
pub fn walk_dir(dir: impl AsRef<Utf8Path>) -> AxoassetWalkDir {
    let dir = dir.as_ref();
    AxoassetWalkDir {
        root_dir: dir.to_owned(),
        inner: walkdir::WalkDir::new(dir),
    }
}

/// Wrapper around [`walkdir::WalkDir`][].
pub struct AxoassetWalkDir {
    root_dir: Utf8PathBuf,
    inner: walkdir::WalkDir,
}

/// Wrapper around [`walkdir::IntoIter`][].
pub struct AxoassetIntoIter {
    root_dir: Utf8PathBuf,
    inner: walkdir::IntoIter,
}

/// Wrapper around [`walkdir::DirEntry`][].
pub struct AxoassetDirEntry {
    /// full path to the entry
    pub full_path: Utf8PathBuf,
    /// path to the entry relative to the dir passed to [`walk_dir`][].
    pub rel_path: Utf8PathBuf,
    /// Inner contents
    pub entry: walkdir::DirEntry,
}

impl IntoIterator for AxoassetWalkDir {
    type IntoIter = AxoassetIntoIter;
    type Item = Result<AxoassetDirEntry>;
    fn into_iter(self) -> Self::IntoIter {
        AxoassetIntoIter {
            root_dir: self.root_dir,
            inner: self.inner.into_iter(),
        }
    }
}

impl Iterator for AxoassetIntoIter {
    type Item = Result<AxoassetDirEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|next| {
            let entry = next.map_err(|e| AxoassetError::WalkDirFailed {
                origin_path: self.root_dir.clone(),
                details: e,
            })?;

            let full_path = Utf8PathBuf::from_path_buf(entry.path().to_owned())
                .map_err(|details| AxoassetError::Utf8Path { path: details })?;
            let rel_path = full_path
                .strip_prefix(&self.root_dir)
                .map_err(|_| AxoassetError::PathNesting {
                    root_dir: self.root_dir.clone(),
                    child_dir: full_path.clone(),
                })?
                .to_owned();

            Ok(AxoassetDirEntry {
                full_path,
                rel_path,
                entry,
            })
        })
    }
}

impl std::ops::Deref for AxoassetDirEntry {
    type Target = walkdir::DirEntry;
    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}
impl std::ops::DerefMut for AxoassetDirEntry {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entry
    }
}

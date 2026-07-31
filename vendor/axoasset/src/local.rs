//! Local file operations

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use crate::{dirs, error::*};

/// A local asset contains a path on the local filesystem and its contents
#[derive(Debug)]
pub struct LocalAsset {
    /// The computed filename from origin_path
    filename: String,
    /// A string representing a path on the local filesystem, where the asset
    /// originated. For a new asset, this will be the path you want the asset
    /// to be written to. This path is how the filename is determined for all
    /// asset operations.
    origin_path: Utf8PathBuf,
    /// The contents of the asset as a vector of bytes.
    contents: Vec<u8>,
}

impl LocalAsset {
    /// Gets the filename of the LocalAsset
    pub fn filename(&self) -> &str {
        &self.filename
    }

    /// Gets the origin_path of the LocalAsset
    pub fn origin_path(&self) -> &Utf8Path {
        &self.origin_path
    }

    /// Gets the bytes of the LocalAsset
    pub fn as_bytes(&self) -> &[u8] {
        &self.contents
    }

    /// Gets the bytes of the LocalAsset by-value
    pub fn into_bytes(self) -> Vec<u8> {
        self.contents
    }

    /// A new asset is created with claimed path on the local filesystem and a
    /// vector of bytes representing its contents.
    ///
    /// Note that this DOES NOT do any IO, it just pretends the given bytes
    /// were loaded from that location.
    pub fn new(origin_path: impl AsRef<Utf8Path>, contents: Vec<u8>) -> Result<Self> {
        let origin_path = origin_path.as_ref();
        Ok(LocalAsset {
            filename: filename(origin_path)?,
            origin_path: origin_path.to_owned(),
            contents,
        })
    }

    /// Loads an asset from a path on the local filesystem, returning a
    /// LocalAsset struct
    pub fn load_asset(origin_path: impl AsRef<Utf8Path>) -> Result<LocalAsset> {
        let origin_path = origin_path.as_ref();
        match origin_path.try_exists() {
            Ok(_) => match fs::read(origin_path) {
                Ok(contents) => Ok(LocalAsset {
                    filename: filename(origin_path)?,
                    origin_path: origin_path.to_owned(),
                    contents,
                }),
                Err(details) => Err(AxoassetError::LocalAssetReadFailed {
                    origin_path: origin_path.to_string(),
                    details,
                }),
            },
            Err(details) => Err(AxoassetError::LocalAssetNotFound {
                origin_path: origin_path.to_string(),
                details,
            }),
        }
    }

    /// Loads an asset from a path on the local filesystem, returning a
    /// string of its contents
    pub fn load_string(origin_path: impl AsRef<Utf8Path>) -> Result<String> {
        let origin_path = origin_path.as_ref();
        match origin_path.try_exists() {
            Ok(_) => match fs::read_to_string(origin_path) {
                Ok(contents) => Ok(contents),
                Err(details) => Err(AxoassetError::LocalAssetReadFailed {
                    origin_path: origin_path.to_string(),
                    details,
                }),
            },
            Err(details) => Err(AxoassetError::LocalAssetNotFound {
                origin_path: origin_path.to_string(),
                details,
            }),
        }
    }

    /// Loads an asset from a path on the local filesystem, returning a
    /// vector of bytes of its contents
    pub fn load_bytes(origin_path: impl AsRef<Utf8Path>) -> Result<Vec<u8>> {
        let origin_path = origin_path.as_ref();
        match origin_path.try_exists() {
            Ok(_) => match fs::read(origin_path) {
                Ok(contents) => Ok(contents),
                Err(details) => Err(AxoassetError::LocalAssetReadFailed {
                    origin_path: origin_path.to_string(),
                    details,
                }),
            },
            Err(details) => Err(AxoassetError::LocalAssetNotFound {
                origin_path: origin_path.to_string(),
                details,
            }),
        }
    }

    /// Writes an asset to a path on the local filesystem, determines the
    /// filename from the origin path
    pub fn write_to_dir(&self, dest_dir: impl AsRef<Utf8Path>) -> Result<Utf8PathBuf> {
        let dest_dir = dest_dir.as_ref();
        let dest_path = dest_dir.join(&self.filename);
        match fs::write(&dest_path, &self.contents) {
            Ok(_) => Ok(dest_path),
            Err(details) => Err(AxoassetError::LocalAssetWriteFailed {
                origin_path: self.origin_path.to_string(),
                dest_path: dest_path.to_string(),
                details,
            }),
        }
    }

    /// Writes an asset to a path on the local filesystem
    pub fn write_new(contents: &str, dest_path: impl AsRef<Utf8Path>) -> Result<Utf8PathBuf> {
        let dest_path = dest_path.as_ref();
        if dest_path.file_name().is_none() {
            return Err(AxoassetError::LocalAssetMissingFilename {
                origin_path: dest_path.to_string(),
            });
        }
        match fs::write(dest_path, contents) {
            Ok(_) => Ok(dest_path.into()),
            Err(details) => Err(AxoassetError::LocalAssetWriteNewFailed {
                dest_path: dest_path.to_string(),
                details,
            }),
        }
    }

    /// Writes an asset and all of its parent directories on the local filesystem.
    pub fn write_new_all(contents: &str, dest_path: impl AsRef<Utf8Path>) -> Result<Utf8PathBuf> {
        let dest_path = dest_path.as_ref();
        if dest_path.file_name().is_none() {
            return Err(AxoassetError::LocalAssetMissingFilename {
                origin_path: dest_path.to_string(),
            });
        }
        let dest_dir = dest_path.parent().unwrap();
        match fs::create_dir_all(dest_dir) {
            Ok(_) => (),
            Err(details) => {
                return Err(AxoassetError::LocalAssetWriteNewFailed {
                    dest_path: dest_path.to_string(),
                    details,
                })
            }
        }
        LocalAsset::write_new(contents, dest_path)
    }

    /// Creates a new directory
    pub fn create_dir(dest: impl AsRef<Utf8Path>) -> Result<Utf8PathBuf> {
        let dest_path = dest.as_ref();
        match fs::create_dir(dest_path) {
            Ok(_) => Ok(dest_path.into()),
            Err(details) => Err(AxoassetError::LocalAssetDirCreationFailed {
                dest_path: dest_path.to_string(),
                details,
            }),
        }
    }

    /// Creates a new directory, including all parent directories
    pub fn create_dir_all(dest: impl AsRef<Utf8Path>) -> Result<Utf8PathBuf> {
        let dest_path = dest.as_ref();
        match fs::create_dir_all(dest_path) {
            Ok(_) => Ok(dest_path.into()),
            Err(details) => Err(AxoassetError::LocalAssetDirCreationFailed {
                dest_path: dest_path.to_string(),
                details,
            }),
        }
    }

    /// Removes a file
    pub fn remove_file(dest: impl AsRef<Utf8Path>) -> Result<()> {
        let dest_path = dest.as_ref();
        if let Err(details) = fs::remove_file(dest_path) {
            return Err(AxoassetError::LocalAssetRemoveFailed {
                dest_path: dest_path.to_string(),
                details,
            });
        }

        Ok(())
    }

    /// Removes a directory
    pub fn remove_dir(dest: impl AsRef<Utf8Path>) -> Result<()> {
        let dest_path = dest.as_ref();
        if dest_path.is_dir() {
            if let Err(details) = fs::remove_dir(dest_path) {
                return Err(AxoassetError::LocalAssetRemoveFailed {
                    dest_path: dest_path.to_string(),
                    details,
                });
            }
        }

        Ok(())
    }

    /// Removes a directory and all of its contents
    pub fn remove_dir_all(dest: impl AsRef<Utf8Path>) -> Result<()> {
        let dest_path = dest.as_ref();
        if dest_path.is_dir() {
            if let Err(details) = fs::remove_dir_all(dest_path) {
                return Err(AxoassetError::LocalAssetRemoveFailed {
                    dest_path: dest_path.to_string(),
                    details,
                });
            }
        }

        Ok(())
    }

    /// Copies an asset from one location on the local filesystem to the given directory
    ///
    /// The destination will use the same file name as the origin has.
    /// If you want to specify the destination file's name, use [`LocalAsset::copy_file_to_file`][].
    ///
    /// The returned path is the resulting file.
    pub fn copy_file_to_dir(
        origin_path: impl AsRef<Utf8Path>,
        dest_dir: impl AsRef<Utf8Path>,
    ) -> Result<Utf8PathBuf> {
        let origin_path = origin_path.as_ref();
        let dest_dir = dest_dir.as_ref();

        let filename = filename(origin_path)?;
        let dest_path = dest_dir.join(filename);
        Self::copy_file_to_file(origin_path, &dest_path)?;

        Ok(dest_path)
    }

    /// Copies an asset from one location on the local filesystem to another
    ///
    /// Both paths are assumed to be file names.
    pub fn copy_file_to_file(
        origin_path: impl AsRef<Utf8Path>,
        dest_path: impl AsRef<Utf8Path>,
    ) -> Result<()> {
        let origin_path = origin_path.as_ref();
        let dest_path = dest_path.as_ref();

        fs::copy(origin_path, dest_path).map_err(|e| AxoassetError::LocalAssetCopyFailed {
            origin_path: origin_path.to_string(),
            dest_path: dest_path.to_string(),
            details: e,
        })?;

        Ok(())
    }

    /// Recursively copies a directory from one location to the given directory
    ///
    /// The destination will use the same dir name as the origin has, so
    /// dest_dir is the *parent* of the copied directory. If you want to specify the destination's
    /// dir name, use [`LocalAsset::copy_dir_to_dir`][].
    ///
    /// The returned path is the resulting dir.
    pub fn copy_dir_to_parent_dir(
        origin_path: impl AsRef<Utf8Path>,
        dest_dir: impl AsRef<Utf8Path>,
    ) -> Result<Utf8PathBuf> {
        let origin_path = origin_path.as_ref();
        let dest_dir = dest_dir.as_ref();

        let filename = filename(origin_path)?;
        let dest_path = dest_dir.join(filename);
        Self::copy_dir_to_dir(origin_path, &dest_path)?;

        Ok(dest_path)
    }

    /// Recursively copies a directory from one location to another
    ///
    /// Both paths are assumed to be the names of the directory being copied
    /// (i.e. dest_path is not the parent dir).
    pub fn copy_dir_to_dir(
        origin_path: impl AsRef<Utf8Path>,
        dest_path: impl AsRef<Utf8Path>,
    ) -> Result<()> {
        let origin_path = origin_path.as_ref();
        let dest_path = dest_path.as_ref();

        for entry in dirs::walk_dir(origin_path) {
            let entry = entry?;
            let from = &entry.full_path;
            let to = dest_path.join(&entry.rel_path);

            if entry.file_type().is_dir() {
                // create directories (even empty ones!)
                LocalAsset::create_dir(to)?;
            } else if entry.file_type().is_file() {
                // copy files
                LocalAsset::copy_file_to_file(from, to)?;
            } else {
                // other kinds of file presumed to be symlinks which we don't handle
                debug_assert!(
                    entry.file_type().is_symlink(),
                    "unknown type of file at {from}, axoasset needs to be updated to support this!"
                );
            }
        }
        Ok(())
    }

    /// Get the current working directory
    pub fn current_dir() -> Result<Utf8PathBuf> {
        let cur_dir =
            std::env::current_dir().map_err(|details| AxoassetError::CurrentDir { details })?;
        let cur_dir = Utf8PathBuf::from_path_buf(cur_dir)
            .map_err(|details| AxoassetError::Utf8Path { path: details })?;
        Ok(cur_dir)
    }

    /// Find a desired file in the provided dir or an ancestor of it.
    ///
    /// On success returns the path to the found file.
    pub fn search_ancestors(
        start_dir: impl AsRef<Utf8Path>,
        desired_filename: &str,
    ) -> Result<Utf8PathBuf> {
        let start_dir = start_dir.as_ref();
        // We want a proper absolute path so we can compare paths to workspace roots easily.
        //
        // Also if someone starts the path with ./ we should trim that to avoid weirdness.
        // Maybe we should be using proper `canonicalize` but then we'd need to canonicalize
        // every path we get from random APIs to be consistent and that's a whole mess of its own!
        let start_dir = if let Ok(clean_dir) = start_dir.strip_prefix("./") {
            clean_dir.to_owned()
        } else {
            start_dir.to_owned()
        };
        let start_dir = if start_dir.is_relative() {
            let current_dir = LocalAsset::current_dir()?;
            current_dir.join(start_dir)
        } else {
            start_dir
        };
        for dir_path in start_dir.ancestors() {
            let file_path = dir_path.join(desired_filename);
            if file_path.is_file() {
                return Ok(file_path);
            }
        }
        Err(AxoassetError::SearchFailed {
            start_dir,
            desired_filename: desired_filename.to_owned(),
        })
    }

    /// Creates a new .tar.gz file from a provided directory
    ///
    /// The with_root argument specifies that all contents of dest_dir should be placed
    /// under the given path within the archive. If None then the contents of the dir will
    /// be placed directly in the root. root_dir can be a proper path with subdirs
    /// (e.g. `root_dir = "some/dir/prefix"` is valid).
    #[cfg(any(feature = "compression", feature = "compression-tar"))]
    pub fn tar_gz_dir(
        origin_dir: impl AsRef<Utf8Path>,
        dest_dir: impl AsRef<Utf8Path>,
        with_root: Option<impl AsRef<Utf8Path>>,
    ) -> Result<()> {
        crate::compression::tar_dir(
            Utf8Path::new(origin_dir.as_ref()),
            Utf8Path::new(dest_dir.as_ref()),
            with_root.as_ref().map(|p| p.as_ref()),
            &crate::compression::CompressionImpl::Gzip,
        )
    }

    /// Extracts the entire tarball at `tarball` to a provided directory
    #[cfg(any(feature = "compression", feature = "compression-tar"))]
    pub fn untar_gz_all(tarball: &Utf8Path, dest_path: &Utf8Path) -> Result<()> {
        crate::compression::untar_all(
            tarball,
            dest_path,
            &crate::compression::CompressionImpl::Gzip,
        )
    }

    /// Extracts the file named `filename` within the tarball at `tarball` and returns its contents as bytes
    #[cfg(any(feature = "compression", feature = "compression-tar"))]
    pub fn untar_gz_file(tarball: &Utf8Path, filename: &str) -> Result<Vec<u8>> {
        crate::compression::untar_file(
            tarball,
            filename,
            &crate::compression::CompressionImpl::Gzip,
        )
    }

    /// Creates a new .tar.xz file from a provided directory
    ///
    /// The with_root argument specifies that all contents of dest_dir should be placed
    /// under the given path within the archive. If None then the contents of the dir will
    /// be placed directly in the root. root_dir can be a proper path with subdirs
    /// (e.g. `root_dir = "some/dir/prefix"` is valid).
    #[cfg(any(feature = "compression", feature = "compression-tar"))]
    pub fn tar_xz_dir(
        origin_dir: impl AsRef<Utf8Path>,
        dest_dir: impl AsRef<Utf8Path>,
        with_root: Option<impl AsRef<Utf8Path>>,
    ) -> Result<()> {
        crate::compression::tar_dir(
            Utf8Path::new(origin_dir.as_ref()),
            Utf8Path::new(dest_dir.as_ref()),
            with_root.as_ref().map(|p| p.as_ref()),
            &crate::compression::CompressionImpl::Xzip,
        )
    }

    /// Extracts the entire tarball at `tarball` to a provided directory
    #[cfg(any(feature = "compression", feature = "compression-tar"))]
    pub fn untar_xz_all(
        tarball: impl AsRef<Utf8Path>,
        dest_path: impl AsRef<Utf8Path>,
    ) -> Result<()> {
        crate::compression::untar_all(
            Utf8Path::new(tarball.as_ref()),
            Utf8Path::new(dest_path.as_ref()),
            &crate::compression::CompressionImpl::Xzip,
        )
    }

    /// Extracts the file named `filename` within the tarball at `tarball` and returns its contents as bytes
    #[cfg(any(feature = "compression", feature = "compression-tar"))]
    pub fn untar_xz_file(tarball: impl AsRef<Utf8Path>, filename: &str) -> Result<Vec<u8>> {
        crate::compression::untar_file(
            Utf8Path::new(tarball.as_ref()),
            filename,
            &crate::compression::CompressionImpl::Xzip,
        )
    }

    /// Creates a new .tar.zstd file from a provided directory
    ///
    /// The with_root argument specifies that all contents of dest_dir should be placed
    /// under the given path within the archive. If None then the contents of the dir will
    /// be placed directly in the root. root_dir can be a proper path with subdirs
    /// (e.g. `root_dir = "some/dir/prefix"` is valid).
    #[cfg(any(feature = "compression", feature = "compression-tar"))]
    pub fn tar_zstd_dir(
        origin_dir: impl AsRef<Utf8Path>,
        dest_dir: impl AsRef<Utf8Path>,
        with_root: Option<impl AsRef<Utf8Path>>,
    ) -> Result<()> {
        crate::compression::tar_dir(
            Utf8Path::new(origin_dir.as_ref()),
            Utf8Path::new(dest_dir.as_ref()),
            with_root.as_ref().map(|p| p.as_ref()),
            &crate::compression::CompressionImpl::Zstd,
        )
    }

    /// Extracts the entire tarball at `tarball` to a provided directory
    #[cfg(any(feature = "compression", feature = "compression-tar"))]
    pub fn untar_zstd_all(
        tarball: impl AsRef<Utf8Path>,
        dest_path: impl AsRef<Utf8Path>,
    ) -> Result<()> {
        crate::compression::untar_all(
            Utf8Path::new(tarball.as_ref()),
            Utf8Path::new(dest_path.as_ref()),
            &crate::compression::CompressionImpl::Zstd,
        )
    }

    /// Extracts the file named `filename` within the tarball at `tarball` and returns its contents as bytes
    #[cfg(any(feature = "compression", feature = "compression-tar"))]
    pub fn untar_zstd_file(tarball: impl AsRef<Utf8Path>, filename: &str) -> Result<Vec<u8>> {
        crate::compression::untar_file(
            Utf8Path::new(tarball.as_ref()),
            filename,
            &crate::compression::CompressionImpl::Zstd,
        )
    }

    /// Creates a new .zip file from a provided directory
    ///
    /// The with_root argument specifies that all contents of dest_dir should be placed
    /// under the given path within the archive. If None then the contents of the dir will
    /// be placed directly in the root. root_dir can be a proper path with subdirs
    /// (e.g. `root_dir = "some/dir/prefix"` is valid).
    #[cfg(any(feature = "compression", feature = "compression-zip"))]
    pub fn zip_dir(
        origin_dir: impl AsRef<Utf8Path>,
        dest_dir: impl AsRef<Utf8Path>,
        with_root: Option<impl AsRef<Utf8Path>>,
    ) -> Result<()> {
        crate::compression::zip_dir(
            Utf8Path::new(origin_dir.as_ref()),
            Utf8Path::new(dest_dir.as_ref()),
            with_root.as_ref().map(|p| p.as_ref()),
        )
    }

    /// Extracts a .zip file to the a provided directory
    #[cfg(any(feature = "compression", feature = "compression-zip"))]
    pub fn unzip_all(zipfile: impl AsRef<Utf8Path>, dest_dir: impl AsRef<Utf8Path>) -> Result<()> {
        crate::compression::unzip_all(
            Utf8Path::new(zipfile.as_ref()),
            Utf8Path::new(dest_dir.as_ref()),
        )
    }

    /// Extracts the file named `filename` within the ZIP file at `zipfile` and returns its contents as bytes
    #[cfg(any(feature = "compression", feature = "compression-zip"))]
    pub fn unzip_file(zipfile: impl AsRef<Utf8Path>, filename: &str) -> Result<Vec<u8>> {
        crate::compression::unzip_file(Utf8Path::new(zipfile.as_ref()), filename)
    }
}

/// Get the filename of a path, or a pretty error
pub fn filename(origin_path: &Utf8Path) -> Result<String> {
    if let Some(filename) = origin_path.file_name() {
        Ok(filename.to_string())
    } else {
        Err(AxoassetError::LocalAssetMissingFilename {
            origin_path: origin_path.to_string(),
        })
    }
}

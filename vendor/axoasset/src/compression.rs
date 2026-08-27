//! Compression-related methods, all used in `axoasset::Local`

use camino::Utf8Path;
#[cfg(feature = "compression-zip")]
use camino::Utf8PathBuf;
#[cfg(feature = "compression-zip")]
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::AxoassetError;

/// Internal tar-file compression algorithms
#[cfg(feature = "compression-tar")]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum CompressionImpl {
    /// .gz
    Gzip,
    /// .xz
    Xzip,
    /// .zstd
    Zstd,
}

lazy_static::lazy_static! {
    static ref DEFAULT_GZ_LEVEL: u32 = {
        std::env::var("AXOASSET_GZ_LEVEL")
            .ok()
            .and_then(|val| val.parse().ok())
            .unwrap_or(6)
    };
    static ref DEFAULT_XZ_LEVEL: u32 = {
        std::env::var("AXOASSET_XZ_LEVEL")
            .ok()
            .and_then(|val| val.parse().ok())
            .unwrap_or(9)
    };
    static ref DEFAULT_ZSTD_LEVEL: i32 = {
        std::env::var("AXOASSET_ZSTD_LEVEL")
            .ok()
            .and_then(|val| val.parse().ok())
            .unwrap_or(3)
    };
}

#[cfg(feature = "compression-tar")]
pub(crate) fn tar_dir(
    src_path: &Utf8Path,
    dest_path: &Utf8Path,
    with_root: Option<&Utf8Path>,
    compression: &CompressionImpl,
) -> crate::error::Result<()> {
    use crate::error::*;
    use flate2::{Compression, GzBuilder};
    use std::fs;
    use xz2::write::XzEncoder;
    use zstd::stream::Encoder as ZstdEncoder;

    // Set up the archive/compression
    // dir_name here is a prefix directory/path that the src dir's contents will be stored
    // under when being tarred. Having it be empty means the contents
    // will be placed in the root of the tarball.
    let dir_name = with_root.unwrap_or_else(|| Utf8Path::new(""));
    let zip_contents_name = format!("{}.tar", dest_path.file_name().unwrap());
    let final_zip_file = match fs::File::create(dest_path) {
        Ok(file) => file,
        Err(details) => {
            return Err(AxoassetError::LocalAssetWriteNewFailed {
                dest_path: dest_path.to_string(),
                details,
            })
        }
    };

    match compression {
        CompressionImpl::Gzip => {
            // Wrap our file in compression
            let zip_output = GzBuilder::new()
                .filename(zip_contents_name)
                .write(final_zip_file, Compression::new(*DEFAULT_GZ_LEVEL));

            // Write the tar to the compression stream
            let mut tar = tar::Builder::new(zip_output);

            // Add the whole dir to the tar
            if let Err(details) = tar.append_dir_all(dir_name, src_path) {
                return Err(AxoassetError::Compression {
                    reason: format!("failed to copy directory into tar: {src_path} => {dir_name}",),
                    details,
                });
            }
            // Finish up the tarring
            let zip_output = match tar.into_inner() {
                Ok(out) => out,
                Err(details) => {
                    return Err(AxoassetError::Compression {
                        reason: format!("failed to write tar: {dest_path}"),
                        details,
                    })
                }
            };
            // Finish up the compression
            let _zip_file = match zip_output.finish() {
                Ok(file) => file,
                Err(details) => {
                    return Err(AxoassetError::Compression {
                        reason: format!("failed to write archive: {dest_path}"),
                        details,
                    })
                }
            };
            // Drop the file to close it
        }
        CompressionImpl::Xzip => {
            let zip_output = XzEncoder::new(final_zip_file, *DEFAULT_XZ_LEVEL);
            // Write the tar to the compression stream
            let mut tar = tar::Builder::new(zip_output);

            // Add the whole dir to the tar
            if let Err(details) = tar.append_dir_all(dir_name, src_path) {
                return Err(AxoassetError::Compression {
                    reason: format!("failed to copy directory into tar: {src_path} => {dir_name}",),
                    details,
                });
            }
            // Finish up the tarring
            let zip_output = match tar.into_inner() {
                Ok(out) => out,
                Err(details) => {
                    return Err(AxoassetError::Compression {
                        reason: format!("failed to write tar: {dest_path}"),
                        details,
                    })
                }
            };
            // Finish up the compression
            let _zip_file = match zip_output.finish() {
                Ok(file) => file,
                Err(details) => {
                    return Err(AxoassetError::Compression {
                        reason: format!("failed to write archive: {dest_path}"),
                        details,
                    })
                }
            };
            // Drop the file to close it
        }
        CompressionImpl::Zstd => {
            // Wrap our file in compression
            let zip_output =
                ZstdEncoder::new(final_zip_file, *DEFAULT_ZSTD_LEVEL).map_err(|details| {
                    AxoassetError::Compression {
                        reason: "failed to create zstd encoder".to_string(),
                        details,
                    }
                })?;

            // Write the tar to the compression stream
            let mut tar = tar::Builder::new(zip_output);

            // Add the whole dir to the tar
            if let Err(details) = tar.append_dir_all(dir_name, src_path) {
                return Err(AxoassetError::Compression {
                    reason: format!("failed to copy directory into tar: {src_path} => {dir_name}",),
                    details,
                });
            }
            // Finish up the tarring
            let zip_output = match tar.into_inner() {
                Ok(out) => out,
                Err(details) => {
                    return Err(AxoassetError::Compression {
                        reason: format!("failed to write tar: {dest_path}"),
                        details,
                    })
                }
            };
            // Finish up the compression
            let _zip_file = match zip_output.finish() {
                Ok(file) => file,
                Err(details) => {
                    return Err(AxoassetError::Compression {
                        reason: format!("failed to write archive: {dest_path}"),
                        details,
                    })
                }
            };
            // Drop the file to close it
        }
    }

    Ok(())
}

#[cfg(feature = "compression-tar")]
fn open_tarball(
    tarball: &Utf8Path,
    compression: &CompressionImpl,
) -> crate::error::Result<Vec<u8>> {
    use crate::LocalAsset;

    let source = LocalAsset::load_bytes(tarball)?;
    let mut tarball_bytes = vec![];
    decompress_tarball_bytes(&source, &mut tarball_bytes, compression)
        .map_err(wrap_decompression_err(tarball.as_str()))?;

    Ok(tarball_bytes)
}

#[cfg(feature = "compression-tar")]
fn decompress_tarball_bytes(
    source: &[u8],
    tarball_bytes: &mut Vec<u8>,
    compression: &CompressionImpl,
) -> std::io::Result<()> {
    use std::io::Read;

    use flate2::read::GzDecoder;
    use xz2::read::XzDecoder;
    use zstd::stream::Decoder as ZstdDecoder;

    match compression {
        CompressionImpl::Gzip => {
            let mut decoder = GzDecoder::new(source);
            decoder.read_to_end(tarball_bytes)?;
        }
        CompressionImpl::Xzip => {
            let mut decoder = XzDecoder::new(source);
            decoder.read_to_end(tarball_bytes)?;
        }
        CompressionImpl::Zstd => {
            let mut decoder = ZstdDecoder::new(source)?;
            decoder.read_to_end(tarball_bytes)?;
        }
    }
    Ok(())
}

#[cfg(feature = "compression-tar")]
pub(crate) fn untar_all(
    tarball: &Utf8Path,
    dest_path: &Utf8Path,
    compression: &CompressionImpl,
) -> crate::error::Result<()> {
    let tarball_bytes = open_tarball(tarball, compression)?;
    let mut archive = tar::Archive::new(tarball_bytes.as_slice());
    archive
        .unpack(dest_path)
        .map_err(wrap_decompression_err(tarball.as_str()))?;

    Ok(())
}

#[cfg(feature = "compression-tar")]
pub(crate) fn untar_file(
    tarball: &Utf8Path,
    filename: &str,
    compression: &CompressionImpl,
) -> crate::error::Result<Vec<u8>> {
    let tarball_bytes = open_tarball(tarball, compression)?;
    let archive = tar::Archive::new(tarball_bytes.as_slice());
    let buf = find_tarball_file_bytes(archive, filename)
        .map_err(wrap_decompression_err(tarball.as_str()))?;
    match buf {
        Some(buf) => Ok(buf),
        None => Err(crate::AxoassetError::ExtractFilenameFailed {
            desired_filename: filename.to_owned(),
        }),
    }
}

#[cfg(feature = "compression-tar")]
fn find_tarball_file_bytes(
    mut tarball: tar::Archive<&[u8]>,
    filename: &str,
) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    for entry in tarball.entries()? {
        let mut entry = entry?;
        if let Some(name) = entry.path()?.file_name() {
            if name == filename {
                let mut buf = vec![];
                entry.read_to_end(&mut buf)?;

                return Ok(Some(buf));
            }
        }
    }
    Ok(None)
}

#[cfg(feature = "compression-zip")]
pub(crate) fn zip_dir(
    src_path: &Utf8Path,
    dest_path: &Utf8Path,
    with_root: Option<&Utf8Path>,
) -> crate::error::Result<()> {
    zip_dir_impl(src_path, dest_path, with_root).map_err(|details| AxoassetError::Compression {
        reason: format!("failed to write zip: {}", dest_path),
        details: details.into(),
    })
}

#[cfg(feature = "compression-zip")]
pub(crate) fn zip_dir_impl(
    src_path: &Utf8Path,
    dest_path: &Utf8Path,
    with_root: Option<&Utf8Path>,
) -> zip::result::ZipResult<()> {
    use std::{
        fs::File,
        io::{Read, Write},
    };
    use zip::{write::SimpleFileOptions, CompressionMethod};

    let file = File::create(dest_path)?;

    // The `zip` crate lacks the conveniences of the `tar` crate so we need to manually
    // walk through all the subdirs of `src_path` and copy each entry. walkdir streamlines
    // that process for us.
    let walkdir = crate::dirs::walk_dir(src_path);
    let it = walkdir.into_iter();

    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    // If there's a root prefix, add entries for all of its components
    if let Some(root) = with_root {
        for path in root.ancestors() {
            if !path.as_str().is_empty() {
                zip.add_directory(path.as_str(), options)?;
            }
        }
    }

    let mut buffer = Vec::new();
    for entry in it.filter_map(|e| e.ok()) {
        let name = &entry.rel_path;
        let path = &entry.full_path;
        // Optionally apply the root prefix
        let name = if let Some(root) = with_root {
            root.join(name)
        } else {
            name.to_owned()
        };

        // ZIP files always need Unix-style file separators; we need to
        // convert any Windows file names to use Unix separators before
        // passing them to any of the other functions.
        let unix_name = Utf8PathBuf::from(&name)
            .components()
            .map(|c| c.as_str())
            .collect::<Vec<&str>>()
            .join("/");

        // Write file or directory explicitly
        // Some unzip tools unzip files with directory paths correctly, some do not!
        if path.is_file() {
            // On Unix, we want to make sure we preserve permissions
            // (like the execute bit!) from the source file
            #[cfg(unix)]
            let options = options.unix_permissions(path.metadata()?.permissions().mode());

            zip.start_file(&unix_name, options)?;
            let mut f = File::open(path)?;

            f.read_to_end(&mut buffer)?;
            zip.write_all(&buffer)?;
            buffer.clear();
        } else if !name.as_str().is_empty() {
            // Only if not root! Avoids path spec / warning
            // and mapname conversion failed error on unzip
            zip.add_directory(&unix_name, options)?;
        }
    }
    zip.finish()?;
    Ok(())
}

#[cfg(feature = "compression-zip")]
pub(crate) fn unzip_all(zipfile: &Utf8Path, dest_path: &Utf8Path) -> crate::error::Result<()> {
    use crate::LocalAsset;

    let source = LocalAsset::load_bytes(zipfile)?;
    unzip_all_impl(&source, dest_path).map_err(|details| AxoassetError::Decompression {
        origin_path: zipfile.to_string(),
        details: details.into(),
    })
}

#[cfg(feature = "compression-zip")]
fn unzip_all_impl(source: &[u8], dest_path: &Utf8Path) -> zip::result::ZipResult<()> {
    use std::io::Cursor;

    let seekable = Cursor::new(source);
    let mut archive = zip::ZipArchive::new(seekable)?;
    archive.extract(dest_path)?;
    Ok(())
}

#[cfg(feature = "compression-zip")]
pub(crate) fn unzip_file(zipfile: &Utf8Path, filename: &str) -> crate::error::Result<Vec<u8>> {
    use std::io::{Cursor, Read};

    use crate::LocalAsset;

    let source = LocalAsset::load_bytes(zipfile)?;
    let seekable = Cursor::new(source);
    let mut archive =
        zip::ZipArchive::new(seekable).map_err(|details| AxoassetError::Decompression {
            origin_path: zipfile.to_string(),
            details: details.into(),
        })?;
    let mut file =
        archive
            .by_name(filename)
            .map_err(|_| crate::AxoassetError::ExtractFilenameFailed {
                desired_filename: filename.to_owned(),
            })?;

    let mut buf = vec![];
    file.read_to_end(&mut buf)
        .map_err(wrap_decompression_err(zipfile.as_str()))?;

    Ok(buf)
}

fn wrap_decompression_err(origin_path: &str) -> impl FnOnce(std::io::Error) -> AxoassetError + '_ {
    |details| AxoassetError::Decompression {
        origin_path: origin_path.to_string(),
        details,
    }
}

//! Axoasset Errors

use miette::Diagnostic;
use thiserror::Error;

/// Axoasset Result
pub type Result<T> = std::result::Result<T, AxoassetError>;

/// The set of errors that can occur when axoasset is used
#[derive(Debug, Error, Diagnostic)]
#[non_exhaustive]
pub enum AxoassetError {
    /// This error indicates that axoasset failed to fetch a remote asset.
    #[error("failed to fetch asset at {origin_path}: Encountered an error when requesting a remote asset.")]
    #[diagnostic(help("Make sure the url you provided is accurate."))]
    #[cfg(feature = "remote")]
    RemoteAssetRequestFailed {
        /// The origin path of the asset, used as an identifier
        origin_path: String,
        /// Details of the error
        #[source]
        details: reqwest::Error,
    },

    /// error indicates that the provided URL did not properly parse and may
    /// either be invalid or an unsupported format.
    #[cfg(feature = "remote")]
    #[error("failed to parse URL {origin_path}")]
    UrlParse {
        /// The origin path of the asset, used as an identifier
        origin_path: String,
        /// Details of the error
        #[source]
        details: url::ParseError,
    },

    /// This error indicates that the received headers were not able to be
    /// parsed into a string, which means they may be corrupted in some way.
    #[error("failed to parse header at {origin_path}")]
    #[cfg(feature = "remote")]
    HeaderParse {
        /// The origin path of the asset, used as an identifier
        origin_path: String,
        /// Details of the error
        #[source]
        details: reqwest::header::ToStrError,
    },

    /// This error indicates that the given mime type was not able to be
    /// parsed into a string, which means it may be corrupted in some way.
    #[error(
        "when fetching asset at {origin_path}, the server's response mime type couldn't be parsed"
    )]
    #[cfg(feature = "remote")]
    MimeParse {
        /// The origin path of the asset, used as an identifier
        origin_path: String,
        /// Details of the error
        #[source]
        details: mime::FromStrError,
    },

    /// This error indicates that the mime type of the requested remote asset
    /// was not an image.
    #[error("when fetching asset at {origin_path}, the server's response mime type did not indicate an image.")]
    #[diagnostic(help(
        "Please make sure the asset url is correct and that the server is properly configured."
    ))]
    #[cfg(feature = "remote")]
    RemoteAssetNonImageMimeType {
        /// The origin path of the asset, used as an identifier
        origin_path: String,
    },

    /// This error indicates that the mime type of the requested remote asset
    /// was of a type that axoasset does not support.
    #[error("when fetching asset at {origin_path}, the server responded with a mime type that was non supported")]
    #[diagnostic(help(
        "Please make sure the asset url is correct and that the server is properly configured"
    ))]
    #[cfg(feature = "remote")]
    RemoteAssetMimeTypeNotSupported {
        /// The origin path of the asset, used as an identifier
        origin_path: String,
        /// The mimetype from the server response
        mimetype: String,
    },

    /// This error indicates that the requested remote asset was an image, but
    /// axoasset could not determine what file extension to use for the
    /// received format.
    #[error("when fetching asset at {origin_path}, we could not determine an appropriate file extension based on the server response")]
    #[diagnostic(help(
        "Please make sure the asset url is correct and that the server is properly configured"
    ))]
    #[cfg(feature = "remote")]
    RemoteAssetIndeterminateImageFormatExtension {
        /// The origin path of the asset, used as an identifier
        origin_path: String,
    },

    /// This error indicates that the server response for the remote asset request
    /// did not include a content-type header. Axoasset needs the content-type
    /// header to determine what type of file the asset contains.
    #[error("when fetching asset at {origin_path}, the server's response did not contain a content type header")]
    #[diagnostic(help(
        "Please make sure the asset url is correct and that the server is properly configured"
    ))]
    #[cfg(feature = "remote")]
    RemoteAssetMissingContentTypeHeader {
        /// The origin path of the asset, used as an identifier
        origin_path: String,
    },

    /// This error indicates that axoasset failed to write a remote asset to the
    /// local filesystem.
    #[error("failed to write asset at {origin_url} to {dest_path}: Could not find asset at provided path.")]
    #[diagnostic(help("Make sure your path is correct and your server is configured correctly."))]
    #[cfg(feature = "remote")]
    RemoteAssetWriteFailed {
        /// The origin path of the asset, used as an identifier
        origin_url: crate::remote::UrlString,
        /// The path where the asset was being written to
        dest_path: camino::Utf8PathBuf,
        /// Details of the error
        #[source]
        details: std::io::Error,
    },

    /// This error indicates that axoasset failed to fetch a local asset at the
    /// provided path.
    #[error("failed to fetch asset at {origin_path}: Could not find asset at provided path.")]
    LocalAssetNotFound {
        /// The origin path of the asset, used as an identifier
        origin_path: String,
        /// Details of the error
        #[source]
        details: std::io::Error,
    },

    /// This error inidcates that axoasset failed to copy a local asset.
    #[error("failed to copy asset from {origin_path} to {dest_path}")]
    LocalAssetCopyFailed {
        /// The origin path of the asset, used as an identifier
        origin_path: String,
        /// The path where the asset was being copied to
        dest_path: String,
        /// Details of the error
        #[source]
        details: std::io::Error,
    },

    /// This error indicates that axoasset failed to read a local asset at the
    /// provided path.
    #[error("failed to read asset from {origin_path}")]
    LocalAssetReadFailed {
        /// The origin path of the asset, used as an identifier
        origin_path: String,
        /// Details of the error
        #[source]
        details: std::io::Error,
    },

    /// This error indicates that axoasset failed to write a local asset.
    #[error("failed to write asset from {origin_path} to {dest_path}.")]
    LocalAssetWriteFailed {
        /// The origin path of the asset, used as an identifier
        origin_path: String,
        /// The path where the asset was being written to
        dest_path: String,
        /// Details of the error
        #[source]
        details: std::io::Error,
    },

    /// This error indicates that axoasset failed to write a new asset
    #[error("failed to write a new asset to {dest_path}.")]
    #[diagnostic(help("Make sure you have the correct permissions to create a new file."))]
    LocalAssetWriteNewFailed {
        /// The path where the asset was being written to
        dest_path: String,
        /// Details of the error
        #[source]
        details: std::io::Error,
    },

    /// This error indicates that axoasset failed to create a new directory
    #[error("failed to write a new directory to {dest_path}.")]
    #[diagnostic(help("Make sure you have the correct permissions to create a new directory."))]
    LocalAssetDirCreationFailed {
        /// The path where the directory was meant to be created
        dest_path: String,
        /// Details of the error
        #[source]
        details: std::io::Error,
    },

    /// This error indicates that axoasset failed to delete an asset
    #[error("failed to delete asset at {dest_path}.")]
    LocalAssetRemoveFailed {
        /// The path that was going to be deleted
        dest_path: String,
        /// Details of the error
        #[source]
        details: std::io::Error,
    },

    /// This error indicates that axoasset could not determine the filename for
    /// a local asset.
    #[error("could not determine file name for asset at {origin_path}")]
    LocalAssetMissingFilename {
        /// The origin path of the asset, used as an identifier
        origin_path: String,
    },

    /// This error indicates we ran into an issue when creating an archive.
    #[error("failed to create archive: {reason}")]
    Compression {
        /// A specific step that failed
        reason: String,
        /// Details of the error
        #[source]
        details: std::io::Error,
    },

    /// Some error decompressing a tarball/zip
    #[cfg(any(feature = "compression-zip", feature = "compression-tar"))]
    #[error("Failed to extract archive {origin_path}")]
    Decompression {
        /// The origin path of the asset, used as an identifier
        origin_path: String,
        /// Details of the error
        #[source]
        details: std::io::Error,
    },

    /// This error indicates we ran `std::env::current_dir` and somehow got an error.
    #[error("Failed to get the current working directory")]
    CurrentDir {
        /// Details of the error
        #[source]
        details: std::io::Error,
    },
    /// This error indicates we failed to convert a Path/PathBuf to a Utf8Path/Utf8PathBuf
    #[error("This path isn't utf8: {path:?}")]
    Utf8Path {
        /// The problematic path
        path: std::path::PathBuf,
    },
    /// This error indicates we tried to strip_prefix a path that should have been
    /// a descendant of another, but it didn't work.
    #[error("Child wasn't nested under its parent: {root_dir} => {child_dir}")]
    #[diagnostic(help("Are symlinks involved?"))]
    PathNesting {
        /// The root/ancestor dir
        root_dir: camino::Utf8PathBuf,
        /// THe child/descendent path
        child_dir: camino::Utf8PathBuf,
    },

    #[error("Failed to find {desired_filename} in an ancestor of {start_dir}")]
    /// This error indicates we failed to find the desired file in an ancestor of the search dir.
    SearchFailed {
        /// The dir we started the search in
        start_dir: camino::Utf8PathBuf,
        /// The filename we were searching for
        desired_filename: String,
    },

    #[error("Failed to find {desired_filename} within archive being decompressed")]
    /// This error indicates we failed to find the desired file within a tarball or zip
    ExtractFilenameFailed {
        /// The filename we were searching for
        desired_filename: String,
    },

    #[error("Failed to walk to ancestor of {origin_path}")]
    /// Walkdir failed to yield an entry
    WalkDirFailed {
        /// The root path we were trying to walkdirs
        origin_path: camino::Utf8PathBuf,
        /// Inner walkdir error
        #[source]
        details: walkdir::Error,
    },

    /// This error indicates we tried to deserialize some JSON with serde_json
    /// but failed.
    #[cfg(feature = "json-serde")]
    #[error("failed to parse JSON")]
    Json {
        /// The SourceFile we were try to parse
        #[source_code]
        source: crate::SourceFile,
        /// The range the error was found on
        #[label]
        span: Option<miette::SourceSpan>,
        /// Details of the error
        #[source]
        details: serde_json::Error,
    },

    /// This error indicates we tried to deserialize some TOML with toml-rs (serde)
    /// but failed.
    #[cfg(feature = "toml-serde")]
    #[error("failed to parse TOML")]
    Toml {
        /// The SourceFile we were try to parse
        #[source_code]
        source: crate::SourceFile,
        /// The range the error was found on
        #[label]
        span: Option<miette::SourceSpan>,
        /// Details of the error
        #[source]
        details: toml::de::Error,
    },

    /// This error indicates we tried to deserialize some TOML with toml_edit
    /// but failed.
    #[cfg(feature = "toml-edit")]
    #[error("failed to edit TOML document")]
    TomlEdit {
        /// The SourceFile we were trying to parse
        #[source_code]
        source: crate::SourceFile,
        /// The range the error was found on
        #[label]
        span: Option<miette::SourceSpan>,
        /// Details of the error
        #[source]
        details: toml_edit::TomlError,
    },

    /// This error indicates we tried to deserialize some YAML with serde_yml
    /// but failed.
    #[cfg(feature = "yaml-serde")]
    #[error("failed to parse YAML")]
    Yaml {
        /// The SourceFile we were try to parse
        #[source_code]
        source: crate::SourceFile,
        /// The range the error was found on
        #[label]
        span: Option<miette::SourceSpan>,
        /// Details of the error
        #[source]
        details: serde_yaml_bw::Error,
    },
}

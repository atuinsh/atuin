#![deny(missing_docs)]
#![allow(clippy::result_large_err)]

//! # axoasset
//! > 📮 load, write, and copy remote and local assets
//!
//! this library is a utility focused on managing both local (filesystem) assets
//! and remote (via http/https) assets. the bulk of the logic is not terribly
//! interesting or uniquely engineered; the purpose this library is primarily
//! to unify and co-locate the logic to make debugging simpler and error handling
//! more consistent and comprehensive.

#[cfg(any(feature = "compression-zip", feature = "compression-tar"))]
pub(crate) mod compression;
pub(crate) mod dirs;
pub mod error;
pub mod local;
#[cfg(feature = "remote")]
pub mod remote;
pub mod source;
pub mod spanned;

pub use error::AxoassetError;
pub use local::LocalAsset;
#[cfg(feature = "remote")]
pub use remote::AxoClient;
// Simplifies raw access to reqwest without depending on a separate copy
#[cfg(feature = "remote")]
pub use reqwest;
#[cfg(feature = "json-serde")]
pub use serde_json;
#[cfg(feature = "yaml-serde")]
pub use serde_yaml_bw;
pub use source::SourceFile;
pub use spanned::Spanned;
#[cfg(feature = "toml-serde")]
pub use toml;
#[cfg(feature = "toml-edit")]
pub use toml_edit;

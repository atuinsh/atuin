//! Example that makes it easy to mess around with the compression backend
//!
//! ```ignore
//! cargo run --example compress --features=compression -- src src.tar.gz --with-root=some/dir
//! ```
//!
//! ```ignore
//! cargo run --example compress --features=compression -- src src.zip --with-root=some/dir
//! ```
#![allow(unused_imports)]
#![allow(unused_variables)]

use axoasset::{AxoassetError, LocalAsset};
use camino::Utf8PathBuf;
use clap::Parser;

#[derive(Parser)]
struct Cli {
    src_path: Utf8PathBuf,
    dest_path: Utf8PathBuf,
    #[clap(long)]
    with_root: Option<Utf8PathBuf>,
}

fn main() {
    let args = Cli::parse();

    doit(args).unwrap()
}

fn doit(args: Cli) -> Result<(), Box<AxoassetError>> {
    #[cfg(feature = "compression-tar")]
    if args.dest_path.as_str().ends_with("tar.zstd") {
        return Ok(LocalAsset::tar_zstd_dir(
            args.src_path,
            args.dest_path,
            args.with_root,
        )?);
    }
    #[cfg(feature = "compression-tar")]
    if args.dest_path.as_str().ends_with("tar.xz") {
        return Ok(LocalAsset::tar_xz_dir(
            args.src_path,
            args.dest_path,
            args.with_root,
        )?);
    }
    #[cfg(feature = "compression-tar")]
    if args.dest_path.as_str().ends_with("tar.gz") {
        return Ok(LocalAsset::tar_gz_dir(
            args.src_path,
            args.dest_path,
            args.with_root,
        )?);
    }
    #[cfg(feature = "compression-zip")]
    if args.dest_path.as_str().ends_with("zip") {
        return Ok(LocalAsset::zip_dir(
            args.src_path,
            args.dest_path,
            args.with_root,
        )?);
    }

    if !cfg!(any(
        feature = "compression-tar",
        feature = "compression-zip"
    )) {
        panic!("this example must be built with --features=compression")
    } else {
        panic!("unsupported dest_path extension")
    }
}

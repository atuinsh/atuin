//! Writes `openapi.json`, prepared for the client (see `build/prepare.rs`), to
//! `$OUT_DIR/openapi.json`, where `src/lib.rs`'s `generate_api!` reads it.

use std::path::{Path, PathBuf};
use std::{env, fs, io};

#[path = "build/prepare.rs"]
mod prepare;

#[derive(Debug, thiserror::Error)]
enum BuildError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("openapi.json is not JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Spec(#[from] prepare::SpecError),
}

fn main() {
    println!("cargo:rerun-if-changed=openapi.json");
    println!("cargo:rerun-if-changed=build");
    // A logged error fails the build with the message alone; a non-zero exit would add a dump of
    // this script's output.
    if let Err(err) = run() {
        println!("cargo::error={err}");
    }
}

fn run() -> Result<(), BuildError> {
    let spec_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
    let out_path =
        PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("openapi.json");

    let spec = fs::read(&spec_path).map_err(|source| BuildError::Io {
        path: spec_path,
        source,
    })?;
    let mut spec = serde_json::from_slice(&spec)?;
    prepare::strip(&mut spec)?;
    fs::write(&out_path, serde_json::to_vec(&spec)?).map_err(|source| BuildError::Io {
        path: out_path,
        source,
    })
}

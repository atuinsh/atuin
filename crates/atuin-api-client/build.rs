//! Generates the client from `openapi.json` into `$OUT_DIR/generated.rs`, which `src/lib.rs`
//! includes.

use std::path::{Path, PathBuf};
use std::{env, fs, io};

#[path = "build/generate.rs"]
mod generate;
#[path = "build/mapping.rs"]
mod mapping;
#[path = "build/prepare.rs"]
mod prepare;

use generate::GenerateError;

#[derive(Debug, thiserror::Error)]
enum BuildError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Generate(#[from] GenerateError),
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
        PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("generated.rs");

    let spec = fs::read(&spec_path).map_err(|source| BuildError::Io {
        path: spec_path,
        source,
    })?;
    let code = generate::generate(&spec)?;
    fs::write(&out_path, code).map_err(|source| BuildError::Io {
        path: out_path,
        source,
    })
}

//! Generates the sync server client from `crates/atuin-client/openapi.json`.
//!
//! The spec is itself generated from `atuin-server`; see README.md.

use std::{env, fs, path::PathBuf};

use progenitor::{GenerationSettings, Generator, InterfaceStyle, TagStyle};

const SPEC: &str = "../atuin-client/openapi.json";

fn main() {
    println!("cargo:rerun-if-changed={SPEC}");
    println!("cargo:rerun-if-changed=build.rs");

    let spec = fs::File::open(SPEC).expect("failed to open openapi.json");
    let spec = serde_json::from_reader(spec).expect("failed to parse openapi.json");

    let mut generator = Generator::new(
        GenerationSettings::default()
            .with_interface(InterfaceStyle::Positional)
            .with_tag(TagStyle::Merged),
    );

    let tokens = generator
        .generate_tokens(&spec)
        .expect("failed to generate client");
    let ast = syn::parse2(tokens).expect("generated code did not parse");

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set")).join("codegen.rs");
    fs::write(out, prettyplease::unparse(&ast)).expect("failed to write generated client");
}

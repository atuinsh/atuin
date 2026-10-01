//! The generator `build.rs` runs, compiled again so its unit tests run under `cargo test`.

#![warn(clippy::pedantic, clippy::nursery)]

#[path = "../build/generate.rs"]
mod generate;
#[path = "../build/mapping.rs"]
mod mapping;
#[path = "../build/prepare.rs"]
mod prepare;

//! `build_const`: crate for creating constants in your build script
//!
//! The build_const crate exists to help create rust constant files at compile time or in a
//! generating script. It is ultra simple and lightweight, making constant creation a simple
//! matter.
//!
//! Recommended use: when developing make your constants in `build.rs`. Once your constants are
//! fairly stable create a script instead and have your constants file be generated in either a
//! single file or an external crate that you can bring in as a dependency.
//!
//! # Example
//!
//! Include `build_const = VERSION` in your `Cargo.toml` file. For `no_std` support (macros only)
//! use `default-features = false`.
//!
//! See `ConstWriter` for how to use in a build.rs or script. To then import a "constants.rs" file
//! created in `build.rs` use:
//!
//! ```c
//! #[macro_use]
//! extern crate build_const;
//!
//! build_const!("constants");
//! println!("VALUE: {}", VALUE);
//! println!("VALUE: {}", ARRAY);
//! ```
//!
//! For writing constants in a script, the macro `src_file!` is also provided.
//! ```c
//! // will write files to `/src/constants.rs`
//! let mut consts = ConstWriter::from_path(&Path::from(src_file!("constants.rs"))).unwrap();
//! // ... use consts
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
mod writer;

#[cfg(feature = "std")]
pub use writer::{
    ConstWriter,
    ConstValueWriter,
    write_array,
    write_array_raw,
};

/// Shortcut macro which expands to the same module path used in
/// `ConstWriter::for_build(mod_name)`.
///
/// If you don't want to include macros, this is simply a one liner:
/// ```ignore
/// include!(concat!(env!("OUT_DIR"), concat!("/", $mod_name)));
/// ```
#[macro_export]
macro_rules! build_const {
    ( $mod_name:expr ) => {
        include!(
            concat!(
                env!("OUT_DIR"),
                concat!("/", concat!($mod_name, ".rs"))
            )
        );
    };
}

/// Macro which returns the path to file in your `src/` directory.
///
/// Example:
/// ```ignore
/// src_file!("constants.rs");
/// ```
/// returns `/path/to/project/src/constants.rs`
///
/// If you need a more custom path, the basic implementation is:
/// ```ignore
/// concat!(env!("CARGO_MANIFEST_DIR"), "/src/path/to/file")
/// ```
#[macro_export]
macro_rules! src_file {
    ( $file_name:expr) => {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            concat!("/", concat!("src", concat!("/", $file_name)))
        )
    };
}


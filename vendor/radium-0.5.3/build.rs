//! Target detection
//!
//! This build script translates the target for which `radium` is being compiled
//! into a set of directives that the crate can use to control which atomic
//! symbols it attempts to name.
//!
//! The compiler maintains its store of target information here:
//! <https://github.com/rust-lang/rust/tree/be28b6235e64e0f662b96b710bf3af9de169215c/compiler/rustc_target/src/spec>
//!
//! That module is not easily extracted into something that can be loaded here,
//! so we are replicating it through string matching on the target name until
//! we are able to uniquely identify targets through `cfg` checks.
//!
//! Use `rustc --print target-list` to enumerate the full list of targets
//! available, and `rustc --print cfg` (optionally with `-Z unstable-options`)
//! to see what `cfg` values are produced for a target.
//!
//! The missing `cfg` checks required for conditional compilation, rather than a
//! build script, are:
//!
//! - [`accessible`](https://github.com/rust-lang/rust/issues/64797)
//! - [`target_feature`](https://github.com/rust-lang/rust/issues/69098)
//! - [`target_has_atomic`](https://github.com/rust-lang/rust/issues/32976)
//!
//! Once any of these becomes usable on the stable series, we can switch to a
//! set of `cfg` checks instead of a build script.

/// Collection of flags indicating whether the target processor supports atomic
/// instructions for a certain width.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct Atomics {
    /// Target supports 8-bit atomics
    has_8: bool,
    /// Target supports 16-bit atomics
    has_16: bool,
    /// Target supports 32-bit atomics
    has_32: bool,
    /// Target supports 64-bit atomics
    has_64: bool,
    /// Target supports word-width atomics
    has_ptr: bool,
}

impl Atomics {
    const ALL: Self = Self {
        has_8: true,
        has_16: true,
        has_32: true,
        has_64: true,
        has_ptr: true,
    };
    const NONE: Self = Self {
        has_8: false,
        has_16: false,
        has_32: false,
        has_64: false,
        has_ptr: false,
    };
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut atomics = Atomics::ALL;

    let target = std::env::var("TARGET")?;
    // Add new target strings here with their atomic availability.
    #[allow(clippy::match_single_binding, clippy::single_match)]
    match &*target {
        "arm-linux-androideabi" => atomics.has_64 = false,
        _ => {}
    }

    let arch = target.split('-').next().ok_or("Invalid target triple")?;
    // Add new architecture sections here with their atomic availability.
    #[allow(clippy::match_single_binding, clippy::single_match)]
    match arch {
        // "riscv32imc-unknown-none-elf" and "riscv32imac-unknown-none-elf" are
        // both `target_arch = "riscv32", and have no stable `cfg`-discoverable
        // distinction. As such, the atomic RISC-V target must be discovered
        // here.
        "armv5te" | "mips" | "mipsel" | "powerpc" | "riscv32imac" | "thumbv7em" | "thumbv7m" => {
            atomics.has_64 = false
        }
        "riscv32i" | "riscv32imc" | "thumbv6m" => atomics = Atomics::NONE,
        _ => {}
    }

    if atomics.has_8 {
        println!("cargo:rustc-cfg=radium_atomic_8");
    }
    if atomics.has_16 {
        println!("cargo:rustc-cfg=radium_atomic_16");
    }
    if atomics.has_32 {
        println!("cargo:rustc-cfg=radium_atomic_32");
    }
    if atomics.has_64 {
        println!("cargo:rustc-cfg=radium_atomic_64");
    }
    if atomics.has_ptr {
        println!("cargo:rustc-cfg=radium_atomic_ptr");
    }

    Ok(())
}

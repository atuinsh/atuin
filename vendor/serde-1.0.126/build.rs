use std::env;
use std::process::Command;
use std::str::{self, FromStr};

// The rustc-cfg strings below are *not* public API. Please let us know by
// opening a GitHub issue if your build environment requires some way to enable
// these cfgs other than by executing our build script.
fn main() {
    let minor = match rustc_minor_version() {
        Some(minor) => minor,
        None => return,
    };

    let target = env::var("TARGET").unwrap();
    let emscripten = target == "asmjs-unknown-emscripten" || target == "wasm32-unknown-emscripten";

    // std::collections::Bound was stabilized in Rust 1.17
    // but it was moved to core::ops later in Rust 1.26:
    // https://doc.rust-lang.org/core/ops/enum.Bound.html
    if minor >= 26 {
        println!("cargo:rustc-cfg=ops_bound");
    } else if minor >= 17 && cfg!(feature = "std") {
        println!("cargo:rustc-cfg=collections_bound");
    }

    // core::cmp::Reverse stabilized in Rust 1.19:
    // https://doc.rust-lang.org/stable/core/cmp/struct.Reverse.html
    if minor >= 19 {
        println!("cargo:rustc-cfg=core_reverse");
    }

    // CString::into_boxed_c_str and PathBuf::into_boxed_path stabilized in Rust 1.20:
    // https://doc.rust-lang.org/std/ffi/struct.CString.html#method.into_boxed_c_str
    // https://doc.rust-lang.org/std/path/struct.PathBuf.html#method.into_boxed_path
    if minor >= 20 {
        println!("cargo:rustc-cfg=de_boxed_c_str");
        println!("cargo:rustc-cfg=de_boxed_path");
    }

    // From<Box<T>> for Rc<T> / Arc<T> stabilized in Rust 1.21:
    // https://doc.rust-lang.org/std/rc/struct.Rc.html#impl-From<Box<T>>
    // https://doc.rust-lang.org/std/sync/struct.Arc.html#impl-From<Box<T>>
    if minor >= 21 {
        println!("cargo:rustc-cfg=de_rc_dst");
    }

    // Duration available in core since Rust 1.25:
    // https://blog.rust-lang.org/2018/03/29/Rust-1.25.html#library-stabilizations
    if minor >= 25 {
        println!("cargo:rustc-cfg=core_duration");
    }

    // 128-bit integers stabilized in Rust 1.26:
    // https://blog.rust-lang.org/2018/05/10/Rust-1.26.html
    //
    // Disabled on Emscripten targets as Emscripten doesn't
    // currently support integers larger than 64 bits.
    if minor >= 26 && !emscripten {
        println!("cargo:rustc-cfg=integer128");
    }

    // Inclusive ranges methods stabilized in Rust 1.27:
    // https://github.com/rust-lang/rust/pull/50758
    if minor >= 27 {
        println!("cargo:rustc-cfg=range_inclusive");
    }

    // Non-zero integers stabilized in Rust 1.28:
    // https://blog.rust-lang.org/2018/08/02/Rust-1.28.html#library-stabilizations
    if minor >= 28 {
        println!("cargo:rustc-cfg=num_nonzero");
    }

    // Current minimum supported version of serde_derive crate is Rust 1.31.
    if minor >= 31 {
        println!("cargo:rustc-cfg=serde_derive");
    }

    // TryFrom, Atomic types, non-zero signed integers, and SystemTime::checked_add
    // stabilized in Rust 1.34:
    // https://blog.rust-lang.org/2019/04/11/Rust-1.34.0.html#tryfrom-and-tryinto
    // https://blog.rust-lang.org/2019/04/11/Rust-1.34.0.html#library-stabilizations
    if minor >= 34 {
        println!("cargo:rustc-cfg=core_try_from");
        println!("cargo:rustc-cfg=num_nonzero_signed");
        println!("cargo:rustc-cfg=systemtime_checked_add");

        // Whitelist of archs that support std::sync::atomic module. Ideally we
        // would use #[cfg(target_has_atomic = "...")] but it is not stable yet.
        // Instead this is based on rustc's src/librustc_target/spec/*.rs.
        let has_atomic64 = target.starts_with("x86_64")
            || target.starts_with("i686")
            || target.starts_with("aarch64")
            || target.starts_with("powerpc64")
            || target.starts_with("sparc64")
            || target.starts_with("mips64el");
        let has_atomic32 = has_atomic64 || emscripten;
        if has_atomic64 {
            println!("cargo:rustc-cfg=std_atomic64");
        }
        if has_atomic32 {
            println!("cargo:rustc-cfg=std_atomic");
        }
    }
}

fn rustc_minor_version() -> Option<u32> {
    let rustc = match env::var_os("RUSTC") {
        Some(rustc) => rustc,
        None => return None,
    };

    let output = match Command::new(rustc).arg("--version").output() {
        Ok(output) => output,
        Err(_) => return None,
    };

    let version = match str::from_utf8(&output.stdout) {
        Ok(version) => version,
        Err(_) => return None,
    };

    let mut pieces = version.split('.');
    if pieces.next() != Some("rustc 1") {
        return None;
    }

    let next = match pieces.next() {
        Some(next) => next,
        None => return None,
    };

    u32::from_str(next).ok()
}

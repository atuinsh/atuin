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

    // 128-bit integers disabled on Emscripten targets as Emscripten doesn't
    // currently support integers larger than 64 bits.
    if !emscripten {
        println!("cargo:rustc-cfg=integer128");
    }

    // MaybeUninit<T> stabilized in Rust 1.36:
    // https://blog.rust-lang.org/2019/07/04/Rust-1.36.0.html
    if minor >= 36 {
        println!("cargo:rustc-cfg=maybe_uninit");
    }
}

fn rustc_minor_version() -> Option<u32> {
    let rustc = env::var_os("RUSTC")?;
    let output = Command::new(rustc).arg("--version").output().ok()?;
    let version = str::from_utf8(&output.stdout).ok()?;
    let mut pieces = version.split('.');
    if pieces.next() != Some("rustc 1") {
        return None;
    }
    let next = pieces.next()?;
    u32::from_str(next).ok()
}

use std::env;
use std::process::Command;
use std::str::{self, FromStr};

fn main(){
    let minor = match rustc_minor_version() {
        Some(minor) => minor,
        None => return,
    };

    // const fn stabilized in Rust 1.31:
    if minor >= 31 {
        println!("cargo:rustc-cfg=bitflags_const_fn");
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
#![deny(warnings)]

use std::env;

fn main() {
    let target = env::var("TARGET").expect("TARGET was not set");
    if target.contains("-uwp-windows-") {
        // for BCryptGenRandom
        println!("cargo:rustc-link-lib=bcrypt");
        // to work around unavailability of `target_vendor` on Rust 1.33
        println!("cargo:rustc-cfg=getrandom_uwp");
    } else if target.contains("windows") {
        // for RtlGenRandom (aka SystemFunction036)
        println!("cargo:rustc-link-lib=advapi32");
    } else if target.contains("apple-ios") {
        // for SecRandomCopyBytes and kSecRandomDefault
        println!("cargo:rustc-link-lib=framework=Security");
    }
}

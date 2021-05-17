#![deny(warnings)]

use std::env;

fn main() {
    let target = env::var("TARGET").expect("TARGET was not set");
    if target.contains("windows") {
        // for BCryptGenRandom
        println!("cargo:rustc-link-lib=bcrypt");
    } else if target.contains("apple-ios") {
        // for SecRandomCopyBytes and kSecRandomDefault
        println!("cargo:rustc-link-lib=framework=Security");
    }
}

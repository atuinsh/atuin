extern crate autocfg;

use std::env;

fn main() {
    let ac = autocfg::new();

    if ac.probe_type("i128") {
        println!("cargo:rustc-cfg=has_i128");
    } else if env::var_os("CARGO_FEATURE_I128").is_some() {
        panic!("i128 support was not detected!");
    }

    // autocfg doesn't have a direct way to probe for `const fn` yet.
    if ac.probe_rustc_version(1, 31) {
        autocfg::emit("has_const_fn");
    }

    autocfg::rerun_path("build.rs");
}

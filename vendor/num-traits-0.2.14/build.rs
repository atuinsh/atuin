extern crate autocfg;

use std::env;

fn main() {
    let ac = autocfg::new();

    // If the "i128" feature is explicity requested, don't bother probing for it.
    // It will still cause a build error if that was set improperly.
    if env::var_os("CARGO_FEATURE_I128").is_some() || ac.probe_type("i128") {
        autocfg::emit("has_i128");
    }

    ac.emit_expression_cfg(
        "unsafe { 1f64.to_int_unchecked::<i32>() }",
        "has_to_int_unchecked",
    );

    autocfg::rerun_path("build.rs");
}

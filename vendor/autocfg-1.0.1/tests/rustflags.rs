extern crate autocfg;

use std::env;

/// Tests that autocfg uses the RUSTFLAGS environment variable when running
/// rustc.
#[test]
fn test_with_sysroot() {
    // Use the same path as this test binary.
    let dir = env::current_exe().unwrap().parent().unwrap().to_path_buf();
    env::set_var("RUSTFLAGS", &format!("-L {}", dir.display()));
    env::set_var("OUT_DIR", &format!("{}", dir.display()));

    // Ensure HOST != TARGET.
    env::set_var("HOST", "lol");

    let ac = autocfg::AutoCfg::new().unwrap();
    assert!(ac.probe_sysroot_crate("autocfg"));
}

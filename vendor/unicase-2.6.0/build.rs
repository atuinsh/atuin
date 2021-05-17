extern crate version_check as rustc;

fn main() {
    if is_rustc_at_least("1.5.0") {
        println!("cargo:rustc-cfg=__unicase__iter_cmp");
    }

    if is_rustc_at_least("1.13.0") {
        println!("cargo:rustc-cfg=__unicase__default_hasher");
    }

    if is_rustc_at_least("1.31.0") {
        println!("cargo:rustc-cfg=__unicase__const_fns");
    }

    if is_rustc_at_least("1.36.0") {
        println!("cargo:rustc-cfg=__unicase__core_and_alloc");
    }
}

fn is_rustc_at_least(v: &str) -> bool {
    rustc::is_min_version(v).unwrap_or(true)
}

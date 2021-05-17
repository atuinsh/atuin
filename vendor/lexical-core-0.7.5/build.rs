fn main() {
    // TARGET
    // ------

    // We need to optimize limb size for performance.
    // Only have optimized 64-bit instructions on certain architectures.
    // See `lexical-core/src/atof/algorithm/math.rs` for detailed
    // instructions of architecture instruction support for 64-bit
    // mathematical operations.
    let limb_width_64 = cfg!(any(
        target_arch = "aarch64",
        target_arch = "mips64",
        target_arch = "powerpc64",
        target_arch = "x86_64"
    ));
    if limb_width_64 {
        println!("cargo:rustc-cfg=limb_width_64");
    } else {
        println!("cargo:rustc-cfg=limb_width_32");
    }
}

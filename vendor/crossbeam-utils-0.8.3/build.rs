use autocfg::AutoCfg;

// The rustc-cfg strings below are *not* public API. Please let us know by
// opening a GitHub issue if your build environment requires some way to enable
// these cfgs other than by executing our build script.
fn main() {
    let cfg = match AutoCfg::new() {
        Ok(cfg) => cfg,
        Err(e) => {
            println!(
                "cargo:warning=crossbeam-utils: unable to determine rustc version: {}",
                e
            );
            return;
        }
    };

    cfg.emit_type_cfg("core::sync::atomic::AtomicU8", "has_atomic_u8");
    cfg.emit_type_cfg("core::sync::atomic::AtomicU16", "has_atomic_u16");
    cfg.emit_type_cfg("core::sync::atomic::AtomicU32", "has_atomic_u32");
    cfg.emit_type_cfg("core::sync::atomic::AtomicU64", "has_atomic_u64");
    cfg.emit_type_cfg("core::sync::atomic::AtomicU128", "has_atomic_u128");
}

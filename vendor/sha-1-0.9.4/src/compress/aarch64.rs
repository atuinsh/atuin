#![cfg(feature = "asm-aarch64")]
use libc::{getauxval, AT_HWCAP, HWCAP_SHA1};

fn sha1_supported() -> bool {
    #[allow(unsafe_code)]
    let hwcaps: u64 = unsafe { getauxval(AT_HWCAP) };
    (hwcaps & HWCAP_SHA1) != 0
}

pub fn compress(state: &mut [u32; 5], blocks: &[u8; 64]) {
    // TODO: Replace this platform-specific call with is_aarch64_feature_detected!("sha1") once
    // that macro is stabilised and https://github.com/rust-lang/rfcs/pull/2725 is implemented
    // to let us use it on no_std.
    if sha1_supported() {
        for block in blocks {
            sha1_asm::compress(state, block);
        }
    } else {
        super::soft::compress(state, blocks);
    }
}

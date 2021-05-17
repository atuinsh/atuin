use libc::{getauxval, AT_HWCAP, HWCAP_SHA2};

#[inline(always)]
pub fn sha2_supported() -> bool {
    let hwcaps: u64 = unsafe { getauxval(AT_HWCAP) };
    (hwcaps & HWCAP_SHA2) != 0
}

pub fn compress(state: &mut [u32; 8], blocks: &[[u8; 64]]) {
    // TODO: Replace with https://github.com/rust-lang/rfcs/pull/2725
    // after stabilization
    if sha2_supported() {
        for block in blocks {
            sha2_asm::compress256(state, block);
        }
    } else {
        super::soft::compress(state, blocks);
    }
}

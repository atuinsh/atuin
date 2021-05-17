// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use libc;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
extern {
    pub fn rust_crypto_util_supports_aesni() -> u32;
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn supports_aesni() -> bool {
    unsafe {
        rust_crypto_util_supports_aesni() != 0
    }
}

extern {
    pub fn rust_crypto_util_fixed_time_eq_asm(
            lhsp: *const u8,
            rhsp: *const u8,
            count: libc::size_t) -> u32;
    pub fn rust_crypto_util_secure_memset(
            dst: *mut u8,
            val: libc::uint8_t,
            count: libc::size_t);
}

pub fn secure_memset(dst: &mut [u8], val: u8) {
    unsafe {
        rust_crypto_util_secure_memset(
            dst.as_mut_ptr(),
            val,
            dst.len() as libc::size_t);
    }
}

/// Compare two vectors using a fixed number of operations. If the two vectors are not of equal
/// length, the function returns false immediately.
pub fn fixed_time_eq(lhs: &[u8], rhs: &[u8]) -> bool {
    if lhs.len() != rhs.len() {
        false
    } else {
        let count = lhs.len() as libc::size_t;

        unsafe {
            let lhsp = lhs.get_unchecked(0);
            let rhsp = rhs.get_unchecked(0);
            rust_crypto_util_fixed_time_eq_asm(lhsp, rhsp, count) == 0
        }
    }
}

#[cfg(test)]
mod test {
    use util::fixed_time_eq;

    #[test]
    pub fn test_fixed_time_eq() {
        let a = [0, 1, 2];
        let b = [0, 1, 2];
        let c = [0, 1, 9];
        let d = [9, 1, 2];
        let e = [2, 1, 0];
        let f = [2, 2, 2];
        let g = [0, 0, 0];

        assert!(fixed_time_eq(&a, &a));
        assert!(fixed_time_eq(&a, &b));

        assert!(!fixed_time_eq(&a, &c));
        assert!(!fixed_time_eq(&a, &d));
        assert!(!fixed_time_eq(&a, &e));
        assert!(!fixed_time_eq(&a, &f));
        assert!(!fixed_time_eq(&a, &g));
    }
}

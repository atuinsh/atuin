

/*
pub fn slice_eq(a: &[u8], b: &[u8]) -> bool {
    // NOTE: In theory n should be libc::size_t and not usize, but libc is not available here
    #[allow(improper_ctypes)]
    extern { fn memcmp(s1: *const u8, s2: *const u8, n: usize) -> i32; }
    a.len() == b.len() && unsafe {
        memcmp(a.as_ptr(),
               b.as_ptr(),
               a.len()) == 0
    }
}
*/

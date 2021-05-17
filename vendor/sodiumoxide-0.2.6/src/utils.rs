//! Libsodium utility functions
use ffi;

/// `memzero()` tries to effectively zero out the data in `x` even if
/// optimizations are being applied to the code.
pub fn memzero(x: &mut [u8]) {
    unsafe {
        ffi::sodium_memzero(x.as_mut_ptr() as *mut _, x.len());
    }
}

/// `memcmp()` returns true if `x[0]`, `x[1]`, ..., `x[len-1]` are the
/// same as `y[0]`, `y[1]`, ..., `y[len-1]`. Otherwise it returns `false`.
///
/// This function is safe to use for secrets `x[0]`, `x[1]`, ..., `x[len-1]`,
/// `y[0]`, `y[1]`, ..., `y[len-1]`. The time taken by `memcmp` is independent
/// of the contents of `x[0]`, `x[1]`, ..., `x[len-1]`, `y[0]`, `y[1]`, ..., `y[len-1]`.
/// In contrast, the standard C comparison function `memcmp(x,y,len)` takes time
/// that depends on the longest matching prefix of `x` and `y`, often allowing easy
/// timing attacks.
pub fn memcmp(x: &[u8], y: &[u8]) -> bool {
    if x.len() != y.len() {
        return false;
    }
    unsafe { ffi::sodium_memcmp(x.as_ptr() as *const _, y.as_ptr() as *const _, x.len()) == 0 }
}

/// `mlock()` locks memory given region which can help avoiding swapping the
/// sensitive memory region to disk.
///
/// Operating system might limit the amount of memory a process can `mlock()`.
/// This function can fail if `mlock()` fails to lock the memory.
pub fn mlock(x: &mut [u8]) -> Result<(), ()> {
    let ret = unsafe { ffi::sodium_mlock(x.as_mut_ptr() as *mut _, x.len()) };
    if ret == 0 {
        Ok(())
    } else {
        Err(())
    }
}

/// `munlock()` unlocks memory region.
///
/// `munlock()` overwrites the region with zeros before unlocking it, so it
/// doesn't have to be done before calling this function.
pub fn munlock(x: &mut [u8]) -> Result<(), ()> {
    let ret = unsafe {
        // sodium_munlock() internally calls sodium_memzero() to clear memory
        // region.
        ffi::sodium_munlock(x.as_mut_ptr() as *mut _, x.len())
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(())
    }
}

/// `increment_le()` treats `x` as an unsigned little-endian number and increments it in
/// constant time.
///
/// WARNING: this method does not check for arithmetic overflow. When used for incrementing
/// nonces it is the caller's responsibility to ensure that any given nonce value
/// is used only once.
/// If the caller does not do that the cryptographic primitives in sodiumoxide
/// will not uphold any security guarantees (i.e. they may break)
pub fn increment_le(x: &mut [u8]) {
    unsafe {
        ffi::sodium_increment(x.as_mut_ptr(), x.len());
    }
}

/// `add_le()` treats `x` and `y` as unsigned little-endian numbers and adds `y` to `x`
/// modulo 2^(8*len) in constant time.
///
/// `add_le()` will return Err<()> if the length of `x` is not equal to the length of `y`.
///
/// WARNING: When used for incrementing nonces it is the caller's responsibility to ensure
/// that any given nonce value is used only once.
/// If the caller does not do that the cryptographic primitives in sodiumoxide
/// will not uphold any security guarantees (i.e. they may break)
pub fn add_le(x: &mut [u8], y: &[u8]) -> Result<(), ()> {
    if x.len() == y.len() {
        unsafe {
            ffi::sodium_add(x.as_mut_ptr(), y.as_ptr(), x.len());
        }
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_memcmp() {
        use randombytes::randombytes;

        for i in 0..256 {
            let x = randombytes(i);
            assert!(memcmp(&x, &x));
            let mut y = x.clone();
            assert!(memcmp(&x, &y));
            y.push(0);
            assert!(!memcmp(&x, &y));
            assert!(!memcmp(&y, &x));

            y = randombytes(i);
            if x == y {
                assert!(memcmp(&x, &y))
            } else {
                assert!(!memcmp(&x, &y))
            }
        }
    }

    #[test]
    fn test_increment_le_zero() {
        for i in 1..256 {
            let mut x = vec![0u8; i];
            increment_le(&mut x);
            assert!(!x.iter().all(|x| *x == 0));
            let mut y = vec![0u8; i];
            y[0] += 1;
            assert_eq!(x, y);
        }
    }

    #[test]
    fn test_increment_le_vectors() {
        let mut x = [255, 2, 3, 4, 5];
        let y = [0, 3, 3, 4, 5];
        increment_le(&mut x);
        assert!(!x.iter().all(|x| *x == 0));
        assert_eq!(x, y);
        let mut x = [255, 255, 3, 4, 5];
        let y = [0, 0, 4, 4, 5];
        increment_le(&mut x);
        assert!(!x.iter().all(|x| *x == 0));
        assert_eq!(x, y);
        let mut x = [255, 255, 255, 4, 5];
        let y = [0, 0, 0, 5, 5];
        increment_le(&mut x);
        assert!(!x.iter().all(|x| *x == 0));
        assert_eq!(x, y);
        let mut x = [255, 255, 255, 255, 5];
        let y = [0, 0, 0, 0, 6];
        increment_le(&mut x);
        assert!(!x.iter().all(|x| *x == 0));
        assert_eq!(x, y);
        let mut x = [255, 255, 255, 255, 255];
        let y = [0, 0, 0, 0, 0];
        increment_le(&mut x);
        assert!(x.iter().all(|x| *x == 0));
        assert_eq!(x, y);
    }

    #[test]
    fn test_increment_le_overflow() {
        for i in 1..256 {
            let mut x = vec![255u8; i];
            increment_le(&mut x);
            assert!(x.iter().all(|xi| *xi == 0));
        }
    }

    #[test]
    fn test_add_le_zero() {
        for i in 1..256 {
            let mut x = vec![0u8; i];
            let mut y = vec![0u8; i];
            y[0] = 42;
            assert!(add_le(&mut x, &y).is_ok());
            assert!(!x.iter().all(|x| *x == 0));
            assert_eq!(x, y);
        }
    }

    #[test]
    fn test_add_le_vectors() {
        let mut x = [255, 2, 3, 4, 5];
        let y = [42, 0, 0, 0, 0];
        let z = [41, 3, 3, 4, 5];
        assert!(add_le(&mut x, &y).is_ok());
        assert!(!x.iter().all(|x| *x == 0));
        assert_eq!(x, z);
        let mut x = [255, 255, 3, 4, 5];
        let z = [41, 0, 4, 4, 5];
        assert!(add_le(&mut x, &y).is_ok());
        assert!(!x.iter().all(|x| *x == 0));
        assert_eq!(x, z);
        let mut x = [255, 255, 255, 4, 5];
        let z = [41, 0, 0, 5, 5];
        assert!(add_le(&mut x, &y).is_ok());
        assert!(!x.iter().all(|x| *x == 0));
        assert_eq!(x, z);
        let mut x = [255, 255, 255, 255, 5];
        let z = [41, 0, 0, 0, 6];
        assert!(add_le(&mut x, &y).is_ok());
        assert!(!x.iter().all(|x| *x == 0));
        assert_eq!(x, z);
        let mut x = [255, 255, 255, 255, 255];
        let z = [41, 0, 0, 0, 0];
        assert!(add_le(&mut x, &y).is_ok());
        assert!(!x.iter().all(|x| *x == 0));
        assert_eq!(x, z);
    }

    #[test]
    fn test_add_le_overflow() {
        for i in 1..256 {
            let mut x = vec![255u8; i];
            let mut y = vec![0u8; i];
            y[0] = 42;
            assert!(add_le(&mut x, &y).is_ok());
            assert!(!x.iter().all(|x| *x == 0));
            y[0] -= 1;
            assert_eq!(x, y);
        }
    }

    #[test]
    fn test_add_le_different_lengths() {
        for i in 1..256 {
            let mut x = vec![1u8; i];
            let y = vec![42u8; i + 1];
            let z = vec![42u8; i - 1];
            assert!(add_le(&mut x, &y).is_err());
            assert_eq!(x, vec![1u8; i]);
            assert!(add_le(&mut x, &z).is_err());
            assert_eq!(x, vec![1u8; i]);
        }
    }

    #[test]
    fn test_mlock_munlock() {
        let t = b"hello world";
        let mut x = Vec::new();
        x.extend_from_slice(t);
        assert!(mlock(&mut x).is_ok());
        assert_eq!(&x, t);
        assert!(munlock(&mut x).is_ok());
        assert_ne!(&x, t);
    }

    #[cfg(unix)]
    #[test]
    fn test_mlock_fail() {
        // This value should be bigger than platform's page size so that we can
        // lock at least page size of memory. And this limit is going to be the
        // RLIMIT_MEMLOCK (see setrlimit(2)) for the rest of the process
        // duration.
        const LOCK_LIMIT: libc::rlim_t = 16384;

        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let ret = unsafe { libc::getrlimit(libc::RLIMIT_MEMLOCK, &mut limit) };
        assert_eq!(ret, 0, "libc::getrlimit failed");

        if limit.rlim_cur > LOCK_LIMIT {
            limit.rlim_cur = LOCK_LIMIT;
        }

        let ret = unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &limit) };
        assert_eq!(ret, 0, "libc::setrlimit failed");

        let mut x = vec![0; 5 * LOCK_LIMIT as usize];
        assert!(mlock(&mut x).is_err());
    }
}

//! Libsodium hexadecimal encoding/decoding helper functions
use ffi;
#[cfg(not(feature = "std"))]
use prelude::*;
use std::ptr;

/// Encodes byte sequence into a hexadecimal string.
///
/// # Panics
///
/// Panics if `2 * bin.len() + 1` overflows.
pub fn encode<T: AsRef<[u8]>>(bin: T) -> String {
    let bin = bin.as_ref();
    let mut hex = vec![0; encoded_len(bin.len()).unwrap()];

    // SAFETY: `sodium_bin2hex` writes `2 * bin.len() + 1`
    // characters from [0-9a-f] and a nul byte to `bin`.
    unsafe {
        ffi::sodium_bin2hex(
            hex.as_mut_ptr() as *mut _,
            hex.len(),
            bin.as_ptr(),
            bin.len(),
        );
        hex.pop();
        String::from_utf8_unchecked(hex)
    }
}

fn encoded_len(len: usize) -> Option<usize> {
    len.checked_mul(2)?.checked_add(1)
}

/// Parses a hexadecimal string into a byte sequence.
///
/// Fails if `hex.len()` is not even or
/// if `hex` contains characters not in [0-9a-fA-F].
pub fn decode<T: AsRef<[u8]>>(hex: T) -> Result<Vec<u8>, ()> {
    let hex = hex.as_ref();
    let mut bin = vec![0; decoded_len(hex.len())?];
    let mut bin_len = 0;

    // SAFETY: If `sodium_hex2bin` returns zero,
    // it has written `bin_len` bytes to `bin`.
    unsafe {
        let rc = ffi::sodium_hex2bin(
            bin.as_mut_ptr(),
            bin.len(),
            hex.as_ptr() as *const _,
            hex.len(),
            ptr::null(),
            &mut bin_len,
            ptr::null_mut(),
        );
        if rc != 0 {
            return Err(());
        }
        bin.truncate(bin_len);
        Ok(bin)
    }
}

fn decoded_len(len: usize) -> Result<usize, ()> {
    if len % 2 != 0 {
        return Err(());
    }

    Ok(len / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode() {
        assert_eq!("".to_string(), encode(b""));
        assert_eq!("666f6f626172".to_string(), encode(b"foobar"));
    }

    #[test]
    fn test_decode() {
        assert_eq!(Ok(b"".to_vec()), decode(""));
        assert_eq!(Ok(b"foobar".to_vec()), decode("666F6F626172"));
        assert_eq!(Err(()), decode("abc"));
        assert_eq!(Err(()), decode("abxy"));
    }
}

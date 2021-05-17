//! Libsodium Base64 encoding/decoding helper functions
use ffi;
#[cfg(not(feature = "std"))]
use prelude::*;
use std::ptr;

/// Supported variants of Base64 encoding/decoding
#[repr(u32)]
#[derive(Debug, PartialEq, Copy, Clone)]
pub enum Variant {
    /// Base64 as defined in RFC 4648 §4
    Original = ffi::sodium_base64_VARIANT_ORIGINAL,
    /// Base64 as defined in RFC 4648 §4 but without padding
    OriginalNoPadding = ffi::sodium_base64_VARIANT_ORIGINAL_NO_PADDING,
    /// Base64 as defined in RFC 4648 §5
    UrlSafe = ffi::sodium_base64_VARIANT_URLSAFE,
    /// Base64 as defined in RFC 4648 §5 but without padding
    UrlSafeNoPadding = ffi::sodium_base64_VARIANT_URLSAFE_NO_PADDING,
}

/// Encodes a byte sequence as a Base64 string using the given variant.
pub fn encode<T: AsRef<[u8]>>(bin: T, variant: Variant) -> String {
    let bin = bin.as_ref();
    // SAFETY: `Variant` contains only valid variant codes.
    let encoded_len = unsafe { ffi::sodium_base64_encoded_len(bin.len(), variant as _) };
    let mut b64 = vec![0; encoded_len];

    // SAFETY: `sodium_base64_encoded_len` ensures space for `bin.len()` bytes
    // and `Variant` contains only valid variant codes
    // and `sodium_bin2base64` writes only single byte ASCII characters.
    unsafe {
        ffi::sodium_bin2base64(
            b64.as_mut_ptr() as *mut _,
            b64.len(),
            bin.as_ptr(),
            bin.len(),
            variant as _,
        );
        b64.pop();
        String::from_utf8_unchecked(b64)
    }
}

/// Decodes a Base64 string into a byte sequence using the given variant.
///
/// Fails if the decoded length overflows
/// or if `b64` contains invalid characters.
pub fn decode<T: AsRef<[u8]>>(b64: T, variant: Variant) -> Result<Vec<u8>, ()> {
    let b64 = b64.as_ref();
    let mut bin = vec![0; decoded_len(b64.len()).ok_or(())?];
    let mut bin_len = 0;

    // SAFETY: `decoded_len` ensures space for 3 bytes
    // for every 4 characters including padding.
    // `Variant` contains only valid variant codes.
    // If `sodium_base642bin` returns zero,
    // it has written `bin_len` bytes to `bin`.
    unsafe {
        let rc = ffi::sodium_base642bin(
            bin.as_mut_ptr(),
            bin.len(),
            b64.as_ptr() as *const _,
            b64.len(),
            ptr::null(),
            &mut bin_len,
            ptr::null_mut(),
            variant as _,
        );
        if rc != 0 {
            return Err(());
        }
        bin.truncate(bin_len);
        Ok(bin)
    }
}

fn decoded_len(b64_len: usize) -> Option<usize> {
    let mut len = (b64_len / 4).checked_mul(3)?;

    if b64_len % 4 != 0 {
        len = len.checked_add(3)?;
    }

    Some(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode() {
        assert_eq!("".to_string(), encode(b"", Variant::Original));
        assert_eq!("Zg==".to_string(), encode(b"f", Variant::Original));
        assert_eq!("Zm8=".to_string(), encode(b"fo", Variant::Original));
        assert_eq!("Zm9v".to_string(), encode(b"foo", Variant::Original));
        assert_eq!("Zm9vYg==".to_string(), encode(b"foob", Variant::Original));
        assert_eq!("Zm9vYmE=".to_string(), encode(b"fooba", Variant::Original));
        assert_eq!("Zm9vYmFy".to_string(), encode(b"foobar", Variant::Original));
    }

    #[test]
    fn test_decode() {
        assert_eq!(Ok(b"".to_vec()), decode("", Variant::Original));
        assert_eq!(Ok(b"f".to_vec()), decode("Zg==", Variant::Original));
        assert_eq!(Ok(b"fo".to_vec()), decode("Zm8=", Variant::Original));
        assert_eq!(Ok(b"foo".to_vec()), decode("Zm9v", Variant::Original));
        assert_eq!(Ok(b"foob".to_vec()), decode("Zm9vYg==", Variant::Original));
        assert_eq!(Ok(b"fooba".to_vec()), decode("Zm9vYmE=", Variant::Original));
        assert_eq!(
            Ok(b"foobar".to_vec()),
            decode("Zm9vYmFy", Variant::Original)
        );
    }
}

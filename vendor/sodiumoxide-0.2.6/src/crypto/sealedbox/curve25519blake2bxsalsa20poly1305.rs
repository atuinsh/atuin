//! A particular combination of `Curve25519`, `Blake2B`, `XSalsa20` and `Poly1305`.

use ffi;
#[cfg(not(feature = "std"))]
use prelude::*;

use libc::c_ulonglong;

use super::super::box_::curve25519xsalsa20poly1305 as box_;

/// Number of additional bytes in a ciphertext compared to the corresponding
/// plaintext.
pub const SEALBYTES: usize = ffi::crypto_box_SEALBYTES as usize;

/// The `seal()` function encrypts a message `m` for a recipient whose public key
/// is `pk`. It returns the ciphertext whose length is `SEALBYTES + m.len()`.
///
/// The function creates a new key pair for each message, and attaches the public
/// key to the ciphertext. The secret key is overwritten and is not accessible
/// after this function returns.
pub fn seal(m: &[u8], pk: &box_::PublicKey) -> Vec<u8> {
    let mut c = vec![0u8; m.len() + SEALBYTES];
    unsafe {
        ffi::crypto_box_seal(
            c.as_mut_ptr(),
            m.as_ptr(),
            m.len() as c_ulonglong,
            pk.0.as_ptr(),
        );
    }
    c
}

/// The `open()` function decrypts the ciphertext `c` using the key pair `(pk, sk)`
/// and returns the decrypted message.
///
/// Key pairs are compatible with other
/// `crypto::box_::curve25519xsalsa20poly1305` operations and can be created
/// using `crypto::box::gen_keypair()`.
///
/// This function doesn't require passing the public key of the sender, as the
/// ciphertext already includes this information.
///
/// If decryption fails it returns `Err(())`.
pub fn open(c: &[u8], pk: &box_::PublicKey, sk: &box_::SecretKey) -> Result<Vec<u8>, ()> {
    if c.len() < SEALBYTES {
        return Err(());
    }
    let mut m = vec![0u8; c.len() - SEALBYTES];
    let ret = unsafe {
        ffi::crypto_box_seal_open(
            m.as_mut_ptr(),
            c.as_ptr(),
            c.len() as c_ulonglong,
            pk.0.as_ptr(),
            sk.0.as_ptr(),
        )
    };
    if ret == 0 {
        Ok(m)
    } else {
        Err(())
    }
}

#[cfg(test)]
mod test {
    use super::super::super::box_::curve25519xsalsa20poly1305 as box_;
    use super::*;

    #[test]
    fn test_seal_open() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let (pk, sk) = box_::gen_keypair();
            let m = randombytes(i);
            let c = seal(&m, &pk);
            let opened = open(&c, &pk, &sk);
            assert!(Ok(m) == opened);
        }
    }

    #[test]
    fn test_seal_open_tamper() {
        use randombytes::randombytes;
        for i in 0..32usize {
            let (pk, sk) = box_::gen_keypair();
            let m = randombytes(i);
            let mut c = seal(&m, &pk);
            for j in 0..c.len() {
                c[j] ^= 0x20;
                assert!(Err(()) == open(&c, &pk, &sk));
                c[j] ^= 0x20;
            }
        }
    }
}

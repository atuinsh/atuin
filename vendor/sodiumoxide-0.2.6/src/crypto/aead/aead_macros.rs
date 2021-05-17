macro_rules! aead_module (($seal_name:ident,
                           $open_name:ident,
                           $seal_detached_name:ident,
                           $open_detached_name:ident,
                           $keybytes:expr,
                           $noncebytes:expr,
                           $tagbytes:expr) => (

#[cfg(not(feature = "std"))] use prelude::*;
use libc::c_ulonglong;
use std::ptr;
use randombytes::randombytes_into;

/// Number of bytes in a `Key`.
pub const KEYBYTES: usize = $keybytes;

/// Number of bytes in a `Nonce`.
pub const NONCEBYTES: usize = $noncebytes;

/// Number of bytes in an authentication `Tag`.
pub const TAGBYTES: usize = $tagbytes;

new_type! {
    /// `Key` for symmetric authenticated encryption with additional data.
    ///
    /// When a `Key` goes out of scope its contents will
    /// be zeroed out
    secret Key(KEYBYTES);
}

new_type! {
    /// `Nonce` for symmetric authenticated encryption with additional data.
    nonce Nonce(NONCEBYTES);
}

new_type! {
    /// Authentication `Tag` for symmetric authenticated encryption with additional data in
    /// detached mode.
    public Tag(TAGBYTES);
}

/// `gen_key()` randomly generates a secret key
///
/// THREAD SAFETY: `gen_key()` is thread-safe provided that you have
/// called `sodiumoxide::init()` once before using any other function
/// from sodiumoxide.
pub fn gen_key() -> Key {
    let mut k = Key([0u8; KEYBYTES]);
    randombytes_into(&mut k.0);
    k
}

/// `seal()` encrypts and authenticates a message `m` together with optional plaintext data `ad`
/// using a secret key `k` and a nonce `n`. It returns a ciphertext `c`.
pub fn seal(m: &[u8], ad: Option<&[u8]>, n: &Nonce, k: &Key) -> Vec<u8> {
    let (ad_p, ad_len) = ad.map(|ad| (ad.as_ptr(), ad.len() as c_ulonglong)).unwrap_or((ptr::null(), 0));
    let mut c = Vec::with_capacity(m.len() + TAGBYTES);
    let mut clen = c.len() as c_ulonglong;

    unsafe {
        $seal_name(
            c.as_mut_ptr(),
            &mut clen,
            m.as_ptr(),
            m.len() as c_ulonglong,
            ad_p,
            ad_len,
            ptr::null_mut(),
            n.0.as_ptr(),
            k.0.as_ptr()
        );
        c.set_len(clen as usize);
    }
    c
}

/// `seal_detached()` encrypts and authenticates a message `m` together with optional plaintext data
/// `ad` using a secret key `k` and a nonce `n`.
/// `m` is encrypted in place, so after this function returns it will contain the ciphertext.
/// The detached authentication tag is returned by value.
pub fn seal_detached(m: &mut [u8], ad: Option<&[u8]>, n: &Nonce, k: &Key) -> Tag {
    let (ad_p, ad_len) = ad.map(|ad| (ad.as_ptr(), ad.len() as c_ulonglong)).unwrap_or((ptr::null(), 0));
    let mut tag = Tag([0u8; TAGBYTES]);
    let mut maclen = TAGBYTES as c_ulonglong;
    unsafe {
        $seal_detached_name(
            m.as_mut_ptr(),
            tag.0.as_mut_ptr(),
            &mut maclen,
            m.as_ptr(),
            m.len() as c_ulonglong,
            ad_p,
            ad_len,
            ptr::null_mut(),
            n.0.as_ptr(),
            k.0.as_ptr()
        );
    }
    tag
}

/// `open()` verifies and decrypts a ciphertext `c` together with optional plaintext data `ad`
/// using a secret key `k` and a nonce `n`.
/// It returns a plaintext `Ok(m)`.
/// If the ciphertext fails verification, `open()` returns `Err(())`.
pub fn open(c: &[u8], ad: Option<&[u8]>, n: &Nonce, k: &Key) -> Result<Vec<u8>, ()> {
    if c.len() < TAGBYTES {
        return Err(());
    }
    let (ad_p, ad_len) = ad.map(|ad| (ad.as_ptr(), ad.len() as c_ulonglong)).unwrap_or((ptr::null(), 0));
    let mut m = Vec::with_capacity(c.len() - TAGBYTES);
    let mut mlen = m.len() as c_ulonglong;

    unsafe {
        let ret =
            $open_name(
                m.as_mut_ptr(),
                &mut mlen,
                ptr::null_mut(),
                c.as_ptr(),
                c.len() as c_ulonglong,
                ad_p,
                ad_len,
                n.0.as_ptr(),
                k.0.as_ptr()
            );
        if ret != 0 {
            return Err(());
        }
        m.set_len(mlen as usize);
    }
    Ok(m)
}
/// `open_detached()` verifies and decrypts a ciphertext `c` toghether with optional plaintext data
/// `ad` and and authentication tag `tag`, using a secret key `k` and a nonce `n`.
/// `c` is decrypted in place, so if this function is successful it will contain the plaintext.
/// If the ciphertext fails verification, `open_detached()` returns `Err(())`,
/// and the ciphertext is not modified.
pub fn open_detached(c: &mut [u8], ad: Option<&[u8]>, t: &Tag, n: &Nonce, k: &Key) -> Result<(), ()> {
    let (ad_p, ad_len) = ad.map(|ad| (ad.as_ptr(), ad.len() as c_ulonglong)).unwrap_or((ptr::null(), 0));
    let ret = unsafe {
        $open_detached_name(
            c.as_mut_ptr(),
            ptr::null_mut(),
            c.as_ptr(),
            c.len() as c_ulonglong,
            t.0.as_ptr(),
            ad_p,
            ad_len,
            n.0.as_ptr(),
            k.0.as_ptr()
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod test_m {
    use super::*;
    use crypto::nonce::gen_random_nonce;

    #[test]
    fn test_seal_open() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let k = gen_key();
            let n = gen_random_nonce();
            let ad = randombytes(i);
            let m = randombytes(i);
            let c = seal(&m, Some(&ad), &n, &k);
            let m2 = open(&c, Some(&ad), &n, &k).unwrap();
            assert_eq!(m, m2);
        }
    }

    #[test]
    fn test_seal_open_tamper() {
        use randombytes::randombytes;
        for i in 0..32usize {
            let k = gen_key();
            let n = gen_random_nonce();
            let mut ad = randombytes(i);
            let m = randombytes(i);
            let mut c = seal(&m, Some(&ad), &n, &k);
            for j in 0..c.len() {
                c[j] ^= 0x20;
                let m2 = open(&c, Some(&ad), &n, &k);
                c[j] ^= 0x20;
                assert!(m2.is_err());
            }
            for j in 0..ad.len() {
                ad[j] ^= 0x20;
                let m2 = open(&c, Some(&ad), &n, &k);
                ad[j] ^= 0x20;
                assert!(m2.is_err());
            }
        }
    }

    #[test]
    fn test_seal_open_detached() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let k = gen_key();
            let n = gen_random_nonce();
            let ad = randombytes(i);
            let mut m = randombytes(i);
            let m2 = m.clone();
            let t = seal_detached(&mut m, Some(&ad), &n, &k);
            open_detached(&mut m, Some(&ad), &t, &n, &k).unwrap();
            assert_eq!(m, m2);
        }
    }

    #[test]
    fn test_seal_open_detached_tamper() {
        use randombytes::randombytes;
        for i in 0..32usize {
            let k = gen_key();
            let n = gen_random_nonce();
            let mut ad = randombytes(i);
            let mut m = randombytes(i);
            let mut t = seal_detached(&mut m, Some(&ad), &n, &k);
            for j in 0..m.len() {
                m[j] ^= 0x20;
                let r = open_detached(&mut m, Some(&ad), &t, &n, &k);
                m[j] ^= 0x20;
                assert!(r.is_err());
            }
            for j in 0..ad.len() {
                ad[j] ^= 0x20;
                let r = open_detached(&mut m, Some(&ad), &t, &n, &k);
                ad[j] ^= 0x20;
                assert!(r.is_err());
            }
            for j in 0..t.0.len() {
                t.0[j] ^= 0x20;
                let r = open_detached(&mut m, Some(&ad), &t, &n, &k);
                t.0[j] ^= 0x20;
                assert!(r.is_err());
            }
        }
    }

    #[test]
    fn test_seal_open_detached_same() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let k = gen_key();
            let n = gen_random_nonce();
            let ad = randombytes(i);
            let mut m = randombytes(i);

            let c = seal(&m, Some(&ad), &n, &k);
            let t = seal_detached(&mut m, Some(&ad), &n, &k);

            assert_eq!(&c[0..c.len()-TAGBYTES], &m[..]);
            assert_eq!(&c[c.len()-TAGBYTES..], &t.0[..]);

            let m2 = open(&c, Some(&ad), &n, &k).unwrap();
            open_detached(&mut m, Some(&ad), &t, &n, &k).unwrap();

            assert_eq!(m2, m);
        }
    }
}

));

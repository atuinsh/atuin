//! `crypto_scalarmult_curve25519` specified in
//! [Cryptography in `NaCl`](http://nacl.cr.yp.to/valid.html), Sections 2, 3, and 4.
//! This function is conjectured to be strong. For background see Bernstein,
//! "Curve25519: new Diffie-Hellman speed records," Lecture Notes in Computer
//! Science 3958 (2006), 207–228, <http://cr.yp.to/papers.html#curve25519>.
use ffi;

/// Number of bytes in a `GroupElement`.
pub const GROUPELEMENTBYTES: usize = ffi::crypto_scalarmult_curve25519_BYTES as usize;

/// Number of bytes in a `Scalar`.
pub const SCALARBYTES: usize = ffi::crypto_scalarmult_curve25519_SCALARBYTES as usize;

new_type! {
    /// `Scalar` value (integer in byte representation)
    secret Scalar(SCALARBYTES);
}

new_type! {
    /// `GroupElement`
    secret GroupElement(GROUPELEMENTBYTES);
}

/// `scalarmult()` multiplies a group element `p`
/// by an integer `n`. It returns the resulting group element `Ok(q)`.
/// If the the `GroupElement` is all zero, `scalarmult()` returns `Err(())` since
/// the resulting `GroupElement` would be all zero, no matter the `Scalar`.
pub fn scalarmult(n: &Scalar, p: &GroupElement) -> Result<GroupElement, ()> {
    let mut q = [0; GROUPELEMENTBYTES];
    unsafe {
        if ffi::crypto_scalarmult_curve25519(q.as_mut_ptr(), n.0.as_ptr(), p.0.as_ptr()) != 0 {
            Err(())
        } else {
            Ok(GroupElement(q))
        }
    }
}

/// `scalarmult_base()` computes the scalar product of a standard
/// group element and an integer `n`. It returns the resulting
/// group element `q`/
pub fn scalarmult_base(n: &Scalar) -> GroupElement {
    let mut q = [0; GROUPELEMENTBYTES];
    unsafe {
        ffi::crypto_scalarmult_curve25519_base(q.as_mut_ptr(), n.0.as_ptr());
    }
    GroupElement(q)
}

#[cfg(test)]
mod test {
    use super::*;
    use randombytes::randombytes_into;

    #[test]
    fn test_vector_1() {
        // corresponding to tests/scalarmult.c and tests/scalarmult3.cpp from NaCl
        let alicesk = Scalar([
            0x77, 0x07, 0x6d, 0x0a, 0x73, 0x18, 0xa5, 0x7d, 0x3c, 0x16, 0xc1, 0x72, 0x51, 0xb2,
            0x66, 0x45, 0xdf, 0x4c, 0x2f, 0x87, 0xeb, 0xc0, 0x99, 0x2a, 0xb1, 0x77, 0xfb, 0xa5,
            0x1d, 0xb9, 0x2c, 0x2a,
        ]);
        let alicepk_expected = [
            0x85, 0x20, 0xf0, 0x09, 0x89, 0x30, 0xa7, 0x54, 0x74, 0x8b, 0x7d, 0xdc, 0xb4, 0x3e,
            0xf7, 0x5a, 0x0d, 0xbf, 0x3a, 0x0d, 0x26, 0x38, 0x1a, 0xf4, 0xeb, 0xa4, 0xa9, 0x8e,
            0xaa, 0x9b, 0x4e, 0x6a,
        ];
        let GroupElement(alicepk) = scalarmult_base(&alicesk);
        assert!(alicepk == alicepk_expected);
    }

    #[test]
    fn test_vector_2() {
        // corresponding to tests/scalarmult2.c and tests/scalarmult4.cpp from NaCl
        let bobsk = Scalar([
            0x5d, 0xab, 0x08, 0x7e, 0x62, 0x4a, 0x8a, 0x4b, 0x79, 0xe1, 0x7f, 0x8b, 0x83, 0x80,
            0x0e, 0xe6, 0x6f, 0x3b, 0xb1, 0x29, 0x26, 0x18, 0xb6, 0xfd, 0x1c, 0x2f, 0x8b, 0x27,
            0xff, 0x88, 0xe0, 0xeb,
        ]);
        let bobpk_expected = [
            0xde, 0x9e, 0xdb, 0x7d, 0x7b, 0x7d, 0xc1, 0xb4, 0xd3, 0x5b, 0x61, 0xc2, 0xec, 0xe4,
            0x35, 0x37, 0x3f, 0x83, 0x43, 0xc8, 0x5b, 0x78, 0x67, 0x4d, 0xad, 0xfc, 0x7e, 0x14,
            0x6f, 0x88, 0x2b, 0x4f,
        ];
        let GroupElement(bobpk) = scalarmult_base(&bobsk);
        assert!(bobpk == bobpk_expected);
    }

    #[test]
    fn test_vector_3() {
        // corresponding to tests/scalarmult5.c and tests/scalarmult7.cpp from NaCl
        let alicesk = Scalar([
            0x77, 0x07, 0x6d, 0x0a, 0x73, 0x18, 0xa5, 0x7d, 0x3c, 0x16, 0xc1, 0x72, 0x51, 0xb2,
            0x66, 0x45, 0xdf, 0x4c, 0x2f, 0x87, 0xeb, 0xc0, 0x99, 0x2a, 0xb1, 0x77, 0xfb, 0xa5,
            0x1d, 0xb9, 0x2c, 0x2a,
        ]);
        let bobpk = GroupElement([
            0xde, 0x9e, 0xdb, 0x7d, 0x7b, 0x7d, 0xc1, 0xb4, 0xd3, 0x5b, 0x61, 0xc2, 0xec, 0xe4,
            0x35, 0x37, 0x3f, 0x83, 0x43, 0xc8, 0x5b, 0x78, 0x67, 0x4d, 0xad, 0xfc, 0x7e, 0x14,
            0x6f, 0x88, 0x2b, 0x4f,
        ]);
        let k_expected = [
            0x4a, 0x5d, 0x9d, 0x5b, 0xa4, 0xce, 0x2d, 0xe1, 0x72, 0x8e, 0x3b, 0xf4, 0x80, 0x35,
            0x0f, 0x25, 0xe0, 0x7e, 0x21, 0xc9, 0x47, 0xd1, 0x9e, 0x33, 0x76, 0xf0, 0x9b, 0x3c,
            0x1e, 0x16, 0x17, 0x42,
        ];
        let GroupElement(k) = scalarmult(&alicesk, &bobpk).unwrap();
        assert!(k == k_expected);
    }

    #[test]
    fn test_vector_4() {
        // corresponding to tests/scalarmult6.c from NaCl
        let bobsk = Scalar([
            0x5d, 0xab, 0x08, 0x7e, 0x62, 0x4a, 0x8a, 0x4b, 0x79, 0xe1, 0x7f, 0x8b, 0x83, 0x80,
            0x0e, 0xe6, 0x6f, 0x3b, 0xb1, 0x29, 0x26, 0x18, 0xb6, 0xfd, 0x1c, 0x2f, 0x8b, 0x27,
            0xff, 0x88, 0xe0, 0xeb,
        ]);
        let alicepk = GroupElement([
            0x85, 0x20, 0xf0, 0x09, 0x89, 0x30, 0xa7, 0x54, 0x74, 0x8b, 0x7d, 0xdc, 0xb4, 0x3e,
            0xf7, 0x5a, 0x0d, 0xbf, 0x3a, 0x0d, 0x26, 0x38, 0x1a, 0xf4, 0xeb, 0xa4, 0xa9, 0x8e,
            0xaa, 0x9b, 0x4e, 0x6a,
        ]);
        let k_expected = [
            0x4a, 0x5d, 0x9d, 0x5b, 0xa4, 0xce, 0x2d, 0xe1, 0x72, 0x8e, 0x3b, 0xf4, 0x80, 0x35,
            0x0f, 0x25, 0xe0, 0x7e, 0x21, 0xc9, 0x47, 0xd1, 0x9e, 0x33, 0x76, 0xf0, 0x9b, 0x3c,
            0x1e, 0x16, 0x17, 0x42,
        ];
        let GroupElement(k) = scalarmult(&bobsk, &alicepk).unwrap();
        assert!(k == k_expected);
    }

    #[test]
    #[should_panic]
    fn test_all_zero() {
        let mut sk = [0; SCALARBYTES];
        randombytes_into(&mut sk);
        let sk = Scalar(sk);
        let pk = GroupElement([0; GROUPELEMENTBYTES]);
        let _ = scalarmult(&sk, &pk).unwrap();
    }
}

#[cfg(feature = "benchmarks")]
#[cfg(test)]
mod bench {
    extern crate test;
    use super::*;
    use randombytes::randombytes_into;

    #[bench]
    fn bench_scalarmult(b: &mut test::Bencher) {
        let mut g = GroupElement([0u8; GROUPELEMENTBYTES]);
        let mut s = Scalar([0u8; SCALARBYTES]);
        randombytes_into(&mut g.0);
        randombytes_into(&mut s.0);
        b.iter(|| {
            scalarmult(&s, &g);
        });
    }

    #[bench]
    fn bench_scalarmult_base(b: &mut test::Bencher) {
        let mut s = Scalar([0u8; SCALARBYTES]);
        randombytes_into(&mut s.0);
        b.iter(|| {
            scalarmult_base(&s);
        });
    }
}

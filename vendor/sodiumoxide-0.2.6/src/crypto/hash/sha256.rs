//! `SHA-256`.
//!
//! There has been considerable degradation of public confidence in the
//! security conjectures for many hash functions, including `SHA-256`.
//! However, for the moment, there do not appear to be alternatives that
//! inspire satisfactory levels of confidence. One can hope that NIST's
//! SHA-3 competition will improve the situation.

use ffi::{
    crypto_hash_sha256, crypto_hash_sha256_BYTES, crypto_hash_sha256_final,
    crypto_hash_sha256_init, crypto_hash_sha256_state, crypto_hash_sha256_update,
};

hash_module!(
    crypto_hash_sha256,
    crypto_hash_sha256_state,
    crypto_hash_sha256_init,
    crypto_hash_sha256_update,
    crypto_hash_sha256_final,
    crypto_hash_sha256_BYTES as usize,
    64
);

#[cfg(test)]
mod test {
    use super::*;
    use hex;
    #[cfg(not(feature = "std"))]
    use prelude::*;

    #[test]
    fn test_vector_1() {
        // hash of empty string
        let x = [];
        let h_expected = [
            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
            0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
            0x78, 0x52, 0xb8, 0x55,
        ];
        let Digest(h) = hash(&x);
        assert!(h == h_expected);
    }

    #[test]
    fn test_vector_2() {
        // The quick brown fox jumps over the lazy dog
        let x = [
            0x54, 0x68, 0x65, 0x20, 0x71, 0x75, 0x69, 0x63, 0x6b, 0x20, 0x62, 0x72, 0x6f, 0x77,
            0x6e, 0x20, 0x66, 0x6f, 0x78, 0x20, 0x6a, 0x75, 0x6d, 0x70, 0x73, 0x20, 0x6f, 0x76,
            0x65, 0x72, 0x20, 0x74, 0x68, 0x65, 0x20, 0x6c, 0x61, 0x7a, 0x79, 0x20, 0x64, 0x6f,
            0x67,
        ];
        let h_expected = [
            0xd7, 0xa8, 0xfb, 0xb3, 0x07, 0xd7, 0x80, 0x94, 0x69, 0xca, 0x9a, 0xbc, 0xb0, 0x08,
            0x2e, 0x4f, 0x8d, 0x56, 0x51, 0xe4, 0x6d, 0x3c, 0xdb, 0x76, 0x2d, 0x02, 0xd0, 0xbf,
            0x37, 0xc9, 0xe5, 0x92,
        ];
        let Digest(h) = hash(&x);
        assert!(h == h_expected);
    }

    fn test_nist_vector(filename: &str) {
        use std::fs::File;
        use std::io::{BufRead, BufReader};

        let mut r = BufReader::new(File::open(filename).unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            r.read_line(&mut line).unwrap();
            if line.is_empty() {
                break;
            }
            let starts_with_len = line.starts_with("Len = ");
            if starts_with_len {
                let len: usize = line[6..].trim().parse().unwrap();
                line.clear();
                r.read_line(&mut line).unwrap();
                let rawmsg = hex::decode(&line[6..].trim()).unwrap();
                let msg = &rawmsg[..len / 8];
                line.clear();
                r.read_line(&mut line).unwrap();
                let md = hex::decode(&line[5..].trim()).unwrap();
                let Digest(digest) = hash(msg);
                assert!(digest[..] == md[..]);
            }
        }
    }

    #[test]
    fn test_vectors_nist_short() {
        test_nist_vector("testvectors/SHA256ShortMsg.rsp");
    }

    #[test]
    fn test_vectors_nist_long() {
        test_nist_vector("testvectors/SHA256LongMsg.rsp");
    }

    #[test]
    fn test_hash_state_empty() {
        // hash of empty string
        let h_expected = [
            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
            0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
            0x78, 0x52, 0xb8, 0x55,
        ];
        let hash_state = State::new();
        let Digest(h) = hash_state.finalize();
        assert!(h == h_expected);
    }

    #[test]
    fn test_hash_state_multi_parts() {
        // The quick brown fox jumps over the lazy dog
        let x = [
            0x54, 0x68, 0x65, 0x20, 0x71, 0x75, 0x69, 0x63, 0x6b, 0x20, 0x62, 0x72, 0x6f, 0x77,
            0x6e, 0x20, 0x66, 0x6f, 0x78, 0x20, 0x6a, 0x75, 0x6d, 0x70, 0x73, 0x20, 0x6f, 0x76,
            0x65, 0x72, 0x20, 0x74, 0x68, 0x65, 0x20, 0x6c, 0x61, 0x7a, 0x79, 0x20, 0x64, 0x6f,
            0x67,
        ];
        let h_expected = [
            0xd7, 0xa8, 0xfb, 0xb3, 0x07, 0xd7, 0x80, 0x94, 0x69, 0xca, 0x9a, 0xbc, 0xb0, 0x08,
            0x2e, 0x4f, 0x8d, 0x56, 0x51, 0xe4, 0x6d, 0x3c, 0xdb, 0x76, 0x2d, 0x02, 0xd0, 0xbf,
            0x37, 0xc9, 0xe5, 0x92,
        ];
        let mut hash_state = State::new();
        for chunk in x.chunks(3) {
            hash_state.update(chunk);
        }
        let Digest(h) = hash_state.finalize();
        assert!(h == h_expected);
    }
}

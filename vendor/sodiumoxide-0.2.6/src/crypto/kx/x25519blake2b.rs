//! `x25519blake2b` is the current default key exchange scheme of `libsodium`.

use ffi;

/// Number of bytes in a `PublicKey`.
pub const PUBLICKEYBYTES: usize = ffi::crypto_kx_PUBLICKEYBYTES as usize;

/// Number of bytes in a `SecretKey`.
pub const SECRETKEYBYTES: usize = ffi::crypto_kx_SECRETKEYBYTES as usize;

/// Number of bytes in a `Seed`.
pub const SEEDBYTES: usize = ffi::crypto_kx_SEEDBYTES as usize;

/// Number of bytes in a `SessionKey`.
pub const SESSIONKEYBYTES: usize = ffi::crypto_kx_SESSIONKEYBYTES as usize;

new_type! {
    /// `PublicKey` for key exchanges.
    public PublicKey(PUBLICKEYBYTES);
}

new_type! {
    /// `SecretKey` for key exchanges.
    ///
    /// When a `SecretKey` goes out of scope its contents will be zeroed out
    secret SecretKey(SECRETKEYBYTES);
}

new_type! {
    /// `Seed` that can be used for keypair generation
    ///
    /// The `Seed` is used by `keypair_from_seed()` to generate a secret and
    /// public signature key.
    ///
    /// When a `Seed` goes out of scope its content will be zeroed out
    secret Seed(SEEDBYTES);
}

new_type! {
    /// `SessionKey` is returned by `client_session_keys` and `server_session_keys` and is the
    /// exchanged secret between the client and server.
    secret SessionKey(SESSIONKEYBYTES);
}

/// `gen_keypair()` randomly generates a secret key and a corresponding public
/// key.
///
/// THREAD SAFETY: `gen_keypair()` is thread-safe provided that you have
/// called `sodiumoxide::init()` once before using any other function
/// from sodiumoxide.
pub fn gen_keypair() -> (PublicKey, SecretKey) {
    unsafe {
        let mut pk = PublicKey([0u8; PUBLICKEYBYTES]);
        let mut sk = SecretKey([0u8; SECRETKEYBYTES]);
        ffi::crypto_kx_keypair(pk.0.as_mut_ptr(), sk.0.as_mut_ptr());
        (pk, sk)
    }
}

/// `keypair_from_seed()` computes a secret key and a corresponding public key
/// from a `Seed`.
pub fn keypair_from_seed(seed: &Seed) -> (PublicKey, SecretKey) {
    unsafe {
        let mut pk = PublicKey([0u8; PUBLICKEYBYTES]);
        let mut sk = SecretKey([0u8; SECRETKEYBYTES]);
        ffi::crypto_kx_seed_keypair(pk.0.as_mut_ptr(), sk.0.as_mut_ptr(), seed.0.as_ptr());
        (pk, sk)
    }
}

/// `server_session_keys()` computes a pair of shared keys (rx and tx) using the server's public
/// key `server_pk`, the server's secret key `server_sk` and the client's public key `client_pk`.
/// If the client's public key is acceptable, it returns the two shared keys, the first for `rx`
/// and the second for `tx`. Otherwise, it returns `None`.
pub fn server_session_keys(
    server_pk: &PublicKey,
    server_sk: &SecretKey,
    client_pk: &PublicKey,
) -> Result<(SessionKey, SessionKey), ()> {
    unsafe {
        let mut rx = SessionKey([0u8; SESSIONKEYBYTES]);
        let mut tx = SessionKey([0u8; SESSIONKEYBYTES]);
        let r = ffi::crypto_kx_server_session_keys(
            rx.0.as_mut_ptr(),
            tx.0.as_mut_ptr(),
            server_pk.0.as_ptr(),
            server_sk.0.as_ptr(),
            client_pk.0.as_ptr(),
        );
        if r != 0 {
            Err(())
        } else {
            Ok((rx, tx))
        }
    }
}

/// `client_session_keys()` computes a pair of shared keys (rx and tx) using the client's public
/// key `client_pk`, the client's secret key `client_sk` and the server's public key `server_pk`.
/// If the server's public key is acceptable, it returns the two shared keys, the first for `rx`
/// and the second for `tx`. Otherwise, it returns `None`.
pub fn client_session_keys(
    client_pk: &PublicKey,
    client_sk: &SecretKey,
    server_pk: &PublicKey,
) -> Result<(SessionKey, SessionKey), ()> {
    unsafe {
        let mut rx = SessionKey([0u8; SESSIONKEYBYTES]);
        let mut tx = SessionKey([0u8; SESSIONKEYBYTES]);
        let r = ffi::crypto_kx_client_session_keys(
            rx.0.as_mut_ptr(),
            tx.0.as_mut_ptr(),
            client_pk.0.as_ptr(),
            client_sk.0.as_ptr(),
            server_pk.0.as_ptr(),
        );

        if r != 0 {
            Err(())
        } else {
            Ok((rx, tx))
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_kx() {
        let (client_pk, client_sk) = gen_keypair();
        let (server_pk, server_sk) = gen_keypair();

        assert!(client_pk != server_pk);
        assert!(client_sk != server_sk);

        let (client_rx, client_tx) =
            client_session_keys(&client_pk, &client_sk, &server_pk).unwrap();
        let (server_rx, server_tx) =
            server_session_keys(&server_pk, &server_sk, &client_pk).unwrap();

        assert!(client_rx == server_tx);
        assert!(client_tx == server_rx);
    }

    #[test]
    fn test_kx_non_acceptable_keys() {
        let (client_pk, client_sk) = gen_keypair();
        let (server_pk, server_sk) = gen_keypair();

        // non correct public keys
        let fake_client_pk = PublicKey([0u8; PUBLICKEYBYTES]);
        let fake_server_pk = PublicKey([0u8; PUBLICKEYBYTES]);

        assert!(client_session_keys(&client_pk, &client_sk, &fake_server_pk) == Err(()));
        assert!(server_session_keys(&server_pk, &server_sk, &fake_client_pk) == Err(()));
    }

    #[test]
    fn test_vectors() {
        let small_order_p = PublicKey([
            0xe0, 0xeb, 0x7a, 0x7c, 0x3b, 0x41, 0xb8, 0xae, 0x16, 0x56, 0xe3, 0xfa, 0xf1, 0x9f,
            0xc4, 0x6a, 0xda, 0x09, 0x8d, 0xeb, 0x9c, 0x32, 0xb1, 0xfd, 0x86, 0x62, 0x05, 0x16,
            0x5f, 0x49, 0xb8, 0x00,
        ]);

        let mut seed = Seed([0u8; SEEDBYTES]);
        for i in 0..seed.0.len() {
            seed.0[i] = i as u8;
        }
        let (mut client_pk, client_sk) = keypair_from_seed(&seed);

        let client_pk_expected = PublicKey([
            0x0e, 0x02, 0x16, 0x22, 0x3f, 0x14, 0x71, 0x43, 0xd3, 0x26, 0x15, 0xa9, 0x11, 0x89,
            0xc2, 0x88, 0xc1, 0x72, 0x8c, 0xba, 0x3c, 0xc5, 0xf9, 0xf6, 0x21, 0xb1, 0x02, 0x6e,
            0x03, 0xd8, 0x31, 0x29,
        ]);
        assert_eq!(client_pk, client_pk_expected);
        let client_sk_expected = SecretKey([
            0xcb, 0x2f, 0x51, 0x60, 0xfc, 0x1f, 0x7e, 0x05, 0xa5, 0x5e, 0xf4, 0x9d, 0x34, 0x0b,
            0x48, 0xda, 0x2e, 0x5a, 0x78, 0x09, 0x9d, 0x53, 0x39, 0x33, 0x51, 0xcd, 0x57, 0x9d,
            0xd4, 0x25, 0x03, 0xd6,
        ]);
        assert_eq!(client_sk, client_sk_expected);

        let (server_pk, server_sk) = gen_keypair();

        assert_eq!(
            client_session_keys(&client_pk, &client_sk, &small_order_p),
            Err(())
        );
        let (client_rx, client_tx) =
            client_session_keys(&client_pk, &client_sk, &server_pk).unwrap();

        assert_eq!(
            server_session_keys(&server_pk, &server_sk, &small_order_p),
            Err(())
        );
        let _ = server_session_keys(&server_pk, &server_sk, &client_pk).unwrap();

        client_pk.0[0] += 1;

        let (server_rx, server_tx) =
            server_session_keys(&server_pk, &server_sk, &client_pk).unwrap();

        assert_ne!(server_rx.0, client_tx.0);
        assert_ne!(server_tx.0, client_rx.0);

        let (client_pk, _) = gen_keypair();
        let (server_rx, server_tx) =
            server_session_keys(&server_pk, &server_sk, &client_pk).unwrap();

        assert_ne!(server_rx.0, client_tx.0);
        assert_ne!(server_tx.0, client_rx.0);

        let (client_pk, client_sk) = keypair_from_seed(&seed);
        seed.0[0] += 1;
        let (server_pk, server_sk) = keypair_from_seed(&seed);

        let (server_rx, server_tx) =
            server_session_keys(&server_pk, &server_sk, &client_pk).unwrap();
        let server_rx_expected = SessionKey([
            0x62, 0xc8, 0xf4, 0xfa, 0x81, 0x80, 0x0a, 0xbd, 0x05, 0x77, 0xd9, 0x99, 0x18, 0xd1,
            0x29, 0xb6, 0x5d, 0xeb, 0x78, 0x9a, 0xf8, 0xc8, 0x35, 0x1f, 0x39, 0x1f, 0xeb, 0x0c,
            0xbf, 0x23, 0x86, 0x04,
        ]);
        let server_tx_expected = SessionKey([
            0x74, 0x95, 0x19, 0xc6, 0x80, 0x59, 0xbc, 0xe6, 0x9f, 0x7c, 0xfc, 0xc7, 0xb3, 0x87,
            0xa3, 0xde, 0x1a, 0x1e, 0x82, 0x37, 0xd1, 0x10, 0x99, 0x13, 0x23, 0xbf, 0x62, 0x87,
            0x01, 0x15, 0x73, 0x1a,
        ]);
        assert_eq!(server_rx, server_rx_expected);
        assert_eq!(server_tx, server_tx_expected);

        let (client_rx, client_tx) =
            client_session_keys(&client_pk, &client_sk, &server_pk).unwrap();
        let client_rx_expected = SessionKey([
            0x74, 0x95, 0x19, 0xc6, 0x80, 0x59, 0xbc, 0xe6, 0x9f, 0x7c, 0xfc, 0xc7, 0xb3, 0x87,
            0xa3, 0xde, 0x1a, 0x1e, 0x82, 0x37, 0xd1, 0x10, 0x99, 0x13, 0x23, 0xbf, 0x62, 0x87,
            0x01, 0x15, 0x73, 0x1a,
        ]);
        let client_tx_expected = SessionKey([
            0x62, 0xc8, 0xf4, 0xfa, 0x81, 0x80, 0x0a, 0xbd, 0x05, 0x77, 0xd9, 0x99, 0x18, 0xd1,
            0x29, 0xb6, 0x5d, 0xeb, 0x78, 0x9a, 0xf8, 0xc8, 0x35, 0x1f, 0x39, 0x1f, 0xeb, 0x0c,
            0xbf, 0x23, 0x86, 0x04,
        ]);
        assert_eq!(client_rx, client_rx_expected);
        assert_eq!(client_tx, client_tx_expected);
    }
}

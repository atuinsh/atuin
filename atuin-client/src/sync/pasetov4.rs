//! Loosely following <https://github.com/paseto-standard/paseto-spec/blob/master/docs/01-Protocol-Versions/Version4.md>.
// DO NOT MODIFY. We can't change old encryption schemes, only add new ones.

use super::key::Key;
use base64::{prelude::BASE64_URL_SAFE, Engine};
use blake2::{
    digest::{FixedOutput, Mac},
    Blake2bMac,
};
use chacha20::{
    cipher::{KeyIvInit, StreamCipher},
    XChaCha20,
};
use chrono::Utc;
use eyre::{bail, Result};
use generic_array::{
    sequence::Split,
    typenum::{U32, U56},
    GenericArray,
};
use serde::{Deserialize, Serialize};
use xsalsa20poly1305::aead::{rand_core::RngCore, OsRng};

#[derive(Debug, Serialize, Deserialize)]
struct HistoryPlaintext {
    pub duration: i64,
    pub exit: i64,
    pub command: String,
    pub cwd: String,
    pub session: String,
    pub hostname: String,
    pub timestamp: chrono::DateTime<Utc>,
}
#[derive(Debug, Serialize, Deserialize)]
struct EncryptedHistory {
    pub ciphertext: Vec<u8>,
    pub nonce: GenericArray<u8, U32>,
}

use crate::history::History;

pub struct Client {
    encryption_key_hasher: Blake2bMac<U56>,
    authentication_key_hasher: Blake2bMac<U32>,
}

static HEADER: &[u8] = b"atuin.paseto.v4.local."; // not spec compliant, but we don't intend to be 100% compliant

/// <https://github.com/paseto-standard/paseto-spec/blob/master/docs/01-Protocol-Versions/Common.md#authentication-padding>
fn pae<M: Mac>(mut mac: M, pieces: &[&[u8]]) -> GenericArray<u8, M::OutputSize> {
    mac.update(&(pieces.len() as u64).to_le_bytes());
    for piece in pieces {
        mac.update(&(piece.len() as u64).to_le_bytes());
        mac.update(piece);
    }
    mac.finalize().into_bytes()
}

impl Client {
    pub fn new(key: &Key) -> Self {
        Self {
            encryption_key_hasher: Blake2bMac::<U56>::new_from_slice(key)
                .expect("32 byte key is less than 56 byte block size"),
            authentication_key_hasher: Blake2bMac::<U32>::new_from_slice(key)
                .expect("32 byte key is equal to 32 byte block size"),
        }
    }

    /// Step 4 of encryption, Step 5 of decryption
    ///
    /// > Split the key into an Encryption key (Ek) and Authentication key (Ak), using keyed BLAKE2b,
    /// > using the domain separation constants and n as the message, and the input key as the key.
    /// > The first value will be 56 bytes, the second will be 32 bytes. The derived key will be the leftmost 32 bytes of the hash output.
    /// > The remaining 24 bytes will be used as a counter nonce (n2):
    /// ```ignore
    /// tmp = crypto_generichash(
    ///     msg = "paseto-encryption-key" || n,
    ///     key = key,
    ///     length = 56
    /// );
    /// Ek = tmp[0:32]
    /// n2 = tmp[32:]
    /// Ak = crypto_generichash(
    ///     msg = "paseto-auth-key-for-aead" || n,
    ///     key = key,
    ///     length = 32
    /// );
    /// ```
    fn keys(&self, nonce: &GenericArray<u8, U32>) -> Result<(XChaCha20, Blake2bMac<U32>)> {
        let (ek, n2) = self
            .encryption_key_hasher
            .clone()
            .chain_update(b"atuin-paseto-encryption-key")
            .chain_update(nonce)
            .finalize_fixed()
            .split();

        let ak = self
            .authentication_key_hasher
            .clone()
            .chain_update(b"atuin-paseto-auth-key-for-aead")
            .chain_update(nonce)
            .finalize_fixed();

        let cipher = XChaCha20::new(&ek, &n2);
        let mac = Blake2bMac::<U32>::new_from_slice(&ak)
            .expect("32 byte key is equal to 32 byte block size");

        Ok((cipher, mac))
    }

    /// <https://github.com/paseto-standard/paseto-spec/blob/master/docs/01-Protocol-Versions/Version4.md#decrypt>
    pub fn encrypt(&self, history: History) -> Result<String> {
        // Step 3: Generate 32 random bytes from the OS's CSPRNG, n.
        let mut nonce = GenericArray::default();
        OsRng.fill_bytes(&mut nonce);

        // Step 4: Key splitting
        let (mut cipher, mac) = self.keys(&nonce)?;

        // Step 5.0: encode the message
        let mut plaintext = rmp_serde::to_vec(&HistoryPlaintext {
            duration: history.duration,
            exit: history.exit,
            command: history.command,
            cwd: history.cwd,
            session: history.session,
            hostname: history.hostname,
            timestamp: history.timestamp,
        })?;

        // Step 5: Encrypt the message using XChaCha20, using n2 from step 3 as the nonce and Ek as the key.
        cipher.apply_keystream(&mut plaintext);
        let mut ciphertext = plaintext;

        // Step 6: Pack h, n, c, f, and i together (in that order) using PAE. We'll call this preAuth.
        // h = HEADER
        // n = nonce
        // c = ciphertext
        // f = history_id
        // i = none
        // Step 7: Calculate BLAKE2b-MAC of the output of preAuth, using Ak as the authentication key. We'll call this t.
        let tag = pae(mac, &[HEADER, &nonce, &ciphertext, history.id.as_bytes()]);

        // Step 8: Encode the message `base64url(n || c || t)` (we store the header and footer elsewhere already)
        ciphertext.splice(0..0, nonce);
        ciphertext.extend(tag);
        Ok(BASE64_URL_SAFE.encode(&ciphertext))
    }

    /// <https://github.com/paseto-standard/paseto-spec/blob/master/docs/01-Protocol-Versions/Version4.md#decrypt>
    pub fn decrypt(&self, encrypted_history: &str, id: &str) -> Result<History> {
        // Step 4: Decode the payload from base64url to raw binary. Set:
        // n to the leftmost 32 bytes
        // t to the rightmost 32 bytes
        // c to the middle remainder of the payload, excluding n and t.
        let mut decoded = BASE64_URL_SAFE.decode(encrypted_history)?;
        if decoded.len() < 64 {
            bail!("encrypted message too short");
        }
        let (nonce, ciphertext) = decoded.split_at_mut(32);
        let (ciphertext, tag) = ciphertext.split_at_mut(ciphertext.len() - 32);

        // Step 5: Key splitting
        let (mut cipher, mac) = self.keys(GenericArray::from_slice(nonce))?;

        // Step 6: Pack h, n, c, f, and i together (in that order) using PAE. We'll call this preAuth.
        // h = HEADER
        // n = nonce
        // c = ciphertext
        // f = history_id
        // i = none
        // Step 7: Re-calculate BLAKE2b-MAC of the output of preAuth, using Ak as the authentication key. We'll call this t2.
        let tag2 = pae(mac, &[HEADER, nonce, ciphertext, id.as_bytes()]);

        // Step 8: Compare t with t2 using a constant-time string compare function. If they are not identical, throw an exception.
        if *tag != *tag2 {
            bail!("message authentication failed");
        }

        cipher.apply_keystream(ciphertext);
        let plaintext = ciphertext;

        let history: HistoryPlaintext = rmp_serde::from_slice(plaintext)?;

        Ok(History {
            id: id.to_owned(),
            cwd: history.cwd,
            exit: history.exit,
            command: history.command,
            session: history.session,
            duration: history.duration,
            hostname: history.hostname,
            timestamp: history.timestamp,
            deleted_at: None,
        })
    }
}

#[cfg(test)]
mod test {
    use crate::{history::History, sync::key};

    use super::Client;

    #[test]
    fn test_encrypt_decrypt() {
        let key = Client::new(&key::random());

        let history1 = History::new(
            chrono::Utc::now(),
            "ls".to_string(),
            "/home/ellie".to_string(),
            0,
            1,
            Some("beep boop".to_string()),
            Some("booop".to_string()),
            None,
        );
        let history2 = History {
            id: "another-id".to_owned(),
            ..history1.clone()
        };

        // same contents, different id, different encryption key
        let e1 = key.encrypt(history1.clone()).unwrap();
        let e2 = key.encrypt(history2.clone()).unwrap();

        assert_ne!(e1, e2);

        // test decryption works
        // this should pass
        match key.decrypt(&e1, &history1.id) {
            Err(e) => panic!("failed to decrypt, got {}", e),
            Ok(h) => assert_eq!(h, history1),
        };
        match key.decrypt(&e2, &history2.id) {
            Err(e) => panic!("failed to decrypt, got {}", e),
            Ok(h) => assert_eq!(h, history2),
        };

        // this should err
        let _ = key
            .decrypt(&e2, &history1.id)
            .expect_err("expected an error decrypting with invalid key");
    }
}

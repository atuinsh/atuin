// The general idea is that we NEVER send cleartext history to the server
// This way the odds of anything private ending up where it should not are
// very low
// The server authenticates via the usual username and password. This has
// nothing to do with the encryption, and is purely authentication! The client
// generates its own secret key, and encrypts all shell history with libsodium's
// secretbox. The data is then sent to the server, where it is stored. All
// clients must share the secret in order to be able to sync, as it is needed
// to decrypt

use std::{io::prelude::*, path::PathBuf};

use base64::prelude::{Engine, BASE64_STANDARD};
use eyre::{eyre, Context, Result};
use fs_err as fs;
use serde::{Deserialize, Serialize};
pub use xsalsa20poly1305::Key;
use xsalsa20poly1305::{
    aead::{Nonce, OsRng},
    AeadInPlace, KeyInit, XSalsa20Poly1305,
};

use crate::{history::History, settings::Settings};

#[derive(Debug, Serialize, Deserialize)]
pub struct EncryptedHistory {
    pub ciphertext: Vec<u8>,
    pub nonce: Nonce<XSalsa20Poly1305>,
}

pub fn new_key(settings: &Settings) -> Result<Key> {
    let path = settings.key_path.as_str();

    let key = XSalsa20Poly1305::generate_key(&mut OsRng);
    let encoded = encode_key(&key)?;

    let mut file = fs::File::create(path)?;
    file.write_all(encoded.as_bytes())?;

    Ok(key)
}

// Loads the secret key, will create + save if it doesn't exist
pub fn load_key(settings: &Settings) -> Result<Key> {
    let path = settings.key_path.as_str();

    let key = if PathBuf::from(path).exists() {
        let key = fs_err::read_to_string(path)?;
        decode_key(key)?
    } else {
        new_key(settings)?
    };

    Ok(key)
}

pub fn load_encoded_key(settings: &Settings) -> Result<String> {
    let path = settings.key_path.as_str();

    if PathBuf::from(path).exists() {
        let key = fs::read_to_string(path)?;
        Ok(key)
    } else {
        let key = XSalsa20Poly1305::generate_key(&mut OsRng);
        let encoded = encode_key(&key)?;

        let mut file = fs::File::create(path)?;
        file.write_all(encoded.as_bytes())?;

        Ok(encoded)
    }
}

pub fn encode_key(key: &Key) -> Result<String> {
    let buf = rmp_serde::to_vec(key.as_slice()).wrap_err("could not encode key to message pack")?;
    let buf = BASE64_STANDARD.encode(buf);

    Ok(buf)
}

pub fn decode_key(key: String) -> Result<Key> {
    let buf = BASE64_STANDARD
        .decode(key.trim_end())
        .wrap_err("encryption key is not a valid base64 encoding")?;
    let buf: &[u8] = rmp_serde::from_slice(&buf)
        .wrap_err("encryption key is not a valid message pack encoding")?;

    Ok(*Key::from_slice(buf))
}

pub fn encrypt(history: &History, key: &Key) -> Result<EncryptedHistory> {
    // serialize with msgpack
    let mut buf = rmp_serde::to_vec(history)?;

    let nonce = XSalsa20Poly1305::generate_nonce(&mut OsRng);
    XSalsa20Poly1305::new(key)
        .encrypt_in_place(&nonce, &[], &mut buf)
        .map_err(|_| eyre!("could not encrypt"))?;

    Ok(EncryptedHistory {
        ciphertext: buf,
        nonce,
    })
}

pub fn decrypt(mut encrypted_history: EncryptedHistory, key: &Key) -> Result<History> {
    XSalsa20Poly1305::new(key)
        .decrypt_in_place(
            &encrypted_history.nonce,
            &[],
            &mut encrypted_history.ciphertext,
        )
        .map_err(|_| eyre!("could not encrypt"))?;
    let plaintext = encrypted_history.ciphertext;

    let history = rmp_serde::from_slice(&plaintext)?;

    Ok(history)
}

#[cfg(test)]
mod test {
    use xsalsa20poly1305::{aead::OsRng, KeyInit, XSalsa20Poly1305};

    use crate::history::History;

    use super::{decrypt, encrypt};

    #[test]
    fn test_encrypt_decrypt() {
        let key1 = XSalsa20Poly1305::generate_key(&mut OsRng);
        let key2 = XSalsa20Poly1305::generate_key(&mut OsRng);

        let history = History::from_db()
            .id("1".into())
            .timestamp(chrono::Utc::now())
            .command("ls".into())
            .cwd("/home/ellie".into())
            .exit(0)
            .duration(1)
            .session("beep boop".into())
            .hostname("booop".into())
            .deleted_at(None)
            .interpreter(Some("zsh".into()))
            .build()
            .into();

        let e1 = encrypt(&history, &key1).unwrap();
        let e2 = encrypt(&history, &key2).unwrap();

        assert_ne!(e1.ciphertext, e2.ciphertext);
        assert_ne!(e1.nonce, e2.nonce);

        // test decryption works
        // this should pass
        match decrypt(e1, &key1) {
            Err(e) => panic!("failed to decrypt, got {}", e),
            Ok(h) => assert_eq!(h, history),
        };

        // this should err
        let _ = decrypt(e2, &key1).expect_err("expected an error decrypting with invalid key");
    }
}

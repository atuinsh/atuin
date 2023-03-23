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
    aead::{Aead, Nonce, OsRng},
    KeyInit, XSalsa20Poly1305,
};

use crate::{
    history::{History, HistoryWithoutDelete},
    settings::Settings,
};

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
    let buf = rmp_serde::to_vec(key).wrap_err("could not encode key to message pack")?;
    let buf = BASE64_STANDARD.encode(buf);

    Ok(buf)
}

pub fn decode_key(key: String) -> Result<Key> {
    let buf = BASE64_STANDARD
        .decode(key.trim_end())
        .wrap_err("encryption key is not a valid base64 encoding")?;
    let buf: [u8; 32] = rmp_serde::from_slice(&buf)
        .wrap_err("encryption key is not a valid message pack encoding")?;

    Ok(buf.into())
}

pub fn encrypt(history: &History, key: &Key) -> Result<EncryptedHistory> {
    // serialize with msgpack
    let buf = rmp_serde::to_vec(history)?;

    let nonce = XSalsa20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = XSalsa20Poly1305::new(key)
        .encrypt(&nonce, buf.as_slice())
        .map_err(|_| eyre!("could not encrypt"))?;

    Ok(EncryptedHistory { ciphertext, nonce })
}

pub fn decrypt(encrypted_history: &EncryptedHistory, key: &Key) -> Result<History> {
    let plaintext = XSalsa20Poly1305::new(key)
        .decrypt(
            &encrypted_history.nonce,
            encrypted_history.ciphertext.as_slice(),
        )
        .map_err(|_| eyre!("could not encrypt"))?;

    let history = rmp_serde::from_slice(&plaintext);

    let Ok(history) = history else {
        let history: HistoryWithoutDelete = rmp_serde::from_slice(&plaintext)?;

        return Ok(History {
            id: history.id,
            cwd: history.cwd,
            exit: history.exit,
            command: history.command,
            session: history.session,
            duration: history.duration,
            hostname: history.hostname,
            timestamp: history.timestamp,
            deleted_at: None,
        });
    };

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

        let history = History::new(
            chrono::Utc::now(),
            "ls".to_string(),
            "/home/ellie".to_string(),
            0,
            1,
            Some("beep boop".to_string()),
            Some("booop".to_string()),
            None,
        );

        let e1 = encrypt(&history, &key1).unwrap();
        let e2 = encrypt(&history, &key2).unwrap();

        assert_ne!(e1.ciphertext, e2.ciphertext);
        assert_ne!(e1.nonce, e2.nonce);

        // test decryption works
        // this should pass
        match decrypt(&e1, &key1) {
            Err(e) => panic!("failed to decrypt, got {}", e),
            Ok(h) => assert_eq!(h, history),
        };

        // this should err
        let _ = decrypt(&e2, &key1).expect_err("expected an error decrypting with invalid key");
    }

    // #[test]
    // fn test_encrypt_decrypt_cryptobox() {
    //     let key1 = XSalsa20Poly1305::generate_key(&mut OsRng);
    //     let key2 = secretbox::Key::from_slice(&key1).unwrap();

    //     let payload = "blahblahblahblahblahblahblahblahb".repeat(20);

    //     let nonce1 = XSalsa20Poly1305::generate_nonce(&mut OsRng);
    //     let nonce2 = secretbox::Nonce::from_slice(&nonce1).unwrap();

    //     let sealed = secretbox::seal(payload.as_bytes(), &nonce2, &key2);

    //     let output = XSalsa20Poly1305::new(&key1)
    //         .decrypt(&nonce1, sealed.as_slice())
    //         .unwrap();

    //     // let output = crypto_box::seal_open(&key1, &sealed).unwrap();
    //     assert_eq!(output, payload.as_bytes());
    // }
}

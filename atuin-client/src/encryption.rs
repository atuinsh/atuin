// The general idea is that we NEVER send cleartext history to the server
// This way the odds of anything private ending up where it should not are
// very low
// The server authenticates via the usual username and password. This has
// nothing to do with the encryption, and is purely authentication! The client
// generates its own secret key, and encrypts all shell history with libsodium's
// secretbox. The data is then sent to the server, where it is stored. All
// clients must share the secret in order to be able to sync, as it is needed
// to decrypt

use std::fs::File;
use std::io::prelude::*;
use std::path::PathBuf;

use eyre::{eyre, Result};
use sodiumoxide::crypto::secretbox;

use crate::history::History;
use crate::settings::Settings;

#[derive(Debug, Serialize, Deserialize)]
pub struct EncryptedHistory {
    pub ciphertext: Vec<u8>,
    pub nonce: secretbox::Nonce,
}

// Loads the secret key, will create + save if it doesn't exist
pub fn load_key(settings: &Settings) -> Result<secretbox::Key> {
    let path = settings.key_path.as_str();

    if PathBuf::from(path).exists() {
        let key = std::fs::read_to_string(path)?;
        let key = decode_key(key)?;
        Ok(key)
    } else {
        let key = secretbox::gen_key();
        let encoded = encode_key(key.clone())?;

        let mut file = File::create(path)?;
        file.write_all(encoded.as_bytes())?;

        Ok(key)
    }
}

pub fn load_encoded_key(settings: &Settings) -> Result<String> {
    let path = settings.key_path.as_str();

    if PathBuf::from(path).exists() {
        let key = std::fs::read_to_string(path)?;
        Ok(key)
    } else {
        let key = secretbox::gen_key();
        let encoded = encode_key(key)?;

        let mut file = File::create(path)?;
        file.write_all(encoded.as_bytes())?;

        Ok(encoded)
    }
}

pub fn encode_key(key: secretbox::Key) -> Result<String> {
    let buf = rmp_serde::to_vec(&key)?;
    let buf = base64::encode(buf);

    Ok(buf)
}

pub fn decode_key(key: String) -> Result<secretbox::Key> {
    let buf = base64::decode(key)?;
    let buf: secretbox::Key = rmp_serde::from_read_ref(&buf)?;

    Ok(buf)
}

pub fn encrypt(history: &History, key: &secretbox::Key) -> Result<EncryptedHistory> {
    // serialize with msgpack
    let buf = rmp_serde::to_vec(history)?;

    let nonce = secretbox::gen_nonce();

    let ciphertext = secretbox::seal(&buf, &nonce, key);

    Ok(EncryptedHistory { ciphertext, nonce })
}

pub fn decrypt(encrypted_history: &EncryptedHistory, key: &secretbox::Key) -> Result<History> {
    let plaintext = secretbox::open(&encrypted_history.ciphertext, &encrypted_history.nonce, key)
        .map_err(|_| eyre!("failed to open secretbox - invalid key?"))?;

    let history = rmp_serde::from_read_ref(&plaintext)?;

    Ok(history)
}

#[cfg(test)]
mod test {
    use sodiumoxide::crypto::secretbox;

    use crate::history::History;

    use super::{decrypt, encrypt};

    #[test]
    fn test_encrypt_decrypt() {
        let key1 = secretbox::gen_key();
        let key2 = secretbox::gen_key();

        let history = History::new(
            chrono::Utc::now(),
            "ls".to_string(),
            "/home/ellie".to_string(),
            0,
            1,
            Some("beep boop".to_string()),
            Some("booop".to_string()),
        );

        let e1 = encrypt(&history, &key1).unwrap();
        let e2 = encrypt(&history, &key2).unwrap();

        assert_ne!(e1.ciphertext, e2.ciphertext);
        assert_ne!(e1.nonce, e2.nonce);

        // test decryption works
        // this should pass
        match decrypt(&e1, &key1) {
            Err(e) => assert!(false, "failed to decrypt, got {}", e),
            Ok(h) => assert_eq!(h, history),
        };

        // this should err
        match decrypt(&e2, &key1) {
            Ok(_) => assert!(false, "expected an error decrypting with invalid key"),
            Err(_) => {}
        };
    }
}

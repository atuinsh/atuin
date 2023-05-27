// The general idea is that we NEVER send cleartext history to the server
// This way the odds of anything private ending up where it should not are
// very low
// The server authenticates via the usual username and password. This has
// nothing to do with the encryption, and is purely authentication! The client
// generates its own secret key, and encrypts all shell history with libsodium's
// secretbox. The data is then sent to the server, where it is stored. All
// clients must share the secret in order to be able to sync, as it is needed
// to decrypt

use std::path::Path;

use base64::prelude::{Engine, BASE64_STANDARD};
use eyre::{eyre, Context, Result};
use serde::{Deserialize, Serialize};
pub use xsalsa20poly1305::Key;
use xsalsa20poly1305::{
    aead::{Nonce, OsRng},
    AeadInPlace, KeyInit, XSalsa20Poly1305,
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

pub fn new_key(username: &str, settings: &Settings) -> Result<Key> {
    let entry = keyring::Entry::new("atuin", username).ok();
    new_key_inner(entry, settings)
}

pub fn save_key(username: &str, settings: &Settings, key: &Key) -> Result<()> {
    let entry = keyring::Entry::new("atuin", username).ok();
    save_key_inner(entry, settings, key)
}

fn new_key_inner(entry: Option<keyring::Entry>, settings: &Settings) -> Result<Key> {
    let key = XSalsa20Poly1305::generate_key(&mut OsRng);
    save_key_inner(entry, settings, &key)?;
    Ok(key)
}

fn save_key_inner(entry: Option<keyring::Entry>, settings: &Settings, key: &Key) -> Result<()> {
    let path = settings.key_path.as_str();

    let encoded = encode_key(key)?;

    // prefer keyring
    if let Some(entry) = entry {
        entry.set_password(&encoded)?;
    }

    // write to the file system too for now
    fs_err::write(path, encoded.as_bytes())?;

    Ok(())
}

// Loads the secret key, will create + save if it doesn't exist
pub fn load_key(username: &str, settings: &Settings) -> Result<Key> {
    let path = settings.key_path.as_str();

    let entry = keyring::Entry::new("atuin", username).ok();
    // prefer the keyring
    let key = match entry.as_ref().map(|e| e.get_password()) {
        Some(Ok(key)) => decode_key(key)?,
        _ if Path::new(path).exists() => decode_key(fs_err::read_to_string(path)?)?,
        _ => {
            let key = XSalsa20Poly1305::generate_key(&mut OsRng);
            save_key_inner(entry, settings, &key)?;
            key
        }
    };

    Ok(key)
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

    let mbuf: Result<[u8; 32]> =
        rmp_serde::from_slice(&buf).wrap_err("encryption key is not a valid message pack encoding");

    match mbuf {
        Ok(b) => Ok(*Key::from_slice(&b)),
        Err(_) => {
            let buf: &[u8] = rmp_serde::from_slice(&buf)
                .wrap_err("encryption key is not a valid message pack encoding")?;

            Ok(*Key::from_slice(buf))
        }
    }
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
        match decrypt(e1, &key1) {
            Err(e) => panic!("failed to decrypt, got {}", e),
            Ok(h) => assert_eq!(h, history),
        };

        // this should err
        let _ = decrypt(e2, &key1).expect_err("expected an error decrypting with invalid key");
    }
}

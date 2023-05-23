// The general idea is that we NEVER send cleartext history to the server
// This way the odds of anything private ending up where it should not are
// very low
// The server authenticates via the usual username and password. This has
// nothing to do with the encryption, and is purely authentication! The client
// generates its own secret key, and encrypts all shell history with libsodium's
// secretbox. The data is then sent to the server, where it is stored. All
// clients must share the secret in order to be able to sync, as it is needed
// to decrypt

pub mod key {
    use std::path::Path;

    use base64::prelude::{Engine, BASE64_STANDARD};
    use eyre::{Context, Result};
    pub use xsalsa20poly1305::Key;
    use xsalsa20poly1305::{aead::OsRng, KeyInit, XSalsa20Poly1305};

    use crate::settings::Settings;

    pub fn new(settings: &Settings) -> Result<Key> {
        let path = settings.key_path.as_str();

        let key = XSalsa20Poly1305::generate_key(&mut OsRng);
        let encoded = encode(&key)?;

        fs_err::write(path, encoded.as_bytes())?;

        Ok(key)
    }

    // Loads the secret key, will create + save if it doesn't exist
    pub fn load(settings: &Settings) -> Result<Key> {
        let path = settings.key_path.as_str();

        let key = if Path::new(path).exists() {
            let key = fs_err::read_to_string(path)?;
            decode(key)?
        } else {
            new(settings)?
        };

        Ok(key)
    }

    pub fn encode(key: &Key) -> Result<String> {
        let buf =
            rmp_serde::to_vec(key.as_slice()).wrap_err("could not encode key to message pack")?;
        let buf = BASE64_STANDARD.encode(buf);

        Ok(buf)
    }

    pub fn decode(key: String) -> Result<Key> {
        let buf = BASE64_STANDARD
            .decode(key.trim_end())
            .wrap_err("encryption key is not a valid base64 encoding")?;

        let mbuf: Result<[u8; 32]> = rmp_serde::from_slice(&buf)
            .wrap_err("encryption key is not a valid message pack encoding");

        match mbuf {
            Ok(b) => Ok(*Key::from_slice(&b)),
            Err(_) => {
                let buf: &[u8] = rmp_serde::from_slice(&buf)
                    .wrap_err("encryption key is not a valid message pack encoding")?;

                Ok(*Key::from_slice(buf))
            }
        }
    }
}

// DO NOT MODIFY. We can't change old encryption schemes, only add new ones.
pub mod xsalsa20poly1305legacy {
    use chrono::Utc;
    use eyre::{eyre, Result};
    use serde::{Deserialize, Serialize};
    use xsalsa20poly1305::{
        aead::{Nonce, OsRng},
        AeadInPlace, Key, KeyInit, XSalsa20Poly1305,
    };

    #[derive(Debug, Serialize, Deserialize)]
    struct EncryptedHistory {
        pub ciphertext: Vec<u8>,
        pub nonce: Nonce<XSalsa20Poly1305>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    pub struct HistoryWithoutDelete {
        pub id: String,
        pub timestamp: chrono::DateTime<Utc>,
        pub duration: i64,
        pub exit: i64,
        pub command: String,
        pub cwd: String,
        pub session: String,
        pub hostname: String,
    }

    use crate::history::History;

    pub fn encrypt(history: &History, key: &Key) -> Result<String> {
        // serialize with msgpack
        let mut buf = rmp_serde::to_vec(history)?;

        let nonce = XSalsa20Poly1305::generate_nonce(&mut OsRng);
        XSalsa20Poly1305::new(key)
            .encrypt_in_place(&nonce, &[], &mut buf)
            .map_err(|_| eyre!("could not encrypt"))?;

        let record = serde_json::to_string(&EncryptedHistory {
            ciphertext: buf,
            nonce,
        })?;

        Ok(record)
    }

    pub fn decrypt(encrypted_history: String, key: &Key) -> Result<History> {
        let mut decoded: EncryptedHistory = serde_json::from_str(&encrypted_history)?;

        XSalsa20Poly1305::new(key)
            .decrypt_in_place(&decoded.nonce, &[], &mut decoded.ciphertext)
            .map_err(|_| eyre!("could not decrypt"))?;
        let plaintext = decoded.ciphertext;

        let history = rmp_serde::from_slice(&plaintext);

        // ugly hack because we broke things
        let Ok(history) = history else {
            // fallback to without deleted_at
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

            assert_ne!(e1, e2);

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
}

// DO NOT MODIFY. We can't change old encryption schemes, only add new ones.
pub mod xchacha20poly1305 {
    use super::key::Key;
    use chacha20poly1305::{
        aead::{Nonce, OsRng},
        AeadCore, AeadInPlace, KeyInit, XChaCha20Poly1305,
    };
    use chrono::Utc;
    use eyre::{eyre, Result};
    use hkdf::Hkdf;
    use serde::{Deserialize, Serialize};
    use sha2::Sha256;

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
        pub nonce: Nonce<XChaCha20Poly1305>,
    }

    use crate::history::History;

    fn content_key(key: &Key, id: &str) -> Result<chacha20poly1305::Key> {
        let mut content_key = chacha20poly1305::Key::default();
        Hkdf::<Sha256>::new(Some(b"history"), key)
            .expand(id.as_bytes(), &mut content_key)
            .map_err(|_| eyre!("could not derive encryption key"))?;
        Ok(content_key)
    }

    pub fn encrypt(history: &History, key: &Key) -> Result<String> {
        // a unique encryption key for this entry
        let content_key = content_key(key, &history.id)?;

        let mut plaintext = rmp_serde::to_vec(&HistoryPlaintext {
            duration: history.duration,
            exit: history.exit,
            command: history.command.to_owned(),
            cwd: history.cwd.to_owned(),
            session: history.session.to_owned(),
            hostname: history.hostname.to_owned(),
            timestamp: history.timestamp,
        })?;

        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        XChaCha20Poly1305::new(&content_key)
            .encrypt_in_place(&nonce, history.id.as_bytes(), &mut plaintext)
            .map_err(|_| eyre!("could not encrypt"))?;

        let record = serde_json::to_string(&EncryptedHistory {
            ciphertext: plaintext,
            nonce,
        })?;

        Ok(record)
    }

    pub fn decrypt(encrypted_history: &str, key: &Key, id: &str) -> Result<History> {
        let content_key = content_key(key, id)?;

        let mut decoded: EncryptedHistory = serde_json::from_str(encrypted_history)?;

        XChaCha20Poly1305::new(&content_key)
            .decrypt_in_place(&decoded.nonce, id.as_bytes(), &mut decoded.ciphertext)
            .map_err(|_| eyre!("could not decrypt"))?;
        let plaintext = decoded.ciphertext;

        let history: HistoryPlaintext = rmp_serde::from_slice(&plaintext)?;

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

    #[cfg(test)]
    mod test {
        use chacha20poly1305::{aead::OsRng, KeyInit};
        use xsalsa20poly1305::XSalsa20Poly1305;

        use crate::history::History;

        use super::{decrypt, encrypt};

        #[test]
        fn test_encrypt_decrypt() {
            let key = XSalsa20Poly1305::generate_key(&mut OsRng);

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
            let e1 = encrypt(&history1, &key).unwrap();
            let e2 = encrypt(&history2, &key).unwrap();

            assert_ne!(e1, e2);

            // test decryption works
            // this should pass
            match decrypt(&e1, &key, &history1.id) {
                Err(e) => panic!("failed to decrypt, got {}", e),
                Ok(h) => assert_eq!(h, history1),
            };
            match decrypt(&e2, &key, &history2.id) {
                Err(e) => panic!("failed to decrypt, got {}", e),
                Ok(h) => assert_eq!(h, history2),
            };

            // this should err
            let _ = decrypt(&e2, &key, &history1.id)
                .expect_err("expected an error decrypting with invalid key");
        }
    }
}

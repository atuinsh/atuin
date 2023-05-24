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
    use eyre::{bail, eyre, Result};
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

    pub struct Client {
        inner: XSalsa20Poly1305,
    }

    use crate::history::History;

    impl Client {
        pub fn new(key: &Key) -> Self {
            Self {
                inner: XSalsa20Poly1305::new(key),
            }
        }

        pub fn encrypt(&self, history: &History) -> Result<String> {
            // serialize with msgpack
            let mut buf = rmp_serde::to_vec(&history)?;

            let nonce = XSalsa20Poly1305::generate_nonce(&mut OsRng);
            self.inner
                .encrypt_in_place(&nonce, &[], &mut buf)
                .map_err(|_| eyre!("could not encrypt"))?;

            let record = serde_json::to_string(&EncryptedHistory {
                ciphertext: buf,
                nonce,
            })?;

            Ok(record)
        }

        pub fn decrypt(&self, encrypted_history: &str, id: &str) -> Result<History> {
            let mut decoded: EncryptedHistory = serde_json::from_str(encrypted_history)?;

            self.inner
                .decrypt_in_place(&decoded.nonce, &[], &mut decoded.ciphertext)
                .map_err(|_| eyre!("could not decrypt"))?;
            let plaintext = decoded.ciphertext;

            let history = rmp_serde::from_slice(&plaintext);

            // ugly hack because we broke things
            let history = match history {
                Ok(history) => history,
                Err(_) => {
                    // fallback to without deleted_at
                    let history: HistoryWithoutDelete = rmp_serde::from_slice(&plaintext)?;

                    History {
                        id: history.id,
                        cwd: history.cwd,
                        exit: history.exit,
                        command: history.command,
                        session: history.session,
                        duration: history.duration,
                        hostname: history.hostname,
                        timestamp: history.timestamp,
                        deleted_at: None,
                    }
                }
            };

            if history.id != id {
                bail!("encryption integrity check failed")
            }

            Ok(history)
        }
    }

    #[cfg(test)]
    mod test {
        use xsalsa20poly1305::{aead::OsRng, KeyInit, XSalsa20Poly1305};

        use crate::history::History;

        use super::Client;

        #[test]
        fn test_encrypt_decrypt() {
            let key1 = Client::new(&XSalsa20Poly1305::generate_key(&mut OsRng));
            let key2 = Client::new(&XSalsa20Poly1305::generate_key(&mut OsRng));

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

            let e1 = key1.encrypt(&history).unwrap();
            let e2 = key2.encrypt(&history).unwrap();

            assert_ne!(e1, e2);

            // test decryption works
            // this should pass
            match key1.decrypt(&e1, &history.id) {
                Err(e) => panic!("failed to decrypt, got {}", e),
                Ok(h) => assert_eq!(h, history),
            };

            // this should err
            let _ = key1
                .decrypt(&e2, &history.id)
                .expect_err("expected an error decrypting with invalid key");

            // this should err
            let _ = key2
                .decrypt(&e2, "bad id")
                .expect_err("expected an error decrypting with incorrect id");
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

    pub struct Client {
        inner: Hkdf<Sha256>,
    }

    impl Client {
        pub fn new(key: &Key) -> Self {
            Self {
                // constant 'salt' is important and actually helps with security, while helping to improving performance
                // <https://soatok.blog/2021/11/17/understanding-hkdf/>
                inner: Hkdf::<Sha256>::new(Some(b"history"), key),
            }
        }

        fn cipher(&self, id: &str) -> Result<XChaCha20Poly1305> {
            let mut content_key = chacha20poly1305::Key::default();
            self.inner
                .expand(id.as_bytes(), &mut content_key)
                .map_err(|_| eyre!("could not derive encryption key"))?;
            Ok(XChaCha20Poly1305::new(&content_key))
        }

        pub fn encrypt(&self, history: History) -> Result<String> {
            let mut plaintext = rmp_serde::to_vec(&HistoryPlaintext {
                duration: history.duration,
                exit: history.exit,
                command: history.command,
                cwd: history.cwd,
                session: history.session,
                hostname: history.hostname,
                timestamp: history.timestamp,
            })?;

            let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
            self.cipher(&history.id)?
                .encrypt_in_place(&nonce, history.id.as_bytes(), &mut plaintext)
                .map_err(|_| eyre!("could not encrypt"))?;

            let record = serde_json::to_string(&EncryptedHistory {
                ciphertext: plaintext,
                nonce,
            })?;

            Ok(record)
        }

        pub fn decrypt(&self, encrypted_history: &str, id: &str) -> Result<History> {
            let mut decoded: EncryptedHistory = serde_json::from_str(encrypted_history)?;

            self.cipher(id)?
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
    }

    #[cfg(test)]
    mod test {
        use chacha20poly1305::{aead::OsRng, KeyInit};
        use xsalsa20poly1305::XSalsa20Poly1305;

        use crate::history::History;

        use super::Client;

        #[test]
        fn test_encrypt_decrypt() {
            let key = Client::new(&XSalsa20Poly1305::generate_key(&mut OsRng));

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
}

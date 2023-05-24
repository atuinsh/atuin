// DO NOT MODIFY. We can't change old encryption schemes, only add new ones.

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

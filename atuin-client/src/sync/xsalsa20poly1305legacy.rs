// DO NOT MODIFY. We can't change old encryption schemes, only add new ones.

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

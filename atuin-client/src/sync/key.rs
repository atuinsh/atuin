use std::path::Path;

use base64::prelude::{Engine, BASE64_STANDARD};
use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use eyre::{Context, Result};
use generic_array::typenum::U32;

use crate::settings::Settings;

pub type Key = generic_array::GenericArray<u8, U32>;

pub fn random() -> Key {
    let mut key = Key::default();
    OsRng.fill_bytes(&mut key);
    key
}

pub fn new(settings: &Settings) -> Result<Key> {
    let path = settings.key_path.as_str();

    let key = random();
    let encoded = encode(&key)?;

    fs_err::write(path, encoded.as_bytes())?;

    Ok(key)
}

// Loads the secret key, will create + save if it doesn't exist
pub fn load(settings: &Settings) -> Result<Key> {
    let path = settings.key_path.as_str();

    let key = if Path::new(path).exists() {
        let key = fs_err::read_to_string(path)?;
        decode(&key)?
    } else {
        new(settings)?
    };

    Ok(key)
}

pub fn encode(key: &Key) -> Result<String> {
    let buf = rmp_serde::to_vec(key.as_slice()).wrap_err("could not encode key to message pack")?;
    let buf = BASE64_STANDARD.encode(buf);

    Ok(buf)
}

pub fn decode(key: &str) -> Result<Key> {
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

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
use chrono::{DateTime, Utc};
use eyre::{bail, eyre, Context, Result};
use fs_err as fs;
use rmp::{decode::Bytes, Marker};
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
    let mut buf = vec![];
    rmp::encode::write_bin(&mut buf, key.as_slice())
        .wrap_err("could not encode key to message pack")?;
    let buf = BASE64_STANDARD.encode(buf);

    Ok(buf)
}

pub fn decode_key(key: String) -> Result<Key> {
    let buf = BASE64_STANDARD
        .decode(key.trim_end())
        .wrap_err("encryption key is not a valid base64 encoding")?;

    // old code wrote the key as a fixed length array of 32 bytes
    // new code writes the key with a length prefix
    if buf.len() == 32 {
        Ok(*Key::from_slice(&buf))
    } else {
        let mut bytes = Bytes::new(&buf);
        let key_len = rmp::decode::read_bin_len(&mut bytes).map_err(error_report)?;
        if key_len != 32 || bytes.remaining_slice().len() != key_len as usize {
            bail!("encryption key is not the correct size")
        }
        Ok(*Key::from_slice(bytes.remaining_slice()))
    }
}

pub fn encrypt(history: &History, key: &Key) -> Result<EncryptedHistory> {
    // serialize with msgpack
    let mut buf = encode(history)?;

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

    let history = decode(&plaintext)?;

    Ok(history)
}

fn encode(h: &History) -> Result<Vec<u8>> {
    use rmp::encode;

    let mut output = vec![];
    // INFO: ensure this is updated when adding new fields
    encode::write_array_len(&mut output, 9)?;

    encode::write_str(&mut output, &h.id)?;
    encode::write_str(
        &mut output,
        &dbg!(h
            .timestamp
            .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)),
    )?;
    encode::write_sint(&mut output, h.duration)?;
    encode::write_sint(&mut output, h.exit)?;
    encode::write_str(&mut output, &h.command)?;
    encode::write_str(&mut output, &h.cwd)?;
    encode::write_str(&mut output, &h.session)?;
    encode::write_str(&mut output, &h.hostname)?;
    match h.deleted_at {
        Some(d) => encode::write_str(
            &mut output,
            &d.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
        )?,
        None => encode::write_nil(&mut output)?,
    }

    Ok(output)
}

fn decode(bytes: &[u8]) -> Result<History> {
    use rmp::decode::{self, DecodeStringError};

    let mut bytes = Bytes::new(bytes);

    let nfields = decode::read_array_len(&mut bytes).map_err(error_report)?;
    if nfields < 8 {
        bail!("malformed decrypted history")
    }
    if nfields > 9 {
        bail!("cannot decrypt history from a newer version of atuin");
    }

    let bytes = bytes.remaining_slice();
    let (id, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
    let (timestamp, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;

    let mut bytes = Bytes::new(bytes);
    let duration = decode::read_int(&mut bytes).map_err(error_report)?;
    let exit = decode::read_int(&mut bytes).map_err(error_report)?;

    let bytes = bytes.remaining_slice();
    let (command, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
    let (cwd, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
    let (session, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
    let (hostname, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;

    // if we have more fields, try and get the deleted_at
    let mut deleted_at = None;
    let mut bytes = bytes;
    if nfields > 8 {
        bytes = match decode::read_str_from_slice(bytes) {
            Ok((d, b)) => {
                deleted_at = Some(d);
                b
            }
            // we accept null here
            Err(DecodeStringError::TypeMismatch(Marker::Null)) => {
                // consume the null marker
                let mut c = Bytes::new(bytes);
                decode::read_nil(&mut c).map_err(error_report)?;
                c.remaining_slice()
            }
            Err(err) => return Err(error_report(err)),
        };
    }

    if !bytes.is_empty() {
        bail!("trailing bytes in encoded history. malformed")
    }

    Ok(History {
        id: id.to_owned(),
        timestamp: DateTime::parse_from_rfc3339(timestamp)?.with_timezone(&Utc),
        duration,
        exit,
        command: command.to_owned(),
        cwd: cwd.to_owned(),
        session: session.to_owned(),
        hostname: hostname.to_owned(),
        deleted_at: deleted_at
            .map(DateTime::parse_from_rfc3339)
            .transpose()?
            .map(|dt| dt.with_timezone(&Utc)),
    })
}

fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
    eyre!("{err:?}")
}

#[cfg(test)]
mod test {
    use xsalsa20poly1305::{aead::OsRng, KeyInit, XSalsa20Poly1305};

    use crate::history::History;

    use super::{decode, decrypt, encode, encrypt};

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

    #[test]
    fn test_decode() {
        let bytes = [
            0x99, 0xD9, 32, 54, 54, 100, 49, 54, 99, 98, 101, 101, 55, 99, 100, 52, 55, 53, 51, 56,
            101, 53, 99, 53, 98, 56, 98, 52, 52, 101, 57, 48, 48, 54, 101, 187, 50, 48, 50, 51, 45,
            48, 53, 45, 50, 56, 84, 49, 56, 58, 51, 53, 58, 52, 48, 46, 54, 51, 51, 56, 55, 50, 90,
            206, 2, 238, 210, 240, 0, 170, 103, 105, 116, 32, 115, 116, 97, 116, 117, 115, 217, 42,
            47, 85, 115, 101, 114, 115, 47, 99, 111, 110, 114, 97, 100, 46, 108, 117, 100, 103, 97,
            116, 101, 47, 68, 111, 99, 117, 109, 101, 110, 116, 115, 47, 99, 111, 100, 101, 47, 97,
            116, 117, 105, 110, 217, 32, 98, 57, 55, 100, 57, 97, 51, 48, 54, 102, 50, 55, 52, 52,
            55, 51, 97, 50, 48, 51, 100, 50, 101, 98, 97, 52, 49, 102, 57, 52, 53, 55, 187, 102,
            118, 102, 103, 57, 51, 54, 99, 48, 107, 112, 102, 58, 99, 111, 110, 114, 97, 100, 46,
            108, 117, 100, 103, 97, 116, 101, 192,
        ];
        let history = History {
            id: "66d16cbee7cd47538e5c5b8b44e9006e".to_owned(),
            timestamp: "2023-05-28T18:35:40.633872Z".parse().unwrap(),
            duration: 49206000,
            exit: 0,
            command: "git status".to_owned(),
            cwd: "/Users/conrad.ludgate/Documents/code/atuin".to_owned(),
            session: "b97d9a306f274473a203d2eba41f9457".to_owned(),
            hostname: "fvfg936c0kpf:conrad.ludgate".to_owned(),
            deleted_at: None,
        };

        let h = decode(&bytes).unwrap();
        assert_eq!(history, h);

        let b = encode(&h).unwrap();
        assert_eq!(&bytes, &*b);
    }

    #[test]
    fn test_decode_deleted() {
        let history = History {
            id: "66d16cbee7cd47538e5c5b8b44e9006e".to_owned(),
            timestamp: "2023-05-28T18:35:40.633872Z".parse().unwrap(),
            duration: 49206000,
            exit: 0,
            command: "git status".to_owned(),
            cwd: "/Users/conrad.ludgate/Documents/code/atuin".to_owned(),
            session: "b97d9a306f274473a203d2eba41f9457".to_owned(),
            hostname: "fvfg936c0kpf:conrad.ludgate".to_owned(),
            deleted_at: Some("2023-05-28T18:35:40.633872Z".parse().unwrap()),
        };

        let b = encode(&history).unwrap();
        let h = decode(&b).unwrap();
        assert_eq!(history, h);
    }

    #[test]
    fn test_decode_old() {
        let bytes = [
            0x98, 0xD9, 32, 54, 54, 100, 49, 54, 99, 98, 101, 101, 55, 99, 100, 52, 55, 53, 51, 56,
            101, 53, 99, 53, 98, 56, 98, 52, 52, 101, 57, 48, 48, 54, 101, 187, 50, 48, 50, 51, 45,
            48, 53, 45, 50, 56, 84, 49, 56, 58, 51, 53, 58, 52, 48, 46, 54, 51, 51, 56, 55, 50, 90,
            206, 2, 238, 210, 240, 0, 170, 103, 105, 116, 32, 115, 116, 97, 116, 117, 115, 217, 42,
            47, 85, 115, 101, 114, 115, 47, 99, 111, 110, 114, 97, 100, 46, 108, 117, 100, 103, 97,
            116, 101, 47, 68, 111, 99, 117, 109, 101, 110, 116, 115, 47, 99, 111, 100, 101, 47, 97,
            116, 117, 105, 110, 217, 32, 98, 57, 55, 100, 57, 97, 51, 48, 54, 102, 50, 55, 52, 52,
            55, 51, 97, 50, 48, 51, 100, 50, 101, 98, 97, 52, 49, 102, 57, 52, 53, 55, 187, 102,
            118, 102, 103, 57, 51, 54, 99, 48, 107, 112, 102, 58, 99, 111, 110, 114, 97, 100, 46,
            108, 117, 100, 103, 97, 116, 101,
        ];
        let history = History {
            id: "66d16cbee7cd47538e5c5b8b44e9006e".to_owned(),
            timestamp: "2023-05-28T18:35:40.633872Z".parse().unwrap(),
            duration: 49206000,
            exit: 0,
            command: "git status".to_owned(),
            cwd: "/Users/conrad.ludgate/Documents/code/atuin".to_owned(),
            session: "b97d9a306f274473a203d2eba41f9457".to_owned(),
            hostname: "fvfg936c0kpf:conrad.ludgate".to_owned(),
            deleted_at: None,
        };

        let h = decode(&bytes).unwrap();
        assert_eq!(history, h);
    }
}

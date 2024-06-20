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
pub use crypto_secretbox::Key;
use crypto_secretbox::{
    aead::{Nonce, OsRng},
    AeadCore, AeadInPlace, KeyInit, XSalsa20Poly1305,
};
use eyre::{bail, ensure, eyre, Context, Result};
use fs_err as fs;
use rmp::{decode::Bytes, Marker};
use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, macros::format_description, OffsetDateTime};

use crate::{history::History, settings::Settings};

#[derive(Debug, Serialize, Deserialize)]
pub struct EncryptedHistory {
    pub ciphertext: Vec<u8>,
    pub nonce: Nonce<XSalsa20Poly1305>,
}

pub fn generate_encoded_key() -> Result<(Key, String)> {
    let key = XSalsa20Poly1305::generate_key(&mut OsRng);
    let encoded = encode_key(&key)?;

    Ok((key, encoded))
}

pub fn new_key(settings: &Settings) -> Result<Key> {
    let path = settings.key_path.as_str();
    let path = PathBuf::from(path);

    if path.exists() {
        bail!("key already exists! cannot overwrite");
    }

    let (key, encoded) = generate_encoded_key()?;

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

pub fn encode_key(key: &Key) -> Result<String> {
    let mut buf = vec![];
    rmp::encode::write_array_len(&mut buf, key.len() as u32)
        .wrap_err("could not encode key to message pack")?;
    for b in key {
        rmp::encode::write_uint(&mut buf, *b as u64)
            .wrap_err("could not encode key to message pack")?;
    }
    let buf = BASE64_STANDARD.encode(buf);

    Ok(buf)
}

pub fn decode_key(key: String) -> Result<Key> {
    use rmp::decode;

    let buf = BASE64_STANDARD
        .decode(key.trim_end())
        .wrap_err("encryption key is not a valid base64 encoding")?;

    // old code wrote the key as a fixed length array of 32 bytes
    // new code writes the key with a length prefix
    match <[u8; 32]>::try_from(&*buf) {
        Ok(key) => Ok(key.into()),
        Err(_) => {
            let mut bytes = rmp::decode::Bytes::new(&buf);

            match Marker::from_u8(buf[0]) {
                Marker::Bin8 => {
                    let len = decode::read_bin_len(&mut bytes).map_err(|err| eyre!("{err:?}"))?;
                    ensure!(len == 32, "encryption key is not the correct size");
                    let key = <[u8; 32]>::try_from(bytes.remaining_slice())
                        .context("could not decode encryption key")?;
                    Ok(key.into())
                }
                Marker::Array16 => {
                    let len = decode::read_array_len(&mut bytes).map_err(|err| eyre!("{err:?}"))?;
                    ensure!(len == 32, "encryption key is not the correct size");

                    let mut key = Key::default();
                    for i in &mut key {
                        *i = rmp::decode::read_int(&mut bytes).map_err(|err| eyre!("{err:?}"))?;
                    }
                    Ok(key)
                }
                _ => bail!("could not decode encryption key"),
            }
        }
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
        .map_err(|_| eyre!("could not decrypt history"))?;
    let plaintext = encrypted_history.ciphertext;

    let history = decode(&plaintext)?;

    Ok(history)
}

fn format_rfc3339(ts: OffsetDateTime) -> Result<String> {
    // horrible hack. chrono AutoSI limits to 0, 3, 6, or 9 decimal places for nanoseconds.
    // time does not have this functionality.
    static PARTIAL_RFC3339_0: &[time::format_description::FormatItem<'static>] =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    static PARTIAL_RFC3339_3: &[time::format_description::FormatItem<'static>] =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");
    static PARTIAL_RFC3339_6: &[time::format_description::FormatItem<'static>] =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z");
    static PARTIAL_RFC3339_9: &[time::format_description::FormatItem<'static>] =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:9]Z");

    let fmt = match ts.nanosecond() {
        0 => PARTIAL_RFC3339_0,
        ns if ns % 1_000_000 == 0 => PARTIAL_RFC3339_3,
        ns if ns % 1_000 == 0 => PARTIAL_RFC3339_6,
        _ => PARTIAL_RFC3339_9,
    };

    Ok(ts.format(fmt)?)
}

fn encode(h: &History) -> Result<Vec<u8>> {
    use rmp::encode;

    let mut output = vec![];
    // INFO: ensure this is updated when adding new fields
    encode::write_array_len(&mut output, 9)?;

    encode::write_str(&mut output, &h.id.0)?;
    encode::write_str(&mut output, &(format_rfc3339(h.timestamp)?))?;
    encode::write_sint(&mut output, h.duration)?;
    encode::write_sint(&mut output, h.exit)?;
    encode::write_str(&mut output, &h.command)?;
    encode::write_str(&mut output, &h.cwd)?;
    encode::write_str(&mut output, &h.session)?;
    encode::write_str(&mut output, &h.hostname)?;
    match h.deleted_at {
        Some(d) => encode::write_str(&mut output, &format_rfc3339(d)?)?,
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
        id: id.to_owned().into(),
        timestamp: OffsetDateTime::parse(timestamp, &Rfc3339)?,
        duration,
        exit,
        command: command.to_owned(),
        cwd: cwd.to_owned(),
        session: session.to_owned(),
        hostname: hostname.to_owned(),
        deleted_at: deleted_at
            .map(|t| OffsetDateTime::parse(t, &Rfc3339))
            .transpose()?,
    })
}

fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
    eyre!("{err:?}")
}

#[cfg(test)]
mod test {
    use crypto_secretbox::{aead::OsRng, KeyInit, XSalsa20Poly1305};
    use pretty_assertions::assert_eq;
    use time::{macros::datetime, OffsetDateTime};

    use crate::history::History;

    use super::{decode, decrypt, encode, encrypt};

    #[test]
    fn test_encrypt_decrypt() {
        let key1 = XSalsa20Poly1305::generate_key(&mut OsRng);
        let key2 = XSalsa20Poly1305::generate_key(&mut OsRng);

        let history = History::from_db()
            .id("1".into())
            .timestamp(OffsetDateTime::now_utc())
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
            id: "66d16cbee7cd47538e5c5b8b44e9006e".to_owned().into(),
            timestamp: datetime!(2023-05-28 18:35:40.633872 +00:00),
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
            id: "66d16cbee7cd47538e5c5b8b44e9006e".to_owned().into(),
            timestamp: datetime!(2023-05-28 18:35:40.633872 +00:00),
            duration: 49206000,
            exit: 0,
            command: "git status".to_owned(),
            cwd: "/Users/conrad.ludgate/Documents/code/atuin".to_owned(),
            session: "b97d9a306f274473a203d2eba41f9457".to_owned(),
            hostname: "fvfg936c0kpf:conrad.ludgate".to_owned(),
            deleted_at: Some(datetime!(2023-05-28 18:35:40.633872 +00:00)),
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
            id: "66d16cbee7cd47538e5c5b8b44e9006e".to_owned().into(),
            timestamp: datetime!(2023-05-28 18:35:40.633872 +00:00),
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

    #[test]
    fn key_encodings() {
        use super::{decode_key, encode_key, Key};

        // a history of our key encodings.
        // v11.0.0 xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==
        // v12.0.0 xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==
        // v13.0.0 xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==
        // v13.0.1 xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==
        // v14.0.0 xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==
        // v14.0.1 xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==
        // c7d89c1 3AAgG1sqW8zSawnM2MyqzL7M8j4GVEXMlMyUNcz7dczizKfMrTRSIsyKbsypfFzM5Q== (https://github.com/ellie/atuin/pull/805)
        // b53ca35 3AAgG1sqW8zSawnM2MyqzL7M8j4GVEXMlMyUNcz7dczizKfMrTRSIsyKbsypfFzM5Q== (https://github.com/ellie/atuin/pull/974)
        // v15.0.0 3AAgG1sqW8zSawnM2MyqzL7M8j4GVEXMlMyUNcz7dczizKfMrTRSIsyKbsypfFzM5Q==
        // b8b57c8 xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==                     (https://github.com/ellie/atuin/pull/1057)
        // 8c94d79 3AAgG1sqW8zSawnM2MyqzL7M8j4GVEXMlMyUNcz7dczizKfMrTRSIsyKbsypfFzM5Q== (https://github.com/ellie/atuin/pull/1089)

        let key = Key::from([
            27, 91, 42, 91, 210, 107, 9, 216, 170, 190, 242, 62, 6, 84, 69, 148, 148, 53, 251, 117,
            226, 167, 173, 52, 82, 34, 138, 110, 169, 124, 92, 229,
        ]);

        assert_eq!(
            encode_key(&key).unwrap(),
            "3AAgG1sqW8zSawnM2MyqzL7M8j4GVEXMlMyUNcz7dczizKfMrTRSIsyKbsypfFzM5Q=="
        );

        // key encodings we have to support
        let valid_encodings = [
            "xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==",
            "3AAgG1sqW8zSawnM2MyqzL7M8j4GVEXMlMyUNcz7dczizKfMrTRSIsyKbsypfFzM5Q==",
        ];

        for k in valid_encodings {
            assert_eq!(decode_key(k.to_owned()).expect(k), key);
        }
    }
}

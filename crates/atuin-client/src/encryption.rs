// The general idea is that we NEVER send cleartext history to the server
// This way the odds of anything private ending up where it should not are
// very low
// The server authenticates via the usual username and password. This has
// nothing to do with the encryption, and is purely authentication! The client
// generates its own secret key, and encrypts all shell history with libsodium's
// secretbox. The data is then sent to the server, where it is stored. All
// clients must share the secret in order to be able to sync, as it is needed
// to decrypt

use std::io::prelude::*;

use base64::prelude::{BASE64_STANDARD, Engine};
pub use crypto_secretbox::Key;
use crypto_secretbox::{KeyInit, XSalsa20Poly1305, aead::OsRng};
use eyre::{Context, Result, bail, ensure};
use fs_err as fs;

use crate::settings::Settings;
use atuin_common::rmp as atu_rmp;
use atuin_common::rmp::decode::DecodeExt;

pub fn generate_encoded_key() -> Result<(Key, String)> {
    let key = XSalsa20Poly1305::generate_key(&mut OsRng);
    let encoded = encode_key(&key)?;

    Ok((key, encoded))
}

pub fn new_key(settings: &Settings) -> Result<Key> {
    let path = &settings.key_path;

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
    let path = &settings.key_path;

    let key = if path.exists() {
        let key = fs_err::read_to_string(path)?;
        decode_key(key)?
    } else {
        new_key(settings)?
    };

    Ok(key)
}

pub fn encode_key(key: &Key) -> Result<String> {
    let mut buf = vec![];
    atu_rmp::encode::write_array_len(&mut buf, key.len() as u32)
        .wrap_err("could not encode key to message pack")?;
    for b in key {
        atu_rmp::encode::write_uint(&mut buf, *b as u64)
            .wrap_err("could not encode key to message pack")?;
    }
    let buf = BASE64_STANDARD.encode(buf);

    Ok(buf)
}

pub fn decode_key(key: String) -> Result<Key> {
    let buf = BASE64_STANDARD
        .decode(key.trim_end())
        .wrap_err("encryption key is not a valid base64 encoding")?;

    // old code wrote the key as a fixed length array of 32 bytes
    // new code writes the key with a length prefix
    match <[u8; 32]>::try_from(&*buf) {
        Ok(key) => Ok(key.into()),
        Err(_) => {
            let mut bytes = atu_rmp::decode::Bytes::new(&buf);

            match atu_rmp::decode::Marker::from_u8(buf[0]) {
                atu_rmp::decode::Marker::Bin8 => {
                    let len = rmp::decode::read_bin_len(&mut bytes).decode()?;
                    ensure!(len == 32, "encryption key is not the correct size");
                    let key = <[u8; 32]>::try_from(bytes.remaining_slice())
                        .context("could not decode encryption key")?;
                    Ok(key.into())
                }
                atu_rmp::decode::Marker::Array16 => {
                    atu_rmp::decode::expect_array_len(&mut bytes, 32)?;

                    let mut key = Key::default();
                    for i in &mut key {
                        *i = rmp::decode::read_int::<u8, _>(&mut bytes).decode()?;
                    }
                    Ok(key)
                }
                _ => bail!("could not decode encryption key"),
            }
        }
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn key_encodings() {
        use super::{Key, decode_key, encode_key};

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

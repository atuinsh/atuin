//! An encryption key store
//!
//! * `tag` = "key;<KEY PURPOSE>"
//! * `version`s:
//!   - "v0"
//!
//! ## Encryption schemes
//!
//! ### v0
//!
//! [`UnsafeNoEncryption`]
//!
//! ## Encoding schemes
//!
//! ### v0
//!
//! JSON encoding of the KeyRecord.
//!
//! KeyRecord {
//!     id: k4.pid.<public key id>,
//!     public_key: k4.public.<public key>,
//!     wrapped_secret_key: k4.secret-wrap.<encrypted secret key>,
//! }

use std::io::Write;
use std::path::PathBuf;

use atuin_common::record::{DecryptedData, HostId};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use eyre::{bail, ensure, eyre, Context, Result};
use rand::rngs::OsRng;
use rand::RngCore;
use rusty_paserk::{
    Key, KeyId, Local, PieWrappedKey, PlaintextKey, Public, SafeForFooter, Secret, V4,
};

use crate::record::store::Store;
use crate::settings::Settings;

use super::unsafe_encryption::UnsafeNoEncryption;

const KEY_VERSION: &str = "v0";
const KEY_TAG_PREFIX: &str = "key;";

#[derive(serde::Deserialize, serde::Serialize)]
struct KeyRecord {
    /// Key ID used to encrypt messages
    id: KeyId<V4, Public>,
    /// Key used to encrypt messages
    public_key: PlaintextKey<V4, Public>,
    /// Wrapped decryption key
    wrapped_secret_key: PieWrappedKey<V4, Secret>,
}

/// Verify that the fields in the KeyRecord are safe to be unencrypted
const _SAFE_UNENCRYPTED: () = {
    const fn safe_for_footer<T: SafeForFooter>() {}

    safe_for_footer::<PieWrappedKey<V4, Secret>>();
    safe_for_footer::<KeyId<V4, Public>>();

    // Public keys are always safe to share - but they should not be in footers of a PASETO.
    // This is because for a PASETO they might be the verification key. Including the verification key
    // with a token is an attack vector and thus you should only include the identifier of the verification key.
    // This is not a problem for us. We don't these public keys for verification, only encryption.
    // safe_for_footer::<PlaintextKey<V4, Public>>();
};

impl KeyRecord {
    pub fn serialize(&self) -> Result<DecryptedData> {
        Ok(DecryptedData(self.id.to_string().into_bytes()))
    }

    pub fn deserialize(data: &DecryptedData, version: &str) -> Result<Self> {
        match version {
            KEY_VERSION => serde_json::from_slice(&data.0).context("not a valid key record"),
            _ => {
                bail!("unknown version {version:?}")
            }
        }
    }
}

pub struct KeyStore {
    purpose: String,
}

impl KeyStore {
    /// Create a new key store for your application.
    ///
    /// Purpose should be unique for your application.
    /// Eg for atuin history this might be "atuin-history".
    /// For atuin kv this might be "atuin-kv".
    /// For mcfly history this might be "mcfly".
    /// etc...
    pub fn new(purpose: &str) -> KeyStore {
        KeyStore {
            purpose: format!("{KEY_TAG_PREFIX}{purpose}"),
        }
    }

    pub async fn get_encryption_key(
        &self,
        store: &mut impl Store,
    ) -> Result<Option<Key<V4, Public>>> {
        // iterate records to find the value we want
        // start at the end, so we get the most recent version
        let tails = store.tag_tails(&self.purpose).await?;

        if tails.is_empty() {
            return Ok(None);
        }

        // first, decide on a record. see kv store for details
        let record = tails.iter().max_by_key(|r| r.timestamp).unwrap().clone();

        let decrypted = match record.version.as_str() {
            KEY_VERSION => record.decrypt::<UnsafeNoEncryption>(&())?,
            version => bail!("unknown version {version:?}"),
        };

        let kv = KeyRecord::deserialize(&decrypted.data, &decrypted.version)?;
        Ok(Some(kv.public_key.0))
    }

    pub async fn get_wrapped_decryption_key(
        &self,
        store: &mut impl Store,
        id: KeyId<V4, Public>,
    ) -> Result<Option<PieWrappedKey<V4, Secret>>> {
        // iterate records to find the value we want
        // start at the end, so we get the most recent version
        let tails = store.tag_tails(&self.purpose).await?;

        if tails.is_empty() {
            return Ok(None);
        }

        // first, decide on a record. see kv store for details
        let mut record = tails.iter().max_by_key(|r| r.timestamp).unwrap().clone();

        loop {
            let decrypted = match record.version.as_str() {
                KEY_VERSION => record.decrypt::<UnsafeNoEncryption>(&())?,
                version => bail!("unknown version {version:?}"),
            };

            let kv = KeyRecord::deserialize(&decrypted.data, &decrypted.version)?;
            if kv.id == id {
                return Ok(Some(kv.wrapped_secret_key));
            }

            if let Some(parent) = decrypted.parent {
                record = store.get(parent).await?;
            } else {
                break;
            }
        }

        // if we get here, then... we didn't find the record with that key id :(
        Ok(None)
    }

    pub async fn set(
        &self,
        store: &mut impl Store,
        host_id: HostId,
        public_key: Key<V4, Public>,
        wrapped_secret_key: PieWrappedKey<V4, Secret>,
    ) -> Result<()> {
        let id = public_key.to_id();
        let record = KeyRecord {
            id,
            public_key: PlaintextKey(public_key),
            wrapped_secret_key,
        };

        let bytes = record.serialize()?;

        let parent = store
            .tail(host_id, &self.purpose)
            .await?
            .map(|entry| entry.id);

        let record = atuin_common::record::Record::builder()
            .host(host_id)
            .version(KEY_VERSION.to_string())
            .tag(self.purpose.to_string())
            .parent(parent)
            .data(bytes)
            .build();

        store
            .push(&record.encrypt::<UnsafeNoEncryption>(&()))
            .await?;

        Ok(())
    }
}

pub enum EncryptionKey {
    /// The current key is invalid
    Invalid {
        /// the id of the key
        kid: KeyId<V4, Local>,
        /// the id of the host that registered the key
        host_id: String,
    },
    Valid {
        encryption_key: AtuinKey,
    },
}
pub type AtuinKey = [u8; 32];

pub fn new_key(settings: &Settings) -> Result<AtuinKey> {
    let path = settings.key_path.as_str();

    let mut key = [0; 32];
    OsRng.fill_bytes(&mut key);
    let encoded = encode_key(&key)?;

    let mut file = fs_err::File::create(path)?;
    file.write_all(encoded.as_bytes())?;

    Ok(key)
}

// Loads the secret key, will create + save if it doesn't exist
pub fn load_key(settings: &Settings) -> Result<AtuinKey> {
    let path = settings.key_path.as_str();

    let key = if PathBuf::from(path).exists() {
        let key = fs_err::read_to_string(path)?;
        decode_key(key)?
    } else {
        new_key(settings)?
    };

    Ok(key)
}

pub fn encode_key(key: &AtuinKey) -> Result<String> {
    let mut buf = vec![];
    rmp::encode::write_bin(&mut buf, key.as_slice())
        .wrap_err("could not encode key to message pack")?;
    let buf = BASE64_STANDARD.encode(buf);

    Ok(buf)
}

pub fn decode_key(key: String) -> Result<AtuinKey> {
    let buf = BASE64_STANDARD
        .decode(key.trim_end())
        .wrap_err("encryption key is not a valid base64 encoding")?;

    // old code wrote the key as a fixed length array of 32 bytes
    // new code writes the key with a length prefix
    match <[u8; 32]>::try_from(&*buf) {
        Ok(key) => Ok(key),
        Err(_) => {
            let mut bytes = rmp::decode::Bytes::new(&buf);
            let key_len = rmp::decode::read_bin_len(&mut bytes).map_err(|err| eyre!("{err:?}"))?;
            ensure!(key_len == 32, "encryption key is not the correct size");
            <[u8; 32]>::try_from(bytes.remaining_slice())
                .context("encryption key is not the correct size")
        }
    }
}

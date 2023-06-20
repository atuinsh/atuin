//! An encryption key (id) store
//!
//! * `tag` = "key"
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
//! UTF8 encoding of the key ID, using the PASERK V4 `lid` (local-id) format.

use std::io::Write;
use std::path::PathBuf;

use atuin_common::record::DecryptedData;
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use eyre::{bail, ensure, eyre, Context, Result};
use rand::rngs::OsRng;
use rand::RngCore;
use rusty_paserk::{Key, KeyId, Local, V4};

use crate::record::encryption::none::UnsafeNoEncryption;
use crate::record::store::Store;
use crate::settings::Settings;

const KEY_VERSION: &str = "v0";
const KEY_TAG: &str = "key";

struct KeyRecord {
    id: KeyId<V4, Local>,
}

impl KeyRecord {
    pub fn serialize(&self) -> Result<DecryptedData> {
        Ok(DecryptedData(self.id.to_string().into_bytes()))
    }

    pub fn deserialize(data: &DecryptedData, version: &str) -> Result<Self> {
        match version {
            KEY_VERSION => {
                let lid = std::str::from_utf8(&data.0).context("key id was not utf8 encoded")?;
                Ok(Self {
                    id: lid.parse().context("invalid key id")?,
                })
            }
            _ => {
                bail!("unknown version {version:?}")
            }
        }
    }
}

pub struct KeyStore;

impl Default for KeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyStore {
    // will want to init the actual kv store when that is done
    pub fn new() -> KeyStore {
        KeyStore {}
    }

    pub async fn record_new_encryption_key(
        &self,
        store: &mut (impl Store + Send + Sync),
        encryption_key: &[u8; 32],
    ) -> Result<()> {
        let host_id = Settings::host_id().expect("failed to get host_id");

        // the local_id is a hashed version of the encryption key. safe to store unencrypted
        let key_id = Key::<V4, Local>::from_bytes(*encryption_key).to_id();

        let record = KeyRecord { id: key_id };
        let bytes = record.serialize()?;

        let parent = store
            .last(host_id.as_str(), KEY_TAG)
            .await?
            .map(|entry| entry.id);

        let record = atuin_common::record::Record::builder()
            .host(host_id)
            .version(KEY_VERSION.to_string())
            .tag(KEY_TAG.to_string())
            .parent(parent)
            .data(bytes)
            .build();

        // the local_id is a hashed version of the encryption key. safe to store unencrypted
        store
            .push(&record.encrypt::<UnsafeNoEncryption>(encryption_key))
            .await?;

        Ok(())
    }

    /// Validates that our current encryption key is the one that was last seen.
    /// If it isn't, the client should import their new key from the host.
    ///
    /// If there is no recorded key, it is inserted automatically.
    pub async fn validate_encryption_key(
        &self,
        store: &mut (impl Store + Send + Sync),
        settings: &Settings,
    ) -> Result<EncryptionKey> {
        let encryption_key: [u8; 32] =
            load_key(settings).context("could not load encryption key")?;

        // TODO: don't load this from disk so much
        let host_id = Settings::host_id().expect("failed to get host_id");

        // get the last recorded key id.
        // TODO: get the last from any host
        let Some(record) = store.last(host_id.as_str(), KEY_TAG).await? else {
            self.record_new_encryption_key(store, &encryption_key).await?;
            return Ok(EncryptionKey::Valid { encryption_key });
        };

        let decrypted = match record.version.as_str() {
            KEY_VERSION => record.decrypt::<UnsafeNoEncryption>(&encryption_key)?,
            version => bail!("unknown version {version:?}"),
        };

        // encode the current key to match the registered version
        let current_key_id = match decrypted.version.as_str() {
            KEY_VERSION => Key::<V4, Local>::from_bytes(encryption_key).to_id(),
            version => bail!("unknown version {version:?}"),
        };

        let key = KeyRecord::deserialize(&decrypted.data, &decrypted.version)?;
        if key.id != current_key_id {
            Ok(EncryptionKey::Invalid {
                kid: key.id,
                host_id: decrypted.host,
            })
        } else {
            Ok(EncryptionKey::Valid { encryption_key })
        }
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

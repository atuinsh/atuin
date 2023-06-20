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

use atuin_common::record::{DecryptedData, HostId};
use eyre::{bail, Context, Result};
use rusty_paserk::{Key, KeyId, Local, V4};

use crate::encryption::{load_key, AtuinKey};
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

        let parent = store.tail(host_id, KEY_TAG).await?.map(|entry| entry.id);

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
        let Some(record) = store.tail(host_id, KEY_TAG).await? else {
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
                kid: key.id.to_string(),
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
        kid: String,
        /// the id of the host that registered the key
        host_id: HostId,
    },
    Valid {
        encryption_key: AtuinKey,
    },
}

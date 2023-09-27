use crate::record::encryption::{KeyEncapsulation, PASETO_V4_ENVELOPE};
use eyre::{ensure, Context, Result};
use rusty_paserk::{Key, KeyId, Local, Public, SealedKey, Secret};
use rusty_paseto::core::V4;
use serde::{Deserialize, Serialize};

/// Use PASETO V4 Local encryption with PASERK key sealing using the additional data as an implicit assertion.
#[allow(non_camel_case_types)]
pub type PASETO_V4_SEAL = PASETO_V4_ENVELOPE<Seal>;

/// Key sealing
pub struct Seal;

impl KeyEncapsulation for Seal {
    type DecryptionKey = rusty_paserk::Key<V4, Secret>;
    type EncryptionKey = rusty_paserk::Key<V4, Public>;

    fn decrypt_cek(
        wrapped_cek: String,
        key: &rusty_paserk::Key<V4, Secret>,
    ) -> Result<Key<V4, Local>> {
        // let wrapping_key = PasetoSymmetricKey::from(Key::from(key));

        let AtuinFooter { kid, wpk } = serde_json::from_str(&wrapped_cek)
            .context("wrapped cek did not contain the correct contents")?;

        // check that the wrapping key matches the required key to decrypt.
        // In future, we could support multiple keys and use this key to
        // look up the key rather than only allow one key.
        // For now though we will only support the one key and key rotation will
        // have to be a hard reset
        let current_kid = key.public_key().to_id();
        ensure!(
            current_kid == kid,
            "attempting to decrypt with incorrect key. currently using {current_kid}, expecting {kid}"
        );

        // decrypt the random key
        Ok(wpk.unseal(key)?)
    }

    fn encrypt_cek(cek: Key<V4, Local>, key: &rusty_paserk::Key<V4, Public>) -> String {
        // wrap the random key so we can decrypt it later
        let wrapped_cek = AtuinFooter {
            wpk: cek.seal(key),
            kid: key.to_id(),
        };
        serde_json::to_string(&wrapped_cek).expect("could not serialize wrapped cek")
    }
}

#[derive(Serialize, Deserialize)]
struct AtuinPayload {
    data: String,
}

#[derive(Serialize, Deserialize)]
/// Well-known footer claims for decrypting. This is not encrypted but is stored in the record.
/// <https://github.com/paseto-standard/paseto-spec/blob/master/docs/02-Implementation-Guide/04-Claims.md#optional-footer-claims>
pub struct AtuinFooter {
    /// Wrapped key
    wpk: SealedKey<V4>,
    /// ID of the key which was used to wrap
    pub kid: KeyId<V4, Public>,
}

#[cfg(test)]
mod tests {
    use atuin_common::{
        record::{AdditionalData, DecryptedData, Encryption, HostId, Record, RecordId},
        utils::uuid_v7,
    };

    use super::*;

    #[test]
    fn round_trip() {
        let key = Key::<V4, Secret>::new_os_random();

        let ad = AdditionalData {
            id: &RecordId(uuid_v7()),
            version: "v0",
            tag: "kv",
            host: &HostId(uuid_v7()),
            parent: None,
        };

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted = PASETO_V4_SEAL::encrypt(data.clone(), ad, &key.public_key());
        let decrypted = PASETO_V4_SEAL::decrypt(encrypted, ad, &key).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn same_entry_different_output() {
        let key = Key::<V4, Secret>::new_os_random();

        let ad = AdditionalData {
            id: &RecordId(uuid_v7()),
            version: "v0",
            tag: "kv",
            host: &HostId(uuid_v7()),
            parent: None,
        };

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted = PASETO_V4_SEAL::encrypt(data.clone(), ad, &key.public_key());
        let encrypted2 = PASETO_V4_SEAL::encrypt(data, ad, &key.public_key());

        assert_ne!(
            encrypted.data, encrypted2.data,
            "re-encrypting the same contents should have different output due to key randomization"
        );
    }

    #[test]
    fn cannot_decrypt_different_key() {
        let key = Key::<V4, Secret>::new_os_random();
        let fake_key = Key::<V4, Secret>::new_os_random();

        let ad = AdditionalData {
            id: &RecordId(uuid_v7()),
            version: "v0",
            tag: "kv",
            host: &HostId(uuid_v7()),
            parent: None,
        };

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted = PASETO_V4_SEAL::encrypt(data, ad, &key.public_key());
        let _ = PASETO_V4_SEAL::decrypt(encrypted, ad, &fake_key).unwrap_err();
    }

    #[test]
    fn cannot_decrypt_different_id() {
        let key = Key::<V4, Secret>::new_os_random();

        let ad = AdditionalData {
            id: &RecordId(uuid_v7()),
            version: "v0",
            tag: "kv",
            host: &HostId(uuid_v7()),
            parent: None,
        };

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted = PASETO_V4_SEAL::encrypt(data, ad, &key.public_key());

        let ad = AdditionalData {
            id: &RecordId(uuid_v7()),
            ..ad
        };
        let _ = PASETO_V4_SEAL::decrypt(encrypted, ad, &key).unwrap_err();
    }

    #[test]
    fn re_encrypt_round_trip() {
        let key1 = Key::<V4, Secret>::new_os_random();
        let key2 = Key::<V4, Secret>::new_os_random();

        let ad = AdditionalData {
            id: &RecordId(uuid_v7()),
            version: "v0",
            tag: "kv",
            host: &HostId(uuid_v7()),
            parent: None,
        };

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted1 = PASETO_V4_SEAL::encrypt(data.clone(), ad, &key1.public_key());
        let encrypted2 =
            PASETO_V4_SEAL::re_encrypt(encrypted1.clone(), ad, &key1, &key2.public_key()).unwrap();

        // we only re-encrypt the content keys
        assert_eq!(encrypted1.data, encrypted2.data);
        assert_ne!(
            encrypted1.content_encryption_key,
            encrypted2.content_encryption_key
        );

        let decrypted = PASETO_V4_SEAL::decrypt(encrypted2, ad, &key2).unwrap();

        assert_eq!(decrypted, data);
    }

    #[test]
    fn full_record_round_trip() {
        let key = Key::from_secret_key([0x55; 32]);
        let record = Record::builder()
            .id(RecordId(uuid_v7()))
            .version("v0".to_owned())
            .tag("kv".to_owned())
            .host(HostId(uuid_v7()))
            .timestamp(1687244806000000)
            .data(DecryptedData(vec![1, 2, 3, 4]))
            .build();

        let encrypted = record.encrypt::<PASETO_V4_SEAL>(&key.public_key());

        assert!(!encrypted.data.data.is_empty());
        assert!(!encrypted.data.content_encryption_key.is_empty());

        let decrypted = encrypted.decrypt::<PASETO_V4_SEAL>(&key).unwrap();

        assert_eq!(decrypted.data.0, [1, 2, 3, 4]);
    }

    #[test]
    fn full_record_round_trip_fail() {
        let key = Key::from_secret_key([0x55; 32]);
        let record = Record::builder()
            .id(RecordId(uuid_v7()))
            .version("v0".to_owned())
            .tag("kv".to_owned())
            .host(HostId(uuid_v7()))
            .timestamp(1687244806000000)
            .data(DecryptedData(vec![1, 2, 3, 4]))
            .build();

        let encrypted = record.encrypt::<PASETO_V4_SEAL>(&key.public_key());

        let mut enc1 = encrypted.clone();
        enc1.host = HostId(uuid_v7());
        let _ = enc1
            .decrypt::<PASETO_V4_SEAL>(&key)
            .expect_err("tampering with the host should result in auth failure");

        let mut enc2 = encrypted;
        enc2.id = RecordId(uuid_v7());
        let _ = enc2
            .decrypt::<PASETO_V4_SEAL>(&key)
            .expect_err("tampering with the id should result in auth failure");
    }
}

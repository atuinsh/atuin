use atuin_common::record::{
    AdditionalData, DecryptedData, EncryptedData, Encryption, HostId, RecordId, RecordIdx,
};
use base64::{Engine, engine::general_purpose};
use eyre::{Context, Result, ensure};
use paseto_core::{validation::NoValidation, version::Local};
use paseto_v4::{EncryptedToken, KeyId, KeyText, LocalKey, PieWrappedLocalKey, UnencryptedToken};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Use PASETO V4 Local encryption using the additional data as an implicit assertion.
#[allow(non_camel_case_types)]
pub struct PASETO_V4;

/*
Why do we use a random content-encryption key?
Originally I was planning on using a derived key for encryption based on additional data.
This would be a lot more secure than using the master key directly.

However, there's an established norm of using a random key. This scheme might be otherwise known as
- client-side encryption
- envelope encryption
- key wrapping

A HSM (Hardware Security Module) provider, eg: AWS, Azure, GCP, or even a physical device like a YubiKey
will have some keys that they keep to themselves. These keys never leave their physical hardware.
If they never leave the hardware, then encrypting large amounts of data means giving them the data and waiting.
This is not a practical solution. Instead, generate a unique key for your data, encrypt that using your HSM
and then store that with your data.

See
 - <https://docs.aws.amazon.com/wellarchitected/latest/financial-services-industry-lens/use-envelope-encryption-with-customer-master-keys.html>
 - <https://cloud.google.com/kms/docs/envelope-encryption>
 - <https://learn.microsoft.com/en-us/azure/storage/blobs/client-side-encryption?tabs=dotnet#encryption-and-decryption-via-the-envelope-technique>
 - <https://www.yubico.com/gb/product/yubihsm-2-fips/>
 - <https://cheatsheetseries.owasp.org/cheatsheets/Cryptographic_Storage_Cheat_Sheet.html#encrypting-stored-keys>

Why would we care? In the past we have received some requests for company solutions. If in future we can configure a
KMS service with little effort, then that would solve a lot of issues for their security team.

Even for personal use, if a user is not comfortable with sharing keys between hosts,
GCP HSM costs $1/month and $0.03 per 10,000 key operations. Assuming an active user runs
1000 atuin records a day, that would only cost them $1 and 10 cent a month.

Additionally, key rotations are much simpler using this scheme. Rotating a key is as simple as re-encrypting the CEK, and not the message contents.
This makes it very fast to rotate a key in bulk.

For future reference, with asymmetric encryption, you can encrypt the CEK without the HSM's involvement, but decrypting
will need the HSM. This allows the encryption path to still be extremely fast (no network calls) but downloads/decryption
that happens in the background can make the network calls to the HSM
*/

impl Encryption for PASETO_V4 {
    fn re_encrypt(
        mut data: EncryptedData,
        _ad: AdditionalData,
        old_key: &[u8; 32],
        new_key: &[u8; 32],
    ) -> Result<EncryptedData> {
        let cek = Self::decrypt_cek(data.content_encryption_key, old_key)?;
        data.content_encryption_key = Self::encrypt_cek(cek, new_key);
        Ok(data)
    }

    fn encrypt(data: DecryptedData, ad: AdditionalData, key: &[u8; 32]) -> EncryptedData {
        // generate a random key for this entry
        // aka content-encryption-key (CEK)
        let random_key = LocalKey::random().expect("could not source from random");

        // encode the implicit assertions
        let assertions = Assertions::from(ad).encode();

        // build the payload and encrypt the token
        let payload = AtuinPayload {
            data: general_purpose::URL_SAFE_NO_PAD.encode(data.0),
        };

        let token = UnencryptedToken::new(Json(payload))
            .encrypt_with_aad(&random_key, assertions.as_bytes())
            .expect("error encrypting atuin data")
            .to_string();

        EncryptedData {
            data: token,
            content_encryption_key: Self::encrypt_cek(random_key, key),
        }
    }

    fn decrypt(data: EncryptedData, ad: AdditionalData, key: &[u8; 32]) -> Result<DecryptedData> {
        let token = data.data;
        let cek = Self::decrypt_cek(data.content_encryption_key, key)?;

        // decode the payload
        let token: EncryptedToken<Json<AtuinPayload>> =
            token.parse().context("could not parse encrypted record")?;

        // encode the implicit assertions
        let assertions = Assertions::from(ad).encode();

        // decrypt the payload with the footer and implicit assertions.
        // we don't need any extra validation of the payload.
        let validation = NoValidation::dangerous_no_validation();
        let token = token
            .decrypt_with_aad(&cek, assertions.as_bytes(), &validation)
            .context("could not decrypt entry")?;

        let data = general_purpose::URL_SAFE_NO_PAD.decode(token.claims.0.data)?;
        Ok(DecryptedData(data))
    }
}

impl PASETO_V4 {
    fn decrypt_cek(wrapped_cek: String, key: &[u8; 32]) -> Result<LocalKey> {
        let wrapping_key: LocalKey = KeyText::<Local>::from_raw_bytes(key)
            .try_into()
            .expect("all 32 byte keys are valid");

        let AtuinFooter { kid, wpk } = serde_json::from_str(&wrapped_cek)
            .context("wrapped cek did not contain the correct contents")?;

        // check that the wrapping key matches the required key to decrypt.
        // In future, we could support multiple keys and use this key to
        // look up the key rather than only allow one key.
        // For now though we will only support the one key and key rotation will
        // have to be a hard reset
        let current_kid = wrapping_key.id();

        ensure!(
            current_kid == kid,
            "attempting to decrypt with incorrect key. currently using {current_kid}, expecting {kid}"
        );

        // decrypt the random key
        Ok(wpk.unwrap(&wrapping_key)?)
    }

    fn encrypt_cek(cek: LocalKey, key: &[u8; 32]) -> String {
        // aka key-encryption-key (KEK)
        let wrapping_key: LocalKey = KeyText::<Local>::from_raw_bytes(key)
            .try_into()
            .expect("all 32 byte keys are valid");

        // wrap the random key so we can decrypt it later
        let wrapped_cek = AtuinFooter {
            wpk: cek.wrap_pie(&wrapping_key).expect("could not encrypt key"),
            kid: wrapping_key.id(),
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
struct AtuinFooter {
    /// Wrapped key
    wpk: PieWrappedLocalKey,
    /// ID of the key which was used to wrap
    kid: KeyId<Local>,
}

/// Used in the implicit assertions. This is not encrypted and not stored in the data blob.
// This cannot be changed, otherwise it breaks the authenticated encryption.
#[derive(Debug, Copy, Clone, Serialize)]
struct Assertions<'a> {
    id: &'a RecordId,
    idx: &'a RecordIdx,
    version: &'a str,
    tag: &'a str,
    host: &'a HostId,
}

impl<'a> From<AdditionalData<'a>> for Assertions<'a> {
    fn from(ad: AdditionalData<'a>) -> Self {
        Self {
            id: ad.id,
            version: ad.version,
            tag: ad.tag,
            host: ad.host,
            idx: ad.idx,
        }
    }
}

impl Assertions<'_> {
    fn encode(&self) -> String {
        serde_json::to_string(self).expect("could not serialize implicit assertions")
    }
}

#[derive(Default)]
struct Json<T>(T);

struct Writer<W: paseto_core::encodings::WriteBytes>(W);
impl<W: paseto_core::encodings::WriteBytes> std::io::Write for Writer<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<M: Serialize + DeserializeOwned> paseto_core::encodings::Payload for Json<M> {
    /// JSON is the standard payload and requires no version suffix
    const SUFFIX: &'static str = "";

    fn encode(
        self,
        writer: impl paseto_core::encodings::WriteBytes,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        serde_json::to_writer(Writer(writer), &self.0).map_err(|err| Box::new(err) as _)
    }

    fn decode(payload: &[u8]) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        serde_json::from_slice(payload)
            .map_err(From::from)
            .map(Self)
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::{
        record::{Host, Record},
        utils::uuid_v7,
    };

    use super::*;

    #[test]
    fn round_trip() {
        let key = LocalKey::random().unwrap();
        let key_bytes: [u8; 32] = key.expose_key().as_raw_bytes().try_into().unwrap();

        let ad = AdditionalData {
            id: &RecordId(uuid_v7()),
            version: "v0",
            tag: "kv",
            host: &HostId(uuid_v7()),
            idx: &0,
        };

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted = PASETO_V4::encrypt(data.clone(), ad, &key_bytes);
        let decrypted = PASETO_V4::decrypt(encrypted, ad, &key_bytes).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn same_entry_different_output() {
        let key = LocalKey::random().unwrap();
        let key_bytes: [u8; 32] = key.expose_key().as_raw_bytes().try_into().unwrap();

        let ad = AdditionalData {
            id: &RecordId(uuid_v7()),
            version: "v0",
            tag: "kv",
            host: &HostId(uuid_v7()),
            idx: &0,
        };

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted = PASETO_V4::encrypt(data.clone(), ad, &key_bytes);
        let encrypted2 = PASETO_V4::encrypt(data, ad, &key_bytes);

        assert_ne!(
            encrypted.data, encrypted2.data,
            "re-encrypting the same contents should have different output due to key randomization"
        );
    }

    #[test]
    fn cannot_decrypt_different_key() {
        let key = LocalKey::random().unwrap();
        let key_bytes: [u8; 32] = key.expose_key().as_raw_bytes().try_into().unwrap();

        let fake_key = LocalKey::random().unwrap();
        let fake_key_bytes: [u8; 32] = fake_key.expose_key().as_raw_bytes().try_into().unwrap();

        let ad = AdditionalData {
            id: &RecordId(uuid_v7()),
            version: "v0",
            tag: "kv",
            host: &HostId(uuid_v7()),
            idx: &0,
        };

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted = PASETO_V4::encrypt(data, ad, &key_bytes);
        let _ = PASETO_V4::decrypt(encrypted, ad, &fake_key_bytes).unwrap_err();
    }

    #[test]
    fn cannot_decrypt_different_id() {
        let key = LocalKey::random().unwrap();
        let key_bytes: [u8; 32] = key.expose_key().as_raw_bytes().try_into().unwrap();

        let ad = AdditionalData {
            id: &RecordId(uuid_v7()),
            version: "v0",
            tag: "kv",
            host: &HostId(uuid_v7()),
            idx: &0,
        };

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted = PASETO_V4::encrypt(data, ad, &key_bytes);

        let ad = AdditionalData {
            id: &RecordId(uuid_v7()),
            ..ad
        };
        let _ = PASETO_V4::decrypt(encrypted, ad, &key_bytes).unwrap_err();
    }

    #[test]
    fn re_encrypt_round_trip() {
        let key1 = LocalKey::random().unwrap();
        let key2 = LocalKey::random().unwrap();
        let key1_bytes: [u8; 32] = key1.expose_key().as_raw_bytes().try_into().unwrap();
        let key2_bytes: [u8; 32] = key2.expose_key().as_raw_bytes().try_into().unwrap();

        let ad = AdditionalData {
            id: &RecordId(uuid_v7()),
            version: "v0",
            tag: "kv",
            host: &HostId(uuid_v7()),
            idx: &0,
        };

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted1 = PASETO_V4::encrypt(data.clone(), ad, &key1_bytes);
        let encrypted2 =
            PASETO_V4::re_encrypt(encrypted1.clone(), ad, &key1_bytes, &key2_bytes).unwrap();

        // we only re-encrypt the content keys
        assert_eq!(encrypted1.data, encrypted2.data);
        assert_ne!(
            encrypted1.content_encryption_key,
            encrypted2.content_encryption_key
        );

        let decrypted = PASETO_V4::decrypt(encrypted2, ad, &key2_bytes).unwrap();

        assert_eq!(decrypted, data);
    }

    #[test]
    fn full_record_round_trip() {
        let key = [0x55; 32];
        let record = Record::builder()
            .id(RecordId(uuid_v7()))
            .version("v0".to_owned())
            .tag("kv".to_owned())
            .host(Host::new(HostId(uuid_v7())))
            .timestamp(1687244806000000)
            .data(DecryptedData(vec![1, 2, 3, 4]))
            .idx(0)
            .build();

        let encrypted = record.encrypt::<PASETO_V4>(&key);

        assert!(!encrypted.data.data.is_empty());
        assert!(!encrypted.data.content_encryption_key.is_empty());

        let decrypted = encrypted.decrypt::<PASETO_V4>(&key).unwrap();

        assert_eq!(decrypted.data.0, [1, 2, 3, 4]);
    }

    #[test]
    fn full_record_round_trip_fail() {
        let key = [0x55; 32];
        let record = Record::builder()
            .id(RecordId(uuid_v7()))
            .version("v0".to_owned())
            .tag("kv".to_owned())
            .host(Host::new(HostId(uuid_v7())))
            .timestamp(1687244806000000)
            .data(DecryptedData(vec![1, 2, 3, 4]))
            .idx(0)
            .build();

        let encrypted = record.encrypt::<PASETO_V4>(&key);

        let mut enc1 = encrypted.clone();
        enc1.host = Host::new(HostId(uuid_v7()));
        let _ = enc1
            .decrypt::<PASETO_V4>(&key)
            .expect_err("tampering with the host should result in auth failure");

        let mut enc2 = encrypted;
        enc2.id = RecordId(uuid_v7());
        let _ = enc2
            .decrypt::<PASETO_V4>(&key)
            .expect_err("tampering with the id should result in auth failure");
    }
}

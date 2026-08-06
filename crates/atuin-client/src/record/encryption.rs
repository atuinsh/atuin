use std::array::TryFromSliceError;

use atuin_domain::record::{
    AdditionalData, DecryptedData, EncryptedData, Encryption, HostId, RecordId, RecordIdx,
};
use base64::{Engine, engine::general_purpose};
use eyre::{Context, Result, ensure};
use rusty_paserk;
use rusty_paseto::Paseto;
use rusty_paseto::core as rusty_paseto;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

type PaserkV4KeyId = rusty_paserk::KeyId<rusty_paserk::V4, rusty_paserk::Local>;
type PaserkV4PieWrappedKey = rusty_paserk::PieWrappedKey<rusty_paserk::V4, rusty_paserk::Local>;
type PasetoV4Nonce<'a> = rusty_paseto::PasetoNonce<'a, rusty_paseto::V4, rusty_paseto::Local>;

/// Use PASETO V4 Local encryption using the additional data as an implicit assertion.
pub struct PasetoV4;

/// Key used for [`PasetoV4`] encryption.
///
/// Intentionally **not** Copy to support zeroing out on Drop.
#[derive(Clone, PartialEq, Eq, derive_more::From, derive_more::Debug)]
#[debug("PasetoV4Key(*******)")]
pub struct PasetoV4Key([u8; 32]);

impl PasetoV4Key {
    /// A key with every byte set to zero.
    #[must_use]
    pub const fn zero() -> Self {
        Self([0u8; 32])
    }

    /// Borrow the raw key bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Equivalent to [`rusty_paserk::Key::new_os_random()`].
    pub fn new_os_random() -> Self {
        rusty_paserk::Key::<rusty_paserk::V4, rusty_paserk::Local>::new_os_random().into()
    }

    pub fn try_new_random() -> Result<Self, rusty_paseto::PasetoError> {
        let paseto: rusty_paseto::Key<32> = rusty_paseto::Key::<32>::try_new_random()?;
        Ok(paseto.into())
    }

    pub fn key_id(&self) -> PaserkV4KeyId {
        let paserk: rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local> = self.into();
        paserk.to_id()
    }

    pub fn wrap_pie(&self, wrapping: &PasetoV4Key) -> PaserkV4PieWrappedKey {
        let p_self: rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local> = self.into();
        let p_wrapping: rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local> = wrapping.into();

        p_self.wrap_pie(&p_wrapping)
    }
}

impl Drop for PasetoV4Key {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl TryFrom<&[u8]> for PasetoV4Key {
    type Error = TryFromSliceError;

    fn try_from(bytes: &[u8]) -> std::result::Result<Self, Self::Error> {
        <[u8; 32]>::try_from(bytes).map(Self)
    }
}

impl From<&PasetoV4Key> for rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local> {
    fn from(value: &PasetoV4Key) -> Self {
        Self::from_bytes(*value.as_bytes())
    }
}

impl From<rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local>> for PasetoV4Key {
    fn from(value: rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local>) -> Self {
        Self(value.to_bytes())
    }
}

impl From<&PasetoV4Key>
    for rusty_paseto::PasetoSymmetricKey<rusty_paseto::V4, rusty_paseto::Local>
{
    fn from(value: &PasetoV4Key) -> Self {
        rusty_paserk::Key::<rusty_paserk::V4, rusty_paserk::Local>::from(value).into()
    }
}

impl From<&PasetoV4Key> for rusty_paseto::Key<32> {
    fn from(value: &PasetoV4Key) -> Self {
        value.0.into()
    }
}

impl From<rusty_paseto::Key<32>> for PasetoV4Key {
    fn from(value: rusty_paseto::Key<32>) -> Self {
        Self(*value)
    }
}

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
 - <https://www.yubico.com/products/hardware-security-module/>
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

impl Encryption for PasetoV4 {
    type Key = PasetoV4Key;

    fn re_encrypt(
        mut data: EncryptedData,
        _ad: AdditionalData,
        old_key: &PasetoV4Key,
        new_key: &PasetoV4Key,
    ) -> Result<EncryptedData> {
        let cek = Self::decrypt_cek(data.content_encryption_key, old_key)?;
        data.content_encryption_key = Self::encrypt_cek(&cek, new_key);
        Ok(data)
    }

    fn encrypt(data: DecryptedData, ad: AdditionalData, key: &PasetoV4Key) -> EncryptedData {
        // generate a random key for this entry
        // aka content-encryption-key (CEK)
        let random_key = PasetoV4Key::try_new_random().expect("could not source from random");

        // encode the implicit assertions
        let assertions = Assertions::from(ad).encode();

        // build the payload and encrypt the token
        let payload = serde_json::to_string(&AtuinPayload {
            data: general_purpose::URL_SAFE_NO_PAD.encode(data.0),
        })
        .expect("json encoding can't fail");
        let nonce: rusty_paseto::Key<32> =
            (&PasetoV4Key::try_new_random().expect("could not source from random")).into();
        let nonce = PasetoV4Nonce::from(&nonce);

        let token = Paseto::<rusty_paseto::V4, rusty_paseto::Local>::builder()
            .set_payload(rusty_paseto::Payload::from(payload.as_str()))
            .set_implicit_assertion(rusty_paseto::ImplicitAssertion::from(assertions.as_str()))
            .try_encrypt(&(&random_key).into(), &nonce)
            .expect("error encrypting atuin data");

        EncryptedData {
            data: token,
            content_encryption_key: Self::encrypt_cek(&random_key, key),
        }
    }

    fn decrypt(
        data: EncryptedData,
        ad: AdditionalData,
        key: &PasetoV4Key,
    ) -> Result<DecryptedData> {
        let token = data.data;
        let cek = Self::decrypt_cek(data.content_encryption_key, key)?;

        // encode the implicit assertions
        let assertions = Assertions::from(ad).encode();

        // decrypt the payload with the footer and implicit assertions
        let payload = Paseto::<rusty_paseto::V4, rusty_paseto::Local>::try_decrypt(
            &token,
            &(&cek).into(),
            None,
            rusty_paseto::ImplicitAssertion::from(&*assertions),
        )
        .context("could not decrypt entry")?;

        let payload: AtuinPayload = serde_json::from_str(&payload)?;
        let data = general_purpose::URL_SAFE_NO_PAD.decode(payload.data)?;
        Ok(DecryptedData(data))
    }
}

impl PasetoV4 {
    fn decrypt_cek(wrapped_cek: String, key: &PasetoV4Key) -> Result<PasetoV4Key> {
        let wrapping_key: rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local> = key.into();

        // let wrapping_key = PasetoSymmetricKey::from(Key::from(key));

        let AtuinFooter { kid, wpk } = serde_json::from_str(&wrapped_cek)
            .context("wrapped cek did not contain the correct contents")?;

        // check that the wrapping key matches the required key to decrypt.
        // In future, we could support multiple keys and use this key to
        // look up the key rather than only allow one key.
        // For now though we will only support the one key and key rotation will
        // have to be a hard reset
        let current_kid = wrapping_key.to_id();

        ensure!(
            current_kid == kid,
            "This record was encrypted with a different key than the one currently configured.\n\
             Currently using {current_kid}, expecting {kid}.\n\n\
             This usually means keys were rotated or do not match across machines. Run `atuin store verify` \
             to check the store. Before purging, back up the store: purging permanently deletes every local \
             record that cannot be decrypted, including records that may still be recoverable with an old key \
             or another machine's key. If the configured key is the one you intend to keep, run \
             `atuin store purge`."
        );

        // decrypt the random key
        Ok(wpk.unwrap_key(&wrapping_key)?.into())
    }

    /// Note that wrapping is a key-encryption-key (KEK)
    fn encrypt_cek(cek: &PasetoV4Key, wrapping_key: &PasetoV4Key) -> String {
        // wrap the random key so we can decrypt it later
        let wrapped_cek = AtuinFooter {
            wpk: cek.wrap_pie(wrapping_key),
            kid: wrapping_key.key_id(),
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
    wpk: PaserkV4PieWrappedKey,
    /// ID of the key which was used to wrap
    kid: PaserkV4KeyId,
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

#[cfg(test)]
mod tests {
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::{Host, Record};
    use rstest::*;

    use super::*;

    #[fixture]
    fn key() -> PasetoV4Key {
        PasetoV4Key::new_os_random()
    }

    #[fixture]
    fn data() -> DecryptedData {
        DecryptedData(vec![1, 2, 3, 4])
    }

    #[fixture]
    fn ad_parts() -> (RecordId, HostId) {
        (RecordId(uuid_v7()), HostId(uuid_v7()))
    }

    #[fixture]
    fn sample_record() -> Record<DecryptedData> {
        Record::builder()
            .id(RecordId(uuid_v7()))
            .version("v0".to_owned())
            .tag("kv".to_owned())
            .host(Host::new(HostId(uuid_v7())))
            .timestamp(1687244806000000)
            .data(DecryptedData(vec![1, 2, 3, 4]))
            .idx(0)
            .build()
    }

    #[rstest]
    fn round_trip(key: PasetoV4Key, data: DecryptedData, ad_parts: (RecordId, HostId)) {
        let (rid, hid) = ad_parts;
        let idx = 0;
        let ad = AdditionalData {
            id: &rid,
            version: "v0",
            tag: "kv",
            host: &hid,
            idx: &idx,
        };

        let encrypted = PasetoV4::encrypt(data.clone(), ad, &key);
        let decrypted = PasetoV4::decrypt(encrypted, ad, &key).unwrap();
        assert_eq!(decrypted, data);
    }

    #[rstest]
    fn same_entry_different_output(
        key: PasetoV4Key,
        data: DecryptedData,
        ad_parts: (RecordId, HostId),
    ) {
        let (rid, hid) = ad_parts;
        let idx = 0;
        let ad = AdditionalData {
            id: &rid,
            version: "v0",
            tag: "kv",
            host: &hid,
            idx: &idx,
        };

        let encrypted = PasetoV4::encrypt(data.clone(), ad, &key);
        let encrypted2 = PasetoV4::encrypt(data, ad, &key);

        assert_ne!(
            encrypted.data, encrypted2.data,
            "re-encrypting the same contents should have different output due to key randomization"
        );
    }

    #[rstest]
    fn cannot_decrypt_different_key(
        key: PasetoV4Key,
        data: DecryptedData,
        ad_parts: (RecordId, HostId),
    ) {
        let fake_key = PasetoV4Key::new_os_random();

        let (rid, hid) = ad_parts;
        let idx = 0;
        let ad = AdditionalData {
            id: &rid,
            version: "v0",
            tag: "kv",
            host: &hid,
            idx: &idx,
        };

        let encrypted = PasetoV4::encrypt(data, ad, &key);
        let error = PasetoV4::decrypt(encrypted, ad, &fake_key).unwrap_err();
        let message = error.to_string();

        assert!(message.contains(
            "This record was encrypted with a different key than the one currently configured."
        ));
        assert!(message.contains(&format!(
            "Currently using {}, expecting {}.",
            fake_key.key_id(),
            key.key_id()
        )));
    }

    #[rstest]
    fn cannot_decrypt_cek_with_missing_footer_contents() {
        let key = PasetoV4Key::new_os_random();

        let Err(error) = PasetoV4::decrypt_cek("{}".to_owned(), &key) else {
            panic!("missing footer contents should result in an error");
        };

        assert_eq!(
            error.to_string(),
            "wrapped cek did not contain the correct contents"
        );
    }

    #[rstest]
    fn cannot_decrypt_different_id(
        key: PasetoV4Key,
        data: DecryptedData,
        ad_parts: (RecordId, HostId),
    ) {
        let (rid, hid) = ad_parts;
        let idx = 0;
        let ad = AdditionalData {
            id: &rid,
            version: "v0",
            tag: "kv",
            host: &hid,
            idx: &idx,
        };

        let encrypted = PasetoV4::encrypt(data, ad, &key);

        let ad = AdditionalData {
            id: &RecordId(uuid_v7()),
            ..ad
        };
        let _ = PasetoV4::decrypt(encrypted, ad, &key).unwrap_err();
    }

    #[rstest]
    fn re_encrypt_round_trip(key: PasetoV4Key, data: DecryptedData, ad_parts: (RecordId, HostId)) {
        let key1 = key;
        let key2 = PasetoV4Key::new_os_random();

        let (rid, hid) = ad_parts;
        let idx = 0;
        let ad = AdditionalData {
            id: &rid,
            version: "v0",
            tag: "kv",
            host: &hid,
            idx: &idx,
        };

        let encrypted1 = PasetoV4::encrypt(data.clone(), ad, &key1);
        let encrypted2 = PasetoV4::re_encrypt(encrypted1.clone(), ad, &key1, &key2).unwrap();

        // we only re-encrypt the content keys
        assert_eq!(encrypted1.data, encrypted2.data);
        assert_ne!(
            encrypted1.content_encryption_key,
            encrypted2.content_encryption_key
        );

        let decrypted = PasetoV4::decrypt(encrypted2, ad, &key2).unwrap();

        assert_eq!(decrypted, data);
    }

    #[rstest]
    fn full_record_round_trip(sample_record: Record<DecryptedData>) {
        let key = PasetoV4Key::from([0x55; 32]);
        let encrypted = sample_record.encrypt::<PasetoV4>(&key);

        assert!(!encrypted.data.data.is_empty());
        assert!(!encrypted.data.content_encryption_key.is_empty());

        let decrypted = encrypted.decrypt::<PasetoV4>(&key).unwrap();

        assert_eq!(decrypted.data.0, [1, 2, 3, 4]);
    }

    #[rstest]
    fn full_record_round_trip_fail(sample_record: Record<DecryptedData>) {
        let key = PasetoV4Key::from([0x55; 32]);
        let encrypted = sample_record.encrypt::<PasetoV4>(&key);

        let mut enc1 = encrypted.clone();
        enc1.host = Host::new(HostId(uuid_v7()));
        let _ = enc1
            .decrypt::<PasetoV4>(&key)
            .expect_err("tampering with the host should result in auth failure");

        let mut enc2 = encrypted;
        enc2.id = RecordId(uuid_v7());
        let _ = enc2
            .decrypt::<PasetoV4>(&key)
            .expect_err("tampering with the id should result in auth failure");
    }
}

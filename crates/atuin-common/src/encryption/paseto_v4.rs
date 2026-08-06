//!  utilities for atuin.
use std::array::TryFromSliceError;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64_URL_SAFE_NO_PAD};
use rusty_paserk;
use rusty_paseto::Paseto;
use rusty_paseto::core as rusty_paseto;
use serde::{Deserialize, Serialize};
use serde_json;
use thiserror::Error;
use tokio;
use zeroize::Zeroize;

pub type PaserkV4KeyId = rusty_paserk::KeyId<rusty_paserk::V4, rusty_paserk::Local>;
pub type PaserkV4PieWrappedKey = rusty_paserk::PieWrappedKey<rusty_paserk::V4, rusty_paserk::Local>;
pub type ImplicitAssertion<'a> = rusty_paseto::ImplicitAssertion<'a>;
pub type Nonce<'a> = rusty_paseto::PasetoNonce<'a, rusty_paseto::V4, rusty_paseto::Local>;

/// Paseto V4 Key.
///
/// Intentionally **not** Copy to support zeroing out on Drop.
#[derive(Clone, PartialEq, Eq, derive_more::From, derive_more::Debug)]
#[debug("Key(*******)")]
pub struct Key([u8; 32]);

impl Key {
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

    /// Equivalent to [`rusty_paseto::Key<T>::try_new_random`].
    pub fn try_new_random() -> Result<Self, rusty_paseto::PasetoError> {
        let paseto: rusty_paseto::Key<32> = rusty_paseto::Key::<32>::try_new_random()?;
        Ok(paseto.into())
    }

    /// Equivalent to [`rusty_paserk::Key::to_id`].
    pub fn key_id(&self) -> PaserkV4KeyId {
        let paserk: rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local> = self.into();
        paserk.to_id()
    }

    /// Equivalent to [`rusty_paserk::Key::wrap_pie`].
    pub fn wrap_pie(&self, wrapping: &Key) -> PaserkV4PieWrappedKey {
        let p_self: rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local> = self.into();
        let p_wrapping: rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local> = wrapping.into();

        p_self.wrap_pie(&p_wrapping)
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl TryFrom<&[u8]> for Key {
    type Error = TryFromSliceError;

    fn try_from(bytes: &[u8]) -> std::result::Result<Self, Self::Error> {
        <[u8; 32]>::try_from(bytes).map(Self)
    }
}

impl From<&Key> for rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local> {
    fn from(value: &Key) -> Self {
        Self::from_bytes(*value.as_bytes())
    }
}

impl From<rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local>> for Key {
    fn from(value: rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local>) -> Self {
        Self(value.to_bytes())
    }
}

impl From<&Key> for rusty_paseto::PasetoSymmetricKey<rusty_paseto::V4, rusty_paseto::Local> {
    fn from(value: &Key) -> Self {
        rusty_paserk::Key::<rusty_paserk::V4, rusty_paserk::Local>::from(value).into()
    }
}

impl From<&Key> for rusty_paseto::Key<32> {
    fn from(value: &Key) -> Self {
        rusty_paseto::Key::<32>::from(value.0)
    }
}

impl From<rusty_paseto::Key<32>> for Key {
    fn from(value: rusty_paseto::Key<32>) -> Self {
        Self(*value)
    }
}

mod cek {
    use super::*;

    #[derive(Debug, Error)]
    pub enum EncryptionError {
        #[error("failed to serialize the given key: {_0}")]
        Json(#[from] serde_json::Error),
    }

    #[derive(Debug, Error)]
    pub enum DecryptionError {
        #[error("failed to deserialize the given key: {_0}")]
        Json(#[from] serde_json::Error),
        #[error("bad key. encrypted key id: {actual}, given decryption key: {given}")]
        MismatchedKey {
            actual: PaserkV4KeyId,
            given: PaserkV4KeyId,
        },
        #[error("failed to decrypt the CEK: {_0}")]
        Paseto(#[from] rusty_paserk::PasetoError),
    }

    /// Structure which contains the content encryption key.
    #[derive(Serialize, Deserialize)]
    pub struct Json {
        /// The content encryption key, encrypted by the parent key with the id `kid`.
        wpk: PaserkV4PieWrappedKey,
        /// ID of the key which was used to wrap the json structure.
        kid: PaserkV4KeyId,
    }

    impl Json {
        /// Create a JSON-serialized `String` for the given content encryption key.
        ///
        /// This will encrypt the given CEK with the given parent key, create the [`cek::Json`] and
        /// serialize it into JSON.
        pub fn encrypt(cek: &Key, parent_key: &Key) -> Result<String, EncryptionError> {
            Ok(serde_json::to_string(&cek::Json {
                wpk: cek.wrap_pie(parent_key),
                kid: parent_key.key_id(),
            })?)
        }

        /// Decrypt a serialized `&str` into the CEK key held under it.
        pub fn decrypt(encrypted_json: &str, key: &Key) -> Result<Key, DecryptionError> {
            let Self { kid, wpk } = serde_json::from_str(encrypted_json)?;

            if kid != key.key_id() {
                return Err(DecryptionError::MismatchedKey {
                    actual: kid,
                    given: key.key_id(),
                });
            }

            let wrapping_key: rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local> = key.into();
            Ok(wpk.unwrap_key(&wrapping_key)?.into())
        }
    }
}

const ENCODER: &'static base64::engine::general_purpose::GeneralPurpose = &B64_URL_SAFE_NO_PAD;

/// Data which was encrypted with the paseto encryption engine.
///
/// Note this contains the encrypted string as [`PasetoEncryptedData::data`] and the content
/// encryption key that it was encrypted with under [`PasetoEncryptedData::cek`].
///
/// See [`::encrypt`] for more information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedData {
    /// The encrypted payload as a string.
    pub raw: String,
    /// Content encryption key, encoded as a JSON string. See [`cek::Json`].
    pub cek: String,
}

#[derive(Debug, Error)]
pub enum EncryptionError {
    #[error("unexpected paseto error creating new CEK: {_0}")]
    CEKGeneration(rusty_paseto::PasetoError),
    #[error("JSON serialization error serializing data: {_0}")]
    DataJson(#[from] serde_json::Error),
    #[error("unexpected paseto error creating new nonce: {_0}")]
    NonceGeneration(rusty_paseto::PasetoError),
    #[error("unexpected encryption error: {_0}")]
    Encryption(rusty_paseto::PasetoError),
    #[error("unexpected error encrypting CEK: {_0}")]
    CEK(#[from] cek::EncryptionError),
}

#[derive(Debug, Error)]
pub enum DecryptionError {
    #[error("unexpected error decrypting CEK: {_0}")]
    CEK(#[from] cek::DecryptionError),
    #[error("failed to decrypt the payload: {_0}")]
    Decryption(#[from] rusty_paserk::PasetoError),
    #[error("failed to deserialize decrypted payload into json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to decode the deserialized payload: {_0}")]
    DecodingError(#[from] base64::DecodeError),
}

#[derive(Serialize, Deserialize)]
struct EncryptedJson {
    data: String,
}

/// Given a piece of data, encrypt it into a paseto-encrypted form.
///
/// This encryptor doesn't actually just encrypt. "encryption", within the context of this algorithm
/// is actually three operations:
///
///   - **_CEK_ generation**: The given data will be with a randomly-generated "content encryption
///     key", which is generated completely randomly.
///   - **Base64-encoding**: The data given is encoded into a url-safe non-padded b64 string. This
///     is necessary because Paseto V4 encryption does not actually support bytes.
///   - **JSON packing**: The resulting data is packed in a JSON of the shape
///     `{ "data": <b64-encoded> }`, and subsequently encoded.
///   - **The PASETO token** is then created, with the following:
///     - The payload is the aforementioned JSON.
///     - The given implicit assertion is optionally added.
///     - A randomly-generated nonce.
///
/// We return the encoded data as a [`EncryptedData`] structure.
///
/// It is what it is. Most of the work here is CPU-bound so we also offer an async version under
/// tokio, which off-loads work to the tokio blocking threads. See [`::encrypt_async`] for more
/// details.
///
/// ## CEK?
///
/// This cypher has a "content encryption key", which is a random 32B key, for each record. Each
/// given data slice gets its own encryption key, which, itself, is encrypted with the given `key`
/// parameter. The returned structure of `encode` is `EncryptedData`, eg.:
///
/// ```txt
/// EncryptedData {
///   // Note the JSON of the `EncryptedJson` type here:
///   data: String = '{ data: "ewqkbjvdbkhrkeqbewqhk...(encoded data)" }',
///   // Note the JSON of the `cek::Json` type here:
///   cek: String = '{
///     wpk: "ewquohewqk(encoded random CEK)",
///     kid: "21380127(hash [key id] of the CEK)"
///   }'
/// }
/// ```
///
/// # Original Author Notes
///
/// I, `@markovejnovic` have moved the original code away from `atuin-client` into `atuin-common`.
/// The original code came with some docs from the original author (`@conradludgate`), which I
/// present here verbatim (albeit formatted for rsdoc):
///
/// > Why do we use a random content-encryption key?
/// >
/// > Originally I was planning on using a derived key for encryption based on additional data.
/// > This would be a lot more secure than using the master key directly.
/// >
/// > However, there's an established norm of using a random key. This scheme might be otherwise
/// > known as:
/// > - client-side encryption
/// > - envelope encryption
/// > - key wrapping
/// >
/// > A HSM (Hardware Security Module) provider, eg: AWS, Azure, GCP, or even a physical device
/// > like a YubiKey will have some keys that they keep to themselves. These keys never leave
/// > their physical hardware. If they never leave the hardware, then encrypting large amounts
/// > of data means giving them the data and waiting. This is not a practical solution. Instead,
/// > generate a unique key for your data, encrypt that using your HSM and then store that with
/// > your data.
/// >
/// > See
/// >  - https://docs.aws.amazon.com/wellarchitected/latest/financial-services-industry-lens/use-envelope-encryption-with-customer-master-keys.html
/// >  - https://cloud.google.com/kms/docs/envelope-encryption
/// >  - https://learn.microsoft.com/en-us/azure/storage/blobs/client-side-encryption?tabs=dotnet#encryption-and-decryption-via-the-envelope-technique
/// >  - https://www.yubico.com/products/hardware-security-module/
/// >  - https://cheatsheetseries.owasp.org/cheatsheets/Cryptographic_Storage_Cheat_Sheet.html#encrypting-stored-keys
/// >
/// > Why would we care? In the past we have received some requests for company solutions. If in
/// > future we can configure a KMS service with little effort, then that would solve a lot of
/// > issues for their security team.
/// >
/// > Even for personal use, if a user is not comfortable with sharing keys between hosts,
/// > GCP HSM costs $1/month and $0.03 per 10,000 key operations. Assuming an active user runs
/// > 1000 atuin records a day, that would only cost them $1 and 10 cent a month.
/// >
/// > Additionally, key rotations are much simpler using this scheme. Rotating a key is as
/// > simple as re-encrypting the CEK, and not the message contents. This makes it very fast to
/// > rotate a key in bulk.
/// >
/// > For future reference, with asymmetric encryption, you can encrypt the CEK without the
/// > HSM's involvement, but decrypting will need the HSM. This allows the encryption path to
/// > still be extremely fast (no network calls) but downloads/decryption that happens in the
/// > background can make the network calls to the HSM
pub fn encrypt_sync<'a, IA>(
    data: &[u8],
    implicit_assertion: IA,
    key: &Key,
) -> Result<EncryptedData, EncryptionError>
where
    IA: Into<Option<ImplicitAssertion<'a>>>,
{
    let random_key = Key::try_new_random().map_err(EncryptionError::CEKGeneration)?;

    let payload = serde_json::to_string(&EncryptedJson {
        data: ENCODER.encode(data),
    })?;

    let nonce = Key::try_new_random().map_err(EncryptionError::NonceGeneration)?;
    let nonce: rusty_paseto::Key<32> = (&nonce).into();
    let nonce = rusty_paseto::PasetoNonce::<rusty_paseto::V4, rusty_paseto::Local>::from(&nonce);

    let mut enc_builder = Paseto::<rusty_paseto::V4, rusty_paseto::Local>::builder();
    enc_builder.set_payload(rusty_paseto::Payload::from(payload.as_str()));

    if let Some(assertion) = implicit_assertion.into() {
        enc_builder.set_implicit_assertion(assertion);
    }

    let token = enc_builder
        .try_encrypt(&(&random_key).into(), &nonce)
        .map_err(EncryptionError::Encryption)?;

    Ok(EncryptedData {
        raw: token,
        cek: cek::Json::encrypt(&random_key, key).map_err(EncryptionError::CEK)?,
    })
}

pub async fn encrypt_async<'a, IA, D>(
    data: D,
    implicit_assertion: IA,
    key: Key,
) -> Result<EncryptedData, EncryptionError>
where
    IA: Into<Option<ImplicitAssertion<'a>>> + Send + 'static,
    D: AsRef<[u8]> + Send + 'static,
{
    tokio::task::spawn_blocking(move || encrypt_sync(data.as_ref(), implicit_assertion, &key))
        .await
        .unwrap()
}

/// The dual to [`encrypt_sync`].
///
/// **Ensure you read that documentation. Does NOT do what the name of the function says.**
pub fn decrypt_sync<'a, IA: Into<Option<ImplicitAssertion<'a>>>>(
    data: &EncryptedData,
    implicit_assertion: IA,
    key: &Key,
) -> Result<Vec<u8>, DecryptionError> {
    let cek = cek::Json::decrypt(&data.cek, key)?;

    let payload_str = rusty_paseto::Paseto::<rusty_paseto::V4, rusty_paseto::Local>::try_decrypt(
        &data.raw,
        &(&cek).into(),
        None,
        implicit_assertion.into(),
    )?;

    let payload: EncryptedJson = serde_json::from_str(&payload_str)?;
    let decoded = ENCODER.decode(payload.data)?;

    Ok(decoded)
}

pub async fn decrypt_async<'a, IA>(
    data: EncryptedData,
    implicit_assertion: IA,
    key: Key,
) -> Result<Vec<u8>, DecryptionError>
where
    IA: Into<Option<ImplicitAssertion<'a>>> + Send + 'static,
{
    tokio::task::spawn_blocking(move || decrypt_sync(&data, implicit_assertion, &key))
        .await
        .unwrap()
}

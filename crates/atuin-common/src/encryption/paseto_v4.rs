//! PASETO v4 / PASERK envelope encryption for atuin records.
//!
//! See [`encrypt_sync`] for the encryption description.
use std::{
    array::TryFromSliceError,
    fs,
    io::{Read, Write},
    path::Path,
};

use base64::{
    Engine,
    engine::general_purpose::{STANDARD as B64_STANDARD, URL_SAFE_NO_PAD as B64_URL_SAFE_NO_PAD},
};
use crypto_secretbox::{KeyInit, XSalsa20Poly1305, aead};
use rusty_paseto::Paseto;
use rusty_paseto::core as rusty_paseto;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroize;

pub type PaserkV4KeyId = rusty_paserk::KeyId<rusty_paserk::V4, rusty_paserk::Local>;
pub type PaserkV4PieWrappedKey = rusty_paserk::PieWrappedKey<rusty_paserk::V4, rusty_paserk::Local>;
pub type ImplicitAssertion<'a> = rusty_paseto::ImplicitAssertion<'a>;

/// Used to encode the given raw bytes into a string before encrypting. See relevant docs.
static PAYLOAD_ENCODER: base64::engine::general_purpose::GeneralPurpose = B64_URL_SAFE_NO_PAD;

/// Used to encode the key in [`Key::encode`].
static KEY_ENCODER: base64::engine::general_purpose::GeneralPurpose = B64_STANDARD;

#[derive(Debug, Error)]
pub enum KeyDecodingError {
    #[error("failed to base64 decode the given string: {_0}")]
    B64Decode(#[from] base64::DecodeError),
    #[error("encryption key is empty")]
    EmptyKey,
    #[error("unexpected decoding error: {_0}")]
    DecodingError(crate::rmp::decode::DecodeError<'static>),
    #[error("encryption key is not the correct size")]
    InvalidSize,
    #[error("failed to parse the slice: {_0}")]
    FailedToParseSlice(#[from] TryFromSliceError),
    #[error("could not decode encryption key")]
    InvalidToken,
}

#[derive(Debug, Error)]
pub enum MnemonicLoadingError {
    #[error("key mnemonic was not valid")]
    InvalidMnemonic,
    #[error("key was not the correct length")]
    InvalidLength,
}

#[derive(Debug, Error)]
pub enum KeyFileLoadingError {
    #[error("the given key path does not exist")]
    NoEntry,
    #[error("unexpected io error: {_0}")]
    Io(#[from] std::io::Error),
    #[error("failed to decode the loaded key: {_0}")]
    Decoding(#[from] KeyDecodingError),
}

#[derive(Debug, Error)]
pub enum KeyFileStoringError {
    #[error("the given key path already exists")]
    AlreadyExists,
    #[error("unexpected io error: {_0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum KeyFileLoadOrGenerateError {
    #[error("failed to decode the loaded key: {_0}")]
    Decoding(#[from] KeyDecodingError),

    #[error("unexpected io error: {_0}")]
    Io(#[from] std::io::Error),
}

/// A type which contains a [`Key`] encoded as a B64 string. See [`Key::encode`] for more details.
///
/// **This should never implement ANY derive.** Most importantly, you should NEVER add `Clone`
/// (otherwise it is bug-prone and users will copy the plain-text string around) and `Serialize` so
/// it doesn't accidentally go over the wire.
pub struct PlainTextEncodedKey(String);

impl PlainTextEncodedKey {
    /// Leaks the plain-text encoded value into a `&str`.
    ///
    /// BEWARE: You should **never** take ownership of that `&str`. Bad things can happen (such as
    /// accidental serialization and transfer over the wire).
    #[must_use]
    pub const fn dangerously_leak_secret(&self) -> &str {
        self.0.as_str()
    }
}

impl Drop for PlainTextEncodedKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Paseto V4 Key.
///
/// Intentionally **not** Copy to support zeroing out on Drop. Intentionally not `Serialize` so it
/// doesn't end up across the wire.
#[derive(Clone, PartialEq, Eq, derive_more::From, derive_more::Debug)]
#[debug("Key(*******)")]
pub struct Key([u8; 32]);

impl Key {
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

    /// Generate a new key with the XSalsa20Poly1305 algorithm.
    pub fn generate() -> Self {
        <[u8; 32]>::from(XSalsa20Poly1305::generate_key(&mut aead::OsRng)).into()
    }

    /// Encode this key into a B64-encoded string, if possible.
    pub fn encode(&self) -> PlainTextEncodedKey {
        let key_bytes = self.as_bytes();
        // A msgpack array16 header (3 bytes) followed by each byte as at most a 2-byte uint.
        let mut buf = Vec::with_capacity(3 + 2 * key_bytes.len());
        // Writing to a `Vec` is infallible, so neither of these can actually error.
        rmp::encode::write_array_len(&mut buf, key_bytes.len() as u32)
            .expect("writing to a Vec is infallible");
        for b in key_bytes {
            rmp::encode::write_uint(&mut buf, u64::from(*b))
                .expect("writing to a Vec is infallible");
        }

        PlainTextEncodedKey(KEY_ENCODER.encode(buf))
    }

    pub fn decode(key: &str) -> Result<Self, KeyDecodingError> {
        let buf = KEY_ENCODER.decode(key.trim_end())?;

        // Legacy code used to naively encode the base64 string into the string. New code does this
        // rmp dance.
        match <[u8; 32]>::try_from(&*buf) {
            Ok(key) => Ok(key.into()),
            Err(_) => {
                if buf.is_empty() {
                    return Err(KeyDecodingError::EmptyKey);
                }

                let mut bytes = rmp::decode::Bytes::new(&buf);

                match rmp::Marker::from_u8(buf[0]) {
                    rmp::Marker::Bin8 => {
                        let len = rmp::decode::read_bin_len(&mut bytes)
                            .map_err(|e| KeyDecodingError::DecodingError(e.into()))?;
                        if len != 32 {
                            return Err(KeyDecodingError::InvalidSize);
                        }

                        let key = <[u8; 32]>::try_from(bytes.remaining_slice())?;

                        Ok(key.into())
                    }
                    rmp::Marker::Array16 => {
                        let len = rmp::decode::read_array_len(&mut bytes)
                            .map_err(|e| KeyDecodingError::DecodingError(e.into()))?;
                        if len != 32 {
                            return Err(KeyDecodingError::InvalidSize);
                        }

                        let mut key = [0u8; 32];
                        for i in &mut key {
                            *i = rmp::decode::read_int(&mut bytes)
                                .map_err(|e| KeyDecodingError::DecodingError(e.into()))?;
                        }
                        Ok(key.into())
                    }
                    _ => Err(KeyDecodingError::InvalidToken),
                }
            }
        }
    }

    /// Try to load the [`Self::encode`]d file from the given path.
    ///
    /// Mostly serves as a convenience function.
    pub fn try_load_from_path(path: &Path) -> Result<Self, KeyFileLoadingError> {
        if !path.exists() {
            return Err(KeyFileLoadingError::NoEntry);
        }

        // TODO(markovejnovic): Whether we should use fs_err or not is up for debate, but it was
        // used here historically, so we'll use it.
        let text = fs_err::read_to_string(path)?;
        Ok(Self::decode(&text)?)
    }

    /// Attempt to write this [`Self::encode`]d key into the given path.
    ///
    /// Refuses to overwrite a file that already exists.
    pub fn try_write_path(&self, path: &Path) -> Result<(), KeyFileStoringError> {
        if path.exists() {
            // We could try to load the path real fast and check whether the contents are the same
            // as to what the user wants -- save them an error handling if we can:
            let mut data = String::new();
            fs::File::open(path)?.read_to_string(&mut data)?;
            if data == self.encode().dangerously_leak_secret() {
                return Ok(());
            }

            return Err(KeyFileStoringError::AlreadyExists);
        }

        let mut file = fs::File::create(path)?;
        file.write_all(self.encode().dangerously_leak_secret().as_bytes())?;

        Ok(())
    }

    /// Write this [`Self::encode`]d key to `path`, replacing any existing file.
    ///
    /// Unlike [`Self::try_write_path`], this deliberately overwrites an existing key.
    pub fn overwrite_path(&self, path: &Path) -> std::io::Result<()> {
        let mut file = fs::File::create(path)?;
        file.write_all(self.encode().dangerously_leak_secret().as_bytes())?;

        Ok(())
    }

    /// [`Self::try_load_from_path`], except if the file doesn't exist, creates a key through
    /// [`Self::generate`], stores it and returns it.
    pub fn try_load_or_generate(path: &Path) -> Result<Self, KeyFileLoadOrGenerateError> {
        match Self::try_load_from_path(path) {
            Ok(s) => Ok(s),
            Err(KeyFileLoadingError::NoEntry) => {
                let key = Self::generate();
                match key.try_write_path(path) {
                    Ok(()) => Ok(key),
                    // We lost a race: another process wrote a key between our existence check and
                    // our write. Adopt whatever landed on disk rather than clobbering it or
                    // panicking.
                    Err(KeyFileStoringError::AlreadyExists) => Self::try_load_from_path(path)
                        .map_err(|e| match e {
                            KeyFileLoadingError::Io(io) => KeyFileLoadOrGenerateError::Io(io),
                            KeyFileLoadingError::Decoding(d) => {
                                KeyFileLoadOrGenerateError::Decoding(d)
                            }
                            KeyFileLoadingError::NoEntry => {
                                KeyFileLoadOrGenerateError::Io(std::io::Error::new(
                                    std::io::ErrorKind::NotFound,
                                    "key file vanished immediately after a concurrent write",
                                ))
                            }
                        }),
                    Err(KeyFileStoringError::Io(io)) => Err(io.into()),
                }
            }
            Err(KeyFileLoadingError::Io(io)) => Err(io.into()),
            Err(KeyFileLoadingError::Decoding(d)) => Err(d.into()),
        }
    }

    /// Get the mnemonic of this particular key.
    pub fn try_mnemonic(&self) -> Result<bip39::Mnemonic, bip39::ErrorKind> {
        bip39::Mnemonic::from_entropy(self.as_bytes(), bip39::Language::English)
    }

    /// Attempt to construct this key from a mnemonic.
    ///
    /// This has quite some logic associated with it that is of debatable decision. **Please read
    /// the implementation before using as it _could_ be a footgun for your use-case.**
    pub fn try_from_mnemonic(mnemonic: &str) -> Result<Self, MnemonicLoadingError> {
        match bip39::Mnemonic::from_phrase(mnemonic, bip39::Language::English) {
            Ok(mnemonic) => Ok(Self::try_from(mnemonic.entropy())
                .map_err(|_| MnemonicLoadingError::InvalidMnemonic)?),
            Err(err) => {
                match err {
                    // Assume the given thing was passed as a plain-text key itself.
                    bip39::ErrorKind::InvalidWord(_) => {
                        Self::decode(mnemonic).map_err(|_| MnemonicLoadingError::InvalidMnemonic)
                    }
                    bip39::ErrorKind::InvalidChecksum => Err(MnemonicLoadingError::InvalidMnemonic),
                    bip39::ErrorKind::InvalidKeysize(_)
                    | bip39::ErrorKind::InvalidWordLength(_)
                    | bip39::ErrorKind::InvalidEntropyLength(_, _) => {
                        Err(MnemonicLoadingError::InvalidLength)
                    }
                }
            }
        }
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

/// Data which was encrypted with the paseto encryption engine.
///
/// Contains the PASETO token (overloaded here to contain arbitrary data) as [`EncryptedData::raw`]
/// and the wrapped content-encryption key that sealed it as [`EncryptedData::cek`].
///
/// See [`encrypt_sync`] for more information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedData {
    /// The encrypted payload as a string.
    ///
    /// Serialized on the wire as `data` - the historical field name. Important this is stable.
    #[serde(rename = "data", alias = "raw")]
    pub raw: String,
    /// Content encryption key, encoded as a JSON string (the `cek::Json` envelope).
    ///
    /// On the wire as `content_encryption_key` for the same backwards-compatibility reason.
    #[serde(rename = "content_encryption_key", alias = "cek")]
    pub cek: String,
}

#[derive(Debug, Error)]
pub enum EncryptionError {
    #[error("unexpected paseto error creating new CEK: {_0}")]
    CekGeneration(rusty_paseto::PasetoError),
    #[error("JSON serialization error serializing data: {_0}")]
    DataJson(#[from] serde_json::Error),
    #[error("unexpected paseto error creating new nonce: {_0}")]
    NonceGeneration(rusty_paseto::PasetoError),
    #[error("unexpected encryption error: {_0}")]
    Encryption(rusty_paseto::PasetoError),
    #[error("unexpected error encrypting CEK: {_0}")]
    Cek(#[from] cek::EncryptionError),
}

#[derive(Debug, Error)]
pub enum DecryptionError {
    #[error("unexpected error decrypting CEK: {_0}")]
    Cek(#[from] cek::DecryptionError),
    #[error("failed to decrypt the payload: {_0}")]
    Decryption(#[from] rusty_paserk::PasetoError),
    #[error("failed to deserialize decrypted payload into json: {_0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to base64-decode the deserialized payload: {_0}")]
    Base64(#[from] base64::DecodeError),
}

#[derive(Debug, Error)]
pub enum ReencryptionError {
    #[error("unexpected error decrypting CEK: {_0}")]
    CekDec(cek::DecryptionError),
    #[error("unexpected error encrypting CEK: {_0}")]
    CekEnc(cek::EncryptionError),
}

#[derive(Serialize, Deserialize)]
struct EncryptedJson {
    data: String,
}

/// Given a piece of data, encrypt it into a paseto-encrypted form.
///
/// This encryptor doesn't actually just encrypt. "encryption", within the context of this algorithm
/// is actually a few operations:
///
///   - **_CEK_ generation**: The given data is encrypted with a randomly-generated "content
///     encryption key" (CEK).
///   - **Base64-encoding**: The data given is encoded into a url-safe non-padded b64 string. This
///     is necessary because Paseto V4 encryption does not actually support bytes.
///   - **JSON packing**: The resulting data is packed in a JSON of the shape
///     `{ "data": <b64-encoded> }`, and subsequently encoded.
///   - **The PASETO token** is then created, with the following:
///     - The payload is the aforementioned JSON.
///     - The given implicit assertion is optionally added.
///     - A randomly-generated nonce.
///
/// We return the encoded data as an [`EncryptedData`] structure.
///
/// Most of the work here is CPU-bound; callers on an async runtime should run this inside
/// `tokio::task::spawn_blocking` (or equivalent) rather than blocking the executor.
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
///   raw: String = '{ data: "ewqkbjvdbkhrkeqbewqhk...(encoded data)" }',
///   // Note the JSON of the `cek::Json` type here:
///   cek: String = '{
///     wpk: "ewquohewqk(encoded random CEK)",
///     kid: "21380127(hash [key id] of the CEK)"
///   }'
/// }
/// ```
///
/// # Why a random content-encryption key?
///
/// Design rationale, originally written by `@conradludgate`:
///
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
/// >  - <https://docs.aws.amazon.com/wellarchitected/latest/financial-services-industry-lens/use-envelope-encryption-with-customer-master-keys.html>
/// >  - <https://cloud.google.com/kms/docs/envelope-encryption>
/// >  - <https://learn.microsoft.com/en-us/azure/storage/blobs/client-side-encryption?tabs=dotnet#encryption-and-decryption-via-the-envelope-technique>
/// >  - <https://www.yubico.com/products/hardware-security-module/>
/// >  - <https://cheatsheetseries.owasp.org/cheatsheets/Cryptographic_Storage_Cheat_Sheet.html#encrypting-stored-keys>
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
    let random_key = Key::try_new_random().map_err(EncryptionError::CekGeneration)?;

    let payload = serde_json::to_string(&EncryptedJson {
        data: PAYLOAD_ENCODER.encode(data),
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
        cek: cek::Json::encrypt(&random_key, key).map_err(EncryptionError::Cek)?,
    })
}

/// The dual to [`encrypt_sync`]: unwrap the CEK with `key`, decrypt the PASETO token with it, then
/// base64-decode the payload back into the original bytes.
///
/// Like [`encrypt_sync`], this is more than a single decrypt step; see that function's docs for the
/// full envelope scheme.
pub fn decrypt_sync<'a, IA>(
    data: &EncryptedData,
    implicit_assertion: IA,
    key: &Key,
) -> Result<Vec<u8>, DecryptionError>
where
    IA: Into<Option<ImplicitAssertion<'a>>>,
{
    let cek = cek::Json::decrypt(&data.cek, key)?;

    let payload_str = rusty_paseto::Paseto::<rusty_paseto::V4, rusty_paseto::Local>::try_decrypt(
        &data.raw,
        &(&cek).into(),
        None,
        implicit_assertion.into(),
    )?;

    let payload: EncryptedJson = serde_json::from_str(&payload_str)?;
    let decoded = PAYLOAD_ENCODER.decode(payload.data)?;

    Ok(decoded)
}

pub fn reencrypt_sync(
    data: &EncryptedData,
    old_key: &Key,
    new_key: &Key,
) -> Result<EncryptedData, ReencryptionError> {
    Ok(EncryptedData {
        raw: data.raw.clone(),
        cek: cek::Json::encrypt(
            &(cek::Json::decrypt(&data.cek, old_key).map_err(ReencryptionError::CekDec)?),
            new_key,
        )
        .map_err(ReencryptionError::CekEnc)?,
    })
}

#[cfg(test)]
mod test {
    use super::*;
    use rstest::{fixture, rstest};

    #[fixture]
    fn key() -> Key {
        Key::from([
            27, 91, 42, 91, 210, 107, 9, 216, 170, 190, 242, 62, 6, 84, 69, 148, 148, 53, 251, 117,
            226, 167, 173, 52, 82, 34, 138, 110, 169, 124, 92, 229,
        ])
    }

    #[rstest]
    fn key_encodes_to_canonical_form(key: Key) {
        assert_eq!(
            key.encode().dangerously_leak_secret(),
            "3AAgG1sqW8zSawnM2MyqzL7M8j4GVEXMlMyUNcz7dczizKfMrTRSIsyKbsypfFzM5Q=="
        );
    }

    // a history of our key encodings — every one of these must still decode.
    // v11.0.0 xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==
    // v12.0.0 xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==
    // v13.0.0 xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==
    // v13.0.1 xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==
    // v14.0.0 xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==
    // v14.0.1 xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==
    // c7d89c1 3AAgG1sqW8zSawnM2MyqzL7M8j4GVEXMlMyUNcz7dczizKfMrTRSIsyKbsypfFzM5Q== (https://github.com/atuinsh/atuin/pull/805)
    // b53ca35 3AAgG1sqW8zSawnM2MyqzL7M8j4GVEXMlMyUNcz7dczizKfMrTRSIsyKbsypfFzM5Q== (https://github.com/atuinsh/atuin/pull/974)
    // v15.0.0 3AAgG1sqW8zSawnM2MyqzL7M8j4GVEXMlMyUNcz7dczizKfMrTRSIsyKbsypfFzM5Q==
    // b8b57c8 xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==                     (https://github.com/atuinsh/atuin/pull/1057)
    // 8c94d79 3AAgG1sqW8zSawnM2MyqzL7M8j4GVEXMlMyUNcz7dczizKfMrTRSIsyKbsypfFzM5Q== (https://github.com/atuinsh/atuin/pull/1089)
    #[rstest]
    #[case::legacy_v11("xCAbWypb0msJ2Kq+8j4GVEWUlDX7deKnrTRSIopuqXxc5Q==")]
    #[case::canonical("3AAgG1sqW8zSawnM2MyqzL7M8j4GVEXMlMyUNcz7dczizKfMrTRSIsyKbsypfFzM5Q==")]
    fn decodes_supported_key_encoding(key: Key, #[case] encoded: &str) {
        assert_eq!(Key::decode(encoded).expect(encoded), key);
    }

    #[rstest]
    #[case::empty("")]
    #[case::whitespace("\n")]
    fn decode_blank_key_is_error_not_panic(#[case] input: &str) {
        // an empty (or whitespace-only) key decodes to an empty buffer;
        // decoding must return an error rather than panic indexing buf[0]
        assert!(Key::decode(input).is_err());
    }

    #[rstest]
    fn encrypted_data_wire_format_is_stable() {
        // The sync wire contract: these JSON field names must stay `data` and
        // `content_encryption_key` regardless of the Rust field names, or old and new
        // clients/servers (which do not upgrade atomically) can no longer exchange records.
        let data = EncryptedData {
            raw: "R".to_owned(),
            cek: "C".to_owned(),
        };
        assert_eq!(
            serde_json::to_string(&data).unwrap(),
            r#"{"data":"R","content_encryption_key":"C"}"#
        );

        // The historical wire form still deserializes...
        let from_wire: EncryptedData =
            serde_json::from_str(r#"{"data":"R","content_encryption_key":"C"}"#).unwrap();
        assert_eq!(from_wire, data);

        // ...and the internal field names are accepted as aliases on the way in.
        let from_alias: EncryptedData = serde_json::from_str(r#"{"raw":"R","cek":"C"}"#).unwrap();
        assert_eq!(from_alias, data);
    }

    #[rstest]
    fn overwrite_path_replaces_an_existing_key() {
        let dir = std::env::temp_dir().join(format!("atuin-key-overwrite-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("key");

        let old = Key::from([0x11u8; 32]);
        let new = Key::from([0x22u8; 32]);

        old.try_write_path(&path)
            .expect("first write creates the file");

        // try_write_path refuses to replace a *different* key (correct for create-if-missing)...
        assert!(matches!(
            new.try_write_path(&path),
            Err(KeyFileStoringError::AlreadyExists)
        ));
        assert_eq!(Key::try_load_from_path(&path).unwrap(), old);

        // ...but overwrite_path deliberately replaces it, as key rotation requires.
        new.overwrite_path(&path)
            .expect("overwrite replaces the key");
        assert_eq!(Key::try_load_from_path(&path).unwrap(), new);

        let _ = fs::remove_dir_all(&dir);
    }
}

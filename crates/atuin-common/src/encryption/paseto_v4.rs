//! PASETO v4 / PASERK envelope encryption for atuin records.
//!
//! See [`encrypt_sync`] for the encryption description.
use std::array::TryFromSliceError;
use std::fs;
use std::io::Write;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::{
    STANDARD as B64_STANDARD, URL_SAFE_NO_PAD as B64_URL_SAFE_NO_PAD,
};
use crypto_secretbox::{KeyInit, XSalsa20Poly1305, aead};
use easy_cast::Conv;
use rusty_paseto::{Paseto, core as rusty_paseto};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

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

    /// This error will essentially never happen in practice. It requires
    /// `/path/to/.key.atuin-tmp.{i}` to exist for *every* `i` up to `usize::MAX`.
    #[error("all temporary file paths are in use")]
    TempFilesExhausted,
}

#[derive(Debug, Error)]
pub enum KeyFileLoadOrGenerateError {
    #[error("failed to decode the loaded key: {_0}")]
    Decoding(#[from] KeyDecodingError),

    #[error("unexpected io error: {_0}")]
    Io(#[from] std::io::Error),

    /// See comment on [`KeyFileStoringError::TempFilesExhausted`] -- this error will essentially
    /// never happen.
    #[error("failed to create key: all temporary file paths are in use")]
    TempFilesExhausted,
}

/// Owner read/write only, since the key file decrypts all synced data.
#[cfg(unix)]
const KEY_FILE_MODE: u32 = 0o600;

fn key_file_options() -> fs::OpenOptions {
    let mut opts = fs::OpenOptions::new();
    opts.write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, KEY_FILE_MODE);
    opts
}

/// A type which contains a [`Key`] encoded as a B64 string. See [`Key::encode`] for more details.
#[derive(Clone, Debug)]
pub struct PlainTextEncodedKey(SecretString);

impl PlainTextEncodedKey {
    /// Leaks the plain-text encoded value into a `&str`.
    ///
    /// BEWARE: You should **never** take ownership of that `&str`. Bad things can happen (such as
    /// accidental serialization and transfer over the wire).
    #[must_use]
    pub fn dangerously_leak_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

/// Paseto V4 Key.
///
/// Intentionally **not** Copy to support zeroing out on Drop. Intentionally not `Serialize` so it
/// doesn't end up across the wire.
#[derive(Clone, PartialEq, Eq, derive_more::From, derive_more::Debug)]
#[debug("paseto_v4::Key(*******)")]
pub struct Key([u8; 32]);

impl Key {
    /// Borrow the raw key bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Equivalent to [`rusty_paserk::Key::new_os_random()`].
    #[must_use]
    pub fn new_os_random() -> Self {
        rusty_paserk::Key::<rusty_paserk::V4, rusty_paserk::Local>::new_os_random().into()
    }

    /// Equivalent to [`rusty_paseto::Key<T>::try_new_random`].
    pub fn try_new_random() -> Result<Self, rusty_paseto::PasetoError> {
        let paseto: rusty_paseto::Key<32> = rusty_paseto::Key::<32>::try_new_random()?;
        Ok(paseto.into())
    }

    /// Equivalent to [`rusty_paserk::Key::to_id`].
    #[must_use]
    pub fn key_id(&self) -> PaserkV4KeyId {
        let paserk: rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local> = self.into();
        paserk.to_id()
    }

    /// Equivalent to [`rusty_paserk::Key::wrap_pie`].
    #[must_use]
    pub fn wrap_pie(&self, wrapping: &Self) -> PaserkV4PieWrappedKey {
        let p_self: rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local> = self.into();
        let p_wrapping: rusty_paserk::Key<rusty_paserk::V4, rusty_paserk::Local> = wrapping.into();

        p_self.wrap_pie(&p_wrapping)
    }

    /// Generate a new key with the XSalsa20Poly1305 algorithm.
    #[must_use]
    pub fn generate() -> Self {
        <[u8; 32]>::from(XSalsa20Poly1305::generate_key(&mut aead::OsRng)).into()
    }

    /// Encode this key into a B64-encoded string, if possible.
    #[must_use]
    pub fn encode(&self) -> PlainTextEncodedKey {
        let key_bytes = self.as_bytes();
        // A msgpack array16 header (3 bytes) followed by each byte as at most a 2-byte uint.
        let mut buf = Zeroizing::new(Vec::with_capacity(3 + 2 * key_bytes.len()));
        // Writing to a `Vec` is infallible, so neither of these can actually error.
        rmp::encode::write_array_len(&mut *buf, u32::conv(key_bytes.len()))
            .expect("writing to a Vec is infallible");
        for b in key_bytes {
            rmp::encode::write_uint(&mut *buf, u64::from(*b))
                .expect("writing to a Vec is infallible");
        }

        PlainTextEncodedKey(KEY_ENCODER.encode(&*buf).into())
    }

    pub fn decode(key: &str) -> Result<Self, KeyDecodingError> {
        let buf = Zeroizing::new(KEY_ENCODER.decode(key.trim_end())?);

        // Legacy code used to naively encode the base64 string into the string. New code does this
        // rmp dance.
        match <[u8; 32]>::try_from(buf.as_slice()).map(Zeroizing::new) {
            Ok(key) => Ok((*key).into()),
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

                        let key = Zeroizing::new(<[u8; 32]>::try_from(bytes.remaining_slice())?);

                        Ok((*key).into())
                    }
                    rmp::Marker::Array16 => {
                        let len = rmp::decode::read_array_len(&mut bytes)
                            .map_err(|e| KeyDecodingError::DecodingError(e.into()))?;
                        if len != 32 {
                            return Err(KeyDecodingError::InvalidSize);
                        }

                        let mut key = Zeroizing::new([0u8; 32]);
                        for i in key.iter_mut() {
                            *i = rmp::decode::read_int(&mut bytes)
                                .map_err(|e| KeyDecodingError::DecodingError(e.into()))?;
                        }
                        Ok((*key).into())
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
        let text = SecretString::from(fs_err::read_to_string(path)?);
        Ok(Self::decode(text.expose_secret())?)
    }

    /// Attempt to write this [`Self::encode`]d key into the given path.
    ///
    /// Refuses to overwrite a file that already exists.
    pub fn try_write_path(&self, path: &Path) -> Result<(), KeyFileStoringError> {
        use std::io::{Error, ErrorKind};
        use std::sync::atomic::{AtomicUsize, Ordering};

        // To avoid race conditions, this function:
        //
        // 1. Creates a temporary file in the same directory as `path`, named after the key file
        //    with a per-writer-unique tag (this process's id plus a monotonic counter) so that
        //    concurrent writers never pick the same temp name. That collision is not benign on
        //    Windows: `create_new` on a name another writer has just removed hits the file's
        //    "delete pending" window and fails with `ERROR_ACCESS_DENIED` rather than
        //    `AlreadyExists`. The trailing `.{i}` (starting at 0) only disambiguates the
        //    near-impossible case where a stale temp file already holds the name.
        //
        //    For example, `/path/to/key` -> `/path/to/.key.atuin-tmp.4321.7.0`.
        //
        // 2. Writes the key to the temporary path.
        //
        // 3. Hardlinks the temporary path to the real key path (`path`). Hardlinking will fail if
        //    the destination already exists, which is what we want.
        //
        // 4. Removes the temporary file.

        let dir = path.parent().ok_or(Error::from(ErrorKind::IsADirectory))?;
        let name = path.file_name().ok_or(Error::from(ErrorKind::IsADirectory))?;

        static TMP_COUNTER: AtomicUsize = AtomicUsize::new(0);
        let tag = format!(
            ".atuin-tmp.{}.{}",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let mut base_tmp_name = std::ffi::OsString::from(".");
        base_tmp_name.push(name);
        base_tmp_name.push(tag);

        let mut i: usize = 0;
        match loop {
            let mut tmp_path = dir.join(&base_tmp_name);
            tmp_path.add_extension(i.to_string());

            // `tmp_path` is first so it gets dropped after `tmp_file`. `tmp_path` is a
            // `RemoveOnDropPath` so it will remove the file when dropped, but this will fail on
            // Windows if the file is still open.
            let (tmp_path, mut tmp_file) = match key_file_options().create_new(true).open(&tmp_path)
            {
                Ok(file) => (crate::fs::RemoveOnDropPath(tmp_path), file),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    // This error will essentially never happen in practice. It requires
                    // `/path/to/.key.atuin-tmp.{i}` to exist for *every* `i` up to
                    // `usize::MAX`.
                    i = i.checked_add(1).ok_or(KeyFileStoringError::TempFilesExhausted)?;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            tmp_file.write_all(self.encode().dangerously_leak_secret().as_bytes())?;
            tmp_file.sync_all()?;
            drop(tmp_file);
            break std::fs::hard_link(&tmp_path, path);
        } {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                return Err(KeyFileStoringError::AlreadyExists);
            }
            Err(e) if e.kind() == ErrorKind::Unsupported => {}
            Err(e) => return Err(e.into()),
        }

        // Hardlinks are unsupported. This is unlikely but can happen on FAT32/exFAT filesystems.
        // Fall back to creating the file and then writing to it. This has the possibility of a race
        // condition where another process could observe a partially written key file, but it is
        // better than unconditionally failing to create the key file. In any case we are careful
        // not to overwrite an existing key file.
        let mut file = match key_file_options().create_new(true).open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                return Err(KeyFileStoringError::AlreadyExists);
            }
            Err(e) => return Err(e.into()),
        };
        file.write_all(self.encode().dangerously_leak_secret().as_bytes())?;
        Ok(())
    }

    /// Write this [`Self::encode`]d key to `path`, replacing any existing file.
    ///
    /// Unlike [`Self::try_write_path`], this deliberately overwrites an existing key.
    pub fn overwrite_path(&self, path: &Path) -> std::io::Result<()> {
        // TODO(taylordotfish): This has a race condition where another process can observe a
        // partially written key file. We should write to a temp file, similar to
        // `Self::try_write_path`, and then use `rename` to move it to the key path (not `hardlink`
        // because we do want it to overwrite an existing key).
        let mut file = key_file_options().create(true).truncate(true).open(path)?;
        file.write_all(self.encode().dangerously_leak_secret().as_bytes())?;
        file.sync_all()?;
        // The mode only applies on creation, so tighten a key file written before it was set.
        // This goes after the write: callers re-encrypt the store first, so a failed chmod must
        // not leave the file truncated without the new key.
        #[cfg(unix)]
        file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(KEY_FILE_MODE))?;

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
                    Err(KeyFileStoringError::TempFilesExhausted) => {
                        Err(KeyFileLoadOrGenerateError::TempFilesExhausted)
                    }
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
        Self::from(value.0)
    }
}

impl From<rusty_paseto::Key<32>> for Key {
    fn from(value: rusty_paseto::Key<32>) -> Self {
        Self(*value)
    }
}

mod cek {
    use serde::{Deserialize, Serialize};
    use thiserror::Error;

    use super::{Key, PaserkV4KeyId, PaserkV4PieWrappedKey};

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
        /// This will encrypt the given CEK with the given parent key, create the [`Json`] and
        /// serialize it into JSON.
        pub fn encrypt(cek: &Key, parent_key: &Key) -> Result<String, EncryptionError> {
            Ok(serde_json::to_string(&Self {
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

/// [`EncryptedData`] in its storage form.
///
/// The wire form above is what PASETO and PASERK speak: a base64url token and a JSON envelope
/// holding two base64url PASERK strings. That is what the crypto library produces and what the
/// server accepts, but on disk it is roughly 40% air. This is the same two values with the
/// base64 and JSON stripped.
///
/// Packing is the trust boundary: it refuses anything that is not in the canonical wire form
/// rather than guess at it, so every value of this type unpacks to exactly the string it came
/// from and unpacking cannot fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedBytes {
    /// The token payload: nonce, ciphertext, tag.
    pub data: Vec<u8>,
    /// The PIE-wrapped content key (tag, nonce, key: 96 bytes) followed by the 33-byte id of the
    /// key that wrapped it. Both are fixed by the PASERK v4 spec.
    pub cek: [u8; CEK_LEN],
}

const WPK_LEN: usize = 96;
const KID_LEN: usize = 33;
pub const CEK_LEN: usize = WPK_LEN + KID_LEN;

#[derive(Debug, Error)]
#[error("`{0}` is not in the PASETO wire form")]
pub struct NotWireForm(&'static str);

impl EncryptedBytes {
    const TOKEN_PREFIX: &str = "v4.local.";
    const WPK_PREFIX: &str = "k4.local-wrap.pie.";
    const KID_PREFIX: &str = "k4.lid.";

    /// `B64_URL_SAFE_NO_PAD` rejects padding and non-zero trailing bits, so a successful decode
    /// re-encodes to the same string.
    fn decode(s: &str, prefix: &str) -> Option<Vec<u8>> {
        B64_URL_SAFE_NO_PAD.decode(s.strip_prefix(prefix)?).ok()
    }

    fn pack_cek(cek: &str) -> Option<[u8; CEK_LEN]> {
        let json: serde_json::Value = serde_json::from_str(cek).ok()?;
        let obj = json.as_object()?;
        let wpk = Self::decode(obj.get("wpk")?.as_str()?, Self::WPK_PREFIX)?;
        let kid = Self::decode(obj.get("kid")?.as_str()?, Self::KID_PREFIX)?;
        let bytes: [u8; CEK_LEN] = [wpk, kid].concat().try_into().ok()?;

        // The JSON must round-trip too: field order and whitespace are not ours to normalise.
        (Self::unpack_cek(&bytes) == cek).then_some(bytes)
    }

    fn unpack_cek(bytes: &[u8; CEK_LEN]) -> String {
        let (wpk, kid) = bytes.split_at(WPK_LEN);

        serde_json::json!({
            "wpk": format!("{}{}", Self::WPK_PREFIX, B64_URL_SAFE_NO_PAD.encode(wpk)),
            "kid": format!("{}{}", Self::KID_PREFIX, B64_URL_SAFE_NO_PAD.encode(kid)),
        })
        .to_string()
    }
}

impl TryFrom<&EncryptedData> for EncryptedBytes {
    type Error = NotWireForm;

    fn try_from(data: &EncryptedData) -> Result<Self, Self::Error> {
        Ok(Self {
            data: Self::decode(&data.raw, Self::TOKEN_PREFIX).ok_or(NotWireForm("data"))?,
            cek: Self::pack_cek(&data.cek).ok_or(NotWireForm("cek"))?,
        })
    }
}

impl From<&EncryptedBytes> for EncryptedData {
    fn from(bytes: &EncryptedBytes) -> Self {
        Self {
            raw: format!(
                "{}{}",
                EncryptedBytes::TOKEN_PREFIX,
                B64_URL_SAFE_NO_PAD.encode(&bytes.data)
            ),
            cek: EncryptedBytes::unpack_cek(&bytes.cek),
        }
    }
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
    use rstest::{fixture, rstest};

    use super::*;

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

        old.try_write_path(&path).expect("first write creates the file");

        // try_write_path refuses to replace a *different* key (correct for create-if-missing)...
        assert!(matches!(new.try_write_path(&path), Err(KeyFileStoringError::AlreadyExists)));
        assert_eq!(Key::try_load_from_path(&path).unwrap(), old);

        // ...but overwrite_path deliberately replaces it, as key rotation requires.
        new.overwrite_path(&path).expect("overwrite replaces the key");
        assert_eq!(Key::try_load_from_path(&path).unwrap(), new);

        let _ = fs::remove_dir_all(&dir);
    }

    #[rstest]
    fn concurrent_generation_yields_one_complete_key() {
        // `try_write_path` used to create the key file and only then write the key into it, so a
        // concurrent reader could observe a zero-length file and fail with
        // `KeyDecodingError::EmptyKey`. Nothing in shell startup creates the key, so the first
        // writers really are concurrent in practice: the backgrounded `atuin history end` hook and
        // a foreground `atuin search`.
        const THREADS: usize = 8;
        const ROUNDS: usize = 100;

        for round in 0..ROUNDS {
            let dir = tempfile::tempdir().expect("create temp dir");
            let path = dir.path().join("key");
            let barrier = std::sync::Barrier::new(THREADS);

            let keys: Vec<Key> = std::thread::scope(|scope| {
                let threads: Vec<_> = (0..THREADS)
                    .map(|_| {
                        scope.spawn(|| {
                            barrier.wait();
                            Key::try_load_or_generate(&path)
                                .unwrap_or_else(|e| panic!("round {round}: {e}"))
                        })
                    })
                    .collect();
                threads.into_iter().map(|t| t.join().expect("thread panicked")).collect()
            });

            // Every racer must end up holding the one key that actually landed on disk; a racer
            // that kept a key the file does not have would encrypt records nothing can decrypt.
            let stored = Key::try_load_from_path(&path).expect("key file is readable");
            for (i, key) in keys.iter().enumerate() {
                assert_eq!(
                    *key, stored,
                    "round {round}: thread {i} kept a key that is not on disk"
                );
            }
        }
    }

    #[rstest]
    fn try_write_path_leaves_no_temporary_files() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("key");

        Key::from([0x33u8; 32]).try_write_path(&path).expect("write creates the key");

        let mut names: Vec<String> = fs::read_dir(dir.path())
            .expect("read temp dir")
            .map(|entry| entry.expect("dir entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["key"], "the temporary file was left behind");
    }

    #[cfg(unix)]
    #[rstest]
    fn key_file_is_owner_only(#[values(false, true)] preexisting: bool) {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("key");
        let key = Key::from([0x44u8; 32]);

        if preexisting {
            fs::write(&path, "stale").expect("write stale key");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod");
            key.overwrite_path(&path).expect("overwrite the key");
        } else {
            key.try_write_path(&path).expect("write the key");
        }

        let mode = fs::metadata(&path).expect("stat key").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[rstest]
    fn encrypted_bytes_round_trip_and_shrink(key: Key) {
        let data = encrypt_sync(b"ls -la", None, &key).unwrap();
        let bytes = EncryptedBytes::try_from(&data).unwrap();

        assert_eq!(EncryptedData::from(&bytes), data);
        assert!(
            bytes.data.len() * 4 < data.raw.len() * 3,
            "{} -> {}",
            data.raw.len(),
            bytes.data.len()
        );
        assert!(CEK_LEN * 4 < data.cek.len() * 3, "{} -> {CEK_LEN}", data.cek.len());
    }

    #[rstest]
    #[case::empty("")]
    #[case::plaintext_manifest("001{\"host\":\"x\"}")]
    #[case::token_with_footer("v4.local.abc.footer")]
    #[case::not_base64("v4.local.not base64!")]
    #[case::non_canonical_base64("v4.local.QR")]
    #[case::cek_wrong_shape(r#"{"wpk":"x","kid":"y"}"#)]
    fn encrypted_bytes_refuse_anything_not_in_wire_form(#[case] value: &str, key: Key) {
        let good = encrypt_sync(b"ls -la", None, &key).unwrap();
        let bad_data = EncryptedData {
            raw: value.into(),
            cek: good.cek.clone(),
        };
        let bad_cek = EncryptedData {
            raw: good.raw,
            cek: value.into(),
        };

        assert!(EncryptedBytes::try_from(&bad_data).is_err());
        assert!(EncryptedBytes::try_from(&bad_cek).is_err());
    }

    #[rstest]
    fn encrypted_bytes_refuse_reformatted_cek_json(key: Key) {
        let good = encrypt_sync(b"ls -la", None, &key).unwrap();
        let reordered: serde_json::Value = serde_json::from_str(&good.cek).unwrap();
        let reordered = format!("{{\"kid\":{},\"wpk\":{}}}", reordered["kid"], reordered["wpk"]);

        assert!(
            EncryptedBytes::try_from(&EncryptedData {
                cek: reordered,
                ..good
            })
            .is_err()
        );
    }
}

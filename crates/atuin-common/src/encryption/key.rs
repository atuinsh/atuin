use std::array::TryFromSliceError;
use std::fs;
use std::io::Write;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64_STANDARD;
use crypto_secretbox::{KeyInit, XSalsa20Poly1305, aead};
use easy_cast::Conv;
use secrecy::{ExposeSecret, SecretString};
use subtle::ConstantTimeEq as _;
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

use super::KeyId;

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

/// The master key every content key is wrapped under.
///
/// Intentionally **not** Copy to support zeroing out on Drop. Intentionally not `Serialize` so it
/// doesn't end up across the wire.
#[derive(Clone, derive_more::Debug)]
#[debug("Key(*******)")]
pub struct Key {
    bytes: [u8; 32],
    /// Cached: every open compares it, and threads.db persists its Display.
    id: KeyId,
}

impl Key {
    /// Borrow the raw key bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    /// The PASERK `k4.lid` of this key.
    #[must_use]
    pub const fn key_id(&self) -> KeyId {
        self.id
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
            let (tmp_path, mut tmp_file) =
                match Self::file_options().create_new(true).open(&tmp_path) {
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
        let mut file = match Self::file_options().create_new(true).open(path) {
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
        let mut file = Self::file_options().create(true).truncate(true).open(path)?;
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

    /// The options every key-file write opens with: write-only, owner read/write on unix.
    fn file_options() -> fs::OpenOptions {
        let mut opts = fs::OpenOptions::new();
        opts.write(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, KEY_FILE_MODE);
        opts
    }
}

impl From<[u8; 32]> for Key {
    fn from(bytes: [u8; 32]) -> Self {
        Self {
            id: KeyId::of(&bytes),
            bytes,
        }
    }
}

impl TryFrom<&[u8]> for Key {
    type Error = TryFromSliceError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        <[u8; 32]>::try_from(bytes).map(Self::from)
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.bytes.ct_eq(&other.bytes).into()
    }
}

impl Eq for Key {}

impl Drop for Key {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

#[cfg(test)]
mod tests {
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

    // a history of our key encodings -- every one of these must still decode.
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
    fn overwrite_path_replaces_an_existing_key() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("key");

        let old = Key::from([0x11u8; 32]);
        let new = Key::from([0x22u8; 32]);

        old.try_write_path(&path).expect("first write creates the file");

        // try_write_path refuses to replace a *different* key (correct for create-if-missing)...
        assert!(matches!(new.try_write_path(&path), Err(KeyFileStoringError::AlreadyExists)));
        assert_eq!(Key::try_load_from_path(&path).unwrap(), old);

        // ...but overwrite_path deliberately replaces it, as key rotation requires.
        new.overwrite_path(&path).expect("overwrite replaces the key");
        assert_eq!(Key::try_load_from_path(&path).unwrap(), new);
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
    #[case::first_byte(0)]
    #[case::last_byte(31)]
    fn keys_one_byte_apart_differ(#[case] index: usize) {
        let mut bytes = [0x55; 32];
        let key = Key::from(bytes);
        bytes[index] ^= 1;

        assert_ne!(key, Key::from(bytes));
        assert_eq!(key, Key::from([0x55; 32]));
    }

    #[rstest]
    #[case::generate(Key::generate)]
    #[case::from_array(|| Key::from([0x55; 32]))]
    #[case::try_from_slice(|| Key::try_from([0x55; 32].as_slice()).unwrap())]
    #[case::decode(|| Key::decode(Key::from([0x55; 32]).encode().dangerously_leak_secret()).unwrap())]
    #[case::mnemonic(|| Key::try_from_mnemonic(&Key::from([0x55; 32]).try_mnemonic().unwrap().into_phrase()).unwrap())]
    #[case::load_from_path(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        Key::from([0x55; 32]).try_write_path(&path).unwrap();
        Key::try_load_from_path(&path).unwrap()
    })]
    fn every_constructor_caches_its_key_id(#[case] construct: fn() -> Key) {
        let key = construct();

        assert_eq!(key.key_id(), KeyId::of(key.as_bytes()));
    }
}

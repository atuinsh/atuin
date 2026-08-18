//! End-to-end encryption for share sessions: AES-256-GCM under one
//! per-session key.
//!
//! The hub is a blind relay — it orders, stores, and replays frames but must
//! never be able to read or forge a byte of terminal content. Everything
//! content-bearing (`output` data, keyframe data, write-mode `input` data)
//! crosses the wire as a sealed blob; envelope metadata (`seq`, sizes,
//! participant counts) stays plaintext because the hub needs it and it is
//! content-free.
//!
//! # Wire format
//!
//! ```text
//! blob = nonce(12) || ciphertext(len(pt)) || tag(16)   // 28 bytes overhead
//! ```
//!
//! The blob then rides the existing base64+JSON transport layer unchanged.
//! `ciphertext || tag` is exactly what both `aes_gcm::Aead::encrypt` and
//! WebCrypto's `crypto.subtle.encrypt` emit, so both ends split at byte 12 and
//! hand the rest to the AEAD. [`SessionKey::decrypt`] rejects blobs shorter
//! than the 28-byte overhead before attempting decryption.
//!
//! # Invariants
//!
//! * **The nonce is never derived from `seq`.** Keyframes are re-encrypted and
//!   re-sent on demand, the hub replays history to late joiners, and a host
//!   reconnect reuses the key under a new session token — any counter-derived
//!   nonce scheme would eventually repeat a nonce, and GCM nonce reuse is
//!   catastrophic. A fresh 12-byte random nonce per [`SessionKey::encrypt`]
//!   call is correct by construction: a re-sent keyframe is a fresh encryption
//!   with a fresh nonce.
//! * **One key per session.** Generated once per `run_share`, shared with
//!   viewers only via the URL fragment (which never reaches the hub), and
//!   never rekeyed; random nonces stay far under the NIST SP 800-38D 2^32
//!   random-IV bound for any plausible frame count.
//! * **The AAD binds frame type and envelope `seq`** ([`frame_aad`]): a blob
//!   sealed as viewer input can never be replayed to a viewer as output, and a
//!   hub that rewrites the plaintext `seq` it orders by causes an
//!   authentication failure instead of spliced history. For **input** the
//!   `seq` is the constant 0 (viewers are anonymous, so there is nothing to
//!   count), so an input AAD binds *direction only* and every input blob in a
//!   session shares one AAD. Nothing in the sealed bytes distinguishes one
//!   input frame from another except its random nonce, which is why input
//!   replay protection is entirely receiver-side state — the never-forgetting
//!   ledger in `transport::AcceptedNonces` — and not a property of this
//!   module.

use aes_gcm::aead::rand_core::RngCore as _;
use aes_gcm::aead::{Aead as _, OsRng, Payload};
use aes_gcm::{AeadCore as _, Aes256Gcm, Key, KeyInit as _, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// AES-GCM nonce length, in bytes: the first [`NONCE_LEN`] bytes of a blob.
pub const NONCE_LEN: usize = 12;
/// GCM authentication tag length, in bytes: the last [`TAG_LEN`] bytes.
pub const TAG_LEN: usize = 16;
/// AES-256 key length, in bytes.
pub const KEY_LEN: usize = 32;
/// AAD length, in bytes: frame-kind byte plus big-endian `u64` seq.
pub const AAD_LEN: usize = 9;

/// The frame domain a blob is sealed for; the first AAD byte.
///
/// These byte values are **wire-stable**: the viewer's `crypto.js` builds the
/// same AAD from the same constants, so renumbering them breaks every deployed
/// viewer. Extend, never reassign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    /// Host → viewer incremental terminal output.
    Output = 0x01,
    /// Host → viewer keyframe (full-screen replay bytes).
    Keyframe = 0x02,
    /// Viewer → host write-mode input.
    Input = 0x03,
}

/// Build the 9-byte AAD binding a blob to its frame kind and envelope `seq`:
/// `kind(1) || seq(u64 BE)`.
///
/// Host→viewer frames use the envelope `seq` the hub orders and replays on;
/// viewer input uses the constant `seq` 0 (viewers are anonymous, so no
/// per-sender counter exists). Input replay is handled on the host by an
/// exact-nonce ledger that **never forgets** — bounded at `INPUT_NONCE_CAP`
/// accepted frames per process, after which input is refused (fail closed).
/// Dedup prevents *duplication*; it does **not** prevent the hub from dropping
/// or reordering input, which remain open. See `transport::AcceptedNonces`.
#[must_use]
pub fn frame_aad(kind: FrameKind, seq: u64) -> [u8; AAD_LEN] {
    let mut aad = [0u8; AAD_LEN];
    aad[0] = kind as u8;
    aad[1..].copy_from_slice(&seq.to_be_bytes());
    aad
}

/// Failures in the sealed-blob layer.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    /// A key fragment did not decode to exactly [`KEY_LEN`] bytes.
    #[error("invalid session key")]
    InvalidKey,

    /// A blob was shorter than the nonce-plus-tag overhead; carries the
    /// received length.
    #[error("encrypted blob too short: {0} bytes")]
    BlobTooShort(usize),

    /// Authenticated decryption failed. Deliberately detail-free: a tampered
    /// tag, a wrong key, and a wrong AAD are indistinguishable by design.
    #[error("decryption failed")]
    DecryptFailed,
}

/// The per-session AES-256 key. Zeroized on drop; `Debug` never prints key
/// material.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SessionKey([u8; KEY_LEN]);

impl std::fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionKey(<redacted>)")
    }
}

impl SessionKey {
    /// Generate a fresh random key from the OS RNG. Called once per session.
    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0u8; KEY_LEN];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Wrap existing key bytes (the viewer-side / test-side constructor).
    #[must_use]
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// Encode the key for the URL fragment: unpadded base64url, always exactly
    /// 43 chars of `[A-Za-z0-9_-]`. The fragment never leaves the client — it
    /// is appended locally to the join URL and browsers never send it in HTTP
    /// requests.
    #[must_use]
    pub fn to_fragment(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }

    /// Decode a URL fragment back into a key.
    ///
    /// # Errors
    ///
    /// [`CryptoError::InvalidKey`] if the fragment is not unpadded base64url
    /// or does not decode to exactly [`KEY_LEN`] bytes.
    pub fn from_fragment(fragment: &str) -> Result<Self, CryptoError> {
        let mut decoded = URL_SAFE_NO_PAD.decode(fragment).map_err(|_| CryptoError::InvalidKey)?;
        if decoded.len() != KEY_LEN {
            decoded.zeroize();
            return Err(CryptoError::InvalidKey);
        }
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(&decoded);
        decoded.zeroize();
        Ok(Self(bytes))
    }

    /// Seal `plaintext` under `aad` with a fresh random nonce, returning the
    /// full wire blob `nonce || ciphertext || tag`.
    #[must_use]
    pub fn encrypt(&self, plaintext: &[u8], aad: &[u8; AAD_LEN]) -> Vec<u8> {
        let nonce: [u8; NONCE_LEN] = Aes256Gcm::generate_nonce(&mut OsRng).into();
        self.seal(&nonce, plaintext, aad)
    }

    /// Open a wire blob sealed by [`SessionKey::encrypt`] (or the viewer's
    /// `encryptBlob`), authenticating it against `aad`.
    ///
    /// # Errors
    ///
    /// [`CryptoError::BlobTooShort`] if `blob` cannot even hold the nonce and
    /// tag; [`CryptoError::DecryptFailed`] on any authentication failure.
    pub fn decrypt(&self, blob: &[u8], aad: &[u8; AAD_LEN]) -> Result<Vec<u8>, CryptoError> {
        if blob.len() < NONCE_LEN + TAG_LEN {
            return Err(CryptoError::BlobTooShort(blob.len()));
        }
        let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
        self.cipher()
            .decrypt(Nonce::from_slice(nonce), Payload {
                msg: ciphertext,
                aad,
            })
            .map_err(|_| CryptoError::DecryptFailed)
    }

    /// Deterministic sealing used by [`SessionKey::encrypt`] and, in tests, by
    /// `encrypt_with_nonce`. Every production nonce comes fresh
    /// from the OS RNG in `encrypt`.
    fn seal(&self, nonce: &[u8; NONCE_LEN], plaintext: &[u8], aad: &[u8; AAD_LEN]) -> Vec<u8> {
        let sealed = self
            .cipher()
            .encrypt(Nonce::from_slice(nonce), Payload {
                msg: plaintext,
                aad,
            })
            .expect("AES-GCM encryption is infallible for in-memory frame sizes");
        let mut blob = Vec::with_capacity(NONCE_LEN + sealed.len());
        blob.extend_from_slice(nonce);
        blob.extend_from_slice(&sealed);
        blob
    }

    /// Test-only deterministic encryption for the frozen interop vector.
    /// Returns the full blob `nonce || ciphertext || tag`.
    #[cfg(test)]
    fn encrypt_with_nonce(
        &self,
        nonce: &[u8; NONCE_LEN],
        plaintext: &[u8],
        aad: &[u8; AAD_LEN],
    ) -> Vec<u8> {
        self.seal(nonce, plaintext, aad)
    }

    /// The AEAD instance. Rebuilt per call — the key schedule is cheap next to
    /// a network frame, and keeping only the raw bytes as state means the
    /// derive-based zeroize-on-drop covers everything the key owns.
    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The frozen cross-implementation vector from the design spec. The viewer's
    // `crypto.js` (via the node interop harness) must reproduce these exact
    // bytes; change them and the two implementations can no longer prove they
    // agree.
    const VECTOR_KEY_HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    const VECTOR_NONCE_HEX: &str = "404142434445464748494a4b";
    const VECTOR_PLAINTEXT: &[u8] = b"atuin-share interop test vector\r\n";
    const VECTOR_CT_TAG_HEX: &str = "83cddb4a4811f46bacb67216f20a673e0ba9227cc3507c5757312b86e37c9fd6929b4ef72f0194e13a1674ed8686300ed4";

    fn vector_key() -> SessionKey {
        let bytes: [u8; KEY_LEN] =
            hex::decode(VECTOR_KEY_HEX).expect("valid hex").try_into().expect("32 bytes");
        SessionKey::from_bytes(bytes)
    }

    fn vector_blob() -> Vec<u8> {
        hex::decode(format!("{VECTOR_NONCE_HEX}{VECTOR_CT_TAG_HEX}")).expect("valid hex")
    }

    #[test]
    fn frozen_vector_encrypts_to_the_exact_bytes() {
        let nonce: [u8; NONCE_LEN] =
            hex::decode(VECTOR_NONCE_HEX).expect("valid hex").try_into().expect("12 bytes");
        let aad = frame_aad(FrameKind::Output, 42);
        assert_eq!(hex::encode(aad), "01000000000000002a");

        let blob = vector_key().encrypt_with_nonce(&nonce, VECTOR_PLAINTEXT, &aad);
        assert_eq!(hex::encode(&blob[..NONCE_LEN]), VECTOR_NONCE_HEX);
        assert_eq!(hex::encode(&blob[NONCE_LEN..]), VECTOR_CT_TAG_HEX);
    }

    #[test]
    fn frozen_vector_decrypts_and_rejects_the_wrong_aad() {
        let key = vector_key();
        let blob = vector_blob();
        let plaintext = key
            .decrypt(&blob, &frame_aad(FrameKind::Output, 42))
            .expect("the frozen vector decrypts");
        assert_eq!(plaintext, VECTOR_PLAINTEXT);

        // Same seq, Input tag (`03…2a`): type binding must reject reflection.
        assert_eq!(
            key.decrypt(&blob, &frame_aad(FrameKind::Input, 42)),
            Err(CryptoError::DecryptFailed)
        );
        // Same kind, wrong seq: seq binding must reject renumbering.
        assert_eq!(
            key.decrypt(&blob, &frame_aad(FrameKind::Output, 43)),
            Err(CryptoError::DecryptFailed)
        );
    }

    #[test]
    fn random_key_round_trips_every_frame_kind() {
        let key = SessionKey::generate();
        for kind in [FrameKind::Output, FrameKind::Keyframe, FrameKind::Input] {
            let aad = frame_aad(kind, 7);
            let blob = key.encrypt(b"round trip", &aad);
            assert_eq!(blob.len(), b"round trip".len() + NONCE_LEN + TAG_LEN);
            assert_eq!(key.decrypt(&blob, &aad).expect("round-trips"), b"round trip");
        }
        // Empty plaintext is legal: the blob is exactly the 28-byte overhead.
        let aad = frame_aad(FrameKind::Output, 8);
        let blob = key.encrypt(b"", &aad);
        assert_eq!(blob.len(), NONCE_LEN + TAG_LEN);
        assert!(key.decrypt(&blob, &aad).expect("round-trips").is_empty());
    }

    #[test]
    fn tampered_tag_fails_decryption() {
        let key = SessionKey::generate();
        let aad = frame_aad(FrameKind::Keyframe, 3);
        let mut blob = key.encrypt(b"payload", &aad);
        let last = blob.len() - 1;
        blob[last] ^= 0x01;
        assert_eq!(key.decrypt(&blob, &aad), Err(CryptoError::DecryptFailed));
    }

    #[test]
    fn blob_shorter_than_the_overhead_is_rejected_before_decryption() {
        let key = SessionKey::generate();
        let blob = [0u8; NONCE_LEN + TAG_LEN - 1];
        assert_eq!(
            key.decrypt(&blob, &frame_aad(FrameKind::Output, 0)),
            Err(CryptoError::BlobTooShort(27))
        );
        assert_eq!(
            key.decrypt(&[], &frame_aad(FrameKind::Output, 0)),
            Err(CryptoError::BlobTooShort(0))
        );
    }

    #[test]
    fn fragment_is_43_urlsafe_chars_and_round_trips() {
        let key = SessionKey::generate();
        let fragment = key.to_fragment();
        assert_eq!(fragment.len(), 43);
        assert!(
            fragment.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "fragment must match ^[A-Za-z0-9_-]{{43}}$, got {fragment:?}"
        );

        let restored = SessionKey::from_fragment(&fragment).expect("round-trips");
        assert_eq!(restored.to_fragment(), fragment);
        // And it is functionally the same key: what one seals, the other opens.
        let aad = frame_aad(FrameKind::Output, 5);
        let blob = restored.encrypt(b"same key", &aad);
        assert_eq!(key.decrypt(&blob, &aad).expect("same key"), b"same key");
    }

    #[test]
    fn from_fragment_rejects_wrong_lengths_and_padded_input() {
        // 42 chars decode to 31 bytes, 44 to 33 — both are not a key.
        assert!(matches!(SessionKey::from_fragment(&"A".repeat(42)), Err(CryptoError::InvalidKey)));
        assert!(matches!(SessionKey::from_fragment(&"A".repeat(44)), Err(CryptoError::InvalidKey)));
        // `=` is not in the no-pad alphabet at all.
        assert!(matches!(
            SessionKey::from_fragment(&format!("{}=", "A".repeat(43))),
            Err(CryptoError::InvalidKey)
        ));
        assert!(matches!(SessionKey::from_fragment(""), Err(CryptoError::InvalidKey)));
    }

    #[test]
    fn debug_never_prints_key_bytes() {
        let key = vector_key();
        assert_eq!(format!("{key:?}"), "SessionKey(<redacted>)");
        assert_eq!(format!("{key:#?}"), "SessionKey(<redacted>)");
    }

    /// Not a behavior test: an emitter for the cross-implementation harness.
    /// Under the fixed vector key, writes the hex of a freshly-encrypted
    /// keyframe blob to the path in `INTEROP_OUT`; the node harness decrypts
    /// it through the shipped viewer `crypto.js`. Skips cleanly when the env
    /// var is unset.
    #[test]
    #[ignore = "interop emitter; run explicitly with INTEROP_OUT=<path>"]
    fn emit_interop_blob() {
        let Ok(path) = std::env::var("INTEROP_OUT") else {
            return;
        };
        let blob = vector_key()
            .encrypt(b"rust-produced blob: hello viewer\n", &frame_aad(FrameKind::Keyframe, 111));
        std::fs::write(&path, hex::encode(blob)).expect("write INTEROP_OUT");
    }
}

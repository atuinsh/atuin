//! Encryption of atuin records and packfile bodies.
//!
//! Every value is envelope-encrypted: a fresh 32-byte content key (CEK) seals the payload, and the
//! CEK is wrapped under the master [`Key`]. A value travels as two strings, [`EncryptedData::raw`]
//! (the payload) and [`EncryptedData::cek`] (the wrap), and the two parse independently. Readers
//! accept both envelope formats; [`EnvelopeFormat::WRITE`] selects the writer.
//!
//! # Formats
//!
//! **Legacy** is a PASETO `v4.local.` token over `{"data": base64url(plaintext)}`, bound to the
//! associated data as its implicit assertion, with the CEK as the JSON
//! `{"wpk":"k4.local-wrap.pie.…","kid":"k4.lid.…"}`. PASETO only takes strings, hence the base64 and
//! the JSON.
//!
//! **v5** seals raw bytes with a committing XChaCha20-Poly1305: Bellare and Hoang's
//! HtE\[UtC\[F, XChaCha20-Poly1305\], H\] ("Efficient Schemes for Committing Authenticated
//! Encryption", EUROCRYPT 2022), with keyed BLAKE2b-256 as both `H` and `F`. For a 32-byte key `K`,
//! a domain `d`, a random 24-byte nonce `N` and associated data `A`:
//!
//! ```text
//! L      = H(K, "atuin-envelope-v5" || 0x01 || d || N || A)
//! P      = H(L, "atuin-envelope-v5" || 0x02 || d || N)      the key commitment
//! Ke     = H(L, "atuin-envelope-v5" || 0x03 || d || N)      the XChaCha20-Poly1305 key
//! C || T = XChaCha20-Poly1305(Ke, N, ad = "", M)
//! sealed = N (24) || P (32) || C || T (16)
//! ```
//!
//! `d` is `0x01` for a record payload, `0x02` for a CEK wrap and `0x03` for a packfile body. An
//! open recomputes `P` and compares it in constant time before Poly1305 runs, so a wrong key,
//! associated data or domain never reaches the AEAD.
//!
//! ```text
//! payload blob = 0x05 || sealed(K = CEK, A = the record's or packfile's JSON, M = plaintext)
//! wrap blob    = 0x01 || kid (33) || sealed(K = master key, d = 0x02, A = 0x01 || kid, M = CEK)
//! data         = "v5." || base64url(payload blob)
//! cek          = "v5." || base64url(wrap blob)
//! packfile     = 0x05 || u16be(wrap blob length) || wrap blob || payload blob
//! ```
//!
//! The wrap blob carries its own type byte, so another kind of wrap (a KMS, an asymmetric key) is a
//! new type under the same `v5.` container with no new payload version. A `vN.` prefix with `N` in
//! `6..=255` in either field is a format a newer atuin wrote ([`UnsupportedFormat`]); anything
//! else that is neither legacy nor `v5.` is [`MalformedError`]. A value opens in this order: parse
//! the wrap, compare its key id with the key's ([`WrongKey`]), parse the payload, unwrap the CEK,
//! open the payload. So a value under another key is [`WrongKey`] whatever its payload, and
//! [`EncryptedData::key_id`] and [`EncryptedData::rewrap`] never parse the payload at all.
//!
//! # What v5 does not claim
//!
//! - Ciphertext integrity rests on Poly1305's 16-byte tag (legacy had a 32-byte BLAKE2b MAC), and
//!   `P` commits to the key, nonce and associated data, not to `C`.
//! - Wraps under one master key share their associated data, so a wrap-nonce collision repeats
//!   `Ke`. With random 24-byte nonces, `2^80` wraps collide with probability about `2^-33`; legacy
//!   PIE wraps had a `2^112` budget.
//! - Our own copies of the CEK, `L` and `Ke` are wiped on drop, as is `chacha20poly1305`'s key copy.
//!   The Poly1305 state, BLAKE2b's keyed state and its finalize temporary, and the `k4.lid`
//!   hasher's buffer are not; legacy has the same BLAKE2b gap. Legacy also leaves `rusty_paserk`'s
//!   copies of the master key and the CEK, and the subkeys both rusty crates derive, unwiped.
//!
//! # Why a random content-encryption key?
//!
//! Design rationale, originally written by `@conradludgate`:
//!
//! > Originally I was planning on using a derived key for encryption based on additional data.
//! > This would be a lot more secure than using the master key directly.
//! >
//! > However, there's an established norm of using a random key. This scheme might be otherwise
//! > known as:
//! > - client-side encryption
//! > - envelope encryption
//! > - key wrapping
//! >
//! > A HSM (Hardware Security Module) provider, eg: AWS, Azure, GCP, or even a physical device
//! > like a YubiKey will have some keys that they keep to themselves. These keys never leave
//! > their physical hardware. If they never leave the hardware, then encrypting large amounts
//! > of data means giving them the data and waiting. This is not a practical solution. Instead,
//! > generate a unique key for your data, encrypt that using your HSM and then store that with
//! > your data.
//! >
//! > See
//! >  - <https://docs.aws.amazon.com/wellarchitected/latest/financial-services-industry-lens/use-envelope-encryption-with-customer-master-keys.html>
//! >  - <https://cloud.google.com/kms/docs/envelope-encryption>
//! >  - <https://learn.microsoft.com/en-us/azure/storage/blobs/client-side-encryption?tabs=dotnet#encryption-and-decryption-via-the-envelope-technique>
//! >  - <https://www.yubico.com/products/hardware-security-module/>
//! >  - <https://cheatsheetseries.owasp.org/cheatsheets/Cryptographic_Storage_Cheat_Sheet.html#encrypting-stored-keys>
//! >
//! > Why would we care? In the past we have received some requests for company solutions. If in
//! > future we can configure a KMS service with little effort, then that would solve a lot of
//! > issues for their security team.
//! >
//! > Even for personal use, if a user is not comfortable with sharing keys between hosts,
//! > GCP HSM costs $1/month and $0.03 per 10,000 key operations. Assuming an active user runs
//! > 1000 atuin records a day, that would only cost them $1 and 10 cent a month.
//! >
//! > Additionally, key rotations are much simpler using this scheme. Rotating a key is as
//! > simple as re-encrypting the CEK, and not the message contents. This makes it very fast to
//! > rotate a key in bulk.
//! >
//! > For future reference, with asymmetric encryption, you can encrypt the CEK without the
//! > HSM's involvement, but decrypting will need the HSM. This allows the encryption path to
//! > still be extremely fast (no network calls) but downloads/decryption that happens in the
//! > background can make the network calls to the HSM
mod aead;
mod envelope;
mod error;
mod key;
mod key_id;
mod legacy;
mod packfile;
pub mod paseto_v4;
mod storage;
mod v5;

pub use envelope::EncryptedData;
pub use error::{
    AuthenticationError, Layer, LegacyCause, MalformedError, OpenError, ParseError, RewrapError,
    SealError, ShapeError, UnsupportedFormat, WrongKey,
};
pub use key::{
    Key, KeyDecodingError, KeyFileLoadOrGenerateError, KeyFileLoadingError, KeyFileStoringError,
    MnemonicLoadingError, PlainTextEncodedKey,
};
pub use key_id::{KeyId, KeyIdParseError};
pub use packfile::PackfileBody;
pub use storage::StoredBlobs;

/// An envelope format this build can seal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeFormat {
    /// PASETO v4.local payload with a PASERK `k4.local-wrap.pie` content key.
    Legacy,
    /// The committing XChaCha20-Poly1305 envelope of this module's docs.
    V5,
}

impl EnvelopeFormat {
    /// The format every writer seals in; **be warned**: atuin releases from before this enum existed cannot read [`EnvelopeFormat::V5`].
    pub const WRITE: Self = Self::Legacy;
}

#[cfg(test)]
mod tests {
    //! Fixtures and frozen inputs shared by every test module of `encryption`.
    use rstest::fixture;

    use super::Key;
    use super::envelope::Cek;
    use super::v5::Draws;

    /// The golden record's `AdditionalData` JSON, which the v5 KATs reuse.
    pub(super) const KAT_AAD: &str = r#"{"id":"00000000-0000-0000-0000-000000000001","idx":7,"version":"v1","tag":"history","host":"00000000-0000-0000-0000-000000000002"}"#;

    /// A frozen `PackIA` JSON.
    pub(super) const KAT_PACK_AAD: &str = r#"{"manifest_id":"00000000-0000-0000-0000-000000000003","manifest_idx":4,"manifest_version":"v1","host":"00000000-0000-0000-0000-000000000002","tag":"packfile"}"#;

    pub(super) const KAT_PLAINTEXT: [u8; 5] = [1, 2, 3, 4, 5];

    /// Sealed by atuin's PASETO envelope (`paseto_v4::encrypt_sync`) over [`KAT_PLAINTEXT`].
    pub(super) const GOLDEN_RAW: &str = "v4.local.cSFhI9n30MfwkZrRAt-YAoxp6DrAMMybmLury7svdFMkapmxQmLQaRzqCfIdanPaQ55VbJjGjqwjst2AnLiBQE9cAQAyH69u2HVHrkaKv7rGtQ";
    pub(super) const GOLDEN_CEK: &str = r#"{"wpk":"k4.local-wrap.pie.8xXPgrNyliEUy_PnbM3S88Yk8tQQA0HN2o6jyUkGHK5duUEfW-zSCI1kSYRpyPESCK7-5822hPzRAbyZXPRVAbCkoLqqwPJ8_oi8clKEr6u8nJuIQQLHVClvYJmZyZIu","kid":"k4.lid.2LzCmxDtbwu2tK5T1X1VLEth8umaI9vbKgTDkkt7ARR0"}"#;

    /// `seal_v5(key(), KAT_AAD, KAT_PLAINTEXT, kat_draws())`.
    pub(super) const KAT_WIRE_DATA: &str = "v5.BUBBQkNERUZHSElKS0xNTk9QUVJTVFVWVwAhrSRi6F-28hqzJzS8IQdRbIAr9c_2iFeGmtX_Tmn5VYPddjwuEmfse_VoF4tdz1qfQC_g";
    pub(super) const KAT_WIRE_CEK: &str = "v5.Adi8wpsQ7W8LtrSuU9V9VSxLYfLpmiPb2yoEw5JLewEUdICBgoOEhYaHiImKi4yNjo-QkZKTlJWWl6COGGuyTR_LwA1ClzlBheoMUkyb1sVbdOd4JSE0bGRqDM5AqfPCUYKSzP28y7G4u153TxLRhIWzNQxXQRCMXUC-64OVdUyIEsId3LmiBhy7";

    #[fixture]
    pub(super) fn key() -> Key {
        Key::from([0x55; 32])
    }

    #[fixture]
    pub(super) fn other_key() -> Key {
        Key::from([0x56; 32])
    }

    #[fixture]
    pub(super) fn kat_cek() -> Cek {
        Cek::from(std::array::from_fn(|i| u8::try_from(i).expect("below 32")))
    }

    #[fixture]
    pub(super) fn kat_draws(kat_cek: Cek) -> Draws {
        Draws {
            cek: kat_cek,
            payload_nonce: std::array::from_fn(|i| 0x40 + u8::try_from(i).expect("below 24")),
            wrap_nonce: std::array::from_fn(|i| 0x80 + u8::try_from(i).expect("below 24")),
        }
    }
}

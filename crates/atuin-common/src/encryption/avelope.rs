use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use zeroize::Zeroizing;

use super::aead::{CommittingAead, Domain};
use super::bytes::EncryptedBytes;
use super::envelope::{Cek, KidMatched};
use super::error::{AuthenticationError, ParseError, SealError, ShapeError, UnsupportedFormat};
use super::{EncryptedData, Key, KeyId, Layer};

/// An avelope payload blob: `0x01 || nonce || P || C || T`.
pub(super) struct AvelopePayload(Vec<u8>);

impl AvelopePayload {
    pub(super) const VERSION: u8 = 0x01;
    pub(super) const MIN_LEN: usize = 1 + CommittingAead::OVERHEAD;
    /// After the version byte, the nonce and the commitment.
    const PLAINTEXT_START: usize = 1 + CommittingAead::CIPHERTEXT_START;

    /// Take a blob whose first byte is [`Self::VERSION`]; any other first byte is a newer version.
    pub(super) fn parse(blob: Vec<u8>) -> Result<Self, ParseError> {
        let len = blob.len();
        match blob.first() {
            Some(&Self::VERSION) if len >= Self::MIN_LEN => Ok(Self(blob)),
            Some(&Self::VERSION) | None => Err(ShapeError::Length {
                layer: Layer::Payload,
                len,
            }
            .into()),
            Some(&version) => Err(UnsupportedFormat::Version {
                layer: Layer::Payload,
                version,
            }
            .into()),
        }
    }

    pub(super) fn seal(
        cek: &Cek,
        nonce: &[u8; 24],
        aad: &str,
        plaintext: &[u8],
    ) -> Result<Self, SealError> {
        let mut blob = Vec::with_capacity(Self::MIN_LEN + plaintext.len());
        blob.push(Self::VERSION);
        CommittingAead::new(cek.as_bytes(), Domain::Payload).seal_into(
            nonce,
            aad.as_bytes(),
            plaintext,
            &mut blob,
        )?;
        Ok(Self(blob))
    }

    /// Open with `cek`, decrypting in place.
    pub(super) fn open(mut self, cek: &Cek, aad: &str) -> Result<Vec<u8>, AuthenticationError> {
        CommittingAead::new(cek.as_bytes(), Domain::Payload)
            .open_in_place(aad.as_bytes(), &mut self.0[1..])
            .map_err(|err| err.at(Layer::Payload))?;
        self.0.truncate(self.0.len() - CommittingAead::TAG_LEN);
        self.0.drain(..Self::PLAINTEXT_START);
        Ok(self.0)
    }

    pub(super) fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// Wrap type `0x01`: the content key sealed under the master key, with `type || kid` as AAD.
pub(super) struct SymmetricWrap([u8; Self::LEN]);

impl SymmetricWrap {
    pub(super) const TYPE: u8 = 0x01;
    pub(super) const LEN: usize = Self::HEADER_LEN + CommittingAead::OVERHEAD + 32;
    const HEADER_LEN: usize = 1 + KeyId::LEN;

    /// Take a blob whose first byte is [`Self::TYPE`]; any length but [`Self::LEN`] is malformed.
    pub(super) fn parse(blob: &[u8]) -> Result<Self, ShapeError> {
        blob.try_into().map(Self).map_err(|_| ShapeError::Length {
            layer: Layer::Wrap,
            len: blob.len(),
        })
    }

    pub(super) fn key_id(&self) -> KeyId {
        KeyId::from_bytes(
            self.0[1..Self::HEADER_LEN].try_into().expect("the header is a type byte and a key id"),
        )
    }

    pub(super) fn seal(cek: &Cek, master: &Key, nonce: &[u8; 24]) -> Result<Self, SealError> {
        let mut header = [0; Self::HEADER_LEN];
        header[0] = Self::TYPE;
        header[1..].copy_from_slice(master.key_id().as_bytes());
        let mut blob = Vec::with_capacity(Self::LEN);
        blob.extend_from_slice(&header);
        CommittingAead::new(master.as_bytes(), Domain::CekWrap).seal_into(
            nonce,
            &header,
            cek.as_bytes(),
            &mut blob,
        )?;
        Ok(Self(blob.try_into().expect("a header and a sealed 32-byte key are LEN bytes")))
    }

    /// Unwrap the content key with `master`, whose kid `proof` says this wrap names.
    pub(super) fn unwrap_cek(
        &self,
        master: &Key,
        _proof: KidMatched,
    ) -> Result<Cek, AuthenticationError> {
        let (header, sealed) = self.0.split_at(Self::HEADER_LEN);
        let mut sealed: Zeroizing<[u8; Self::LEN - Self::HEADER_LEN]> =
            Zeroizing::new(sealed.try_into().expect("split at the header length"));
        CommittingAead::new(master.as_bytes(), Domain::CekWrap)
            .open_in_place(header, sealed.as_mut_slice())
            .map_err(|err| err.at(Layer::Wrap))?;
        let cek = &sealed[CommittingAead::CIPHERTEXT_START..sealed.len() - CommittingAead::TAG_LEN];
        Ok(Cek::from(<[u8; 32]>::try_from(cek).expect("a wrap seals exactly 32 bytes")))
    }

    pub(super) const fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }
}

const _: () = assert!(
    SymmetricWrap::LEN != EncryptedBytes::LEGACY_CEK_LEN,
    "an avelope wrap must never look like a packed legacy cek"
);

/// The random values one avelope seal consumes; the KATs fix them.
pub(super) struct Draws {
    pub(super) cek: Cek,
    pub(super) payload_nonce: [u8; 24],
    pub(super) wrap_nonce: [u8; 24],
}

impl Draws {
    pub(super) fn random() -> Result<Self, getrandom::Error> {
        let mut payload_nonce = [0; 24];
        getrandom::fill(&mut payload_nonce)?;
        let mut wrap_nonce = [0; 24];
        getrandom::fill(&mut wrap_nonce)?;
        Ok(Self {
            cek: Cek::random()?,
            payload_nonce,
            wrap_nonce,
        })
    }
}

impl EncryptedBytes {
    /// Seal as avelope blobs with `draws`.
    pub(super) fn seal_avelope(
        key: &Key,
        aad: &str,
        plaintext: &[u8],
        Draws {
            cek,
            payload_nonce,
            wrap_nonce,
        }: Draws,
    ) -> Result<Self, SealError> {
        let payload = AvelopePayload::seal(&cek, &payload_nonce, aad, plaintext)?;
        let wrap = SymmetricWrap::seal(&cek, key, &wrap_nonce)?;
        Ok(Self {
            data: payload.into_bytes(),
            cek: wrap.as_bytes().to_vec(),
        })
    }
}

impl EncryptedData {
    /// The text prefix of both avelope fields, before the unpadded base64url blob.
    pub(super) const AVELOPE_PREFIX: &'static str = "avelope.";

    /// The text form of an avelope payload or wrap blob.
    pub(super) fn avelope_text(blob: &[u8]) -> String {
        format!("{}{}", Self::AVELOPE_PREFIX, URL_SAFE_NO_PAD.encode(blob))
    }

    /// Seal as avelope text with `draws`.
    pub(super) fn seal_avelope(
        key: &Key,
        aad: &str,
        plaintext: &[u8],
        draws: Draws,
    ) -> Result<Self, SealError> {
        let EncryptedBytes { data, cek } =
            EncryptedBytes::seal_avelope(key, aad, plaintext, draws)?;
        Ok(Self {
            raw: Self::avelope_text(&data),
            cek: Self::avelope_text(&cek),
        })
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;
    use crate::encryption::error::{OpenError, UnsupportedFormat};
    use crate::encryption::tests::{
        KAT_AAD, KAT_PLAINTEXT, KAT_WIRE_CEK, KAT_WIRE_DATA, kat_draws, key,
    };

    /// Which check a flip in each byte range of the KAT payload and wrap blobs must fail.
    #[derive(Debug, Clone, Copy)]
    enum Field {
        PayloadVersion,
        PayloadNonceOrCommitment,
        PayloadCiphertextOrTag,
        WrapType,
        WrapKid,
        WrapNonceOrCommitment,
        WrapCiphertextOrTag,
    }

    impl Field {
        fn of(blob: Blob, offset: usize) -> Self {
            match (blob, offset) {
                (Blob::Payload, 0) => Self::PayloadVersion,
                (Blob::Payload, 1..57) => Self::PayloadNonceOrCommitment,
                (Blob::Payload, _) => Self::PayloadCiphertextOrTag,
                (Blob::Wrap, 0) => Self::WrapType,
                (Blob::Wrap, 1..34) => Self::WrapKid,
                (Blob::Wrap, 34..90) => Self::WrapNonceOrCommitment,
                (Blob::Wrap, _) => Self::WrapCiphertextOrTag,
            }
        }

        fn accepts(self, err: &OpenError) -> bool {
            match self {
                Self::PayloadVersion => matches!(
                    err,
                    OpenError::UnsupportedFormat(UnsupportedFormat::Version {
                        layer: Layer::Payload,
                        ..
                    })
                ),
                Self::PayloadNonceOrCommitment => matches!(
                    err,
                    OpenError::Authentication(AuthenticationError::Commitment(Layer::Payload))
                ),
                Self::PayloadCiphertextOrTag => matches!(
                    err,
                    OpenError::Authentication(AuthenticationError::Tag(Layer::Payload))
                ),
                Self::WrapType => {
                    matches!(err, OpenError::UnsupportedFormat(UnsupportedFormat::WrapType(_)))
                }
                Self::WrapKid => matches!(err, OpenError::WrongKey(_)),
                Self::WrapNonceOrCommitment => matches!(
                    err,
                    OpenError::Authentication(AuthenticationError::Commitment(Layer::Wrap))
                ),
                Self::WrapCiphertextOrTag => {
                    matches!(err, OpenError::Authentication(AuthenticationError::Tag(Layer::Wrap)))
                }
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum Blob {
        Payload,
        Wrap,
    }

    impl Blob {
        /// The KAT value with bit `bit` of byte `offset` of this blob flipped, re-encoded strictly.
        fn flipped(self, offset: usize, bit: u8) -> EncryptedData {
            let decode = |s: &str| {
                URL_SAFE_NO_PAD
                    .decode(s.strip_prefix(EncryptedData::AVELOPE_PREFIX).unwrap())
                    .unwrap()
            };
            let (mut payload, mut wrap) = (decode(KAT_WIRE_DATA), decode(KAT_WIRE_CEK));
            match self {
                Self::Payload => payload[offset] ^= 1 << bit,
                Self::Wrap => wrap[offset] ^= 1 << bit,
            }
            EncryptedData {
                raw: EncryptedData::avelope_text(&payload),
                cek: EncryptedData::avelope_text(&wrap),
            }
        }

        const fn len(self) -> usize {
            match self {
                Self::Payload => AvelopePayload::MIN_LEN + KAT_PLAINTEXT.len(),
                Self::Wrap => SymmetricWrap::LEN,
            }
        }
    }

    #[rstest]
    fn seals_the_record_kat(key: Key, kat_draws: Draws) {
        let sealed = EncryptedData::seal_avelope(&key, KAT_AAD, &KAT_PLAINTEXT, kat_draws).unwrap();

        assert_eq!(sealed.raw, KAT_WIRE_DATA);
        assert_eq!(sealed.cek, KAT_WIRE_CEK);
        assert_eq!(sealed.open(&key, KAT_AAD).unwrap(), KAT_PLAINTEXT);
    }

    #[rstest]
    #[case::payload_version(Blob::Payload, 0)]
    #[case::payload_nonce(Blob::Payload, 1)]
    #[case::payload_commitment(Blob::Payload, 56)]
    #[case::payload_ciphertext(Blob::Payload, 57)]
    #[case::payload_tag(Blob::Payload, 77)]
    #[case::wrap_type(Blob::Wrap, 0)]
    #[case::wrap_kid(Blob::Wrap, 33)]
    #[case::wrap_nonce(Blob::Wrap, 34)]
    #[case::wrap_commitment(Blob::Wrap, 89)]
    #[case::wrap_ciphertext(Blob::Wrap, 90)]
    #[case::wrap_tag(Blob::Wrap, 137)]
    fn a_flipped_bit_fails_its_field_check(key: Key, #[case] blob: Blob, #[case] offset: usize) {
        let err = blob.flipped(offset, 0).open(&key, KAT_AAD).unwrap_err();

        assert!(Field::of(blob, offset).accepts(&err), "{err:?}");
    }

    #[rstest]
    fn every_flipped_bit_fails_its_field_check(
        key: Key,
        #[values(Blob::Payload, Blob::Wrap)] blob: Blob,
    ) {
        proptest!(|(offset in 0..blob.len(), bit in 0..8u8)| {
            let err = blob.flipped(offset, bit).open(&key, KAT_AAD).unwrap_err();
            prop_assert!(Field::of(blob, offset).accepts(&err), "{err:?}");
        });
    }

    #[rstest]
    fn a_flipped_aad_bit_fails_the_payload_commitment(key: Key) {
        let value = EncryptedData {
            raw: KAT_WIRE_DATA.to_owned(),
            cek: KAT_WIRE_CEK.to_owned(),
        };
        // Bit 7 of an ASCII byte would make the AAD invalid UTF-8, which `open` cannot take.
        proptest!(|(offset in 0..KAT_AAD.len(), bit in 0..7u8)| {
            let mut aad = KAT_AAD.as_bytes().to_vec();
            aad[offset] ^= 1 << bit;
            let aad = String::from_utf8(aad).unwrap();

            let err = value.open(&key, &aad).unwrap_err();
            prop_assert!(
                matches!(err, OpenError::Authentication(AuthenticationError::Commitment(Layer::Payload))),
                "{err:?}"
            );
        });
    }

    #[rstest]
    #[case::empty(vec![], |r: &Result<(), ParseError>| matches!(r, Err(ParseError::Malformed(ShapeError::Length { layer: Layer::Payload, len: 0 }))))]
    #[case::version_only(vec![0x01], |r: &Result<(), ParseError>| matches!(r, Err(ParseError::Malformed(ShapeError::Length { layer: Layer::Payload, len: 1 }))))]
    #[case::one_short(vec![0x01; 72], |r: &Result<(), ParseError>| matches!(r, Err(ParseError::Malformed(ShapeError::Length { layer: Layer::Payload, len: 72 }))))]
    #[case::shortest(vec![0x01; 73], |r: &Result<(), ParseError>| r.is_ok())]
    #[case::newer_version(vec![0x02; 73], |r: &Result<(), ParseError>| matches!(r, Err(ParseError::UnsupportedFormat(UnsupportedFormat::Version { layer: Layer::Payload, version: 2 }))))]
    fn parses_a_payload_blob(
        #[case] blob: Vec<u8>,
        #[case] expected: fn(&Result<(), ParseError>) -> bool,
    ) {
        let parsed = AvelopePayload::parse(blob).map(drop);

        assert!(expected(&parsed), "{parsed:?}");
    }
}

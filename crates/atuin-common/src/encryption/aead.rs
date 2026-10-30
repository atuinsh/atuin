use blake2::Blake2bMac;
use blake2::digest::Mac;
use blake2::digest::consts::U32;
use chacha20poly1305::{AeadInPlace as _, KeyInit as _, Tag, XChaCha20Poly1305, XNonce};
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

use super::error::{AuthenticationError, Layer, SealError};

/// What a sealed value is for; bound into every derivation so one cannot pass for another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Domain {
    Payload = 0x01,
    CekWrap = 0x02,
}

/// Why a sealed value did not open, before the caller says which layer it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AeadOpenError {
    Short {
        len: usize,
    },
    Commitment,
    Tag,
}

impl AeadOpenError {
    /// This failure at `layer`, for a caller that opens only length-checked blobs: `Short` is a bug.
    pub(super) fn at(self, layer: Layer) -> AuthenticationError {
        match self {
            Self::Short { .. } => {
                unreachable!(
                    "an AvelopePayload or SymmetricWrap is never shorter than the overhead"
                )
            }
            Self::Commitment => AuthenticationError::Commitment(layer),
            Self::Tag => AuthenticationError::Tag(layer),
        }
    }
}

/// XChaCha20-Poly1305 refused a message over its 256 GiB limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MessageTooLong {
    pub(super) len: usize,
}

impl From<MessageTooLong> for SealError {
    fn from(MessageTooLong { len }: MessageTooLong) -> Self {
        Self::TooLong { len }
    }
}

/// Which derivation a BLAKE2b input belongs to.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
enum Role {
    Subkey = 0x01,
    Commitment = 0x02,
    AeadKey = 0x03,
}

/// HtE[UtC[F, XChaCha20-Poly1305], H] over keyed BLAKE2b-256 (Bellare-Hoang 2022): a CMT-4 committing AEAD.
pub(super) struct CommittingAead<'k> {
    key: &'k [u8; 32],
    domain: Domain,
}

impl<'k> CommittingAead<'k> {
    pub(super) const NONCE_LEN: usize = 24;
    const COMMITMENT_LEN: usize = 32;
    pub(super) const TAG_LEN: usize = 16;
    /// Where the ciphertext starts: after the nonce and the commitment.
    pub(super) const CIPHERTEXT_START: usize = Self::NONCE_LEN + Self::COMMITMENT_LEN;
    /// Nonce, commitment and tag.
    pub(super) const OVERHEAD: usize = Self::CIPHERTEXT_START + Self::TAG_LEN;
    const LABEL: &'static [u8; 9] = b"avelope-1";

    pub(super) const fn new(key: &'k [u8; 32], domain: Domain) -> Self {
        Self { key, domain }
    }

    /// Append `nonce || P || C || T` for `plaintext` to `out`.
    pub(super) fn seal_into(
        &self,
        nonce: &[u8; 24],
        aad: &[u8],
        plaintext: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), MessageTooLong> {
        let (commitment, key) = self.derive_seal(nonce, aad);
        let start = out.len();
        // Reserved up front: growing `out` while it holds plaintext would leave a copy of it in the
        // freed buffer.
        out.reserve_exact(Self::OVERHEAD + plaintext.len());
        out.extend_from_slice(nonce);
        out.extend_from_slice(&commitment.0);
        out.extend_from_slice(plaintext);
        let sealed = key.cipher().encrypt_in_place_detached(
            XNonce::from_slice(nonce),
            b"",
            &mut out[start + Self::CIPHERTEXT_START..],
        );
        // The only encryption error is a message over the limit (chacha20poly1305 0.10.1,
        // `cipher.rs:55`), checked before any byte is touched.
        let Ok(tag) = sealed else {
            out.truncate(start);
            return Err(MessageTooLong {
                len: plaintext.len(),
            });
        };
        out.extend_from_slice(&tag);
        Ok(())
    }

    /// Open `sealed` in place, checking the commitment first; the plaintext is then `sealed[56..len - 16]`.
    pub(super) fn open_in_place(&self, aad: &[u8], sealed: &mut [u8]) -> Result<(), AeadOpenError> {
        let len = sealed.len();
        if len < Self::OVERHEAD {
            return Err(AeadOpenError::Short { len });
        }
        let (head, body) = sealed.split_at_mut(Self::CIPHERTEXT_START);
        let (nonce, claimed) = head.split_at(Self::NONCE_LEN);
        let nonce: &[u8; 24] = nonce.try_into().expect("split at the nonce length");
        let (ciphertext, tag) = body.split_at_mut(body.len() - Self::TAG_LEN);

        let key =
            self.derive_open(nonce, aad).verified(claimed).ok_or(AeadOpenError::Commitment)?;
        key.cipher()
            .decrypt_in_place_detached(
                XNonce::from_slice(nonce),
                b"",
                ciphertext,
                Tag::from_slice(tag),
            )
            .map_err(|_| AeadOpenError::Tag)
    }

    /// BLAKE2b-256 keyed with `key` over `LABEL || role || domain || nonce || tail`.
    fn mac(
        key: &[u8; 32],
        role: Role,
        domain: Domain,
        nonce: &[u8; 24],
        tail: &[u8],
    ) -> Zeroizing<[u8; 32]> {
        // `Mac::new_from_slice`, never `KeyInit::new`: `Blake2bMac`'s `KeySize` is 64, so `new`
        // would key with 64 bytes and change every output.
        let mut mac = <Blake2bMac<U32> as Mac>::new_from_slice(key)
            .expect("a 32-byte key is within BLAKE2b's 64-byte limit");
        mac.update(Self::LABEL);
        mac.update(&[role as u8, domain as u8]);
        mac.update(nonce);
        mac.update(tail);
        Zeroizing::new(mac.finalize().into_bytes().into())
    }

    /// `L` and `P` for an open; the AEAD key waits behind [`Unverified::verified`].
    fn derive_open(&self, nonce: &[u8; 24], aad: &[u8]) -> Unverified {
        let subkey = Self::mac(self.key, Role::Subkey, self.domain, nonce, aad);
        let commitment = Commitment(*Self::mac(&subkey, Role::Commitment, self.domain, nonce, &[]));
        Unverified {
            subkey,
            commitment,
            domain: self.domain,
            nonce: *nonce,
        }
    }

    /// The commitment to store and the AEAD key to seal with.
    fn derive_seal(&self, nonce: &[u8; 24], aad: &[u8]) -> (Commitment, AeadKey) {
        let Unverified {
            subkey, commitment, ..
        } = self.derive_open(nonce, aad);
        (commitment, AeadKey::derive(&subkey, self.domain, nonce))
    }
}

/// HtE's subkey and UtC's commitment for one open, before the stored commitment is checked.
struct Unverified {
    subkey: Zeroizing<[u8; 32]>,
    commitment: Commitment,
    domain: Domain,
    nonce: [u8; 24],
}

impl Unverified {
    /// The AEAD key, if `claimed` is this derivation's commitment (constant time).
    fn verified(self, claimed: &[u8]) -> Option<AeadKey> {
        bool::from(self.commitment.0.as_slice().ct_eq(claimed))
            .then(|| AeadKey::derive(&self.subkey, self.domain, &self.nonce))
    }
}

/// A key commitment; deliberately without `PartialEq`, so it is compared only in constant time.
struct Commitment([u8; 32]);

/// UtC's XChaCha20-Poly1305 key, wiped on drop.
struct AeadKey(Zeroizing<[u8; 32]>);

impl AeadKey {
    fn derive(subkey: &[u8; 32], domain: Domain, nonce: &[u8; 24]) -> Self {
        Self(CommittingAead::mac(subkey, Role::AeadKey, domain, nonce, &[]))
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&*self.0).into())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;
    use crate::encryption::tests::{KAT_AAD, KAT_PLAINTEXT};

    const PRIM_KEY: [u8; 32] = [0xa5; 32];
    const PRIM_NONCE: [u8; 24] = [0xc3; 24];

    impl CommittingAead<'_> {
        fn sealed(&self, nonce: &[u8; 24], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            self.seal_into(nonce, aad, plaintext, &mut out).unwrap();
            out
        }
    }

    #[rstest]
    #[case::empty_message(
        "a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebf",
        Domain::Payload,
        "c0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7",
        Vec::new(),
        "",
        "c0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d767743964e8d0c202f4543e797fc0d0ce090f882f8d1c71bc26dc52300a3fba1a280121f7af514395006135761723b4a4"
    )]
    #[case::payload(
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        Domain::Payload,
        "404142434445464748494a4b4c4d4e4f5051525354555657",
        KAT_AAD.as_bytes().to_vec(),
        "0102030405",
        "404142434445464748494a4b4c4d4e4f505152535455565735d7831a44d6a272a43e69e138fa6764228c4707ae067fff63152bb3032de44424aaeb22636f5497b8e626373df74ed17c419aa9d1"
    )]
    #[case::wrap(
        "5555555555555555555555555555555555555555555555555555555555555555",
        Domain::CekWrap,
        "808182838485868788898a8b8c8d8e8f9091929394959697",
        hex::decode("01d8bcc29b10ed6f0bb6b4ae53d57d552c4b61f2e99a23dbdb2a04c3924b7b011474").unwrap(),
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        "808182838485868788898a8b8c8d8e8f9091929394959697308155ce26222c27a999c2261ecad62ea743f5990249099e51e0288489e09a0735c286c06b86b8c797bdfa113bea6a6bfccf4c8f73a25ca84de09f1d8743c4cd89f2ecf36ea8e009467c95438409d78a"
    )]
    fn seals_and_opens_the_kats(
        #[case] key: &str,
        #[case] domain: Domain,
        #[case] nonce: &str,
        #[case] aad: Vec<u8>,
        #[case] plaintext: &str,
        #[case] expected: &str,
    ) {
        let key: [u8; 32] = hex::decode(key).unwrap().try_into().unwrap();
        let nonce: [u8; 24] = hex::decode(nonce).unwrap().try_into().unwrap();
        let plaintext = hex::decode(plaintext).unwrap();
        let aead = CommittingAead::new(&key, domain);

        let mut sealed = aead.sealed(&nonce, &aad, &plaintext);
        assert_eq!(hex::encode(&sealed), expected);

        aead.open_in_place(&aad, &mut sealed).unwrap();
        assert_eq!(sealed[CommittingAead::CIPHERTEXT_START..sealed.len() - 16], plaintext);
    }

    #[rstest]
    #[case::payload(
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        Domain::Payload,
        "404142434445464748494a4b4c4d4e4f5051525354555657",
        KAT_AAD.as_bytes().to_vec(),
        "2fcf0a95159afd259f09f1146f67c2e64f637963f515eacac4e1cb3e0ec7347f",
        "35d7831a44d6a272a43e69e138fa6764228c4707ae067fff63152bb3032de444",
        "7ed7231ad0d2409cdbba585c7d4e11678dd08635dbfbca7b31c38d71dc6e1713"
    )]
    #[case::wrap(
        "5555555555555555555555555555555555555555555555555555555555555555",
        Domain::CekWrap,
        "808182838485868788898a8b8c8d8e8f9091929394959697",
        hex::decode("01d8bcc29b10ed6f0bb6b4ae53d57d552c4b61f2e99a23dbdb2a04c3924b7b011474").unwrap(),
        "7177b1d6af46344cd0e43a6c7ff6f7bf1b4a2c540a112f01e58cde1528ca2881",
        "308155ce26222c27a999c2261ecad62ea743f5990249099e51e0288489e09a07",
        "ae8127937453a75203fd2b23e58c355db5bb963fd565935a26a92ca83c238ee6"
    )]
    fn derives_the_kat_subkeys(
        #[case] key: &str,
        #[case] domain: Domain,
        #[case] nonce: &str,
        #[case] aad: Vec<u8>,
        #[case] subkey: &str,
        #[case] commitment: &str,
        #[case] aead_key: &str,
    ) {
        let key: [u8; 32] = hex::decode(key).unwrap().try_into().unwrap();
        let nonce: [u8; 24] = hex::decode(nonce).unwrap().try_into().unwrap();
        let aead = CommittingAead::new(&key, domain);

        let open = aead.derive_open(&nonce, &aad);
        assert_eq!(hex::encode(*open.subkey), subkey);
        assert_eq!(hex::encode(open.commitment.0), commitment);
        let (sealed_commitment, sealed_key) = aead.derive_seal(&nonce, &aad);
        assert_eq!(hex::encode(sealed_commitment.0), commitment);
        assert_eq!(hex::encode(*sealed_key.0), aead_key);
        let verified = open.verified(&hex::decode(commitment).unwrap()).unwrap();
        assert_eq!(hex::encode(*verified.0), aead_key);
    }

    #[rstest]
    fn round_trips() {
        let domain = prop_oneof![Just(Domain::Payload), Just(Domain::CekWrap)];
        proptest!(|(
            key in any::<[u8; 32]>(),
            nonce in any::<[u8; 24]>(),
            domain in domain,
            aad in prop::collection::vec(any::<u8>(), 0..512),
            message in prop::collection::vec(any::<u8>(), 0..4096),
        )| {
            let aead = CommittingAead::new(&key, domain);
            let mut sealed = aead.sealed(&nonce, &aad, &message);
            prop_assert_eq!(sealed.len(), CommittingAead::OVERHEAD + message.len());

            aead.open_in_place(&aad, &mut sealed).unwrap();
            prop_assert_eq!(&sealed[CommittingAead::CIPHERTEXT_START..sealed.len() - 16], &message[..]);
        });
    }

    #[rstest]
    #[case::nonce(0, AeadOpenError::Commitment)]
    #[case::nonce_end(23, AeadOpenError::Commitment)]
    #[case::commitment(24, AeadOpenError::Commitment)]
    #[case::commitment_end(55, AeadOpenError::Commitment)]
    #[case::ciphertext(56, AeadOpenError::Tag)]
    #[case::ciphertext_end(60, AeadOpenError::Tag)]
    #[case::tag(61, AeadOpenError::Tag)]
    #[case::tag_end(76, AeadOpenError::Tag)]
    fn a_flipped_bit_fails_its_check(
        #[case] offset: usize,
        #[case] expected: AeadOpenError,
        #[values(0, 7)] bit: u8,
    ) {
        let aead = CommittingAead::new(&PRIM_KEY, Domain::Payload);
        let mut sealed = aead.sealed(&PRIM_NONCE, b"aad", &KAT_PLAINTEXT);
        sealed[offset] ^= 1 << bit;

        assert_eq!(aead.open_in_place(b"aad", &mut sealed), Err(expected));
    }

    #[rstest]
    fn every_flipped_bit_fails_its_check() {
        let aead = CommittingAead::new(&PRIM_KEY, Domain::Payload);
        let original = aead.sealed(&PRIM_NONCE, b"aad", &KAT_PLAINTEXT);
        proptest!(|(offset in 0..original.len(), bit in 0..8u8)| {
            let mut flipped = original.clone();
            flipped[offset] ^= 1 << bit;
            let expected = if offset < CommittingAead::CIPHERTEXT_START {
                AeadOpenError::Commitment
            } else {
                AeadOpenError::Tag
            };

            prop_assert_eq!(aead.open_in_place(b"aad", &mut flipped), Err(expected));
        });
    }

    #[rstest]
    #[case::aad_byte(PRIM_KEY, Domain::Payload, b"aaD".as_slice())]
    #[case::aad_longer(PRIM_KEY, Domain::Payload, b"aad ".as_slice())]
    #[case::wrap_domain(PRIM_KEY, Domain::CekWrap, b"aad".as_slice())]
    #[case::key([0x5a; 32], Domain::Payload, b"aad".as_slice())]
    fn the_wrong_context_fails_the_commitment(
        #[case] key: [u8; 32],
        #[case] domain: Domain,
        #[case] aad: &[u8],
    ) {
        let mut sealed = CommittingAead::new(&PRIM_KEY, Domain::Payload).sealed(
            &PRIM_NONCE,
            b"aad",
            &KAT_PLAINTEXT,
        );

        assert_eq!(
            CommittingAead::new(&key, domain).open_in_place(aad, &mut sealed),
            Err(AeadOpenError::Commitment)
        );
    }

    #[rstest]
    #[case::empty(0)]
    #[case::one(1)]
    #[case::one_short(71)]
    fn shorter_than_the_overhead_is_short(#[case] len: usize) {
        let mut sealed = vec![0; len];

        assert_eq!(
            CommittingAead::new(&PRIM_KEY, Domain::Payload).open_in_place(b"", &mut sealed),
            Err(AeadOpenError::Short { len })
        );
    }
}

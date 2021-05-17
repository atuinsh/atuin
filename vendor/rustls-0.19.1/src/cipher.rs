use crate::error::TLSError;
use crate::key_schedule::{derive_traffic_iv, derive_traffic_key};
use crate::msgs::codec;
use crate::msgs::codec::Codec;
use crate::msgs::enums::{ContentType, ProtocolVersion};
use crate::msgs::fragmenter::MAX_FRAGMENT_LEN;
use crate::msgs::message::{BorrowMessage, Message, MessagePayload};
use crate::session::SessionSecrets;
use crate::suites::SupportedCipherSuite;
use ring::{aead, hkdf};
use std::io::Write;

/// Objects with this trait can decrypt TLS messages.
pub trait MessageDecrypter: Send + Sync {
    fn decrypt(&self, m: Message, seq: u64) -> Result<Message, TLSError>;
}

/// Objects with this trait can encrypt TLS messages.
pub trait MessageEncrypter: Send + Sync {
    fn encrypt(&self, m: BorrowMessage, seq: u64) -> Result<Message, TLSError>;
}

impl dyn MessageEncrypter {
    pub fn invalid() -> Box<dyn MessageEncrypter> {
        Box::new(InvalidMessageEncrypter {})
    }
}

impl dyn MessageDecrypter {
    pub fn invalid() -> Box<dyn MessageDecrypter> {
        Box::new(InvalidMessageDecrypter {})
    }
}

pub type MessageCipherPair = (Box<dyn MessageDecrypter>, Box<dyn MessageEncrypter>);

const TLS12_AAD_SIZE: usize = 8 + 1 + 2 + 2;
fn make_tls12_aad(
    seq: u64,
    typ: ContentType,
    vers: ProtocolVersion,
    len: usize,
) -> ring::aead::Aad<[u8; TLS12_AAD_SIZE]> {
    let mut out = [0; TLS12_AAD_SIZE];
    codec::put_u64(seq, &mut out[0..]);
    out[8] = typ.get_u8();
    codec::put_u16(vers.get_u16(), &mut out[9..]);
    codec::put_u16(len as u16, &mut out[11..]);
    ring::aead::Aad::from(out)
}

fn make_tls12_gcm_nonce(write_iv: &[u8], explicit: &[u8]) -> Iv {
    debug_assert_eq!(write_iv.len(), 4);
    debug_assert_eq!(explicit.len(), 8);

    // The GCM nonce is constructed from a 32-bit 'salt' derived
    // from the master-secret, and a 64-bit explicit part,
    // with no specified construction.  Thanks for that.
    //
    // We use the same construction as TLS1.3/ChaCha20Poly1305:
    // a starting point extracted from the key block, xored with
    // the sequence number.
    let mut iv = Iv(Default::default());
    iv.0[..4].copy_from_slice(write_iv);
    iv.0[4..].copy_from_slice(explicit);
    iv
}

pub type BuildTLS12Decrypter = fn(&[u8], &[u8]) -> Box<dyn MessageDecrypter>;
pub type BuildTLS12Encrypter = fn(&[u8], &[u8], &[u8]) -> Box<dyn MessageEncrypter>;

pub fn build_tls12_gcm_128_decrypter(key: &[u8], iv: &[u8]) -> Box<dyn MessageDecrypter> {
    Box::new(GCMMessageDecrypter::new(&aead::AES_128_GCM, key, iv))
}

pub fn build_tls12_gcm_128_encrypter(
    key: &[u8],
    iv: &[u8],
    extra: &[u8],
) -> Box<dyn MessageEncrypter> {
    let nonce = make_tls12_gcm_nonce(iv, extra);
    Box::new(GCMMessageEncrypter::new(&aead::AES_128_GCM, key, nonce))
}

pub fn build_tls12_gcm_256_decrypter(key: &[u8], iv: &[u8]) -> Box<dyn MessageDecrypter> {
    Box::new(GCMMessageDecrypter::new(&aead::AES_256_GCM, key, iv))
}

pub fn build_tls12_gcm_256_encrypter(
    key: &[u8],
    iv: &[u8],
    extra: &[u8],
) -> Box<dyn MessageEncrypter> {
    let nonce = make_tls12_gcm_nonce(iv, extra);
    Box::new(GCMMessageEncrypter::new(&aead::AES_256_GCM, key, nonce))
}

pub fn build_tls12_chacha_decrypter(key: &[u8], iv: &[u8]) -> Box<dyn MessageDecrypter> {
    Box::new(ChaCha20Poly1305MessageDecrypter::new(
        &aead::CHACHA20_POLY1305,
        key,
        Iv::copy(iv),
    ))
}

pub fn build_tls12_chacha_encrypter(key: &[u8], iv: &[u8], _: &[u8]) -> Box<dyn MessageEncrypter> {
    Box::new(ChaCha20Poly1305MessageEncrypter::new(
        &aead::CHACHA20_POLY1305,
        key,
        Iv::copy(iv),
    ))
}

/// Make a `MessageCipherPair` based on the given supported ciphersuite `scs`,
/// and the session's `secrets`.
pub fn new_tls12(
    scs: &'static SupportedCipherSuite,
    secrets: &SessionSecrets,
) -> MessageCipherPair {
    // Make a key block, and chop it up.
    // nb. we don't implement any ciphersuites with nonzero mac_key_len.
    let key_block = secrets.make_key_block(scs.key_block_len());

    let mut offs = 0;
    let client_write_key = &key_block[offs..offs + scs.enc_key_len];
    offs += scs.enc_key_len;
    let server_write_key = &key_block[offs..offs + scs.enc_key_len];
    offs += scs.enc_key_len;
    let client_write_iv = &key_block[offs..offs + scs.fixed_iv_len];
    offs += scs.fixed_iv_len;
    let server_write_iv = &key_block[offs..offs + scs.fixed_iv_len];
    offs += scs.fixed_iv_len;

    let (write_key, write_iv) = if secrets.randoms.we_are_client {
        (client_write_key, client_write_iv)
    } else {
        (server_write_key, server_write_iv)
    };

    let (read_key, read_iv) = if secrets.randoms.we_are_client {
        (server_write_key, server_write_iv)
    } else {
        (client_write_key, client_write_iv)
    };

    (
        scs.build_tls12_decrypter.unwrap()(read_key, read_iv),
        scs.build_tls12_encrypter.unwrap()(write_key, write_iv, &key_block[offs..]),
    )
}

pub fn new_tls13_read(
    scs: &'static SupportedCipherSuite,
    secret: &hkdf::Prk,
) -> Box<dyn MessageDecrypter> {
    let key = derive_traffic_key(secret, scs.aead_algorithm);
    let iv = derive_traffic_iv(secret);

    Box::new(TLS13MessageDecrypter::new(key, iv))
}

pub fn new_tls13_write(
    scs: &'static SupportedCipherSuite,
    secret: &hkdf::Prk,
) -> Box<dyn MessageEncrypter> {
    let key = derive_traffic_key(secret, scs.aead_algorithm);
    let iv = derive_traffic_iv(secret);

    Box::new(TLS13MessageEncrypter::new(key, iv))
}

/// A `MessageEncrypter` for AES-GCM AEAD ciphersuites. TLS 1.2 only.
pub struct GCMMessageEncrypter {
    enc_key: aead::LessSafeKey,
    iv: Iv,
}

/// A `MessageDecrypter` for AES-GCM AEAD ciphersuites.  TLS1.2 only.
pub struct GCMMessageDecrypter {
    dec_key: aead::LessSafeKey,
    dec_salt: [u8; 4],
}

const GCM_EXPLICIT_NONCE_LEN: usize = 8;
const GCM_OVERHEAD: usize = GCM_EXPLICIT_NONCE_LEN + 16;

impl MessageDecrypter for GCMMessageDecrypter {
    fn decrypt(&self, mut msg: Message, seq: u64) -> Result<Message, TLSError> {
        let payload = msg
            .take_opaque_payload()
            .ok_or(TLSError::DecryptError)?;
        let mut buf = payload.0;

        if buf.len() < GCM_OVERHEAD {
            return Err(TLSError::DecryptError);
        }

        let nonce = {
            let mut nonce = [0u8; 12];
            nonce
                .as_mut()
                .write_all(&self.dec_salt)
                .unwrap();
            nonce[4..]
                .as_mut()
                .write_all(&buf[..8])
                .unwrap();
            aead::Nonce::assume_unique_for_key(nonce)
        };

        let aad = make_tls12_aad(seq, msg.typ, msg.version, buf.len() - GCM_OVERHEAD);

        let plain_len = self
            .dec_key
            .open_within(nonce, aad, &mut buf, GCM_EXPLICIT_NONCE_LEN..)
            .map_err(|_| TLSError::DecryptError)?
            .len();

        if plain_len > MAX_FRAGMENT_LEN {
            return Err(TLSError::PeerSentOversizedRecord);
        }

        buf.truncate(plain_len);

        Ok(Message {
            typ: msg.typ,
            version: msg.version,
            payload: MessagePayload::new_opaque(buf),
        })
    }
}

impl MessageEncrypter for GCMMessageEncrypter {
    fn encrypt(&self, msg: BorrowMessage, seq: u64) -> Result<Message, TLSError> {
        let nonce = make_tls13_nonce(&self.iv, seq);
        let aad = make_tls12_aad(seq, msg.typ, msg.version, msg.payload.len());

        let total_len = msg.payload.len() + self.enc_key.algorithm().tag_len();
        let mut payload = Vec::with_capacity(GCM_EXPLICIT_NONCE_LEN + total_len);
        payload.extend_from_slice(&nonce.as_ref()[4..]);
        payload.extend_from_slice(&msg.payload);

        self.enc_key
            .seal_in_place_separate_tag(nonce, aad, &mut payload[GCM_EXPLICIT_NONCE_LEN..])
            .map(|tag| payload.extend(tag.as_ref()))
            .map_err(|_| TLSError::General("encrypt failed".to_string()))?;

        Ok(Message {
            typ: msg.typ,
            version: msg.version,
            payload: MessagePayload::new_opaque(payload),
        })
    }
}

impl GCMMessageEncrypter {
    fn new(alg: &'static aead::Algorithm, enc_key: &[u8], iv: Iv) -> GCMMessageEncrypter {
        let key = aead::UnboundKey::new(alg, enc_key).unwrap();
        GCMMessageEncrypter {
            enc_key: aead::LessSafeKey::new(key),
            iv,
        }
    }
}

impl GCMMessageDecrypter {
    fn new(alg: &'static aead::Algorithm, dec_key: &[u8], dec_iv: &[u8]) -> GCMMessageDecrypter {
        let key = aead::UnboundKey::new(alg, dec_key).unwrap();
        let mut ret = GCMMessageDecrypter {
            dec_key: aead::LessSafeKey::new(key),
            dec_salt: [0u8; 4],
        };

        debug_assert_eq!(dec_iv.len(), 4);
        ret.dec_salt
            .as_mut()
            .write_all(dec_iv)
            .unwrap();
        ret
    }
}

/// A TLS 1.3 write or read IV.
pub(crate) struct Iv([u8; ring::aead::NONCE_LEN]);

impl Iv {
    pub(crate) fn new(value: [u8; ring::aead::NONCE_LEN]) -> Self {
        Self(value)
    }

    fn copy(value: &[u8]) -> Self {
        debug_assert_eq!(value.len(), ring::aead::NONCE_LEN);
        let mut iv = Iv::new(Default::default());
        iv.0.copy_from_slice(value);
        iv
    }

    #[cfg(test)]
    pub(crate) fn value(&self) -> &[u8; 12] {
        &self.0
    }
}

pub(crate) struct IvLen;

impl hkdf::KeyType for IvLen {
    fn len(&self) -> usize {
        aead::NONCE_LEN
    }
}

impl From<hkdf::Okm<'_, IvLen>> for Iv {
    fn from(okm: hkdf::Okm<IvLen>) -> Self {
        let mut r = Iv(Default::default());
        okm.fill(&mut r.0[..]).unwrap();
        r
    }
}

struct TLS13MessageEncrypter {
    enc_key: aead::LessSafeKey,
    iv: Iv,
}

struct TLS13MessageDecrypter {
    dec_key: aead::LessSafeKey,
    iv: Iv,
}

fn unpad_tls13(v: &mut Vec<u8>) -> ContentType {
    loop {
        match v.pop() {
            Some(0) => {}

            Some(content_type) => return ContentType::read_bytes(&[content_type]).unwrap(),

            None => return ContentType::Unknown(0),
        }
    }
}

fn make_tls13_nonce(iv: &Iv, seq: u64) -> ring::aead::Nonce {
    let mut nonce = [0u8; ring::aead::NONCE_LEN];
    codec::put_u64(seq, &mut nonce[4..]);

    nonce
        .iter_mut()
        .zip(iv.0.iter())
        .for_each(|(nonce, iv)| {
            *nonce ^= *iv;
        });

    aead::Nonce::assume_unique_for_key(nonce)
}

fn make_tls13_aad(len: usize) -> ring::aead::Aad<[u8; 1 + 2 + 2]> {
    ring::aead::Aad::from([
        0x17, // ContentType::ApplicationData
        0x3,  // ProtocolVersion (major)
        0x3,  // ProtocolVersion (minor)
        (len >> 8) as u8,
        len as u8,
    ])
}

impl MessageEncrypter for TLS13MessageEncrypter {
    fn encrypt(&self, msg: BorrowMessage, seq: u64) -> Result<Message, TLSError> {
        let total_len = msg.payload.len() + 1 + self.enc_key.algorithm().tag_len();
        let mut buf = Vec::with_capacity(total_len);
        buf.extend_from_slice(&msg.payload);
        msg.typ.encode(&mut buf);

        let nonce = make_tls13_nonce(&self.iv, seq);
        let aad = make_tls13_aad(total_len);

        self.enc_key
            .seal_in_place_append_tag(nonce, aad, &mut buf)
            .map_err(|_| TLSError::General("encrypt failed".to_string()))?;

        Ok(Message {
            typ: ContentType::ApplicationData,
            version: ProtocolVersion::TLSv1_2,
            payload: MessagePayload::new_opaque(buf),
        })
    }
}

impl MessageDecrypter for TLS13MessageDecrypter {
    fn decrypt(&self, mut msg: Message, seq: u64) -> Result<Message, TLSError> {
        let payload = msg
            .take_opaque_payload()
            .ok_or(TLSError::DecryptError)?;
        let mut buf = payload.0;

        if buf.len() < self.dec_key.algorithm().tag_len() {
            return Err(TLSError::DecryptError);
        }

        let nonce = make_tls13_nonce(&self.iv, seq);
        let aad = make_tls13_aad(buf.len());
        let plain_len = self
            .dec_key
            .open_in_place(nonce, aad, &mut buf)
            .map_err(|_| TLSError::DecryptError)?
            .len();

        buf.truncate(plain_len);

        if buf.len() > MAX_FRAGMENT_LEN + 1 {
            return Err(TLSError::PeerSentOversizedRecord);
        }

        let content_type = unpad_tls13(&mut buf);
        if content_type == ContentType::Unknown(0) {
            let msg = "peer sent bad TLSInnerPlaintext".to_string();
            return Err(TLSError::PeerMisbehavedError(msg));
        }

        if buf.len() > MAX_FRAGMENT_LEN {
            return Err(TLSError::PeerSentOversizedRecord);
        }

        Ok(Message {
            typ: content_type,
            version: ProtocolVersion::TLSv1_3,
            payload: MessagePayload::new_opaque(buf),
        })
    }
}

impl TLS13MessageEncrypter {
    fn new(key: aead::UnboundKey, enc_iv: Iv) -> TLS13MessageEncrypter {
        TLS13MessageEncrypter {
            enc_key: aead::LessSafeKey::new(key),
            iv: enc_iv,
        }
    }
}

impl TLS13MessageDecrypter {
    fn new(key: aead::UnboundKey, dec_iv: Iv) -> TLS13MessageDecrypter {
        TLS13MessageDecrypter {
            dec_key: aead::LessSafeKey::new(key),
            iv: dec_iv,
        }
    }
}

/// The RFC7905/RFC7539 ChaCha20Poly1305 construction.
/// This implementation does the AAD construction required in TLS1.2.
/// TLS1.3 uses `TLS13MessageEncrypter`.
pub struct ChaCha20Poly1305MessageEncrypter {
    enc_key: aead::LessSafeKey,
    enc_offset: Iv,
}

/// The RFC7905/RFC7539 ChaCha20Poly1305 construction.
/// This implementation does the AAD construction required in TLS1.2.
/// TLS1.3 uses `TLS13MessageDecrypter`.
pub struct ChaCha20Poly1305MessageDecrypter {
    dec_key: aead::LessSafeKey,
    dec_offset: Iv,
}

impl ChaCha20Poly1305MessageEncrypter {
    fn new(
        alg: &'static aead::Algorithm,
        enc_key: &[u8],
        enc_iv: Iv,
    ) -> ChaCha20Poly1305MessageEncrypter {
        let key = aead::UnboundKey::new(alg, enc_key).unwrap();
        ChaCha20Poly1305MessageEncrypter {
            enc_key: aead::LessSafeKey::new(key),
            enc_offset: enc_iv,
        }
    }
}

impl ChaCha20Poly1305MessageDecrypter {
    fn new(
        alg: &'static aead::Algorithm,
        dec_key: &[u8],
        dec_iv: Iv,
    ) -> ChaCha20Poly1305MessageDecrypter {
        let key = aead::UnboundKey::new(alg, dec_key).unwrap();
        ChaCha20Poly1305MessageDecrypter {
            dec_key: aead::LessSafeKey::new(key),
            dec_offset: dec_iv,
        }
    }
}

const CHACHAPOLY1305_OVERHEAD: usize = 16;

impl MessageDecrypter for ChaCha20Poly1305MessageDecrypter {
    fn decrypt(&self, mut msg: Message, seq: u64) -> Result<Message, TLSError> {
        let payload = msg
            .take_opaque_payload()
            .ok_or(TLSError::DecryptError)?;
        let mut buf = payload.0;

        if buf.len() < CHACHAPOLY1305_OVERHEAD {
            return Err(TLSError::DecryptError);
        }

        let nonce = make_tls13_nonce(&self.dec_offset, seq);
        let aad = make_tls12_aad(
            seq,
            msg.typ,
            msg.version,
            buf.len() - CHACHAPOLY1305_OVERHEAD,
        );

        let plain_len = self
            .dec_key
            .open_in_place(nonce, aad, &mut buf)
            .map_err(|_| TLSError::DecryptError)?
            .len();

        if plain_len > MAX_FRAGMENT_LEN {
            return Err(TLSError::PeerSentOversizedRecord);
        }

        buf.truncate(plain_len);

        Ok(Message {
            typ: msg.typ,
            version: msg.version,
            payload: MessagePayload::new_opaque(buf),
        })
    }
}

impl MessageEncrypter for ChaCha20Poly1305MessageEncrypter {
    fn encrypt(&self, msg: BorrowMessage, seq: u64) -> Result<Message, TLSError> {
        let nonce = make_tls13_nonce(&self.enc_offset, seq);
        let aad = make_tls12_aad(seq, msg.typ, msg.version, msg.payload.len());

        let total_len = msg.payload.len() + self.enc_key.algorithm().tag_len();
        let mut buf = Vec::with_capacity(total_len);
        buf.extend_from_slice(&msg.payload);

        self.enc_key
            .seal_in_place_append_tag(nonce, aad, &mut buf)
            .map_err(|_| TLSError::General("encrypt failed".to_string()))?;

        Ok(Message {
            typ: msg.typ,
            version: msg.version,
            payload: MessagePayload::new_opaque(buf),
        })
    }
}

/// A `MessageEncrypter` which doesn't work.
pub struct InvalidMessageEncrypter {}

impl MessageEncrypter for InvalidMessageEncrypter {
    fn encrypt(&self, _m: BorrowMessage, _seq: u64) -> Result<Message, TLSError> {
        Err(TLSError::General("encrypt not yet available".to_string()))
    }
}

/// A `MessageDecrypter` which doesn't work.
pub struct InvalidMessageDecrypter {}

impl MessageDecrypter for InvalidMessageDecrypter {
    fn decrypt(&self, _m: Message, _seq: u64) -> Result<Message, TLSError> {
        Err(TLSError::DecryptError)
    }
}

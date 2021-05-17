/// This module contains optional APIs for implementing QUIC TLS.
use crate::client::{ClientConfig, ClientSession, ClientSessionImpl};
use crate::error::TLSError;
use crate::key_schedule::hkdf_expand;
use crate::msgs::enums::{AlertDescription, ContentType, ProtocolVersion};
use crate::msgs::handshake::{ClientExtension, ServerExtension};
use crate::msgs::message::{Message, MessagePayload};
use crate::server::{ServerConfig, ServerSession, ServerSessionImpl};
use crate::session::{Protocol, SessionCommon};
use crate::suites::{BulkAlgorithm, SupportedCipherSuite, TLS13_AES_128_GCM_SHA256};

use std::sync::Arc;

use ring::{aead, hkdf};
use webpki;

/// Secrets used to encrypt/decrypt traffic
#[derive(Clone, Debug)]
pub(crate) struct Secrets {
    /// Secret used to encrypt packets transmitted by the client
    pub client: hkdf::Prk,
    /// Secret used to encrypt packets transmitted by the server
    pub server: hkdf::Prk,
}

impl Secrets {
    fn local_remote(&self, is_client: bool) -> (&hkdf::Prk, &hkdf::Prk) {
        if is_client {
            (&self.client, &self.server)
        } else {
            (&self.server, &self.client)
        }
    }
}

/// Generic methods for QUIC sessions
pub trait QuicExt {
    /// Return the TLS-encoded transport parameters for the session's peer.
    fn get_quic_transport_parameters(&self) -> Option<&[u8]>;

    /// Compute the keys for encrypting/decrypting 0-RTT packets, if available
    fn get_0rtt_keys(&self) -> Option<DirectionalKeys>;

    /// Consume unencrypted TLS handshake data.
    ///
    /// Handshake data obtained from separate encryption levels should be supplied in separate calls.
    fn read_hs(&mut self, plaintext: &[u8]) -> Result<(), TLSError>;

    /// Emit unencrypted TLS handshake data.
    ///
    /// When this returns `Some(_)`, the new keys must be used for future handshake data.
    fn write_hs(&mut self, buf: &mut Vec<u8>) -> Option<Keys>;

    /// Emit the TLS description code of a fatal alert, if one has arisen.
    ///
    /// Check after `read_hs` returns `Err(_)`.
    fn get_alert(&self) -> Option<AlertDescription>;

    /// Compute the keys to use following a 1-RTT key update
    ///
    /// Must not be called until the handshake is complete
    fn next_1rtt_keys(&mut self) -> PacketKeySet;
}

impl QuicExt for ClientSession {
    fn get_quic_transport_parameters(&self) -> Option<&[u8]> {
        self.imp
            .common
            .quic
            .params
            .as_ref()
            .map(|v| v.as_ref())
    }

    fn get_0rtt_keys(&self) -> Option<DirectionalKeys> {
        Some(DirectionalKeys::new(
            self.imp.resumption_ciphersuite?,
            self.imp
                .common
                .quic
                .early_secret
                .as_ref()?,
        ))
    }

    fn read_hs(&mut self, plaintext: &[u8]) -> Result<(), TLSError> {
        read_hs(&mut self.imp.common, plaintext)?;
        self.imp
            .process_new_handshake_messages()
    }

    fn write_hs(&mut self, buf: &mut Vec<u8>) -> Option<Keys> {
        write_hs(&mut self.imp.common, buf)
    }

    fn get_alert(&self) -> Option<AlertDescription> {
        self.imp.common.quic.alert
    }

    fn next_1rtt_keys(&mut self) -> PacketKeySet {
        next_1rtt_keys(&mut self.imp.common)
    }
}

impl QuicExt for ServerSession {
    fn get_quic_transport_parameters(&self) -> Option<&[u8]> {
        self.imp
            .common
            .quic
            .params
            .as_ref()
            .map(|v| v.as_ref())
    }

    fn get_0rtt_keys(&self) -> Option<DirectionalKeys> {
        Some(DirectionalKeys::new(
            self.imp.common.get_suite()?,
            self.imp
                .common
                .quic
                .early_secret
                .as_ref()?,
        ))
    }

    fn read_hs(&mut self, plaintext: &[u8]) -> Result<(), TLSError> {
        read_hs(&mut self.imp.common, plaintext)?;
        self.imp
            .process_new_handshake_messages()
    }
    fn write_hs(&mut self, buf: &mut Vec<u8>) -> Option<Keys> {
        write_hs(&mut self.imp.common, buf)
    }

    fn get_alert(&self) -> Option<AlertDescription> {
        self.imp.common.quic.alert
    }

    fn next_1rtt_keys(&mut self) -> PacketKeySet {
        next_1rtt_keys(&mut self.imp.common)
    }
}

/// Keys used to communicate in a single direction
pub struct DirectionalKeys {
    /// Encrypts or decrypts a packet's headers
    pub header: aead::quic::HeaderProtectionKey,
    /// Encrypts or decrypts the payload of a packet
    pub packet: PacketKey,
}

impl DirectionalKeys {
    fn new(suite: &'static SupportedCipherSuite, secret: &hkdf::Prk) -> Self {
        let hp_alg = match suite.bulk {
            BulkAlgorithm::AES_128_GCM => &aead::quic::AES_128,
            BulkAlgorithm::AES_256_GCM => &aead::quic::AES_256,
            BulkAlgorithm::CHACHA20_POLY1305 => &aead::quic::CHACHA20,
        };

        Self {
            header: hkdf_expand(secret, hp_alg, b"quic hp", &[]),
            packet: PacketKey::new(suite, secret),
        }
    }
}

/// Keys to encrypt or decrypt the payload of a packet
pub struct PacketKey {
    /// Encrypts or decrypts a packet's payload
    pub key: aead::LessSafeKey,
    /// Computes unique nonces for each packet
    pub iv: Iv,
}

impl PacketKey {
    fn new(suite: &'static SupportedCipherSuite, secret: &hkdf::Prk) -> Self {
        Self {
            key: aead::LessSafeKey::new(hkdf_expand(
                secret,
                suite.aead_algorithm,
                b"quic key",
                &[],
            )),
            iv: hkdf_expand(secret, IvLen, b"quic iv", &[]),
        }
    }
}

/// Packet protection keys for bidirectional 1-RTT communication
pub struct PacketKeySet {
    /// Encrypts outgoing packets
    pub local: PacketKey,
    /// Decrypts incoming packets
    pub remote: PacketKey,
}

/// Computes unique nonces for each packet
pub struct Iv([u8; aead::NONCE_LEN]);

impl Iv {
    /// Compute the nonce to use for encrypting or decrypting `packet_number`
    pub fn nonce_for(&self, packet_number: u64) -> ring::aead::Nonce {
        let mut out = [0; aead::NONCE_LEN];
        out[4..].copy_from_slice(&packet_number.to_be_bytes());
        for (out, inp) in out.iter_mut().zip(self.0.iter()) {
            *out ^= inp;
        }
        aead::Nonce::assume_unique_for_key(out)
    }
}

impl From<hkdf::Okm<'_, IvLen>> for Iv {
    fn from(okm: hkdf::Okm<IvLen>) -> Self {
        let mut iv = [0; aead::NONCE_LEN];
        okm.fill(&mut iv[..]).unwrap();
        Iv(iv)
    }
}

struct IvLen;

impl hkdf::KeyType for IvLen {
    fn len(&self) -> usize {
        aead::NONCE_LEN
    }
}

/// Complete set of keys used to communicate with the peer
pub struct Keys {
    /// Encrypts outgoing packets
    pub local: DirectionalKeys,
    /// Decrypts incoming packets
    pub remote: DirectionalKeys,
}

impl Keys {
    /// Construct keys for use with initial packets
    pub fn initial(
        initial_salt: &hkdf::Salt,
        client_dst_connection_id: &[u8],
        is_client: bool,
    ) -> Self {
        const CLIENT_LABEL: &[u8] = b"client in";
        const SERVER_LABEL: &[u8] = b"server in";
        let hs_secret = initial_salt.extract(client_dst_connection_id);

        let secrets = Secrets {
            client: hkdf_expand(&hs_secret, hkdf::HKDF_SHA256, CLIENT_LABEL, &[]),
            server: hkdf_expand(&hs_secret, hkdf::HKDF_SHA256, SERVER_LABEL, &[]),
        };
        Self::new(&TLS13_AES_128_GCM_SHA256, is_client, &secrets)
    }

    fn new(suite: &'static SupportedCipherSuite, is_client: bool, secrets: &Secrets) -> Self {
        let (local, remote) = secrets.local_remote(is_client);
        Keys {
            local: DirectionalKeys::new(suite, local),
            remote: DirectionalKeys::new(suite, remote),
        }
    }
}

fn read_hs(this: &mut SessionCommon, plaintext: &[u8]) -> Result<(), TLSError> {
    if this
        .handshake_joiner
        .take_message(Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_3,
            payload: MessagePayload::new_opaque(plaintext.into()),
        })
        .is_none()
    {
        this.quic.alert = Some(AlertDescription::DecodeError);
        return Err(TLSError::CorruptMessage);
    }
    Ok(())
}

fn write_hs(this: &mut SessionCommon, buf: &mut Vec<u8>) -> Option<Keys> {
    while let Some((_, msg)) = this.quic.hs_queue.pop_front() {
        buf.extend_from_slice(&msg);
        if let Some(&(true, _)) = this.quic.hs_queue.front() {
            if this.quic.hs_secrets.is_some() {
                // Allow the caller to switch keys before proceeding.
                break;
            }
        }
    }
    if let Some(secrets) = this.quic.hs_secrets.take() {
        return Some(Keys::new(this.get_suite_assert(), this.is_client, &secrets));
    }
    if let Some(secrets) = this.quic.traffic_secrets.as_ref() {
        if !this.quic.returned_traffic_keys {
            this.quic.returned_traffic_keys = true;
            return Some(Keys::new(this.get_suite_assert(), this.is_client, &secrets));
        }
    }
    None
}

fn next_1rtt_keys(this: &mut SessionCommon) -> PacketKeySet {
    let hkdf_alg = this.get_suite_assert().hkdf_algorithm;
    let secrets = this
        .quic
        .traffic_secrets
        .as_ref()
        .expect("traffic keys not yet available");

    let next = next_1rtt_secrets(hkdf_alg, secrets);

    let (local, remote) = next.local_remote(this.is_client);
    let keys = PacketKeySet {
        local: PacketKey::new(this.get_suite_assert(), local),
        remote: PacketKey::new(this.get_suite_assert(), remote),
    };

    this.quic.traffic_secrets = Some(next);
    keys
}

fn next_1rtt_secrets(hkdf_alg: hkdf::Algorithm, prev: &Secrets) -> Secrets {
    Secrets {
        client: hkdf_expand(&prev.client, hkdf_alg, b"quic ku", &[]),
        server: hkdf_expand(&prev.server, hkdf_alg, b"quic ku", &[]),
    }
}

/// Methods specific to QUIC client sessions
pub trait ClientQuicExt {
    /// Make a new QUIC ClientSession. This differs from `ClientSession::new()`
    /// in that it takes an extra argument, `params`, which contains the
    /// TLS-encoded transport parameters to send.
    fn new_quic(
        config: &Arc<ClientConfig>,
        hostname: webpki::DNSNameRef,
        params: Vec<u8>,
    ) -> ClientSession {
        assert!(
            config
                .versions
                .iter()
                .all(|x| x.get_u16() >= ProtocolVersion::TLSv1_3.get_u16()),
            "QUIC requires TLS version >= 1.3"
        );
        let mut imp = ClientSessionImpl::new(config);
        imp.common.protocol = Protocol::Quic;
        imp.start_handshake(
            hostname.into(),
            vec![ClientExtension::TransportParameters(params)],
        );
        ClientSession { imp }
    }
}

impl ClientQuicExt for ClientSession {}

/// Methods specific to QUIC server sessions
pub trait ServerQuicExt {
    /// Make a new QUIC ServerSession. This differs from `ServerSession::new()`
    /// in that it takes an extra argument, `params`, which contains the
    /// TLS-encoded transport parameters to send.
    fn new_quic(config: &Arc<ServerConfig>, params: Vec<u8>) -> ServerSession {
        assert!(
            config
                .versions
                .iter()
                .all(|x| x.get_u16() >= ProtocolVersion::TLSv1_3.get_u16()),
            "QUIC requires TLS version >= 1.3"
        );
        assert!(
            config.max_early_data_size == 0 || config.max_early_data_size == 0xffff_ffff,
            "QUIC sessions must set a max early data of 0 or 2^32-1"
        );
        let mut imp =
            ServerSessionImpl::new(config, vec![ServerExtension::TransportParameters(params)]);
        imp.common.protocol = Protocol::Quic;
        ServerSession { imp }
    }
}

impl ServerQuicExt for ServerSession {}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn initial_keys_test_vectors() {
        // Test vectors based on draft 27
        const INITIAL_SALT: [u8; 20] = [
            0xc3, 0xee, 0xf7, 0x12, 0xc7, 0x2e, 0xbb, 0x5a, 0x11, 0xa7, 0xd2, 0x43, 0x2b, 0xb4,
            0x63, 0x65, 0xbe, 0xf9, 0xf5, 0x02,
        ];

        const CONNECTION_ID: &[u8] = &[0x83, 0x94, 0xc8, 0xf0, 0x3e, 0x51, 0x57, 0x08];
        const PACKET_NUMBER: u64 = 42;

        let initial_salt = hkdf::Salt::new(hkdf::HKDF_SHA256, &INITIAL_SALT);
        let server_keys = Keys::initial(&initial_salt, &CONNECTION_ID, false);
        let client_keys = Keys::initial(&initial_salt, &CONNECTION_ID, true);

        // Nonces
        const SERVER_NONCE: [u8; 12] = [
            0x5e, 0x5a, 0xe6, 0x51, 0xfd, 0x1e, 0x84, 0x95, 0xaf, 0x13, 0x50, 0xa1,
        ];
        assert_eq!(
            server_keys
                .local
                .packet
                .iv
                .nonce_for(PACKET_NUMBER)
                .as_ref(),
            &SERVER_NONCE
        );
        assert_eq!(
            client_keys
                .remote
                .packet
                .iv
                .nonce_for(PACKET_NUMBER)
                .as_ref(),
            &SERVER_NONCE
        );
        const CLIENT_NONCE: [u8; 12] = [
            0x86, 0x81, 0x35, 0x94, 0x10, 0xa7, 0x0b, 0xb9, 0xc9, 0x2f, 0x04, 0x0a,
        ];
        assert_eq!(
            server_keys
                .remote
                .packet
                .iv
                .nonce_for(PACKET_NUMBER)
                .as_ref(),
            &CLIENT_NONCE
        );
        assert_eq!(
            client_keys
                .local
                .packet
                .iv
                .nonce_for(PACKET_NUMBER)
                .as_ref(),
            &CLIENT_NONCE
        );

        // Header encryption mask
        const SAMPLE: &[u8] = &[
            0x70, 0x02, 0x59, 0x6f, 0x99, 0xae, 0x67, 0xab, 0xf6, 0x5a, 0x58, 0x52, 0xf5, 0x4f,
            0x58, 0xc3,
        ];

        const SERVER_MASK: [u8; 5] = [0x38, 0x16, 0x8a, 0x0c, 0x25];
        assert_eq!(
            server_keys
                .local
                .header
                .new_mask(SAMPLE)
                .unwrap(),
            SERVER_MASK
        );
        assert_eq!(
            client_keys
                .remote
                .header
                .new_mask(SAMPLE)
                .unwrap(),
            SERVER_MASK
        );
        const CLIENT_MASK: [u8; 5] = [0xae, 0x96, 0x2e, 0x67, 0xec];
        assert_eq!(
            server_keys
                .remote
                .header
                .new_mask(SAMPLE)
                .unwrap(),
            CLIENT_MASK
        );
        assert_eq!(
            client_keys
                .local
                .header
                .new_mask(SAMPLE)
                .unwrap(),
            CLIENT_MASK
        );

        const AAD: &[u8] = &[
            0xc9, 0xff, 0x00, 0x00, 0x1b, 0x00, 0x08, 0xf0, 0x67, 0xa5, 0x50, 0x2a, 0x42, 0x62,
            0xb5, 0x00, 0x40, 0x74, 0x16, 0x8b,
        ];
        let aad = aead::Aad::from(AAD);
        const PLAINTEXT: [u8; 12] = [
            0x0d, 0x00, 0x00, 0x00, 0x00, 0x18, 0x41, 0x0a, 0x02, 0x00, 0x00, 0x56,
        ];
        let mut payload = PLAINTEXT;
        let server_nonce = server_keys
            .local
            .packet
            .iv
            .nonce_for(PACKET_NUMBER);
        let tag = server_keys
            .local
            .packet
            .key
            .seal_in_place_separate_tag(server_nonce, aad, &mut payload)
            .unwrap();
        assert_eq!(
            payload,
            [
                0x0d, 0x91, 0x96, 0x31, 0xc0, 0xeb, 0x84, 0xf2, 0x88, 0x59, 0xfe, 0xc0
            ]
        );
        assert_eq!(
            tag.as_ref(),
            &[
                0xdf, 0xee, 0x06, 0x81, 0x9e, 0x7a, 0x08, 0x34, 0xe4, 0x94, 0x19, 0x79, 0x5f, 0xe0,
                0xd7, 0x3f
            ]
        );

        let aad = aead::Aad::from(AAD);
        let mut payload = PLAINTEXT;
        let client_nonce = client_keys
            .local
            .packet
            .iv
            .nonce_for(PACKET_NUMBER);
        let tag = client_keys
            .local
            .packet
            .key
            .seal_in_place_separate_tag(client_nonce, aad, &mut payload)
            .unwrap();
        assert_eq!(
            payload,
            [
                0x89, 0x6c, 0x66, 0x91, 0xe0, 0x9f, 0x47, 0x7a, 0x91, 0x42, 0xa4, 0x46
            ]
        );
        assert_eq!(
            tag.as_ref(),
            &[
                0xb6, 0xff, 0xef, 0x89, 0xd5, 0xcb, 0x53, 0xd0, 0x98, 0xf7, 0x40, 0xa, 0x8d, 0x97,
                0x72, 0x6e
            ]
        );
    }

    #[test]
    fn key_update_test_vector() {
        fn equal_prk(x: &hkdf::Prk, y: &hkdf::Prk) -> bool {
            let mut x_data = [0; 16];
            let mut y_data = [0; 16];
            let x_okm = x
                .expand(&[b"info"], &aead::quic::AES_128)
                .unwrap();
            x_okm.fill(&mut x_data[..]).unwrap();
            let y_okm = y
                .expand(&[b"info"], &aead::quic::AES_128)
                .unwrap();
            y_okm.fill(&mut y_data[..]).unwrap();
            x_data == y_data
        }

        let initial = Secrets {
            // Constant dummy values for reproducibility
            client: hkdf::Prk::new_less_safe(
                hkdf::HKDF_SHA256,
                &[
                    0xb8, 0x76, 0x77, 0x08, 0xf8, 0x77, 0x23, 0x58, 0xa6, 0xea, 0x9f, 0xc4, 0x3e,
                    0x4a, 0xdd, 0x2c, 0x96, 0x1b, 0x3f, 0x52, 0x87, 0xa6, 0xd1, 0x46, 0x7e, 0xe0,
                    0xae, 0xab, 0x33, 0x72, 0x4d, 0xbf,
                ],
            ),
            server: hkdf::Prk::new_less_safe(
                hkdf::HKDF_SHA256,
                &[
                    0x42, 0xdc, 0x97, 0x21, 0x40, 0xe0, 0xf2, 0xe3, 0x98, 0x45, 0xb7, 0x67, 0x61,
                    0x34, 0x39, 0xdc, 0x67, 0x58, 0xca, 0x43, 0x25, 0x9b, 0x87, 0x85, 0x06, 0x82,
                    0x4e, 0xb1, 0xe4, 0x38, 0xd8, 0x55,
                ],
            ),
        };
        let updated = next_1rtt_secrets(hkdf::HKDF_SHA256, &initial);

        assert!(equal_prk(
            &updated.client,
            &hkdf::Prk::new_less_safe(
                hkdf::HKDF_SHA256,
                &[
                    0x42, 0xca, 0xc8, 0xc9, 0x1c, 0xd5, 0xeb, 0x40, 0x68, 0x2e, 0x43, 0x2e, 0xdf,
                    0x2d, 0x2b, 0xe9, 0xf4, 0x1a, 0x52, 0xca, 0x6b, 0x22, 0xd8, 0xe6, 0xcd, 0xb1,
                    0xe8, 0xac, 0xa9, 0x6, 0x1f, 0xce
                ]
            )
        ));
        assert!(equal_prk(
            &updated.server,
            &hkdf::Prk::new_less_safe(
                hkdf::HKDF_SHA256,
                &[
                    0xeb, 0x7f, 0x5e, 0x2a, 0x12, 0x3f, 0x40, 0x7d, 0xb4, 0x99, 0xe3, 0x61, 0xca,
                    0xe5, 0x90, 0xd4, 0xd9, 0x92, 0xe1, 0x4b, 0x7a, 0xce, 0x3, 0xc2, 0x44, 0xe0,
                    0x42, 0x21, 0x15, 0xb6, 0xd3, 0x8a
                ]
            )
        ));
    }
}

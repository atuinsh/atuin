use crate::cipher::{Iv, IvLen};
use crate::error::TLSError;
use crate::msgs::base::PayloadU8;
use crate::KeyLog;
/// Key schedule maintenance for TLS1.3
use ring::{
    aead, digest,
    hkdf::{self, KeyType as _},
    hmac,
};

/// The kinds of secret we can extract from `KeySchedule`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SecretKind {
    ResumptionPSKBinderKey,
    ClientEarlyTrafficSecret,
    ClientHandshakeTrafficSecret,
    ServerHandshakeTrafficSecret,
    ClientApplicationTrafficSecret,
    ServerApplicationTrafficSecret,
    ExporterMasterSecret,
    ResumptionMasterSecret,
    DerivedSecret,
}

impl SecretKind {
    fn to_bytes(self) -> &'static [u8] {
        match self {
            SecretKind::ResumptionPSKBinderKey => b"res binder",
            SecretKind::ClientEarlyTrafficSecret => b"c e traffic",
            SecretKind::ClientHandshakeTrafficSecret => b"c hs traffic",
            SecretKind::ServerHandshakeTrafficSecret => b"s hs traffic",
            SecretKind::ClientApplicationTrafficSecret => b"c ap traffic",
            SecretKind::ServerApplicationTrafficSecret => b"s ap traffic",
            SecretKind::ExporterMasterSecret => b"exp master",
            SecretKind::ResumptionMasterSecret => b"res master",
            SecretKind::DerivedSecret => b"derived",
        }
    }

    fn log_label(self) -> Option<&'static str> {
        use self::SecretKind::*;
        Some(match self {
            ClientEarlyTrafficSecret => "CLIENT_EARLY_TRAFFIC_SECRET",
            ClientHandshakeTrafficSecret => "CLIENT_HANDSHAKE_TRAFFIC_SECRET",
            ServerHandshakeTrafficSecret => "SERVER_HANDSHAKE_TRAFFIC_SECRET",
            ClientApplicationTrafficSecret => "CLIENT_TRAFFIC_SECRET_0",
            ServerApplicationTrafficSecret => "SERVER_TRAFFIC_SECRET_0",
            ExporterMasterSecret => "EXPORTER_SECRET",
            _ => {
                return None;
            }
        })
    }
}

/// This is the TLS1.3 key schedule.  It stores the current secret and
/// the type of hash.  This isn't used directly; but only through the
/// typestates.
struct KeySchedule {
    current: hkdf::Prk,
    algorithm: ring::hkdf::Algorithm,
}

// We express the state of a contained KeySchedule using these
// typestates.  This means we can write code that cannot accidentally
// (eg) encrypt application data using a KeySchedule solely constructed
// with an empty or trivial secret, or extract the wrong kind of secrets
// at a given point.

/// KeySchedule for early data stage.
pub struct KeyScheduleEarly {
    ks: KeySchedule,
}

impl KeyScheduleEarly {
    pub fn new(algorithm: hkdf::Algorithm, secret: &[u8]) -> KeyScheduleEarly {
        KeyScheduleEarly {
            ks: KeySchedule::new(algorithm, secret),
        }
    }

    pub fn client_early_traffic_secret(
        &self,
        hs_hash: &[u8],
        key_log: &dyn KeyLog,
        client_random: &[u8; 32],
    ) -> hkdf::Prk {
        self.ks.derive_logged_secret(
            SecretKind::ClientEarlyTrafficSecret,
            hs_hash,
            key_log,
            client_random,
        )
    }

    pub fn resumption_psk_binder_key_and_sign_verify_data(&self, hs_hash: &[u8]) -> Vec<u8> {
        let resumption_psk_binder_key = self
            .ks
            .derive_for_empty_hash(SecretKind::ResumptionPSKBinderKey);
        self.ks
            .sign_verify_data(&resumption_psk_binder_key, hs_hash)
    }

    pub fn into_handshake(mut self, secret: &[u8]) -> KeyScheduleHandshake {
        self.ks.input_secret(secret);
        KeyScheduleHandshake {
            ks: self.ks,
            current_client_traffic_secret: None,
            current_server_traffic_secret: None,
        }
    }
}

/// KeySchedule for skipping early data stage.  No secrets can be extracted
/// (since there are none), but the handshake secret can be input.
pub struct KeyScheduleNonSecret {
    ks: KeySchedule,
}

impl KeyScheduleNonSecret {
    pub fn new(algorithm: hkdf::Algorithm) -> KeyScheduleNonSecret {
        KeyScheduleNonSecret {
            ks: KeySchedule::new_with_empty_secret(algorithm),
        }
    }

    pub fn into_handshake(mut self, secret: &[u8]) -> KeyScheduleHandshake {
        self.ks.input_secret(secret);
        KeyScheduleHandshake {
            ks: self.ks,
            current_client_traffic_secret: None,
            current_server_traffic_secret: None,
        }
    }
}

/// KeySchedule during handshake.
pub struct KeyScheduleHandshake {
    ks: KeySchedule,
    current_client_traffic_secret: Option<hkdf::Prk>,
    current_server_traffic_secret: Option<hkdf::Prk>,
}

impl KeyScheduleHandshake {
    pub fn client_handshake_traffic_secret(
        &mut self,
        hs_hash: &[u8],
        key_log: &dyn KeyLog,
        client_random: &[u8; 32],
    ) -> hkdf::Prk {
        let secret = self.ks.derive_logged_secret(
            SecretKind::ClientHandshakeTrafficSecret,
            hs_hash,
            key_log,
            client_random,
        );
        self.current_client_traffic_secret = Some(secret.clone());
        secret
    }

    pub fn server_handshake_traffic_secret(
        &mut self,
        hs_hash: &[u8],
        key_log: &dyn KeyLog,
        client_random: &[u8; 32],
    ) -> hkdf::Prk {
        let secret = self.ks.derive_logged_secret(
            SecretKind::ServerHandshakeTrafficSecret,
            hs_hash,
            key_log,
            client_random,
        );
        self.current_server_traffic_secret = Some(secret.clone());
        secret
    }

    pub fn sign_server_finish(&self, hs_hash: &[u8]) -> Vec<u8> {
        self.ks.sign_finish(
            self.current_server_traffic_secret
                .as_ref()
                .unwrap(),
            hs_hash,
        )
    }

    pub fn into_traffic_with_client_finished_pending(
        mut self,
    ) -> KeyScheduleTrafficWithClientFinishedPending {
        self.ks.input_empty();
        KeyScheduleTrafficWithClientFinishedPending {
            ks: self.ks,
            handshake_client_traffic_secret: self
                .current_client_traffic_secret
                .unwrap(),
            current_client_traffic_secret: None,
            current_server_traffic_secret: None,
            current_exporter_secret: None,
        }
    }
}

/// KeySchedule during traffic stage, retaining the ability to calculate the client's
/// finished verify_data, and incrementally generate the first traffic keys.
pub struct KeyScheduleTrafficWithClientFinishedPending {
    ks: KeySchedule,
    handshake_client_traffic_secret: hkdf::Prk,
    current_client_traffic_secret: Option<hkdf::Prk>,
    current_server_traffic_secret: Option<hkdf::Prk>,
    current_exporter_secret: Option<hkdf::Prk>,
}

impl KeyScheduleTrafficWithClientFinishedPending {
    pub fn sign_client_finish(&self, hs_hash: &[u8]) -> Vec<u8> {
        self.ks
            .sign_finish(&self.handshake_client_traffic_secret, hs_hash)
    }

    pub fn server_application_traffic_secret(
        &mut self,
        hs_hash: &[u8],
        key_log: &dyn KeyLog,
        client_random: &[u8; 32],
    ) -> hkdf::Prk {
        let secret = self.ks.derive_logged_secret(
            SecretKind::ServerApplicationTrafficSecret,
            hs_hash,
            key_log,
            client_random,
        );
        self.current_server_traffic_secret = Some(secret.clone());
        secret
    }

    pub fn client_application_traffic_secret(
        &mut self,
        hs_hash: &[u8],
        key_log: &dyn KeyLog,
        client_random: &[u8; 32],
    ) -> hkdf::Prk {
        let secret = self.ks.derive_logged_secret(
            SecretKind::ClientApplicationTrafficSecret,
            hs_hash,
            key_log,
            client_random,
        );
        self.current_client_traffic_secret = Some(secret.clone());
        secret
    }

    pub fn exporter_master_secret(
        &mut self,
        hs_hash: &[u8],
        key_log: &dyn KeyLog,
        client_random: &[u8; 32],
    ) {
        let secret = self.ks.derive_logged_secret(
            SecretKind::ExporterMasterSecret,
            hs_hash,
            key_log,
            client_random,
        );
        self.current_exporter_secret = Some(secret);
    }

    pub fn into_traffic(self) -> KeyScheduleTraffic {
        KeyScheduleTraffic {
            ks: self.ks,
            current_client_traffic_secret: self
                .current_client_traffic_secret
                .unwrap(),
            current_server_traffic_secret: self
                .current_server_traffic_secret
                .unwrap(),
            current_exporter_secret: self.current_exporter_secret.unwrap(),
        }
    }
}

/// KeySchedule during traffic stage.  All traffic & exporter keys are guaranteed
/// to be available.
pub struct KeyScheduleTraffic {
    ks: KeySchedule,
    current_client_traffic_secret: hkdf::Prk,
    current_server_traffic_secret: hkdf::Prk,
    current_exporter_secret: hkdf::Prk,
}

impl KeyScheduleTraffic {
    pub fn next_server_application_traffic_secret(&mut self) -> hkdf::Prk {
        let secret = self
            .ks
            .derive_next(&self.current_server_traffic_secret);
        self.current_server_traffic_secret = secret.clone();
        secret
    }

    pub fn next_client_application_traffic_secret(&mut self) -> hkdf::Prk {
        let secret = self
            .ks
            .derive_next(&self.current_client_traffic_secret);
        self.current_client_traffic_secret = secret.clone();
        secret
    }

    pub fn resumption_master_secret_and_derive_ticket_psk(
        &self,
        hs_hash: &[u8],
        nonce: &[u8],
    ) -> Vec<u8> {
        let resumption_master_secret = self.ks.derive(
            self.ks.algorithm(),
            SecretKind::ResumptionMasterSecret,
            hs_hash,
        );
        self.ks
            .derive_ticket_psk(&resumption_master_secret, nonce)
    }

    pub fn export_keying_material(
        &self,
        out: &mut [u8],
        label: &[u8],
        context: Option<&[u8]>,
    ) -> Result<(), TLSError> {
        self.ks
            .export_keying_material(&self.current_exporter_secret, out, label, context)
    }
}

impl KeySchedule {
    fn new(algorithm: hkdf::Algorithm, secret: &[u8]) -> KeySchedule {
        let zeroes = [0u8; digest::MAX_OUTPUT_LEN];
        let zeroes = &zeroes[..algorithm.len()];
        let salt = hkdf::Salt::new(algorithm, &zeroes);
        KeySchedule {
            current: salt.extract(secret),
            algorithm,
        }
    }

    #[inline]
    fn algorithm(&self) -> hkdf::Algorithm {
        self.algorithm
    }

    fn new_with_empty_secret(algorithm: hkdf::Algorithm) -> KeySchedule {
        let zeroes = [0u8; digest::MAX_OUTPUT_LEN];
        Self::new(algorithm, &zeroes[..algorithm.len()])
    }

    /// Input the empty secret.
    fn input_empty(&mut self) {
        let zeroes = [0u8; digest::MAX_OUTPUT_LEN];
        self.input_secret(&zeroes[..self.algorithm.len()]);
    }

    /// Input the given secret.
    fn input_secret(&mut self, secret: &[u8]) {
        let salt: hkdf::Salt = self.derive_for_empty_hash(SecretKind::DerivedSecret);
        self.current = salt.extract(secret);
    }

    /// Derive a secret of given `kind`, using current handshake hash `hs_hash`.
    fn derive<T, L>(&self, key_type: L, kind: SecretKind, hs_hash: &[u8]) -> T
    where
        T: for<'a> From<hkdf::Okm<'a, L>>,
        L: hkdf::KeyType,
    {
        hkdf_expand(&self.current, key_type, kind.to_bytes(), hs_hash)
    }

    fn derive_logged_secret(
        &self,
        kind: SecretKind,
        hs_hash: &[u8],
        key_log: &dyn KeyLog,
        client_random: &[u8; 32],
    ) -> hkdf::Prk {
        let log_label = kind
            .log_label()
            .expect("not a loggable secret");
        if key_log.will_log(log_label) {
            let secret = self
                .derive::<PayloadU8, _>(PayloadU8Len(self.algorithm.len()), kind, hs_hash)
                .into_inner();
            key_log.log(log_label, client_random, &secret);
        }
        self.derive(self.algorithm, kind, hs_hash)
    }

    /// Derive a secret of given `kind` using the hash of the empty string
    /// for the handshake hash.  Useful only for
    /// `SecretKind::ResumptionPSKBinderKey` and
    /// `SecretKind::DerivedSecret`.
    fn derive_for_empty_hash<T>(&self, kind: SecretKind) -> T
    where
        T: for<'a> From<hkdf::Okm<'a, hkdf::Algorithm>>,
    {
        let digest_alg = self
            .algorithm
            .hmac_algorithm()
            .digest_algorithm();
        let empty_hash = digest::digest(digest_alg, &[]);
        self.derive(self.algorithm, kind, empty_hash.as_ref())
    }

    /// Sign the finished message consisting of `hs_hash` using a current
    /// traffic secret.
    fn sign_finish(&self, base_key: &hkdf::Prk, hs_hash: &[u8]) -> Vec<u8> {
        self.sign_verify_data(base_key, hs_hash)
    }

    /// Sign the finished message consisting of `hs_hash` using the key material
    /// `base_key`.
    fn sign_verify_data(&self, base_key: &hkdf::Prk, hs_hash: &[u8]) -> Vec<u8> {
        let hmac_alg = self.algorithm.hmac_algorithm();
        let hmac_key = hkdf_expand(base_key, hmac_alg, b"finished", &[]);
        hmac::sign(&hmac_key, hs_hash)
            .as_ref()
            .to_vec()
    }

    /// Derive the next application traffic secret, returning it.
    fn derive_next(&self, base_key: &hkdf::Prk) -> hkdf::Prk {
        hkdf_expand(&base_key, self.algorithm, b"traffic upd", &[])
    }

    /// Derive the PSK to use given a resumption_master_secret and
    /// ticket_nonce.
    fn derive_ticket_psk(&self, rms: &hkdf::Prk, nonce: &[u8]) -> Vec<u8> {
        let payload: PayloadU8 = hkdf_expand(
            rms,
            PayloadU8Len(self.algorithm.len()),
            b"resumption",
            nonce,
        );
        payload.into_inner()
    }

    fn export_keying_material(
        &self,
        current_exporter_secret: &hkdf::Prk,
        out: &mut [u8],
        label: &[u8],
        context: Option<&[u8]>,
    ) -> Result<(), TLSError> {
        let digest_alg = self
            .algorithm
            .hmac_algorithm()
            .digest_algorithm();

        let h_empty = digest::digest(digest_alg, &[]);
        let secret: hkdf::Prk = hkdf_expand(
            current_exporter_secret,
            self.algorithm,
            label,
            h_empty.as_ref(),
        );

        let h_context = digest::digest(digest_alg, context.unwrap_or(&[]));

        // TODO: Test what happens when this fails
        hkdf_expand_info(
            &secret,
            PayloadU8Len(out.len()),
            b"exporter",
            h_context.as_ref(),
            |okm| okm.fill(out),
        )
        .map_err(|_| TLSError::General("exporting too much".to_string()))
    }
}

pub(crate) fn hkdf_expand<T, L>(secret: &hkdf::Prk, key_type: L, label: &[u8], context: &[u8]) -> T
where
    T: for<'a> From<hkdf::Okm<'a, L>>,
    L: hkdf::KeyType,
{
    hkdf_expand_info(secret, key_type, label, context, |okm| okm.into())
}

fn hkdf_expand_info<F, T, L>(
    secret: &hkdf::Prk,
    key_type: L,
    label: &[u8],
    context: &[u8],
    f: F,
) -> T
where
    F: for<'b> FnOnce(hkdf::Okm<'b, L>) -> T,
    L: hkdf::KeyType,
{
    const LABEL_PREFIX: &[u8] = b"tls13 ";

    let output_len = u16::to_be_bytes(key_type.len() as u16);
    let label_len = u8::to_be_bytes((LABEL_PREFIX.len() + label.len()) as u8);
    let context_len = u8::to_be_bytes(context.len() as u8);

    let info = &[
        &output_len[..],
        &label_len[..],
        LABEL_PREFIX,
        label,
        &context_len[..],
        context,
    ];
    let okm = secret.expand(info, key_type).unwrap();

    f(okm)
}

pub(crate) struct PayloadU8Len(pub(crate) usize);
impl hkdf::KeyType for PayloadU8Len {
    fn len(&self) -> usize {
        self.0
    }
}

impl From<hkdf::Okm<'_, PayloadU8Len>> for PayloadU8 {
    fn from(okm: hkdf::Okm<PayloadU8Len>) -> Self {
        let mut r = vec![0u8; okm.len().0];
        okm.fill(&mut r[..]).unwrap();
        PayloadU8::new(r)
    }
}

pub fn derive_traffic_key(
    secret: &hkdf::Prk,
    aead_algorithm: &'static aead::Algorithm,
) -> aead::UnboundKey {
    hkdf_expand(secret, aead_algorithm, b"key", &[])
}

pub(crate) fn derive_traffic_iv(secret: &hkdf::Prk) -> Iv {
    hkdf_expand(secret, IvLen, b"iv", &[])
}

#[cfg(test)]
mod test {
    use super::{derive_traffic_iv, derive_traffic_key, KeySchedule, SecretKind};
    use crate::KeyLog;
    use ring::{aead, hkdf};

    #[test]
    fn test_vectors() {
        /* These test vectors generated with OpenSSL. */
        let hs_start_hash = [
            0xec, 0x14, 0x7a, 0x06, 0xde, 0xa3, 0xc8, 0x84, 0x6c, 0x02, 0xb2, 0x23, 0x8e, 0x41,
            0xbd, 0xdc, 0x9d, 0x89, 0xf9, 0xae, 0xa1, 0x7b, 0x5e, 0xfd, 0x4d, 0x74, 0x82, 0xaf,
            0x75, 0x88, 0x1c, 0x0a,
        ];

        let hs_full_hash = [
            0x75, 0x1a, 0x3d, 0x4a, 0x14, 0xdf, 0xab, 0xeb, 0x68, 0xe9, 0x2c, 0xa5, 0x91, 0x8e,
            0x24, 0x08, 0xb9, 0xbc, 0xb0, 0x74, 0x89, 0x82, 0xec, 0x9c, 0x32, 0x30, 0xac, 0x30,
            0xbb, 0xeb, 0x23, 0xe2,
        ];

        let ecdhe_secret = [
            0xe7, 0xb8, 0xfe, 0xf8, 0x90, 0x3b, 0x52, 0x0c, 0xb9, 0xa1, 0x89, 0x71, 0xb6, 0x9d,
            0xd4, 0x5d, 0xca, 0x53, 0xce, 0x2f, 0x12, 0xbf, 0x3b, 0xef, 0x93, 0x15, 0xe3, 0x12,
            0x71, 0xdf, 0x4b, 0x40,
        ];

        let client_hts = [
            0x61, 0x7b, 0x35, 0x07, 0x6b, 0x9d, 0x0e, 0x08, 0xcf, 0x73, 0x1d, 0x94, 0xa8, 0x66,
            0x14, 0x78, 0x41, 0x09, 0xef, 0x25, 0x55, 0x51, 0x92, 0x1d, 0xd4, 0x6e, 0x04, 0x01,
            0x35, 0xcf, 0x46, 0xab,
        ];

        let client_hts_key = [
            0x62, 0xd0, 0xdd, 0x00, 0xf6, 0x96, 0x19, 0xd3, 0xb8, 0x19, 0x3a, 0xb4, 0xa0, 0x95,
            0x85, 0xa7,
        ];

        let client_hts_iv = [
            0xff, 0xf7, 0x5d, 0xf5, 0xad, 0x35, 0xd5, 0xcb, 0x3c, 0x53, 0xf3, 0xa9,
        ];

        let server_hts = [
            0xfc, 0xf7, 0xdf, 0xe6, 0x4f, 0xa2, 0xc0, 0x4f, 0x62, 0x35, 0x38, 0x7f, 0x43, 0x4e,
            0x01, 0x42, 0x23, 0x36, 0xd9, 0xc0, 0x39, 0xde, 0x68, 0x47, 0xa0, 0xb9, 0xdd, 0xcf,
            0x29, 0xa8, 0x87, 0x59,
        ];

        let server_hts_key = [
            0x04, 0x67, 0xf3, 0x16, 0xa8, 0x05, 0xb8, 0xc4, 0x97, 0xee, 0x67, 0x04, 0x7b, 0xbc,
            0xbc, 0x54,
        ];

        let server_hts_iv = [
            0xde, 0x83, 0xa7, 0x3e, 0x9d, 0x81, 0x4b, 0x04, 0xc4, 0x8b, 0x78, 0x09,
        ];

        let client_ats = [
            0xc1, 0x4a, 0x6d, 0x79, 0x76, 0xd8, 0x10, 0x2b, 0x5a, 0x0c, 0x99, 0x51, 0x49, 0x3f,
            0xee, 0x87, 0xdc, 0xaf, 0xf8, 0x2c, 0x24, 0xca, 0xb2, 0x14, 0xe8, 0xbe, 0x71, 0xa8,
            0x20, 0x6d, 0xbd, 0xa5,
        ];

        let client_ats_key = [
            0xcc, 0x9f, 0x5f, 0x98, 0x0b, 0x5f, 0x10, 0x30, 0x6c, 0xba, 0xd7, 0xbe, 0x98, 0xd7,
            0x57, 0x2e,
        ];

        let client_ats_iv = [
            0xb8, 0x09, 0x29, 0xe8, 0xd0, 0x2c, 0x70, 0xf6, 0x11, 0x62, 0xed, 0x6b,
        ];

        let server_ats = [
            0x2c, 0x90, 0x77, 0x38, 0xd3, 0xf8, 0x37, 0x02, 0xd1, 0xe4, 0x59, 0x8f, 0x48, 0x48,
            0x53, 0x1d, 0x9f, 0x93, 0x65, 0x49, 0x1b, 0x9f, 0x7f, 0x52, 0xc8, 0x22, 0x29, 0x0d,
            0x4c, 0x23, 0x21, 0x92,
        ];

        let server_ats_key = [
            0x0c, 0xb2, 0x95, 0x62, 0xd8, 0xd8, 0x8f, 0x48, 0xb0, 0x2c, 0xbf, 0xbe, 0xd7, 0xe6,
            0x2b, 0xb3,
        ];

        let server_ats_iv = [
            0x0d, 0xb2, 0x8f, 0x98, 0x85, 0x86, 0xa1, 0xb7, 0xe4, 0xd5, 0xc6, 0x9c,
        ];

        let hkdf = hkdf::HKDF_SHA256;
        let mut ks = KeySchedule::new_with_empty_secret(hkdf);
        ks.input_secret(&ecdhe_secret);

        assert_traffic_secret(
            &ks,
            SecretKind::ClientHandshakeTrafficSecret,
            &hs_start_hash,
            &client_hts,
            &client_hts_key,
            &client_hts_iv,
        );

        assert_traffic_secret(
            &ks,
            SecretKind::ServerHandshakeTrafficSecret,
            &hs_start_hash,
            &server_hts,
            &server_hts_key,
            &server_hts_iv,
        );

        ks.input_empty();

        assert_traffic_secret(
            &ks,
            SecretKind::ClientApplicationTrafficSecret,
            &hs_full_hash,
            &client_ats,
            &client_ats_key,
            &client_ats_iv,
        );

        assert_traffic_secret(
            &ks,
            SecretKind::ServerApplicationTrafficSecret,
            &hs_full_hash,
            &server_ats,
            &server_ats_key,
            &server_ats_iv,
        );
    }

    fn assert_traffic_secret(
        ks: &KeySchedule,
        kind: SecretKind,
        hash: &[u8],
        expected_traffic_secret: &[u8],
        expected_key: &[u8],
        expected_iv: &[u8],
    ) {
        struct Log<'a>(&'a [u8]);
        impl KeyLog for Log<'_> {
            fn log(&self, _label: &str, _client_random: &[u8], secret: &[u8]) {
                assert_eq!(self.0, secret);
            }
        }
        let log = Log(expected_traffic_secret);
        let traffic_secret = ks.derive_logged_secret(kind, &hash, &log, &[0; 32]);

        // Since we can't test key equality, we test the output of sealing with the key instead.
        let aead_alg = &aead::AES_128_GCM;
        let key = derive_traffic_key(&traffic_secret, aead_alg);
        let seal_output = seal_zeroes(key);
        let expected_key = aead::UnboundKey::new(aead_alg, expected_key).unwrap();
        let expected_seal_output = seal_zeroes(expected_key);
        assert_eq!(seal_output, expected_seal_output);
        assert!(seal_output.len() >= 48); // Sanity check.

        let iv = derive_traffic_iv(&traffic_secret);
        assert_eq!(iv.value(), expected_iv);
    }

    fn seal_zeroes(key: aead::UnboundKey) -> Vec<u8> {
        let key = aead::LessSafeKey::new(key);
        let mut seal_output = vec![0; 32];
        key.seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key([0; aead::NONCE_LEN]),
            aead::Aad::empty(),
            &mut seal_output,
        )
        .unwrap();
        seal_output
    }
}

use crate::check::check_message;
use crate::error::TLSError;
#[cfg(feature = "logging")]
use crate::log::{debug, trace};
use crate::msgs::base::Payload;
use crate::msgs::ccs::ChangeCipherSpecPayload;
use crate::msgs::codec::Codec;
use crate::msgs::enums::AlertDescription;
use crate::msgs::enums::{ContentType, HandshakeType, ProtocolVersion};
use crate::msgs::handshake::HandshakeMessagePayload;
use crate::msgs::handshake::HandshakePayload;
use crate::msgs::handshake::NewSessionTicketPayload;
use crate::msgs::message::{Message, MessagePayload};
use crate::msgs::persist;
use crate::server::ServerSessionImpl;
use crate::session::SessionSecrets;
use crate::verify;

use crate::server::common::{ClientCertDetails, HandshakeDetails, ServerKXDetails};
use crate::server::hs;

use ring::constant_time;

// --- Process client's Certificate for client auth ---
pub struct ExpectCertificate {
    pub handshake: HandshakeDetails,
    pub server_kx: ServerKXDetails,
    pub send_ticket: bool,
}

impl ExpectCertificate {
    fn into_expect_tls12_client_kx(self, cert: Option<ClientCertDetails>) -> hs::NextState {
        Box::new(ExpectClientKX {
            handshake: self.handshake,
            server_kx: self.server_kx,
            client_cert: cert,
            send_ticket: self.send_ticket,
        })
    }
}

impl hs::State for ExpectCertificate {
    fn handle(
        mut self: Box<Self>,
        sess: &mut ServerSessionImpl,
        m: Message,
    ) -> hs::NextStateOrError {
        let cert_chain =
            require_handshake_msg!(m, HandshakeType::Certificate, HandshakePayload::Certificate)?;
        self.handshake
            .transcript
            .add_message(&m);

        // If we can't determine if the auth is mandatory, abort
        let mandatory = sess
            .config
            .verifier
            .client_auth_mandatory(sess.get_sni())
            .ok_or_else(|| {
                debug!("could not determine if client auth is mandatory based on SNI");
                sess.common
                    .send_fatal_alert(AlertDescription::AccessDenied);
                TLSError::General("client rejected by client_auth_mandatory".into())
            })?;

        if cert_chain.is_empty() {
            if !mandatory {
                debug!("client auth requested but no certificate supplied");
                self.handshake
                    .transcript
                    .abandon_client_auth();
                return Ok(self.into_expect_tls12_client_kx(None));
            }
            sess.common
                .send_fatal_alert(AlertDescription::CertificateRequired);
            return Err(TLSError::NoCertificatesPresented);
        }

        trace!("certs {:?}", cert_chain);

        sess.config
            .verifier
            .verify_client_cert(cert_chain, sess.get_sni())
            .or_else(|err| {
                hs::incompatible(sess, "certificate invalid");
                Err(err)
            })?;

        let cert = ClientCertDetails::new(cert_chain.clone());
        Ok(self.into_expect_tls12_client_kx(Some(cert)))
    }
}

// --- Process client's KeyExchange ---
pub struct ExpectClientKX {
    pub handshake: HandshakeDetails,
    pub server_kx: ServerKXDetails,
    pub client_cert: Option<ClientCertDetails>,
    pub send_ticket: bool,
}

impl ExpectClientKX {
    fn into_expect_tls12_certificate_verify(self, secrets: SessionSecrets) -> hs::NextState {
        Box::new(ExpectCertificateVerify {
            secrets,
            handshake: self.handshake,
            client_cert: self.client_cert.unwrap(),
            send_ticket: self.send_ticket,
        })
    }

    fn into_expect_tls12_ccs(self, secrets: SessionSecrets) -> hs::NextState {
        Box::new(ExpectCCS {
            secrets,
            handshake: self.handshake,
            resuming: false,
            send_ticket: self.send_ticket,
        })
    }
}

impl hs::State for ExpectClientKX {
    fn handle(
        mut self: Box<Self>,
        sess: &mut ServerSessionImpl,
        m: Message,
    ) -> hs::NextStateOrError {
        let client_kx = require_handshake_msg!(
            m,
            HandshakeType::ClientKeyExchange,
            HandshakePayload::ClientKeyExchange
        )?;
        self.handshake
            .transcript
            .add_message(&m);

        // Complete key agreement, and set up encryption with the
        // resulting premaster secret.
        let kx = self.server_kx.take_kx();
        if !kx.check_client_params(&client_kx.0) {
            sess.common
                .send_fatal_alert(AlertDescription::DecodeError);
            return Err(TLSError::CorruptMessagePayload(ContentType::Handshake));
        }

        let kxd = kx
            .server_complete(&client_kx.0)
            .ok_or_else(|| {
                TLSError::PeerMisbehavedError("key exchange completion failed".to_string())
            })?;

        let hashalg = sess
            .common
            .get_suite_assert()
            .get_hash();
        let secrets = if self.handshake.using_ems {
            let handshake_hash = self
                .handshake
                .transcript
                .get_current_hash();
            SessionSecrets::new_ems(
                &self.handshake.randoms,
                &handshake_hash,
                hashalg,
                &kxd.shared_secret,
            )
        } else {
            SessionSecrets::new(&self.handshake.randoms, hashalg, &kxd.shared_secret)
        };
        sess.config.key_log.log(
            "CLIENT_RANDOM",
            &secrets.randoms.client,
            &secrets.master_secret,
        );
        sess.common
            .start_encryption_tls12(&secrets);

        if self.client_cert.is_some() {
            Ok(self.into_expect_tls12_certificate_verify(secrets))
        } else {
            Ok(self.into_expect_tls12_ccs(secrets))
        }
    }
}

// --- Process client's certificate proof ---
pub struct ExpectCertificateVerify {
    secrets: SessionSecrets,
    handshake: HandshakeDetails,
    client_cert: ClientCertDetails,
    send_ticket: bool,
}

impl ExpectCertificateVerify {
    fn into_expect_tls12_ccs(self) -> hs::NextState {
        Box::new(ExpectCCS {
            secrets: self.secrets,
            handshake: self.handshake,
            resuming: false,
            send_ticket: self.send_ticket,
        })
    }
}

impl hs::State for ExpectCertificateVerify {
    fn handle(
        mut self: Box<Self>,
        sess: &mut ServerSessionImpl,
        m: Message,
    ) -> hs::NextStateOrError {
        let rc = {
            let sig = require_handshake_msg!(
                m,
                HandshakeType::CertificateVerify,
                HandshakePayload::CertificateVerify
            )?;
            let handshake_msgs = self
                .handshake
                .transcript
                .take_handshake_buf();
            let certs = &self.client_cert.cert_chain;

            sess.config
                .get_verifier()
                .verify_tls12_signature(&handshake_msgs, &certs[0], sig)
        };

        if let Err(e) = rc {
            sess.common
                .send_fatal_alert(AlertDescription::AccessDenied);
            return Err(e);
        }

        trace!("client CertificateVerify OK");
        sess.client_cert_chain = Some(self.client_cert.take_chain());

        self.handshake
            .transcript
            .add_message(&m);
        Ok(self.into_expect_tls12_ccs())
    }
}

// --- Process client's ChangeCipherSpec ---
pub struct ExpectCCS {
    pub secrets: SessionSecrets,
    pub handshake: HandshakeDetails,
    pub resuming: bool,
    pub send_ticket: bool,
}

impl ExpectCCS {
    fn into_expect_tls12_finished(self) -> hs::NextState {
        Box::new(ExpectFinished {
            secrets: self.secrets,
            handshake: self.handshake,
            resuming: self.resuming,
            send_ticket: self.send_ticket,
        })
    }
}

impl hs::State for ExpectCCS {
    fn handle(self: Box<Self>, sess: &mut ServerSessionImpl, m: Message) -> hs::NextStateOrError {
        check_message(&m, &[ContentType::ChangeCipherSpec], &[])?;

        // CCS should not be received interleaved with fragmented handshake-level
        // message.
        hs::check_aligned_handshake(sess)?;

        sess.common
            .record_layer
            .start_decrypting();
        Ok(self.into_expect_tls12_finished())
    }
}

// --- Process client's Finished ---
fn get_server_session_value_tls12(
    secrets: &SessionSecrets,
    handshake: &HandshakeDetails,
    sess: &ServerSessionImpl,
) -> persist::ServerSessionValue {
    let scs = sess.common.get_suite_assert();
    let version = ProtocolVersion::TLSv1_2;
    let secret = secrets.get_master_secret();

    let mut v = persist::ServerSessionValue::new(
        sess.get_sni(),
        version,
        scs.suite,
        secret,
        &sess.client_cert_chain,
        sess.alpn_protocol.clone(),
        sess.resumption_data.clone(),
    );

    if handshake.using_ems {
        v.set_extended_ms_used();
    }

    v
}

pub fn emit_ticket(
    secrets: &SessionSecrets,
    handshake: &mut HandshakeDetails,
    sess: &mut ServerSessionImpl,
) {
    // If we can't produce a ticket for some reason, we can't
    // report an error. Send an empty one.
    let plain = get_server_session_value_tls12(secrets, handshake, sess).get_encoding();
    let ticket = sess
        .config
        .ticketer
        .encrypt(&plain)
        .unwrap_or_else(Vec::new);
    let ticket_lifetime = sess.config.ticketer.get_lifetime();

    let m = Message {
        typ: ContentType::Handshake,
        version: ProtocolVersion::TLSv1_2,
        payload: MessagePayload::Handshake(HandshakeMessagePayload {
            typ: HandshakeType::NewSessionTicket,
            payload: HandshakePayload::NewSessionTicket(NewSessionTicketPayload::new(
                ticket_lifetime,
                ticket,
            )),
        }),
    };

    handshake.transcript.add_message(&m);
    sess.common.send_msg(m, false);
}

pub fn emit_ccs(sess: &mut ServerSessionImpl) {
    let m = Message {
        typ: ContentType::ChangeCipherSpec,
        version: ProtocolVersion::TLSv1_2,
        payload: MessagePayload::ChangeCipherSpec(ChangeCipherSpecPayload {}),
    };

    sess.common.send_msg(m, false);
}

pub fn emit_finished(
    secrets: &SessionSecrets,
    handshake: &mut HandshakeDetails,
    sess: &mut ServerSessionImpl,
) {
    let vh = handshake.transcript.get_current_hash();
    let verify_data = secrets.server_verify_data(&vh);
    let verify_data_payload = Payload::new(verify_data);

    let f = Message {
        typ: ContentType::Handshake,
        version: ProtocolVersion::TLSv1_2,
        payload: MessagePayload::Handshake(HandshakeMessagePayload {
            typ: HandshakeType::Finished,
            payload: HandshakePayload::Finished(verify_data_payload),
        }),
    };

    handshake.transcript.add_message(&f);
    sess.common.send_msg(f, true);
}

pub struct ExpectFinished {
    secrets: SessionSecrets,
    handshake: HandshakeDetails,
    resuming: bool,
    send_ticket: bool,
}

impl ExpectFinished {
    fn into_expect_tls12_traffic(self, fin: verify::FinishedMessageVerified) -> hs::NextState {
        Box::new(ExpectTraffic {
            secrets: self.secrets,
            _fin_verified: fin,
        })
    }
}

impl hs::State for ExpectFinished {
    fn handle(
        mut self: Box<Self>,
        sess: &mut ServerSessionImpl,
        m: Message,
    ) -> hs::NextStateOrError {
        let finished =
            require_handshake_msg!(m, HandshakeType::Finished, HandshakePayload::Finished)?;

        hs::check_aligned_handshake(sess)?;

        let vh = self
            .handshake
            .transcript
            .get_current_hash();
        let expect_verify_data = self.secrets.client_verify_data(&vh);

        let fin = constant_time::verify_slices_are_equal(&expect_verify_data, &finished.0)
            .map_err(|_| {
                sess.common
                    .send_fatal_alert(AlertDescription::DecryptError);
                TLSError::DecryptError
            })
            .map(|_| verify::FinishedMessageVerified::assertion())?;

        // Save session, perhaps
        if !self.resuming && !self.handshake.session_id.is_empty() {
            let value = get_server_session_value_tls12(&self.secrets, &self.handshake, sess);

            let worked = sess.config.session_storage.put(
                self.handshake.session_id.get_encoding(),
                value.get_encoding(),
            );
            if worked {
                debug!("Session saved");
            } else {
                debug!("Session not saved");
            }
        }

        // Send our CCS and Finished.
        self.handshake
            .transcript
            .add_message(&m);
        if !self.resuming {
            if self.send_ticket {
                emit_ticket(&self.secrets, &mut self.handshake, sess);
            }
            emit_ccs(sess);
            sess.common
                .record_layer
                .start_encrypting();
            emit_finished(&self.secrets, &mut self.handshake, sess);
        }

        sess.common.start_traffic();
        Ok(self.into_expect_tls12_traffic(fin))
    }
}

// --- Process traffic ---
pub struct ExpectTraffic {
    secrets: SessionSecrets,
    _fin_verified: verify::FinishedMessageVerified,
}

impl ExpectTraffic {}

impl hs::State for ExpectTraffic {
    fn handle(
        self: Box<Self>,
        sess: &mut ServerSessionImpl,
        mut m: Message,
    ) -> hs::NextStateOrError {
        check_message(&m, &[ContentType::ApplicationData], &[])?;
        sess.common
            .take_received_plaintext(m.take_opaque_payload().unwrap());
        Ok(self)
    }

    fn export_keying_material(
        &self,
        output: &mut [u8],
        label: &[u8],
        context: Option<&[u8]>,
    ) -> Result<(), TLSError> {
        self.secrets
            .export_keying_material(output, label, context);
        Ok(())
    }
}

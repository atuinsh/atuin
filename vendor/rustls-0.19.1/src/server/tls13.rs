use crate::check::check_message;
use crate::cipher;
use crate::error::TLSError;
use crate::key_schedule::{
    KeyScheduleEarly, KeyScheduleHandshake, KeyScheduleNonSecret, KeyScheduleTraffic,
    KeyScheduleTrafficWithClientFinishedPending,
};
#[cfg(feature = "logging")]
use crate::log::{debug, trace, warn};
use crate::msgs::base::{Payload, PayloadU8};
use crate::msgs::ccs::ChangeCipherSpecPayload;
use crate::msgs::codec::Codec;
use crate::msgs::enums::KeyUpdateRequest;
use crate::msgs::enums::{AlertDescription, NamedGroup, SignatureScheme};
use crate::msgs::enums::{Compression, PSKKeyExchangeMode};
use crate::msgs::enums::{ContentType, HandshakeType, ProtocolVersion};
use crate::msgs::handshake::CertReqExtension;
use crate::msgs::handshake::CertificateEntry;
use crate::msgs::handshake::CertificateExtension;
use crate::msgs::handshake::CertificatePayloadTLS13;
use crate::msgs::handshake::CertificateRequestPayloadTLS13;
use crate::msgs::handshake::CertificateStatus;
use crate::msgs::handshake::ClientHelloPayload;
use crate::msgs::handshake::DigitallySignedStruct;
use crate::msgs::handshake::HandshakeMessagePayload;
use crate::msgs::handshake::HandshakePayload;
use crate::msgs::handshake::HelloRetryExtension;
use crate::msgs::handshake::HelloRetryRequest;
use crate::msgs::handshake::KeyShareEntry;
use crate::msgs::handshake::NewSessionTicketPayloadTLS13;
use crate::msgs::handshake::Random;
use crate::msgs::handshake::ServerExtension;
use crate::msgs::handshake::ServerHelloPayload;
use crate::msgs::handshake::SessionID;
use crate::msgs::message::{Message, MessagePayload};
use crate::msgs::persist;
use crate::rand;
use crate::server::ServerSessionImpl;
use crate::sign;
use crate::suites;
use crate::verify;
#[cfg(feature = "quic")]
use crate::{msgs::handshake::NewSessionTicketExtension, quic, session::Protocol};

use crate::server::common::{ClientCertDetails, HandshakeDetails};
use crate::server::hs;

use ring::constant_time;

pub struct CompleteClientHelloHandling {
    pub handshake: HandshakeDetails,
    pub done_retry: bool,
    pub send_cert_status: bool,
    pub send_sct: bool,
    pub send_ticket: bool,
}

impl CompleteClientHelloHandling {
    fn check_binder(
        &self,
        sess: &mut ServerSessionImpl,
        client_hello: &Message,
        psk: &[u8],
        binder: &[u8],
    ) -> bool {
        let binder_plaintext = match client_hello.payload {
            MessagePayload::Handshake(ref hmp) => hmp.get_encoding_for_binder_signing(),
            _ => unreachable!(),
        };

        let suite = sess.common.get_suite_assert();
        let suite_hash = suite.get_hash();
        let handshake_hash = self
            .handshake
            .transcript
            .get_hash_given(suite_hash, &binder_plaintext);

        let key_schedule = KeyScheduleEarly::new(suite.hkdf_algorithm, &psk);
        let real_binder =
            key_schedule.resumption_psk_binder_key_and_sign_verify_data(&handshake_hash);

        constant_time::verify_slices_are_equal(&real_binder, binder).is_ok()
    }

    fn into_expect_retried_client_hello(self) -> hs::NextState {
        Box::new(hs::ExpectClientHello {
            handshake: self.handshake,
            done_retry: true,
            send_cert_status: self.send_cert_status,
            send_sct: self.send_sct,
            send_ticket: self.send_ticket,
        })
    }

    fn into_expect_certificate(
        self,
        key_schedule: KeyScheduleTrafficWithClientFinishedPending,
    ) -> hs::NextState {
        Box::new(ExpectCertificate {
            handshake: self.handshake,
            key_schedule,
            send_ticket: self.send_ticket,
        })
    }

    fn into_expect_finished(
        self,
        key_schedule: KeyScheduleTrafficWithClientFinishedPending,
    ) -> hs::NextState {
        Box::new(ExpectFinished {
            handshake: self.handshake,
            key_schedule,
            send_ticket: self.send_ticket,
        })
    }

    fn emit_server_hello(
        &mut self,
        sess: &mut ServerSessionImpl,
        session_id: &SessionID,
        share: &KeyShareEntry,
        chosen_psk_idx: Option<usize>,
        resuming_psk: Option<&[u8]>,
    ) -> Result<KeyScheduleHandshake, TLSError> {
        let mut extensions = Vec::new();

        // Do key exchange
        let kxr = suites::KeyExchange::start_ecdhe(share.group)
            .and_then(|kx| kx.complete(&share.payload.0))
            .ok_or_else(|| TLSError::PeerMisbehavedError("key exchange failed".to_string()))?;

        let kse = KeyShareEntry::new(share.group, kxr.pubkey.as_ref());
        extensions.push(ServerExtension::KeyShare(kse));
        extensions.push(ServerExtension::SupportedVersions(ProtocolVersion::TLSv1_3));

        if let Some(psk_idx) = chosen_psk_idx {
            extensions.push(ServerExtension::PresharedKey(psk_idx as u16));
        }

        let sh = Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_2,
            payload: MessagePayload::Handshake(HandshakeMessagePayload {
                typ: HandshakeType::ServerHello,
                payload: HandshakePayload::ServerHello(ServerHelloPayload {
                    legacy_version: ProtocolVersion::TLSv1_2,
                    random: Random::from_slice(&self.handshake.randoms.server),
                    session_id: *session_id,
                    cipher_suite: sess.common.get_suite_assert().suite,
                    compression_method: Compression::Null,
                    extensions,
                }),
            }),
        };

        hs::check_aligned_handshake(sess)?;

        #[cfg(feature = "quic")]
        let client_hello_hash = self
            .handshake
            .transcript
            .get_hash_given(
                sess.common
                    .get_suite_assert()
                    .get_hash(),
                &[],
            );

        trace!("sending server hello {:?}", sh);
        self.handshake
            .transcript
            .add_message(&sh);
        sess.common.send_msg(sh, false);

        // Start key schedule
        let suite = sess.common.get_suite_assert();
        let mut key_schedule = if let Some(psk) = resuming_psk {
            let early_key_schedule = KeyScheduleEarly::new(suite.hkdf_algorithm, psk);

            #[cfg(feature = "quic")]
            {
                if sess.common.protocol == Protocol::Quic {
                    let client_early_traffic_secret = early_key_schedule
                        .client_early_traffic_secret(
                            &client_hello_hash,
                            &*sess.config.key_log,
                            &self.handshake.randoms.client,
                        );
                    // If 0-RTT should be rejected, this will be clobbered by ExtensionProcessing
                    // before the application can see.
                    sess.common.quic.early_secret = Some(client_early_traffic_secret);
                }
            }

            early_key_schedule.into_handshake(&kxr.shared_secret)
        } else {
            KeyScheduleNonSecret::new(suite.hkdf_algorithm).into_handshake(&kxr.shared_secret)
        };

        let handshake_hash = self
            .handshake
            .transcript
            .get_current_hash();
        let write_key = key_schedule.server_handshake_traffic_secret(
            &handshake_hash,
            &*sess.config.key_log,
            &self.handshake.randoms.client,
        );
        sess.common
            .record_layer
            .set_message_encrypter(cipher::new_tls13_write(suite, &write_key));

        let read_key = key_schedule.client_handshake_traffic_secret(
            &handshake_hash,
            &*sess.config.key_log,
            &self.handshake.randoms.client,
        );
        sess.common
            .record_layer
            .set_message_decrypter(cipher::new_tls13_read(suite, &read_key));

        #[cfg(feature = "quic")]
        {
            sess.common.quic.hs_secrets = Some(quic::Secrets {
                client: read_key,
                server: write_key,
            });
        }

        Ok(key_schedule)
    }

    fn emit_fake_ccs(&mut self, sess: &mut ServerSessionImpl) {
        if sess.common.is_quic() {
            return;
        }
        let m = Message {
            typ: ContentType::ChangeCipherSpec,
            version: ProtocolVersion::TLSv1_2,
            payload: MessagePayload::ChangeCipherSpec(ChangeCipherSpecPayload {}),
        };
        sess.common.send_msg(m, false);
    }

    fn emit_hello_retry_request(&mut self, sess: &mut ServerSessionImpl, group: NamedGroup) {
        let mut req = HelloRetryRequest {
            legacy_version: ProtocolVersion::TLSv1_2,
            session_id: SessionID::empty(),
            cipher_suite: sess.common.get_suite_assert().suite,
            extensions: Vec::new(),
        };

        req.extensions
            .push(HelloRetryExtension::KeyShare(group));
        req.extensions
            .push(HelloRetryExtension::SupportedVersions(
                ProtocolVersion::TLSv1_3,
            ));

        let m = Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_2,
            payload: MessagePayload::Handshake(HandshakeMessagePayload {
                typ: HandshakeType::HelloRetryRequest,
                payload: HandshakePayload::HelloRetryRequest(req),
            }),
        };

        trace!("Requesting retry {:?}", m);
        self.handshake
            .transcript
            .rollup_for_hrr();
        self.handshake
            .transcript
            .add_message(&m);
        sess.common.send_msg(m, false);
    }

    fn emit_encrypted_extensions(
        &mut self,
        sess: &mut ServerSessionImpl,
        server_key: &mut sign::CertifiedKey,
        hello: &ClientHelloPayload,
        resumedata: Option<&persist::ServerSessionValue>,
    ) -> Result<(), TLSError> {
        let mut ep = hs::ExtensionProcessing::new();
        ep.process_common(sess, Some(server_key), hello, resumedata, &self.handshake)?;

        self.send_cert_status = ep.send_cert_status;
        self.send_sct = ep.send_sct;

        let ee = Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_3,
            payload: MessagePayload::Handshake(HandshakeMessagePayload {
                typ: HandshakeType::EncryptedExtensions,
                payload: HandshakePayload::EncryptedExtensions(ep.exts),
            }),
        };

        trace!("sending encrypted extensions {:?}", ee);
        self.handshake
            .transcript
            .add_message(&ee);
        sess.common.send_msg(ee, true);
        Ok(())
    }

    fn emit_certificate_req_tls13(
        &mut self,
        sess: &mut ServerSessionImpl,
    ) -> Result<bool, TLSError> {
        if !sess.config.verifier.offer_client_auth() {
            return Ok(false);
        }

        let mut cr = CertificateRequestPayloadTLS13 {
            context: PayloadU8::empty(),
            extensions: Vec::new(),
        };

        let schemes = sess
            .config
            .get_verifier()
            .supported_verify_schemes();
        cr.extensions
            .push(CertReqExtension::SignatureAlgorithms(schemes.to_vec()));

        let names = sess
            .config
            .verifier
            .client_auth_root_subjects(sess.get_sni())
            .ok_or_else(|| {
                debug!("could not determine root subjects based on SNI");
                sess.common
                    .send_fatal_alert(AlertDescription::AccessDenied);
                TLSError::General("client rejected by client_auth_root_subjects".into())
            })?;

        if !names.is_empty() {
            cr.extensions
                .push(CertReqExtension::AuthorityNames(names));
        }

        let m = Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_3,
            payload: MessagePayload::Handshake(HandshakeMessagePayload {
                typ: HandshakeType::CertificateRequest,
                payload: HandshakePayload::CertificateRequestTLS13(cr),
            }),
        };

        trace!("Sending CertificateRequest {:?}", m);
        self.handshake
            .transcript
            .add_message(&m);
        sess.common.send_msg(m, true);
        Ok(true)
    }

    fn emit_certificate_tls13(
        &mut self,
        sess: &mut ServerSessionImpl,
        server_key: &mut sign::CertifiedKey,
    ) {
        let mut cert_entries = vec![];
        for cert in server_key.take_cert() {
            let entry = CertificateEntry {
                cert,
                exts: Vec::new(),
            };

            cert_entries.push(entry);
        }

        if let Some(end_entity_cert) = cert_entries.first_mut() {
            // Apply OCSP response to first certificate (we don't support OCSP
            // except for leaf certs).
            if self.send_cert_status {
                if let Some(ocsp) = server_key.take_ocsp() {
                    let cst = CertificateStatus::new(ocsp);
                    end_entity_cert
                        .exts
                        .push(CertificateExtension::CertificateStatus(cst));
                }
            }

            // Likewise, SCT
            if self.send_sct {
                if let Some(sct_list) = server_key.take_sct_list() {
                    end_entity_cert
                        .exts
                        .push(CertificateExtension::make_sct(sct_list));
                }
            }
        }

        let cert_body = CertificatePayloadTLS13::new(cert_entries);
        let c = Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_3,
            payload: MessagePayload::Handshake(HandshakeMessagePayload {
                typ: HandshakeType::Certificate,
                payload: HandshakePayload::CertificateTLS13(cert_body),
            }),
        };

        trace!("sending certificate {:?}", c);
        self.handshake
            .transcript
            .add_message(&c);
        sess.common.send_msg(c, true);
    }

    fn emit_certificate_verify_tls13(
        &mut self,
        sess: &mut ServerSessionImpl,
        server_key: &mut sign::CertifiedKey,
        schemes: &[SignatureScheme],
    ) -> Result<(), TLSError> {
        let message = verify::construct_tls13_server_verify_message(
            &self
                .handshake
                .transcript
                .get_current_hash(),
        );

        let signing_key = &server_key.key;
        let signer = signing_key
            .choose_scheme(schemes)
            .ok_or_else(|| hs::incompatible(sess, "no overlapping sigschemes"))?;

        let scheme = signer.get_scheme();
        let sig = signer.sign(&message)?;

        let cv = DigitallySignedStruct::new(scheme, sig);

        let m = Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_3,
            payload: MessagePayload::Handshake(HandshakeMessagePayload {
                typ: HandshakeType::CertificateVerify,
                payload: HandshakePayload::CertificateVerify(cv),
            }),
        };

        trace!("sending certificate-verify {:?}", m);
        self.handshake
            .transcript
            .add_message(&m);
        sess.common.send_msg(m, true);
        Ok(())
    }

    fn emit_finished_tls13(
        &mut self,
        sess: &mut ServerSessionImpl,
        key_schedule: KeyScheduleHandshake,
    ) -> KeyScheduleTrafficWithClientFinishedPending {
        let handshake_hash = self
            .handshake
            .transcript
            .get_current_hash();
        let verify_data = key_schedule.sign_server_finish(&handshake_hash);
        let verify_data_payload = Payload::new(verify_data);

        let m = Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_3,
            payload: MessagePayload::Handshake(HandshakeMessagePayload {
                typ: HandshakeType::Finished,
                payload: HandshakePayload::Finished(verify_data_payload),
            }),
        };

        trace!("sending finished {:?}", m);
        self.handshake
            .transcript
            .add_message(&m);
        self.handshake.hash_at_server_fin = self
            .handshake
            .transcript
            .get_current_hash();
        sess.common.send_msg(m, true);

        // Now move to application data keys.  Read key change is deferred until
        // the Finish message is received & validated.
        let mut key_schedule_traffic = key_schedule.into_traffic_with_client_finished_pending();
        let suite = sess.common.get_suite_assert();
        let write_key = key_schedule_traffic.server_application_traffic_secret(
            &self.handshake.hash_at_server_fin,
            &*sess.config.key_log,
            &self.handshake.randoms.client,
        );
        sess.common
            .record_layer
            .set_message_encrypter(cipher::new_tls13_write(suite, &write_key));

        key_schedule_traffic.exporter_master_secret(
            &self.handshake.hash_at_server_fin,
            &*sess.config.key_log,
            &self.handshake.randoms.client,
        );

        let _read_key = key_schedule_traffic.client_application_traffic_secret(
            &self.handshake.hash_at_server_fin,
            &*sess.config.key_log,
            &self.handshake.randoms.client,
        );

        #[cfg(feature = "quic")]
        {
            sess.common.quic.traffic_secrets = Some(quic::Secrets {
                client: _read_key,
                server: write_key,
            });
        }

        key_schedule_traffic
    }

    fn attempt_tls13_ticket_decryption(
        &mut self,
        sess: &mut ServerSessionImpl,
        ticket: &[u8],
    ) -> Option<persist::ServerSessionValue> {
        if sess.config.ticketer.enabled() {
            sess.config
                .ticketer
                .decrypt(ticket)
                .and_then(|plain| persist::ServerSessionValue::read_bytes(&plain))
        } else {
            sess.config
                .session_storage
                .take(ticket)
                .and_then(|plain| persist::ServerSessionValue::read_bytes(&plain))
        }
    }

    pub fn handle_client_hello(
        mut self,
        sess: &mut ServerSessionImpl,
        mut server_key: sign::CertifiedKey,
        chm: &Message,
    ) -> hs::NextStateOrError {
        let client_hello = require_handshake_msg!(
            chm,
            HandshakeType::ClientHello,
            HandshakePayload::ClientHello
        )?;

        if client_hello.compression_methods.len() != 1 {
            return Err(hs::illegal_param(sess, "client offered wrong compressions"));
        }

        let groups_ext = client_hello
            .get_namedgroups_extension()
            .ok_or_else(|| hs::incompatible(sess, "client didn't describe groups"))?;

        let mut sigschemes_ext = client_hello
            .get_sigalgs_extension()
            .ok_or_else(|| hs::incompatible(sess, "client didn't describe sigschemes"))?
            .clone();

        let tls13_schemes = sign::supported_sign_tls13();
        sigschemes_ext.retain(|scheme| tls13_schemes.contains(scheme));

        let shares_ext = client_hello
            .get_keyshare_extension()
            .ok_or_else(|| hs::incompatible(sess, "client didn't send keyshares"))?;

        if client_hello.has_keyshare_extension_with_duplicates() {
            return Err(hs::illegal_param(sess, "client sent duplicate keyshares"));
        }

        let share_groups: Vec<NamedGroup> = shares_ext
            .iter()
            .map(|share| share.group)
            .collect();

        let supported_groups = suites::KeyExchange::supported_groups();
        let chosen_group = supported_groups
            .iter()
            .filter(|group| share_groups.contains(group))
            .nth(0)
            .cloned();

        if chosen_group.is_none() {
            // We don't have a suitable key share.  Choose a suitable group and
            // send a HelloRetryRequest.
            let retry_group_maybe = supported_groups
                .iter()
                .filter(|group| groups_ext.contains(group))
                .nth(0)
                .cloned();
            self.handshake
                .transcript
                .add_message(chm);

            if let Some(group) = retry_group_maybe {
                if self.done_retry {
                    return Err(hs::illegal_param(sess, "did not follow retry request"));
                }

                self.emit_hello_retry_request(sess, group);
                self.emit_fake_ccs(sess);
                return Ok(self.into_expect_retried_client_hello());
            }

            return Err(hs::incompatible(sess, "no kx group overlap with client"));
        }

        let chosen_group = chosen_group.unwrap();
        let chosen_share = shares_ext
            .iter()
            .find(|share| share.group == chosen_group)
            .unwrap();

        let mut chosen_psk_index = None;
        let mut resumedata = None;
        if let Some(psk_offer) = client_hello.get_psk() {
            if !client_hello.check_psk_ext_is_last() {
                return Err(hs::illegal_param(sess, "psk extension in wrong position"));
            }

            if psk_offer.binders.is_empty() {
                return Err(hs::decode_error(sess, "psk extension missing binder"));
            }

            if psk_offer.binders.len() != psk_offer.identities.len() {
                return Err(hs::illegal_param(
                    sess,
                    "psk extension mismatched ids/binders",
                ));
            }

            for (i, psk_id) in psk_offer.identities.iter().enumerate() {
                let maybe_resume = self.attempt_tls13_ticket_decryption(sess, &psk_id.identity.0);

                if !hs::can_resume(sess, &self.handshake, &maybe_resume) {
                    continue;
                }

                let resume = maybe_resume.unwrap();

                if !self.check_binder(sess, chm, &resume.master_secret.0, &psk_offer.binders[i].0) {
                    sess.common
                        .send_fatal_alert(AlertDescription::DecryptError);
                    return Err(TLSError::PeerMisbehavedError(
                        "client sent wrong binder".to_string(),
                    ));
                }

                chosen_psk_index = Some(i);
                resumedata = Some(resume);
                break;
            }
        }

        if !client_hello.psk_mode_offered(PSKKeyExchangeMode::PSK_DHE_KE) {
            debug!("Client unwilling to resume, DHE_KE not offered");
            self.send_ticket = false;
            chosen_psk_index = None;
            resumedata = None;
        } else {
            self.send_ticket = true;
        }

        if let Some(ref resume) = resumedata {
            sess.received_resumption_data = Some(resume.application_data.0.clone());
            sess.client_cert_chain = resume.client_cert_chain.clone();
        }

        let full_handshake = resumedata.is_none();
        self.handshake
            .transcript
            .add_message(chm);
        let key_schedule = self.emit_server_hello(
            sess,
            &client_hello.session_id,
            chosen_share,
            chosen_psk_index,
            resumedata
                .as_ref()
                .map(|x| &x.master_secret.0[..]),
        )?;
        if !self.done_retry {
            self.emit_fake_ccs(sess);
        }
        self.emit_encrypted_extensions(sess, &mut server_key, client_hello, resumedata.as_ref())?;

        let doing_client_auth = if full_handshake {
            let client_auth = self.emit_certificate_req_tls13(sess)?;
            self.emit_certificate_tls13(sess, &mut server_key);
            self.emit_certificate_verify_tls13(sess, &mut server_key, &sigschemes_ext)?;
            client_auth
        } else {
            false
        };

        hs::check_aligned_handshake(sess)?;
        let key_schedule_traffic = self.emit_finished_tls13(sess, key_schedule);

        if doing_client_auth {
            Ok(self.into_expect_certificate(key_schedule_traffic))
        } else {
            Ok(self.into_expect_finished(key_schedule_traffic))
        }
    }
}

pub struct ExpectCertificate {
    pub handshake: HandshakeDetails,
    pub key_schedule: KeyScheduleTrafficWithClientFinishedPending,
    pub send_ticket: bool,
}

impl ExpectCertificate {
    fn into_expect_finished(self) -> hs::NextState {
        Box::new(ExpectFinished {
            key_schedule: self.key_schedule,
            handshake: self.handshake,
            send_ticket: self.send_ticket,
        })
    }

    fn into_expect_certificate_verify(self, cert: ClientCertDetails) -> hs::NextState {
        Box::new(ExpectCertificateVerify {
            handshake: self.handshake,
            key_schedule: self.key_schedule,
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
        let certp = require_handshake_msg!(
            m,
            HandshakeType::Certificate,
            HandshakePayload::CertificateTLS13
        )?;
        self.handshake
            .transcript
            .add_message(&m);

        // We don't send any CertificateRequest extensions, so any extensions
        // here are illegal.
        if certp.any_entry_has_extension() {
            return Err(TLSError::PeerMisbehavedError(
                "client sent unsolicited cert extension".to_string(),
            ));
        }

        let cert_chain = certp.convert();

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
                return Ok(self.into_expect_finished());
            }

            sess.common
                .send_fatal_alert(AlertDescription::CertificateRequired);
            return Err(TLSError::NoCertificatesPresented);
        }

        sess.config
            .get_verifier()
            .verify_client_cert(&cert_chain, sess.get_sni())
            .or_else(|err| {
                hs::incompatible(sess, "certificate invalid");
                Err(err)
            })?;

        let cert = ClientCertDetails::new(cert_chain);
        Ok(self.into_expect_certificate_verify(cert))
    }
}

pub struct ExpectCertificateVerify {
    handshake: HandshakeDetails,
    key_schedule: KeyScheduleTrafficWithClientFinishedPending,
    client_cert: ClientCertDetails,
    send_ticket: bool,
}

impl ExpectCertificateVerify {
    fn into_expect_finished(self) -> hs::NextState {
        Box::new(ExpectFinished {
            key_schedule: self.key_schedule,
            handshake: self.handshake,
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
            let handshake_hash = self
                .handshake
                .transcript
                .get_current_hash();
            self.handshake
                .transcript
                .abandon_client_auth();
            let certs = &self.client_cert.cert_chain;
            let msg = verify::construct_tls13_client_verify_message(&handshake_hash);

            sess.config
                .get_verifier()
                .verify_tls13_signature(&msg, &certs[0], sig)
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
        Ok(self.into_expect_finished())
    }
}

// --- Process client's Finished ---
fn get_server_session_value(
    handshake: &mut HandshakeDetails,
    key_schedule: &KeyScheduleTraffic,
    sess: &ServerSessionImpl,
    nonce: &[u8],
) -> persist::ServerSessionValue {
    let scs = sess.common.get_suite_assert();
    let version = ProtocolVersion::TLSv1_3;

    let handshake_hash = handshake.transcript.get_current_hash();
    let secret =
        key_schedule.resumption_master_secret_and_derive_ticket_psk(&handshake_hash, nonce);

    persist::ServerSessionValue::new(
        sess.get_sni(),
        version,
        scs.suite,
        secret,
        &sess.client_cert_chain,
        sess.alpn_protocol.clone(),
        sess.resumption_data.clone(),
    )
}

pub struct ExpectFinished {
    pub handshake: HandshakeDetails,
    pub key_schedule: KeyScheduleTrafficWithClientFinishedPending,
    pub send_ticket: bool,
}

impl ExpectFinished {
    fn into_expect_traffic(
        fin: verify::FinishedMessageVerified,
        ks: KeyScheduleTraffic,
    ) -> hs::NextState {
        Box::new(ExpectTraffic {
            key_schedule: ks,
            want_write_key_update: false,
            _fin_verified: fin,
        })
    }

    fn emit_stateless_ticket(
        handshake: &mut HandshakeDetails,
        sess: &mut ServerSessionImpl,
        key_schedule: &KeyScheduleTraffic,
    ) {
        let nonce = rand::random_vec(32);
        let plain = get_server_session_value(handshake, key_schedule, sess, &nonce).get_encoding();
        let maybe_ticket = sess.config.ticketer.encrypt(&plain);
        let ticket_lifetime = sess.config.ticketer.get_lifetime();

        if maybe_ticket.is_none() {
            return;
        }

        let ticket = maybe_ticket.unwrap();
        let age_add = rand::random_u32(); // nb, we don't do 0-RTT data, so whatever
        #[allow(unused_mut)]
        let mut payload =
            NewSessionTicketPayloadTLS13::new(ticket_lifetime, age_add, nonce, ticket);
        #[cfg(feature = "quic")]
        {
            if sess.config.max_early_data_size > 0 && sess.common.protocol == Protocol::Quic {
                payload
                    .exts
                    .push(NewSessionTicketExtension::EarlyData(
                        sess.config.max_early_data_size,
                    ));
            }
        }
        let m = Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_3,
            payload: MessagePayload::Handshake(HandshakeMessagePayload {
                typ: HandshakeType::NewSessionTicket,
                payload: HandshakePayload::NewSessionTicketTLS13(payload),
            }),
        };

        trace!("sending new ticket {:?}", m);
        handshake.transcript.add_message(&m);
        sess.common.send_msg(m, true);
    }

    fn emit_stateful_ticket(
        handshake: &mut HandshakeDetails,
        sess: &mut ServerSessionImpl,
        key_schedule: &KeyScheduleTraffic,
    ) {
        let nonce = rand::random_vec(32);
        let id = rand::random_vec(32);
        let plain = get_server_session_value(handshake, key_schedule, sess, &nonce).get_encoding();

        if sess
            .config
            .session_storage
            .put(id.clone(), plain)
        {
            let stateful_lifetime = 24 * 60 * 60; // this is a bit of a punt
            let age_add = rand::random_u32();
            #[allow(unused_mut)]
            let mut payload =
                NewSessionTicketPayloadTLS13::new(stateful_lifetime, age_add, nonce, id);
            #[cfg(feature = "quic")]
            {
                if sess.config.max_early_data_size > 0 && sess.common.protocol == Protocol::Quic {
                    payload
                        .exts
                        .push(NewSessionTicketExtension::EarlyData(
                            sess.config.max_early_data_size,
                        ));
                }
            }
            let m = Message {
                typ: ContentType::Handshake,
                version: ProtocolVersion::TLSv1_3,
                payload: MessagePayload::Handshake(HandshakeMessagePayload {
                    typ: HandshakeType::NewSessionTicket,
                    payload: HandshakePayload::NewSessionTicketTLS13(payload),
                }),
            };

            trace!("sending new stateful ticket {:?}", m);
            handshake.transcript.add_message(&m);
            sess.common.send_msg(m, true);
        } else {
            trace!("resumption not available; not issuing ticket");
        }
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

        let handshake_hash = self
            .handshake
            .transcript
            .get_current_hash();
        let expect_verify_data = self
            .key_schedule
            .sign_client_finish(&handshake_hash);

        let fin = constant_time::verify_slices_are_equal(&expect_verify_data, &finished.0)
            .map_err(|_| {
                sess.common
                    .send_fatal_alert(AlertDescription::DecryptError);
                warn!("Finished wrong");
                TLSError::DecryptError
            })
            .map(|_| verify::FinishedMessageVerified::assertion())?;

        // nb. future derivations include Client Finished, but not the
        // main application data keying.
        self.handshake
            .transcript
            .add_message(&m);

        hs::check_aligned_handshake(sess)?;

        let suite = sess.common.get_suite_assert();

        // Install keying to read future messages.
        let read_key = self
            .key_schedule
            .client_application_traffic_secret(
                &self.handshake.hash_at_server_fin,
                &*sess.config.key_log,
                &self.handshake.randoms.client,
            );
        sess.common
            .record_layer
            .set_message_decrypter(cipher::new_tls13_read(suite, &read_key));

        let key_schedule_traffic = self.key_schedule.into_traffic();

        if self.send_ticket {
            if sess.config.ticketer.enabled() {
                Self::emit_stateless_ticket(&mut self.handshake, sess, &key_schedule_traffic);
            } else {
                Self::emit_stateful_ticket(&mut self.handshake, sess, &key_schedule_traffic);
            }
        }

        sess.common.start_traffic();

        #[cfg(feature = "quic")]
        {
            if sess.common.protocol == Protocol::Quic {
                return Ok(Box::new(ExpectQUICTraffic {
                    key_schedule: key_schedule_traffic,
                    _fin_verified: fin,
                }));
            }
        }

        Ok(Self::into_expect_traffic(fin, key_schedule_traffic))
    }
}

// --- Process traffic ---
pub struct ExpectTraffic {
    key_schedule: KeyScheduleTraffic,
    want_write_key_update: bool,
    _fin_verified: verify::FinishedMessageVerified,
}

impl ExpectTraffic {
    fn handle_traffic(&self, sess: &mut ServerSessionImpl, mut m: Message) -> Result<(), TLSError> {
        sess.common
            .take_received_plaintext(m.take_opaque_payload().unwrap());
        Ok(())
    }

    fn handle_key_update(
        &mut self,
        sess: &mut ServerSessionImpl,
        kur: &KeyUpdateRequest,
    ) -> Result<(), TLSError> {
        #[cfg(feature = "quic")]
        {
            if let Protocol::Quic = sess.common.protocol {
                sess.common
                    .send_fatal_alert(AlertDescription::UnexpectedMessage);
                let msg = "KeyUpdate received in QUIC connection".to_string();
                warn!("{}", msg);
                return Err(TLSError::PeerMisbehavedError(msg));
            }
        }

        hs::check_aligned_handshake(sess)?;

        match kur {
            KeyUpdateRequest::UpdateNotRequested => {}
            KeyUpdateRequest::UpdateRequested => {
                self.want_write_key_update = true;
            }
            _ => {
                sess.common
                    .send_fatal_alert(AlertDescription::IllegalParameter);
                return Err(TLSError::CorruptMessagePayload(ContentType::Handshake));
            }
        }

        // Update our read-side keys.
        let new_read_key = self
            .key_schedule
            .next_client_application_traffic_secret();
        let suite = sess.common.get_suite_assert();
        sess.common
            .record_layer
            .set_message_decrypter(cipher::new_tls13_read(suite, &new_read_key));

        Ok(())
    }
}

impl hs::State for ExpectTraffic {
    fn handle(
        mut self: Box<Self>,
        sess: &mut ServerSessionImpl,
        m: Message,
    ) -> hs::NextStateOrError {
        if m.is_content_type(ContentType::ApplicationData) {
            self.handle_traffic(sess, m)?;
        } else if let Ok(key_update) =
            require_handshake_msg!(m, HandshakeType::KeyUpdate, HandshakePayload::KeyUpdate)
        {
            self.handle_key_update(sess, key_update)?;
        } else {
            check_message(
                &m,
                &[ContentType::ApplicationData, ContentType::Handshake],
                &[HandshakeType::KeyUpdate],
            )?;
        }

        Ok(self)
    }

    fn export_keying_material(
        &self,
        output: &mut [u8],
        label: &[u8],
        context: Option<&[u8]>,
    ) -> Result<(), TLSError> {
        self.key_schedule
            .export_keying_material(output, label, context)
    }

    fn perhaps_write_key_update(&mut self, sess: &mut ServerSessionImpl) {
        if self.want_write_key_update {
            self.want_write_key_update = false;
            sess.common
                .send_msg_encrypt(Message::build_key_update_notify());

            let write_key = self
                .key_schedule
                .next_server_application_traffic_secret();
            let scs = sess.common.get_suite_assert();
            sess.common
                .record_layer
                .set_message_encrypter(cipher::new_tls13_write(scs, &write_key));
        }
    }
}

#[cfg(feature = "quic")]
pub struct ExpectQUICTraffic {
    key_schedule: KeyScheduleTraffic,
    _fin_verified: verify::FinishedMessageVerified,
}

#[cfg(feature = "quic")]
impl hs::State for ExpectQUICTraffic {
    fn handle(self: Box<Self>, _: &mut ServerSessionImpl, m: Message) -> hs::NextStateOrError {
        // reject all messages
        check_message(&m, &[], &[])?;
        unreachable!();
    }

    fn export_keying_material(
        &self,
        output: &mut [u8],
        label: &[u8],
        context: Option<&[u8]>,
    ) -> Result<(), TLSError> {
        self.key_schedule
            .export_keying_material(output, label, context)
    }
}

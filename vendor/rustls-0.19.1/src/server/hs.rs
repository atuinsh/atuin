use crate::error::TLSError;
#[cfg(feature = "logging")]
use crate::log::{debug, trace};
use crate::msgs::codec::Codec;
use crate::msgs::enums::{AlertDescription, ExtensionType};
use crate::msgs::enums::{CipherSuite, Compression, ECPointFormat, NamedGroup};
use crate::msgs::enums::{ClientCertificateType, SignatureScheme};
use crate::msgs::enums::{ContentType, HandshakeType, ProtocolVersion};
use crate::msgs::handshake::CertificateRequestPayload;
use crate::msgs::handshake::CertificateStatus;
use crate::msgs::handshake::ClientExtension;
use crate::msgs::handshake::{ClientHelloPayload, ServerExtension, SessionID};
use crate::msgs::handshake::{ConvertProtocolNameList, ConvertServerNameList};
use crate::msgs::handshake::{DigitallySignedStruct, ServerECDHParams};
use crate::msgs::handshake::{ECDHEServerKeyExchange, ServerKeyExchangePayload};
use crate::msgs::handshake::{ECPointFormatList, SupportedPointFormats};
use crate::msgs::handshake::{HandshakeMessagePayload, Random, ServerHelloPayload};
use crate::msgs::handshake::{HandshakePayload, SupportedSignatureSchemes};
use crate::msgs::message::{Message, MessagePayload};
use crate::msgs::persist;
use crate::rand;
use crate::server::{ClientHello, ServerConfig, ServerSessionImpl};
#[cfg(feature = "quic")]
use crate::session::Protocol;
use crate::session::SessionSecrets;
use crate::sign;
use crate::suites;
use webpki;

use crate::server::common::{HandshakeDetails, ServerKXDetails};
use crate::server::{tls12, tls13};

pub type NextState = Box<dyn State + Send + Sync>;
pub type NextStateOrError = Result<NextState, TLSError>;

pub trait State {
    fn handle(self: Box<Self>, sess: &mut ServerSessionImpl, m: Message) -> NextStateOrError;

    fn export_keying_material(
        &self,
        _output: &mut [u8],
        _label: &[u8],
        _context: Option<&[u8]>,
    ) -> Result<(), TLSError> {
        Err(TLSError::HandshakeNotComplete)
    }

    fn perhaps_write_key_update(&mut self, _sess: &mut ServerSessionImpl) {}
}

pub fn incompatible(sess: &mut ServerSessionImpl, why: &str) -> TLSError {
    sess.common
        .send_fatal_alert(AlertDescription::HandshakeFailure);
    TLSError::PeerIncompatibleError(why.to_string())
}

fn bad_version(sess: &mut ServerSessionImpl, why: &str) -> TLSError {
    sess.common
        .send_fatal_alert(AlertDescription::ProtocolVersion);
    TLSError::PeerIncompatibleError(why.to_string())
}

pub fn illegal_param(sess: &mut ServerSessionImpl, why: &str) -> TLSError {
    sess.common
        .send_fatal_alert(AlertDescription::IllegalParameter);
    TLSError::PeerMisbehavedError(why.to_string())
}

pub fn decode_error(sess: &mut ServerSessionImpl, why: &str) -> TLSError {
    sess.common
        .send_fatal_alert(AlertDescription::DecodeError);
    TLSError::PeerMisbehavedError(why.to_string())
}

pub fn can_resume(
    sess: &ServerSessionImpl,
    handshake: &HandshakeDetails,
    resumedata: &Option<persist::ServerSessionValue>,
) -> bool {
    // The RFCs underspecify what happens if we try to resume to
    // an unoffered/varying suite.  We merely don't resume in weird cases.
    //
    // RFC 6066 says "A server that implements this extension MUST NOT accept
    // the request to resume the session if the server_name extension contains
    // a different name. Instead, it proceeds with a full handshake to
    // establish a new session."

    if let Some(ref resume) = *resumedata {
        resume.cipher_suite == sess.common.get_suite_assert().suite
            && (resume.extended_ms == handshake.using_ems
                || (resume.extended_ms && !handshake.using_ems))
            && same_dns_name_or_both_none(resume.sni.as_ref(), sess.sni.as_ref())
    } else {
        false
    }
}

// Require an exact match for the purpose of comparing SNI DNS Names from two
// client hellos, even though a case-insensitive comparison might also be OK.
fn same_dns_name_or_both_none(a: Option<&webpki::DNSName>, b: Option<&webpki::DNSName>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            let a: &str = a.as_ref().into();
            let b: &str = b.as_ref().into();
            a == b
        }
        (None, None) => true,
        _ => false,
    }
}

// Changing the keys must not span any fragmented handshake
// messages.  Otherwise the defragmented messages will have
// been protected with two different record layer protections,
// which is illegal.  Not mentioned in RFC.
pub fn check_aligned_handshake(sess: &mut ServerSessionImpl) -> Result<(), TLSError> {
    if !sess.common.handshake_joiner.is_empty() {
        sess.common
            .send_fatal_alert(AlertDescription::UnexpectedMessage);
        Err(TLSError::PeerMisbehavedError(
            "key epoch or handshake flight with pending fragment".to_string(),
        ))
    } else {
        Ok(())
    }
}

pub fn save_sni(sess: &mut ServerSessionImpl, sni: Option<webpki::DNSName>) {
    if let Some(sni) = sni {
        // Save the SNI into the session.
        sess.set_sni(sni);
    }
}

#[derive(Default)]
pub struct ExtensionProcessing {
    // extensions to reply with
    pub exts: Vec<ServerExtension>,

    // effects on later handshake steps
    pub send_cert_status: bool,
    pub send_sct: bool,
    pub send_ticket: bool,
}

impl ExtensionProcessing {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn process_common(
        &mut self,
        sess: &mut ServerSessionImpl,
        server_key: Option<&mut sign::CertifiedKey>,
        hello: &ClientHelloPayload,
        resumedata: Option<&persist::ServerSessionValue>,
        handshake: &HandshakeDetails,
    ) -> Result<(), TLSError> {
        // ALPN
        let our_protocols = &sess.config.alpn_protocols;
        let maybe_their_protocols = hello.get_alpn_extension();
        if let Some(their_protocols) = maybe_their_protocols {
            let their_protocols = their_protocols.to_slices();

            if their_protocols
                .iter()
                .any(|protocol| protocol.is_empty())
            {
                return Err(TLSError::PeerMisbehavedError(
                    "client offered empty ALPN protocol".to_string(),
                ));
            }

            sess.alpn_protocol = our_protocols
                .iter()
                .filter(|protocol| their_protocols.contains(&protocol.as_slice()))
                .nth(0)
                .cloned();
            if let Some(ref selected_protocol) = sess.alpn_protocol {
                debug!("Chosen ALPN protocol {:?}", selected_protocol);
                self.exts
                    .push(ServerExtension::make_alpn(&[selected_protocol]));
            } else {
                // For compatibility, strict ALPN validation is not employed unless targeting QUIC
                #[cfg(feature = "quic")]
                {
                    if sess.common.protocol == Protocol::Quic && !our_protocols.is_empty() {
                        sess.common
                            .send_fatal_alert(AlertDescription::NoApplicationProtocol);
                        return Err(TLSError::NoApplicationProtocol);
                    }
                }
            }
        }

        #[cfg(feature = "quic")]
        {
            if sess.common.protocol == Protocol::Quic {
                if let Some(params) = hello.get_quic_params_extension() {
                    sess.common.quic.params = Some(params);
                }

                if let Some(resume) = resumedata {
                    if sess.config.max_early_data_size > 0
                        && hello.early_data_extension_offered()
                        && resume.version == sess.common.negotiated_version.unwrap()
                        && resume.cipher_suite == sess.common.get_suite_assert().suite
                        && resume.alpn.as_ref().map(|x| &x.0) == sess.alpn_protocol.as_ref()
                        && !sess.reject_early_data
                    {
                        self.exts
                            .push(ServerExtension::EarlyData);
                    } else {
                        // Clobber value set in tls13::emit_server_hello
                        sess.common.quic.early_secret = None;
                    }
                }
            }
        }

        let for_resume = resumedata.is_some();
        // SNI
        if !for_resume && hello.get_sni_extension().is_some() {
            self.exts
                .push(ServerExtension::ServerNameAck);
        }

        if let Some(server_key) = server_key {
            // Send status_request response if we have one.  This is not allowed
            // if we're resuming, and is only triggered if we have an OCSP response
            // to send.
            if !for_resume
                && hello
                    .find_extension(ExtensionType::StatusRequest)
                    .is_some()
                && server_key.has_ocsp()
            {
                self.send_cert_status = true;

                if !sess.common.is_tls13() {
                    // Only TLS1.2 sends confirmation in ServerHello
                    self.exts
                        .push(ServerExtension::CertificateStatusAck);
                }
            }

            if !for_resume
                && hello
                    .find_extension(ExtensionType::SCT)
                    .is_some()
                && server_key.has_sct_list()
            {
                self.send_sct = true;

                if !sess.common.is_tls13() {
                    let sct_list = server_key.take_sct_list().unwrap();
                    self.exts
                        .push(ServerExtension::make_sct(sct_list));
                }
            }
        }

        if !sess.common.is_tls13() {}

        self.exts
            .extend(handshake.extra_exts.iter().cloned());

        Ok(())
    }

    fn process_tls12(
        &mut self,
        sess: &ServerSessionImpl,
        hello: &ClientHelloPayload,
        handshake: &HandshakeDetails,
    ) {
        // Renegotiation.
        // (We don't do reneg at all, but would support the secure version if we did.)
        let secure_reneg_offered = hello
            .find_extension(ExtensionType::RenegotiationInfo)
            .is_some()
            || hello
                .cipher_suites
                .contains(&CipherSuite::TLS_EMPTY_RENEGOTIATION_INFO_SCSV);

        if secure_reneg_offered {
            self.exts
                .push(ServerExtension::make_empty_renegotiation_info());
        }

        // Tickets:
        // If we get any SessionTicket extension and have tickets enabled,
        // we send an ack.
        if hello
            .find_extension(ExtensionType::SessionTicket)
            .is_some()
            && sess.config.ticketer.enabled()
        {
            self.send_ticket = true;
            self.exts
                .push(ServerExtension::SessionTicketAck);
        }

        // Confirm use of EMS if offered.
        if handshake.using_ems {
            self.exts
                .push(ServerExtension::ExtendedMasterSecretAck);
        }
    }
}

pub struct ExpectClientHello {
    pub handshake: HandshakeDetails,
    pub done_retry: bool,
    pub send_cert_status: bool,
    pub send_sct: bool,
    pub send_ticket: bool,
}

impl ExpectClientHello {
    pub fn new(
        server_config: &ServerConfig,
        extra_exts: Vec<ServerExtension>,
    ) -> ExpectClientHello {
        let mut ech = ExpectClientHello {
            handshake: HandshakeDetails::new(extra_exts),
            done_retry: false,
            send_cert_status: false,
            send_sct: false,
            send_ticket: false,
        };

        if server_config
            .verifier
            .offer_client_auth()
        {
            ech.handshake
                .transcript
                .set_client_auth_enabled();
        }

        ech
    }

    fn into_expect_tls12_ccs(self, secrets: SessionSecrets) -> NextState {
        Box::new(tls12::ExpectCCS {
            secrets,
            handshake: self.handshake,
            resuming: true,
            send_ticket: self.send_ticket,
        })
    }

    fn into_complete_tls13_client_hello_handling(self) -> tls13::CompleteClientHelloHandling {
        tls13::CompleteClientHelloHandling {
            handshake: self.handshake,
            done_retry: self.done_retry,
            send_cert_status: self.send_cert_status,
            send_sct: self.send_sct,
            send_ticket: self.send_ticket,
        }
    }

    fn into_expect_tls12_certificate(self, kx: suites::KeyExchange) -> NextState {
        Box::new(tls12::ExpectCertificate {
            handshake: self.handshake,
            server_kx: ServerKXDetails::new(kx),
            send_ticket: self.send_ticket,
        })
    }

    fn into_expect_tls12_client_kx(self, kx: suites::KeyExchange) -> NextState {
        Box::new(tls12::ExpectClientKX {
            handshake: self.handshake,
            server_kx: ServerKXDetails::new(kx),
            client_cert: None,
            send_ticket: self.send_ticket,
        })
    }

    fn emit_server_hello(
        &mut self,
        sess: &mut ServerSessionImpl,
        server_key: Option<&mut sign::CertifiedKey>,
        hello: &ClientHelloPayload,
        resumedata: Option<&persist::ServerSessionValue>,
    ) -> Result<(), TLSError> {
        let mut ep = ExtensionProcessing::new();
        ep.process_common(sess, server_key, hello, resumedata, &self.handshake)?;
        ep.process_tls12(sess, hello, &self.handshake);

        self.send_ticket = ep.send_ticket;
        self.send_cert_status = ep.send_cert_status;
        self.send_sct = ep.send_sct;

        let sh = Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_2,
            payload: MessagePayload::Handshake(HandshakeMessagePayload {
                typ: HandshakeType::ServerHello,
                payload: HandshakePayload::ServerHello(ServerHelloPayload {
                    legacy_version: ProtocolVersion::TLSv1_2,
                    random: Random::from_slice(&self.handshake.randoms.server),
                    session_id: self.handshake.session_id,
                    cipher_suite: sess.common.get_suite_assert().suite,
                    compression_method: Compression::Null,
                    extensions: ep.exts,
                }),
            }),
        };

        trace!("sending server hello {:?}", sh);
        self.handshake
            .transcript
            .add_message(&sh);
        sess.common.send_msg(sh, false);
        Ok(())
    }

    fn emit_certificate(
        &mut self,
        sess: &mut ServerSessionImpl,
        server_certkey: &mut sign::CertifiedKey,
    ) {
        let cert_chain = server_certkey.take_cert();

        let c = Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_2,
            payload: MessagePayload::Handshake(HandshakeMessagePayload {
                typ: HandshakeType::Certificate,
                payload: HandshakePayload::Certificate(cert_chain),
            }),
        };

        self.handshake
            .transcript
            .add_message(&c);
        sess.common.send_msg(c, false);
    }

    fn emit_cert_status(
        &mut self,
        sess: &mut ServerSessionImpl,
        server_certkey: &mut sign::CertifiedKey,
    ) {
        if !self.send_cert_status || !server_certkey.has_ocsp() {
            return;
        }

        let ocsp = server_certkey.take_ocsp();
        let st = CertificateStatus::new(ocsp.unwrap());

        let c = Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_2,
            payload: MessagePayload::Handshake(HandshakeMessagePayload {
                typ: HandshakeType::CertificateStatus,
                payload: HandshakePayload::CertificateStatus(st),
            }),
        };

        self.handshake
            .transcript
            .add_message(&c);
        sess.common.send_msg(c, false);
    }

    fn emit_server_kx(
        &mut self,
        sess: &mut ServerSessionImpl,
        sigschemes: Vec<SignatureScheme>,
        group: NamedGroup,
        server_certkey: &mut sign::CertifiedKey,
    ) -> Result<suites::KeyExchange, TLSError> {
        let kx = sess
            .common
            .get_suite_assert()
            .start_server_kx(group)
            .ok_or_else(|| TLSError::PeerMisbehavedError("key exchange failed".to_string()))?;
        let secdh = ServerECDHParams::new(group, kx.pubkey.as_ref());

        let mut msg = Vec::new();
        msg.extend(&self.handshake.randoms.client);
        msg.extend(&self.handshake.randoms.server);
        secdh.encode(&mut msg);

        let signing_key = &server_certkey.key;
        let signer = signing_key
            .choose_scheme(&sigschemes)
            .ok_or_else(|| TLSError::General("incompatible signing key".to_string()))?;
        let sigscheme = signer.get_scheme();
        let sig = signer.sign(&msg)?;

        let skx = ServerKeyExchangePayload::ECDHE(ECDHEServerKeyExchange {
            params: secdh,
            dss: DigitallySignedStruct::new(sigscheme, sig),
        });

        let m = Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_2,
            payload: MessagePayload::Handshake(HandshakeMessagePayload {
                typ: HandshakeType::ServerKeyExchange,
                payload: HandshakePayload::ServerKeyExchange(skx),
            }),
        };

        self.handshake
            .transcript
            .add_message(&m);
        sess.common.send_msg(m, false);
        Ok(kx)
    }

    fn emit_certificate_req(&mut self, sess: &mut ServerSessionImpl) -> Result<bool, TLSError> {
        let client_auth = sess.config.get_verifier();

        if !client_auth.offer_client_auth() {
            return Ok(false);
        }

        let verify_schemes = client_auth.supported_verify_schemes();

        let names = client_auth
            .client_auth_root_subjects(sess.get_sni())
            .ok_or_else(|| {
                debug!("could not determine root subjects based on SNI");
                sess.common
                    .send_fatal_alert(AlertDescription::AccessDenied);
                TLSError::General("client rejected by client_auth_root_subjects".into())
            })?;

        let cr = CertificateRequestPayload {
            certtypes: vec![
                ClientCertificateType::RSASign,
                ClientCertificateType::ECDSASign,
            ],
            sigschemes: verify_schemes,
            canames: names,
        };

        let m = Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_2,
            payload: MessagePayload::Handshake(HandshakeMessagePayload {
                typ: HandshakeType::CertificateRequest,
                payload: HandshakePayload::CertificateRequest(cr),
            }),
        };

        trace!("Sending CertificateRequest {:?}", m);
        self.handshake
            .transcript
            .add_message(&m);
        sess.common.send_msg(m, false);
        Ok(true)
    }

    fn emit_server_hello_done(&mut self, sess: &mut ServerSessionImpl) {
        let m = Message {
            typ: ContentType::Handshake,
            version: ProtocolVersion::TLSv1_2,
            payload: MessagePayload::Handshake(HandshakeMessagePayload {
                typ: HandshakeType::ServerHelloDone,
                payload: HandshakePayload::ServerHelloDone,
            }),
        };

        self.handshake
            .transcript
            .add_message(&m);
        sess.common.send_msg(m, false);
    }

    fn start_resumption(
        mut self,
        sess: &mut ServerSessionImpl,
        client_hello: &ClientHelloPayload,
        sni: Option<&webpki::DNSName>,
        id: &SessionID,
        resumedata: persist::ServerSessionValue,
    ) -> NextStateOrError {
        debug!("Resuming session");

        if resumedata.extended_ms && !self.handshake.using_ems {
            return Err(illegal_param(sess, "refusing to resume without ems"));
        }

        self.handshake.session_id = *id;
        self.emit_server_hello(sess, None, client_hello, Some(&resumedata))?;

        let hashalg = sess
            .common
            .get_suite_assert()
            .get_hash();
        let secrets = SessionSecrets::new_resume(
            &self.handshake.randoms,
            hashalg,
            &resumedata.master_secret.0,
        );
        sess.config.key_log.log(
            "CLIENT_RANDOM",
            &secrets.randoms.client,
            &secrets.master_secret,
        );
        sess.common
            .start_encryption_tls12(&secrets);
        sess.client_cert_chain = resumedata.client_cert_chain;

        if self.send_ticket {
            tls12::emit_ticket(&secrets, &mut self.handshake, sess);
        }
        tls12::emit_ccs(sess);
        sess.common
            .record_layer
            .start_encrypting();
        tls12::emit_finished(&secrets, &mut self.handshake, sess);

        assert!(same_dns_name_or_both_none(sni, sess.get_sni()));

        Ok(self.into_expect_tls12_ccs(secrets))
    }
}

impl State for ExpectClientHello {
    fn handle(mut self: Box<Self>, sess: &mut ServerSessionImpl, m: Message) -> NextStateOrError {
        let client_hello =
            require_handshake_msg!(m, HandshakeType::ClientHello, HandshakePayload::ClientHello)?;
        let tls13_enabled = sess
            .config
            .supports_version(ProtocolVersion::TLSv1_3);
        let tls12_enabled = sess
            .config
            .supports_version(ProtocolVersion::TLSv1_2);
        trace!("we got a clienthello {:?}", client_hello);

        if !client_hello
            .compression_methods
            .contains(&Compression::Null)
        {
            sess.common
                .send_fatal_alert(AlertDescription::IllegalParameter);
            return Err(TLSError::PeerIncompatibleError(
                "client did not offer Null compression".to_string(),
            ));
        }

        if client_hello.has_duplicate_extension() {
            return Err(decode_error(sess, "client sent duplicate extensions"));
        }

        // No handshake messages should follow this one in this flight.
        check_aligned_handshake(sess)?;

        // Are we doing TLS1.3?
        let maybe_versions_ext = client_hello.get_versions_extension();
        if let Some(versions) = maybe_versions_ext {
            if versions.contains(&ProtocolVersion::TLSv1_3) && tls13_enabled {
                sess.common.negotiated_version = Some(ProtocolVersion::TLSv1_3);
            } else if !versions.contains(&ProtocolVersion::TLSv1_2) || !tls12_enabled {
                return Err(bad_version(sess, "TLS1.2 not offered/enabled"));
            }
        } else if client_hello.client_version.get_u16() < ProtocolVersion::TLSv1_2.get_u16() {
            return Err(bad_version(sess, "Client does not support TLSv1_2"));
        } else if !tls12_enabled && tls13_enabled {
            return Err(bad_version(
                sess,
                "Server requires TLS1.3, but client omitted versions ext",
            ));
        }

        if sess.common.negotiated_version == None {
            sess.common.negotiated_version = Some(ProtocolVersion::TLSv1_2);
        }

        // --- Common to TLS1.2 and TLS1.3: ciphersuite and certificate selection.

        // Extract and validate the SNI DNS name, if any, before giving it to
        // the cert resolver. In particular, if it is invalid then we should
        // send an Illegal Parameter alert instead of the Internal Error alert
        // (or whatever) that we'd send if this were checked later or in a
        // different way.
        let sni: Option<webpki::DNSName> = match client_hello.get_sni_extension() {
            Some(sni) => {
                if sni.has_duplicate_names_for_type() {
                    return Err(decode_error(
                        sess,
                        "ClientHello SNI contains duplicate name types",
                    ));
                }

                if let Some(hostname) = sni.get_single_hostname() {
                    Some(hostname.into())
                } else {
                    return Err(illegal_param(
                        sess,
                        "ClientHello SNI did not contain a hostname",
                    ));
                }
            }
            None => None,
        };

        if !self.done_retry {
            // save only the first SNI
            save_sni(sess, sni.clone());
        }

        // We communicate to the upper layer what kind of key they should choose
        // via the sigschemes value.  Clients tend to treat this extension
        // orthogonally to offered ciphersuites (even though, in TLS1.2 it is not).
        // So: reduce the offered sigschemes to those compatible with the
        // intersection of ciphersuites.
        let mut common_suites = sess.config.ciphersuites.clone();
        common_suites.retain(|scs| {
            client_hello
                .cipher_suites
                .contains(&scs.suite)
        });

        let mut sigschemes_ext = client_hello
            .get_sigalgs_extension()
            .cloned()
            .unwrap_or_else(SupportedSignatureSchemes::default);
        sigschemes_ext
            .retain(|scheme| suites::compatible_sigscheme_for_suites(*scheme, &common_suites));

        let alpn_protocols = client_hello
            .get_alpn_extension()
            .map(|protos| protos.to_slices());

        // Choose a certificate.
        let mut certkey = {
            let sni_ref = sni
                .as_ref()
                .map(webpki::DNSName::as_ref);
            trace!("sni {:?}", sni_ref);
            trace!("sig schemes {:?}", sigschemes_ext);
            trace!("alpn protocols {:?}", alpn_protocols);

            let alpn_slices = match alpn_protocols {
                Some(ref vec) => Some(vec.as_slice()),
                None => None,
            };

            let client_hello = ClientHello::new(sni_ref, &sigschemes_ext, alpn_slices);

            let certkey = sess
                .config
                .cert_resolver
                .resolve(client_hello);
            certkey.ok_or_else(|| {
                sess.common
                    .send_fatal_alert(AlertDescription::AccessDenied);
                TLSError::General("no server certificate chain resolved".to_string())
            })?
        };

        // Reduce our supported ciphersuites by the certificate.
        // (no-op for TLS1.3)
        let suitable_suites =
            suites::reduce_given_sigalg(&sess.config.ciphersuites, certkey.key.algorithm());

        // And version
        let protocol_version = sess.common.negotiated_version.unwrap();
        let suitable_suites = suites::reduce_given_version(&suitable_suites, protocol_version);

        let maybe_ciphersuite = if sess.config.ignore_client_order {
            suites::choose_ciphersuite_preferring_server(
                &client_hello.cipher_suites,
                &suitable_suites,
            )
        } else {
            suites::choose_ciphersuite_preferring_client(
                &client_hello.cipher_suites,
                &suitable_suites,
            )
        };

        if maybe_ciphersuite.is_none() {
            return Err(incompatible(sess, "no ciphersuites in common"));
        }

        debug!(
            "decided upon suite {:?}",
            maybe_ciphersuite.as_ref().unwrap()
        );
        sess.common
            .set_suite(maybe_ciphersuite.unwrap());

        // Start handshake hash.
        let starting_hash = sess
            .common
            .get_suite_assert()
            .get_hash();
        if !self
            .handshake
            .transcript
            .start_hash(starting_hash)
        {
            sess.common
                .send_fatal_alert(AlertDescription::IllegalParameter);
            return Err(TLSError::PeerIncompatibleError(
                "hash differed on retry".to_string(),
            ));
        }

        // Save their Random.
        client_hello
            .random
            .write_slice(&mut self.handshake.randoms.client);

        if sess.common.is_tls13() {
            return self
                .into_complete_tls13_client_hello_handling()
                .handle_client_hello(sess, certkey, &m);
        }

        // -- TLS1.2 only from hereon in --
        self.handshake
            .transcript
            .add_message(&m);

        if client_hello.ems_support_offered() {
            self.handshake.using_ems = true;
        }

        let groups_ext = client_hello
            .get_namedgroups_extension()
            .ok_or_else(|| incompatible(sess, "client didn't describe groups"))?;
        let ecpoints_ext = client_hello
            .get_ecpoints_extension()
            .ok_or_else(|| incompatible(sess, "client didn't describe ec points"))?;

        trace!("namedgroups {:?}", groups_ext);
        trace!("ecpoints {:?}", ecpoints_ext);

        if !ecpoints_ext.contains(&ECPointFormat::Uncompressed) {
            sess.common
                .send_fatal_alert(AlertDescription::IllegalParameter);
            return Err(TLSError::PeerIncompatibleError(
                "client didn't support uncompressed ec points".to_string(),
            ));
        }

        // -- If TLS1.3 is enabled, signal the downgrade in the server random
        if tls13_enabled {
            self.handshake
                .randoms
                .set_tls12_downgrade_marker();
        }

        // -- Check for resumption --
        // We can do this either by (in order of preference):
        // 1. receiving a ticket that decrypts
        // 2. receiving a sessionid that is in our cache
        //
        // If we receive a ticket, the sessionid won't be in our
        // cache, so don't check.
        //
        // If either works, we end up with a ServerSessionValue
        // which is passed to start_resumption and concludes
        // our handling of the ClientHello.
        //
        let mut ticket_received = false;

        if let Some(ticket_ext) = client_hello.get_ticket_extension() {
            if let ClientExtension::SessionTicketOffer(ref ticket) = *ticket_ext {
                ticket_received = true;
                debug!("Ticket received");

                let maybe_resume = sess
                    .config
                    .ticketer
                    .decrypt(&ticket.0)
                    .and_then(|plain| persist::ServerSessionValue::read_bytes(&plain));

                if can_resume(sess, &self.handshake, &maybe_resume) {
                    return self.start_resumption(
                        sess,
                        client_hello,
                        sni.as_ref(),
                        &client_hello.session_id,
                        maybe_resume.unwrap(),
                    );
                } else {
                    debug!("Ticket didn't decrypt");
                }
            }
        }

        // If we're not offered a ticket or a potential session ID,
        // allocate a session ID.
        if self.handshake.session_id.is_empty() && !ticket_received {
            let mut bytes = [0u8; 32];
            rand::fill_random(&mut bytes);
            self.handshake.session_id = SessionID::new(&bytes);
        }

        // Perhaps resume?  If we received a ticket, the sessionid
        // does not correspond to a real session.
        if !client_hello.session_id.is_empty() && !ticket_received {
            let maybe_resume = sess
                .config
                .session_storage
                .get(&client_hello.session_id.get_encoding())
                .and_then(|x| persist::ServerSessionValue::read_bytes(&x));

            if can_resume(sess, &self.handshake, &maybe_resume) {
                return self.start_resumption(
                    sess,
                    client_hello,
                    sni.as_ref(),
                    &client_hello.session_id,
                    maybe_resume.unwrap(),
                );
            }
        }

        // Now we have chosen a ciphersuite, we can make kx decisions.
        let sigschemes = sess
            .common
            .get_suite_assert()
            .resolve_sig_schemes(&sigschemes_ext);

        if sigschemes.is_empty() {
            return Err(incompatible(sess, "no supported sig scheme"));
        }

        let group = suites::KeyExchange::supported_groups()
            .iter()
            .filter(|group| groups_ext.contains(group))
            .nth(0)
            .cloned()
            .ok_or_else(|| incompatible(sess, "no supported group"))?;

        let ecpoint = ECPointFormatList::supported()
            .iter()
            .filter(|format| ecpoints_ext.contains(format))
            .nth(0)
            .cloned()
            .ok_or_else(|| incompatible(sess, "no supported point format"))?;

        debug_assert_eq!(ecpoint, ECPointFormat::Uncompressed);

        self.emit_server_hello(sess, Some(&mut certkey), client_hello, None)?;
        self.emit_certificate(sess, &mut certkey);
        self.emit_cert_status(sess, &mut certkey);
        let kx = self.emit_server_kx(sess, sigschemes, group, &mut certkey)?;
        let doing_client_auth = self.emit_certificate_req(sess)?;
        self.emit_server_hello_done(sess);

        if doing_client_auth {
            Ok(self.into_expect_tls12_certificate(kx))
        } else {
            Ok(self.into_expect_tls12_client_kx(kx))
        }
    }
}

use crate::error::TLSError;
use crate::key;
use crate::keylog::{KeyLog, NoKeyLog};
#[cfg(feature = "logging")]
use crate::log::trace;
use crate::msgs::enums::ContentType;
use crate::msgs::enums::SignatureScheme;
use crate::msgs::enums::{AlertDescription, HandshakeType, ProtocolVersion};
use crate::msgs::handshake::ServerExtension;
use crate::msgs::message::Message;
use crate::session::{MiddleboxCCS, Session, SessionCommon};
use crate::sign;
use crate::suites::{SupportedCipherSuite, ALL_CIPHERSUITES};
use crate::verify;

use webpki;

use std::fmt;
use std::io::{self, IoSlice};
use std::sync::Arc;

#[macro_use]
mod hs;
mod common;
pub mod handy;
mod tls12;
mod tls13;

/// A trait for the ability to store server session data.
///
/// The keys and values are opaque.
///
/// Both the keys and values should be treated as
/// **highly sensitive data**, containing enough key material
/// to break all security of the corresponding sessions.
///
/// Implementations can be lossy (in other words, forgetting
/// key/value pairs) without any negative security consequences.
///
/// However, note that `take` **must** reliably delete a returned
/// value.  If it does not, there may be security consequences.
///
/// `put` and `take` are mutating operations; this isn't expressed
/// in the type system to allow implementations freedom in
/// how to achieve interior mutability.  `Mutex` is a common
/// choice.
pub trait StoresServerSessions: Send + Sync {
    /// Store session secrets encoded in `value` against `key`,
    /// overwrites any existing value against `key`.  Returns `true`
    /// if the value was stored.
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> bool;

    /// Find a value with the given `key`.  Return it, or None
    /// if it doesn't exist.
    fn get(&self, key: &[u8]) -> Option<Vec<u8>>;

    /// Find a value with the given `key`.  Return it and delete it;
    /// or None if it doesn't exist.
    fn take(&self, key: &[u8]) -> Option<Vec<u8>>;
}

/// A trait for the ability to encrypt and decrypt tickets.
pub trait ProducesTickets: Send + Sync {
    /// Returns true if this implementation will encrypt/decrypt
    /// tickets.  Should return false if this is a dummy
    /// implementation: the server will not send the SessionTicket
    /// extension and will not call the other functions.
    fn enabled(&self) -> bool;

    /// Returns the lifetime in seconds of tickets produced now.
    /// The lifetime is provided as a hint to clients that the
    /// ticket will not be useful after the given time.
    ///
    /// This lifetime must be implemented by key rolling and
    /// erasure, *not* by storing a lifetime in the ticket.
    ///
    /// The objective is to limit damage to forward secrecy caused
    /// by tickets, not just limiting their lifetime.
    fn get_lifetime(&self) -> u32;

    /// Encrypt and authenticate `plain`, returning the resulting
    /// ticket.  Return None if `plain` cannot be encrypted for
    /// some reason: an empty ticket will be sent and the connection
    /// will continue.
    fn encrypt(&self, plain: &[u8]) -> Option<Vec<u8>>;

    /// Decrypt `cipher`, validating its authenticity protection
    /// and recovering the plaintext.  `cipher` is fully attacker
    /// controlled, so this decryption must be side-channel free,
    /// panic-proof, and otherwise bullet-proof.  If the decryption
    /// fails, return None.
    fn decrypt(&self, cipher: &[u8]) -> Option<Vec<u8>>;
}

/// How to choose a certificate chain and signing key for use
/// in server authentication.
pub trait ResolvesServerCert: Send + Sync {
    /// Choose a certificate chain and matching key given simplified
    /// ClientHello information.
    ///
    /// Return `None` to abort the handshake.
    fn resolve(&self, client_hello: ClientHello) -> Option<sign::CertifiedKey>;
}

/// A struct representing the received Client Hello
pub struct ClientHello<'a> {
    server_name: Option<webpki::DNSNameRef<'a>>,
    sigschemes: &'a [SignatureScheme],
    alpn: Option<&'a [&'a [u8]]>,
}

impl<'a> ClientHello<'a> {
    /// Creates a new ClientHello
    fn new(
        server_name: Option<webpki::DNSNameRef<'a>>,
        sigschemes: &'a [SignatureScheme],
        alpn: Option<&'a [&'a [u8]]>,
    ) -> Self {
        ClientHello {
            server_name,
            sigschemes,
            alpn,
        }
    }

    /// Get the server name indicator.
    ///
    /// Returns `None` if the client did not supply a SNI.
    pub fn server_name(&self) -> Option<webpki::DNSNameRef> {
        self.server_name
    }

    /// Get the compatible signature schemes.
    ///
    /// Returns standard-specified default if the client omitted this extension.
    pub fn sigschemes(&self) -> &[SignatureScheme] {
        self.sigschemes
    }

    /// Get the alpn.
    ///
    /// Returns `None` if the client did not include an ALPN extension
    pub fn alpn(&self) -> Option<&'a [&'a [u8]]> {
        self.alpn
    }
}

/// Common configuration for a set of server sessions.
///
/// Making one of these can be expensive, and should be
/// once per process rather than once per connection.
#[derive(Clone)]
pub struct ServerConfig {
    /// List of ciphersuites, in preference order.
    pub ciphersuites: Vec<&'static SupportedCipherSuite>,

    /// Ignore the client's ciphersuite order. Instead,
    /// choose the top ciphersuite in the server list
    /// which is supported by the client.
    pub ignore_client_order: bool,

    /// Our MTU.  If None, we don't limit TLS message sizes.
    pub mtu: Option<usize>,

    /// How to store client sessions.
    pub session_storage: Arc<dyn StoresServerSessions + Send + Sync>,

    /// How to produce tickets.
    pub ticketer: Arc<dyn ProducesTickets>,

    /// How to choose a server cert and key.
    pub cert_resolver: Arc<dyn ResolvesServerCert>,

    /// Protocol names we support, most preferred first.
    /// If empty we don't do ALPN at all.
    pub alpn_protocols: Vec<Vec<u8>>,

    /// Supported protocol versions, in no particular order.
    /// The default is all supported versions.
    pub versions: Vec<ProtocolVersion>,

    /// How to verify client certificates.
    verifier: Arc<dyn verify::ClientCertVerifier>,

    /// How to output key material for debugging.  The default
    /// does nothing.
    pub key_log: Arc<dyn KeyLog>,

    /// Amount of early data to accept; 0 to disable.
    #[cfg(feature = "quic")] // TLS support unimplemented
    #[doc(hidden)]
    pub max_early_data_size: u32,
}

impl ServerConfig {
    /// Make a `ServerConfig` with a default set of ciphersuites,
    /// no keys/certificates, and no ALPN protocols.  Session resumption
    /// is enabled by storing up to 256 recent sessions in memory. Tickets are
    /// disabled.
    ///
    /// Publicly-available web servers on the internet generally don't do client
    /// authentication; for this use case, `client_cert_verifier` should be a
    /// `NoClientAuth`. Otherwise, use `AllowAnyAuthenticatedClient` or another
    /// implementation to enforce client authentication.
    ///
    /// We don't provide a default for `client_cert_verifier` because the safest
    /// default, requiring client authentication, requires additional
    /// configuration that we cannot provide reasonable defaults for.
    pub fn new(client_cert_verifier: Arc<dyn verify::ClientCertVerifier>) -> ServerConfig {
        ServerConfig::with_ciphersuites(client_cert_verifier, &ALL_CIPHERSUITES)
    }

    /// Make a `ServerConfig` with a custom set of ciphersuites,
    /// no keys/certificates, and no ALPN protocols.  Session resumption
    /// is enabled by storing up to 256 recent sessions in memory. Tickets are
    /// disabled.
    ///
    /// Publicly-available web servers on the internet generally don't do client
    /// authentication; for this use case, `client_cert_verifier` should be a
    /// `NoClientAuth`. Otherwise, use `AllowAnyAuthenticatedClient` or another
    /// implementation to enforce client authentication.
    ///
    /// We don't provide a default for `client_cert_verifier` because the safest
    /// default, requiring client authentication, requires additional
    /// configuration that we cannot provide reasonable defaults for.
    pub fn with_ciphersuites(
        client_cert_verifier: Arc<dyn verify::ClientCertVerifier>,
        ciphersuites: &[&'static SupportedCipherSuite],
    ) -> ServerConfig {
        ServerConfig {
            ciphersuites: ciphersuites.to_vec(),
            ignore_client_order: false,
            mtu: None,
            session_storage: handy::ServerSessionMemoryCache::new(256),
            ticketer: Arc::new(handy::NeverProducesTickets {}),
            alpn_protocols: Vec::new(),
            cert_resolver: Arc::new(handy::FailResolveChain {}),
            versions: vec![ProtocolVersion::TLSv1_3, ProtocolVersion::TLSv1_2],
            verifier: client_cert_verifier,
            key_log: Arc::new(NoKeyLog {}),
            #[cfg(feature = "quic")]
            max_early_data_size: 0,
        }
    }

    #[doc(hidden)]
    /// We support a given TLS version if it's quoted in the configured
    /// versions *and* at least one ciphersuite for this version is
    /// also configured.
    pub fn supports_version(&self, v: ProtocolVersion) -> bool {
        self.versions.contains(&v)
            && self
                .ciphersuites
                .iter()
                .any(|cs| cs.usable_for_version(v))
    }

    #[doc(hidden)]
    pub fn get_verifier(&self) -> &dyn verify::ClientCertVerifier {
        self.verifier.as_ref()
    }

    /// Sets the session persistence layer to `persist`.
    pub fn set_persistence(&mut self, persist: Arc<dyn StoresServerSessions + Send + Sync>) {
        self.session_storage = persist;
    }

    /// Sets a single certificate chain and matching private key.  This
    /// certificate and key is used for all subsequent connections,
    /// irrespective of things like SNI hostname.
    ///
    /// Note that the end-entity certificate must have the
    /// [Subject Alternative Name](https://tools.ietf.org/html/rfc6125#section-4.1)
    /// extension to describe, e.g., the valid DNS name. The `commonName` field is
    /// disregarded.
    ///
    /// `cert_chain` is a vector of DER-encoded certificates.
    /// `key_der` is a DER-encoded RSA, ECDSA, or Ed25519 private key.
    ///
    /// This function fails if `key_der` is invalid.
    pub fn set_single_cert(
        &mut self,
        cert_chain: Vec<key::Certificate>,
        key_der: key::PrivateKey,
    ) -> Result<(), TLSError> {
        let resolver = handy::AlwaysResolvesChain::new(cert_chain, &key_der)?;
        self.cert_resolver = Arc::new(resolver);
        Ok(())
    }

    /// Sets a single certificate chain, matching private key and OCSP
    /// response.  This certificate and key is used for all subsequent
    /// connections, irrespective of things like SNI hostname.
    ///
    /// `cert_chain` is a vector of DER-encoded certificates.
    /// `key_der` is a DER-encoded RSA, ECDSA, or Ed25519 private key.
    /// `ocsp` is a DER-encoded OCSP response.  Ignored if zero length.
    /// `scts` is an `SignedCertificateTimestampList` encoding (see RFC6962)
    /// and is ignored if empty.
    ///
    /// This function fails if `key_der` is invalid.
    pub fn set_single_cert_with_ocsp_and_sct(
        &mut self,
        cert_chain: Vec<key::Certificate>,
        key_der: key::PrivateKey,
        ocsp: Vec<u8>,
        scts: Vec<u8>,
    ) -> Result<(), TLSError> {
        let resolver =
            handy::AlwaysResolvesChain::new_with_extras(cert_chain, &key_der, ocsp, scts)?;
        self.cert_resolver = Arc::new(resolver);
        Ok(())
    }

    /// Set the ALPN protocol list to the given protocol names.
    /// Overwrites any existing configured protocols.
    ///
    /// The first element in the `protocols` list is the most
    /// preferred, the last is the least preferred.
    pub fn set_protocols(&mut self, protocols: &[Vec<u8>]) {
        self.alpn_protocols.clear();
        self.alpn_protocols
            .extend_from_slice(protocols);
    }

    /// Overrides the default `ClientCertVerifier` with something else.
    pub fn set_client_certificate_verifier(
        &mut self,
        verifier: Arc<dyn verify::ClientCertVerifier>,
    ) {
        self.verifier = verifier;
    }
}

pub struct ServerSessionImpl {
    pub config: Arc<ServerConfig>,
    pub common: SessionCommon,
    sni: Option<webpki::DNSName>,
    pub alpn_protocol: Option<Vec<u8>>,
    pub quic_params: Option<Vec<u8>>,
    pub received_resumption_data: Option<Vec<u8>>,
    pub resumption_data: Vec<u8>,
    pub error: Option<TLSError>,
    pub state: Option<Box<dyn hs::State + Send + Sync>>,
    pub client_cert_chain: Option<Vec<key::Certificate>>,
    /// Whether to reject early data even if it would otherwise be accepted
    pub reject_early_data: bool,
}

impl fmt::Debug for ServerSessionImpl {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ServerSessionImpl")
            .finish()
    }
}

impl ServerSessionImpl {
    pub fn new(
        server_config: &Arc<ServerConfig>,
        extra_exts: Vec<ServerExtension>,
    ) -> ServerSessionImpl {
        ServerSessionImpl {
            config: server_config.clone(),
            common: SessionCommon::new(server_config.mtu, false),
            sni: None,
            alpn_protocol: None,
            quic_params: None,
            received_resumption_data: None,
            resumption_data: Vec::new(),
            error: None,
            state: Some(Box::new(hs::ExpectClientHello::new(
                server_config,
                extra_exts,
            ))),
            client_cert_chain: None,
            reject_early_data: false,
        }
    }

    pub fn wants_read(&self) -> bool {
        // We want to read more data all the time, except when we
        // have unprocessed plaintext.  This provides back-pressure
        // to the TCP buffers.
        //
        // This also covers the handshake case, because we don't have
        // readable plaintext before handshake has completed.
        !self.common.has_readable_plaintext()
    }

    pub fn wants_write(&self) -> bool {
        !self.common.sendable_tls.is_empty()
    }

    pub fn is_handshaking(&self) -> bool {
        !self.common.traffic
    }

    pub fn set_buffer_limit(&mut self, len: usize) {
        self.common.set_buffer_limit(len)
    }

    pub fn process_msg(&mut self, mut msg: Message) -> Result<(), TLSError> {
        // TLS1.3: drop CCS at any time during handshaking
        if let MiddleboxCCS::Drop = self.common.filter_tls13_ccs(&msg)? {
            trace!("Dropping CCS");
            return Ok(());
        }

        // Decrypt if demanded by current state.
        if self.common.record_layer.is_decrypting() {
            let dm = self.common.decrypt_incoming(msg)?;
            msg = dm;
        }

        // For handshake messages, we need to join them before parsing
        // and processing.
        if self
            .common
            .handshake_joiner
            .want_message(&msg)
        {
            self.common
                .handshake_joiner
                .take_message(msg)
                .ok_or_else(|| {
                    self.common
                        .send_fatal_alert(AlertDescription::DecodeError);
                    TLSError::CorruptMessagePayload(ContentType::Handshake)
                })?;
            return self.process_new_handshake_messages();
        }

        // Now we can fully parse the message payload.
        msg.decode_payload();

        if msg.is_content_type(ContentType::Alert) {
            return self.common.process_alert(msg);
        }

        self.process_main_protocol(msg)
    }

    pub fn process_new_handshake_messages(&mut self) -> Result<(), TLSError> {
        while let Some(msg) = self
            .common
            .handshake_joiner
            .frames
            .pop_front()
        {
            self.process_main_protocol(msg)?;
        }

        Ok(())
    }

    fn queue_unexpected_alert(&mut self) {
        self.common
            .send_fatal_alert(AlertDescription::UnexpectedMessage);
    }

    fn maybe_send_unexpected_alert(&mut self, rc: hs::NextStateOrError) -> hs::NextStateOrError {
        match rc {
            Err(TLSError::InappropriateMessage { .. })
            | Err(TLSError::InappropriateHandshakeMessage { .. }) => {
                self.queue_unexpected_alert();
            }
            _ => {}
        };
        rc
    }

    pub fn process_main_protocol(&mut self, msg: Message) -> Result<(), TLSError> {
        if self.common.traffic
            && !self.common.is_tls13()
            && msg.is_handshake_type(HandshakeType::ClientHello)
        {
            self.common
                .send_warning_alert(AlertDescription::NoRenegotiation);
            return Ok(());
        }

        let state = self.state.take().unwrap();
        let maybe_next_state = state.handle(self, msg);
        let next_state = self.maybe_send_unexpected_alert(maybe_next_state)?;
        self.state = Some(next_state);

        Ok(())
    }

    pub fn process_new_packets(&mut self) -> Result<(), TLSError> {
        if let Some(ref err) = self.error {
            return Err(err.clone());
        }

        if self.common.message_deframer.desynced {
            return Err(TLSError::CorruptMessage);
        }

        while let Some(msg) = self
            .common
            .message_deframer
            .frames
            .pop_front()
        {
            match self.process_msg(msg) {
                Ok(_) => {}
                Err(err) => {
                    self.error = Some(err.clone());
                    return Err(err);
                }
            }
        }

        Ok(())
    }

    pub fn get_peer_certificates(&self) -> Option<Vec<key::Certificate>> {
        self.client_cert_chain
            .as_ref()
            .map(|chain| chain.iter().cloned().collect())
    }

    pub fn get_alpn_protocol(&self) -> Option<&[u8]> {
        self.alpn_protocol
            .as_ref()
            .map(AsRef::as_ref)
    }

    pub fn get_protocol_version(&self) -> Option<ProtocolVersion> {
        self.common.negotiated_version
    }

    pub fn get_negotiated_ciphersuite(&self) -> Option<&'static SupportedCipherSuite> {
        self.common.get_suite()
    }

    pub fn get_sni(&self) -> Option<&webpki::DNSName> {
        self.sni.as_ref()
    }

    pub fn set_sni(&mut self, value: webpki::DNSName) {
        // The SNI hostname is immutable once set.
        assert!(self.sni.is_none());
        self.sni = Some(value)
    }

    fn export_keying_material(
        &self,
        output: &mut [u8],
        label: &[u8],
        context: Option<&[u8]>,
    ) -> Result<(), TLSError> {
        self.state
            .as_ref()
            .ok_or_else(|| TLSError::HandshakeNotComplete)
            .and_then(|st| st.export_keying_material(output, label, context))
    }

    fn send_some_plaintext(&mut self, buf: &[u8]) -> usize {
        let mut st = self.state.take();
        st.as_mut()
            .map(|st| st.perhaps_write_key_update(self));
        self.state = st;
        self.common.send_some_plaintext(buf)
    }
}

/// This represents a single TLS server session.
///
/// Send TLS-protected data to the peer using the `io::Write` trait implementation.
/// Read data from the peer using the `io::Read` trait implementation.
#[derive(Debug)]
pub struct ServerSession {
    // We use the pimpl idiom to hide unimportant details.
    pub(crate) imp: ServerSessionImpl,
}

impl ServerSession {
    /// Make a new ServerSession.  `config` controls how
    /// we behave in the TLS protocol.
    pub fn new(config: &Arc<ServerConfig>) -> ServerSession {
        ServerSession {
            imp: ServerSessionImpl::new(config, vec![]),
        }
    }

    /// Retrieves the SNI hostname, if any, used to select the certificate and
    /// private key.
    ///
    /// This returns `None` until some time after the client's SNI extension
    /// value is processed during the handshake. It will never be `None` when
    /// the connection is ready to send or process application data, unless the
    /// client does not support SNI.
    ///
    /// This is useful for application protocols that need to enforce that the
    /// SNI hostname matches an application layer protocol hostname. For
    /// example, HTTP/1.1 servers commonly expect the `Host:` header field of
    /// every request on a connection to match the hostname in the SNI extension
    /// when the client provides the SNI extension.
    ///
    /// The SNI hostname is also used to match sessions during session
    /// resumption.
    pub fn get_sni_hostname(&self) -> Option<&str> {
        self.imp
            .get_sni()
            .map(|s| s.as_ref().into())
    }

    /// Application-controlled portion of the resumption ticket supplied by the client, if any.
    ///
    /// Recovered from the prior session's `set_resumption_data`. Integrity is guaranteed by rustls.
    ///
    /// Returns `Some` iff a valid resumption ticket has been received from the client.
    pub fn received_resumption_data(&self) -> Option<&[u8]> {
        self.imp
            .received_resumption_data
            .as_ref()
            .map(|x| &x[..])
    }

    /// Set the resumption data to embed in future resumption tickets supplied to the client.
    ///
    /// Defaults to the empty byte string. Must be less than 2^15 bytes to allow room for other
    /// data. Should be called while `is_handshaking` returns true to ensure all transmitted
    /// resumption tickets are affected.
    ///
    /// Integrity will be assured by rustls, but the data will be visible to the client. If secrecy
    /// from the client is desired, encrypt the data separately.
    pub fn set_resumption_data(&mut self, data: &[u8]) {
        assert!(data.len() < 2usize.pow(15));
        self.imp.resumption_data = data.into();
    }

    /// Explicitly discard early data, notifying the client
    ///
    /// Useful if invariants encoded in `received_resumption_data()` cannot be respected.
    ///
    /// Must be called while `is_handshaking` is true.
    pub fn reject_early_data(&mut self) {
        assert!(
            self.is_handshaking(),
            "cannot retroactively reject early data"
        );
        self.imp.reject_early_data = true;
    }
}

impl Session for ServerSession {
    fn read_tls(&mut self, rd: &mut dyn io::Read) -> io::Result<usize> {
        self.imp.common.read_tls(rd)
    }

    /// Writes TLS messages to `wr`.
    fn write_tls(&mut self, wr: &mut dyn io::Write) -> io::Result<usize> {
        self.imp.common.write_tls(wr)
    }

    fn process_new_packets(&mut self) -> Result<(), TLSError> {
        self.imp.process_new_packets()
    }

    fn wants_read(&self) -> bool {
        self.imp.wants_read()
    }

    fn wants_write(&self) -> bool {
        self.imp.wants_write()
    }

    fn is_handshaking(&self) -> bool {
        self.imp.is_handshaking()
    }

    fn set_buffer_limit(&mut self, len: usize) {
        self.imp.set_buffer_limit(len)
    }

    fn send_close_notify(&mut self) {
        self.imp.common.send_close_notify()
    }

    fn get_peer_certificates(&self) -> Option<Vec<key::Certificate>> {
        self.imp.get_peer_certificates()
    }

    fn get_alpn_protocol(&self) -> Option<&[u8]> {
        self.imp.get_alpn_protocol()
    }

    fn get_protocol_version(&self) -> Option<ProtocolVersion> {
        self.imp.get_protocol_version()
    }

    fn export_keying_material(
        &self,
        output: &mut [u8],
        label: &[u8],
        context: Option<&[u8]>,
    ) -> Result<(), TLSError> {
        self.imp
            .export_keying_material(output, label, context)
    }

    fn get_negotiated_ciphersuite(&self) -> Option<&'static SupportedCipherSuite> {
        self.imp.get_negotiated_ciphersuite()
    }
}

impl io::Read for ServerSession {
    /// Obtain plaintext data received from the peer over this TLS connection.
    ///
    /// If the peer closes the TLS session cleanly, this fails with an error of
    /// kind ErrorKind::ConnectionAborted once all the pending data has been read.
    /// No further data can be received on that connection, so the underlying TCP
    /// connection should closed too.
    ///
    /// Note that support close notify varies in peer TLS libraries: many do not
    /// support it and uncleanly close the TCP connection (this might be
    /// vulnerable to truncation attacks depending on the application protocol).
    /// This means applications using rustls must both handle ErrorKind::ConnectionAborted
    /// from this function, *and* unexpected closure of the underlying TCP connection.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.imp.common.read(buf)
    }
}

impl io::Write for ServerSession {
    /// Send the plaintext `buf` to the peer, encrypting
    /// and authenticating it.  Once this function succeeds
    /// you should call `write_tls` which will output the
    /// corresponding TLS records.
    ///
    /// This function buffers plaintext sent before the
    /// TLS handshake completes, and sends it as soon
    /// as it can.  This buffer is of *unlimited size* so
    /// writing much data before it can be sent will
    /// cause excess memory usage.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(self.imp.send_some_plaintext(buf))
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let mut sz = 0;
        for buf in bufs {
            sz += self.imp.send_some_plaintext(buf);
        }
        Ok(sz)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.imp.common.flush_plaintext();
        Ok(())
    }
}

// This is a test shim for the BoringSSL-Go ('bogo') TLS
// test suite. See bogo/ for this in action.
//
// https://boringssl.googlesource.com/boringssl/+/master/ssl/test
//

use base64;
use env_logger;
use rustls;
use sct;
use webpki;

use rustls::internal::msgs::enums::ProtocolVersion;
use rustls::quic::ClientQuicExt;
use rustls::quic::ServerQuicExt;
use rustls::ClientHello;
use std::env;
use std::fs;
use std::io;
use std::io::BufReader;
use std::io::Write;
use std::net;
use std::ops::{Deref, DerefMut};
use std::process;
use std::sync::Arc;

static BOGO_NACK: i32 = 89;

macro_rules! println_err(
  ($($arg:tt)*) => { {
    writeln!(&mut ::std::io::stderr(), $($arg)*).unwrap();
  } }
);

#[derive(Debug)]
struct Options {
    port: u16,
    server: bool,
    mtu: Option<usize>,
    resumes: usize,
    verify_peer: bool,
    require_any_client_cert: bool,
    offer_no_client_cas: bool,
    tickets: bool,
    resume_with_tickets_disabled: bool,
    queue_data: bool,
    shut_down_after_handshake: bool,
    check_close_notify: bool,
    host_name: String,
    use_sni: bool,
    send_sct: bool,
    key_file: String,
    cert_file: String,
    protocols: Vec<String>,
    support_tls13: bool,
    support_tls12: bool,
    min_version: Option<ProtocolVersion>,
    max_version: Option<ProtocolVersion>,
    server_ocsp_response: Vec<u8>,
    server_sct_list: Vec<u8>,
    use_signing_scheme: u16,
    expect_curve: u16,
    export_keying_material: usize,
    export_keying_material_label: String,
    export_keying_material_context: String,
    export_keying_material_context_used: bool,
    read_size: usize,
    quic_transport_params: Vec<u8>,
    expect_quic_transport_params: Vec<u8>,
    enable_early_data: bool,
    expect_ticket_supports_early_data: bool,
    expect_accept_early_data: bool,
    expect_reject_early_data: bool,
    queue_data_on_resume: bool,
    expect_version: u16,
}

impl Options {
    fn new() -> Options {
        Options {
            port: 0,
            server: false,
            mtu: None,
            resumes: 0,
            verify_peer: false,
            tickets: true,
            resume_with_tickets_disabled: false,
            host_name: "example.com".to_string(),
            use_sni: false,
            send_sct: false,
            queue_data: false,
            shut_down_after_handshake: false,
            check_close_notify: false,
            require_any_client_cert: false,
            offer_no_client_cas: false,
            key_file: "".to_string(),
            cert_file: "".to_string(),
            protocols: vec![],
            support_tls13: true,
            support_tls12: true,
            min_version: None,
            max_version: None,
            server_ocsp_response: vec![],
            server_sct_list: vec![],
            use_signing_scheme: 0,
            expect_curve: 0,
            export_keying_material: 0,
            export_keying_material_label: "".to_string(),
            export_keying_material_context: "".to_string(),
            export_keying_material_context_used: false,
            read_size: 512,
            quic_transport_params: vec![],
            expect_quic_transport_params: vec![],
            enable_early_data: false,
            expect_ticket_supports_early_data: false,
            expect_accept_early_data: false,
            expect_reject_early_data: false,
            queue_data_on_resume: false,
            expect_version: 0,
        }
    }

    fn version_allowed(&self, vers: ProtocolVersion) -> bool {
        (self.min_version.is_none() || vers.get_u16() >= self.min_version.unwrap().get_u16())
            && (self.max_version.is_none() || vers.get_u16() <= self.max_version.unwrap().get_u16())
    }

    fn tls13_supported(&self) -> bool {
        self.support_tls13
            && (self.version_allowed(ProtocolVersion::TLSv1_3)
                || self.version_allowed(ProtocolVersion::Unknown(0x7f17)))
    }

    fn tls12_supported(&self) -> bool {
        self.support_tls12 && self.version_allowed(ProtocolVersion::TLSv1_2)
    }
}

fn load_cert(filename: &str) -> Vec<rustls::Certificate> {
    let certfile = fs::File::open(filename).expect("cannot open certificate file");
    let mut reader = BufReader::new(certfile);
    rustls::internal::pemfile::certs(&mut reader).unwrap()
}

fn load_key(filename: &str) -> rustls::PrivateKey {
    let keyfile = fs::File::open(filename).expect("cannot open private key file");
    let mut reader = BufReader::new(keyfile);
    let keys = rustls::internal::pemfile::pkcs8_private_keys(&mut reader).unwrap();
    assert!(keys.len() == 1);
    keys[0].clone()
}

fn split_protocols(protos: &str) -> Vec<String> {
    let mut ret = Vec::new();

    let mut offs = 0;
    while offs < protos.len() {
        let len = protos.as_bytes()[offs] as usize;
        let item = protos[offs + 1..offs + 1 + len].to_string();
        ret.push(item);
        offs += 1 + len;
    }

    ret
}

struct DummyClientAuth {
    mandatory: bool,
}

impl rustls::ClientCertVerifier for DummyClientAuth {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self, _sni: Option<&webpki::DNSName>) -> Option<bool> {
        Some(self.mandatory)
    }

    fn client_auth_root_subjects(
        &self,
        _sni: Option<&webpki::DNSName>,
    ) -> Option<rustls::DistinguishedNames> {
        Some(rustls::DistinguishedNames::new())
    }

    fn verify_client_cert(
        &self,
        _certs: &[rustls::Certificate],
        _sni: Option<&webpki::DNSName>,
    ) -> Result<rustls::ClientCertVerified, rustls::TLSError> {
        Ok(rustls::ClientCertVerified::assertion())
    }
}

struct DummyServerAuth {}

impl rustls::ServerCertVerifier for DummyServerAuth {
    fn verify_server_cert(
        &self,
        _roots: &rustls::RootCertStore,
        _certs: &[rustls::Certificate],
        _hostname: webpki::DNSNameRef<'_>,
        _ocsp: &[u8],
    ) -> Result<rustls::ServerCertVerified, rustls::TLSError> {
        Ok(rustls::ServerCertVerified::assertion())
    }
}

struct FixedSignatureSchemeSigningKey {
    key: Arc<Box<dyn rustls::sign::SigningKey>>,
    scheme: rustls::SignatureScheme,
}

impl rustls::sign::SigningKey for FixedSignatureSchemeSigningKey {
    fn choose_scheme(
        &self,
        offered: &[rustls::SignatureScheme],
    ) -> Option<Box<dyn rustls::sign::Signer>> {
        if offered.contains(&self.scheme) {
            self.key.choose_scheme(&[self.scheme])
        } else {
            self.key.choose_scheme(&[])
        }
    }
    fn algorithm(&self) -> rustls::internal::msgs::enums::SignatureAlgorithm {
        self.key.algorithm()
    }
}

struct FixedSignatureSchemeServerCertResolver {
    resolver: Arc<dyn rustls::ResolvesServerCert>,
    scheme: rustls::SignatureScheme,
}

impl rustls::ResolvesServerCert for FixedSignatureSchemeServerCertResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<rustls::sign::CertifiedKey> {
        let mut certkey = self.resolver.resolve(client_hello)?;
        certkey.key = Arc::new(Box::new(FixedSignatureSchemeSigningKey {
            key: certkey.key.clone(),
            scheme: self.scheme,
        }));
        Some(certkey)
    }
}

struct FixedSignatureSchemeClientCertResolver {
    resolver: Arc<dyn rustls::ResolvesClientCert>,
    scheme: rustls::SignatureScheme,
}

impl rustls::ResolvesClientCert for FixedSignatureSchemeClientCertResolver {
    fn resolve(
        &self,
        acceptable_issuers: &[&[u8]],
        sigschemes: &[rustls::SignatureScheme],
    ) -> Option<rustls::sign::CertifiedKey> {
        if !sigschemes.contains(&self.scheme) {
            quit(":NO_COMMON_SIGNATURE_ALGORITHMS:");
        }
        let mut certkey = self
            .resolver
            .resolve(acceptable_issuers, sigschemes)?;
        certkey.key = Arc::new(Box::new(FixedSignatureSchemeSigningKey {
            key: certkey.key.clone(),
            scheme: self.scheme,
        }));
        Some(certkey)
    }

    fn has_certs(&self) -> bool {
        self.resolver.has_certs()
    }
}

fn lookup_scheme(scheme: u16) -> rustls::SignatureScheme {
    match scheme {
        0x0401 => rustls::SignatureScheme::RSA_PKCS1_SHA256,
        0x0501 => rustls::SignatureScheme::RSA_PKCS1_SHA384,
        0x0601 => rustls::SignatureScheme::RSA_PKCS1_SHA512,
        0x0403 => rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
        0x0503 => rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
        0x0804 => rustls::SignatureScheme::RSA_PSS_SHA256,
        0x0805 => rustls::SignatureScheme::RSA_PSS_SHA384,
        0x0806 => rustls::SignatureScheme::RSA_PSS_SHA512,
        0x0807 => rustls::SignatureScheme::ED25519,
        // TODO: add support for Ed448
        // 0x0808 => rustls::SignatureScheme::ED448,
        _ => {
            println_err!("Unsupported signature scheme {:04x}", scheme);
            process::exit(BOGO_NACK);
        }
    }
}

fn make_server_cfg(opts: &Options) -> Arc<rustls::ServerConfig> {
    let client_auth =
        if opts.verify_peer || opts.offer_no_client_cas || opts.require_any_client_cert {
            Arc::new(DummyClientAuth {
                mandatory: opts.require_any_client_cert,
            })
        } else {
            rustls::NoClientAuth::new()
        };

    let mut cfg = rustls::ServerConfig::new(client_auth);
    let persist = rustls::ServerSessionMemoryCache::new(32);
    cfg.set_persistence(persist);

    cfg.mtu = opts.mtu;

    let cert = load_cert(&opts.cert_file);
    let key = load_key(&opts.key_file);
    cfg.set_single_cert_with_ocsp_and_sct(
        cert.clone(),
        key,
        opts.server_ocsp_response.clone(),
        opts.server_sct_list.clone(),
    )
    .unwrap();
    if opts.use_signing_scheme > 0 {
        let scheme = lookup_scheme(opts.use_signing_scheme);
        cfg.cert_resolver = Arc::new(FixedSignatureSchemeServerCertResolver {
            resolver: cfg.cert_resolver.clone(),
            scheme,
        });
    }

    if opts.tickets {
        cfg.ticketer = rustls::Ticketer::new();
    } else if opts.resumes == 0 {
        cfg.set_persistence(Arc::new(rustls::NoServerSessionStorage {}));
    }

    if !opts.protocols.is_empty() {
        cfg.set_protocols(
            &opts
                .protocols
                .iter()
                .map(|proto| proto.as_bytes().to_vec())
                .collect::<Vec<_>>()[..],
        );
    }

    cfg.versions.clear();

    if opts.tls12_supported() {
        cfg.versions
            .push(ProtocolVersion::TLSv1_2);
    }

    if opts.tls13_supported() {
        cfg.versions
            .push(ProtocolVersion::TLSv1_3);
    }

    Arc::new(cfg)
}

static EMPTY_LOGS: [&sct::Log<'_>; 0] = [];

struct ClientCacheWithoutKxHints(Arc<rustls::ClientSessionMemoryCache>);

impl ClientCacheWithoutKxHints {
    fn new() -> Arc<ClientCacheWithoutKxHints> {
        Arc::new(ClientCacheWithoutKxHints(
            rustls::ClientSessionMemoryCache::new(32),
        ))
    }
}

impl rustls::StoresClientSessions for ClientCacheWithoutKxHints {
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> bool {
        if key.len() > 2 && key[0] == b'k' && key[1] == b'x' {
            true
        } else {
            self.0.put(key, value)
        }
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.0.get(key)
    }
}

fn make_client_cfg(opts: &Options) -> Arc<rustls::ClientConfig> {
    let mut cfg = rustls::ClientConfig::new();
    let persist = ClientCacheWithoutKxHints::new();
    cfg.set_persistence(persist);
    cfg.root_store
        .add(&load_cert("cert.pem")[0])
        .unwrap();
    cfg.enable_sni = opts.use_sni;
    cfg.mtu = opts.mtu;

    if opts.send_sct {
        cfg.ct_logs = Some(&EMPTY_LOGS);
    }

    if !opts.cert_file.is_empty() && !opts.key_file.is_empty() {
        let cert = load_cert(&opts.cert_file);
        let key = load_key(&opts.key_file);
        cfg.set_single_client_cert(cert, key)
            .unwrap();
    }

    if !opts.cert_file.is_empty() && opts.use_signing_scheme > 0 {
        let scheme = lookup_scheme(opts.use_signing_scheme);
        cfg.client_auth_cert_resolver = Arc::new(FixedSignatureSchemeClientCertResolver {
            resolver: cfg.client_auth_cert_resolver.clone(),
            scheme,
        });
    }

    cfg.dangerous()
        .set_certificate_verifier(Arc::new(DummyServerAuth {}));

    if !opts.protocols.is_empty() {
        cfg.set_protocols(
            &opts
                .protocols
                .iter()
                .map(|proto| proto.as_bytes().to_vec())
                .collect::<Vec<_>>()[..],
        );
    }

    cfg.versions.clear();

    if opts.tls12_supported() {
        cfg.versions
            .push(ProtocolVersion::TLSv1_2);
    }

    if opts.tls13_supported() {
        cfg.versions
            .push(ProtocolVersion::TLSv1_3);
    }

    if opts.enable_early_data {
        cfg.enable_early_data = true;
    }

    Arc::new(cfg)
}

fn quit(why: &str) -> ! {
    println_err!("{}", why);
    process::exit(0)
}

fn quit_err(why: &str) -> ! {
    println_err!("{}", why);
    process::exit(1)
}

fn handle_err(err: rustls::TLSError) -> ! {
    use rustls::internal::msgs::enums::{AlertDescription, ContentType};
    use rustls::TLSError;
    use std::{thread, time};

    println!("TLS error: {:?}", err);
    thread::sleep(time::Duration::from_millis(100));

    match err {
        TLSError::InappropriateHandshakeMessage { .. } | TLSError::InappropriateMessage { .. } => {
            quit(":UNEXPECTED_MESSAGE:")
        }
        TLSError::AlertReceived(AlertDescription::RecordOverflow) => {
            quit(":TLSV1_ALERT_RECORD_OVERFLOW:")
        }
        TLSError::AlertReceived(AlertDescription::HandshakeFailure) => quit(":HANDSHAKE_FAILURE:"),
        TLSError::AlertReceived(AlertDescription::ProtocolVersion) => quit(":WRONG_VERSION:"),
        TLSError::AlertReceived(AlertDescription::InternalError) => {
            quit(":PEER_ALERT_INTERNAL_ERROR:")
        }
        TLSError::CorruptMessagePayload(ContentType::Alert) => quit(":BAD_ALERT:"),
        TLSError::CorruptMessagePayload(ContentType::ChangeCipherSpec) => {
            quit(":BAD_CHANGE_CIPHER_SPEC:")
        }
        TLSError::CorruptMessagePayload(ContentType::Handshake) => quit(":BAD_HANDSHAKE_MSG:"),
        TLSError::CorruptMessagePayload(ContentType::Unknown(42)) => quit(":GARBAGE:"),
        TLSError::CorruptMessage => quit(":GARBAGE:"),
        TLSError::DecryptError => quit(":DECRYPTION_FAILED_OR_BAD_RECORD_MAC:"),
        TLSError::PeerIncompatibleError(_) => quit(":INCOMPATIBLE:"),
        TLSError::PeerMisbehavedError(_) => quit(":PEER_MISBEHAVIOUR:"),
        TLSError::NoCertificatesPresented => quit(":NO_CERTS:"),
        TLSError::AlertReceived(AlertDescription::UnexpectedMessage) => quit(":BAD_ALERT:"),
        TLSError::AlertReceived(AlertDescription::DecompressionFailure) => {
            quit_err(":SSLV3_ALERT_DECOMPRESSION_FAILURE:")
        }
        TLSError::WebPKIError(webpki::Error::BadDER) => quit(":CANNOT_PARSE_LEAF_CERT:"),
        TLSError::WebPKIError(webpki::Error::InvalidSignatureForPublicKey) => {
            quit(":BAD_SIGNATURE:")
        }
        TLSError::WebPKIError(webpki::Error::UnsupportedSignatureAlgorithmForPublicKey) => {
            quit(":WRONG_SIGNATURE_TYPE:")
        }
        TLSError::PeerSentOversizedRecord => quit(":DATA_LENGTH_TOO_LONG:"),
        _ => {
            println_err!("unhandled error: {:?}", err);
            quit(":FIXME:")
        }
    }
}

fn flush(sess: &mut ClientOrServer, conn: &mut net::TcpStream) {
    while sess.wants_write() {
        match sess.write_tls(conn) {
            Err(err) => {
                println!("IO error: {:?}", err);
                process::exit(0);
            }
            Ok(_) => {}
        }
    }
    conn.flush().unwrap();
}

enum ClientOrServer {
    Client(rustls::ClientSession),
    Server(rustls::ServerSession),
}

impl Deref for ClientOrServer {
    type Target = dyn rustls::Session;

    fn deref(&self) -> &Self::Target {
        match &self {
            ClientOrServer::Client(ref c) => c,
            ClientOrServer::Server(ref s) => s,
        }
    }
}

impl DerefMut for ClientOrServer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            ClientOrServer::Client(ref mut c) => c,
            ClientOrServer::Server(ref mut s) => s,
        }
    }
}

impl ClientOrServer {
    fn client(&mut self) -> &mut rustls::ClientSession {
        match self {
            ClientOrServer::Client(ref mut c) => c,
            ClientOrServer::Server(_) => panic!("ClientSession required here"),
        }
    }
}

fn exec(opts: &Options, mut sess: ClientOrServer, count: usize) {
    if opts.queue_data || (opts.queue_data_on_resume && count > 0) {
        if count > 0 && opts.enable_early_data {
            let len = sess
                .client()
                .early_data()
                .expect("0rtt not available")
                .write(b"hello")
                .expect("0rtt write failed");
            sess.write_all(&b"hello"[len..])
                .unwrap();
        } else {
            let _ = sess.write_all(b"hello");
        }
    }

    let addrs = [
        net::SocketAddr::from((net::Ipv4Addr::LOCALHOST, opts.port)),
        net::SocketAddr::from((net::Ipv6Addr::LOCALHOST, opts.port)),
    ];
    let mut conn = net::TcpStream::connect(&addrs[..]).expect("cannot connect");
    let mut sent_shutdown = false;
    let mut seen_eof = false;
    let mut sent_exporter = false;

    loop {
        flush(&mut sess, &mut conn);

        if sess.wants_read() {
            let len = match sess.read_tls(&mut conn) {
                Ok(len) => len,
                Err(ref err) if err.kind() == io::ErrorKind::ConnectionReset => 0,
                err @ Err(_) => err.expect("read failed"),
            };

            if len == 0 {
                if opts.check_close_notify {
                    if !seen_eof {
                        seen_eof = true;
                    } else {
                        quit_err(":CLOSE_WITHOUT_CLOSE_NOTIFY:");
                    }
                } else {
                    println!("EOF (plain)");
                    return;
                }
            }

            if let Err(err) = sess.process_new_packets() {
                flush(&mut sess, &mut conn); /* send any alerts before exiting */
                handle_err(err);
            }
        }

        if !sess.is_handshaking() && opts.export_keying_material > 0 && !sent_exporter {
            let mut export = Vec::new();
            export.resize(opts.export_keying_material, 0u8);
            sess.export_keying_material(
                &mut export,
                opts.export_keying_material_label
                    .as_bytes(),
                if opts.export_keying_material_context_used {
                    Some(
                        opts.export_keying_material_context
                            .as_bytes(),
                    )
                } else {
                    None
                },
            )
            .unwrap();
            sess.write_all(&export).unwrap();
            sent_exporter = true;
        }

        if opts.enable_early_data && !sess.is_handshaking() && count > 0 {
            if opts.expect_accept_early_data && !sess.client().is_early_data_accepted() {
                quit_err("Early data was not accepted, but we expect the opposite");
            } else if opts.expect_reject_early_data && sess.client().is_early_data_accepted() {
                quit_err("Early data was accepted, but we expect the opposite");
            }
            if opts.expect_version == 0x0304 {
                match sess.get_protocol_version() {
                    Some(ProtocolVersion::TLSv1_3) | Some(ProtocolVersion::Unknown(0x7f17)) => {}
                    _ => quit_err("wrong protocol version"),
                }
            }
        }

        if !sess.is_handshaking()
            && !opts
                .expect_quic_transport_params
                .is_empty()
        {
            let their_transport_params = sess
                .get_quic_transport_parameters()
                .expect("missing peer quic transport params");
            assert_eq!(opts.expect_quic_transport_params, their_transport_params);
        }

        let mut buf = [0u8; 1024];
        let len = match sess.read(&mut buf[..opts.read_size]) {
            Ok(len) => len,
            Err(ref err) if err.kind() == io::ErrorKind::ConnectionAborted => {
                if opts.check_close_notify {
                    println!("close notify ok");
                }
                println!("EOF (tls)");
                return;
            }
            Err(err) => panic!("unhandled read error {:?}", err),
        };

        if opts.shut_down_after_handshake && !sent_shutdown && !sess.is_handshaking() {
            sess.send_close_notify();
            sent_shutdown = true;
        }

        for b in buf.iter_mut() {
            *b ^= 0xff;
        }

        sess.write_all(&buf[..len]).unwrap();
    }
}

fn main() {
    let mut args: Vec<_> = env::args().collect();
    env_logger::init();

    args.remove(0);

    if !args.is_empty() && args[0] == "-is-handshaker-supported" {
        println!("No");
        process::exit(0);
    }
    println!("options: {:?}", args);

    let mut opts = Options::new();

    while !args.is_empty() {
        let arg = args.remove(0);
        match arg.as_ref() {
            "-port" => {
                opts.port = args.remove(0).parse::<u16>().unwrap();
            }
            "-server" => {
                opts.server = true;
            }
            "-key-file" => {
                opts.key_file = args.remove(0);
            }
            "-cert-file" => {
                opts.cert_file = args.remove(0);
            }
            "-resume-count" => {
                opts.resumes = args.remove(0).parse::<usize>().unwrap();
            }
            "-no-tls13" => {
                opts.support_tls13 = false;
            }
            "-no-tls12" => {
                opts.support_tls12 = false;
            }
            "-min-version" => {
                let min = args.remove(0).parse::<u16>().unwrap();
                opts.min_version = Some(ProtocolVersion::Unknown(min));
            }
            "-max-version" => {
                let max = args.remove(0).parse::<u16>().unwrap();
                opts.max_version = Some(ProtocolVersion::Unknown(max));
            }
            "-max-send-fragment" => {
                let mtu = args.remove(0).parse::<usize>().unwrap();
                opts.mtu = Some(mtu);
            }
            "-read-size" => {
                let rdsz = args.remove(0).parse::<usize>().unwrap();
                opts.read_size = rdsz;
            }
            "-tls13-variant" => {
                let variant = args.remove(0).parse::<u16>().unwrap();
                if variant != 1 {
                    println!("NYI TLS1.3 variant selection: {:?} {:?}", arg, variant);
                    process::exit(BOGO_NACK);
                }
            }
            "-no-ticket" => {
                opts.tickets = false;
            }
            "-on-resume-no-ticket" => {
                opts.resume_with_tickets_disabled = true;
            }
            "-signing-prefs" => {
                let alg = args.remove(0).parse::<u16>().unwrap();
                opts.use_signing_scheme = alg;
            }
            "-max-cert-list" |
            "-expect-curve-id" |
            "-expect-resume-curve-id" |
            "-expect-peer-signature-algorithm" |
            "-expect-peer-verify-pref" |
            "-expect-advertised-alpn" |
            "-expect-alpn" |
            "-on-initial-expect-alpn" |
            "-on-resume-expect-alpn" |
            "-on-retry-expect-alpn" |
            "-expect-server-name" |
            "-expect-ocsp-response" |
            "-expect-signed-cert-timestamps" |
            "-expect-certificate-types" |
            "-expect-client-ca-list" |
            "-on-retry-expect-early-data-reason" |
            "-on-resume-expect-early-data-reason" |
            "-on-initial-expect-early-data-reason" |
            "-handshaker-path" |
            "-expect-msg-callback" => {
                println!("not checking {} {}; NYI", arg, args.remove(0));
            }

            "-expect-secure-renegotiation" |
            "-expect-no-session-id" |
            "-enable-ed25519" |
            "-expect-hrr" |
            "-expect-no-hrr" |
            "-on-resume-expect-no-offer-early-data" |
            "-key-update" | //< we could implement an API for this
            "-expect-tls13-downgrade" |
            "-expect-session-id" => {
                println!("not checking {}; NYI", arg);
            }

            "-export-keying-material" => {
                opts.export_keying_material = args.remove(0).parse::<usize>().unwrap();
            }
            "-export-label" => {
                opts.export_keying_material_label = args.remove(0);
            }
            "-export-context" => {
                opts.export_keying_material_context = args.remove(0);
            }
            "-use-export-context" => {
                opts.export_keying_material_context_used = true;
            }
            "-quic-transport-params" => {
                opts.quic_transport_params = base64::decode(args.remove(0).as_bytes())
                    .expect("invalid base64");
            }
            "-expect-quic-transport-params" => {
                opts.expect_quic_transport_params = base64::decode(args.remove(0).as_bytes())
                    .expect("invalid base64");
            }

            "-ocsp-response" => {
                opts.server_ocsp_response = base64::decode(args.remove(0).as_bytes())
                    .expect("invalid base64");
            }
            "-signed-cert-timestamps" => {
                opts.server_sct_list = base64::decode(args.remove(0).as_bytes())
                    .expect("invalid base64");

                if opts.server_sct_list.len() == 2 &&
                    opts.server_sct_list[0] == 0x00 &&
                    opts.server_sct_list[1] == 0x00 {
                    quit(":INVALID_SCT_LIST:");
                }
            }
            "-select-alpn" => {
                opts.protocols.push(args.remove(0));
            }
            "-require-any-client-certificate" => {
                opts.require_any_client_cert = true;
            }
            "-verify-peer" => {
                opts.verify_peer = true;
            }
            "-shim-writes-first" => {
                opts.queue_data = true;
            }
            "-shim-shuts-down" => {
                opts.shut_down_after_handshake = true;
            }
            "-check-close-notify" => {
                opts.check_close_notify = true;
            }
            "-host-name" => {
                opts.host_name = args.remove(0);
                opts.use_sni = true;
            }
            "-advertise-alpn" => {
                opts.protocols = split_protocols(&args.remove(0));
            }
            "-use-null-client-ca-list" => {
                opts.offer_no_client_cas = true;
            }
            "-enable-signed-cert-timestamps" => {
                opts.send_sct = true;
            }
            "-enable-early-data" |
            "-on-resume-enable-early-data" => {
                opts.enable_early_data = true;
            }
            "-on-resume-shim-writes-first" => {
                opts.queue_data_on_resume = true;
            }
            "-expect-ticket-supports-early-data" => {
                opts.expect_ticket_supports_early_data = true;
            }
            "-expect-accept-early-data" |
            "-on-resume-expect-accept-early-data" => {
                opts.expect_accept_early_data = true;
            }
            "-expect-early-data-reason" |
            "-on-resume-expect-reject-early-data-reason" => {
                let reason = args.remove(0);
                match reason.as_str() {
                    "disabled" | "protocol_version" => {
                        opts.expect_reject_early_data = true;
                    }
                    _ => {
                        println!("NYI early data reason: {}", reason);
                        process::exit(1);
                    }
                }
            }
            "-expect-reject-early-data" |
            "-on-resume-expect-reject-early-data" => {
                opts.expect_reject_early_data = true;
            }
            "-expect-version" => {
                opts.expect_version = args.remove(0).parse::<u16>().unwrap();
            }

            // defaults:
            "-enable-all-curves" |
            "-renegotiate-ignore" |
            "-no-tls11" |
            "-no-tls1" |
            "-no-ssl3" |
            "-handoff" |
            "-decline-alpn" |
            "-expect-no-session" |
            "-expect-session-miss" |
            "-expect-extended-master-secret" |
            "-expect-ticket-renewal" |
            "-enable-ocsp-stapling" |
            // internal openssl details:
            "-async" |
            "-implicit-handshake" |
            "-use-old-client-cert-callback" |
            "-use-early-callback" => {}

            // Not implemented things
            "-dtls" |
            "-cipher" |
            "-psk" |
            "-renegotiate-freely" |
            "-false-start" |
            "-fallback-scsv" |
            "-fail-early-callback" |
            "-fail-cert-callback" |
            "-install-ddos-callback" |
            "-advertise-npn" |
            "-verify-fail" |
            "-expect-channel-id" |
            "-send-channel-id" |
            "-select-next-proto" |
            "-p384-only" |
            "-expect-verify-result" |
            "-send-alert" |
            "-digest-prefs" |
            "-use-exporter-between-reads" |
            "-ticket-key" |
            "-tls-unique" |
            "-curves" |
            "-enable-server-custom-extension" |
            "-enable-client-custom-extension" |
            "-expect-dhe-group-size" |
            "-use-ticket-callback" |
            "-enable-grease" |
            "-enable-channel-id" |
            "-resumption-delay" |
            "-expect-early-data-info" |
            "-expect-cipher-aes" |
            "-retain-only-sha256-client-cert-initial" |
            "-use-client-ca-list" |
            "-expect-draft-downgrade" |
            "-allow-unknown-alpn-protos" |
            "-on-initial-tls13-variant" |
            "-on-initial-expect-curve-id" |
            "-on-resume-export-early-keying-material" |
            "-export-early-keying-material" |
            "-handshake-twice" |
            "-on-resume-verify-fail" |
            "-reverify-on-resume" |
            "-verify-prefs" |
            "-no-op-extra-handshake" |
            "-read-with-unfinished-write" |
            "-on-resume-read-with-unfinished-write" |
            "-expect-peer-cert-file" |
            "-no-rsa-pss-rsae-certs" |
            "-ignore-tls13-downgrade" |
            "-on-initial-expect-peer-cert-file" => {
                println!("NYI option {:?}", arg);
                process::exit(BOGO_NACK);
            }

            _ => {
                println!("unhandled option {:?}", arg);
                process::exit(1);
            }
        }
    }

    if opts.enable_early_data && opts.server {
        println!("For now we only test client-side early data");
        process::exit(BOGO_NACK);
    }

    println!("opts {:?}", opts);

    let mut server_cfg = if opts.server {
        Some(make_server_cfg(&opts))
    } else {
        None
    };
    let client_cfg = if !opts.server {
        Some(make_client_cfg(&opts))
    } else {
        None
    };

    fn make_session(
        opts: &Options,
        scfg: &Option<Arc<rustls::ServerConfig>>,
        ccfg: &Option<Arc<rustls::ClientConfig>>,
    ) -> ClientOrServer {
        if opts.server {
            let s = if opts.quic_transport_params.is_empty() {
                rustls::ServerSession::new(scfg.as_ref().unwrap())
            } else {
                rustls::ServerSession::new_quic(
                    scfg.as_ref().unwrap(),
                    opts.quic_transport_params.clone(),
                )
            };
            ClientOrServer::Server(s)
        } else {
            let dns_name = webpki::DNSNameRef::try_from_ascii_str(&opts.host_name).unwrap();
            let c = if opts.quic_transport_params.is_empty() {
                rustls::ClientSession::new(ccfg.as_ref().unwrap(), dns_name)
            } else {
                rustls::ClientSession::new_quic(
                    ccfg.as_ref().unwrap(),
                    dns_name,
                    opts.quic_transport_params.clone(),
                )
            };
            ClientOrServer::Client(c)
        }
    }

    for i in 0..opts.resumes + 1 {
        let sess = make_session(&opts, &server_cfg, &client_cfg);
        exec(&opts, sess, i);

        if opts.resume_with_tickets_disabled {
            server_cfg = {
                let mut newcfg = server_cfg.unwrap();
                let default = rustls::ServerConfig::new(rustls::NoClientAuth::new());
                Arc::make_mut(&mut newcfg).ticketer = default.ticketer.clone();
                Some(newcfg)
            };
        }
    }
}

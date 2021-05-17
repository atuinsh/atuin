use std::io;
use std::sync::Arc;

use rustls;

use rustls::internal::msgs::{codec::Codec, codec::Reader, message::Message};
use rustls::internal::pemfile;
use rustls::ProtocolVersion;
use rustls::Session;
use rustls::TLSError;
use rustls::{AllowAnyAuthenticatedClient, NoClientAuth, RootCertStore};
use rustls::{Certificate, PrivateKey};
use rustls::{ClientConfig, ClientSession};
use rustls::{ServerConfig, ServerSession};

#[cfg(feature = "dangerous_configuration")]
use rustls::{
    ClientCertVerified, ClientCertVerifier, DistinguishedNames, SignatureScheme, WebPKIVerifier,
};

use webpki;

macro_rules! embed_files {
    (
        $(
            ($name:ident, $keytype:expr, $path:expr);
        )+
    ) => {
        $(
            const $name: &'static [u8] = include_bytes!(
                concat!("../../../test-ca/", $keytype, "/", $path));
        )+

        pub fn bytes_for(keytype: &str, path: &str) -> &'static [u8] {
            match (keytype, path) {
                $(
                    ($keytype, $path) => $name,
                )+
                _ => panic!("unknown keytype {} with path {}", keytype, path),
            }
        }
    }
}

embed_files! {
    (ECDSA_CA_CERT, "ecdsa", "ca.cert");
    (ECDSA_CA_DER, "ecdsa", "ca.der");
    (ECDSA_CA_KEY, "ecdsa", "ca.key");
    (ECDSA_CLIENT_CERT, "ecdsa", "client.cert");
    (ECDSA_CLIENT_CHAIN, "ecdsa", "client.chain");
    (ECDSA_CLIENT_FULLCHAIN, "ecdsa", "client.fullchain");
    (ECDSA_CLIENT_KEY, "ecdsa", "client.key");
    (ECDSA_CLIENT_REQ, "ecdsa", "client.req");
    (ECDSA_END_CERT, "ecdsa", "end.cert");
    (ECDSA_END_CHAIN, "ecdsa", "end.chain");
    (ECDSA_END_FULLCHAIN, "ecdsa", "end.fullchain");
    (ECDSA_END_KEY, "ecdsa", "end.key");
    (ECDSA_END_REQ, "ecdsa", "end.req");
    (ECDSA_INTER_CERT, "ecdsa", "inter.cert");
    (ECDSA_INTER_KEY, "ecdsa", "inter.key");
    (ECDSA_INTER_REQ, "ecdsa", "inter.req");
    (ECDSA_NISTP256_PEM, "ecdsa", "nistp256.pem");
    (ECDSA_NISTP384_PEM, "ecdsa", "nistp384.pem");

    (EDDSA_CA_CERT, "eddsa", "ca.cert");
    (EDDSA_CA_DER, "eddsa", "ca.der");
    (EDDSA_CA_KEY, "eddsa", "ca.key");
    (EDDSA_CLIENT_CERT, "eddsa", "client.cert");
    (EDDSA_CLIENT_CHAIN, "eddsa", "client.chain");
    (EDDSA_CLIENT_FULLCHAIN, "eddsa", "client.fullchain");
    (EDDSA_CLIENT_KEY, "eddsa", "client.key");
    (EDDSA_CLIENT_REQ, "eddsa", "client.req");
    (EDDSA_END_CERT, "eddsa", "end.cert");
    (EDDSA_END_CHAIN, "eddsa", "end.chain");
    (EDDSA_END_FULLCHAIN, "eddsa", "end.fullchain");
    (EDDSA_END_KEY, "eddsa", "end.key");
    (EDDSA_END_REQ, "eddsa", "end.req");
    (EDDSA_INTER_CERT, "eddsa", "inter.cert");
    (EDDSA_INTER_KEY, "eddsa", "inter.key");
    (EDDSA_INTER_REQ, "eddsa", "inter.req");

    (RSA_CA_CERT, "rsa", "ca.cert");
    (RSA_CA_DER, "rsa", "ca.der");
    (RSA_CA_KEY, "rsa", "ca.key");
    (RSA_CLIENT_CERT, "rsa", "client.cert");
    (RSA_CLIENT_CHAIN, "rsa", "client.chain");
    (RSA_CLIENT_FULLCHAIN, "rsa", "client.fullchain");
    (RSA_CLIENT_KEY, "rsa", "client.key");
    (RSA_CLIENT_REQ, "rsa", "client.req");
    (RSA_CLIENT_RSA, "rsa", "client.rsa");
    (RSA_END_CERT, "rsa", "end.cert");
    (RSA_END_CHAIN, "rsa", "end.chain");
    (RSA_END_FULLCHAIN, "rsa", "end.fullchain");
    (RSA_END_KEY, "rsa", "end.key");
    (RSA_END_REQ, "rsa", "end.req");
    (RSA_END_RSA, "rsa", "end.rsa");
    (RSA_INTER_CERT, "rsa", "inter.cert");
    (RSA_INTER_KEY, "rsa", "inter.key");
    (RSA_INTER_REQ, "rsa", "inter.req");
}

pub fn transfer(left: &mut dyn Session, right: &mut dyn Session) -> usize {
    let mut buf = [0u8; 262144];
    let mut total = 0;

    while left.wants_write() {
        let sz = {
            let into_buf: &mut dyn io::Write = &mut &mut buf[..];
            left.write_tls(into_buf).unwrap()
        };
        total += sz;
        if sz == 0 {
            return total;
        }

        let mut offs = 0;
        loop {
            let from_buf: &mut dyn io::Read = &mut &buf[offs..sz];
            offs += right.read_tls(from_buf).unwrap();
            if sz == offs {
                break;
            }
        }
    }

    total
}

pub fn transfer_altered<F>(left: &mut dyn Session, filter: F, right: &mut dyn Session) -> usize
where
    F: Fn(&mut Message),
{
    let mut buf = [0u8; 262144];
    let mut total = 0;

    while left.wants_write() {
        let sz = {
            let into_buf: &mut dyn io::Write = &mut &mut buf[..];
            left.write_tls(into_buf).unwrap()
        };
        total += sz;
        if sz == 0 {
            return total;
        }

        let mut reader = Reader::init(&buf[..sz]);
        while reader.any_left() {
            let mut message = Message::read(&mut reader).unwrap();
            message.decode_payload();
            filter(&mut message);
            let message_enc = message.get_encoding();
            let message_enc_reader: &mut dyn io::Read = &mut &message_enc[..];
            let len = right
                .read_tls(message_enc_reader)
                .unwrap();
            assert_eq!(len, message_enc.len());
        }
    }

    total
}

#[derive(Clone, Copy, PartialEq)]
pub enum KeyType {
    RSA,
    ECDSA,
    ED25519,
}

pub static ALL_KEY_TYPES: [KeyType; 3] = [KeyType::RSA, KeyType::ECDSA, KeyType::ED25519];

impl KeyType {
    fn bytes_for(&self, part: &str) -> &'static [u8] {
        match self {
            KeyType::RSA => bytes_for("rsa", part),
            KeyType::ECDSA => bytes_for("ecdsa", part),
            KeyType::ED25519 => bytes_for("eddsa", part),
        }
    }

    pub fn get_chain(&self) -> Vec<Certificate> {
        pemfile::certs(&mut io::BufReader::new(self.bytes_for("end.fullchain"))).unwrap()
    }

    pub fn get_key(&self) -> PrivateKey {
        pemfile::pkcs8_private_keys(&mut io::BufReader::new(self.bytes_for("end.key"))).unwrap()[0]
            .clone()
    }

    fn get_client_chain(&self) -> Vec<Certificate> {
        pemfile::certs(&mut io::BufReader::new(self.bytes_for("client.fullchain"))).unwrap()
    }

    fn get_client_key(&self) -> PrivateKey {
        pemfile::pkcs8_private_keys(&mut io::BufReader::new(self.bytes_for("client.key"))).unwrap()
            [0]
        .clone()
    }
}

pub fn make_server_config(kt: KeyType) -> ServerConfig {
    let mut cfg = ServerConfig::new(NoClientAuth::new());
    cfg.set_single_cert(kt.get_chain(), kt.get_key())
        .unwrap();

    cfg
}

pub fn get_client_root_store(kt: KeyType) -> RootCertStore {
    let roots = kt.get_chain();
    let mut client_auth_roots = RootCertStore::empty();
    for root in roots {
        client_auth_roots.add(&root).unwrap();
    }
    client_auth_roots
}

pub fn make_server_config_with_mandatory_client_auth(kt: KeyType) -> ServerConfig {
    let client_auth_roots = get_client_root_store(kt);

    let client_auth = AllowAnyAuthenticatedClient::new(client_auth_roots);
    let mut cfg = ServerConfig::new(NoClientAuth::new());
    cfg.set_client_certificate_verifier(client_auth);
    cfg.set_single_cert(kt.get_chain(), kt.get_key())
        .unwrap();

    cfg
}

pub fn make_client_config(kt: KeyType) -> ClientConfig {
    let mut cfg = ClientConfig::new();
    let mut rootbuf = io::BufReader::new(kt.bytes_for("ca.cert"));
    cfg.root_store
        .add_pem_file(&mut rootbuf)
        .unwrap();

    cfg
}

pub fn make_client_config_with_auth(kt: KeyType) -> ClientConfig {
    let mut cfg = make_client_config(kt);
    cfg.set_single_client_cert(kt.get_client_chain(), kt.get_client_key())
        .unwrap();
    cfg
}

pub fn make_pair(kt: KeyType) -> (ClientSession, ServerSession) {
    make_pair_for_configs(make_client_config(kt), make_server_config(kt))
}

pub fn make_pair_for_configs(
    client_config: ClientConfig,
    server_config: ServerConfig,
) -> (ClientSession, ServerSession) {
    make_pair_for_arc_configs(&Arc::new(client_config), &Arc::new(server_config))
}

pub fn make_pair_for_arc_configs(
    client_config: &Arc<ClientConfig>,
    server_config: &Arc<ServerConfig>,
) -> (ClientSession, ServerSession) {
    (
        ClientSession::new(client_config, dns_name("localhost")),
        ServerSession::new(server_config),
    )
}

pub fn do_handshake(client: &mut ClientSession, server: &mut ServerSession) -> (usize, usize) {
    let (mut to_client, mut to_server) = (0, 0);
    while server.is_handshaking() || client.is_handshaking() {
        to_server += transfer(client, server);
        server.process_new_packets().unwrap();
        to_client += transfer(server, client);
        client.process_new_packets().unwrap();
    }
    (to_server, to_client)
}

pub struct AllClientVersions {
    client_config: ClientConfig,
    index: usize,
}

impl AllClientVersions {
    pub fn new(client_config: ClientConfig) -> AllClientVersions {
        AllClientVersions {
            client_config,
            index: 0,
        }
    }
}

impl Iterator for AllClientVersions {
    type Item = ClientConfig;

    fn next(&mut self) -> Option<ClientConfig> {
        let mut config = self.client_config.clone();
        self.index += 1;

        match self.index {
            1 => {
                config.versions = vec![ProtocolVersion::TLSv1_2];
                Some(config)
            }
            2 => {
                config.versions = vec![ProtocolVersion::TLSv1_3];
                Some(config)
            }
            _ => None,
        }
    }
}

#[cfg(feature = "dangerous_configuration")]
pub struct MockClientVerifier {
    pub verified: fn() -> Result<ClientCertVerified, TLSError>,
    pub subjects: Option<DistinguishedNames>,
    pub mandatory: Option<bool>,
    pub offered_schemes: Option<Vec<SignatureScheme>>,
}

#[cfg(feature = "dangerous_configuration")]
impl ClientCertVerifier for MockClientVerifier {
    fn client_auth_mandatory(&self, sni: Option<&webpki::DNSName>) -> Option<bool> {
        // This is just an added 'test' to make sure we plumb through the SNI,
        // although its valid for it to be None, its just our tests should (as of now) always provide it
        assert!(sni.is_some());
        self.mandatory
    }

    fn client_auth_root_subjects(
        &self,
        sni: Option<&webpki::DNSName>,
    ) -> Option<DistinguishedNames> {
        assert!(sni.is_some());
        self.subjects.as_ref().cloned()
    }

    fn verify_client_cert(
        &self,
        _presented_certs: &[Certificate],
        sni: Option<&webpki::DNSName>,
    ) -> Result<ClientCertVerified, TLSError> {
        assert!(sni.is_some());
        (self.verified)()
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        if let Some(schemes) = &self.offered_schemes {
            schemes.clone()
        } else {
            WebPKIVerifier::verification_schemes()
        }
    }
}

#[derive(PartialEq, Debug)]
pub enum TLSErrorFromPeer {
    Client(TLSError),
    Server(TLSError),
}

pub fn do_handshake_until_error(
    client: &mut ClientSession,
    server: &mut ServerSession,
) -> Result<(), TLSErrorFromPeer> {
    while server.is_handshaking() || client.is_handshaking() {
        transfer(client, server);
        server
            .process_new_packets()
            .map_err(|err| TLSErrorFromPeer::Server(err))?;
        transfer(server, client);
        client
            .process_new_packets()
            .map_err(|err| TLSErrorFromPeer::Client(err))?;
    }

    Ok(())
}

pub fn do_handshake_until_both_error(
    client: &mut ClientSession,
    server: &mut ServerSession,
) -> Result<(), Vec<TLSErrorFromPeer>> {
    match do_handshake_until_error(client, server) {
        Err(server_err @ TLSErrorFromPeer::Server(_)) => {
            let mut errors = vec![server_err];
            transfer(server, client);
            let client_err = client
                .process_new_packets()
                .map_err(|err| TLSErrorFromPeer::Client(err))
                .expect_err("client didn't produce error after server error");
            errors.push(client_err);
            Err(errors)
        }

        Err(client_err @ TLSErrorFromPeer::Client(_)) => {
            let mut errors = vec![client_err];
            transfer(client, server);
            let server_err = server
                .process_new_packets()
                .map_err(|err| TLSErrorFromPeer::Server(err))
                .expect_err("server didn't produce error after client error");
            errors.push(server_err);
            Err(errors)
        }

        Ok(()) => Ok(()),
    }
}

pub fn dns_name(name: &'static str) -> webpki::DNSNameRef<'_> {
    webpki::DNSNameRef::try_from_ascii_str(name).unwrap()
}

pub struct FailsReads {
    errkind: io::ErrorKind,
}

impl FailsReads {
    pub fn new(errkind: io::ErrorKind) -> FailsReads {
        FailsReads { errkind }
    }
}

impl io::Read for FailsReads {
    fn read(&mut self, _b: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::from(self.errkind))
    }
}

use crate::hash_hs;
use crate::key;
use crate::msgs::handshake::{ServerExtension, SessionID};
use crate::session::SessionRandoms;
use crate::suites;

use std::mem;

pub struct HandshakeDetails {
    pub transcript: hash_hs::HandshakeHash,
    pub hash_at_server_fin: Vec<u8>,
    pub session_id: SessionID,
    pub randoms: SessionRandoms,
    pub using_ems: bool,
    pub extra_exts: Vec<ServerExtension>,
}

impl HandshakeDetails {
    pub fn new(extra_exts: Vec<ServerExtension>) -> HandshakeDetails {
        HandshakeDetails {
            transcript: hash_hs::HandshakeHash::new(),
            hash_at_server_fin: Vec::new(),
            session_id: SessionID::empty(),
            randoms: SessionRandoms::for_server(),
            using_ems: false,
            extra_exts,
        }
    }
}

pub struct ServerKXDetails {
    pub kx: Option<suites::KeyExchange>,
}

impl ServerKXDetails {
    pub fn new(kx: suites::KeyExchange) -> ServerKXDetails {
        ServerKXDetails { kx: Some(kx) }
    }

    pub fn take_kx(&mut self) -> suites::KeyExchange {
        self.kx.take().unwrap()
    }
}

pub struct ClientCertDetails {
    pub cert_chain: Vec<key::Certificate>,
}

impl ClientCertDetails {
    pub fn new(chain: Vec<key::Certificate>) -> ClientCertDetails {
        ClientCertDetails { cert_chain: chain }
    }

    pub fn take_chain(&mut self) -> Vec<key::Certificate> {
        mem::replace(&mut self.cert_chain, Vec::new())
    }
}

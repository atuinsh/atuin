use crate::msgs::enums::{AlertDescription, ContentType, HandshakeType};
use sct;
use std::error::Error;
use std::fmt;
use webpki;

/// rustls reports protocol errors using this type.
#[derive(Debug, PartialEq, Clone)]
pub enum TLSError {
    /// We received a TLS message that isn't valid right now.
    /// `expect_types` lists the message types we can expect right now.
    /// `got_type` is the type we found.  This error is typically
    /// caused by a buggy TLS stack (the peer or this one), a broken
    /// network, or an attack.
    InappropriateMessage {
        /// Which types we expected
        expect_types: Vec<ContentType>,
        /// What type we received
        got_type: ContentType,
    },

    /// We received a TLS handshake message that isn't valid right now.
    /// `expect_types` lists the handshake message types we can expect
    /// right now.  `got_type` is the type we found.
    InappropriateHandshakeMessage {
        /// Which handshake type we expected
        expect_types: Vec<HandshakeType>,
        /// What handshake type we received
        got_type: HandshakeType,
    },

    /// The peer sent us a syntactically incorrect TLS message.
    CorruptMessage,

    /// The peer sent us a TLS message with invalid contents.
    CorruptMessagePayload(ContentType),

    /// The peer didn't give us any certificates.
    NoCertificatesPresented,

    /// We couldn't decrypt a message.  This is invariably fatal.
    DecryptError,

    /// The peer doesn't support a protocol version/feature we require.
    /// The parameter gives a hint as to what version/feature it is.
    PeerIncompatibleError(String),

    /// The peer deviated from the standard TLS protocol.
    /// The parameter gives a hint where.
    PeerMisbehavedError(String),

    /// We received a fatal alert.  This means the peer is unhappy.
    AlertReceived(AlertDescription),

    /// The presented certificate chain is invalid.
    WebPKIError(webpki::Error),

    /// The presented SCT(s) were invalid.
    InvalidSCT(sct::Error),

    /// A catch-all error for unlikely errors.
    General(String),

    /// We failed to figure out what time it currently is.
    FailedToGetCurrentTime,

    /// This function doesn't work until the TLS handshake
    /// is complete.
    HandshakeNotComplete,

    /// The peer sent an oversized record/fragment.
    PeerSentOversizedRecord,

    /// An incoming connection did not support any known application protocol.
    NoApplicationProtocol,
}

fn join<T: fmt::Debug>(items: &[T]) -> String {
    items
        .iter()
        .map(|x| format!("{:?}", x))
        .collect::<Vec<String>>()
        .join(" or ")
}

impl fmt::Display for TLSError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            TLSError::InappropriateMessage {
                ref expect_types,
                ref got_type,
            } => write!(
                f,
                "received unexpected message: got {:?} when expecting {}",
                got_type,
                join::<ContentType>(expect_types)
            ),
            TLSError::InappropriateHandshakeMessage {
                ref expect_types,
                ref got_type,
            } => write!(
                f,
                "received unexpected handshake message: got {:?} when expecting {}",
                got_type,
                join::<HandshakeType>(expect_types)
            ),
            TLSError::CorruptMessagePayload(ref typ) => {
                write!(f, "received corrupt message of type {:?}", typ)
            }
            TLSError::PeerIncompatibleError(ref why) => write!(f, "peer is incompatible: {}", why),
            TLSError::PeerMisbehavedError(ref why) => write!(f, "peer misbehaved: {}", why),
            TLSError::AlertReceived(ref alert) => write!(f, "received fatal alert: {:?}", alert),
            TLSError::WebPKIError(ref err) => write!(f, "invalid certificate: {:?}", err),
            TLSError::CorruptMessage => write!(f, "received corrupt message"),
            TLSError::NoCertificatesPresented => write!(f, "peer sent no certificates"),
            TLSError::DecryptError => write!(f, "cannot decrypt peer's message"),
            TLSError::PeerSentOversizedRecord => write!(f, "peer sent excess record size"),
            TLSError::HandshakeNotComplete => write!(f, "handshake not complete"),
            TLSError::NoApplicationProtocol => write!(f, "peer doesn't support any known protocol"),
            TLSError::InvalidSCT(ref err) => write!(f, "invalid certificate timestamp: {:?}", err),
            TLSError::FailedToGetCurrentTime => write!(f, "failed to get current time"),
            TLSError::General(ref err) => write!(f, "unexpected error: {}", err), // (please file a bug)
        }
    }
}

impl Error for TLSError {}

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        use super::TLSError;
        use crate::msgs::enums::{AlertDescription, ContentType, HandshakeType};
        use sct;
        use webpki;

        let all = vec![
            TLSError::InappropriateMessage {
                expect_types: vec![ContentType::Alert],
                got_type: ContentType::Handshake,
            },
            TLSError::InappropriateHandshakeMessage {
                expect_types: vec![HandshakeType::ClientHello, HandshakeType::Finished],
                got_type: HandshakeType::ServerHello,
            },
            TLSError::CorruptMessage,
            TLSError::CorruptMessagePayload(ContentType::Alert),
            TLSError::NoCertificatesPresented,
            TLSError::DecryptError,
            TLSError::PeerIncompatibleError("no tls1.2".to_string()),
            TLSError::PeerMisbehavedError("inconsistent something".to_string()),
            TLSError::AlertReceived(AlertDescription::ExportRestriction),
            TLSError::WebPKIError(webpki::Error::ExtensionValueInvalid),
            TLSError::InvalidSCT(sct::Error::MalformedSCT),
            TLSError::General("undocumented error".to_string()),
            TLSError::FailedToGetCurrentTime,
            TLSError::HandshakeNotComplete,
            TLSError::PeerSentOversizedRecord,
            TLSError::NoApplicationProtocol,
        ];

        for err in all {
            println!("{:?}:", err);
            println!("  fmt '{}'", err);
        }
    }
}

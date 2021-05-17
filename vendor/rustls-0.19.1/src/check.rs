use crate::error::TLSError;
#[cfg(feature = "logging")]
use crate::log::warn;
use crate::msgs::enums::{ContentType, HandshakeType};
use crate::msgs::message::{Message, MessagePayload};

/// For a Message $m, and a HandshakePayload enum member $payload_type,
/// return Ok(payload) if $m is both a handshake message and one that
/// has the given $payload_type.  If not, return Err(TLSError) quoting
/// $handshake_type as the expected handshake type.
macro_rules! require_handshake_msg(
  ( $m:expr, $handshake_type:path, $payload_type:path ) => (
    match $m.payload {
        MessagePayload::Handshake(ref hsp) => match hsp.payload {
            $payload_type(ref hm) => Ok(hm),
            _ => Err(TLSError::InappropriateHandshakeMessage {
                     expect_types: vec![ $handshake_type ],
                     got_type: hsp.typ})
        }
        _ => Err(TLSError::InappropriateMessage {
                 expect_types: vec![ ContentType::Handshake ],
                 got_type: $m.typ})
    }
  )
);

/// Like require_handshake_msg, but moves the payload out of $m.
macro_rules! require_handshake_msg_mut(
  ( $m:expr, $handshake_type:path, $payload_type:path ) => (
    match $m.payload {
        MessagePayload::Handshake(hsp) => match hsp.payload {
            $payload_type(hm) => Ok(hm),
            _ => Err(TLSError::InappropriateHandshakeMessage {
                     expect_types: vec![ $handshake_type ],
                     got_type: hsp.typ})
        }
        _ => Err(TLSError::InappropriateMessage {
                 expect_types: vec![ ContentType::Handshake ],
                 got_type: $m.typ})
    }
  )
);

/// Validate the message `m`: return an error if:
///
/// - the type of m does not appear in `content_types`.
/// - if m is a handshake message, the handshake message type does
///   not appear in `handshake_types`.
pub fn check_message(
    m: &Message,
    content_types: &[ContentType],
    handshake_types: &[HandshakeType],
) -> Result<(), TLSError> {
    if !content_types.contains(&m.typ) {
        warn!(
            "Received a {:?} message while expecting {:?}",
            m.typ, content_types
        );
        return Err(TLSError::InappropriateMessage {
            expect_types: content_types.to_vec(),
            got_type: m.typ,
        });
    }

    if let MessagePayload::Handshake(ref hsp) = m.payload {
        if !handshake_types.is_empty() && !handshake_types.contains(&hsp.typ) {
            warn!(
                "Received a {:?} handshake message while expecting {:?}",
                hsp.typ, handshake_types
            );
            return Err(TLSError::InappropriateHandshakeMessage {
                expect_types: handshake_types.to_vec(),
                got_type: hsp.typ,
            });
        }
    }

    Ok(())
}

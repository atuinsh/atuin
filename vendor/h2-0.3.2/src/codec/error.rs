use crate::frame::{Reason, StreamId};

use std::{error, fmt, io};

/// Errors that are received
#[derive(Debug)]
pub enum RecvError {
    Connection(Reason),
    Stream { id: StreamId, reason: Reason },
    Io(io::Error),
}

/// Errors caused by sending a message
#[derive(Debug)]
pub enum SendError {
    /// User error
    User(UserError),

    /// Connection error prevents sending.
    Connection(Reason),

    /// I/O error
    Io(io::Error),
}

/// Errors caused by users of the library
#[derive(Debug)]
pub enum UserError {
    /// The stream ID is no longer accepting frames.
    InactiveStreamId,

    /// The stream is not currently expecting a frame of this type.
    UnexpectedFrameType,

    /// The payload size is too big
    PayloadTooBig,

    /// A header size is too big
    HeaderTooBig,

    /// The application attempted to initiate too many streams to remote.
    Rejected,

    /// The released capacity is larger than claimed capacity.
    ReleaseCapacityTooBig,

    /// The stream ID space is overflowed.
    ///
    /// A new connection is needed.
    OverflowedStreamId,

    /// Illegal headers, such as connection-specific headers.
    MalformedHeaders,

    /// Request submitted with relative URI.
    MissingUriSchemeAndAuthority,

    /// Calls `SendResponse::poll_reset` after having called `send_response`.
    PollResetAfterSendResponse,

    /// Calls `PingPong::send_ping` before receiving a pong.
    SendPingWhilePending,

    /// Tries to update local SETTINGS while ACK has not been received.
    SendSettingsWhilePending,

    /// Tries to send push promise to peer who has disabled server push
    PeerDisabledServerPush,
}

// ===== impl RecvError =====

impl From<io::Error> for RecvError {
    fn from(src: io::Error) -> Self {
        RecvError::Io(src)
    }
}

impl error::Error for RecvError {}

impl fmt::Display for RecvError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        use self::RecvError::*;

        match *self {
            Connection(ref reason) => reason.fmt(fmt),
            Stream { ref reason, .. } => reason.fmt(fmt),
            Io(ref e) => e.fmt(fmt),
        }
    }
}

// ===== impl SendError =====

impl error::Error for SendError {}

impl fmt::Display for SendError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        use self::SendError::*;

        match *self {
            User(ref e) => e.fmt(fmt),
            Connection(ref reason) => reason.fmt(fmt),
            Io(ref e) => e.fmt(fmt),
        }
    }
}

impl From<io::Error> for SendError {
    fn from(src: io::Error) -> Self {
        SendError::Io(src)
    }
}

impl From<UserError> for SendError {
    fn from(src: UserError) -> Self {
        SendError::User(src)
    }
}

// ===== impl UserError =====

impl error::Error for UserError {}

impl fmt::Display for UserError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        use self::UserError::*;

        fmt.write_str(match *self {
            InactiveStreamId => "inactive stream",
            UnexpectedFrameType => "unexpected frame type",
            PayloadTooBig => "payload too big",
            HeaderTooBig => "header too big",
            Rejected => "rejected",
            ReleaseCapacityTooBig => "release capacity too big",
            OverflowedStreamId => "stream ID overflowed",
            MalformedHeaders => "malformed headers",
            MissingUriSchemeAndAuthority => "request URI missing scheme and authority",
            PollResetAfterSendResponse => "poll_reset after send_response is illegal",
            SendPingWhilePending => "send_ping before received previous pong",
            SendSettingsWhilePending => "sending SETTINGS before received previous ACK",
            PeerDisabledServerPush => "sending PUSH_PROMISE to peer who disabled server push",
        })
    }
}

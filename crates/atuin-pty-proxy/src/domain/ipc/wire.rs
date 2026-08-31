//! Wire-protocol for the pty-proxy IPC.
//!
//! Each frame is encoded as a sequence of bytes, split between a "header" and a "body". The header
//! is designed to be stable, simple and mostly unchanging. Currently the protocol is:
//!
//! ```text
//! ||<--             header                 --> || <--    body   --> ||
//! ||---------------|----------------|----------||-------------------||
//! || message width | header version | reserved || arbitrary body... ||
//! || 4 bytes       | 1 byte         | 27 bytes || arbitrary width   ||
//! ```
//!
//! The header is designed to fit 32 bytes exactly. See the [`Header`] structure for more
//! information. This invariant should always be upheld.
//!
//! The body is always encoded via postcard. If you want to add more data, you should probably look
//! at versioning the body rather than the header.
//!
//! The header fields are all big-endian encoded.
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Copy, Error)]
pub enum HeaderParseError {
    #[error("message is too long: {0} > {1}")]
    MessageTooLong(u32, u32),
    #[error("invalid version: {0}")]
    BadVersion(u8),
}

/// The absolute maximum number of bytes a message can contain.
pub const MAX_MSG_LEN: u32 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
struct V1Payload;

impl V1Payload {
    const SERIALIZED_LEN: usize = 27;

    fn to_slice(slice: &mut [u8]) {
        debug_assert_eq!(slice.len(), Self::SERIALIZED_LEN);
        slice.fill(0);
    }

    #[allow(clippy::unnecessary_wraps)]
    fn parse(_bytes: [u8; Self::SERIALIZED_LEN]) -> Result<Self, HeaderParseError> {
        Ok(Self {})
    }
}

/// The versioned payload carried inside the header.
#[derive(Debug, Clone, Copy)]
enum HeaderPayload {
    /// Version one of the header payload carries no semantics.
    V1(V1Payload),
}

impl HeaderPayload {
    const SERIALIZED_LEN: usize = 28;
    const VERSION_LEN: usize = 1;

    fn version(self) -> u8 {
        match self {
            Self::V1(_) => 1,
        }
    }

    fn to_slice(self, slice: &mut [u8]) {
        debug_assert_eq!(slice.len(), Self::SERIALIZED_LEN);
        slice[0] = self.version();
        match self {
            Self::V1(_) => V1Payload::to_slice(&mut slice[Self::VERSION_LEN..]),
        }
    }

    /// Parse the payload from the given byte array.
    fn parse(bytes: [u8; Self::SERIALIZED_LEN]) -> Result<Self, HeaderParseError> {
        // The first byte is always the version, as per the docs.
        let (version_b, payload_b) = bytes.split_at(1);

        let version = u8::from_be_bytes(version_b.try_into().unwrap());

        match version {
            1 => Ok(Self::V1(V1Payload::parse(payload_b.try_into().unwrap())?)),
            _ => Err(HeaderParseError::BadVersion(version)),
        }
    }
}

pub struct Header {
    /// The total width of the message, including the header.
    pub message_width: u32,
    /// Additional data, versioned by some version.
    payload: HeaderPayload,
}

impl Header {
    /// Be very careful changing this -- bad things could happen. I haven't thought about them all.
    /// You probably don't want to change this.
    pub const SERIALIZED_LEN: usize = 32;

    /// Encode the header into the given slice.
    fn to_slice(&self, slice: &mut [u8]) {
        debug_assert_eq!(slice.len(), Self::SERIALIZED_LEN);
        slice[..4].copy_from_slice(&self.message_width.to_be_bytes());
        self.payload.to_slice(&mut slice[4..]);
    }

    /// Parse the header from the given header bytes.
    pub fn parse(header: [u8; Self::SERIALIZED_LEN]) -> Result<Self, HeaderParseError> {
        let (width_bytes, payload_bytes) = header.split_at(4);

        let message_width = u32::from_be_bytes(width_bytes.try_into().unwrap());
        if message_width > MAX_MSG_LEN {
            return Err(HeaderParseError::MessageTooLong(message_width, MAX_MSG_LEN));
        }

        // Awesome, let's parse the version
        let payload = HeaderPayload::parse(payload_bytes.try_into().unwrap())?;

        Ok(Self {
            message_width,
            payload,
        })
    }
}

#[derive(Debug, Clone, Error)]
pub enum EncodeError {
    #[error("failed to encode the value: {0}")]
    DataEncodingErr(#[from] postcard::Error),

    #[error("message too long: {0} > {1}")]
    TooLong(usize, u32),
}

/// Attempt to encode the given payload into a header+payload byte array.
pub fn try_encode<T: Serialize>(data: &T) -> Result<Vec<u8>, EncodeError> {
    let mut buf = postcard::to_extend(data, vec![0u8; Header::SERIALIZED_LEN])?;

    let message_width =
        u32::try_from(buf.len()).map_err(|_| EncodeError::TooLong(buf.len(), MAX_MSG_LEN))?;
    if message_width > MAX_MSG_LEN {
        return Err(EncodeError::TooLong(buf.len(), MAX_MSG_LEN));
    }

    let header = Header {
        message_width,
        payload: HeaderPayload::V1(V1Payload),
    };
    header.to_slice(&mut buf[..Header::SERIALIZED_LEN]);

    Ok(buf)
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("io error while decoding frame: {0}")]
    Io(std::io::Error),

    #[error("failed to parse header: {0}")]
    Header(HeaderParseError),

    #[error("failed to decode body: {0}")]
    Decode(postcard::Error),
}

pub fn try_decode<R: std::io::Read, T: serde::de::DeserializeOwned>(
    reader: &mut R,
) -> Result<Option<T>, DecodeError> {
    let mut header_bytes = [0u8; Header::SERIALIZED_LEN];
    if let Err(err) = reader.read_exact(&mut header_bytes) {
        return match err.kind() {
            std::io::ErrorKind::UnexpectedEof => Ok(None),
            _ => Err(DecodeError::Io(err)),
        };
    }

    let header = Header::parse(header_bytes).map_err(DecodeError::Header)?;
    let body_len = (header.message_width as usize).saturating_sub(Header::SERIALIZED_LEN);
    let mut buf = vec![0u8; body_len];
    reader.read_exact(&mut buf).map_err(DecodeError::Io)?;

    postcard::from_bytes(&buf).map(Some).map_err(DecodeError::Decode)
}

pub async fn try_decode_async<R, T>(reader: &mut R) -> Result<Option<T>, DecodeError>
where
    R: tokio::io::AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    use tokio::io::AsyncReadExt as _;

    let mut header_bytes = [0u8; Header::SERIALIZED_LEN];
    if let Err(err) = reader.read_exact(&mut header_bytes).await {
        return match err.kind() {
            std::io::ErrorKind::UnexpectedEof => Ok(None),
            _ => Err(DecodeError::Io(err)),
        };
    }

    let header = Header::parse(header_bytes).map_err(DecodeError::Header)?;
    let body_len = (header.message_width as usize).saturating_sub(Header::SERIALIZED_LEN);
    let mut buf = vec![0u8; body_len];
    reader.read_exact(&mut buf).await.map_err(DecodeError::Io)?;

    postcard::from_bytes(&buf).map(Some).map_err(DecodeError::Decode)
}

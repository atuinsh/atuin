use super::KeyId;
use super::key_id::KeyIdParseError;
use super::legacy::PasetoError;

/// The part of an envelope an error is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum Layer {
    #[display("payload")]
    Payload,
    #[display("content key wrap")]
    Wrap,
}

/// Why a value did not open.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// The content key is wrapped under another key; checked before any cryptography.
    #[error(transparent)]
    WrongKey(#[from] WrongKey),
    /// The kid matched but a check failed: the value was altered or is corrupt, never a wrong key.
    #[error(transparent)]
    Authentication(#[from] AuthenticationError),
    /// Not a well-formed value of the format it claims, or of any format.
    #[error(transparent)]
    Malformed(#[from] MalformedError),
    /// A format this build does not know; a newer writer sealed it.
    #[error(transparent)]
    UnsupportedFormat(#[from] UnsupportedFormat),
}

/// A content key wrapped under another key than the one given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "unexpected error decrypting CEK: bad key. encrypted key id: {found}, given decryption key: \
     {given}"
)]
pub struct WrongKey {
    pub found: KeyId,
    pub given: KeyId,
}

/// Which check rejected a value whose kid matched.
#[derive(Debug, thiserror::Error)]
pub enum AuthenticationError {
    #[error("the {0} does not match its key commitment: it was altered or sealed for other data")]
    Commitment(Layer),
    #[error("the {0} failed authentication: it was altered or is corrupt")]
    Tag(Layer),
    #[error("the legacy {layer} failed authentication: {cause}")]
    Legacy {
        layer: Layer,
        #[source]
        cause: LegacyCause,
    },
}

/// How a value is not well-formed.
#[derive(Debug, thiserror::Error)]
pub enum MalformedError {
    /// Found by parsing, before any key is involved.
    #[error(transparent)]
    Shape(#[from] ShapeError),
    #[error("the legacy token body is {chars} base64 characters, shorter than any valid token")]
    LegacyShortBody {
        chars: usize,
    },
    #[error("the legacy {layer} is malformed: {cause}")]
    Legacy {
        layer: Layer,
        #[source]
        cause: LegacyCause,
    },
    #[error("failed to deserialize decrypted payload into json: {0}")]
    LegacyPayloadJson(#[source] serde_json::Error),
    #[error("failed to base64-decode the deserialized payload: {0}")]
    LegacyPayloadBase64(#[source] base64::DecodeError),
}

/// How a value's shape is wrong, found before any key is involved.
#[derive(Debug, thiserror::Error)]
pub enum ShapeError {
    #[error("the {0} is in no known envelope format")]
    UnknownShape(Layer),
    #[error("the {layer} is not canonical unpadded base64url: {source}")]
    Base64 {
        layer: Layer,
        #[source]
        source: base64::DecodeError,
    },
    #[error("the avelope {layer} is {len} bytes, which its format does not allow")]
    Length {
        layer: Layer,
        len: usize,
    },
    // `atuin-domain`'s record tests pin this text.
    #[error("failed to deserialize the given key: {0}")]
    LegacyCekJson(#[source] serde_json::Error),
    #[error("the legacy content key names no valid key id: {0}")]
    LegacyKeyId(#[source] KeyIdParseError),
}

/// An envelope format this build does not know; a newer writer sealed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UnsupportedFormat {
    #[error("the {layer} is avelope version {version:#04x}, which this build cannot read")]
    Version {
        layer: Layer,
        version: u8,
    },
    #[error("the content key uses avelope wrap type {0:#04x}, which this build cannot read")]
    WrapType(u8),
}

/// Why a value could not be classified, before any key is involved.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error(transparent)]
    Malformed(#[from] ShapeError),
    #[error(transparent)]
    UnsupportedFormat(#[from] UnsupportedFormat),
}

impl From<ParseError> for OpenError {
    fn from(err: ParseError) -> Self {
        match err {
            ParseError::Malformed(shape) => Self::Malformed(shape.into()),
            ParseError::UnsupportedFormat(format) => Self::UnsupportedFormat(format),
        }
    }
}

/// Why sealing failed.
#[derive(Debug, thiserror::Error)]
pub enum SealError {
    #[error("the OS random source failed: {0}")]
    Random(#[from] getrandom::Error),
    #[error("{len} bytes is more than one envelope can seal")]
    TooLong {
        len: usize,
    },
    #[error("legacy encryption failed: {0}")]
    Legacy(#[source] LegacyCause),
    #[error("failed to serialize the legacy payload: {0}")]
    LegacyJson(#[source] serde_json::Error),
}

/// Why rewrapping a content key failed.
#[derive(Debug, thiserror::Error)]
pub enum RewrapError {
    #[error("failed to unwrap the content key: {0}")]
    Open(#[from] OpenError),
    #[error("failed to wrap the content key under the new key: {0}")]
    Seal(#[from] SealError),
}

/// A `rusty_paseto` / `rusty_paserk` error, opaque so neither crate is part of this API.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct LegacyCause(pub(super) PasetoError);

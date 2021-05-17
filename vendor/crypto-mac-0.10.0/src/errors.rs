use core::fmt;

/// Error type for signaling failed MAC verification
#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
pub struct MacError;

/// Error type for signaling invalid key length for MAC initialization
#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
pub struct InvalidKeyLength;

impl fmt::Display for MacError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("failed MAC verification")
    }
}

impl fmt::Display for InvalidKeyLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid key length")
    }
}

#[cfg(feature = "std")]
impl std::error::Error for MacError {}

#[cfg(feature = "std")]
impl std::error::Error for InvalidKeyLength {}

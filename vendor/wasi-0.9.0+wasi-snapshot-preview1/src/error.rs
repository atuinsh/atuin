use super::Errno;
use core::fmt;
use core::num::NonZeroU16;

/// A raw error returned by wasi APIs, internally containing a 16-bit error
/// code.
#[derive(Copy, Clone, PartialEq, Eq, Ord, PartialOrd)]
pub struct Error {
    code: NonZeroU16,
}

impl Error {
    /// Constructs a new error from a raw error code, returning `None` if the
    /// error code is zero (which means success).
    pub fn from_raw_error(error: Errno) -> Option<Error> {
        Some(Error {
            code: NonZeroU16::new(error)?,
        })
    }

    /// Returns the raw error code that this error represents.
    pub fn raw_error(&self) -> u16 {
        self.code.get()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (error {})",
            super::strerror(self.code.get()),
            self.code
        )?;
        Ok(())
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("code", &self.code)
            .field("message", &super::strerror(self.code.get()))
            .finish()
    }
}

#[cfg(feature = "std")]
extern crate std;
#[cfg(feature = "std")]
impl std::error::Error for Error {}

use crate::string::NonNulStr;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
pub struct WindowsHostname(String);

impl WindowsHostname {
    const MAX_HOSTNAME_LENGTH: usize = 63;

    /// Equivalent to [`whoami::hostname`].
    pub fn get() -> Result<Self, super::HostnameGetError> {
        whoami::hostname().map(Self)
    }
}

impl<S: AsRef<str>> TryFrom<S> for WindowsHostname {
    type Error = super::HostnameStringConversionError;

    fn try_from(value: S) -> Result<Self, Self::Error> {
        // The constraints here reference <https://docs.rs/whoami/latest/whoami/fn.hostname.html>
        let value = value.as_ref();

        if value.len() > Self::MAX_HOSTNAME_LENGTH {
            return Err(Self::Error::TooLong(Self::MAX_HOSTNAME_LENGTH));
        }

        Ok(WindowsHostname(value.into()))
    }
}

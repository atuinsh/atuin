use super::DnsHostname;
use crate::string::NonNulStr;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
pub struct WindowsHostname(DnsHostname);

impl WindowsHostname {
    const MAX_HOSTNAME_LENGTH: usize = 63;

    /// Equivalent to [`whoami::hostname`].
    pub fn get() -> Result<Self, super::HostnameGetError> {
        whoami::hostname().map(Self)
    }
}

impl From<DnsHostname> for WindowsHostname {
    type Error = super::HostnameStringConversionError;

    fn from(value: DnsHostname) -> Result<Self, Self::Error> {
        Ok(WindowsHostname(value))
    }
}

impl<S: AsRef<str>> TryFrom<S> for WindowsHostname {
    type Error = super::HostnameStringConversionError;

    fn try_from(value: S) -> Result<Self, Self::Error> {
        // <https://docs.rs/whoami/latest/whoami/fn.hostname.html> indicates that windows hostnames
        // are just [`super::DnsHostname`]s.
        let as_dns = DnsHostname::try_from(value.as_ref());
        Ok(as_dns.into())
    }
}

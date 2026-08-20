/// The placeholder username [`CmdOrigin::parse_lenient`] assigns to a legacy value with no `:`.
pub const UNKNOWN_USER: &str = "unknown-user";

/// A hostname, generic over its backing storage so it can be an owned
/// `CmdHost<String>` or a zero-copy `CmdHost<&str>` view into a [`CmdOrigin`].
#[derive(
    Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, derive_more::Display, derive_more::From,
)]
pub struct CmdHost<T = String>(T);

impl<T: AsRef<str>> AsRef<str> for CmdHost<T> {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl<T: Into<String>> CmdHost<T> {
    pub fn into_owned(self) -> CmdHost<String> {
        CmdHost(self.0.into())
    }
}

impl CmdHost<String> {
    pub fn probe_current() -> Self {
        std::env::var("ATUIN_HOST_NAME")
            .ok()
            .or_else(|| whoami::hostname().ok())
            .map(Self)
            .unwrap_or_default()
    }
}

impl<T> CmdHost<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl Default for CmdHost<String> {
    fn default() -> Self {
        Self(String::from("unknown-host"))
    }
}

/// A username, generic over its backing storage (owned `String` or `&str` view).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, derive_more::Display, derive_more::From)]
pub struct CmdUser<T = String>(T);

impl<T: AsRef<str>> AsRef<str> for CmdUser<T> {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl<T: Into<String>> CmdUser<T> {
    pub fn into_owned(self) -> CmdUser<String> {
        CmdUser(self.0.into())
    }
}

impl CmdUser<String> {
    pub fn probe_current() -> Self {
        std::env::var("ATUIN_HOST_USER")
            .ok()
            .or_else(|| whoami::username().ok())
            .map(Self)
            .unwrap_or_default()
    }
}

impl<T> CmdUser<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl Default for CmdUser<String> {
    fn default() -> Self {
        Self(String::from(UNKNOWN_USER))
    }
}

/// The origin of a command: the `host:user` pair it was run under.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
#[display("{raw}")]
pub struct CmdOrigin {
    raw: String,
    sep: usize,
}

impl CmdOrigin {
    pub fn new<H: AsRef<str>, U: AsRef<str>>(host: &CmdHost<H>, user: &CmdUser<U>) -> Self {
        let host = host.as_ref();
        let sep = host.len();
        Self {
            raw: format!("{host}:{}", user.as_ref()),
            sep,
        }
    }

    pub fn probe_current() -> Self {
        Self::new(&CmdHost::probe_current(), &CmdUser::probe_current())
    }

    /// The host portion, as a zero-copy view.
    pub fn host(&self) -> CmdHost<&str> {
        CmdHost(&self.raw[..self.sep])
    }

    /// The user portion.
    ///
    /// May be `""` if [`CmdOrigin`] was created through [`CmdOrigin::parse_lenient`] and no user was
    /// found in the string.
    pub fn user(&self) -> CmdUser<&str> {
        CmdUser(self.raw.get(self.sep + 1..).unwrap_or(""))
    }

    /// The whole `host:user` string, borrowed.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Consume into the owned `host:user` string, without copying.
    pub fn into_string(self) -> String {
        self.raw
    }

    /// Leniently parse a string.
    ///
    /// If a `:` exists, the string is parsed as `<host>:<user>`.
    /// If no `:` exists, the string is parsed as `host`, and [`CmdUser::default`] is used.
    #[deprecated(note = "this function is considered an anti-pattern and should not be used \
                         moving forwards. it is mostly used to interface with legacy code and \
                         need to deserialize potentially malformed data.")]
    pub fn parse_lenient<T: Into<String> + AsRef<str>>(value: T) -> Self {
        match value.as_ref().find(':') {
            Some(sep) => Self {
                raw: value.into(),
                sep,
            },
            None => Self::new(&CmdHost::from(value.into()), &CmdUser::default()),
        }
    }
}

impl Default for CmdOrigin {
    fn default() -> Self {
        let def_str = String::from("unknown-host:unknown-user");
        Self {
            sep: def_str.find(':').expect("literal missing :"),
            raw: def_str,
        }
    }
}

/// A [`CmdOrigin`] string was missing the `:` host/user separator.
#[derive(Debug, thiserror::Error)]
#[error("`{0}` is not a valid host:user command origin (missing `:`)")]
pub struct CmdOriginParseError(pub String);

impl TryFrom<String> for CmdOrigin {
    type Error = CmdOriginParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.find(':') {
            Some(sep) => Ok(Self { raw: value, sep }),
            None => Err(CmdOriginParseError(value)),
        }
    }
}

impl TryFrom<&str> for CmdOrigin {
    type Error = CmdOriginParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_string())
    }
}

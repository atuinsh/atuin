#![deny(unsafe_code)]

//! Atuin's core domain model.
//!
//! Where `atuin-common` is a grab-bag of utility helpers, this crate holds the
//! types that make up Atuin's domain: the sync [`record`] types, the HTTP
//! [`api`] request/response types, and the [`caps`] capability types. These are
//! shared across the client, the daemon, and the server.

/// Defines a new UUID type wrapper
macro_rules! new_uuid {
    ($name:ident) => {
        #[derive(
            Debug,
            Copy,
            Clone,
            PartialEq,
            Eq,
            Hash,
            PartialOrd,
            Ord,
            serde::Serialize,
            serde::Deserialize,
            derive_more::Display,
            derive_more::From,
            derive_more::Deref,
        )]
        #[serde(transparent)]
        #[display("{_0}")]
        pub struct $name(pub Uuid);

        impl<DB: sqlx::Database> sqlx::Type<DB> for $name
        where
            Uuid: sqlx::Type<DB>,
        {
            fn type_info() -> <DB as sqlx::Database>::TypeInfo {
                Uuid::type_info()
            }
        }

        impl<'r, DB: sqlx::Database> sqlx::Decode<'r, DB> for $name
        where
            Uuid: sqlx::Decode<'r, DB>,
        {
            fn decode(
                value: DB::ValueRef<'r>,
            ) -> std::result::Result<Self, sqlx::error::BoxDynError> {
                Uuid::decode(value).map(Self)
            }
        }

        impl<'q, DB: sqlx::Database> sqlx::Encode<'q, DB> for $name
        where
            Uuid: sqlx::Encode<'q, DB>,
        {
            fn encode_by_ref(
                &self,
                buf: &mut DB::ArgumentBuffer,
            ) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync + 'static>>
            {
                self.0.encode_by_ref(buf)
            }
        }
    };
}

pub mod api;
pub mod caps;
pub mod record;

/// A hostname, generic over its backing storage so it can be an owned
/// `CmdHost<String>` or a zero-copy `CmdHost<&str>` view into a [`CmdOrigin`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, derive_more::Display, derive_more::From)]
pub struct CmdHost<T: AsRef<str> = String>(T);

impl<T: AsRef<str>> AsRef<str> for CmdHost<T> {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl<T: AsRef<str>> CmdHost<T> {
    pub fn owned(&self) -> CmdHost<String> {
        CmdHost(self.0.as_ref().to_string())
    }
}

impl CmdHost<String> {
    pub fn probe() -> Self {
        std::env::var("ATUIN_HOST_NAME")
            .ok()
            .or_else(|| whoami::hostname().ok())
            .map(Self)
            .unwrap_or_default()
    }
}

impl<'a> CmdHost<&'a str> {
    pub fn as_str(&self) -> &'a str {
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
pub struct CmdUser<T: AsRef<str> = String>(T);

impl<T: AsRef<str>> AsRef<str> for CmdUser<T> {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl<T: AsRef<str>> CmdUser<T> {
    pub fn owned(&self) -> CmdUser<String> {
        CmdUser(self.0.as_ref().to_string())
    }
}

impl CmdUser<String> {
    pub fn probe() -> Self {
        std::env::var("ATUIN_HOST_USER")
            .ok()
            .or_else(|| whoami::username().ok())
            .map(Self)
            .unwrap_or_default()
    }
}

impl<'a> CmdUser<&'a str> {
    pub fn as_str(&self) -> &'a str {
        self.0
    }
}

impl Default for CmdUser<String> {
    fn default() -> Self {
        Self(String::from("unknown-user"))
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
    pub fn new<H: AsRef<str>, U: AsRef<str>>(host: CmdHost<H>, user: CmdUser<U>) -> Self {
        let host = host.as_ref();
        let sep = host.len();
        Self {
            raw: format!("{host}:{}", user.as_ref()),
            sep,
        }
    }

    pub fn probe() -> Self {
        Self::new(CmdHost::probe(), CmdUser::probe())
    }

    /// The host portion, as a zero-copy view.
    pub fn host(&self) -> CmdHost<&str> {
        CmdHost(&self.raw[..self.sep])
    }

    /// The user portion, as a zero-copy view (empty when there is no `:`).
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
    pub fn parse_fuzzy<T: Into<String> + AsRef<str>>(value: T) -> Self {
        match value.as_ref().find(':') {
            Some(sep) => Self {
                raw: value.into(),
                sep,
            },
            None => Self::new(CmdHost::from(value.into()), CmdUser::default()),
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

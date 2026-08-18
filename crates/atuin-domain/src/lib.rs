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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
pub struct CmdHost(String);

impl CmdHost {
    pub fn probe() -> Self {
        std::env::var("ATUIN_HOST_NAME")
            .ok()
            .or_else(|| atuin_common::os::hostname().ok())
            .map(Self)
            .unwrap_or_default()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for CmdHost {
    fn default() -> Self {
        Self(String::from("unknown-host"))
    }
}

impl From<String> for CmdHost {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for CmdHost {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
pub struct CmdUser(String);

impl CmdUser {
    pub fn probe() -> Self {
        std::env::var("ATUIN_HOST_USER")
            .ok()
            .or_else(|| atuin_common::os::username().ok())
            .map(Self)
            .unwrap_or_default()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for CmdUser {
    fn default() -> Self {
        Self(String::from("unknown-user"))
    }
}

impl From<String> for CmdUser {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for CmdUser {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
#[display("{hostname}:{username}")]
pub struct CmdOrigin {
    pub hostname: CmdHost,
    pub username: CmdUser,
}

impl CmdOrigin {
    pub fn new(hostname: CmdHost, username: CmdUser) -> Self {
        Self { hostname, username }
    }

    pub fn probe() -> Self {
        Self {
            hostname: CmdHost::probe(),
            username: CmdUser::probe(),
        }
    }
}

impl From<&str> for CmdOrigin {
    fn from(value: &str) -> Self {
        let (hostname, username) = value.split_once(':').unwrap_or((value, ""));
        Self {
            hostname: CmdHost::from(hostname),
            username: CmdUser::from(username),
        }
    }
}

impl From<String> for CmdOrigin {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

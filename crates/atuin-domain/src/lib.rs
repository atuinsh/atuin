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

//! This module exposes fjall keyspaces to the user.
//!
//! The way you're intended to use this is through the [`Keyspace`] trait. This trait explicitly
//! defines the types that are used to store data into command and is responsible for:
//!
//!  - Versioning the [`fjall::Keyspace`], which includes the keyspace creation options, which
//!    **must** remain stable for the lifetime of the keyspace.
//!  - Versioning the [`Keyspace::Key`] and [`Keyspace::Value`] types, which are the types that are
//!    directly inserted into [`fjall`].
//!
//! There is currently only one implementation of this keyspace -- [`KeyspaceV1`]. This trait is,
//! however, quite useful as it enables us to upgrade between keyspaces. Please be very careful
//! deleting it, even though there is only one implementation.

mod v1;
use fjall::{UserKey, UserValue};
pub use v1::Keyspace as KeyspaceV1;

/// See the moduledoc.
pub trait Keyspace {
    /// The serialized representation of the key.
    type KeySerialized: Into<UserKey>;

    /// The serialized representation of the value.
    type ValueSerialized: Into<UserValue>;

    /// The name of the keyspace. Ensure no other keyspaces share this name as bad things can
    /// happen.
    const NAME: &'static str;

    type Key;
    type Value;

    type KeySerializationError;

    /// Try to serialize the key into a serialized form.
    fn serialize_key(key: Self::Key) -> Result<Self::KeySerialized, Self::KeySerializationError>;

    type ValueSerializationError;
    fn serialize_value(
        value: Self::Value,
    ) -> Result<Self::ValueSerialized, Self::ValueSerializationError>;

    type ValueDeserializationError;
    fn deserialize_value(
        serialized: Self::ValueSerialized,
    ) -> Result<Self::Value, Self::ValueDeserializationError>;
}

//! This module exposes fjall keyspaces to the user.
//!
//! The way you're intended to use this is through the [`Schema`] trait. This trait explicitly
//! defines the types that are used to store data into command and is responsible for:
//!
//!  - Versioning the [`fjall::Keyspace`], which includes the keyspace creation options, which
//!    **must** remain stable for the lifetime of the keyspace.
//!  - Versioning the [`Schema::Key`] and [`Schema::Value`] types, which are the types that are
//!    directly inserted into [`fjall`].
//!
//! There is currently only one implementation of this schema -- [`SchemaV1`]. This trait is,
//! however, quite useful as it enables us to upgrade between schemas. Please be very careful
//! deleting it, even though there is only one implementation.

mod v1;
use fjall::{UserKey, UserValue};
pub use v1::Schema as SchemaV1;

/// See the moduledoc.
pub trait Schema {
    /// The type of the key in the output capture.
    type Key;

    /// The serialized representation of the key.
    type KeySerialized: Into<UserKey>;

    /// Error thrown when trying to serialize keys.
    type KeySerializationError;

    /// The type of the value in the output capture.
    type Value;

    /// The serialized representation of the value.
    type ValueSerialized: Into<UserValue>;

    /// Errors thrown when trying to serialize values.
    type ValueSerializationError;

    /// Errors thrown when trying to deserialize values.
    type ValueDeserializationError;

    /// The name of the keyspace. Ensure no other keyspaces share this name as bad things can
    /// happen.
    const NAME: &'static str;

    /// Try to serialize the key into a serialized form.
    fn serialize_key(key: Self::Key) -> Result<Self::KeySerialized, Self::KeySerializationError>;

    /// Try to serialize the value.
    fn serialize_value(
        value: Self::Value,
    ) -> Result<Self::ValueSerialized, Self::ValueSerializationError>;

    /// Try to deserialize the value.
    fn deserialize_value(
        serialized: Self::ValueSerialized,
    ) -> Result<Self::Value, Self::ValueDeserializationError>;

    /// Create a [`fjall::KeyspaceCreateOptions`] which defines the compaction/compression/etc fjall
    /// options.
    fn create_options() -> fjall::KeyspaceCreateOptions;
}

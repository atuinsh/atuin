use std::convert::Infallible;

use atuin_client::history::HistoryId;
use prost::Message;

use super::Keyspace as KeyspaceTrait;
use crate::grpc::history::pb::CommandCapture;

pub struct Keyspace;

impl KeyspaceTrait for Keyspace {
    const NAME: &'static str = "output_capture_v1";

    type Key = HistoryId;
    type Value = CommandCapture;

    // Keys are the 16 raw UUID bytes; values are prost-encoded.
    type KeySerialized = [u8; 16];
    type ValueSerialized = Vec<u8>;

    type KeySerializationError = Infallible;
    fn serialize_key(key: Self::Key) -> Result<Self::KeySerialized, Self::KeySerializationError> {
        Ok(key.into_bytes())
    }

    type KeyDeserializationError = Infallible;
    fn deserialize_key(
        serialized: Self::KeySerialized,
    ) -> Result<Self::Key, Self::KeyDeserializationError> {
        // The `[u8; 16]` type carries the length invariant, so this cannot fail.
        Ok(HistoryId::from_bytes(serialized))
    }

    type ValueSerializationError = Infallible;
    fn serialize_value(
        value: Self::Value,
    ) -> Result<Self::ValueSerialized, Self::ValueSerializationError> {
        Ok(value.encode_to_vec())
    }

    type ValueDeserializationError = prost::DecodeError;
    fn deserialize_value(
        serialized: Self::ValueSerialized,
    ) -> Result<Self::Value, Self::ValueDeserializationError> {
        CommandCapture::decode(&serialized[..])
    }
}

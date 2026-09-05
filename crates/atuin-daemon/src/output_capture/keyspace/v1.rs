use std::convert::Infallible;

use atuin_client::history::{CommandCapture, HistoryId};
use atuin_common::rmp::decode::{self, Bytes, DecodeError};
use atuin_common::rmp::encode::{self, ByteBuf, EncodeError};

use super::Keyspace as KeyspaceTrait;

pub struct Keyspace;

/// The number of MessagePack fields in a serialized value: `output`, `output_observed_bytes`,
/// `output_truncated`, `terminal_width`, `terminal_height`.
const VALUE_FIELDS: u32 = 5;

impl KeyspaceTrait for Keyspace {
    const NAME: &'static str = "output_capture_v1";

    type Key = HistoryId;
    type Value = CommandCapture;

    // Keys are the 16 raw UUID bytes; values are hand-encoded MessagePack.
    type KeySerialized = [u8; 16];
    type ValueSerialized = Vec<u8>;

    type KeySerializationError = Infallible;
    fn serialize_key(key: Self::Key) -> Result<Self::KeySerialized, Self::KeySerializationError> {
        Ok(key.into_bytes())
    }

    type ValueSerializationError = EncodeError;
    fn serialize_value(
        value: Self::Value,
    ) -> Result<Self::ValueSerialized, Self::ValueSerializationError> {
        let mut out = ByteBuf::new();
        encode::write_array_len(&mut out, VALUE_FIELDS)?;
        encode::write_str(&mut out, &value.output)?;
        encode::write_u64(&mut out, value.output_observed_bytes)?;
        encode::write_u8(&mut out, u8::from(value.output_truncated))?;
        encode::write_u16(&mut out, value.terminal_width)?;
        encode::write_u16(&mut out, value.terminal_height)?;
        Ok(out.into_vec())
    }

    type ValueDeserializationError = DecodeError<'static>;
    fn deserialize_value(
        serialized: Self::ValueSerialized,
    ) -> Result<Self::Value, Self::ValueDeserializationError> {
        // The decode closure yields a `DecodeError` borrowing `serialized` (via `read_string`);
        // erase that borrow to `'static` once, at the boundary.
        (|| {
            let mut bytes = Bytes::new(&serialized);

            let nfields = decode::read_array_len(&mut bytes).map_err(DecodeError::from)?;
            if nfields != VALUE_FIELDS {
                return Err(DecodeError::WrongArrayLength {
                    expected: VALUE_FIELDS as usize,
                    actual: nfields,
                });
            }

            let output = decode::read_string(&mut bytes)?;
            let output_observed_bytes = decode::read_u64(&mut bytes).map_err(DecodeError::from)?;
            let output_truncated = decode::read_u8(&mut bytes).map_err(DecodeError::from)? != 0;
            let terminal_width = decode::read_u16(&mut bytes).map_err(DecodeError::from)?;
            let terminal_height = decode::read_u16(&mut bytes).map_err(DecodeError::from)?;

            Ok(CommandCapture {
                output,
                output_observed_bytes,
                output_truncated,
                terminal_width,
                terminal_height,
            })
        })()
        .map_err(DecodeError::into_static)
    }
}

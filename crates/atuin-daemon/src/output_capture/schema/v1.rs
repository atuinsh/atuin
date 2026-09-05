use std::convert::Infallible;

use atuin_client::history::{CommandCapture, HistoryId};
use atuin_common::rmp::decode::{self, Bytes, DecodeError};
use atuin_common::rmp::encode::{self, ByteBuf, EncodeError};

pub struct Schema;

/// The number of MessagePack fields in a serialized value: `output`, `output_observed_bytes`,
/// `output_truncated`, `terminal_width`, `terminal_height`.
const VALUE_FIELDS: u32 = 5;

impl super::Schema for Schema {
    const NAME: &'static str = "output_capture_v1";

    type Key = HistoryId;
    type KeySerialized = [u8; 16];
    type KeySerializationError = Infallible;

    type Value = CommandCapture;
    type ValueSerialized = Vec<u8>;
    type ValueSerializationError = EncodeError;
    type ValueDeserializationError = DecodeError<'static>;

    fn serialize_key(key: Self::Key) -> Result<Self::KeySerialized, Self::KeySerializationError> {
        Ok(key.into_bytes())
    }

    fn serialize_value(
        value: Self::Value,
    ) -> Result<Self::ValueSerialized, Self::ValueSerializationError> {
        let mut out = ByteBuf::new();
        encode::write_array_len(&mut out, VALUE_FIELDS)?;
        encode::write_str(&mut out, &value.output)?;
        encode::write_uint(&mut out, value.output_observed_bytes)?;
        encode::write_bool(&mut out, value.output_truncated);
        encode::write_uint(&mut out, value.terminal_width.into())?;
        encode::write_uint(&mut out, value.terminal_height.into())?;
        Ok(out.into_vec())
    }

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
            let output_observed_bytes: u64 =
                decode::read_int(&mut bytes).map_err(DecodeError::from)?;
            let output_truncated = decode::read_bool(&mut bytes).map_err(DecodeError::from)?;
            let terminal_width: u16 = decode::read_int(&mut bytes).map_err(DecodeError::from)?;
            let terminal_height: u16 = decode::read_int(&mut bytes).map_err(DecodeError::from)?;

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

    fn create_options() -> fjall::KeyspaceCreateOptions {
        fjall::KeyspaceCreateOptions::default()
            .data_block_compression_policy(fjall::config::CompressionPolicy::all(
                fjall::CompressionType::Lz4,
            ))
            .with_kv_separation(Some(fjall::KvSeparationOptions::default()))
    }
}

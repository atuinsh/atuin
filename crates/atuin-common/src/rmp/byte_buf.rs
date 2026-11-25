//! Serializes a byte buffer as MessagePack's binary type, which takes one byte per byte.
//!
//! Use it on a `Vec<u8>` field with `#[serde(with = "atuin_common::rmp::byte_buf")]`.

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserializer, Serializer};

/// The most bytes that a buffer's declared length reserves before the bytes arrive.
const MAX_RESERVED_BYTES: usize = 64 * 1024;

/// Serializes `bytes` as one binary value.
///
/// # Errors
///
/// Fails when the serializer fails.
#[expect(clippy::ptr_arg, reason = "serde's `with` passes a reference to the field")]
pub fn serialize<S: Serializer>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_bytes(bytes)
}

/// Deserializes a byte buffer from a binary value or a sequence of bytes.
///
/// # Errors
///
/// Fails when the deserializer fails, or when the value doesn't hold bytes.
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    struct BytesVisitor;

    impl<'de> Visitor<'de> for BytesVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a byte buffer")
        }

        fn visit_bytes<E: de::Error>(self, bytes: &[u8]) -> Result<Vec<u8>, E> {
            Ok(bytes.to_vec())
        }

        fn visit_byte_buf<E: de::Error>(self, bytes: Vec<u8>) -> Result<Vec<u8>, E> {
            Ok(bytes)
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
            // The sender declares the length, so the reservation stays small, and the buffer
            // grows with the bytes that actually arrive.
            let declared = seq.size_hint().unwrap_or(0);
            let mut bytes = Vec::with_capacity(declared.min(MAX_RESERVED_BYTES));
            while let Some(byte) = seq.next_element::<u8>()? {
                bytes.push(byte);
            }
            Ok(bytes)
        }
    }

    deserializer.deserialize_byte_buf(BytesVisitor)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Holder(#[serde(with = "super")] Vec<u8>);

    /// A deserializer that hands out bytes as a sequence whose declared length is a lie.
    struct LyingSeq(std::vec::IntoIter<u8>);

    impl<'de> serde::de::SeqAccess<'de> for LyingSeq {
        type Error = serde::de::value::Error;

        fn next_element_seed<T: serde::de::DeserializeSeed<'de>>(
            &mut self,
            seed: T,
        ) -> Result<Option<T::Value>, Self::Error> {
            use serde::de::IntoDeserializer as _;
            self.0.next().map(|byte| seed.deserialize(byte.into_deserializer())).transpose()
        }

        fn size_hint(&self) -> Option<usize> {
            Some(usize::MAX)
        }
    }

    impl<'de> serde::Deserializer<'de> for LyingSeq {
        type Error = serde::de::value::Error;

        fn deserialize_any<V: serde::de::Visitor<'de>>(
            self,
            visitor: V,
        ) -> Result<V::Value, Self::Error> {
            visitor.visit_seq(self)
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
            option unit unit_struct newtype_struct seq tuple tuple_struct map struct enum
            identifier ignored_any
        }
    }

    #[rstest]
    fn a_declared_length_reserves_no_more_than_the_limit() {
        let bytes = b"Inbox".to_vec();
        let decoded = super::deserialize(LyingSeq(bytes.clone().into_iter()));
        assert_eq!(decoded, Ok(bytes));
    }

    #[rstest]
    #[case::empty(&[])]
    #[case::text(b"Inbox")]
    #[case::every_byte(&[0, 0x7f, 0x80, 0xc1, 0xff])]
    fn a_buffer_round_trips_as_one_binary_value(#[case] bytes: &[u8]) {
        let holder = Holder(bytes.to_vec());
        let encoded = rmp_serde::to_vec(&holder).unwrap();
        // A `bin 8` marker and a length byte come before the bytes.
        assert_eq!(encoded.len(), bytes.len() + 2);
        assert_eq!(rmp_serde::from_slice::<Holder>(&encoded).unwrap(), holder);
    }
}

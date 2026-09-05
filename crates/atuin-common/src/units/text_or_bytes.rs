//! serde glue shared by the size types: accept either the type's text form (`"1MB"`, `"10%"`,
//! `"unlimited"`) or a bare integer, which is a byte count.

use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;

use serde::de::{Deserializer, Error, Visitor};

use super::ByteSize;

/// Deserialize `T` from a string via [`FromStr`], or from a non-negative integer via
/// `From<ByteSize>`.
pub fn deserialize<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr + From<ByteSize>,
    T::Err: fmt::Display,
{
    deserializer.deserialize_any(TextOrBytes(PhantomData))
}

struct TextOrBytes<T>(PhantomData<T>);

impl<T> Visitor<'_> for TextOrBytes<T>
where
    T: FromStr + From<ByteSize>,
    T::Err: fmt::Display,
{
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a size like \"1MB\" or a number of bytes")
    }

    fn visit_u64<E: Error>(self, bytes: u64) -> Result<T, E> {
        Ok(ByteSize::from_bytes(bytes).into())
    }

    fn visit_i64<E: Error>(self, bytes: i64) -> Result<T, E> {
        let bytes = u64::try_from(bytes).map_err(|_| E::custom("a size cannot be negative"))?;
        self.visit_u64(bytes)
    }

    fn visit_str<E: Error>(self, text: &str) -> Result<T, E> {
        text.parse().map_err(E::custom)
    }
}

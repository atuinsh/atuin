// Copyright 2013-2014 The Rust Project Developers.
// Copyright 2018 The Uuid Project Developers.
//
// See the COPYRIGHT file at the top-level directory of this distribution.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::prelude::*;
use core::fmt;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

impl Serialize for Uuid {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer
                .serialize_str(&self.to_hyphenated().encode_lower(&mut [0; 36]))
        } else {
            serializer.serialize_bytes(self.as_bytes())
        }
    }
}

impl<'de> Deserialize<'de> for Uuid {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        fn de_error<E: de::Error>(e: crate::Error) -> E {
            E::custom(format_args!("UUID parsing failed: {}", e))
        }

        if deserializer.is_human_readable() {
            struct UuidStringVisitor;

            impl<'vi> de::Visitor<'vi> for UuidStringVisitor {
                type Value = Uuid;

                fn expecting(
                    &self,
                    formatter: &mut fmt::Formatter<'_>,
                ) -> fmt::Result {
                    write!(formatter, "a UUID string")
                }

                fn visit_str<E: de::Error>(
                    self,
                    value: &str,
                ) -> Result<Uuid, E> {
                    value.parse::<Uuid>().map_err(de_error)
                }

                fn visit_bytes<E: de::Error>(
                    self,
                    value: &[u8],
                ) -> Result<Uuid, E> {
                    Uuid::from_slice(value).map_err(de_error)
                }
            }

            deserializer.deserialize_str(UuidStringVisitor)
        } else {
            struct UuidBytesVisitor;

            impl<'vi> de::Visitor<'vi> for UuidBytesVisitor {
                type Value = Uuid;

                fn expecting(
                    &self,
                    formatter: &mut fmt::Formatter<'_>,
                ) -> fmt::Result {
                    write!(formatter, "bytes")
                }

                fn visit_bytes<E: de::Error>(
                    self,
                    value: &[u8],
                ) -> Result<Uuid, E> {
                    Uuid::from_slice(value).map_err(de_error)
                }
            }

            deserializer.deserialize_bytes(UuidBytesVisitor)
        }
    }
}

#[cfg(test)]
mod serde_tests {
    use serde_test::{Compact, Configure, Readable, Token};

    use crate::prelude::*;

    #[test]
    fn test_serialize_readable() {
        let uuid_str = "f9168c5e-ceb2-4faa-b6bf-329bf39fa1e4";
        let u = Uuid::parse_str(uuid_str).unwrap();
        serde_test::assert_tokens(&u.readable(), &[Token::Str(uuid_str)]);
    }

    #[test]
    fn test_serialize_compact() {
        let uuid_bytes = b"F9168C5E-CEB2-4F";
        let u = Uuid::from_slice(uuid_bytes).unwrap();
        serde_test::assert_tokens(&u.compact(), &[Token::Bytes(uuid_bytes)]);
    }

    #[test]
    fn test_de_failure() {
        serde_test::assert_de_tokens_error::<Readable<Uuid>>(
            &[Token::Str("hello_world")],
            "UUID parsing failed: invalid length: expected one of [36, 32], found 11",
        );

        serde_test::assert_de_tokens_error::<Compact<Uuid>>(
            &[Token::Bytes(b"hello_world")],
            "UUID parsing failed: invalid bytes length: expected 16, found 11",
        );
    }
}

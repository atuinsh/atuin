//! Module for use with `#[serde(with = "...")]` to serialize a [`Uuid`]
//! as a `[u8; 16]`.
//!
//! [`Uuid`]: ../../struct.Uuid.html

/// Serializer for a [`Uuid`] into a `[u8; 16]`
///
/// [`Uuid`]: ../../struct.Uuid.html
pub fn serialize<S>(u: &crate::Uuid, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serde::Serialize::serialize(u.as_bytes(), serializer)
}

/// Deserializer from a `[u8; 16]` into a [`Uuid`]
///
/// [`Uuid`]: ../../struct.Uuid.html
pub fn deserialize<'de, D>(deserializer: D) -> Result<crate::Uuid, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let bytes: [u8; 16] = serde::Deserialize::deserialize(deserializer)?;

    Ok(crate::Uuid::from_bytes(bytes))
}

#[cfg(test)]
mod tests {

    use serde_test;

    #[test]
    fn test_serialize_compact() {
        #[derive(
            serde_derive::Serialize, Debug, serde_derive::Deserialize, PartialEq,
        )]
        struct UuidContainer {
            #[serde(with = "super")]
            u: crate::Uuid,
        }
        use serde_test::Configure;

        let uuid_bytes = b"F9168C5E-CEB2-4F";
        let container = UuidContainer {
            u: crate::Uuid::from_slice(uuid_bytes).unwrap(),
        };

        // more complex because of the struct wrapping the actual UUID
        // serialization
        serde_test::assert_tokens(
            &container.compact(),
            &[
                serde_test::Token::Struct {
                    name: "UuidContainer",
                    len: 1,
                },
                serde_test::Token::Str("u"),
                serde_test::Token::Tuple { len: 16 },
                serde_test::Token::U8(uuid_bytes[0]),
                serde_test::Token::U8(uuid_bytes[1]),
                serde_test::Token::U8(uuid_bytes[2]),
                serde_test::Token::U8(uuid_bytes[3]),
                serde_test::Token::U8(uuid_bytes[4]),
                serde_test::Token::U8(uuid_bytes[5]),
                serde_test::Token::U8(uuid_bytes[6]),
                serde_test::Token::U8(uuid_bytes[7]),
                serde_test::Token::U8(uuid_bytes[8]),
                serde_test::Token::U8(uuid_bytes[9]),
                serde_test::Token::U8(uuid_bytes[10]),
                serde_test::Token::U8(uuid_bytes[11]),
                serde_test::Token::U8(uuid_bytes[12]),
                serde_test::Token::U8(uuid_bytes[13]),
                serde_test::Token::U8(uuid_bytes[14]),
                serde_test::Token::U8(uuid_bytes[15]),
                serde_test::Token::TupleEnd,
                serde_test::Token::StructEnd,
            ],
        )
    }
}

//! Defines the [`AutoOsString`] converter for use with [`serde_with`].

use std::ffi::{OsStr, OsString};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, SerializeAs};

/// A [`serde_with`] converter for [`OsStr`]-like values that tries to use UTF-8 when possible.
///
/// [`OsString`]s that happen to be valid UTF-8 are encoded as strings; otherwise, this converter
/// falls back to serde's default serialization of [`OsString`], with separate `Unix` and `Windows`
/// variants.
///
/// In JSON, this converter produces results like `{"Utf8": "/tmp/atuin.sock"}` or
/// `{"NonUtf8": {"Unix": [47, 116, ...]}}`.
///
/// # Example
///
/// ```
/// use std::path::PathBuf;
///
/// use atuin_common::string::AutoOsString;
/// use serde::{Deserialize, Serialize};
/// use serde_with::serde_as;
///
/// #[serde_as]
/// #[derive(Serialize, Deserialize)]
/// struct Info {
///     #[serde_as(as = "Option<AutoOsString>")]
///     socket_path: Option<PathBuf>,
/// }
/// ```
pub struct AutoOsString;

#[derive(Serialize)]
#[serde(rename = "AutoOsString")]
enum Borrowed<'a> {
    Utf8(&'a str),
    NonUtf8(&'a OsStr),
}

#[derive(Deserialize)]
#[serde(rename = "AutoOsString")]
enum Owned {
    Utf8(String),
    NonUtf8(OsString),
}

impl<T: AsRef<OsStr>> SerializeAs<T> for AutoOsString {
    fn serialize_as<S: Serializer>(source: &T, serializer: S) -> Result<S::Ok, S::Error> {
        let os_str = source.as_ref();
        match os_str.to_str() {
            Some(s) => Borrowed::Utf8(s),
            None => Borrowed::NonUtf8(os_str),
        }
        .serialize(serializer)
    }
}

impl<'de, T: From<OsString>> DeserializeAs<'de, T> for AutoOsString {
    fn deserialize_as<D: Deserializer<'de>>(deserializer: D) -> Result<T, D::Error> {
        let os_string = match Owned::deserialize(deserializer)? {
            Owned::Utf8(s) => s.into(),
            Owned::NonUtf8(s) => s,
        };
        Ok(os_string.into())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use proptest::prelude::*;
    use rstest::rstest;
    use serde_json::{Value, json};
    use serde_with::{As, serde_as};

    use super::*;

    fn to_json<T: AsRef<OsStr>>(value: &T) -> Value {
        As::<AutoOsString>::serialize(value, serde_json::value::Serializer).unwrap()
    }

    fn from_json<T: From<OsString>>(value: Value) -> serde_json::Result<T> {
        As::<AutoOsString>::deserialize(value)
    }

    #[serde_as]
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Record {
        #[serde_as(as = "Option<AutoOsString>")]
        socket_path: Option<PathBuf>,

        #[serde_as(as = "Vec<AutoOsString>")]
        #[serde(default)]
        paths: Vec<PathBuf>,
    }

    #[cfg(unix)]
    fn non_utf8() -> OsString {
        use std::os::unix::ffi::OsStringExt;

        OsString::from_vec(b"/tmp/\xff.sock".to_vec())
    }

    #[rstest]
    #[case::path("/tmp/atuin-1000/atuin.sock")]
    #[case::empty("")]
    #[case::unicode("/home/zoë/🐢.sock")]
    fn test_utf8_is_a_string(#[case] s: &str) {
        let expected = json!({ "Utf8": s });
        assert_eq!(to_json(&OsString::from(s)), expected);
        assert_eq!(to_json(&PathBuf::from(s)), expected);
        assert_eq!(from_json::<PathBuf>(expected).unwrap(), PathBuf::from(s));
    }

    #[cfg(unix)]
    #[rstest]
    fn test_non_utf8_uses_os_string_encoding() {
        let expected =
            json!({ "NonUtf8": { "Unix": [47, 116, 109, 112, 47, 255, 46, 115, 111, 99, 107] } });
        assert_eq!(to_json(&non_utf8()), expected);
        assert_eq!(to_json(&PathBuf::from(non_utf8())), expected);
        assert_eq!(from_json::<OsString>(expected).unwrap(), non_utf8());
    }

    #[rstest]
    #[case::string(json!("/tmp/a.sock"))]
    #[case::unknown_variant(json!({ "Latin1": "/tmp/a.sock" }))]
    #[case::number(json!(1))]
    fn test_rejects_other_encodings(#[case] value: Value) {
        assert!(from_json::<PathBuf>(value).is_err());
    }

    #[rstest]
    #[case::some(
        Record { socket_path: Some("/tmp/a.sock".into()), paths: vec!["/a".into(), "/b".into()] },
        json!({ "socket_path": { "Utf8": "/tmp/a.sock" }, "paths": [{ "Utf8": "/a" }, { "Utf8": "/b" }] }),
    )]
    #[case::none(
        Record { socket_path: None, paths: vec![] },
        json!({ "socket_path": null, "paths": [] }),
    )]
    fn test_in_containers(#[case] record: Record, #[case] expected: Value) {
        assert_eq!(serde_json::to_value(&record).unwrap(), expected);
        assert_eq!(serde_json::from_value::<Record>(expected).unwrap(), record);
    }

    #[rstest]
    fn test_missing_option_is_none() {
        // `#[serde_as]` adds `#[serde(default)]` to `Option` fields.
        let record: Record = serde_json::from_value(json!({})).unwrap();
        assert_eq!(record, Record {
            socket_path: None,
            paths: vec![]
        });
    }

    #[cfg(unix)]
    fn os_string() -> impl Strategy<Value = OsString> {
        use std::os::unix::ffi::OsStringExt;

        proptest::collection::vec(any::<u8>(), 0..64).prop_map(OsString::from_vec)
    }

    proptest! {
        #[test]
        fn prop_utf8_is_a_string(s in ".*") {
            prop_assert_eq!(to_json(&OsString::from(&s)), json!({ "Utf8": s }));
        }
    }

    #[cfg(unix)]
    proptest! {
        #[test]
        fn prop_round_trip(s in os_string(), others in proptest::collection::vec(os_string(), 0..4)) {
            prop_assert_eq!(&from_json::<OsString>(to_json(&s)).unwrap(), &s);
            prop_assert_eq!(from_json::<PathBuf>(to_json(&s)).unwrap(), PathBuf::from(&s));

            let record = Record {
                socket_path: Some(s.into()),
                paths: others.into_iter().map(PathBuf::from).collect(),
            };
            let json = serde_json::to_string(&record).unwrap();
            prop_assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), record);
        }
    }
}

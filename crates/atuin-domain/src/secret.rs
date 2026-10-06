//! Serde helpers for secrets that must cross the wire in plaintext.
//!
//! [`SecretString`] deliberately has no `Serialize`; request and response types opt a field in
//! with `#[serde(serialize_with = "crate::secret::serialize")]`.

use secrecy::{ExposeSecret, SecretString};
use serde::Serializer;

/// Serialize the secret as a plain string.
pub fn serialize<S: Serializer>(secret: &SecretString, ser: S) -> Result<S::Ok, S::Error> {
    ser.serialize_str(secret.expose_secret())
}

/// Equivalent to [`serialize`], for an optional secret.
pub fn serialize_option<S: Serializer>(
    secret: &Option<SecretString>,
    ser: S,
) -> Result<S::Ok, S::Error> {
    match secret {
        Some(secret) => ser.serialize_some(secret.expose_secret()),
        None => ser.serialize_none(),
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use secrecy::SecretString;

    use crate::api::LoginRequest;

    #[rstest]
    #[case::without_totp(None, r#"{"username":"u","password":"hunter2"}"#)]
    #[case::with_totp(
        Some("123456"),
        r#"{"username":"u","password":"hunter2","totp_code":"123456"}"#
    )]
    fn secret_fields_serialize_in_plaintext_but_debug_redacted(
        #[case] totp_code: Option<&str>,
        #[case] json: &str,
    ) {
        let req = LoginRequest {
            username: "u".into(),
            password: SecretString::from("hunter2"),
            totp_code: totp_code.map(SecretString::from),
        };

        assert_eq!(serde_json::to_string(&req).unwrap(), json);
        let debug = format!("{req:?}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(!debug.contains("123456"), "{debug}");
    }
}

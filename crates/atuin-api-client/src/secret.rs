use std::fmt;

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, Serializer};

/// A secret string the API carries in plaintext: a password, 2FA code, session, token or login
/// code.
///
/// Serializes as the plain string, so it reaches the wire; `Debug` never prints it. Read it with
/// [`ExposeSecret`].
#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub struct Secret(SecretString);

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.expose_secret())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

impl ExposeSecret<str> for Secret {
    fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

impl From<SecretString> for Secret {
    fn from(secret: SecretString) -> Self {
        Self(secret)
    }
}

impl From<Secret> for SecretString {
    fn from(secret: Secret) -> Self {
        secret.0
    }
}

impl From<String> for Secret {
    fn from(secret: String) -> Self {
        Self(secret.into())
    }
}

impl From<&str> for Secret {
    fn from(secret: &str) -> Self {
        Self(secret.into())
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use serde_json::json;

    use super::Secret;
    use crate::types::LoginRequest;

    const PLAINTEXT: &str = "hunter2";

    fn secret() -> Secret {
        Secret::from(PLAINTEXT)
    }

    #[rstest]
    fn request_secrets_serialize_in_plaintext_but_never_debug() {
        let request = LoginRequest {
            username: "ellie".into(),
            password: secret(),
            totp_code: Some(secret()),
        };
        let debug = format!("{request:?}");
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({"username": "ellie", "password": PLAINTEXT, "totp_code": PLAINTEXT})
        );
        assert!(!debug.contains(PLAINTEXT), "{debug}");
    }
}

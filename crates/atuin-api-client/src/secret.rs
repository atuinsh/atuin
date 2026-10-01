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
    use std::fmt::Debug;

    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use secrecy::ExposeSecret;
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};

    use super::Secret;
    use crate::types::{
        ChangePasswordRequest, CliCodeResponse, CliVerifyResponse, DeleteUserRequest,
        LinkAccountRequest, LoginRequest, LoginResponse, RegisterRequest, RegisterResponse,
    };

    const PLAINTEXT: &str = "hunter2";

    fn secret() -> Secret {
        Secret::from(PLAINTEXT)
    }

    /// `value`'s `Debug` output and JSON.
    fn encoded<T: Debug + Serialize>(value: &T) -> (String, Value) {
        (format!("{value:?}"), serde_json::to_value(value).unwrap())
    }

    /// The `Debug` output of `body` decoded as `T`, and the secret `field` reads from it.
    fn decoded<T: Debug + DeserializeOwned>(
        body: &str,
        field: fn(&T) -> &Secret,
    ) -> (String, String) {
        let value: T = serde_json::from_str(body).unwrap();
        (format!("{value:?}"), field(&value).expose_secret().to_owned())
    }

    #[rstest]
    #[case::login(
        encoded(&LoginRequest { username: "ellie".into(), password: secret(), totp_code: Some(secret()) }),
        json!({"username": "ellie", "password": PLAINTEXT, "totp_code": PLAINTEXT})
    )]
    #[case::register(
        encoded(&RegisterRequest { email: "e@example.com".into(), username: "ellie".into(), password: secret() }),
        json!({"email": "e@example.com", "username": "ellie", "password": PLAINTEXT})
    )]
    #[case::change_password(
        encoded(&ChangePasswordRequest { current_password: secret(), new_password: secret(), totp_code: Some(secret()) }),
        json!({"current_password": PLAINTEXT, "new_password": PLAINTEXT, "totp_code": PLAINTEXT})
    )]
    #[case::delete_user(
        encoded(&DeleteUserRequest { password: secret(), totp_code: Some(secret()) }),
        json!({"password": PLAINTEXT, "totp_code": PLAINTEXT})
    )]
    #[case::link_account(encoded(&LinkAccountRequest { token: secret() }), json!({"token": PLAINTEXT}))]
    fn request_secrets_serialize_in_plaintext_but_never_debug(
        #[case] encoded: (String, Value),
        #[case] wire: Value,
    ) {
        let (debug, json) = encoded;
        assert_eq!(json, wire);
        assert!(!debug.contains(PLAINTEXT), "{debug}");
    }

    #[rstest]
    #[case::login(decoded(r#"{"session": "hunter2", "auth": "cli"}"#, |r: &LoginResponse| &r.session))]
    #[case::register(decoded(r#"{"session": "hunter2"}"#, |r: &RegisterResponse| &r.session))]
    #[case::cli_code(decoded(r#"{"code": "hunter2"}"#, |r: &CliCodeResponse| &r.code))]
    #[case::cli_verify(decoded(r#"{"success": true, "token": "hunter2"}"#, |r: &CliVerifyResponse| &r.token))]
    fn response_secrets_deserialize_but_never_debug(#[case] decoded: (String, String)) {
        let (debug, secret) = decoded;
        assert_eq!(secret, PLAINTEXT);
        assert!(!debug.contains(PLAINTEXT), "{debug}");
    }
}

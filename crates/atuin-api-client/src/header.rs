use reqwest::header::{HeaderValue, InvalidHeaderValue};
use secrecy::zeroize::Zeroizing;
use secrecy::{ExposeSecret, SecretString};

/// `<scheme> <token>` as a sensitive `Authorization` value, e.g. `Bearer atapi_...`.
///
/// Sensitive, so the token stays out of `Debug` output and HTTP/2 header compression.
///
/// # Errors
///
/// [`InvalidHeaderValue`] when the token holds a byte a header cannot carry, e.g. a newline.
pub fn authorization(
    scheme: &str,
    token: &SecretString,
) -> Result<HeaderValue, InvalidHeaderValue> {
    let value = Zeroizing::new(format!("{scheme} {}", token.expose_secret()));
    let mut header = HeaderValue::from_str(&value)?;
    header.set_sensitive(true);
    Ok(header)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use secrecy::SecretString;

    use super::authorization;

    #[rstest]
    #[case::bearer("Bearer")]
    #[case::token("Token")]
    fn is_sensitive_and_never_debug_printed(#[case] scheme: &str) {
        let header = authorization(scheme, &SecretString::from("hunter2")).unwrap();

        assert_eq!(header, format!("{scheme} hunter2").as_str());
        assert!(header.is_sensitive());
        assert!(!format!("{header:?}").contains("hunter2"), "{header:?}");
    }

    #[rstest]
    fn rejects_a_token_a_header_cannot_carry() {
        assert!(authorization("Bearer", &SecretString::from("atapi_\ntoken")).is_err());
    }
}

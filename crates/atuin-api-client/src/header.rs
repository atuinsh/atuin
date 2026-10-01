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
    fn carries_the_scheme_and_is_sensitive() {
        let header = authorization("Bearer", &SecretString::from("hunter2")).unwrap();

        assert_eq!(header, "Bearer hunter2");
        assert!(header.is_sensitive());
    }
}

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

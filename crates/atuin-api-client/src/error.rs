use std::fmt;

use progenitor_client::Error;
use reqwest::StatusCode;
use serde_json::{Map, Value};
use url::Url;

/// A failed API call.
///
/// Displays no secret: error bodies are the server's text, and URLs lose their query, which
/// carries the CLI login code and presigned signatures.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The server answered outside 2xx.
    #[error("{url} answered {status}{}", with_reason(reason.as_deref().or(body.as_deref())))]
    Status {
        status: StatusCode,
        /// The URL that answered, without its query. Boxed to keep every `Result` of this crate
        /// small.
        url: Box<Url>,
        /// The reason a JSON body gives: its `reason`, else `message`, `error`, or the first of
        /// `errors`.
        reason: Option<String>,
        /// The body's machine-readable `code`, e.g. `2fa_required`.
        code: Option<String>,
        /// The body's trimmed text when it gives no `reason`, e.g. a proxy's HTML page; `None`
        /// when it is empty or could not be read.
        body: Option<String>,
    },
    /// The request got no answer, or its body could not be read: DNS, TLS, connect or timeout.
    #[error(transparent)]
    Transport(reqwest::Error),
    /// A 2xx answer whose body does not match the API.
    #[error("the server's answer does not match the API: {0}")]
    Decode(serde_json::Error),
    /// The client refused to send the request, e.g. for a header value that is not ASCII.
    #[error("request not sent: {0}")]
    NotSent(String),
}

impl ApiError {
    /// The status the server answered with, if it answered outside 2xx.
    #[must_use]
    pub const fn status(&self) -> Option<StatusCode> {
        match self {
            Self::Status { status, .. } => Some(*status),
            Self::Transport(_) | Self::Decode(_) | Self::NotSent(_) => None,
        }
    }

    async fn from_error(err: Error) -> Self {
        match err {
            Error::UnexpectedResponse(response) => Self::from_response(response).await,
            Error::CommunicationError(err)
            | Error::InvalidUpgrade(err)
            | Error::ResponseBodyError(err) => Self::Transport(without_query(err)),
            // The body may be a secret-bearing success, e.g. a login's session: keep it out.
            Error::InvalidResponsePayload(_body, err) => Self::Decode(err),
            Error::InvalidRequest(reason) | Error::Custom(reason) => Self::NotSent(reason),
            Error::ErrorResponse(_) => unreachable!(
                "codegen strips every documented error response, so no generated call returns \
                 ErrorResponse"
            ),
        }
    }

    /// The error for `response`, which answered outside 2xx, with the reason read from its body.
    ///
    /// For answers that do not come through a [`Client`](crate::Client) operation, e.g. a
    /// presigned upload.
    pub async fn from_response(response: reqwest::Response) -> Self {
        let status = response.status();
        let mut url = response.url().clone();
        url.set_query(None);
        // The status is the answer; a body that breaks off only loses its detail.
        let Body { reason, code, text } =
            response.text().await.map_or_else(|_| Body::default(), |body| Body::parse(&body));
        Self::Status {
            status,
            url: Box::new(url),
            reason,
            code,
            body: text,
        }
    }
}

/// Maps the failure of a [`Client`](crate::Client) call into an [`ApiError`].
pub trait MapApiError<T>: Future<Output = Result<T, Error>> + Sized {
    /// Await the call, reading the body of an error answer into [`ApiError::Status`].
    fn map_api_error(self) -> impl Future<Output = Result<T, ApiError>>;
}

impl<T, F> MapApiError<T> for F
where
    F: Future<Output = Result<T, Error>>,
{
    async fn map_api_error(self) -> Result<T, ApiError> {
        match self.await {
            Ok(value) => Ok(value),
            Err(err) => Err(ApiError::from_error(err).await),
        }
    }
}

fn without_query(mut err: reqwest::Error) -> reqwest::Error {
    if let Some(url) = err.url_mut() {
        url.set_query(None);
    }
    err
}

fn with_reason(reason: Option<&str>) -> impl fmt::Display {
    fmt::from_fn(move |f| reason.map_or(Ok(()), |reason| write!(f, ": {reason}")))
}

/// What an error body says, in any envelope an Atuin server, or a proxy in front of one, sends:
/// `{reason, code}`, the AI routes' `{error, message}`, Phoenix's `{errors: [..]}`, the older
/// `{error}`, or text.
#[derive(Debug, Default, PartialEq, Eq)]
struct Body {
    reason: Option<String>,
    code: Option<String>,
    /// The trimmed body when it gives no `reason` and is not empty.
    text: Option<String>,
}

impl Body {
    fn parse(body: &str) -> Self {
        let text = body.trim();
        let object = match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(object)) => Some(object),
            _ => None,
        };
        let reason = object.as_ref().and_then(|object| {
            ["reason", "message", "error"]
                .into_iter()
                .find_map(|key| string(object, key))
                .or_else(|| object.get("errors")?.get(0)?.as_str().map(str::to_owned))
        });
        Self {
            code: object.as_ref().and_then(|object| string(object, "code")),
            text: (reason.is_none() && !text.is_empty()).then(|| text.to_owned()),
            reason,
        }
    }
}

fn string(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key)?.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::Body;

    #[rstest]
    #[case::reason(r#"{"reason": "invalid session"}"#, Some("invalid session"), None, None)]
    #[case::reason_and_code(
        r#"{"reason": "2FA required", "code": "2fa_required"}"#,
        Some("2FA required"),
        Some("2fa_required"),
        None
    )]
    #[case::reason_wins_over_legacy_keys(
        r#"{"reason": "new", "error": "old", "errors": ["older"]}"#,
        Some("new"),
        None,
        None
    )]
    #[case::ai_envelope(
        r#"{"error": "internal_error", "message": "The engine failed"}"#,
        Some("The engine failed"),
        None,
        None
    )]
    #[case::legacy_error(r#"{"error": "code expired"}"#, Some("code expired"), None, None)]
    #[case::phoenix_errors(r#"{"errors": ["Unauthorized"]}"#, Some("Unauthorized"), None, None)]
    #[case::code_without_reason(
        r#"{"code": "2fa_required"}"#,
        None,
        Some("2fa_required"),
        Some(r#"{"code": "2fa_required"}"#)
    )]
    #[case::phoenix_errors_detail(
        r#"{"errors": {"detail": "Not Found"}}"#,
        None,
        None,
        Some(r#"{"errors": {"detail": "Not Found"}}"#)
    )]
    #[case::unknown_json_object(r#"{"detail": "nope"}"#, None, None, Some(r#"{"detail": "nope"}"#))]
    #[case::non_string_reason(r#"{"reason": 42}"#, None, None, Some(r#"{"reason": 42}"#))]
    #[case::plain_text("capabilities out of date", None, None, Some("capabilities out of date"))]
    #[case::json_string(r#""quoted""#, None, None, Some(r#""quoted""#))]
    #[case::html(
        "<html>502 Bad Gateway</html>\n",
        None,
        None,
        Some("<html>502 Bad Gateway</html>")
    )]
    #[case::empty("", None, None, None)]
    #[case::whitespace(" \n", None, None, None)]
    fn parses_every_error_envelope_leniently(
        #[case] body: &str,
        #[case] reason: Option<&str>,
        #[case] code: Option<&str>,
        #[case] text: Option<&str>,
    ) {
        assert_eq!(Body::parse(body), Body {
            reason: reason.map(str::to_owned),
            code: code.map(str::to_owned),
            text: text.map(str::to_owned),
        });
    }

    /// `from_error` may treat `ErrorResponse` as unreachable only while no generated operation
    /// returns one, which holds while codegen strips every documented error response.
    #[rstest]
    fn no_generated_operation_returns_a_documented_error() {
        let code: String =
            include_str!(concat!(env!("OUT_DIR"), "/generated.rs")).split_whitespace().collect();
        assert!(!code.contains("Error::ErrorResponse("));
    }
}

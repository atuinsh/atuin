use progenitor_client::{Error, ResponseValue};
use reqwest::StatusCode;
use serde_json::Value;
use url::Url;

/// A failed API call.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The server answered outside 2xx.
    #[error(
        "{url} answered {status}{}",
        reason.as_ref().or(body.as_ref()).map(|reason| format!(": {reason}")).unwrap_or_default()
    )]
    Status {
        status: StatusCode,
        /// The URL that answered, without its query.
        url: Box<Url>,
        /// The reason a JSON body gives: `reason`, `message`, `error`, or the first of `errors`.
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
            Error::CommunicationError(mut err)
            | Error::InvalidUpgrade(mut err)
            | Error::ResponseBodyError(mut err) => {
                if let Some(url) = err.url_mut() {
                    url.set_query(None);
                }
                Self::Transport(err)
            }
            // The body may be a secret-bearing success, e.g. a login's session: keep it out.
            Error::InvalidResponsePayload(_body, err) => Self::Decode(err),
            Error::InvalidRequest(reason) | Error::Custom(reason) => Self::NotSent(reason),
            Error::ErrorResponse(_) => unreachable!(
                "codegen strips every documented error response, so no generated call returns \
                 ErrorResponse"
            ),
        }
    }

    /// `response`, or the error it answers with when its status is outside 2xx.
    pub async fn check(response: reqwest::Response) -> Result<reqwest::Response, Self> {
        if response.status().is_success() {
            return Ok(response);
        }
        Err(Self::from_response(response).await)
    }

    /// The error for `response`, whatever its status, with the reason read from its body.
    async fn from_response(response: reqwest::Response) -> Self {
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

/// The body of a [`Client`](crate::Client) call whose caller needs no status or headers.
pub trait ApiBody<T>: Future<Output = Result<ResponseValue<T>, Error>> + Sized {
    /// Await the call and return its body, failing as [`MapApiError::map_api_error`] does.
    fn body(self) -> impl Future<Output = Result<T, ApiError>>;
}

impl<T, F> ApiBody<T> for F
where
    F: Future<Output = Result<ResponseValue<T>, Error>>,
{
    async fn body(self) -> Result<T, ApiError> {
        self.map_api_error().await.map(ResponseValue::into_inner)
    }
}

/// What an error body says.
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
                .find_map(|key| object.get(key)?.as_str().map(str::to_owned))
                .or_else(|| object.get("errors")?.get(0)?.as_str().map(str::to_owned))
        });
        Self {
            code: object
                .as_ref()
                .and_then(|object| object.get("code")?.as_str().map(str::to_owned)),
            text: (reason.is_none() && !text.is_empty()).then(|| text.to_owned()),
            reason,
        }
    }
}

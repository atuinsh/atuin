//! Which request a call serves, so what the daemon tells the user while serving it reaches the
//! client that asked, and only that client.
//!
//! A client stamps every call it makes inside [`scope`] with the request's [`RequestId`] in
//! [`HEADER`]. The daemon serves each stamped call in [`RequestId::span`], so every message it shows
//! meanwhile carries the request, and its sink sends that message to the client listening for it.

use std::task::{Context, Poll};

use atuin_common::userio::log::RequestId;
use tonic::metadata::MetadataValue;
use tonic::service::Interceptor;
use tonic::{Request, Status};
use tower::{Layer, Service};
use tracing::Instrument as _;
use tracing::instrument::Instrumented;

use crate::grpc::versioning::StrictVersionInterceptor;

/// The header that names the request a call serves.
pub const HEADER: &str = "atuin-request";

tokio::task_local! {
    static CURRENT: RequestId;
}

/// Runs `calls`, stamping every call to the daemon they make with `request`.
pub async fn scope<F: Future>(request: RequestId, calls: F) -> F::Output {
    CURRENT.scope(request, calls).await
}

/// The request `headers` name, if they name one.
#[must_use]
pub fn of(headers: &http::HeaderMap) -> Option<RequestId> {
    headers.get(HEADER)?.to_str().ok()?.parse().ok()
}

/// Stamps each call a client makes with its build and, inside [`scope`], its request.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClientInterceptor;

impl Interceptor for ClientInterceptor {
    fn call(&mut self, request: Request<()>) -> Result<Request<()>, Status> {
        let mut request = StrictVersionInterceptor.call(request)?;
        if let Ok(id) = CURRENT.try_with(|id| *id) {
            let id = MetadataValue::try_from(id.to_string()).expect("a request id is ASCII");
            request.metadata_mut().insert(HEADER, id);
        }
        Ok(request)
    }
}

/// Serves each call that names a request in that request's [`RequestId::span`].
///
/// Only until the handler returns: hyper polls a streamed reply's body outside the span, so what
/// producing its items shows doesn't carry the request.
#[derive(Clone, Copy, Debug, Default)]
pub struct RequestSpanLayer;

impl<S> Layer<S> for RequestSpanLayer {
    type Service = RequestSpanService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequestSpanService { inner }
    }
}

/// See [`RequestSpanLayer`].
#[derive(Clone, Debug)]
pub struct RequestSpanService<S> {
    inner: S,
}

impl<S, B> Service<http::Request<B>> for RequestSpanService<S>
where
    S: Service<http::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Instrumented<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        let span = of(request.headers()).map_or_else(tracing::Span::none, RequestId::span);
        self.inner.call(request).instrument(span)
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use atuin_common::userio::log::{Held, RequestId, SinkStack, USER, UserLayer};
    use rstest::rstest;
    use tower::{Layer as _, Service as _, ServiceExt as _};
    use tracing_subscriber::layer::SubscriberExt as _;

    use super::{HEADER, RequestSpanLayer};

    #[rstest]
    #[tokio::test]
    async fn a_call_naming_a_request_shows_its_messages_with_it() {
        static STACK: SinkStack = SinkStack::new();
        let shown = Held::default();
        let _shown = STACK.push(shown.clone());
        let _subscriber = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(UserLayer::new(&STACK)),
        );
        let mut service = RequestSpanLayer.layer(tower::service_fn(|_: http::Request<()>| async {
            tracing::info!(target: USER, "shown");
            Ok::<_, Infallible>(())
        }));
        let request = RequestId::generate();

        let call = http::Request::builder().header(HEADER, request.to_string()).body(()).unwrap();
        service.ready().await.unwrap().call(call).await.unwrap();

        let requests: Vec<_> = shown.take().into_iter().map(|m| m.request).collect();
        assert_eq!(requests, [Some(request)]);
    }
}

//! The client side of capability negotiation: the server's capabilities, fetched and cached.
//!
//! [`CapClient`] fetches the server's capabilities document through a function its owner injects,
//! so it knows nothing of the transport or the route; `atuin-api-client` injects its
//! `get_capabilities` operation. A negotiating client stamps [`CapClient::known_token`] onto each
//! request. When that token is stale, [`CapMismatch`] decides what happens: `Continue` lets the
//! server serve the request and refreshes capabilities in the background, to the token
//! [`token_to_refresh`] picks; `Error` asks the server to reject with `412`. The original request
//! is never resent.

use std::error::Error;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use tokio::sync::{Mutex, watch};

use super::Capability;

type BoxError = Box<dyn Error + Send + Sync>;

type FetchFuture = Pin<Box<dyn Future<Output = Result<CapsDocument, BoxError>> + Send>>;

/// How the client reacts when its capability token is out of date with the server's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapMismatch {
    /// Let the server serve the request despite the mismatch, then refresh capabilities in the
    /// background so later requests are current. The original request is never resent.
    Continue,
    /// Ask the server (via `x-atuin-capabilities-enforce`) to reject the request with `412` on a
    /// mismatch, surfacing it to the caller.
    Error,
}

/// The capabilities document a server advertises: `{"version": <token>, "capabilities": {..}}`.
#[derive(Debug, Deserialize)]
pub struct CapsDocument {
    /// The server's capability token.
    pub version: String,
    /// Each advertised capability's value, by name.
    pub capabilities: Map<String, Value>,
}

/// The server's capabilities, as the client last fetched them.
///
/// They are fetched once on construction and again by [`CapClient::refresh_if_stale`] whenever
/// negotiation finds the cache stale; [`CapClient::get_server`] is then a read of that cache.
///
/// Thread it as an [`Arc`].
#[derive(derive_more::Debug)]
pub struct CapClient {
    /// The server's capabilities as last fetched; `None` until the first refresh. Cheap concurrent
    /// reads; writes are serialized by `fetching`.
    server: RwLock<Option<CapsDocument>>,
    /// Serializes capability fetches so a burst of stale callers makes a single fetch.
    fetching: Mutex<()>,
    /// Fetches the server's document.
    #[debug(skip)]
    fetch: Box<dyn Fn() -> FetchFuture + Send + Sync>,
    /// Flips to `true` once the warm-up fetch has finished, whether or not it succeeded.
    warmed: watch::Receiver<bool>,
}

/// A failed [`CapClient`] fetch, the fetch's own error kept as the source.
#[derive(Debug, thiserror::Error)]
#[error("failed to fetch the server's capabilities")]
pub struct FetchError(#[source] BoxError);

/// Why reading a server capability could not yield a value.
#[derive(Debug, thiserror::Error)]
pub enum ServerSupportError {
    /// Capabilities have not been fetched from the server yet -- the caller may want to
    /// [`CapClient::refresh`] and ask again. This is an absence of knowledge, distinct from the
    /// server telling us it does not advertise the capability (which is `Ok(None)`).
    #[error("server capabilities have not been fetched yet")]
    NotFetched,
    /// The server advertises the capability, but its value did not deserialize into the type the
    /// caller asked for -- typically a version skew, or the wrong type for the name.
    #[error("server capability {name:?} did not deserialize into the requested type")]
    Malformed {
        name: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

impl CapClient {
    /// Read the capabilities `fetch` returns, starting a warm-up fetch now.
    ///
    /// Must run inside a tokio runtime.
    #[must_use]
    pub fn new<F, Fut, E>(fetch: F) -> Arc<Self>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<CapsDocument, E>> + Send + 'static,
        E: Error + Send + Sync + 'static,
    {
        let (warmed, warmed_rx) = watch::channel(false);
        let new = Arc::new(Self {
            server: RwLock::new(None),
            fetching: Mutex::new(()),
            fetch: Box::new(move || -> FetchFuture {
                let fetched = fetch();
                Box::pin(async move { fetched.await.map_err(BoxError::from) })
            }),
            warmed: warmed_rx,
        });

        let this = Arc::clone(&new);
        // Detached on purpose: only `get_server` waits on it, through `warmed`, and a failed fetch
        // leaves the cache empty, which `get_server` reports as `NotFetched`.
        tokio::spawn(async move {
            let _ = this.refresh().await;
            let _ = warmed.send(true);
        });

        new
    }

    /// Fetch the server's capabilities and replace the cache with them.
    ///
    /// Fetches are serialized so parallel callers never overlap; this one always fetches.
    ///
    /// # Errors
    ///
    /// [`FetchError`] when the fetch fails, leaving the cache as it was.
    pub async fn refresh(&self) -> Result<(), FetchError> {
        let _fetching = self.fetching.lock().await;
        let document = (self.fetch)().await.map_err(FetchError)?;
        *self.server.write() = Some(document);
        Ok(())
    }

    /// Refresh the server's capabilities only if the cached token differs from `available`.
    ///
    /// Double-checked so a burst of stale callers makes a single fetch, and a caller whose token
    /// already matches `available` does no work.
    ///
    /// # Errors
    ///
    /// [`FetchError`] when the fetch fails, leaving the cache as it was.
    pub async fn refresh_if_stale(&self, available: &str) -> Result<(), FetchError> {
        if !self.is_stale(available) {
            return Ok(());
        }
        let _fetching = self.fetching.lock().await;
        if !self.is_stale(available) {
            return Ok(());
        }
        let document = (self.fetch)().await.map_err(FetchError)?;
        *self.server.write() = Some(document);
        Ok(())
    }

    /// Whether the cached server token differs from `available` (or nothing is cached yet).
    fn is_stale(&self, available: &str) -> bool {
        self.server.read().as_ref().map(|document| document.version.as_str()) != Some(available)
    }

    /// Read whether the server supports the given capability, from the last [`CapClient::refresh`].
    ///
    /// Waits for the warm-up fetch first.
    ///
    /// - `Ok(Some(c))` - the server advertises the capability and it deserialized into `C`.
    /// - `Ok(None)` - fetched, and the server does not advertise it (a definitive "no").
    /// - `Err(ServerSupportError::NotFetched)` - capabilities have not been fetched yet.
    /// - `Err(ServerSupportError::Malformed)` - advertised, but its value did not deserialize into
    ///   `C`. The caller decides whether that is fatal or a reason to fall back.
    ///
    /// # Errors
    ///
    /// [`ServerSupportError`], as above.
    pub async fn get_server<C: Capability + DeserializeOwned>(
        &self,
    ) -> Result<Option<C>, ServerSupportError> {
        let _ = self.warmed.clone().wait_for(|&done| done).await;

        let cached = self
            .server
            .read()
            .as_ref()
            .map(|document| document.capabilities.get(C::static_name()).cloned());
        let Some(raw) = cached.ok_or(ServerSupportError::NotFetched)? else {
            return Ok(None);
        };

        serde_json::from_value(raw).map(Some).map_err(|source| ServerSupportError::Malformed {
            name: C::static_name(),
            source,
        })
    }

    /// The capability token this client currently knows, or `None` if it has never fetched.
    ///
    /// The token is opaque: it is the [`CapsDocument::version`] the server last sent, echoed back
    /// to the server verbatim. The client never interprets it.
    #[must_use]
    pub fn known_token(&self) -> Option<String> {
        self.server.read().as_ref().map(|document| document.version.clone())
    }
}

/// The capability token to refresh to after an answer advertised `available`, if any.
///
/// Only [`CapMismatch::Continue`] refreshes, and only to a token that differs from both `sent`,
/// the one the request carried, and `cached`, the one cached now.
#[must_use]
pub fn token_to_refresh<'a>(
    on_mismatch: CapMismatch,
    available: Option<&'a str>,
    sent: Option<&str>,
    cached: Option<&str>,
) -> Option<&'a str> {
    match on_mismatch {
        CapMismatch::Error => None,
        CapMismatch::Continue => {
            available.filter(|available| Some(*available) != sent && Some(*available) != cached)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future;

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{CapClient, CapsDocument};
    use crate::caps::{CapServer, CapabilitiesCap};

    #[rstest]
    #[tokio::test]
    async fn observes_the_capability_the_server_advertises() {
        let advertised = CapServer::new().add(CapabilitiesCap { version: 1 }).unwrap();
        let body = advertised.body().to_owned();
        let fetch = move || future::ready(serde_json::from_str::<CapsDocument>(&body));

        let caps = CapClient::new(fetch);

        assert_eq!(
            caps.get_server::<CapabilitiesCap>().await.unwrap(),
            Some(CapabilitiesCap { version: 1 })
        );
    }
}

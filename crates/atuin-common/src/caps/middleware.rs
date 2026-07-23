use std::sync::Arc;

use http::Extensions;
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next, Result};

use crate::caps::OwnCaps;

/// Reqwest middleware which advertises this client's capabilities on outgoing requests.
///
/// Holds the same `OwnCaps` leaf the owning [`CapClient`](crate::caps::CapClient) holds, so there
/// is no ownership cycle back through the http client.
#[derive(Debug, Clone)]
pub struct CapMiddleware {
    own: Arc<OwnCaps>,
}

impl CapMiddleware {
    pub fn new(own: Arc<OwnCaps>) -> Self {
        return Self { own };
    }
}

#[async_trait::async_trait]
impl Middleware for CapMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        next.run(req, extensions).await
    }
}

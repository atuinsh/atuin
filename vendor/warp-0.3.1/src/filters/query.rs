//! Query Filters

use futures::future;
use serde::de::DeserializeOwned;
use serde_urlencoded;

use crate::filter::{filter_fn_one, Filter, One};
use crate::reject::{self, Rejection};

/// Creates a `Filter` that decodes query parameters to the type `T`.
///
/// If cannot decode into a `T`, the request is rejected with a `400 Bad Request`.
pub fn query<T: DeserializeOwned + Send + 'static>(
) -> impl Filter<Extract = One<T>, Error = Rejection> + Copy {
    filter_fn_one(|route| {
        let query_string = route.query().unwrap_or_else(|| {
            tracing::debug!("route was called without a query string, defaulting to empty");
            ""
        });

        let query_encoded = serde_urlencoded::from_str(query_string).map_err(|e| {
            tracing::debug!("failed to decode query string '{}': {:?}", query_string, e);
            reject::invalid_query()
        });
        future::ready(query_encoded)
    })
}

/// Creates a `Filter` that returns the raw query string as type String.
pub fn raw() -> impl Filter<Extract = One<String>, Error = Rejection> + Copy {
    filter_fn_one(|route| {
        let route = route
            .query()
            .map(|q| q.to_owned())
            .map(Ok)
            .unwrap_or_else(|| Err(reject::invalid_query()));
        future::ready(route)
    })
}

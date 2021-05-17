//! Integration with [`tiny_http`](https://github.com/frewsxcv/tiny-http) with the `tiny_http`
//! feature (optional).
//!
//! Contains `impl `[`HttpRequest`](../trait.HttpRequest.html)` for tiny_http::Request` (not shown
//! here; see [`HttpRequest`'s implementors](../trait.HttpRequest.html#implementors)).

pub use tiny_http::Request as TinyHttpRequest;

use super::HttpRequest;

use std::io::Read;

impl<'r> HttpRequest for &'r mut TinyHttpRequest {
    type Body = &'r mut dyn Read;

    fn multipart_boundary(&self) -> Option<&str> {
        const BOUNDARY: &str = "boundary=";

        let content_type = try_opt!(self
            .headers()
            .iter()
            .find(|header| header.field.equiv("Content-Type")))
        .value
        .as_str();
        let start = try_opt!(content_type.find(BOUNDARY)) + BOUNDARY.len();
        let end = content_type[start..]
            .find(';')
            .map_or(content_type.len(), |end| start + end);

        Some(&content_type[start..end])
    }

    fn body(self) -> Self::Body {
        self.as_reader()
    }
}

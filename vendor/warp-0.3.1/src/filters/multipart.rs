//! Multipart body filters
//!
//! Filters that extract a multipart body for a route.

use std::fmt;
use std::future::Future;
use std::io::{Cursor, Read};
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Buf, Bytes};
use futures::{future, Stream};
use headers::ContentType;
use mime::Mime;
use multipart::server::Multipart;

use crate::filter::{Filter, FilterBase, Internal};
use crate::reject::{self, Rejection};

// If not otherwise configured, default to 2MB.
const DEFAULT_FORM_DATA_MAX_LENGTH: u64 = 1024 * 1024 * 2;

/// A `Filter` to extract a `multipart/form-data` body from a request.
///
/// Create with the `warp::multipart::form()` function.
#[derive(Debug, Clone)]
pub struct FormOptions {
    max_length: u64,
}

/// A `Stream` of multipart/form-data `Part`s.
///
/// Extracted with a `warp::multipart::form` filter.
pub struct FormData {
    inner: Multipart<Cursor<::bytes::Bytes>>,
}

/// A single "part" of a multipart/form-data body.
///
/// Yielded from the `FormData` stream.
pub struct Part {
    name: String,
    filename: Option<String>,
    content_type: Option<String>,
    data: Option<Vec<u8>>,
}

/// Create a `Filter` to extract a `multipart/form-data` body from a request.
///
/// The extracted `FormData` type is a `Stream` of `Part`s, and each `Part`
/// in turn is a `Stream` of bytes.
pub fn form() -> FormOptions {
    FormOptions {
        max_length: DEFAULT_FORM_DATA_MAX_LENGTH,
    }
}

// ===== impl Form =====

impl FormOptions {
    /// Set the maximum byte length allowed for this body.
    ///
    /// Defaults to 2MB.
    pub fn max_length(mut self, max: u64) -> Self {
        self.max_length = max;
        self
    }
}

type FormFut = Pin<Box<dyn Future<Output = Result<(FormData,), Rejection>> + Send>>;

impl FilterBase for FormOptions {
    type Extract = (FormData,);
    type Error = Rejection;
    type Future = FormFut;

    fn filter(&self, _: Internal) -> Self::Future {
        let boundary = super::header::header2::<ContentType>().and_then(|ct| {
            let mime = Mime::from(ct);
            let mime = mime
                .get_param("boundary")
                .map(|v| v.to_string())
                .ok_or_else(|| reject::invalid_header("content-type"));
            future::ready(mime)
        });

        let filt = super::body::content_length_limit(self.max_length)
            .and(boundary)
            .and(super::body::bytes())
            .map(|boundary, body| FormData {
                inner: Multipart::with_body(Cursor::new(body), boundary),
            });

        let fut = filt.filter(Internal);

        Box::pin(fut)
    }
}

// ===== impl FormData =====

impl fmt::Debug for FormData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("FormData").finish()
    }
}

impl Stream for FormData {
    type Item = Result<Part, crate::Error>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Option<Self::Item>> {
        match (*self).inner.read_entry() {
            Ok(Some(mut field)) => {
                let mut data = Vec::new();
                field
                    .data
                    .read_to_end(&mut data)
                    .map_err(crate::Error::new)?;
                Poll::Ready(Some(Ok(Part {
                    name: field.headers.name.to_string(),
                    filename: field.headers.filename,
                    content_type: field.headers.content_type.map(|m| m.to_string()),
                    data: Some(data),
                })))
            }
            Ok(None) => Poll::Ready(None),
            Err(e) => Poll::Ready(Some(Err(crate::Error::new(e)))),
        }
    }
}

// ===== impl Part =====

impl Part {
    /// Get the name of this part.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the filename of this part, if present.
    pub fn filename(&self) -> Option<&str> {
        self.filename.as_ref().map(|s| &**s)
    }

    /// Get the content-type of this part, if present.
    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_ref().map(|s| &**s)
    }

    /// Asynchronously get some of the data for this `Part`.
    pub async fn data(&mut self) -> Option<Result<impl Buf, crate::Error>> {
        self.take_data()
    }

    /// Convert this `Part` into a `Stream` of `Buf`s.
    pub fn stream(self) -> impl Stream<Item = Result<impl Buf, crate::Error>> {
        PartStream(self)
    }

    fn take_data(&mut self) -> Option<Result<Bytes, crate::Error>> {
        self.data.take().map(|vec| Ok(vec.into()))
    }
}

impl fmt::Debug for Part {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut builder = f.debug_struct("Part");
        builder.field("name", &self.name);

        if let Some(ref filename) = self.filename {
            builder.field("filename", filename);
        }

        if let Some(ref mime) = self.content_type {
            builder.field("content_type", mime);
        }

        builder.finish()
    }
}

struct PartStream(Part);

impl Stream for PartStream {
    type Item = Result<Bytes, crate::Error>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.0.take_data())
    }
}

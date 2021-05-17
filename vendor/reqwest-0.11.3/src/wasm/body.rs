#[cfg(feature = "multipart")]
use super::multipart::Form;
/// dox
use bytes::Bytes;
use js_sys::Uint8Array;
use std::fmt;
use wasm_bindgen::JsValue;

/// The body of a `Request`.
///
/// In most cases, this is not needed directly, as the
/// [`RequestBuilder.body`][builder] method uses `Into<Body>`, which allows
/// passing many things (like a string or vector of bytes).
///
/// [builder]: ./struct.RequestBuilder.html#method.body
pub struct Body {
    inner: Inner,
}

enum Inner {
    Bytes(Bytes),
    #[cfg(feature = "multipart")]
    Multipart(Form),
}

impl Body {
    pub(crate) fn to_js_value(&self) -> crate::Result<JsValue> {
        match &self.inner {
            Inner::Bytes(body_bytes) => {
                let body_bytes: &[u8] = body_bytes.as_ref();
                let body_array: Uint8Array = body_bytes.into();
                let js_value: &JsValue = body_array.as_ref();
                Ok(js_value.to_owned())
            }
            #[cfg(feature = "multipart")]
            Inner::Multipart(form) => {
                let form_data = form.to_form_data()?;
                let js_value: &JsValue = form_data.as_ref();
                Ok(js_value.to_owned())
            }
        }
    }

    #[inline]
    #[cfg(feature = "multipart")]
    pub(crate) fn from_form(f: Form) -> Body {
        Self {
            inner: Inner::Multipart(f),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        match &self.inner {
            Inner::Bytes(bytes) => bytes.is_empty(),
            #[cfg(feature = "multipart")]
            Inner::Multipart(form) => form.is_empty(),
        }
    }
}

impl From<Bytes> for Body {
    #[inline]
    fn from(bytes: Bytes) -> Body {
        Body {
            inner: Inner::Bytes(bytes),
        }
    }
}

impl From<Vec<u8>> for Body {
    #[inline]
    fn from(vec: Vec<u8>) -> Body {
        Body {
            inner: Inner::Bytes(vec.into()),
        }
    }
}

impl From<&'static [u8]> for Body {
    #[inline]
    fn from(s: &'static [u8]) -> Body {
        Body {
            inner: Inner::Bytes(Bytes::from_static(s)),
        }
    }
}

impl From<String> for Body {
    #[inline]
    fn from(s: String) -> Body {
        Body {
            inner: Inner::Bytes(s.into()),
        }
    }
}

impl From<&'static str> for Body {
    #[inline]
    fn from(s: &'static str) -> Body {
        s.as_bytes().into()
    }
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Body").finish()
    }
}

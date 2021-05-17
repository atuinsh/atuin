#![allow(unused_imports)]
use super::*;
use wasm_bindgen::prelude::*;
#[wasm_bindgen]
extern "C" {
    # [wasm_bindgen (extends = :: js_sys :: Object , js_name = ReadableStream , typescript_type = "ReadableStream")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[doc = "The `ReadableStream` class."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/ReadableStream)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `ReadableStream`*"]
    pub type ReadableStream;
}

#![allow(unused_imports)]
use super::*;
use wasm_bindgen::prelude::*;
#[wasm_bindgen]
extern "C" {
    # [wasm_bindgen (extends = :: js_sys :: Object , js_name = TextMetrics , typescript_type = "TextMetrics")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[doc = "The `TextMetrics` class."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/TextMetrics)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `TextMetrics`*"]
    pub type TextMetrics;
    # [wasm_bindgen (structural , method , getter , js_class = "TextMetrics" , js_name = width)]
    #[doc = "Getter for the `width` field of this object."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/TextMetrics/width)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `TextMetrics`*"]
    pub fn width(this: &TextMetrics) -> f64;
}

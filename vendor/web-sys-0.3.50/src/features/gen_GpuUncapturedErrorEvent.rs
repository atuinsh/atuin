#![allow(unused_imports)]
use super::*;
use wasm_bindgen::prelude::*;
#[cfg(web_sys_unstable_apis)]
#[wasm_bindgen]
extern "C" {
    # [wasm_bindgen (extends = Event , extends = :: js_sys :: Object , js_name = GPUUncapturedErrorEvent , typescript_type = "GPUUncapturedErrorEvent")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[doc = "The `GpuUncapturedErrorEvent` class."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUUncapturedErrorEvent)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuUncapturedErrorEvent`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub type GpuUncapturedErrorEvent;
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (structural , method , getter , js_class = "GPUUncapturedErrorEvent" , js_name = error)]
    #[doc = "Getter for the `error` field of this object."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUUncapturedErrorEvent/error)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuUncapturedErrorEvent`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn error(this: &GpuUncapturedErrorEvent) -> ::js_sys::Object;
}

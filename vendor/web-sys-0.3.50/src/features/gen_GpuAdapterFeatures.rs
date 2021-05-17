#![allow(unused_imports)]
use super::*;
use wasm_bindgen::prelude::*;
#[cfg(web_sys_unstable_apis)]
#[wasm_bindgen]
extern "C" {
    # [wasm_bindgen (extends = :: js_sys :: Object , js_name = GPUAdapterFeatures , typescript_type = "GPUAdapterFeatures")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[doc = "The `GpuAdapterFeatures` class."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUAdapterFeatures)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuAdapterFeatures`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub type GpuAdapterFeatures;
}

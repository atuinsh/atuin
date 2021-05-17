#![allow(unused_imports)]
use super::*;
use wasm_bindgen::prelude::*;
#[cfg(web_sys_unstable_apis)]
#[wasm_bindgen]
extern "C" {
    # [wasm_bindgen (extends = :: js_sys :: Object , js_name = GPUMapMode , typescript_type = "GPUMapMode")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[doc = "The `GpuMapMode` class."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUMapMode)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuMapMode`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub type GpuMapMode;
}
#[cfg(web_sys_unstable_apis)]
impl GpuMapMode {
    #[cfg(web_sys_unstable_apis)]
    #[doc = "The `GPUMapMode.READ` const."]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuMapMode`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub const READ: u32 = 1u64 as u32;
    #[cfg(web_sys_unstable_apis)]
    #[doc = "The `GPUMapMode.WRITE` const."]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuMapMode`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub const WRITE: u32 = 2u64 as u32;
}

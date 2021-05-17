#![allow(unused_imports)]
use super::*;
use wasm_bindgen::prelude::*;
#[cfg(web_sys_unstable_apis)]
#[wasm_bindgen]
extern "C" {
    # [wasm_bindgen (extends = :: js_sys :: Object , js_name = GPUColorWrite , typescript_type = "GPUColorWrite")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[doc = "The `GpuColorWrite` class."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUColorWrite)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuColorWrite`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub type GpuColorWrite;
}
#[cfg(web_sys_unstable_apis)]
impl GpuColorWrite {
    #[cfg(web_sys_unstable_apis)]
    #[doc = "The `GPUColorWrite.RED` const."]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuColorWrite`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub const RED: u32 = 1u64 as u32;
    #[cfg(web_sys_unstable_apis)]
    #[doc = "The `GPUColorWrite.GREEN` const."]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuColorWrite`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub const GREEN: u32 = 2u64 as u32;
    #[cfg(web_sys_unstable_apis)]
    #[doc = "The `GPUColorWrite.BLUE` const."]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuColorWrite`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub const BLUE: u32 = 4u64 as u32;
    #[cfg(web_sys_unstable_apis)]
    #[doc = "The `GPUColorWrite.ALPHA` const."]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuColorWrite`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub const ALPHA: u32 = 8u64 as u32;
    #[cfg(web_sys_unstable_apis)]
    #[doc = "The `GPUColorWrite.ALL` const."]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuColorWrite`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub const ALL: u32 = 15u64 as u32;
}

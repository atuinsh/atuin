#![allow(unused_imports)]
use super::*;
use wasm_bindgen::prelude::*;
#[cfg(web_sys_unstable_apis)]
#[wasm_bindgen]
extern "C" {
    # [wasm_bindgen (extends = :: js_sys :: Object , js_name = GPUCanvasContext , typescript_type = "GPUCanvasContext")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[doc = "The `GpuCanvasContext` class."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUCanvasContext)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuCanvasContext`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub type GpuCanvasContext;
    #[cfg(web_sys_unstable_apis)]
    #[cfg(all(feature = "GpuSwapChain", feature = "GpuSwapChainDescriptor",))]
    # [wasm_bindgen (method , structural , js_class = "GPUCanvasContext" , js_name = configureSwapChain)]
    #[doc = "The `configureSwapChain()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUCanvasContext/configureSwapChain)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuCanvasContext`, `GpuSwapChain`, `GpuSwapChainDescriptor`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn configure_swap_chain(
        this: &GpuCanvasContext,
        descriptor: &GpuSwapChainDescriptor,
    ) -> GpuSwapChain;
    #[cfg(web_sys_unstable_apis)]
    #[cfg(all(feature = "GpuAdapter", feature = "GpuTextureFormat",))]
    # [wasm_bindgen (method , structural , js_class = "GPUCanvasContext" , js_name = getSwapChainPreferredFormat)]
    #[doc = "The `getSwapChainPreferredFormat()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUCanvasContext/getSwapChainPreferredFormat)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuAdapter`, `GpuCanvasContext`, `GpuTextureFormat`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn get_swap_chain_preferred_format(
        this: &GpuCanvasContext,
        adapter: &GpuAdapter,
    ) -> GpuTextureFormat;
}

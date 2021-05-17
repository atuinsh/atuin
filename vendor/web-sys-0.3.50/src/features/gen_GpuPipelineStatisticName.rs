#![allow(unused_imports)]
use wasm_bindgen::prelude::*;
#[cfg(web_sys_unstable_apis)]
#[wasm_bindgen]
#[doc = "The `GpuPipelineStatisticName` enum."]
#[doc = ""]
#[doc = "*This API requires the following crate features to be activated: `GpuPipelineStatisticName`*"]
#[doc = ""]
#[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
#[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuPipelineStatisticName {
    VertexShaderInvocations = "vertex-shader-invocations",
    ClipperInvocations = "clipper-invocations",
    ClipperPrimitivesOut = "clipper-primitives-out",
    FragmentShaderInvocations = "fragment-shader-invocations",
    ComputeShaderInvocations = "compute-shader-invocations",
}

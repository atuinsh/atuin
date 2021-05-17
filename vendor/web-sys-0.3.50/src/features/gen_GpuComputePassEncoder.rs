#![allow(unused_imports)]
use super::*;
use wasm_bindgen::prelude::*;
#[cfg(web_sys_unstable_apis)]
#[wasm_bindgen]
extern "C" {
    # [wasm_bindgen (extends = :: js_sys :: Object , js_name = GPUComputePassEncoder , typescript_type = "GPUComputePassEncoder")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[doc = "The `GpuComputePassEncoder` class."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub type GpuComputePassEncoder;
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (structural , method , getter , js_class = "GPUComputePassEncoder" , js_name = label)]
    #[doc = "Getter for the `label` field of this object."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/label)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn label(this: &GpuComputePassEncoder) -> Option<String>;
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (structural , method , setter , js_class = "GPUComputePassEncoder" , js_name = label)]
    #[doc = "Setter for the `label` field of this object."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/label)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn set_label(this: &GpuComputePassEncoder, value: Option<&str>);
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuQuerySet")]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = beginPipelineStatisticsQuery)]
    #[doc = "The `beginPipelineStatisticsQuery()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/beginPipelineStatisticsQuery)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuComputePassEncoder`, `GpuQuerySet`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn begin_pipeline_statistics_query(
        this: &GpuComputePassEncoder,
        query_set: &GpuQuerySet,
        query_index: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = dispatch)]
    #[doc = "The `dispatch()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/dispatch)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn dispatch(this: &GpuComputePassEncoder, x: u32);
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = dispatch)]
    #[doc = "The `dispatch()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/dispatch)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn dispatch_with_y(this: &GpuComputePassEncoder, x: u32, y: u32);
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = dispatch)]
    #[doc = "The `dispatch()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/dispatch)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn dispatch_with_y_and_z(this: &GpuComputePassEncoder, x: u32, y: u32, z: u32);
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = dispatchIndirect)]
    #[doc = "The `dispatchIndirect()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/dispatchIndirect)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn dispatch_indirect_with_u32(
        this: &GpuComputePassEncoder,
        indirect_buffer: &GpuBuffer,
        indirect_offset: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = dispatchIndirect)]
    #[doc = "The `dispatchIndirect()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/dispatchIndirect)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn dispatch_indirect_with_f64(
        this: &GpuComputePassEncoder,
        indirect_buffer: &GpuBuffer,
        indirect_offset: f64,
    );
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = endPass)]
    #[doc = "The `endPass()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/endPass)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn end_pass(this: &GpuComputePassEncoder);
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = endPipelineStatisticsQuery)]
    #[doc = "The `endPipelineStatisticsQuery()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/endPipelineStatisticsQuery)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn end_pipeline_statistics_query(this: &GpuComputePassEncoder);
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuComputePipeline")]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = setPipeline)]
    #[doc = "The `setPipeline()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/setPipeline)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuComputePassEncoder`, `GpuComputePipeline`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn set_pipeline(this: &GpuComputePassEncoder, pipeline: &GpuComputePipeline);
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuQuerySet")]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = writeTimestamp)]
    #[doc = "The `writeTimestamp()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/writeTimestamp)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuComputePassEncoder`, `GpuQuerySet`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_timestamp(this: &GpuComputePassEncoder, query_set: &GpuQuerySet, query_index: u32);
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = insertDebugMarker)]
    #[doc = "The `insertDebugMarker()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/insertDebugMarker)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn insert_debug_marker(this: &GpuComputePassEncoder, marker_label: &str);
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = popDebugGroup)]
    #[doc = "The `popDebugGroup()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/popDebugGroup)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn pop_debug_group(this: &GpuComputePassEncoder);
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = pushDebugGroup)]
    #[doc = "The `pushDebugGroup()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/pushDebugGroup)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn push_debug_group(this: &GpuComputePassEncoder, group_label: &str);
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBindGroup")]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = setBindGroup)]
    #[doc = "The `setBindGroup()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/setBindGroup)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBindGroup`, `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn set_bind_group(this: &GpuComputePassEncoder, index: u32, bind_group: &GpuBindGroup);
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBindGroup")]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = setBindGroup)]
    #[doc = "The `setBindGroup()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/setBindGroup)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBindGroup`, `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn set_bind_group_with_u32_sequence(
        this: &GpuComputePassEncoder,
        index: u32,
        bind_group: &GpuBindGroup,
        dynamic_offsets: &::wasm_bindgen::JsValue,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBindGroup")]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = setBindGroup)]
    #[doc = "The `setBindGroup()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/setBindGroup)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBindGroup`, `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn set_bind_group_with_u32_array_and_u32_and_dynamic_offsets_data_length(
        this: &GpuComputePassEncoder,
        index: u32,
        bind_group: &GpuBindGroup,
        dynamic_offsets_data: &[u32],
        dynamic_offsets_data_start: u32,
        dynamic_offsets_data_length: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBindGroup")]
    # [wasm_bindgen (method , structural , js_class = "GPUComputePassEncoder" , js_name = setBindGroup)]
    #[doc = "The `setBindGroup()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUComputePassEncoder/setBindGroup)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBindGroup`, `GpuComputePassEncoder`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn set_bind_group_with_u32_array_and_f64_and_dynamic_offsets_data_length(
        this: &GpuComputePassEncoder,
        index: u32,
        bind_group: &GpuBindGroup,
        dynamic_offsets_data: &[u32],
        dynamic_offsets_data_start: f64,
        dynamic_offsets_data_length: u32,
    );
}

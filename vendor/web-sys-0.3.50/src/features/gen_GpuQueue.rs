#![allow(unused_imports)]
use super::*;
use wasm_bindgen::prelude::*;
#[cfg(web_sys_unstable_apis)]
#[wasm_bindgen]
extern "C" {
    # [wasm_bindgen (extends = :: js_sys :: Object , js_name = GPUQueue , typescript_type = "GPUQueue")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[doc = "The `GpuQueue` class."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub type GpuQueue;
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (structural , method , getter , js_class = "GPUQueue" , js_name = label)]
    #[doc = "Getter for the `label` field of this object."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/label)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn label(this: &GpuQueue) -> Option<String>;
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (structural , method , setter , js_class = "GPUQueue" , js_name = label)]
    #[doc = "Setter for the `label` field of this object."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/label)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn set_label(this: &GpuQueue, value: Option<&str>);
    #[cfg(web_sys_unstable_apis)]
    #[cfg(all(feature = "GpuImageCopyImageBitmap", feature = "GpuImageCopyTexture",))]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = copyImageBitmapToTexture)]
    #[doc = "The `copyImageBitmapToTexture()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/copyImageBitmapToTexture)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuImageCopyImageBitmap`, `GpuImageCopyTexture`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn copy_image_bitmap_to_texture_with_u32_sequence(
        this: &GpuQueue,
        source: &GpuImageCopyImageBitmap,
        destination: &GpuImageCopyTexture,
        copy_size: &::wasm_bindgen::JsValue,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(all(
        feature = "GpuExtent3dDict",
        feature = "GpuImageCopyImageBitmap",
        feature = "GpuImageCopyTexture",
    ))]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = copyImageBitmapToTexture)]
    #[doc = "The `copyImageBitmapToTexture()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/copyImageBitmapToTexture)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuExtent3dDict`, `GpuImageCopyImageBitmap`, `GpuImageCopyTexture`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn copy_image_bitmap_to_texture_with_gpu_extent_3d_dict(
        this: &GpuQueue,
        source: &GpuImageCopyImageBitmap,
        destination: &GpuImageCopyTexture,
        copy_size: &GpuExtent3dDict,
    );
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = onSubmittedWorkDone)]
    #[doc = "The `onSubmittedWorkDone()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/onSubmittedWorkDone)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn on_submitted_work_done(this: &GpuQueue) -> ::js_sys::Promise;
    #[cfg(web_sys_unstable_apis)]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = submit)]
    #[doc = "The `submit()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/submit)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn submit(this: &GpuQueue, command_buffers: &::wasm_bindgen::JsValue);
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_u32_and_buffer_source(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: u32,
        data: &::js_sys::Object,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_f64_and_buffer_source(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: f64,
        data: &::js_sys::Object,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_u32_and_u8_array(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: u32,
        data: &[u8],
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_f64_and_u8_array(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: f64,
        data: &[u8],
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_u32_and_buffer_source_and_u32(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: u32,
        data: &::js_sys::Object,
        data_offset: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_f64_and_buffer_source_and_u32(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: f64,
        data: &::js_sys::Object,
        data_offset: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_u32_and_u8_array_and_u32(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: u32,
        data: &[u8],
        data_offset: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_f64_and_u8_array_and_u32(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: f64,
        data: &[u8],
        data_offset: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_u32_and_buffer_source_and_f64(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: u32,
        data: &::js_sys::Object,
        data_offset: f64,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_f64_and_buffer_source_and_f64(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: f64,
        data: &::js_sys::Object,
        data_offset: f64,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_u32_and_u8_array_and_f64(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: u32,
        data: &[u8],
        data_offset: f64,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_f64_and_u8_array_and_f64(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: f64,
        data: &[u8],
        data_offset: f64,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_u32_and_buffer_source_and_u32_and_u32(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: u32,
        data: &::js_sys::Object,
        data_offset: u32,
        size: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_f64_and_buffer_source_and_u32_and_u32(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: f64,
        data: &::js_sys::Object,
        data_offset: u32,
        size: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_u32_and_u8_array_and_u32_and_u32(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: u32,
        data: &[u8],
        data_offset: u32,
        size: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_f64_and_u8_array_and_u32_and_u32(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: f64,
        data: &[u8],
        data_offset: u32,
        size: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_u32_and_buffer_source_and_f64_and_u32(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: u32,
        data: &::js_sys::Object,
        data_offset: f64,
        size: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_f64_and_buffer_source_and_f64_and_u32(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: f64,
        data: &::js_sys::Object,
        data_offset: f64,
        size: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_u32_and_u8_array_and_f64_and_u32(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: u32,
        data: &[u8],
        data_offset: f64,
        size: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_f64_and_u8_array_and_f64_and_u32(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: f64,
        data: &[u8],
        data_offset: f64,
        size: u32,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_u32_and_buffer_source_and_u32_and_f64(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: u32,
        data: &::js_sys::Object,
        data_offset: u32,
        size: f64,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_f64_and_buffer_source_and_u32_and_f64(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: f64,
        data: &::js_sys::Object,
        data_offset: u32,
        size: f64,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_u32_and_u8_array_and_u32_and_f64(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: u32,
        data: &[u8],
        data_offset: u32,
        size: f64,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_f64_and_u8_array_and_u32_and_f64(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: f64,
        data: &[u8],
        data_offset: u32,
        size: f64,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_u32_and_buffer_source_and_f64_and_f64(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: u32,
        data: &::js_sys::Object,
        data_offset: f64,
        size: f64,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_f64_and_buffer_source_and_f64_and_f64(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: f64,
        data: &::js_sys::Object,
        data_offset: f64,
        size: f64,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_u32_and_u8_array_and_f64_and_f64(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: u32,
        data: &[u8],
        data_offset: f64,
        size: f64,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(feature = "GpuBuffer")]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeBuffer)]
    #[doc = "The `writeBuffer()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeBuffer)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuBuffer`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_buffer_with_f64_and_u8_array_and_f64_and_f64(
        this: &GpuQueue,
        buffer: &GpuBuffer,
        buffer_offset: f64,
        data: &[u8],
        data_offset: f64,
        size: f64,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(all(feature = "GpuImageCopyTexture", feature = "GpuImageDataLayout",))]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeTexture)]
    #[doc = "The `writeTexture()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeTexture)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuImageCopyTexture`, `GpuImageDataLayout`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_texture_with_buffer_source_and_u32_sequence(
        this: &GpuQueue,
        destination: &GpuImageCopyTexture,
        data: &::js_sys::Object,
        data_layout: &GpuImageDataLayout,
        size: &::wasm_bindgen::JsValue,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(all(feature = "GpuImageCopyTexture", feature = "GpuImageDataLayout",))]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeTexture)]
    #[doc = "The `writeTexture()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeTexture)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuImageCopyTexture`, `GpuImageDataLayout`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_texture_with_u8_array_and_u32_sequence(
        this: &GpuQueue,
        destination: &GpuImageCopyTexture,
        data: &[u8],
        data_layout: &GpuImageDataLayout,
        size: &::wasm_bindgen::JsValue,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(all(
        feature = "GpuExtent3dDict",
        feature = "GpuImageCopyTexture",
        feature = "GpuImageDataLayout",
    ))]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeTexture)]
    #[doc = "The `writeTexture()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeTexture)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuExtent3dDict`, `GpuImageCopyTexture`, `GpuImageDataLayout`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_texture_with_buffer_source_and_gpu_extent_3d_dict(
        this: &GpuQueue,
        destination: &GpuImageCopyTexture,
        data: &::js_sys::Object,
        data_layout: &GpuImageDataLayout,
        size: &GpuExtent3dDict,
    );
    #[cfg(web_sys_unstable_apis)]
    #[cfg(all(
        feature = "GpuExtent3dDict",
        feature = "GpuImageCopyTexture",
        feature = "GpuImageDataLayout",
    ))]
    # [wasm_bindgen (method , structural , js_class = "GPUQueue" , js_name = writeTexture)]
    #[doc = "The `writeTexture()` method."]
    #[doc = ""]
    #[doc = "[MDN Documentation](https://developer.mozilla.org/en-US/docs/Web/API/GPUQueue/writeTexture)"]
    #[doc = ""]
    #[doc = "*This API requires the following crate features to be activated: `GpuExtent3dDict`, `GpuImageCopyTexture`, `GpuImageDataLayout`, `GpuQueue`*"]
    #[doc = ""]
    #[doc = "*This API is unstable and requires `--cfg=web_sys_unstable_apis` to be activated, as"]
    #[doc = "[described in the `wasm-bindgen` guide](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)*"]
    pub fn write_texture_with_u8_array_and_gpu_extent_3d_dict(
        this: &GpuQueue,
        destination: &GpuImageCopyTexture,
        data: &[u8],
        data_layout: &GpuImageDataLayout,
        size: &GpuExtent3dDict,
    );
}

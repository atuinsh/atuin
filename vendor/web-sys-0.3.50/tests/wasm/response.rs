use js_sys::{ArrayBuffer, DataView};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;
use web_sys::Response;

#[wasm_bindgen(module = "/tests/wasm/response.js")]
extern "C" {
    fn new_response() -> Response;
}

#[wasm_bindgen_test]
fn test_response_from_js() {
    let response = new_response();
    assert!(!response.ok());
    assert!(!response.redirected());
    assert_eq!(response.status(), 501);
}

#[wasm_bindgen_test]
async fn test_response_from_bytes() {
    let mut bytes: [u8; 3] = [1, 3, 5];
    let response = Response::new_with_opt_u8_array(Some(&mut bytes)).unwrap();
    assert!(response.ok());
    assert_eq!(response.status(), 200);

    let buf_promise = response.array_buffer().unwrap();
    let buf_val = JsFuture::from(buf_promise).await.unwrap();
    assert!(buf_val.is_instance_of::<ArrayBuffer>());
    let array_buf: ArrayBuffer = buf_val.dyn_into().unwrap();
    let data_view = DataView::new(&array_buf, 0, bytes.len());
    for (i, byte) in bytes.iter().enumerate() {
        assert_eq!(&data_view.get_uint8(i), byte);
    }
}

#[wasm_bindgen_test]
async fn test_response_from_other_body() {
    let input = "Hello, world!";
    let response_a = Response::new_with_opt_str(Some(input)).unwrap();
    let body = response_a.body();
    let response_b = Response::new_with_opt_readable_stream(body.as_ref()).unwrap();
    let output = JsFuture::from(response_b.text().unwrap()).await.unwrap();
    assert_eq!(JsValue::from_str(input), output);
}

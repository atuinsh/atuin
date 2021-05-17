use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

#[rustfmt::skip]
#[wasm_bindgen(module = "tests/wasm/futures.js")]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn call_exports() -> Result<JsValue, JsValue>;

    async fn call_promise() -> JsValue;
    #[wasm_bindgen(catch)]
    async fn call_promise_ok() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn call_promise_err() -> Result<JsValue, JsValue>;

    #[wasm_bindgen]
    async fn call_promise_unit();
    #[wasm_bindgen(catch)]
    async fn call_promise_ok_unit() -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    async fn call_promise_err_unit() -> Result<(), JsValue>;
}

#[wasm_bindgen_test]
async fn smoke() {
    call_exports().await.unwrap();
}

#[wasm_bindgen]
pub async fn async_do_nothing() {}

#[wasm_bindgen]
pub async fn async_return_1() -> JsValue {
    1.into()
}

#[wasm_bindgen]
pub async fn async_return_2() -> u32 {
    2
}

#[wasm_bindgen]
pub async fn async_nothing_again() -> Result<(), JsValue> {
    Ok(())
}

#[wasm_bindgen]
pub async fn async_return_3() -> Result<u32, JsValue> {
    Ok(3)
}

#[wasm_bindgen]
pub async fn async_return_4() -> Result<JsValue, JsValue> {
    Ok(4.into())
}

#[wasm_bindgen]
pub struct AsyncCustomReturn {
    pub val: u32,
}

#[wasm_bindgen]
pub async fn async_return_5() -> AsyncCustomReturn {
    AsyncCustomReturn { val: 5 }
}

#[wasm_bindgen]
pub async fn async_return_6() -> Result<AsyncCustomReturn, JsValue> {
    Ok(AsyncCustomReturn { val: 6 })
}

#[wasm_bindgen]
pub async fn async_return_7() -> Result<AsyncCustomReturn, u32> {
    Ok(AsyncCustomReturn { val: 7 })
}

#[wasm_bindgen]
pub async fn async_return_8() -> Result<AsyncCustomReturn, AsyncCustomReturn> {
    Ok(AsyncCustomReturn { val: 8 })
}

#[wasm_bindgen]
pub async fn async_throw() -> Result<(), js_sys::Error> {
    Err(js_sys::Error::new("async message"))
}

#[wasm_bindgen_test]
async fn test_promise() {
    assert_eq!(call_promise().await.as_string(), Some(String::from("ok")))
}

#[wasm_bindgen_test]
async fn test_promise_ok() {
    assert_eq!(
        call_promise_ok().await.map(|j| j.as_string()),
        Ok(Some(String::from("ok")))
    )
}

#[wasm_bindgen_test]
async fn test_promise_err() {
    assert_eq!(
        call_promise_err().await.map_err(|j| j.as_string()),
        Err(Some(String::from("error")))
    )
}

#[wasm_bindgen_test]
async fn test_promise_unit() {
    call_promise_unit().await
}

#[wasm_bindgen_test]
async fn test_promise_ok_unit() {
    call_promise_ok_unit().await.unwrap()
}

#[wasm_bindgen_test]
async fn test_promise_err_unit() {
    assert_eq!(
        call_promise_err_unit().await.map_err(|j| j.as_string()),
        Err::<(), _>(Some(String::from("error")))
    )
}

// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
use crate::Error;

extern crate std;
use std::thread_local;

use js_sys::Uint8Array;
use wasm_bindgen::prelude::*;

// Maximum is 65536 bytes see https://developer.mozilla.org/en-US/docs/Web/API/Crypto/getRandomValues
const BROWSER_CRYPTO_BUFFER_SIZE: usize = 256;

enum RngSource {
    Node(NodeCrypto),
    Browser(BrowserCrypto, Uint8Array),
}

// JsValues are always per-thread, so we initialize RngSource for each thread.
//   See: https://github.com/rustwasm/wasm-bindgen/pull/955
thread_local!(
    static RNG_SOURCE: Result<RngSource, Error> = getrandom_init();
);

pub(crate) fn getrandom_inner(dest: &mut [u8]) -> Result<(), Error> {
    RNG_SOURCE.with(|result| {
        let source = result.as_ref().map_err(|&e| e)?;

        match source {
            RngSource::Node(n) => {
                if n.random_fill_sync(dest).is_err() {
                    return Err(Error::NODE_RANDOM_FILL_SYNC);
                }
            }
            RngSource::Browser(crypto, buf) => {
                // getRandomValues does not work with all types of WASM memory,
                // so we initially write to browser memory to avoid exceptions.
                for chunk in dest.chunks_mut(BROWSER_CRYPTO_BUFFER_SIZE) {
                    // The chunk can be smaller than buf's length, so we call to
                    // JS to create a smaller view of buf without allocation.
                    let sub_buf = buf.subarray(0, chunk.len() as u32);

                    if crypto.get_random_values(&sub_buf).is_err() {
                        return Err(Error::WEB_GET_RANDOM_VALUES);
                    }
                    sub_buf.copy_to(chunk);
                }
            }
        };
        Ok(())
    })
}

fn getrandom_init() -> Result<RngSource, Error> {
    if let Ok(self_) = Global::get_self() {
        // If `self` is defined then we're in a browser somehow (main window
        // or web worker). We get `self.crypto` (called `msCrypto` on IE), so we
        // can call `crypto.getRandomValues`. If `crypto` isn't defined, we
        // assume we're in an older web browser and the OS RNG isn't available.

        let crypto: BrowserCrypto = match (self_.crypto(), self_.ms_crypto()) {
            (crypto, _) if !crypto.is_undefined() => crypto,
            (_, crypto) if !crypto.is_undefined() => crypto,
            _ => return Err(Error::WEB_CRYPTO),
        };

        let buf = Uint8Array::new_with_length(BROWSER_CRYPTO_BUFFER_SIZE as u32);
        return Ok(RngSource::Browser(crypto, buf));
    }

    let crypto = MODULE.require("crypto").map_err(|_| Error::NODE_CRYPTO)?;
    Ok(RngSource::Node(crypto))
}

#[wasm_bindgen]
extern "C" {
    type Global;
    #[wasm_bindgen(getter, catch, static_method_of = Global, js_class = self, js_name = self)]
    fn get_self() -> Result<Self_, JsValue>;

    type Self_;
    #[wasm_bindgen(method, getter, js_name = "msCrypto")]
    fn ms_crypto(me: &Self_) -> BrowserCrypto;
    #[wasm_bindgen(method, getter)]
    fn crypto(me: &Self_) -> BrowserCrypto;

    type BrowserCrypto;
    #[wasm_bindgen(method, js_name = getRandomValues, catch)]
    fn get_random_values(me: &BrowserCrypto, buf: &Uint8Array) -> Result<(), JsValue>;

    #[wasm_bindgen(js_name = module)]
    static MODULE: NodeModule;

    type NodeModule;
    #[wasm_bindgen(method, catch)]
    fn require(this: &NodeModule, s: &str) -> Result<NodeCrypto, JsValue>;

    type NodeCrypto;
    #[wasm_bindgen(method, js_name = randomFillSync, catch)]
    fn random_fill_sync(crypto: &NodeCrypto, buf: &mut [u8]) -> Result<(), JsValue>;
}

// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Implementation for WASM via wasm-bindgen
extern crate std;

use core::cell::RefCell;
use core::mem;
use std::thread_local;

use js_sys::Uint8Array;
// We have to rename wasm_bindgen to bindgen in the Cargo.toml for backwards
// compatibility. We have to rename it back here or else the macros will break.
extern crate bindgen as wasm_bindgen;
use wasm_bindgen::prelude::*;

use crate::error::{BINDGEN_CRYPTO_UNDEF, BINDGEN_GRV_UNDEF};
use crate::Error;

const CHUNK_SIZE: usize = 256;

#[derive(Clone, Debug)]
enum RngSource {
    Node(NodeCrypto),
    Browser(BrowserCrypto, Uint8Array),
}

// JsValues are always per-thread, so we initialize RngSource for each thread.
//   See: https://github.com/rustwasm/wasm-bindgen/pull/955
thread_local!(
    static RNG_SOURCE: RefCell<Option<RngSource>> = RefCell::new(None);
);

pub fn getrandom_inner(dest: &mut [u8]) -> Result<(), Error> {
    assert_eq!(mem::size_of::<usize>(), 4);

    RNG_SOURCE.with(|f| {
        let mut source = f.borrow_mut();
        if source.is_none() {
            *source = Some(getrandom_init()?);
        }

        match source.as_ref().unwrap() {
            RngSource::Node(n) => n.random_fill_sync(dest),
            RngSource::Browser(crypto, buf) => {
                // getRandomValues does not work with all types of WASM memory,
                // so we initially write to browser memory to avoid exceptions.
                for chunk in dest.chunks_mut(CHUNK_SIZE) {
                    // The chunk can be smaller than buf's length, so we call to
                    // JS to create a smaller view of buf without allocation.
                    let sub_buf = buf.subarray(0, chunk.len() as u32);

                    crypto.get_random_values(&sub_buf);
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
            (crypto, _) if !crypto.is_undefined() => crypto.into(),
            (_, crypto) if !crypto.is_undefined() => crypto.into(),
            _ => return Err(BINDGEN_CRYPTO_UNDEF),
        };

        // Test if `crypto.getRandomValues` is undefined as well
        if crypto.get_random_values_fn().is_undefined() {
            return Err(BINDGEN_GRV_UNDEF);
        }

        let buf = Uint8Array::new_with_length(CHUNK_SIZE as u32);
        return Ok(RngSource::Browser(crypto, buf));
    }

    return Ok(RngSource::Node(MODULE.require("crypto")));
}

#[wasm_bindgen]
extern "C" {
    type Global;
    #[wasm_bindgen(getter, catch, static_method_of = Global, js_class = self, js_name = self)]
    fn get_self() -> Result<Self_, JsValue>;

    type Self_;
    #[wasm_bindgen(method, getter, js_name = "msCrypto", structural)]
    fn ms_crypto(me: &Self_) -> JsValue;
    #[wasm_bindgen(method, getter, structural)]
    fn crypto(me: &Self_) -> JsValue;

    #[derive(Clone, Debug)]
    type BrowserCrypto;

    // TODO: these `structural` annotations here ideally wouldn't be here to
    // avoid a JS shim, but for now with feature detection they're
    // unavoidable.
    #[wasm_bindgen(method, js_name = getRandomValues, structural, getter)]
    fn get_random_values_fn(me: &BrowserCrypto) -> JsValue;
    #[wasm_bindgen(method, js_name = getRandomValues, structural)]
    fn get_random_values(me: &BrowserCrypto, buf: &Uint8Array);

    #[derive(Clone, Debug)]
    type NodeCrypto;

    #[wasm_bindgen(method, js_name = randomFillSync, structural)]
    fn random_fill_sync(me: &NodeCrypto, buf: &mut [u8]);

    type NodeModule;

    #[wasm_bindgen(js_name = module)]
    static MODULE: NodeModule;

    #[wasm_bindgen(method)]
    fn require(this: &NodeModule, s: &str) -> NodeCrypto;
}

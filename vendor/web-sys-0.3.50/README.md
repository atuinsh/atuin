# `web-sys`

Raw bindings to Web APIs for projects using `wasm-bindgen`.

* [The `web-sys` section of the `wasm-bindgen`
  guide](https://rustwasm.github.io/wasm-bindgen/web-sys/index.html)
* [API Documentation](https://rustwasm.github.io/wasm-bindgen/api/web_sys/)

## Crate features

This crate by default contains very little when compiled as almost all of its
exposed APIs are gated by Cargo features. The exhaustive list of features can be
found in `crates/web-sys/Cargo.toml`, but the rule of thumb for `web-sys` is
that each type has its own cargo feature (named after the type). Using an API
requires enabling the features for all types used in the API, and APIs should
mention in the documentation what features they require.

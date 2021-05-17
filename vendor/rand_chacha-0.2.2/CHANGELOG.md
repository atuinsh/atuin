# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.2] - 2020-03-09
- Integrate `c2-chacha`, reducing dependency count (#931)
- Add CryptoRng to ChaChaXCore (#944)

## [0.2.1] - 2019-07-22
- Force enable the `simd` feature of `c2-chacha` (#845)

## [0.2.0] - 2019-06-06
- Rewrite based on the much faster `c2-chacha` crate (#789)

## [0.1.1] - 2019-01-04
- Disable `i128` and `u128` if the `target_os` is `emscripten` (#671: work-around Emscripten limitation)
- Update readme and doc links

## [0.1.0] - 2018-10-17
- Pulled out of the Rand crate

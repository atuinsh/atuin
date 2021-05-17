# 0.2.6 (Jul 19, 2020)

* Remove support for using vcpkg for windows (msvc) builds since it was undertested
* Update prebuilt windows binaries to those published at 2020-06-30 at libsodium.org
* Update libsodium submodule to latest stable branch commit. (#420, #400)
* Avoid ptr_cast feature as it was not yet stable in Rust 1.36. (#408)
* Fix build script by using the correct arguments for xcopy (#407)
* Avoid dev dependencies on tar and libflate by including libsodium as a Git submodule. (#393)
* Add bindings to KDF functions (#351)
* Expose libsodium's hexadecimal encoding/decoding helper functions (#390)
* Expose libsodium's Base64 encoding/decoding helper functions (#391)
* remove ill-advised gen_nonce functions (#395)

# 0.2.5 (Oct 11, 2019)

* Fix Digest PartialEq (#381)
* Fix compiler warnings (#380)
* Use copy_from_slice instead of manual loop where possible (#378)

# 0.2.4 (Sep 3, 2019)

* Fixed erronoeus dependency on older libsodium-sys
* Fixed use of deprecated try! macro (#369)

# 0.2.3 (Sep 1, 2019)

* Allow reusing Vec for secretstream (#357)
* Replace mem::uninitialized with MaybeUninit (#350, #356)
* Fix warning about deprecated uint64_t (#358)
* Fix path to ./configure script (#355)
* Add API to derive Ed25519 public keys from secret keys (#345)
* Add DEP_SODIUM_INCLUDE & DEP_SODIUM_LIB env variables (#344)
* Update libsodium to 1.0.18 (#342)

# 0.2.2 (May 16, 2019)

* Bundle libsodium .a .lib for win & cygwin (#332)
* Add `AsRef<[u8]>` for newtypes (#323)
* Implement memory locking and unlocking (#324)
* Add convenience functions for pwhash() and derive_key() (#309)

# 0.2.1 (March 1, 2019)

* Remove statik option from pkg_config usage (#296)
* Fix struct alignments that are not correctly recognized by bindgen (#304)
* Add streaming calculation of ed25519 (#237)
* Update libsodium to 1.0.17 (#306)
* Add support for secretstream (#301)
* Expose randombytes_uniform (#311)
* Use stable rustc to check fmt in TravisCI (#314)
* Reduce deps to build on linux from 61 -> to 48 (#312)
* Add instructions how to compile for armv7-unknown-linux-musleabihf (#294)
* Add use-pkg-config feature (#315)

# 0.2.0 (December 2, 2018)

* Add binding for sodium_add function (#210)
* Add bindings for crypto_generichash (#196)
* Add #[derive(Clone)] to State struct in hash_macros.rs (#228)
* Add bindings for keypair_from_seed for authenticated encryption (#230)
* Instruct OSX users to install pkg-config in README (#243)
* Mark libsodium-sys as a member of workspace (#247)
* Update README w/ Clang + Sodium version (#248)
* Add xchacha20-poly1305 support (#253)
* Fix tests for no_std (#257)
* Removes allow_failures in Travis config (#259)
* Add CONTRIBUTING.md (#261)
* Add CODE_OF_CONDUCT.md (#264)
* Typo fix in docs (#266)
* Fix libsodium linking for MSVC (#265)
* Add Windows support (MSVC) (#269)
* Fix redundant linker flag specified for libsodium (#274)
* Remove gh-pages support in favor of docs.rs (#270)
* Add OSX build in TravisCI (#271)
* Fix build on Win using vcpkg (#276)
* Updated Argon2 Support (#239)
* Option to download / compile libsodium (#279)
* Fix warning about unused macro definition (#282)
* Static bindings - no bindgen (#281)
* Format all code using cargo fmt (#285)
* Happy clippy (#287)
* Add cargo-coveralls to TravisCI (#289)
* Take code format from rust_sodium (#290)

# 0.1.0 (June 6, 2018)

# 0.0.16 (December 4, 2017)

# 0.0.15 (May 25, 2017)

# 0.0.14 (January 26, 2017)

# 0.0.13 (January 5, 2017)

# 0.0.12 (July 10, 2016)

# 0.0.11 (July 10, 2016)

# 0.0.10 (April 4, 2016)

# 0.0.9 (November 5, 2015)

# 0.0.8 (August 26, 2015)

# 0.0.7 (August 26, 2015)

# 0.0.6 (August 10, 2015)

# 0.0.5 (April 28, 2015)

# 0.0.4 (April 4, 2015)

# 0.0.3 (April 4, 2015)

# 0.0.2 (March 26, 2015)

# 0.0.1 (March 25, 2015)

# rand_hc

[![Build Status](https://travis-ci.org/rust-random/rand.svg)](https://travis-ci.org/rust-random/rand)
[![Build Status](https://ci.appveyor.com/api/projects/status/github/rust-random/rand?svg=true)](https://ci.appveyor.com/project/rust-random/rand)
[![Latest version](https://img.shields.io/crates/v/rand_hc.svg)](https://crates.io/crates/rand_hc)
[[![Book](https://img.shields.io/badge/book-master-yellow.svg)](https://rust-random.github.io/book/)
[![API](https://img.shields.io/badge/api-master-yellow.svg)](https://rust-random.github.io/rand/rand_hc)
[![API](https://docs.rs/rand_hc/badge.svg)](https://docs.rs/rand_hc)
[![Minimum rustc version](https://img.shields.io/badge/rustc-1.32+-lightgray.svg)](https://github.com/rust-random/rand#rust-version-requirements)

A cryptographically secure random number generator that uses the HC-128
algorithm.

HC-128 is a stream cipher designed by Hongjun Wu[^1], that we use as an
RNG. It is selected as one of the "stream ciphers suitable for widespread
adoption" by eSTREAM[^2].

Links:

-   [API documentation (master)](https://rust-random.github.io/rand/rand_hc)
-   [API documentation (docs.rs)](https://docs.rs/rand_hc)
-   [Changelog](https://github.com/rust-random/rand/blob/master/rand_hc/CHANGELOG.md)

[rand]: https://crates.io/crates/rand
[^1]: Hongjun Wu (2008). ["The Stream Cipher HC-128"](
      http://www.ecrypt.eu.org/stream/p3ciphers/hc/hc128_p3.pdf).
      *The eSTREAM Finalists*, LNCS 4986, pp. 39–47, Springer-Verlag.

[^2]: [eSTREAM: the ECRYPT Stream Cipher Project](
      http://www.ecrypt.eu.org/stream/)


## Crate Features

`rand_hc` is `no_std` compatible. It does not require any functionality
outside of the `core` lib, thus there are no features to configure.


# License

`rand_hc` is distributed under the terms of both the MIT license and the
Apache License (Version 2.0).

See [LICENSE-APACHE](LICENSE-APACHE) and [LICENSE-MIT](LICENSE-MIT), and
[COPYRIGHT](COPYRIGHT) for details.

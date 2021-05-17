# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2020-12-08
- Bump `rand_core` version to 0.6.0
- Bump MSRV to 1.36 (#1011)
- impl PartialEq+Eq for Hc128Rng and Hc128Core (#979)
- Drop some unsafe code, fixing an unsound internal function (#960)

## [0.2.0] - 2019-06-12
- Bump minor crate version since rand_core bump is a breaking change
- Switch to Edition 2018

## [0.1.1] - 2019-06-06 - yanked
- Bump `rand_core` version
- Adjust usage of `#[inline]`

## [0.1.0] - 2018-10-17
- Pulled out of the Rand crate

#!/bin/bash

set -ex

echo Testing num-traits on rustc ${TRAVIS_RUST_VERSION}

# num-integer should build and test everywhere.
cargo build --verbose
cargo test --verbose

# We have no features to test...

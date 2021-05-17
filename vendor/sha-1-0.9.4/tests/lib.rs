#![no_std]

use digest::dev::{digest_test, one_million_a};
use digest::new_test;

new_test!(sha1_main, "sha1", sha1::Sha1, digest_test);

#[test]
fn sha1_1million_a() {
    let output = include_bytes!("data/one_million_a.bin");
    one_million_a::<sha1::Sha1>(output);
}

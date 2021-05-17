#![feature(test)]

extern crate test;

use bytes::Bytes;
use http::HeaderValue;
use test::Bencher;

static SHORT: &'static [u8] = b"localhost";
static LONG: &'static [u8] = b"Mozilla/5.0 (X11; CrOS x86_64 9592.71.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/60.0.3112.80 Safari/537.36";

#[bench]
fn from_shared_short(b: &mut Bencher) {
    b.bytes = SHORT.len() as u64;
    let bytes = Bytes::from_static(SHORT);
    b.iter(|| {
        HeaderValue::from_maybe_shared(bytes.clone()).unwrap();
    });
}

#[bench]
fn from_shared_long(b: &mut Bencher) {
    b.bytes = LONG.len() as u64;
    let bytes = Bytes::from_static(LONG);
    b.iter(|| {
        HeaderValue::from_maybe_shared(bytes.clone()).unwrap();
    });
}

#[bench]
fn from_shared_unchecked_short(b: &mut Bencher) {
    b.bytes = SHORT.len() as u64;
    let bytes = Bytes::from_static(SHORT);
    b.iter(|| unsafe {
        HeaderValue::from_maybe_shared_unchecked(bytes.clone());
    });
}

#[bench]
fn from_shared_unchecked_long(b: &mut Bencher) {
    b.bytes = LONG.len() as u64;
    let bytes = Bytes::from_static(LONG);
    b.iter(|| unsafe {
        HeaderValue::from_maybe_shared_unchecked(bytes.clone());
    });
}

#![feature(test)]
extern crate test;

use std::io::Write;
use test::Bencher;
use uuid::Uuid;

#[bench]
fn bench_hyphen(b: &mut Bencher) {
    let uuid = Uuid::parse_str("F9168C5E-CEB2-4faa-B6BF-329BF39FA1E4").unwrap();
    b.iter(|| {
        let mut buffer = [0_u8; 36];
        write!(&mut buffer as &mut [_], "{:x}", uuid.to_hyphenated()).unwrap();
        test::black_box(buffer);
    });
}

#[bench]
fn bench_simple(b: &mut Bencher) {
    let uuid = Uuid::parse_str("F9168C5E-CEB2-4faa-B6BF-329BF39FA1E4").unwrap();
    b.iter(|| {
        let mut buffer = [0_u8; 32];
        write!(&mut buffer as &mut [_], "{:x}", uuid.to_simple()).unwrap();
        test::black_box(buffer);
    })
}

#[bench]
fn bench_urn(b: &mut Bencher) {
    let uuid = Uuid::parse_str("F9168C5E-CEB2-4faa-B6BF-329BF39FA1E4").unwrap();
    b.iter(|| {
        let mut buffer = [0_u8; 36 + 9];
        write!(&mut buffer as &mut [_], "{:x}", uuid.to_urn()).unwrap();
        test::black_box(buffer);
    })
}

#[bench]
fn bench_encode_hyphen(b: &mut Bencher) {
    let uuid = Uuid::parse_str("F9168C5E-CEB2-4faa-B6BF-329BF39FA1E4").unwrap();
    b.iter(|| {
        let mut buffer = [0_u8; 36];
        uuid.to_hyphenated().encode_lower(&mut buffer);
        test::black_box(buffer);
    });
}

#[bench]
fn bench_encode_simple(b: &mut Bencher) {
    let uuid = Uuid::parse_str("F9168C5E-CEB2-4faa-B6BF-329BF39FA1E4").unwrap();
    b.iter(|| {
        let mut buffer = [0_u8; 32];
        uuid.to_simple().encode_lower(&mut buffer);
        test::black_box(buffer);
    })
}

#[bench]
fn bench_encode_urn(b: &mut Bencher) {
    let uuid = Uuid::parse_str("F9168C5E-CEB2-4faa-B6BF-329BF39FA1E4").unwrap();
    b.iter(|| {
        let mut buffer = [0_u8; 36 + 9];
        uuid.to_urn().encode_lower(&mut buffer);
        test::black_box(buffer);
    })
}

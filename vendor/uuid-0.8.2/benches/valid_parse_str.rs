#![feature(test)]

extern crate test;

use test::Bencher;
use uuid::Uuid;

#[bench]
fn bench_parse_valid_strings(b: &mut Bencher) {
    b.iter(|| {
        // Valid
        let _ = Uuid::parse_str("00000000000000000000000000000000");
        let _ = Uuid::parse_str("67e55044-10b1-426f-9247-bb680e5fe0c8");
        let _ = Uuid::parse_str("67e55044-10b1-426f-9247-bb680e5fe0c8");
        let _ = Uuid::parse_str("F9168C5E-CEB2-4faa-B6BF-329BF39FA1E4");
        let _ = Uuid::parse_str("67e5504410b1426f9247bb680e5fe0c8");
        let _ = Uuid::parse_str("01020304-1112-2122-3132-414243444546");
        let _ =
            Uuid::parse_str("urn:uuid:67e55044-10b1-426f-9247-bb680e5fe0c8");

        // Nil
        let _ = Uuid::parse_str("00000000000000000000000000000000");
        let _ = Uuid::parse_str("00000000-0000-0000-0000-000000000000");
    });
}

#[bench]
fn bench_valid_hyphenated(b: &mut Bencher) {
    b.iter(|| {
        let _ = Uuid::parse_str("67e55044-10b1-426f-9247-bb680e5fe0c8");
    });
}

#[bench]
fn bench_valid_short(b: &mut Bencher) {
    b.iter(|| {
        let _ = Uuid::parse_str("67e5504410b1426f9247bb680e5fe0c8");
    });
}

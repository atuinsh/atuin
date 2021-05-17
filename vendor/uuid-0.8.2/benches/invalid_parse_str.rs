#![feature(test)]
extern crate test;

use test::Bencher;
use uuid::Uuid;

#[bench]
fn bench_parse_invalid_strings(b: &mut Bencher) {
    b.iter(|| {
        let _ = Uuid::parse_str("");
        let _ = Uuid::parse_str("!");
        let _ = Uuid::parse_str("F9168C5E-CEB2-4faa-B6BF-329BF39FA1E45");
        let _ = Uuid::parse_str("F9168C5E-CEB2-4faa-BBF-329BF39FA1E4");
        let _ = Uuid::parse_str("F9168C5E-CEB2-4faa-BGBF-329BF39FA1E4");
        let _ = Uuid::parse_str("F9168C5E-CEB2-4faa-B6BFF329BF39FA1E4");
        let _ = Uuid::parse_str("F9168C5E-CEB2-4faa");
        let _ = Uuid::parse_str("F9168C5E-CEB2-4faaXB6BFF329BF39FA1E4");
        let _ = Uuid::parse_str("F9168C5E-CEB-24fa-eB6BFF32-BF39FA1E4");
        let _ = Uuid::parse_str("01020304-1112-2122-3132-41424344");
        let _ = Uuid::parse_str("67e5504410b1426f9247bb680e5fe0c88");
        let _ = Uuid::parse_str("67e5504410b1426f9247bb680e5fe0cg8");
        let _ = Uuid::parse_str("67e5504410b1426%9247bb680e5fe0c8");

        // Test error reporting
        let _ = Uuid::parse_str("67e5504410b1426f9247bb680e5fe0c");
        let _ = Uuid::parse_str("67e550X410b1426f9247bb680e5fe0cd");
        let _ = Uuid::parse_str("67e550-4105b1426f9247bb680e5fe0c");
        let _ = Uuid::parse_str("F9168C5E-CEB2-4faa-B6BF1-02BF39FA1E4");
    });
}

#[bench]
fn bench_parse_invalid_len(b: &mut Bencher) {
    b.iter(|| {
        let _ = Uuid::parse_str("F9168C5E-CEB2-4faa-BBF-329BF39FA1E4");
    })
}

#[bench]
fn bench_parse_invalid_character(b: &mut Bencher) {
    b.iter(|| {
        let _ = Uuid::parse_str("F9168C5E-CEB2-4faa-BGBF-329BF39FA1E4");
    })
}

#[bench]
fn bench_parse_invalid_group_len(b: &mut Bencher) {
    b.iter(|| {
        let _ = Uuid::parse_str("01020304-1112-2122-3132-41424344");
    });
}

#[bench]
fn bench_parse_invalid_groups(b: &mut Bencher) {
    b.iter(|| {
        let _ = Uuid::parse_str("F9168C5E-CEB2-4faa-B6BFF329BF39FA1E4");
    });
}

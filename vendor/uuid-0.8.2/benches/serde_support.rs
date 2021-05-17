#![cfg(feature = "serde")]
#![feature(test)]

use bincode;
use serde_json;
extern crate test;

use test::Bencher;
use uuid::Uuid;

#[bench]
fn bench_json_encode(b: &mut Bencher) {
    let uuid = Uuid::parse_str("F9168C5E-CEB2-4faa-B6BF-329BF39FA1E4").unwrap();
    let mut buffer = [0_u8; 38];
    b.iter(|| {
        serde_json::to_writer(&mut buffer as &mut [u8], &uuid).unwrap();
        test::black_box(buffer);
    });
    b.bytes = buffer.len() as u64;
}

#[bench]
fn bench_json_decode(b: &mut Bencher) {
    let s = "\"F9168C5E-CEB2-4faa-B6BF-329BF39FA1E4\"";
    b.iter(|| serde_json::from_str::<Uuid>(s).unwrap());
    b.bytes = s.len() as u64;
}

#[bench]
fn bench_bincode_encode(b: &mut Bencher) {
    let uuid = Uuid::parse_str("F9168C5E-CEB2-4faa-B6BF-329BF39FA1E4").unwrap();
    let mut buffer = [0_u8; 24];
    b.iter(|| {
        bincode::serialize_into(&mut buffer as &mut [u8], &uuid).unwrap();
        test::black_box(buffer);
    });
    b.bytes = buffer.len() as u64;
}

#[bench]
fn bench_bincode_decode(b: &mut Bencher) {
    let bytes = [
        16, 0, 0, 0, 0, 0, 0, 0, 249, 22, 140, 94, 206, 178, 79, 170, 182, 191,
        50, 155, 243, 159, 161, 228,
    ];
    b.iter(|| bincode::deserialize::<Uuid>(&bytes).unwrap());
    b.bytes = bytes.len() as u64;
}

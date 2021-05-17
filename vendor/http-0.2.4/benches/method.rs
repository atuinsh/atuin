#![feature(test)]

extern crate test;

use http::method::Method;
use test::Bencher;

fn make_all_methods() -> Vec<Vec<u8>> {
    vec![
        b"OPTIONS".to_vec(),
        b"GET".to_vec(),
        b"POST".to_vec(),
        b"PUT".to_vec(),
        b"DELETE".to_vec(),
        b"HEAD".to_vec(),
        b"TRACE".to_vec(),
        b"CONNECT".to_vec(),
        b"PATCH".to_vec(),
        b"CUSTOM_SHORT".to_vec(),
        b"CUSTOM_LONG_METHOD".to_vec(),
    ]
}

#[bench]
fn method_easy(b: &mut Bencher) {
    let name = b"GET";
    b.iter(|| {
        Method::from_bytes(&name[..]).unwrap();
    });
}

#[bench]
fn method_various(b: &mut Bencher) {
    let all_methods = make_all_methods();
    b.iter(|| {
        for name in &all_methods {
            Method::from_bytes(name.as_slice()).unwrap();
        }
    });
}

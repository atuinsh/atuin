#![feature(test)]

extern crate test;

use http::Uri;
use test::Bencher;

#[bench]
fn uri_parse_slash(b: &mut Bencher) {
    b.bytes = 1;
    b.iter(|| {
        "/".parse::<Uri>().unwrap();
    });
}

#[bench]
fn uri_parse_relative_medium(b: &mut Bencher) {
    let s = "/wp-content/uploads/2010/03/hello-kitty-darth-vader-pink.jpg";
    b.bytes = s.len() as u64;
    b.iter(|| {
        s.parse::<Uri>().unwrap();
    });
}

#[bench]
fn uri_parse_relative_query(b: &mut Bencher) {
    let s = "/wp-content/uploads/2010/03/hello-kitty-darth-vader-pink.jpg?foo={bar}|baz%13%11quux";
    b.bytes = s.len() as u64;
    b.iter(|| {
        s.parse::<Uri>().unwrap();
    });
}

#![cfg(feature = "collections")]
use bumpalo::{collections::String, format, Bump};
use std::fmt::Write;

#[test]
fn format_a_bunch_of_strings() {
    let b = Bump::new();
    let mut s = String::from_str_in("hello", &b);
    for i in 0..1000 {
        write!(&mut s, " {}", i).unwrap();
    }
}

#[test]
fn trailing_comma_in_format_macro() {
    let b = Bump::new();
    let v = format![in &b, "{}{}", 1, 2, ];
    assert_eq!(v, "12");
}

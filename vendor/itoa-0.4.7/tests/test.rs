#![cfg_attr(feature = "cargo-clippy", allow(cast_lossless, string_lit_as_bytes))]
#![allow(non_snake_case)]

extern crate itoa;

macro_rules! test {
    (
        $(
            $(#[$attr:meta])*
            $name:ident($value:expr, $expected:expr)
        ),*
    ) => {
        $(
            $(#[$attr])*
            #[test]
            fn $name() {
                #[cfg(feature = "std")]
                {
                    let mut buf = [b'\0'; 40];
                    let len = itoa::write(&mut buf[..], $value).unwrap();
                    assert_eq!(&buf[0..len], $expected.as_bytes());
                }

                let mut s = String::new();
                itoa::fmt(&mut s, $value).unwrap();
                assert_eq!(s, $expected);
            }
        )*
    }
}

test! {
    test_u64_0(0u64, "0"),
    test_u64_half(<u32>::max_value() as u64, "4294967295"),
    test_u64_max(<u64>::max_value(), "18446744073709551615"),
    test_i64_min(<i64>::min_value(), "-9223372036854775808"),

    test_i16_0(0i16, "0"),
    test_i16_min(<i16>::min_value(), "-32768"),

    #[cfg(feature = "i128")]
    test_u128_0(0u128, "0"),
    #[cfg(feature = "i128")]
    test_u128_max(<u128>::max_value(), "340282366920938463463374607431768211455"),
    #[cfg(feature = "i128")]
    test_i128_min(<i128>::min_value(), "-170141183460469231731687303715884105728")
}

extern crate rmp_serde as rmps;

use std::io::Cursor;
use std::fmt::{self, Formatter};

use serde::de;
use serde::Deserialize;

use rmp::Marker;
use crate::rmps::{Deserializer, Raw, RawRef};
use crate::rmps::decode::{self, Error};

#[test]
fn pass_nil() {
    let buf = [0xc0];
    let mut de = Deserializer::new(&buf[..]);
    assert_eq!((), Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn fail_nil_from_reserved() {
    let buf = [0xc1];
    let mut de = Deserializer::new(&buf[..]);

    let res: Result<(), Error> = Deserialize::deserialize(&mut de);
    match res.err() {
        Some(Error::TypeMismatch(Marker::Reserved)) => (),
        other => panic!("unexpected result: {:?}", other)
    }
}

#[test]
fn pass_bool() {
    let buf = [0xc3, 0xc2];
    let mut de = Deserializer::new(&buf[..]);

    assert_eq!(true, Deserialize::deserialize(&mut de).unwrap());
    assert_eq!(false, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn fail_bool_from_fixint() {
    let buf = [0x00];
    let cur = Cursor::new(&buf[..]);

    let mut deserializer = Deserializer::new(cur);

    let res: Result<bool, Error> = Deserialize::deserialize(&mut deserializer);
    match res.err().unwrap() {
        Error::Syntax(..) => (),
        other => panic!("unexpected result: {:?}", other)
    }
}

#[test]
fn pass_u64() {
    let buf = [0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(18446744073709551615u64, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn pass_u32() {
    let buf = [0xce, 0xff, 0xff, 0xff, 0xff];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(4294967295u32, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn fail_u32_from_u64() {
    let buf = [0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    let res: Result<u32, Error> = Deserialize::deserialize(&mut de);
    match res.err().unwrap() {
        Error::Syntax(..) => (),
        other => panic!("unexpected result: {:?}", other)
    }
}

#[test]
fn pass_u16() {
    let buf = [0xcd, 0xff, 0xff];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(65535u16, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn pass_u8() {
    let buf = [0xcc, 0xff];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(255u8, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn pass_u8_from_64() {
    let buf = [0xcf, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2a];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(42u8, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn pass_usize() {
    let buf = [0xcc, 0xff];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(255usize, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn pass_i64() {
    let buf = [0xd3, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(9223372036854775807i64, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn pass_i32() {
    let buf = [0xd2, 0x7f, 0xff, 0xff, 0xff];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(2147483647i32, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn pass_i16() {
    let buf = [0xd1, 0x7f, 0xff];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(32767i16, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn pass_i8() {
    let buf = [0xd0, 0x7f];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(127i8, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn pass_isize() {
    let buf = [0xd0, 0x7f];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(127isize, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn pass_f32() {
    let buf = [0xca, 0x7f, 0x7f, 0xff, 0xff];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(3.4028234e38_f32, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn pass_f64() {
    let buf = [0xcb, 0x40, 0x45, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(42f64, Deserialize::deserialize(&mut de).unwrap());
}

// spot check tests for general integers -> float conversions

#[test]
fn pass_i8_as_f32() {
    let buf = [0xd0, 0x7f];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(127f32, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn pass_u32_as_f64() {
    let buf = [0xce, 0xff, 0xff, 0xff, 0xff];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!(4294967295f64, Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn pass_string() {
    let buf = [0xaa, 0x6c, 0x65, 0x20, 0x6d, 0x65, 0x73, 0x73, 0x61, 0x67, 0x65];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);
    let actual: String = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!("le message".to_string(), actual);
}

#[test]
fn pass_tuple() {
    let buf = [0x92, 0x2a, 0xce, 0x0, 0x1, 0x88, 0x94];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);
    let actual: (u32, u32) = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!((42, 100500), actual);
}

#[ignore]
#[test]
fn fail_tuple_len_mismatch() {
    let buf = [0x92, 0x2a, 0xce, 0x0, 0x1, 0x88, 0x94];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);
    let actual: Result<(u32,), Error> = Deserialize::deserialize(&mut de);

    match actual.err().unwrap() {
        Error::LengthMismatch(1) => (),
        other => panic!("unexpected result: {:?}", other)
    }
}

#[test]
fn pass_option_some() {
    let buf = [0x1f];

    let mut de = Deserializer::new(&buf[..]);
    let actual: Option<u8> = Deserialize::deserialize(&mut de).unwrap();
    assert_eq!(Some(31), actual);
}

#[test]
fn pass_option_none() {
    let buf = [0xc0];

    let mut de = Deserializer::new(&buf[..]);
    let actual: Option<u8> = Deserialize::deserialize(&mut de).unwrap();
    assert_eq!(None, actual);
}

#[test]
fn pass_nested_option_some() {
    let buf = [0x1f];

    let mut de = Deserializer::new(&buf[..]);
    let actual: Option<Option<u8>> = Deserialize::deserialize(&mut de).unwrap();
    assert_eq!(Some(Some(31)), actual);
}

#[test]
fn pass_nested_option_none() {
    let buf = [0xc0];

    let mut de = Deserializer::new(&buf[..]);
    let actual: Option<Option<u8>> = Deserialize::deserialize(&mut de).unwrap();
    assert_eq!(None, actual);
}

#[test]
fn fail_option_u8_from_reserved() {
    let buf = [0xc1];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);
    let actual: Result<Option<u8>, Error> = Deserialize::deserialize(&mut de);
    match actual.err() {
        Some(Error::TypeMismatch(Marker::Reserved)) => (),
        other => panic!("unexpected result: {:?}", other)
    }
}

#[test]
fn pass_vector() {
    let buf = [0x92, 0x00, 0xcc, 0x80];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);
    let actual: Vec<u8> = Deserialize::deserialize(&mut de).unwrap();
    assert_eq!(vec![0, 128], actual);
}

#[test]
fn pass_map() {
    use std::collections::HashMap;

    let buf = [
        0x82, // 2 (size)
        0xa3, 0x69, 0x6e, 0x74, // 'int'
        0xcc, 0x80, // 128
        0xa3, 0x6b, 0x65, 0x79, // 'key'
        0x2a // 42
    ];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);
    let actual = Deserialize::deserialize(&mut de).unwrap();
    let mut expected = HashMap::new();
    expected.insert("int".to_string(), 128);
    expected.insert("key".to_string(), 42);

    assert_eq!(expected, actual);
}

// TODO: Merge three of them.
#[test]
fn pass_bin8_into_bytebuf() {
    use serde_bytes::ByteBuf;

    let buf = [0xc4, 0x02, 0xcc, 0x80];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);
    let actual: ByteBuf = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!([0xcc, 0x80], actual[..]);
}

#[test]
fn pass_bin16_into_bytebuf() {
    use serde_bytes::ByteBuf;

    let buf = [0xc5, 0x00, 0x02, 0xcc, 0x80];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);
    let actual: ByteBuf = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!([0xcc, 0x80], actual[..]);
}

#[test]
fn pass_bin32_into_bytebuf() {
    use serde_bytes::ByteBuf;

    let buf = [0xc6, 0x00, 0x00, 0x00, 0x02, 0xcc, 0x80];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);
    let actual: ByteBuf = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!([0xcc, 0x80], actual[..]);
}

#[test]
fn pass_bin8_into_bytebuf_regression_growing_buffer() {
    use serde_bytes::ByteBuf;

    // Try to deserialize large buf and a small buf
    let buf = [0x92, 0xc4, 0x04, 0x71, 0x75, 0x75, 0x78, 0xc4, 0x03, 0x62, 0x61, 0x72];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);
    let (large, small): (ByteBuf, ByteBuf) = Deserialize::deserialize(&mut de).unwrap();
    let (large, small): (Vec<u8>, Vec<u8>) = (large.into_vec(), small.into_vec());

    assert_eq!((b"quux".to_vec(), b"bar".to_vec()), (large, small));
}

#[test]
fn test_deserialize_numeric() {
    #[derive(Debug, PartialEq)]
    enum FloatOrInteger {
        Float(f64),
        Integer(u64),
    }

    impl<'de> de::Deserialize<'de> for FloatOrInteger {
        fn deserialize<D>(de: D) -> Result<FloatOrInteger, D::Error>
            where D: de::Deserializer<'de>
        {
            struct FloatOrIntegerVisitor;

            impl<'de> de::Visitor<'de> for FloatOrIntegerVisitor {
                type Value = FloatOrInteger;

                fn expecting(&self, fmt: &mut Formatter<'_>) ->  Result<(), fmt::Error> {
                    write!(fmt, "either a float or an integer")
                }

                fn visit_u64<E>(self, value: u64) -> Result<FloatOrInteger, E> {
                    Ok(FloatOrInteger::Integer(value))
                }

                fn visit_f64<E>(self, value: f64) -> Result<FloatOrInteger, E> {
                    Ok(FloatOrInteger::Float(value))
                }
            }
            de.deserialize_any(FloatOrIntegerVisitor)
        }
    }

    let buf = [203, 64, 36, 102, 102, 102, 102, 102, 102]; // 10.2
    let mut de = Deserializer::new(&buf[..]);
    let x: FloatOrInteger = Deserialize::deserialize(&mut de).unwrap();
    assert_eq!(x, FloatOrInteger::Float(10.2));

    let buf = [36]; // 36
    let mut de = Deserializer::new(&buf[..]);
    let x: FloatOrInteger = Deserialize::deserialize(&mut de).unwrap();
    assert_eq!(x, FloatOrInteger::Integer(36));
}

#[test]
fn pass_deserializer_get_ref() {
    let buf = [0xc0];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!((), Deserialize::deserialize(&mut de).unwrap());
    assert_eq!(1, de.get_ref().position());
}

#[test]
fn pass_deserializer_get_mut() {
    let buf = [0xc0];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!((), Deserialize::deserialize(&mut de).unwrap());
    de.get_mut().set_position(0);

    assert_eq!((), Deserialize::deserialize(&mut de).unwrap());
}

#[test]
fn pass_deserializer_into_inner() {
    let buf = [0xc0];
    let cur = Cursor::new(&buf[..]);

    let mut de = Deserializer::new(cur);

    assert_eq!((), Deserialize::deserialize(&mut de).unwrap());
    let cur = de.into_inner();

    assert_eq!(1, cur.position());
}

#[test]
fn pass_deserializer_cursor_position() {
    let mut de = Deserializer::new(Cursor::new(vec![0xce, 0xff, 0xff, 0xff, 0xff]));

    assert_eq!(4294967295u32, Deserialize::deserialize(&mut de).unwrap());
    assert_eq!(5, de.position());
}

#[test]
fn pass_from() {
    assert_eq!(2147483647, decode::from_read(&[0xd2, 0x7f, 0xff, 0xff, 0xff][..]).unwrap());
}

#[test]
fn pass_raw_valid_utf8() {
    let buf = vec![0xa3, 0x6b, 0x65, 0x79];
    let raw: Raw = rmps::from_slice(&buf[..]).unwrap();

    assert!(raw.is_str());
    assert_eq!("key", raw.as_str().unwrap());
    assert_eq!([0x6b, 0x65, 0x79], raw.as_bytes());
}

#[test]
fn pass_raw_invalid_utf8() {
    // >>> msgpack.dumps(msgpack.dumps([200, []]))
    // '\xa4\x92\xcc\xc8\x90'
    let buf = vec![0xa4, 0x92, 0xcc, 0xc8, 0x90];
    let raw: Raw = rmps::from_slice(&buf[..]).unwrap();

    assert!(raw.is_err());
    assert_eq!(0, raw.as_err().unwrap().valid_up_to());
    assert_eq!([0x92, 0xcc, 0xc8, 0x90], raw.as_bytes());
}

#[test]
fn pass_raw_ref_valid_utf8() {
    let buf = vec![0xa3, 0x6b, 0x65, 0x79];
    let raw: RawRef<'_> = rmps::from_slice(&buf[..]).unwrap();

    assert!(raw.is_str());
    assert_eq!("key", raw.as_str().unwrap());
    assert_eq!([0x6b, 0x65, 0x79], raw.as_bytes());
}

#[test]
fn pass_raw_ref_invalid_utf8() {
    // >>> msgpack.dumps(msgpack.dumps([200, []]))
    // '\xa4\x92\xcc\xc8\x90'
    let buf = vec![0xa4, 0x92, 0xcc, 0xc8, 0x90];
    let raw: RawRef<'_> = rmps::from_slice(&buf[..]).unwrap();

    assert!(raw.is_err());
    assert_eq!(0, raw.as_err().unwrap().valid_up_to());
    assert_eq!([0x92, 0xcc, 0xc8, 0x90], raw.as_bytes());
}

#[test]
fn fail_str_invalid_utf8() {
    let buf = vec![0xa4, 0x92, 0xcc, 0xc8, 0x90];
    let err: Result<String, decode::Error> = rmps::from_slice(&buf[..]);

    assert!(err.is_err());
    match err.err().unwrap() {
        decode::Error::Utf8Error(err) => assert_eq!(0, err.valid_up_to()),
        // decode::Error::Syntax(err) => {}
        err => panic!("unexpected error: {:?}", err),
    }
}

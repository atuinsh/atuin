extern crate rmp_serde as rmps;

use std::io::Cursor;

use serde::Serialize;

use crate::rmps::{Raw, RawRef, Serializer};
use crate::rmps::encode::{self, Error};

#[test]
fn pass_null() {
    let mut buf = [0x00];

    let val = ();
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xc0], buf);
}

#[test]
fn fail_null() {
    let mut buf = [];

    let val = ();

    match val.serialize(&mut Serializer::new(&mut &mut buf[..])) {
        Err(Error::InvalidValueWrite(..)) => (),
        other => panic!("unexpected result: {:?}", other)
    }
}

#[test]
fn pass_bool() {
    let mut buf = [0x00, 0x00];

    {
        let mut cur = Cursor::new(&mut buf[..]);

        let mut encoder = Serializer::new(&mut cur);

        let val = true;
        val.serialize(&mut encoder).ok().unwrap();
        let val = false;
        val.serialize(&mut encoder).ok().unwrap();
    }

    assert_eq!([0xc3, 0xc2], buf);
}

#[test]
fn pass_usize() {
    let mut buf = [0x00, 0x00];

    let val = 255usize;
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xcc, 0xff], buf);
}

#[test]
fn pass_u8() {
    let mut buf = [0x00, 0x00];

    let val = 255u8;
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xcc, 0xff], buf);
}

#[test]
fn pass_u16() {
    let mut buf = [0x00, 0x00, 0x00];

    let val = 65535u16;
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xcd, 0xff, 0xff], buf);
}

#[test]
fn pass_u32() {
    let mut buf = [0x00, 0x00, 0x00, 0x00, 0x00];

    let val = 4294967295u32;
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xce, 0xff, 0xff, 0xff, 0xff], buf);
}

#[test]
fn pass_u64() {
    let mut buf = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

    let val = 18446744073709551615u64;
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff], buf);
}

#[test]
fn pass_isize() {
    let mut buf = [0x00, 0x00];

    let val = -128isize;
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xd0, 0x80], buf);
}

#[test]
fn pass_i8() {
    let mut buf = [0x00, 0x00];

    let val = -128i8;
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xd0, 0x80], buf);
}

#[test]
fn pass_i16() {
    let mut buf = [0x00, 0x00, 0x00];

    let val = -32768i16;
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xd1, 0x80, 0x00], buf);
}

#[test]
fn pass_i32() {
    let mut buf = [0x00, 0x00, 0x00, 0x00, 0x00];

    let val = -2147483648i32;
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xd2, 0x80, 0x00, 0x00, 0x00], buf);
}

#[test]
fn pass_i64() {
    let mut buf = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

    let val = -9223372036854775808i64;
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xd3, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], buf);
}

#[test]
fn pass_i64_most_effective() {
    let mut buf = [0x00, 0x00];

    // This value can be represented using 2 bytes although it's i64.
    let val = 128i64;
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).unwrap();

    assert_eq!([0xcc, 0x80], buf);
}


#[test]
fn pass_f32() {
    let mut buf = [0x00, 0x00, 0x00, 0x00, 0x00];

    let val = 3.4028234e38_f32;
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xca, 0x7f, 0x7f, 0xff, 0xff], buf);
}

#[test]
fn pass_f64() {
    let mut buf = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

    let val = 42f64;
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xcb, 0x40, 0x45, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], buf);
}

#[test]
fn pass_char() {
    let mut buf = [0x00, 0x00];

    let val = '!';
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xa1, 0x21], buf);
}


#[test]
fn pass_string() {
    let mut buf = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

    let val = "le message";
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xaa, 0x6c, 0x65, 0x20, 0x6d, 0x65, 0x73, 0x73, 0x61, 0x67, 0x65], buf);
}

#[test]
fn pass_tuple() {
    let mut buf = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

    let val = (42u32, 100500u32);
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0x92, 0x2a, 0xce, 0x0, 0x1, 0x88, 0x94], buf);
}

#[test]
fn pass_option_some() {
    let mut buf = [0x00];

    let val = Some(100u32);
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0x64], buf);
}

#[test]
fn pass_option_none() {
    let mut buf = [0x00];

    let val: Option<u32> = None;
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0xc0], buf);
}

#[test]
fn pass_seq() {
    let mut buf = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

    let val = vec!["le", "shit"];
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    assert_eq!([0x92, 0xa2, 0x6c, 0x65, 0xa4, 0x73, 0x68, 0x69, 0x74], buf);
}

#[test]
fn pass_map() {
    use std::collections::BTreeMap;

    let mut buf = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

    let mut val = BTreeMap::new();
    val.insert(0u8, "le");
    val.insert(1u8, "shit");
    val.serialize(&mut Serializer::new(&mut &mut buf[..])).ok().unwrap();

    let out = [
        0x82, // 2 (size)
        0x00, // 0
        0xa2, 0x6c, 0x65, // "le"
        0x01, // 1
        0xa4, 0x73, 0x68, 0x69, 0x74, // "shit"
    ];
    assert_eq!(out, buf);
}

#[test]
fn pass_empty_map() {
    use std::collections::BTreeMap;

    let mut buf = vec![];

    let val: BTreeMap<u64, u64> = BTreeMap::new();
    val.serialize(&mut Serializer::new(&mut buf)).ok().unwrap();

    let out = vec![
        0x80, // (size: 0)
    ];
    assert_eq!(out, buf);
}

#[test]
fn pass_encoding_struct_into_vec() {
    let val = (42u8, "the Answer");

    let mut buf: Vec<u8> = Vec::new();

    val.serialize(&mut Serializer::new(&mut buf)).unwrap();

    assert_eq!(vec![0x92, 0x2a, 0xaa, 0x74, 0x68, 0x65, 0x20, 0x41, 0x6e, 0x73, 0x77, 0x65, 0x72], buf);
}

#[test]
fn pass_bin() {
    use serde_bytes::Bytes;

    let mut buf = Vec::new();
    let val = Bytes::new(&[0xcc, 0x80]);

    val.serialize(&mut Serializer::new(&mut buf)).ok().unwrap();

    assert_eq!(vec![0xc4, 0x02, 0xcc, 0x80], buf);
}

#[test]
fn pass_to_vec() {
    assert_eq!(vec![0xc0], encode::to_vec(&()).unwrap());
    assert_eq!(vec![0xaa, 0x6c, 0x65, 0x20, 0x6d, 0x65, 0x73, 0x73, 0x61, 0x67, 0x65],
        encode::to_vec("le message").unwrap());
}

#[test]
fn get_mut() {
    let mut se = Serializer::new(Vec::new());
    true.serialize(&mut se).unwrap();

    assert_eq!(&vec![0xc3], se.get_ref());

    se.get_mut().push(42);
    assert_eq!(vec![0xc3, 42], se.into_inner());
}

#[test]
fn pass_raw_valid_utf8() {
    let raw = Raw::new("key".into());

    let mut buf = Vec::new();
    raw.serialize(&mut Serializer::new(&mut buf)).unwrap();

    assert_eq!(vec![0xa3, 0x6b, 0x65, 0x79], buf);
}

#[test]
fn pass_raw_invalid_utf8() {
    // >>> msgpack.dumps(msgpack.dumps([200, []]))
    // '\xa4\x92\xcc\xc8\x90'
    let raw = Raw::from_utf8(vec![0x92, 0xcc, 0xc8, 0x90]);

    let mut buf = Vec::new();
    raw.serialize(&mut Serializer::new(&mut buf)).unwrap();

    assert_eq!(vec![0xa4, 0x92, 0xcc, 0xc8, 0x90], buf);
}

#[test]
fn pass_raw_ref_valid_utf8() {
    let raw = RawRef::new("key");

    let mut buf = Vec::new();
    raw.serialize(&mut Serializer::new(&mut buf)).unwrap();

    assert_eq!(vec![0xa3, 0x6b, 0x65, 0x79], buf);
}

#[test]
fn pass_raw_ref_invalid_utf8() {
    // >>> msgpack.dumps(msgpack.dumps([200, []]))
    // '\xa4\x92\xcc\xc8\x90'
    let b = &[0x92, 0xcc, 0xc8, 0x90];
    let raw = RawRef::from_utf8(b);

    let mut buf = Vec::new();
    raw.serialize(&mut Serializer::new(&mut buf)).unwrap();

    assert_eq!(vec![0xa4, 0x92, 0xcc, 0xc8, 0x90], buf);
}

#[test]
fn serializer_one_type_arg() {
    let _s: rmp_serde::Serializer<&mut dyn std::io::Write>;
}

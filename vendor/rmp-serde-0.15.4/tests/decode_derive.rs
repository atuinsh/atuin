#[macro_use]
extern crate serde_derive;

extern crate rmp_serde as rmps;

use std::io::Cursor;

use serde::Deserialize;

use crate::rmps::Deserializer;
use crate::rmps::decode::Error;

#[test]
fn pass_newtype() {
    let buf = [0x2a];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    struct Struct(u32);

    let mut de = Deserializer::new(cur);
    let actual: Struct = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!(Struct(42), actual);
}

#[test]
fn pass_tuple_struct() {
    let buf = [0x92, 0x2a, 0xce, 0x0, 0x1, 0x88, 0x94];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    struct Decoded(u32, u32);

    let mut de = Deserializer::new(cur);
    let actual: Decoded = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!(Decoded(42, 100500), actual);
}

#[test]
fn pass_single_field_struct() {
    let buf = [0x91, 0x2a];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    struct Struct {
        inner: u32
    };

    let mut de = Deserializer::new(cur);
    let actual: Struct = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!(Struct{inner: 42}, actual);
}

#[test]
fn pass_struct() {
    let buf = [0x92, 0x2a, 0xce, 0x0, 0x1, 0x88, 0x94];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    struct Decoded {
        id: u32,
        value: u32
    };

    let mut de = Deserializer::new(cur);
    let actual: Decoded = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!(Decoded { id: 42, value: 100500 }, actual);
}

#[test]
fn pass_struct_from_map() {
    #[derive(Debug, PartialEq, Deserialize)]
    struct Struct {
        et: String,
        le: u8,
        shit: u8,
    }

    let buf = [
        0x83, // 3 (size)
        0xa2, 0x65, 0x74, // "et"
        0xa5, 0x76, 0x6f, 0x69, 0x6c, 0x61, // "voila"
        0xa2, 0x6c, 0x65, // "le"
        0x00, // 0
        0xa4, 0x73, 0x68, 0x69, 0x74, // "shit"
        0x01, // 1
    ];
    let cur = Cursor::new(&buf[..]);

    // It appears no special behavior is needed for deserializing structs encoded as maps.
    let mut de = Deserializer::new(cur);
    let actual: Struct = Deserialize::deserialize(&mut de).unwrap();
    let expected = Struct { et: "voila".into(), le: 0, shit: 1 };

    assert_eq!(expected, actual);
}

#[test]
fn pass_unit_variant() {
    // We expect enums to be encoded as a map {variant_idx => nil}

    let buf = [0x81, 0x0, 0xC0, 0x81, 0x1, 0xC0];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    enum Enum {
        A,
        B,
    }

    let mut de = Deserializer::new(cur);
    let enum_a = Enum::deserialize(&mut de).unwrap();
    let enum_b = Enum::deserialize(&mut de).unwrap();

    assert_eq!(enum_a , Enum::A);
    assert_eq!(enum_b , Enum::B);
    assert_eq!(6, de.get_ref().position());
}

#[test]
fn pass_tuple_enum_with_arg() {
    // The encoded byte-array is: {1 => 42}.
    let buf = [0x81, 0x01, 0x2a];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    enum Enum {
        A,
        B(u32),
    }

    let mut de = Deserializer::new(cur);
    let actual: Enum = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!(Enum::B(42), actual);
    assert_eq!(3, de.get_ref().position())
}

#[test]
fn pass_tuple_enum_with_args() {
    // The encoded bytearray is: {1 => [42, 58]}.
    let buf = [0x81, 0x01, 0x92, 0x2a, 0x3a];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    enum Enum {
        A,
        B(u32, u32),
    }

    let mut de = Deserializer::new(cur);
    let actual: Enum = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!(Enum::B(42, 58), actual);
    assert_eq!(5, de.get_ref().position())
}

#[test]
fn fail_enum_map_mismatch() {
    let buf = [0x82, 0x0, 0x24, 0x1, 0x25];

    #[derive(Debug, PartialEq, Deserialize)]
    enum Enum {
        A(i32),
    }

    let err: Result<Enum, _> = rmps::from_slice(&buf);

    match err.unwrap_err() {
        Error::LengthMismatch(2) => (),
        other => panic!("unexpected result: {:?}", other)
    }
}

#[test]
fn fail_enum_overflow() {
    // The encoded bytearray is: {1 => [42]}.
    let buf = [0x81, 0x01, 0x2a];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    // TODO: Rename to Enum: A, B, C, ...
    enum Enum {
        A,
    }

    let mut de = Deserializer::new(cur);
    let actual: Result<Enum, Error> = Deserialize::deserialize(&mut de);

    match actual.err().unwrap() {
        Error::Syntax(..) => (),
        other => panic!("unexpected result: {:?}", other)
    }
}

#[test]
fn pass_struct_enum_with_arg() {
    // The encoded bytearray is: {1 => [42]}.
    let buf = [0x81, 0x01, 0x91, 0x2a];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    enum Enum {
        A,
        B { id: u32 },
    }

    let mut de = Deserializer::new(cur);
    let actual: Enum = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!(Enum::B { id: 42 }, actual);
    assert_eq!(4, de.get_ref().position())
}

#[test]
fn pass_newtype_variant() {
    // The encoded bytearray is: {0 => 'le message'}.
    let buf = [0x81, 0x0, 0xaa, 0x6c, 0x65, 0x20, 0x6d, 0x65, 0x73, 0x73, 0x61, 0x67, 0x65];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    struct Newtype(String);

    #[derive(Debug, PartialEq, Deserialize)]
    enum Enum {
        A(Newtype),
    }

    let mut de = Deserializer::new(cur);
    let actual: Enum = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!(Enum::A(Newtype("le message".into())), actual);
    assert_eq!(buf.len() as u64, de.get_ref().position())
}

#[cfg(disabled)]  // This test doesn't actually compile anymore
#[test]
fn pass_enum_custom_policy() {
    use std::io::Read;
    use rmp_serde::decode::VariantVisitor;

    // We expect enums to be endoded as id, [...] (without wrapping tuple).

    let buf = [0x01, 0x90];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    enum Enum {
        A,
        B,
    }

    struct CustomDeserializer<R: Read> {
        inner: Deserializer<R>,
    }

    impl<R: Read> serde::Deserializer for CustomDeserializer<R> {
        type Error = Error;

        fn deserialize<V>(&mut self, visitor: V) -> Result<V::Value, Error>
            where V: serde::de::Visitor
        {
            self.inner.deserialize(visitor)
        }

        fn deserialize_enum<V>(&mut self, _enum: &str, _variants: &'static [&'static str], mut visitor: V)
            -> Result<V::Value, Error>
            where V: serde::de::EnumVisitor
        {
            visitor.visit(VariantVisitor::new(&mut self.inner))
        }

        forward_to_deserialize! {
            bool usize u8 u16 u32 u64 isize i8 i16 i32 i64 f32 f64 char str string unit seq
            seq_fixed_size bytes map tuple_struct unit_struct struct struct_field
            tuple option newtype_struct ignored_any
        }
    }

    let mut de = CustomDeserializer { inner: Deserializer::new(cur) };
    let actual: Enum = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!(Enum::B, actual);
    assert_eq!(2, de.inner.get_ref().position());
}

#[test]
fn pass_struct_variant() {
    #[derive(Debug, PartialEq, Deserialize)]
    enum Custom {
        First { data: u32 },
        Second { data: u32 },
    }
    let out_first = vec![0x81, 0x00, 0x91, 0x2a];
    let out_second = vec![0x81, 0x01, 0x91, 0x2a];

    for (expected, out) in vec![(Custom::First{ data: 42 }, out_first), (Custom::Second { data: 42 }, out_second)] {
        let mut de = Deserializer::new(Cursor::new(&out[..]));
        let val: Custom = Deserialize::deserialize(&mut de).unwrap();
        assert_eq!(expected, val);
    }
}

#[test]
fn pass_adjacently_tagged_enum() {
    // ["Foo", 123]
    let buf = [146, 163, 70, 111, 111, 123];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(tag = "t", content = "c")]
    enum Enum {
        Foo(i32),
        Bar(u32),
    }

    let mut de = Deserializer::new(cur);
    let actual = Deserialize::deserialize(&mut de);

    assert!(actual.is_ok());
    assert_eq!(Enum::Foo(123), actual.unwrap());
}

#[test]
#[should_panic(expected = "assertion failed")]
fn fail_internally_tagged_enum_tuple() {
    // ["Foo", 123]
    let buf = [146, 163, 70, 111, 111, 123];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(tag = "t")]
    enum Enum {
        Foo(i32),
        Bar(u32),
    }

    let mut de = Deserializer::new(cur);
    let actual: Result<Enum, Error> = Deserialize::deserialize(&mut de);

    assert!(actual.is_ok())
}

#[test]
fn pass_internally_tagged_enum_struct() {
    let buf = [130, 161, 116, 163, 70, 111, 111, 165, 118, 97, 108, 117, 101, 123];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(tag = "t")]
    enum Enum {
        Foo { value: i32 },
        Bar { value: u32 },
    }

    let mut de = Deserializer::new(cur);
    let actual: Result<Enum, Error> = Deserialize::deserialize(&mut de);

    assert!(actual.is_ok());
    assert_eq!(Enum::Foo{ value: 123 }, actual.unwrap())

}

#[test]
fn pass_enum_with_one_arg() {
    // The encoded bytearray is: {0 => [1, 2]}.
    let buf = [0x81, 0x0, 0x92, 0x01, 0x02];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    enum Enum {
        V1(Vec<u32>),
    }

    let mut de = Deserializer::new(cur);
    let actual: Enum = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!(Enum::V1(vec![1, 2]), actual);
    assert_eq!(buf.len() as u64, de.get_ref().position())
}

#[test]
fn pass_struct_with_nested_options() {
    // The encoded bytearray is: [null, 13].
    let buf = [0x92, 0xc0, 0x0D];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    struct Struct {
        f1: Option<Option<u32>>,
        f2: Option<Option<u32>>,
    }

    let mut de = Deserializer::new(cur);
    let actual: Struct = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!(Struct { f1: None, f2: Some(Some(13)) }, actual);
    assert_eq!(buf.len() as u64, de.get_ref().position());
}

#[test]
fn pass_struct_with_flattened_map_field() {
    use std::collections::BTreeMap;

    // The encoded bytearray is: { "f1": 0, "f2": { "german": "Hallo Welt!" }, "english": "Hello World!" }.
    let buf = [
        0x83, 0xA2, 0x66, 0x31, 0x00, 0xA2, 0x66, 0x32, 0x81, 0xA6, 0x67, 0x65, 0x72, 0x6D, 0x61, 0x6E, 0xAB,
        0x48, 0x61, 0x6C, 0x6C, 0x6F, 0x20, 0x57, 0x65, 0x6C, 0x74, 0x21, 0xA7, 0x65, 0x6E, 0x67, 0x6C, 0x69,
        0x73, 0x68, 0xAC, 0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x20, 0x57, 0x6F, 0x72, 0x6C, 0x64, 0x21,
    ];
    let cur = Cursor::new(&buf[..]);

    #[derive(Debug, PartialEq, Deserialize)]
    struct Struct {
        f1: u32,
        // not flattend!
        f2: BTreeMap<String, String>,
        #[serde(flatten)]
        f3: BTreeMap<String, String>
    }

    let expected = Struct {
        f1: 0,
        f2: {
            let mut map = BTreeMap::new();
            map.insert("german".to_string(), "Hallo Welt!".to_string());
            map
        },
        f3: {
            let mut map = BTreeMap::new();
            map.insert("english".to_string(), "Hello World!".to_string());
            map
        }
    };

    let mut de = Deserializer::new(cur);
    let actual: Struct = Deserialize::deserialize(&mut de).unwrap();

    assert_eq!(expected, actual);
    assert_eq!(buf.len() as u64, de.get_ref().position());
}

#[test]
fn pass_struct_with_flattened_struct_field() {
    #[derive(Debug, PartialEq, Deserialize)]
    struct Struct {
        f1: u32,
        // not flattend!
        f2: InnerStruct,
        #[serde(flatten)]
        f3: InnerStruct
    }

    #[derive(Debug, PartialEq, Deserialize)]
    struct InnerStruct {
        f4: u32,
        f5: u32
    }

    let expected = Struct {
        f1: 0,
        f2: InnerStruct {
            f4: 8,
            f5: 13
        },
        f3: InnerStruct {
            f4: 21,
            f5: 34
        }
    };

    // struct-as-tuple
    {
        // The encoded bytearray is: { "f1": 0, "f2": [8, 13], "f4": 21, "f5": 34 }.
        let buf = [
            0x84, 0xA2, 0x66, 0x31, 0x00, 0xA2, 0x66, 0x32, 0x92, 0x08, 0x0D, 0xA2, 0x66, 0x34, 0x15, 0xA2, 0x66, 0x35,
            0x22,
        ];
        let cur = Cursor::new(&buf[..]);

        let mut de = Deserializer::new(cur);
        let actual: Struct = Deserialize::deserialize(&mut de).unwrap();

        assert_eq!(expected, actual);
        assert_eq!(buf.len() as u64, de.get_ref().position());
    }

    // struct-as-map
    {
        // The encoded bytearray is: { "f1": 0, "f2": { "f4": 8, "f5": 13 }, "f4": 21, "f5": 34 }.
        let buf = [
            0x84, 0xA2, 0x66, 0x31, 0x00, 0xA2, 0x66, 0x32, 0x82, 0xA2, 0x66, 0x34, 0x08,
            0xA2, 0x66, 0x35, 0x0D, 0xA2, 0x66, 0x34, 0x15, 0xA2, 0x66, 0x35, 0x22,
        ];
        let cur = Cursor::new(&buf[..]);

        let mut de = Deserializer::new(cur);
        let actual: Struct = Deserialize::deserialize(&mut de).unwrap();

        assert_eq!(expected, actual);
        assert_eq!(buf.len() as u64, de.get_ref().position());
    }
}


#[test]
fn pass_from_slice() {
    let buf = [0x93, 0xa4, 0x4a, 0x6f, 0x68, 0x6e, 0xa5, 0x53, 0x6d, 0x69, 0x74, 0x68, 0x2a];

    #[derive(Debug, PartialEq, Deserialize)]
    struct Person<'a> {
        name: &'a str,
        surname: &'a str,
        age: u8,
    }

    assert_eq!(Person { name: "John", surname: "Smith", age: 42 }, rmps::from_slice(&buf[..]).unwrap());
}

#[test]
fn pass_from_ref() {
    let buf = [0x92, 0xa5, 0x42, 0x6f, 0x62, 0x62, 0x79, 0x8];

    #[derive(Debug, Deserialize, PartialEq)]
    struct Dog<'a> {
        name: &'a str,
        age: u8,
    }

    assert_eq!(Dog { name: "Bobby", age: 8 }, rmps::from_read_ref(&buf).unwrap());
}

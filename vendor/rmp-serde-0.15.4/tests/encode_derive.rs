#[macro_use]
extern crate serde_derive;

extern crate rmp_serde as rmps;

use serde::Serialize;
use crate::rmps::Serializer;

#[test]
fn pass_unit_struct() {
    #[derive(Serialize)]
    struct Unit;

    let mut buf = Vec::new();
    Unit.serialize(&mut Serializer::new(&mut buf)).unwrap();

    // Expect: [].
    assert_eq!(vec![0x90], buf);
}

#[test]
fn pass_unit_variant() {
    #[derive(Serialize)]
    enum Enum {
        V1,
        V2,
    }

    let mut buf = Vec::new();
    Enum::V1.serialize(&mut Serializer::new(&mut buf)).unwrap();
    Enum::V2.serialize(&mut Serializer::new(&mut buf)).unwrap();

    // Expect: {0 => nil} {1 => nil}.
    assert_eq!(vec![0x81, 0x00, 0xC0, 0x81, 0x01, 0xC0], buf);
}

#[test]
fn pass_newtype_struct() {
    #[derive(Serialize)]
    struct Struct(u64);

    let val = Struct(42);
    let mut buf = Vec::new();
    val.serialize(&mut Serializer::new(&mut buf)).unwrap();

    assert_eq!(vec![0x2a], buf);
}

#[test]
fn pass_newtype_variant() {
    #[derive(Serialize)]
    enum Enum {
        V2(u64),
    }

    let mut buf = Vec::new();
    Enum::V2(42).serialize(&mut Serializer::new(&mut buf)).unwrap();

    // Expect: {0 => 42}.
    assert_eq!(vec![0x81, 0x00, 0x2a], buf);
}

#[test]
fn pass_untagged_newtype_variant() {
    #[derive(Serialize)]
    #[serde(untagged)]
    enum Enum1 {
        A(u64),
        B(Enum2),
    }

    #[derive(Serialize)]
    enum Enum2 {
        C,
    }

    let buf1 = rmps::to_vec(&Enum1::A(123)).unwrap();
    let buf2 = rmps::to_vec(&Enum1::B(Enum2::C)).unwrap();

    assert_eq!(buf1, [123]);
    assert_eq!(buf2, [0x81, 0x0, 0xC0]);
}

#[test]
fn pass_tuple_struct() {
    #[derive(Serialize)]
    struct Struct(u32, u64);

    let val = Struct(42, 100500);
    let mut buf = Vec::new();
    val.serialize(&mut Serializer::new(&mut buf)).unwrap();

    // Expect: [42, 100500].
    assert_eq!(vec![0x92, 0x2a, 0xce, 0x00, 0x01, 0x88, 0x94], buf);
}

#[test]
fn pass_tuple_variant() {
    #[derive(Serialize)]
    enum Enum {
        V1,
        V2(u32, u64),
    }

    let mut buf = Vec::new();
    Enum::V1.serialize(&mut Serializer::new(&mut buf)).unwrap();
    Enum::V2(42, 100500).serialize(&mut Serializer::new(&mut buf)).unwrap();

    // Expect: {0 => nil} {1 => [42, 100500]}
    assert_eq!(vec![0x81, 0x00, 0xC0, 0x81, 0x01, 0x92, 0x2a, 0xce, 0x00, 0x01, 0x88, 0x94], buf);
}

#[test]
fn pass_struct() {
    #[derive(Serialize)]
    struct Struct {
        f1: u32,
        f2: u32,
    }

    let val = Struct {
        f1: 42,
        f2: 100500,
    };
    let mut buf = Vec::new();
    val.serialize(&mut Serializer::new(&mut buf)).unwrap();

    // Expect: [42, 100500].
    assert_eq!(vec![0x92, 0x2a, 0xce, 0x0, 0x1, 0x88, 0x94], buf);
}

#[test]
fn serialize_struct_variant() {
    #[derive(Serialize)]
    enum Enum {
        V1 {
            f1: u32,
        },
        V2 {
            f1: u32,
        },
    }

    let mut buf = Vec::new();
    Enum::V1 { f1: 42 }.serialize(&mut Serializer::new(&mut buf)).unwrap();
    Enum::V2 { f1: 43 }.serialize(&mut Serializer::new(&mut buf)).unwrap();

    // Expect: {0 => [42]} {1 => [43]}.
    assert_eq!(vec![0x81, 0x00, 0x91, 0x2a, 0x81, 0x01, 0x91, 0x2b], buf);
}

#[test]
fn serialize_struct_variant_as_map() {
    #[derive(Serialize)]
    enum Enum {
        V1 {
            f1: u32,
        }
    }

    let mut se = Serializer::new(Vec::new())
        .with_struct_map();
    Enum::V1 { f1: 42 }.serialize(&mut se).unwrap();

    // Expect: {0 => {"f1": 42}}.
    assert_eq!(vec![0x81, 0x00, 0x81, 0xa2, 0x66, 0x31, 0x2a], se.into_inner());
}

#[test]
fn serialize_struct_with_flattened_map_field() {
    use std::collections::BTreeMap;

    #[derive(Serialize)]
    struct Struct {
        f1: u32,
        // not flattend!
        f2: BTreeMap<String, String>,
        #[serde(flatten)]
        f3: BTreeMap<String, String>
    }

    let mut se = Serializer::new(Vec::new());
    Struct {
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
    }
    .serialize(&mut se).unwrap();

    // Expect: { "f1": 0, "f2": { "german": "Hallo Welt!" }, "english": "Hello World!" }.
    assert_eq!(
        vec![
            0x83, 0xA2, 0x66, 0x31, 0x00, 0xA2, 0x66, 0x32, 0x81, 0xA6, 0x67, 0x65, 0x72, 0x6D, 0x61, 0x6E, 0xAB,
            0x48, 0x61, 0x6C, 0x6C, 0x6F, 0x20, 0x57, 0x65, 0x6C, 0x74, 0x21, 0xA7, 0x65, 0x6E, 0x67, 0x6C, 0x69,
            0x73, 0x68, 0xAC, 0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x20, 0x57, 0x6F, 0x72, 0x6C, 0x64, 0x21,
        ],
        se.into_inner()
    );
}

#[test]
fn serialize_struct_with_flattened_struct_field() {
    #[derive(Serialize)]
    struct Struct {
        f1: u32,
        // not flattend!
        f2: InnerStruct,
        #[serde(flatten)]
        f3: InnerStruct
    }

    #[derive(Serialize)]
    struct InnerStruct {
        f4: u32,
        f5: u32
    }

    let strct = Struct {
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
        let mut se = Serializer::new(Vec::new());
        strct.serialize(&mut se).unwrap();

        // Expect: { "f1": 0, "f2": [8, 13], "f4": 21, "f5": 34 }.
        assert_eq!(vec![
            0x84, 0xA2, 0x66, 0x31, 0x00, 0xA2, 0x66, 0x32, 0x92, 0x08, 0x0D, 0xA2, 0x66, 0x34, 0x15, 0xA2, 0x66, 0x35, 0x22,
        ],
        se.into_inner());
    }

    // struct-as-map
    {
        let mut se = Serializer::new(Vec::new()).with_struct_map();
        strct.serialize(&mut se).unwrap();

        // Expect: { "f1": 0, "f2": { "f4": 8, "f5": 13 }, "f4": 21, "f5": 34 }.
        assert_eq!(
            vec![
                0x84, 0xA2, 0x66, 0x31, 0x00, 0xA2, 0x66, 0x32, 0x82, 0xA2, 0x66, 0x34, 0x08,
                0xA2, 0x66, 0x35, 0x0D, 0xA2, 0x66, 0x34, 0x15, 0xA2, 0x66, 0x35, 0x22,
            ],
            se.into_inner()
        );
    }
}

#[test]
fn pass_struct_as_map_using_ext() {
    #[derive(Serialize)]
    struct Dog<'a> {
        name: &'a str,
        age: u16,
    }

    let dog = Dog {
        name: "Bobby",
        age: 8,
    };

    let mut se = Serializer::new(Vec::new())
        .with_struct_map();

    dog.serialize(&mut se).unwrap();

    // Expect: {"name": "Bobby", "age": 8}.
    assert_eq!(vec![0x82, 0xa4, 0x6e, 0x61, 0x6d, 0x65, 0xa5, 0x42, 0x6f, 0x62, 0x62, 0x79, 0xa3, 0x61, 0x67, 0x65, 0x08],
               se.into_inner());
}

#[test]
fn pass_struct_as_tuple_using_double_ext() {
    #[derive(Serialize)]
    struct Dog<'a> {
        name: &'a str,
        age: u16,
    }

    let dog = Dog {
        name: "Bobby",
        age: 8,
    };

    let mut se = Serializer::new(Vec::new())
        .with_struct_map()
        .with_struct_tuple();

    dog.serialize(&mut se).unwrap();

    assert_eq!(vec![0x92, 0xa5, 0x42, 0x6f, 0x62, 0x62, 0x79, 0x08],
               se.into_inner());
}

#[test]
fn pass_struct_as_map_using_triple_ext() {
    #[derive(Serialize)]
    struct Dog<'a> {
        name: &'a str,
        age: u16,
    }

    let dog = Dog {
        name: "Bobby",
        age: 8,
    };

    let mut se = Serializer::new(Vec::new())
        .with_struct_map()
        .with_struct_tuple()
        .with_struct_map();

    dog.serialize(&mut se).unwrap();

    // Expect: {"name": "Bobby", "age": 8}.
    assert_eq!(vec![0x82, 0xa4, 0x6e, 0x61, 0x6d, 0x65, 0xa5, 0x42, 0x6f, 0x62, 0x62, 0x79, 0xa3, 0x61, 0x67, 0x65, 0x08],
               se.into_inner());
}

#[test]
fn pass_struct_as_map_using_triple_ext_many_times() {
    #[derive(Serialize)]
    struct Dog<'a> {
        name: &'a str,
        age: u16,
    }

    let dog = Dog {
        name: "Bobby",
        age: 8,
    };

    let mut se = Serializer::new(Vec::new())
        .with_struct_map()
        .with_struct_tuple()
        .with_struct_map()
        .with_struct_map()
        .with_struct_map()
        .with_struct_map();

    dog.serialize(&mut se).unwrap();

    // Expect: {"name": "Bobby", "age": 8}.
    assert_eq!(vec![0x82, 0xa4, 0x6e, 0x61, 0x6d, 0x65, 0xa5, 0x42, 0x6f, 0x62, 0x62, 0x79, 0xa3, 0x61, 0x67, 0x65, 0x08],
               se.into_inner());
}

use std::fmt;
use std::io;
use std::mem;

use itoa;
use ryu;
use serde::ser::{
    Error as SerdeError, Serialize, SerializeMap, SerializeSeq,
    SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant, Serializer,
};
use serde::serde_if_integer128;

use crate::error::{Error, ErrorKind};
use crate::writer::Writer;

/// Serialize the given value to the given writer, and return an error if
/// anything went wrong.
pub fn serialize<S: Serialize, W: io::Write>(
    wtr: &mut Writer<W>,
    value: S,
) -> Result<(), Error> {
    value.serialize(&mut SeRecord { wtr: wtr })
}

struct SeRecord<'w, W: 'w + io::Write> {
    wtr: &'w mut Writer<W>,
}

impl<'a, 'w, W: io::Write> Serializer for &'a mut SeRecord<'w, W> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, v: bool) -> Result<Self::Ok, Self::Error> {
        if v {
            self.wtr.write_field("true")
        } else {
            self.wtr.write_field("false")
        }
    }

    fn serialize_i8(self, v: i8) -> Result<Self::Ok, Self::Error> {
        let mut buffer = itoa::Buffer::new();
        self.wtr.write_field(buffer.format(v))
    }

    fn serialize_i16(self, v: i16) -> Result<Self::Ok, Self::Error> {
        let mut buffer = itoa::Buffer::new();
        self.wtr.write_field(buffer.format(v))
    }

    fn serialize_i32(self, v: i32) -> Result<Self::Ok, Self::Error> {
        let mut buffer = itoa::Buffer::new();
        self.wtr.write_field(buffer.format(v))
    }

    fn serialize_i64(self, v: i64) -> Result<Self::Ok, Self::Error> {
        let mut buffer = itoa::Buffer::new();
        self.wtr.write_field(buffer.format(v))
    }

    serde_if_integer128! {
        fn serialize_i128(self, v: i128) -> Result<Self::Ok, Self::Error> {
            self.collect_str(&v)
        }
    }

    fn serialize_u8(self, v: u8) -> Result<Self::Ok, Self::Error> {
        let mut buffer = itoa::Buffer::new();
        self.wtr.write_field(buffer.format(v))
    }

    fn serialize_u16(self, v: u16) -> Result<Self::Ok, Self::Error> {
        let mut buffer = itoa::Buffer::new();
        self.wtr.write_field(buffer.format(v))
    }

    fn serialize_u32(self, v: u32) -> Result<Self::Ok, Self::Error> {
        let mut buffer = itoa::Buffer::new();
        self.wtr.write_field(buffer.format(v))
    }

    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error> {
        let mut buffer = itoa::Buffer::new();
        self.wtr.write_field(buffer.format(v))
    }

    serde_if_integer128! {
        fn serialize_u128(self, v: u128) -> Result<Self::Ok, Self::Error> {
            self.collect_str(&v)
        }
    }

    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        let mut buffer = ryu::Buffer::new();
        self.wtr.write_field(buffer.format(v))
    }

    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        let mut buffer = ryu::Buffer::new();
        self.wtr.write_field(buffer.format(v))
    }

    fn serialize_char(self, v: char) -> Result<Self::Ok, Self::Error> {
        self.wtr.write_field(v.encode_utf8(&mut [0; 4]))
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        self.wtr.write_field(value)
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Self::Ok, Self::Error> {
        self.wtr.write_field(value)
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.wtr.write_field(&[])
    }

    fn serialize_some<T: ?Sized + Serialize>(
        self,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        None::<()>.serialize(self)
    }

    fn serialize_unit_struct(
        self,
        name: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.wtr.write_field(name)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.wtr.write_field(variant)
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_seq(
        self,
        _len: Option<usize>,
    ) -> Result<Self::SerializeSeq, Self::Error> {
        Ok(self)
    }

    fn serialize_tuple(
        self,
        _len: usize,
    ) -> Result<Self::SerializeTuple, Self::Error> {
        Ok(self)
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Ok(self)
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Err(Error::custom("serializing enum tuple variants is not supported"))
    }

    fn serialize_map(
        self,
        _len: Option<usize>,
    ) -> Result<Self::SerializeMap, Self::Error> {
        // The right behavior for serializing maps isn't clear.
        Err(Error::custom(
            "serializing maps is not supported, \
             if you have a use case, please file an issue at \
             https://github.com/BurntSushi/rust-csv",
        ))
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(self)
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Err(Error::custom("serializing enum struct variants is not supported"))
    }
}

impl<'a, 'w, W: io::Write> SerializeSeq for &'a mut SeRecord<'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: ?Sized + Serialize>(
        &mut self,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl<'a, 'w, W: io::Write> SerializeTuple for &'a mut SeRecord<'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: ?Sized + Serialize>(
        &mut self,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl<'a, 'w, W: io::Write> SerializeTupleStruct for &'a mut SeRecord<'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl<'a, 'w, W: io::Write> SerializeTupleVariant for &'a mut SeRecord<'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _value: &T,
    ) -> Result<(), Self::Error> {
        unreachable!()
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        unreachable!()
    }
}

impl<'a, 'w, W: io::Write> SerializeMap for &'a mut SeRecord<'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_key<T: ?Sized + Serialize>(
        &mut self,
        _key: &T,
    ) -> Result<(), Self::Error> {
        unreachable!()
    }

    fn serialize_value<T: ?Sized + Serialize>(
        &mut self,
        _value: &T,
    ) -> Result<(), Self::Error> {
        unreachable!()
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        unreachable!()
    }
}

impl<'a, 'w, W: io::Write> SerializeStruct for &'a mut SeRecord<'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl<'a, 'w, W: io::Write> SerializeStructVariant for &'a mut SeRecord<'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _key: &'static str,
        _value: &T,
    ) -> Result<(), Self::Error> {
        unreachable!()
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        unreachable!()
    }
}

impl SerdeError for Error {
    fn custom<T: fmt::Display>(msg: T) -> Error {
        Error::new(ErrorKind::Serialize(msg.to_string()))
    }
}

fn error_scalar_outside_struct<T: fmt::Display>(name: T) -> Error {
    Error::custom(format!(
        "cannot serialize {} scalar outside struct \
         when writing headers from structs",
        name
    ))
}

fn error_container_inside_struct<T: fmt::Display>(name: T) -> Error {
    Error::custom(format!(
        "cannot serialize {} container inside struct \
         when writing headers from structs",
        name
    ))
}

/// Write header names corresponding to the field names of the value (if the
/// value has field names).
///
/// If the type to be serialized has field names (e.g. it's a struct), then
/// header names are written, and the `Ok` return value is `true`.
///
/// If the type to be serialized doesn't have field names, then nothing is
/// written, and the `Ok` return value is `false`.
pub fn serialize_header<S: Serialize, W: io::Write>(
    wtr: &mut Writer<W>,
    value: S,
) -> Result<bool, Error> {
    let mut ser = SeHeader::new(wtr);
    value.serialize(&mut ser).map(|_| ser.wrote_header())
}

/// State machine for `SeHeader`.
///
/// This is a diagram of the transitions in the state machine. Note that only
/// some serialization events cause a state transition, and only for certain
/// states. For example, encountering a scalar causes a transition if the state
/// is `Write` or `EncounteredStructField`, but not if the state is
/// `ErrorIfWrite(err)` or `InStructField`.
///
/// ```text
///                              +-----+
///                              |Write|
///                              +-----+
///                                 |
///              /------------------+------------------\
///              |                  |                  |
///          encounter            finish           encounter
///            scalar               |             struct field
///              |                  |                  |
///              v                  v                  v
///     +-----------------+       Ok(())        +-------------+
///     |ErrorIfWrite(err)|                     |InStructField|<--------\
///     +-----------------+                     +-------------+         |
///              |                                     |                |
///       /------+------\            /-----------------+                |
///       |             |            |                 |                |
///   encounter       finish     encounter          finish          encounter
///  struct field       |        container           field         struct field
///       |             |            |                 |                |
///       v             v            v                 v                |
///   Err(err)       Ok(())        Err(_)   +----------------------+    |
///                                         |EncounteredStructField|    |
///                                         +----------------------+    |
///                                                    |                |
///                                         /----------+----------------/
///                                         |          |
///                                     encounter    finish
///                                       scalar       |
///                                         |          |
///                                         v          v
///                                       Err(_)    Ok(())
/// ```
enum HeaderState {
    /// Start here. Headers need to be written if the type has field names.
    Write,
    /// The serializer still has not encountered a struct field. If one is
    /// encountered (headers need to be written), return the enclosed error.
    ErrorIfWrite(Error),
    /// The serializer encountered one or more struct fields (and wrote their
    /// names).
    EncounteredStructField,
    /// The serializer is currently in a struct field value.
    InStructField,
}

struct SeHeader<'w, W: 'w + io::Write> {
    wtr: &'w mut Writer<W>,
    state: HeaderState,
}

impl<'w, W: io::Write> SeHeader<'w, W> {
    fn new(wtr: &'w mut Writer<W>) -> Self {
        SeHeader { wtr: wtr, state: HeaderState::Write }
    }

    fn wrote_header(&self) -> bool {
        use self::HeaderState::*;
        match self.state {
            Write | ErrorIfWrite(_) => false,
            EncounteredStructField | InStructField => true,
        }
    }

    fn handle_scalar<T: fmt::Display>(
        &mut self,
        name: T,
    ) -> Result<(), Error> {
        use self::HeaderState::*;

        match self.state {
            Write => {
                self.state = ErrorIfWrite(error_scalar_outside_struct(name));
                Ok(())
            }
            ErrorIfWrite(_) | InStructField => Ok(()),
            EncounteredStructField => Err(error_scalar_outside_struct(name)),
        }
    }

    fn handle_container<T: fmt::Display>(
        &mut self,
        name: T,
    ) -> Result<&mut Self, Error> {
        if let HeaderState::InStructField = self.state {
            Err(error_container_inside_struct(name))
        } else {
            Ok(self)
        }
    }
}

impl<'a, 'w, W: io::Write> Serializer for &'a mut SeHeader<'w, W> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, v: bool) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(v)
    }

    fn serialize_i8(self, v: i8) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(v)
    }

    fn serialize_i16(self, v: i16) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(v)
    }

    fn serialize_i32(self, v: i32) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(v)
    }

    fn serialize_i64(self, v: i64) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(v)
    }

    serde_if_integer128! {
        fn serialize_i128(self, v: i128) -> Result<Self::Ok, Self::Error> {
            self.handle_scalar(v)
        }
    }

    fn serialize_u8(self, v: u8) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(v)
    }

    fn serialize_u16(self, v: u16) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(v)
    }

    fn serialize_u32(self, v: u32) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(v)
    }

    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(v)
    }

    serde_if_integer128! {
        fn serialize_u128(self, v: u128) -> Result<Self::Ok, Self::Error> {
            self.handle_scalar(v)
        }
    }

    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(v)
    }

    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(v)
    }

    fn serialize_char(self, v: char) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(v)
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(value)
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar("&[u8]")
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar("None")
    }

    fn serialize_some<T: ?Sized + Serialize>(
        self,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar("Some(_)")
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar("()")
    }

    fn serialize_unit_struct(
        self,
        name: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(name)
    }

    fn serialize_unit_variant(
        self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(format!("{}::{}", name, variant))
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(format!("{}(_)", name))
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.handle_scalar(format!("{}::{}(_)", name, variant))
    }

    fn serialize_seq(
        self,
        _len: Option<usize>,
    ) -> Result<Self::SerializeSeq, Self::Error> {
        self.handle_container("sequence")
    }

    fn serialize_tuple(
        self,
        _len: usize,
    ) -> Result<Self::SerializeTuple, Self::Error> {
        self.handle_container("tuple")
    }

    fn serialize_tuple_struct(
        self,
        name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.handle_container(name)
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Err(Error::custom("serializing enum tuple variants is not supported"))
    }

    fn serialize_map(
        self,
        _len: Option<usize>,
    ) -> Result<Self::SerializeMap, Self::Error> {
        // The right behavior for serializing maps isn't clear.
        Err(Error::custom(
            "serializing maps is not supported, \
             if you have a use case, please file an issue at \
             https://github.com/BurntSushi/rust-csv",
        ))
    }

    fn serialize_struct(
        self,
        name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.handle_container(name)
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Err(Error::custom("serializing enum struct variants is not supported"))
    }
}

impl<'a, 'w, W: io::Write> SerializeSeq for &'a mut SeHeader<'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: ?Sized + Serialize>(
        &mut self,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl<'a, 'w, W: io::Write> SerializeTuple for &'a mut SeHeader<'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: ?Sized + Serialize>(
        &mut self,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl<'a, 'w, W: io::Write> SerializeTupleStruct for &'a mut SeHeader<'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl<'a, 'w, W: io::Write> SerializeTupleVariant for &'a mut SeHeader<'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _value: &T,
    ) -> Result<(), Self::Error> {
        unreachable!()
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        unreachable!()
    }
}

impl<'a, 'w, W: io::Write> SerializeMap for &'a mut SeHeader<'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_key<T: ?Sized + Serialize>(
        &mut self,
        _key: &T,
    ) -> Result<(), Self::Error> {
        unreachable!()
    }

    fn serialize_value<T: ?Sized + Serialize>(
        &mut self,
        _value: &T,
    ) -> Result<(), Self::Error> {
        unreachable!()
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        unreachable!()
    }
}

impl<'a, 'w, W: io::Write> SerializeStruct for &'a mut SeHeader<'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        // Grab old state and update state to `EncounteredStructField`.
        let old_state =
            mem::replace(&mut self.state, HeaderState::EncounteredStructField);
        if let HeaderState::ErrorIfWrite(err) = old_state {
            return Err(err);
        }
        self.wtr.write_field(key)?;

        // Check that there aren't any containers in the value.
        self.state = HeaderState::InStructField;
        value.serialize(&mut **self)?;
        self.state = HeaderState::EncounteredStructField;

        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl<'a, 'w, W: io::Write> SerializeStructVariant for &'a mut SeHeader<'w, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _key: &'static str,
        _value: &T,
    ) -> Result<(), Self::Error> {
        unreachable!()
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    use bstr::ByteSlice;
    use serde::{serde_if_integer128, Serialize};

    use crate::error::{Error, ErrorKind};
    use crate::writer::Writer;

    use super::{SeHeader, SeRecord};

    fn serialize<S: Serialize>(s: S) -> String {
        let mut wtr = Writer::from_writer(vec![]);
        s.serialize(&mut SeRecord { wtr: &mut wtr }).unwrap();
        wtr.write_record(None::<&[u8]>).unwrap();
        String::from_utf8(wtr.into_inner().unwrap()).unwrap()
    }

    /// Serialize using `SeHeader`. Returns whether a header was written and
    /// the output of the writer.
    fn serialize_header<S: Serialize>(s: S) -> (bool, String) {
        let mut wtr = Writer::from_writer(vec![]);
        let wrote = {
            let mut ser = SeHeader::new(&mut wtr);
            s.serialize(&mut ser).unwrap();
            ser.wrote_header()
        };
        (wrote, String::from_utf8(wtr.into_inner().unwrap()).unwrap())
    }

    fn serialize_err<S: Serialize>(s: S) -> Error {
        let mut wtr = Writer::from_writer(vec![]);
        s.serialize(&mut SeRecord { wtr: &mut wtr }).unwrap_err()
    }

    fn serialize_header_err<S: Serialize>(s: S) -> Error {
        let mut wtr = Writer::from_writer(vec![]);
        s.serialize(&mut SeHeader::new(&mut wtr)).unwrap_err()
    }

    #[test]
    fn bool() {
        let got = serialize(true);
        assert_eq!(got, "true\n");
        let (wrote, got) = serialize_header(true);
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn integer() {
        let got = serialize(12345);
        assert_eq!(got, "12345\n");
        let (wrote, got) = serialize_header(12345);
        assert!(!wrote);
        assert_eq!(got, "");
    }

    serde_if_integer128! {
        #[test]
        fn integer_u128() {
            let got = serialize(i128::max_value() as u128 + 1);
            assert_eq!(got, "170141183460469231731687303715884105728\n");
            let (wrote, got) = serialize_header(12345);
            assert!(!wrote);
            assert_eq!(got, "");
        }

        #[test]
        fn integer_i128() {
            let got = serialize(i128::max_value());
            assert_eq!(got, "170141183460469231731687303715884105727\n");
            let (wrote, got) = serialize_header(12345);
            assert!(!wrote);
            assert_eq!(got, "");
        }
    }

    #[test]
    fn float() {
        let got = serialize(1.23);
        assert_eq!(got, "1.23\n");
        let (wrote, got) = serialize_header(1.23);
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn float_nan() {
        let got = serialize(::std::f64::NAN);
        assert_eq!(got, "NaN\n");
        let (wrote, got) = serialize_header(::std::f64::NAN);
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn char() {
        let got = serialize('☃');
        assert_eq!(got, "☃\n");
        let (wrote, got) = serialize_header('☃');
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn str() {
        let got = serialize("how\nare\n\"you\"?");
        assert_eq!(got, "\"how\nare\n\"\"you\"\"?\"\n");
        let (wrote, got) = serialize_header("how\nare\n\"you\"?");
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn bytes() {
        let got = serialize(b"how\nare\n\"you\"?".as_bstr());
        assert_eq!(got, "\"how\nare\n\"\"you\"\"?\"\n");
        let (wrote, got) = serialize_header(&b"how\nare\n\"you\"?"[..]);
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn option() {
        let got = serialize(None::<()>);
        assert_eq!(got, "\"\"\n");
        let (wrote, got) = serialize_header(None::<()>);
        assert!(!wrote);
        assert_eq!(got, "");

        let got = serialize(Some(5));
        assert_eq!(got, "5\n");
        let (wrote, got) = serialize_header(Some(5));
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn unit() {
        let got = serialize(());
        assert_eq!(got, "\"\"\n");
        let (wrote, got) = serialize_header(());
        assert!(!wrote);
        assert_eq!(got, "");

        let got = serialize((5, ()));
        assert_eq!(got, "5,\n");
        let (wrote, got) = serialize_header(());
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn struct_unit() {
        #[derive(Serialize)]
        struct Foo;

        let got = serialize(Foo);
        assert_eq!(got, "Foo\n");
        let (wrote, got) = serialize_header(Foo);
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn struct_newtype() {
        #[derive(Serialize)]
        struct Foo(f64);

        let got = serialize(Foo(1.5));
        assert_eq!(got, "1.5\n");
        let (wrote, got) = serialize_header(Foo(1.5));
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn enum_units() {
        #[derive(Serialize)]
        enum Wat {
            Foo,
            Bar,
            Baz,
        }

        let got = serialize(Wat::Foo);
        assert_eq!(got, "Foo\n");
        let (wrote, got) = serialize_header(Wat::Foo);
        assert!(!wrote);
        assert_eq!(got, "");

        let got = serialize(Wat::Bar);
        assert_eq!(got, "Bar\n");
        let (wrote, got) = serialize_header(Wat::Bar);
        assert!(!wrote);
        assert_eq!(got, "");

        let got = serialize(Wat::Baz);
        assert_eq!(got, "Baz\n");
        let (wrote, got) = serialize_header(Wat::Baz);
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn enum_newtypes() {
        #[derive(Serialize)]
        enum Wat {
            Foo(i32),
            Bar(f32),
            Baz(bool),
        }

        let got = serialize(Wat::Foo(5));
        assert_eq!(got, "5\n");
        let (wrote, got) = serialize_header(Wat::Foo(5));
        assert!(!wrote);
        assert_eq!(got, "");

        let got = serialize(Wat::Bar(1.5));
        assert_eq!(got, "1.5\n");
        let (wrote, got) = serialize_header(Wat::Bar(1.5));
        assert!(!wrote);
        assert_eq!(got, "");

        let got = serialize(Wat::Baz(true));
        assert_eq!(got, "true\n");
        let (wrote, got) = serialize_header(Wat::Baz(true));
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn seq() {
        let got = serialize(vec![1, 2, 3]);
        assert_eq!(got, "1,2,3\n");
        let (wrote, got) = serialize_header(vec![1, 2, 3]);
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn tuple() {
        let row = (true, 1.5, "hi");
        let got = serialize(row.clone());
        assert_eq!(got, "true,1.5,hi\n");
        let (wrote, got) = serialize_header(row.clone());
        assert!(!wrote);
        assert_eq!(got, "");

        let row = (true, 1.5, vec![1, 2, 3]);
        let got = serialize(row.clone());
        assert_eq!(got, "true,1.5,1,2,3\n");
        let (wrote, got) = serialize_header(row.clone());
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn tuple_struct() {
        #[derive(Clone, Serialize)]
        struct Foo(bool, i32, String);

        let row = Foo(false, 42, "hi".to_string());
        let got = serialize(row.clone());
        assert_eq!(got, "false,42,hi\n");
        let (wrote, got) = serialize_header(row.clone());
        assert!(!wrote);
        assert_eq!(got, "");
    }

    #[test]
    fn tuple_variant() {
        #[derive(Clone, Serialize)]
        enum Foo {
            X(bool, i32, String),
        }

        let row = Foo::X(false, 42, "hi".to_string());
        let err = serialize_err(row.clone());
        match *err.kind() {
            ErrorKind::Serialize(_) => {}
            ref x => panic!("expected ErrorKind::Serialize but got '{:?}'", x),
        }
        let err = serialize_header_err(row.clone());
        match *err.kind() {
            ErrorKind::Serialize(_) => {}
            ref x => panic!("expected ErrorKind::Serialize but got '{:?}'", x),
        }
    }

    #[test]
    fn enum_struct_variant() {
        #[derive(Clone, Serialize)]
        enum Foo {
            X { a: bool, b: i32, c: String },
        }

        let row = Foo::X { a: false, b: 1, c: "hi".into() };
        let err = serialize_err(row.clone());
        match *err.kind() {
            ErrorKind::Serialize(_) => {}
            ref x => panic!("expected ErrorKind::Serialize but got '{:?}'", x),
        }
        let err = serialize_header_err(row.clone());
        match *err.kind() {
            ErrorKind::Serialize(_) => {}
            ref x => panic!("expected ErrorKind::Serialize but got '{:?}'", x),
        }
    }

    #[test]
    fn struct_no_headers() {
        #[derive(Serialize)]
        struct Foo {
            x: bool,
            y: i32,
            z: String,
        }

        let got = serialize(Foo { x: true, y: 5, z: "hi".into() });
        assert_eq!(got, "true,5,hi\n");
    }

    serde_if_integer128! {
        #[test]
        fn struct_no_headers_128() {
            #[derive(Serialize)]
            struct Foo {
                x: i128,
                y: u128,
            }

            let got =
                serialize(Foo { x: i128::max_value(), y: u128::max_value() });
            assert_eq!(
                got,
                "170141183460469231731687303715884105727,\
                 340282366920938463463374607431768211455\n"
            );
        }
    }

    #[test]
    fn struct_headers() {
        #[derive(Clone, Serialize)]
        struct Foo {
            x: bool,
            y: i32,
            z: String,
        }

        let row = Foo { x: true, y: 5, z: "hi".into() };
        let (wrote, got) = serialize_header(row.clone());
        assert!(wrote);
        assert_eq!(got, "x,y,z");
        let got = serialize(row.clone());
        assert_eq!(got, "true,5,hi\n");
    }

    #[test]
    fn struct_headers_nested() {
        #[derive(Clone, Serialize)]
        struct Foo {
            label: String,
            nest: Nested,
        }
        #[derive(Clone, Serialize)]
        struct Nested {
            label2: String,
            value: i32,
        }

        let row = Foo {
            label: "foo".into(),
            nest: Nested { label2: "bar".into(), value: 5 },
        };

        let got = serialize(row.clone());
        assert_eq!(got, "foo,bar,5\n");

        let err = serialize_header_err(row.clone());
        match *err.kind() {
            ErrorKind::Serialize(_) => {}
            ref x => panic!("expected ErrorKind::Serialize but got '{:?}'", x),
        }
    }

    #[test]
    fn struct_headers_nested_seq() {
        #[derive(Clone, Serialize)]
        struct Foo {
            label: String,
            values: Vec<i32>,
        }
        let row = Foo { label: "foo".into(), values: vec![1, 2, 3] };

        let got = serialize(row.clone());
        assert_eq!(got, "foo,1,2,3\n");

        let err = serialize_header_err(row.clone());
        match *err.kind() {
            ErrorKind::Serialize(_) => {}
            ref x => panic!("expected ErrorKind::Serialize but got '{:?}'", x),
        }
    }

    #[test]
    fn struct_headers_inside_tuple() {
        #[derive(Clone, Serialize)]
        struct Foo {
            label: String,
            num: f64,
        }
        #[derive(Clone, Serialize)]
        struct Bar {
            label2: bool,
            value: i32,
            empty: (),
        }
        let row = (
            Foo { label: "hi".to_string(), num: 5.0 },
            Bar { label2: true, value: 3, empty: () },
            Foo { label: "baz".to_string(), num: 2.3 },
        );

        let got = serialize(row.clone());
        assert_eq!(got, "hi,5.0,true,3,,baz,2.3\n");

        let (wrote, got) = serialize_header(row.clone());
        assert!(wrote);
        assert_eq!(got, "label,num,label2,value,empty,label,num");
    }

    #[test]
    fn struct_headers_inside_tuple_scalar_before() {
        #[derive(Clone, Serialize)]
        struct Foo {
            label: String,
            num: f64,
        }
        let row = (3.14, Foo { label: "hi".to_string(), num: 5.0 });

        let got = serialize(row.clone());
        assert_eq!(got, "3.14,hi,5.0\n");

        let err = serialize_header_err(row.clone());
        match *err.kind() {
            ErrorKind::Serialize(_) => {}
            ref x => panic!("expected ErrorKind::Serialize but got '{:?}'", x),
        }
    }

    #[test]
    fn struct_headers_inside_tuple_scalar_after() {
        #[derive(Clone, Serialize)]
        struct Foo {
            label: String,
            num: f64,
        }
        let row = (Foo { label: "hi".to_string(), num: 5.0 }, 3.14);

        let got = serialize(row.clone());
        assert_eq!(got, "hi,5.0,3.14\n");

        let err = serialize_header_err(row.clone());
        match *err.kind() {
            ErrorKind::Serialize(_) => {}
            ref x => panic!("expected ErrorKind::Serialize but got '{:?}'", x),
        }
    }

    #[test]
    fn struct_headers_inside_seq() {
        #[derive(Clone, Serialize)]
        struct Foo {
            label: String,
            num: f64,
        }
        let row = vec![
            Foo { label: "hi".to_string(), num: 5.0 },
            Foo { label: "baz".to_string(), num: 2.3 },
        ];

        let got = serialize(row.clone());
        assert_eq!(got, "hi,5.0,baz,2.3\n");

        let (wrote, got) = serialize_header(row.clone());
        assert!(wrote);
        assert_eq!(got, "label,num,label,num");
    }

    #[test]
    fn struct_headers_inside_nested_tuple_seq() {
        #[derive(Clone, Serialize)]
        struct Foo {
            label: String,
            num: f64,
        }
        #[derive(Clone, Serialize)]
        struct Bar {
            label2: Baz,
            value: i32,
            empty: (),
        }
        #[derive(Clone, Serialize)]
        struct Baz(bool);
        let row = (
            (
                Foo { label: "hi".to_string(), num: 5.0 },
                Bar { label2: Baz(true), value: 3, empty: () },
            ),
            vec![(Foo { label: "baz".to_string(), num: 2.3 },)],
        );

        let got = serialize(row.clone());
        assert_eq!(got, "hi,5.0,true,3,,baz,2.3\n");

        let (wrote, got) = serialize_header(row.clone());
        assert!(wrote);
        assert_eq!(got, "label,num,label2,value,empty,label,num");
    }
}

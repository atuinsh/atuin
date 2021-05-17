//! Generic deserialization framework.

#[cfg(feature = "std")]
use std::error;
#[cfg(not(feature = "std"))]
use error;

#[cfg(all(not(feature = "std"), feature = "collections"))]
use collections::{String, Vec};

use core::fmt;

///////////////////////////////////////////////////////////////////////////////

pub mod impls;
pub mod value;
mod from_primitive;

///////////////////////////////////////////////////////////////////////////////

/// `Error` is a trait that allows a `Deserialize` to generically create a
/// `Deserializer` error.
pub trait Error: Sized + error::Error {
    /// Raised when there is general error when deserializing a type.
    #[cfg(any(feature = "std", feature = "collections"))]
    fn custom<T: Into<String>>(msg: T) -> Self;

    /// Raised when there is general error when deserializing a type.
    #[cfg(all(not(feature = "std"), not(feature = "collections")))]
    fn custom<T: Into<&'static str>>(msg: T) -> Self;

    /// Raised when a `Deserialize` type unexpectedly hit the end of the stream.
    fn end_of_stream() -> Self;

    /// Raised when a `Deserialize` was passed an incorrect type.
    fn invalid_type(ty: Type) -> Self {
        Error::custom(format!("Invalid type. Expected `{:?}`", ty))
    }

    /// Raised when a `Deserialize` was passed an incorrect value.
    fn invalid_value(msg: &str) -> Self {
        Error::custom(format!("Invalid value: {}", msg))
    }

    /// Raised when a fixed sized sequence or map was passed in the wrong amount of arguments.
    ///
    /// The parameter `len` is the number of arguments found in the serialization. The sequence
    /// may either expect more arguments or less arguments.
    fn invalid_length(len: usize) -> Self {
        Error::custom(format!("Invalid length: {}", len))
    }

    /// Raised when a `Deserialize` enum type received an unexpected variant.
    fn unknown_variant(field: &str) -> Self {
        Error::custom(format!("Unknown variant `{}`", field))
    }

    /// Raised when a `Deserialize` struct type received an unexpected struct field.
    fn unknown_field(field: &str) -> Self {
        Error::custom(format!("Unknown field `{}`", field))
    }

    /// raised when a `deserialize` struct type did not receive a field.
    fn missing_field(field: &'static str) -> Self {
        Error::custom(format!("Missing field `{}`", field))
    }

    /// Raised when a `Deserialize` struct type received more than one of the
    /// same struct field.
    fn duplicate_field(field: &'static str) -> Self {
        Error::custom(format!("Duplicate field `{}`", field))
    }
}

/// `Type` represents all the primitive types that can be deserialized. This is used by
/// `Error::invalid_type`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Type {
    /// Represents a `bool` type.
    Bool,

    /// Represents a `usize` type.
    Usize,

    /// Represents a `u8` type.
    U8,

    /// Represents a `u16` type.
    U16,

    /// Represents a `u32` type.
    U32,

    /// Represents a `u64` type.
    U64,

    /// Represents a `isize` type.
    Isize,

    /// Represents a `i8` type.
    I8,

    /// Represents a `i16` type.
    I16,

    /// Represents a `i32` type.
    I32,

    /// Represents a `i64` type.
    I64,

    /// Represents a `f32` type.
    F32,

    /// Represents a `f64` type.
    F64,

    /// Represents a `char` type.
    Char,

    /// Represents a `&str` type.
    Str,

    /// Represents a `String` type.
    String,

    /// Represents a `()` type.
    Unit,

    /// Represents an `Option<T>` type.
    Option,

    /// Represents a sequence type.
    Seq,

    /// Represents a map type.
    Map,

    /// Represents a unit struct type.
    UnitStruct,

    /// Represents a newtype type.
    NewtypeStruct,

    /// Represents a tuple struct type.
    TupleStruct,

    /// Represents a struct type.
    Struct,

    /// Represents a struct field name.
    FieldName,

    /// Represents a tuple type.
    Tuple,

    /// Represents an `enum` type.
    Enum,

    /// Represents an enum variant name.
    VariantName,

    /// Represents a struct variant.
    StructVariant,

    /// Represents a tuple variant.
    TupleVariant,

    /// Represents a unit variant.
    UnitVariant,

    /// Represents a `&[u8]` type.
    Bytes,
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        let display = match *self {
            Type::Bool          => "bool",
            Type::Usize         => "usize",
            Type::U8            => "u8",
            Type::U16           => "u16",
            Type::U32           => "u32",
            Type::U64           => "u64",
            Type::Isize         => "isize",
            Type::I8            => "i8",
            Type::I16           => "i16",
            Type::I32           => "i32",
            Type::I64           => "i64",
            Type::F32           => "f32",
            Type::F64           => "f64",
            Type::Char          => "char",
            Type::Str           => "str",
            Type::String        => "string",
            Type::Unit          => "unit",
            Type::Option        => "option",
            Type::Seq           => "seq",
            Type::Map           => "map",
            Type::UnitStruct    => "unit struct",
            Type::NewtypeStruct => "newtype struct",
            Type::TupleStruct   => "tuple struct",
            Type::Struct        => "struct",
            Type::FieldName     => "field name",
            Type::Tuple         => "tuple",
            Type::Enum          => "enum",
            Type::VariantName   => "variant name",
            Type::StructVariant => "struct variant",
            Type::TupleVariant  => "tuple variant",
            Type::UnitVariant   => "unit variant",
            Type::Bytes         => "bytes",
        };
        display.fmt(formatter)
    }
}

///////////////////////////////////////////////////////////////////////////////

/// `Deserialize` represents a type that can be deserialized.
pub trait Deserialize: Sized {
    /// Deserialize this value given this `Deserializer`.
    fn deserialize<D>(deserializer: &mut D) -> Result<Self, D::Error>
        where D: Deserializer;
}

///////////////////////////////////////////////////////////////////////////////

/// `Deserializer` is a trait that can deserialize values by threading a `Visitor` trait through a
/// value. It supports two entry point styles which enables different kinds of deserialization.
///
/// 1) The `deserialize` method. File formats like JSON embed the type of its construct in its file
///    format. This allows the `Deserializer` to deserialize into a generic type like
///    `json::Value`, which can represent all JSON types.
///
/// 2) The `deserialize_*` methods. File formats like bincode do not embed in its format how to
///    decode its values. It relies instead on the `Deserialize` type to hint to the `Deserializer`
///    with the `deserialize_*` methods how it should parse the next value. One downside though to
///    only supporting the `deserialize_*` types is that it does not allow for deserializing into a
///    generic `json::Value`-esque type.
pub trait Deserializer {
    /// The error type that can be returned if some error occurs during deserialization.
    type Error: Error;

    /// This method walks a visitor through a value as it is being deserialized.
    fn deserialize<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a `bool` value.
    fn deserialize_bool<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting an `usize` value.
    /// A reasonable default is to forward to `deserialize_u64`.
    fn deserialize_usize<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting an `u8` value.
    /// A reasonable default is to forward to `deserialize_u64`.
    fn deserialize_u8<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting an `u16` value.
    /// A reasonable default is to forward to `deserialize_u64`.
    fn deserialize_u16<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting an `u32` value.
    /// A reasonable default is to forward to `deserialize_u64`.
    fn deserialize_u32<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting an `u64` value.
    fn deserialize_u64<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting an `isize` value.
    /// A reasonable default is to forward to `deserialize_i64`.
    fn deserialize_isize<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting an `i8` value.
    /// A reasonable default is to forward to `deserialize_i64`.
    fn deserialize_i8<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting an `i16` value.
    /// A reasonable default is to forward to `deserialize_i64`.
    fn deserialize_i16<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting an `i32` value.
    /// A reasonable default is to forward to `deserialize_i64`.
    fn deserialize_i32<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting an `i64` value.
    fn deserialize_i64<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a `f32` value.
    /// A reasonable default is to forward to `deserialize_f64`.
    fn deserialize_f32<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a `f64` value.
    fn deserialize_f64<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a `char` value.
    fn deserialize_char<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a `&str` value.
    fn deserialize_str<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a `String` value.
    fn deserialize_string<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting an `unit` value.
    fn deserialize_unit<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting an `Option` value. This allows
    /// deserializers that encode an optional value as a nullable value to convert the null value
    /// into a `None`, and a regular value as `Some(value)`.
    fn deserialize_option<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a sequence value. This allows
    /// deserializers to parse sequences that aren't tagged as sequences.
    fn deserialize_seq<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a fixed size array. This allows
    /// deserializers to parse arrays that aren't tagged as arrays.
    fn deserialize_seq_fixed_size<V>(&mut self,
                                     len: usize,
                                     visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a `Vec<u8>`. This allows
    /// deserializers that provide a custom byte vector serialization to properly deserialize the
    /// type.
    fn deserialize_bytes<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a map of values. This allows
    /// deserializers to parse sequences that aren't tagged as maps.
    fn deserialize_map<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a unit struct. This allows
    /// deserializers to a unit struct that aren't tagged as a unit struct.
    fn deserialize_unit_struct<V>(&mut self,
                                  name: &'static str,
                                  visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a newtype struct. This allows
    /// deserializers to a newtype struct that aren't tagged as a newtype struct.
    /// A reasonable default is to simply deserialize the expected value directly.
    fn deserialize_newtype_struct<V>(&mut self,
                                     name: &'static str,
                                     visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a tuple struct. This allows
    /// deserializers to parse sequences that aren't tagged as sequences.
    fn deserialize_tuple_struct<V>(&mut self,
                                   name: &'static str,
                                   len: usize,
                                   visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a struct. This allows
    /// deserializers to parse sequences that aren't tagged as maps.
    fn deserialize_struct<V>(&mut self,
                             name: &'static str,
                             fields: &'static [&'static str],
                             visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting some sort of struct field
    /// name.  This allows deserializers to choose between &str, usize, or &[u8] to properly
    /// deserialize a struct field.
    fn deserialize_struct_field<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting a tuple value. This allows
    /// deserializers that provide a custom tuple serialization to properly deserialize the type.
    fn deserialize_tuple<V>(&mut self, len: usize, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// This method hints that the `Deserialize` type is expecting an enum value. This allows
    /// deserializers that provide a custom enumeration serialization to properly deserialize the
    /// type.
    fn deserialize_enum<V>(&mut self,
                           name: &'static str,
                           variants: &'static [&'static str],
                           visitor: V) -> Result<V::Value, Self::Error>
        where V: EnumVisitor;

    /// This method hints that the `Deserialize` type needs to deserialize a value whose type
    /// doesn't matter because it is ignored.
    fn deserialize_ignored_any<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;
}

///////////////////////////////////////////////////////////////////////////////

/// This trait represents a visitor that walks through a deserializer.
pub trait Visitor {
    /// The value produced by this visitor.
    type Value: Deserialize;

    /// `visit_bool` deserializes a `bool` into a `Value`.
    fn visit_bool<E>(&mut self, v: bool) -> Result<Self::Value, E>
        where E: Error,
    {
        let _ = v;
        Err(Error::invalid_type(Type::Bool))
    }

    /// `visit_isize` deserializes a `isize` into a `Value`.
    fn visit_isize<E>(&mut self, v: isize) -> Result<Self::Value, E>
        where E: Error,
    {
        self.visit_i64(v as i64)
    }

    /// `visit_i8` deserializes a `i8` into a `Value`.
    fn visit_i8<E>(&mut self, v: i8) -> Result<Self::Value, E>
        where E: Error,
    {
        self.visit_i64(v as i64)
    }

    /// `visit_i16` deserializes a `i16` into a `Value`.
    fn visit_i16<E>(&mut self, v: i16) -> Result<Self::Value, E>
        where E: Error,
    {
        self.visit_i64(v as i64)
    }

    /// `visit_i32` deserializes a `i32` into a `Value`.
    fn visit_i32<E>(&mut self, v: i32) -> Result<Self::Value, E>
        where E: Error,
    {
        self.visit_i64(v as i64)
    }

    /// `visit_i64` deserializes a `i64` into a `Value`.
    fn visit_i64<E>(&mut self, v: i64) -> Result<Self::Value, E>
        where E: Error,
    {
        let _ = v;
        Err(Error::invalid_type(Type::I64))
    }

    /// `visit_usize` deserializes a `usize` into a `Value`.
    fn visit_usize<E>(&mut self, v: usize) -> Result<Self::Value, E>
        where E: Error,
    {
        self.visit_u64(v as u64)
    }

    /// `visit_u8` deserializes a `u8` into a `Value`.
    fn visit_u8<E>(&mut self, v: u8) -> Result<Self::Value, E>
        where E: Error,
    {
        self.visit_u64(v as u64)
    }

    /// `visit_u16` deserializes a `u16` into a `Value`.
    fn visit_u16<E>(&mut self, v: u16) -> Result<Self::Value, E>
        where E: Error,
    {
        self.visit_u64(v as u64)
    }

    /// `visit_u32` deserializes a `u32` into a `Value`.
    fn visit_u32<E>(&mut self, v: u32) -> Result<Self::Value, E>
        where E: Error,
    {
        self.visit_u64(v as u64)
    }

    /// `visit_u64` deserializes a `u64` into a `Value`.
    fn visit_u64<E>(&mut self, v: u64) -> Result<Self::Value, E>
        where E: Error,
    {
        let _ = v;
        Err(Error::invalid_type(Type::U64))
    }

    /// `visit_f32` deserializes a `f32` into a `Value`.
    fn visit_f32<E>(&mut self, v: f32) -> Result<Self::Value, E>
        where E: Error,
    {
        self.visit_f64(v as f64)
    }

    /// `visit_f64` deserializes a `f64` into a `Value`.
    fn visit_f64<E>(&mut self, v: f64) -> Result<Self::Value, E>
        where E: Error,
    {
        let _ = v;
        Err(Error::invalid_type(Type::F64))
    }

    /// `visit_char` deserializes a `char` into a `Value`.
    #[inline]
    fn visit_char<E>(&mut self, v: char) -> Result<Self::Value, E>
        where E: Error,
    {
        self.visit_str(::utils::encode_utf8(v).as_str())
    }

    /// `visit_str` deserializes a `&str` into a `Value`.
    fn visit_str<E>(&mut self, v: &str) -> Result<Self::Value, E>
        where E: Error,
    {
        let _ = v;
        Err(Error::invalid_type(Type::Str))
    }

    /// `visit_string` deserializes a `String` into a `Value`.  This allows a deserializer to avoid
    /// a copy if it is deserializing a string from a `String` type.  By default it passes a `&str`
    /// to the `visit_str` method.
    #[inline]
    #[cfg(any(feature = "std", feature = "collections"))]
    fn visit_string<E>(&mut self, v: String) -> Result<Self::Value, E>
        where E: Error,
    {
        self.visit_str(&v)
    }

    /// `visit_unit` deserializes a `()` into a `Value`.
    fn visit_unit<E>(&mut self) -> Result<Self::Value, E>
        where E: Error,
    {
        Err(Error::invalid_type(Type::Unit))
    }

    /// `visit_unit_struct` deserializes a unit struct into a `Value`.
    #[inline]
    fn visit_unit_struct<E>(&mut self, name: &'static str) -> Result<Self::Value, E>
        where E: Error,
    {
        let _ = name;
        self.visit_unit()
    }

    /// `visit_none` deserializes a none value into a `Value`.
    fn visit_none<E>(&mut self) -> Result<Self::Value, E>
        where E: Error,
    {
        Err(Error::invalid_type(Type::Option))
    }

    /// `visit_some` deserializes a value into a `Value`.
    fn visit_some<D>(&mut self, deserializer: &mut D) -> Result<Self::Value, D::Error>
        where D: Deserializer,
    {
        let _ = deserializer;
        Err(Error::invalid_type(Type::Option))
    }

    /// `visit_newtype_struct` deserializes a value into a `Value`.
    fn visit_newtype_struct<D>(&mut self, deserializer: &mut D) -> Result<Self::Value, D::Error>
        where D: Deserializer,
    {
        let _ = deserializer;
        Err(Error::invalid_type(Type::NewtypeStruct))
    }

    /// `visit_seq` deserializes a `SeqVisitor` into a `Value`.
    fn visit_seq<V>(&mut self, visitor: V) -> Result<Self::Value, V::Error>
        where V: SeqVisitor,
    {
        let _ = visitor;
        Err(Error::invalid_type(Type::Seq))
    }

    /// `visit_map` deserializes a `MapVisitor` into a `Value`.
    fn visit_map<V>(&mut self, visitor: V) -> Result<Self::Value, V::Error>
        where V: MapVisitor,
    {
        let _ = visitor;
        Err(Error::invalid_type(Type::Map))
    }

    /// `visit_bytes` deserializes a `&[u8]` into a `Value`.
    fn visit_bytes<E>(&mut self, v: &[u8]) -> Result<Self::Value, E>
        where E: Error,
    {
        let _ = v;
        Err(Error::invalid_type(Type::Bytes))
    }

    /// `visit_byte_buf` deserializes a `Vec<u8>` into a `Value`.
    #[cfg(any(feature = "std", feature = "collections"))]
    fn visit_byte_buf<E>(&mut self, v: Vec<u8>) -> Result<Self::Value, E>
        where E: Error,
    {
        self.visit_bytes(&v)
    }
}

///////////////////////////////////////////////////////////////////////////////

/// `SeqVisitor` visits each item in a sequence.
///
/// This is a trait that a `Deserializer` passes to a `Visitor` implementation, which deserializes
/// each item in a sequence.
pub trait SeqVisitor {
    /// The error type that can be returned if some error occurs during deserialization.
    type Error: Error;

    /// This returns a `Ok(Some(value))` for the next value in the sequence, or `Ok(None)` if there
    /// are no more remaining items.
    fn visit<T>(&mut self) -> Result<Option<T>, Self::Error>
        where T: Deserialize;

    /// This signals to the `SeqVisitor` that the `Visitor` does not expect any more items.
    fn end(&mut self) -> Result<(), Self::Error>;

    /// Return the lower and upper bound of items remaining in the sequence.
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }
}

impl<'a, V> SeqVisitor for &'a mut V where V: SeqVisitor {
    type Error = V::Error;

    #[inline]
    fn visit<T>(&mut self) -> Result<Option<T>, V::Error>
        where T: Deserialize
    {
        (**self).visit()
    }

    #[inline]
    fn end(&mut self) -> Result<(), V::Error> {
        (**self).end()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (**self).size_hint()
    }
}

///////////////////////////////////////////////////////////////////////////////

/// `MapVisitor` visits each item in a sequence.
///
/// This is a trait that a `Deserializer` passes to a `Visitor` implementation.
pub trait MapVisitor {
    /// The error type that can be returned if some error occurs during deserialization.
    type Error: Error;

    /// This returns a `Ok(Some((key, value)))` for the next (key-value) pair in the map, or
    /// `Ok(None)` if there are no more remaining items.
    #[inline]
    fn visit<K, V>(&mut self) -> Result<Option<(K, V)>, Self::Error>
        where K: Deserialize,
              V: Deserialize,
    {
        match try!(self.visit_key()) {
            Some(key) => {
                let value = try!(self.visit_value());
                Ok(Some((key, value)))
            }
            None => Ok(None)
        }
    }

    /// This returns a `Ok(Some(key))` for the next key in the map, or `Ok(None)` if there are no
    /// more remaining items.
    fn visit_key<K>(&mut self) -> Result<Option<K>, Self::Error>
        where K: Deserialize;

    /// This returns a `Ok(value)` for the next value in the map.
    fn visit_value<V>(&mut self) -> Result<V, Self::Error>
        where V: Deserialize;

    /// This signals to the `MapVisitor` that the `Visitor` does not expect any more items.
    fn end(&mut self) -> Result<(), Self::Error>;

    /// Return the lower and upper bound of items remaining in the sequence.
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }

    /// Report that the struct has a field that wasn't deserialized
    fn missing_field<V>(&mut self, field: &'static str) -> Result<V, Self::Error>
        where V: Deserialize,
    {
        Err(Error::missing_field(field))
    }
}

impl<'a, V_> MapVisitor for &'a mut V_ where V_: MapVisitor {
    type Error = V_::Error;

    #[inline]
    fn visit<K, V>(&mut self) -> Result<Option<(K, V)>, V_::Error>
        where K: Deserialize,
              V: Deserialize,
    {
        (**self).visit()
    }

    #[inline]
    fn visit_key<K>(&mut self) -> Result<Option<K>, V_::Error>
        where K: Deserialize
    {
        (**self).visit_key()
    }

    #[inline]
    fn visit_value<V>(&mut self) -> Result<V, V_::Error>
        where V: Deserialize
    {
        (**self).visit_value()
    }

    #[inline]
    fn end(&mut self) -> Result<(), V_::Error> {
        (**self).end()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (**self).size_hint()
    }
}

///////////////////////////////////////////////////////////////////////////////

/// `EnumVisitor` is a visitor that is created by the `Deserialize` and passed to the
/// `Deserializer` in order to deserialize enums.
pub trait EnumVisitor {
    /// The value produced by this visitor.
    type Value;

    /// Visit the specific variant with the `VariantVisitor`.
    fn visit<V>(&mut self, visitor: V) -> Result<Self::Value, V::Error>
        where V: VariantVisitor;
}

///////////////////////////////////////////////////////////////////////////////

/// `VariantVisitor` is a visitor that is created by the `Deserializer` and passed to the
/// `Deserialize` in order to deserialize a specific enum variant.
pub trait VariantVisitor {
    /// The error type that can be returned if some error occurs during deserialization.
    type Error: Error;

    /// `visit_variant` is called to identify which variant to deserialize.
    fn visit_variant<V>(&mut self) -> Result<V, Self::Error>
        where V: Deserialize;

    /// `visit_unit` is called when deserializing a variant with no values.
    fn visit_unit(&mut self) -> Result<(), Self::Error> {
        Err(Error::invalid_type(Type::UnitVariant))
    }

    /// `visit_newtype` is called when deserializing a variant with a single value.
    /// A good default is often to use the `visit_tuple` method to deserialize a `(value,)`.
    fn visit_newtype<T>(&mut self) -> Result<T, Self::Error>
        where T: Deserialize;

    /// `visit_tuple` is called when deserializing a tuple-like variant.
    /// If no tuple variants are expected, yield a
    /// `Err(serde::de::Error::invalid_type(serde::de::Type::TupleVariant))`
    fn visit_tuple<V>(&mut self,
                      len: usize,
                      visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;

    /// `visit_struct` is called when deserializing a struct-like variant.
    /// If no struct variants are expected, yield a
    /// `Err(serde::de::Error::invalid_type(serde::de::Type::StructVariant))`
    fn visit_struct<V>(&mut self,
                       fields: &'static [&'static str],
                       visitor: V) -> Result<V::Value, Self::Error>
        where V: Visitor;
}

impl<'a, T> VariantVisitor for &'a mut T where T: VariantVisitor {
    type Error = T::Error;

    fn visit_variant<V>(&mut self) -> Result<V, T::Error>
        where V: Deserialize
    {
        (**self).visit_variant()
    }

    fn visit_unit(&mut self) -> Result<(), T::Error> {
        (**self).visit_unit()
    }

    fn visit_newtype<D>(&mut self) -> Result<D, T::Error>
        where D: Deserialize,
    {
        (**self).visit_newtype()
    }

    fn visit_tuple<V>(&mut self,
                      len: usize,
                      visitor: V) -> Result<V::Value, T::Error>
        where V: Visitor,
    {
        (**self).visit_tuple(len, visitor)
    }

    fn visit_struct<V>(&mut self,
                       fields: &'static [&'static str],
                       visitor: V) -> Result<V::Value, T::Error>
        where V: Visitor,
    {
        (**self).visit_struct(fields, visitor)
    }
}

//! Serialize a Rust data structure into MessagePack data.

use std::error;
use std::fmt::{self, Display};
use std::io::Write;

use serde;
use serde::Serialize;
use serde::ser::{SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant,
                 SerializeTuple, SerializeTupleStruct, SerializeTupleVariant};

use rmp::{Marker, encode};
use rmp::encode::ValueWriteError;

use crate::config::{
    DefaultConfig, SerializerConfig, StructMapConfig, StructTupleConfig, VariantIntegerConfig,
    VariantStringConfig, HumanReadableConfig, BinaryConfig,
};
use crate::MSGPACK_EXT_STRUCT_NAME;

/// This type represents all possible errors that can occur when serializing or
/// deserializing MessagePack data.
#[derive(Debug)]
pub enum Error {
    /// Failed to write a MessagePack value.
    InvalidValueWrite(ValueWriteError),
    //TODO: This can be removed at some point
    /// Failed to serialize struct, sequence or map, because its length is unknown.
    UnknownLength,
    /// Invalid Data model, i.e. Serialize trait is not implmented correctly
    InvalidDataModel(&'static str),
    /// Depth limit exceeded
    DepthLimitExceeded,
    /// Catchall for syntax error messages.
    Syntax(String),
}

impl error::Error for Error {
    #[cold]
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match *self {
            Error::InvalidValueWrite(ref err) => Some(err),
            Error::UnknownLength => None,
            Error::InvalidDataModel(_) => None,
            Error::DepthLimitExceeded => None,
            Error::Syntax(..) => None,
        }
    }
}

impl Display for Error {
    #[cold]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match *self {
            Error::InvalidValueWrite(ref err) => write!(f, "invalid value write: {}", err),
            Error::UnknownLength => {
                f.write_str("attempt to serialize struct, sequence or map with unknown length")
            }
            Error::InvalidDataModel(r) => write!(f, "serialize data model is invalid: {}", r),
            Error::DepthLimitExceeded => f.write_str("depth limit exceeded"),
            Error::Syntax(ref msg) => f.write_str(msg),
        }
    }
}

impl From<ValueWriteError> for Error {
    #[cold]
    fn from(err: ValueWriteError) -> Error {
        Error::InvalidValueWrite(err)
    }
}

impl serde::ser::Error for Error {
    /// Raised when there is general error when deserializing a type.
    #[cold]
    fn custom<T: Display>(msg: T) -> Error {
        Error::Syntax(msg.to_string())
    }
}

/// Obtain the underlying writer.
pub trait UnderlyingWrite {
    /// Underlying writer type.
    type Write: Write;

    /// Gets a reference to the underlying writer.
    fn get_ref(&self) -> &Self::Write;

    /// Gets a mutable reference to the underlying writer.
    ///
    /// It is inadvisable to directly write to the underlying writer.
    fn get_mut(&mut self) -> &mut Self::Write;

    /// Unwraps this `Serializer`, returning the underlying writer.
    fn into_inner(self) -> Self::Write;
}

/// Represents MessagePack serialization implementation.
///
/// # Note
///
/// MessagePack has no specification about how to encode enum types. Thus we are free to do
/// whatever we want, so the given choice may be not ideal for you.
///
/// An enum value is represented as a single-entry map whose key is the variant
/// id and whose value is a sequence containing all associated data. If the enum
/// does not have associated data, the sequence is empty.
///
/// All instances of `ErrorKind::Interrupted` are handled by this function and the underlying
/// operation is retried.
// TODO: Docs. Examples.
#[derive(Debug)]
pub struct Serializer<W, C = DefaultConfig> {
    wr: W,
    config: C,
    depth: usize,
}


impl<W: Write, C> Serializer<W, C> {
    /// Gets a reference to the underlying writer.
    #[inline(always)]
    pub fn get_ref(&self) -> &W {
        &self.wr
    }

    /// Gets a mutable reference to the underlying writer.
    ///
    /// It is inadvisable to directly write to the underlying writer.
    #[inline(always)]
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.wr
    }

    /// Unwraps this `Serializer`, returning the underlying writer.
    #[inline(always)]
    pub fn into_inner(self) -> W {
        self.wr
    }

    /// Changes the maximum nesting depth that is allowed.
    ///
    /// Currently unused.
    #[doc(hidden)]
    #[inline]
    pub fn set_max_depth(&mut self, depth: usize) {
        self.depth = depth;
    }
}

impl<W: Write> Serializer<W, DefaultConfig> {
    /// Constructs a new `MessagePack` serializer whose output will be written to the writer
    /// specified.
    ///
    /// # Note
    ///
    /// This is the default constructor, which returns a serializer that will serialize structs
    /// and enums using the most compact representation.
    #[inline]
    pub fn new(wr: W) -> Self {
        Serializer {
            wr,
            depth: 1024,
            config: DefaultConfig,
        }
    }
}

impl<W: Write> Serializer<W, StructTupleConfig<DefaultConfig>> {
    #[deprecated(note = "use `Serializer::new` instead")]
    #[doc(hidden)]
    pub fn compact(wr: W) -> Self {
        Serializer::new(wr).with_struct_tuple()
    }
}

impl<W: Write> Serializer<W, StructMapConfig<DefaultConfig>> {
    #[deprecated(note = "use `Serializer::with_struct_map()` instead")]
    #[doc(hidden)]
    pub fn new_named(wr: W) -> Self {
        Serializer::new(wr).with_struct_map()
    }
}

impl<'a, W: Write + 'a, C> Serializer<W, C> {
    #[inline]
    fn compound(&'a mut self) -> Result<Compound<'a, W, C>, Error> {
        let c = Compound { se: self };
        Ok(c)
    }
}

impl<'a, W: Write + 'a, C: SerializerConfig> Serializer<W, C> {
    #[inline]
    fn maybe_unknown_len_compound<F>(&'a mut self, len: Option<usize>, f: F) -> Result<MaybeUnknownLengthCompound<'a, W, C>, Error>
    where F: Fn(&mut W, u32) -> Result<Marker, ValueWriteError>
    {
        Ok(MaybeUnknownLengthCompound {
            compound: match len {
                Some(len) => {
                    f(&mut self.wr, len as u32)?;
                    None
                }
                None => Some(UnknownLengthCompound::from(&*self))
            },
            se: self,
        })
    }
}

impl<W: Write, C> Serializer<W, C> {
    /// Consumes this serializer returning the new one, which will serialize structs as a map.
    ///
    /// This is used, when the default struct serialization as a tuple does not fit your
    /// requirements.
    #[inline]
    pub fn with_struct_map(self) -> Serializer<W, StructMapConfig<C>> {
        let Serializer { wr, depth, config } = self;
        Serializer {
            wr,
            depth,
            config: StructMapConfig::new(config),
        }
    }

    /// Consumes this serializer returning the new one, which will serialize structs as a tuple
    /// without field names.
    ///
    /// This is the default MessagePack serialization mechanism, emitting the most compact
    /// representation.
    #[inline]
    pub fn with_struct_tuple(self) -> Serializer<W, StructTupleConfig<C>> {
        let Serializer { wr, depth, config } = self;
        Serializer {
            wr,
            depth,
            config: StructTupleConfig::new(config),
        }
    }

    /// Consumes this serializer returning the new one, which will serialize enum variants as strings.
    ///
    /// This is used, when the default struct serialization as integers does not fit your
    /// requirements.
    #[inline]
    pub fn with_string_variants(self) -> Serializer<W, VariantStringConfig<C>> {
        let Serializer { wr, depth, config } = self;
        Serializer {
            wr,
            depth,
            config: VariantStringConfig::new(config),
        }
    }

    /// Consumes this serializer returning the new one, which will serialize enum variants as a their
    /// integer indices.
    ///
    /// This is the default MessagePack serialization mechanism, emitting the most compact
    /// representation.
    #[inline]
    pub fn with_integer_variants(self) -> Serializer<W, VariantIntegerConfig<C>> {
        let Serializer { wr, depth, config } = self;
        Serializer {
            wr,
            depth,
            config: VariantIntegerConfig::new(config),
        }
    }

    /// Consumes this serializer returning the new one, which will serialize some types in
    /// human-readable representations (`Serializer::is_human_readable` will return `true`). Note
    /// that the overall representation is still binary, but some types such as IP addresses will
    /// be saved as human-readable strings.
    ///
    /// This is primarily useful if you need to interoperate with serializations produced by older
    /// versions of `rmp-serde`.
    #[inline]
    pub fn with_human_readable(self) -> Serializer<W, HumanReadableConfig<C>> {
        let Serializer { wr, depth, config } = self;
        Serializer {
            wr,
            depth,
            config: HumanReadableConfig::new(config),
        }
    }

    /// Consumes this serializer returning the new one, which will serialize types as binary
    /// (`Serializer::is_human_readable` will return `false`).
    ///
    /// This is the default MessagePack serialization mechanism, emitting the most compact
    /// representation.
    #[inline]
    pub fn with_binary(self) -> Serializer<W, BinaryConfig<C>> {
        let Serializer { wr, depth, config } = self;
        Serializer {
            wr,
            depth,
            config: BinaryConfig::new(config),
        }
    }
}

impl<W: Write, C> UnderlyingWrite for Serializer<W, C> {
    type Write = W;

    #[inline(always)]
    fn get_ref(&self) -> &Self::Write {
        &self.wr
    }

    #[inline(always)]
    fn get_mut(&mut self) -> &mut Self::Write {
        &mut self.wr
    }

    #[inline(always)]
    fn into_inner(self) -> Self::Write {
        self.wr
    }
}

/// Part of serde serialization API.
#[derive(Debug)]
pub struct Compound<'a, W: 'a, C: 'a> {
    se: &'a mut Serializer<W, C>,
}

#[derive(Debug)]
#[allow(missing_docs)]
pub struct ExtFieldSerializer<'a, W> {
    wr: &'a mut W,
    tag: Option<i8>,
    finish: bool
}

/// Represents MessagePack serialization implementation for Ext.
#[derive(Debug)]
pub struct ExtSerializer<'a, W> {
    fields_se: ExtFieldSerializer<'a, W>,
    tuple_received: bool
}

impl<'a, W: Write + 'a, C: SerializerConfig> SerializeSeq for Compound<'a, W, C> {
    type Ok = ();
    type Error = Error;

    #[inline]
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        value.serialize(&mut *self.se)
    }

    #[inline(always)]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl<'a, W: Write + 'a, C: SerializerConfig> SerializeTuple for Compound<'a, W, C> {
    type Ok = ();
    type Error = Error;

    #[inline]
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        value.serialize(&mut *self.se)
    }

    #[inline(always)]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl<'a, W: Write + 'a, C: SerializerConfig> SerializeTupleStruct for Compound<'a, W, C> {
    type Ok = ();
    type Error = Error;

    #[inline]
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        value.serialize(&mut *self.se)
    }

    #[inline(always)]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl<'a, W: Write + 'a, C: SerializerConfig> SerializeStruct for Compound<'a, W, C> {
    type Ok = ();
    type Error = Error;

    #[inline]
    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, value: &T) ->
        Result<(), Self::Error>
    {
        C::write_struct_field(&mut *self.se, key, value)
    }

    #[inline(always)]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl<'a, W: Write + 'a, C: SerializerConfig> SerializeTupleVariant for Compound<'a, W, C> {
    type Ok = ();
    type Error = Error;

    #[inline]
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        value.serialize(&mut *self.se)
    }

    #[inline(always)]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl<'a, W: Write + 'a, C: SerializerConfig> SerializeStructVariant for Compound<'a, W, C> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, value: &T) ->
        Result<(), Self::Error>
    {
        C::write_struct_field(&mut *self.se, key, value)
    }

    #[inline(always)]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

/// Contains a `Serializer` for sequences and maps whose length is not yet known
/// and a counter for the number of elements that are encoded by the `Serializer`.
#[derive(Debug)]
struct UnknownLengthCompound<C> {
    se: Serializer<Vec<u8>, C>,
    elem_count: u32
}
impl<W, C: SerializerConfig> From<&Serializer<W, C>> for UnknownLengthCompound<C> {
    fn from(se: &Serializer<W, C>) -> Self {
        Self {
            se: Serializer { wr: Vec::with_capacity(128), config: se.config, depth: se.depth },
            elem_count: 0
        }
    }
}

/// Contains a `Serializer` for encoding elements of sequences and maps.
///
/// # Note
///
/// If , for example, a field inside a struct is tagged with `#serde(flatten)` the total number of
/// fields of this struct will be unknown to serde because flattened fields may have name clashes
/// and then will be overwritten. So, serde wants to serialize the struct as a map with an unknown
/// length.
///
/// For the described case a `UnknownLengthCompound` is used to encode the elements. On `end()`
/// the counted length and the encoded elements will be written to the `Serializer`. A caveat is,
/// that structs that contain flattened fields arem always written as a map, even when compact
/// representaion is desired.
///
/// Otherwise, if the length is known, the elements will be encoded directly by the `Serializer`.
#[derive(Debug)]
pub struct MaybeUnknownLengthCompound<'a, W: 'a, C: 'a> {
    se: &'a mut Serializer<W, C>,
    compound: Option<UnknownLengthCompound<C>>,
}

impl<'a, W: Write + 'a, C: SerializerConfig> SerializeSeq for MaybeUnknownLengthCompound<'a, W, C> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        match self.compound.as_mut() {
            None => value.serialize(&mut *self.se),
            Some(buf) =>  {
                value.serialize(&mut buf.se)?;
                buf.elem_count += 1;
                Ok(())
            }
        }
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        if let Some(compound) = self.compound {
            encode::write_array_len(&mut self.se.wr, compound.elem_count)?;
            self.se.wr.write_all(&compound.se.into_inner())
                .map_err(ValueWriteError::InvalidDataWrite)?;
        }
        Ok(())
    }
}

impl<'a, W: Write + 'a, C: SerializerConfig> SerializeMap for MaybeUnknownLengthCompound<'a, W, C> {
    type Ok = ();
    type Error = Error;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Self::Error> {
        <Self as SerializeSeq>::serialize_element(self, key)
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        <Self as SerializeSeq>::serialize_element(self, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        if let Some(compound) = self.compound {
            encode::write_map_len(&mut self.se.wr, compound.elem_count / 2)?;
            self.se.wr.write_all(&compound.se.into_inner())
                .map_err(ValueWriteError::InvalidDataWrite)?;
        }
        Ok(())
    }
}

impl<'a, W, C> serde::Serializer for &'a mut Serializer<W, C>
where
    W: Write,
    C: SerializerConfig,
{
    type Ok = ();
    type Error = Error;

    type SerializeSeq = MaybeUnknownLengthCompound<'a, W, C>;
    type SerializeTuple = Compound<'a, W, C>;
    type SerializeTupleStruct = Compound<'a, W, C>;
    type SerializeTupleVariant = Compound<'a, W, C>;
    type SerializeMap = MaybeUnknownLengthCompound<'a, W, C>;
    type SerializeStruct = Compound<'a, W, C>;
    type SerializeStructVariant = Compound<'a, W, C>;

    fn is_human_readable(&self) -> bool {
        C::is_human_readable()
    }

    fn serialize_bool(self, v: bool) -> Result<Self::Ok, Self::Error> {
        encode::write_bool(&mut self.wr, v)
            .map_err(|err| Error::InvalidValueWrite(ValueWriteError::InvalidMarkerWrite(err)))
    }

    fn serialize_i8(self, v: i8) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(v as i64)
    }

    fn serialize_i16(self, v: i16) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(v as i64)
    }

    fn serialize_i32(self, v: i32) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(v as i64)
    }

    fn serialize_i64(self, v: i64) -> Result<Self::Ok, Self::Error> {
        encode::write_sint(&mut self.wr, v)?;
        Ok(())
    }

	#[cfg(feature = "serde128")]
	fn serialize_i128(self, v: i128) -> Result<Self::Ok, Self::Error> {
		self.serialize_bytes(&v.to_be_bytes())
	}

    fn serialize_u8(self, v: u8) -> Result<Self::Ok, Self::Error> {
        self.serialize_u64(v as u64)
    }

    fn serialize_u16(self, v: u16) -> Result<Self::Ok, Self::Error> {
        self.serialize_u64(v as u64)
    }

    fn serialize_u32(self, v: u32) -> Result<Self::Ok, Self::Error> {
        self.serialize_u64(v as u64)
    }

    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error> {
        encode::write_uint(&mut self.wr, v)?;
        Ok(())
    }

	#[cfg(feature = "serde128")]
    fn serialize_u128(self, v: u128) -> Result<Self::Ok, Self::Error> {
        self.serialize_bytes(&v.to_be_bytes())
    }

    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        encode::write_f32(&mut self.wr, v)?;
        Ok(())
    }

    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        encode::write_f64(&mut self.wr, v)?;
        Ok(())
    }

    fn serialize_char(self, v: char) -> Result<Self::Ok, Self::Error> {
        // A char encoded as UTF-8 takes 4 bytes at most.
        let mut buf = [0; 4];
        self.serialize_str(v.encode_utf8(&mut buf))
    }

    fn serialize_str(self, v: &str) -> Result<Self::Ok, Self::Error> {
        encode::write_str(&mut self.wr, v)?;
        Ok(())
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Self::Ok, Self::Error> {
        encode::write_bin_len(&mut self.wr, value.len() as u32)?;
        self.wr
            .write_all(value)
            .map_err(|err| Error::InvalidValueWrite(ValueWriteError::InvalidDataWrite(err)))
    }

    fn serialize_none(self) -> Result<(), Self::Error> {
        self.serialize_unit()
    }

    fn serialize_some<T: ?Sized + serde::Serialize>(self, v: &T) -> Result<(), Self::Error> {
        v.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        encode::write_nil(&mut self.wr)
            .map_err(|err| Error::InvalidValueWrite(ValueWriteError::InvalidMarkerWrite(err)))
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        encode::write_array_len(&mut self.wr, 0)?;
        Ok(())
    }

    fn serialize_unit_variant(self, _name: &str, idx: u32, variant: &'static str) ->
        Result<Self::Ok, Self::Error>
    {
        // encode as a map from variant idx to nil, like: {idx => nil}
        encode::write_map_len(&mut self.wr, 1)?;
        C::write_variant_ident(self, idx, variant)?;
        self.serialize_unit()
    }

    fn serialize_newtype_struct<T: ?Sized + serde::Serialize>(self, name: &'static str, value: &T) -> Result<(), Self::Error> {
        if name == MSGPACK_EXT_STRUCT_NAME {
            let mut ext_se = ExtSerializer::new(self);
            value.serialize(&mut ext_se)?;

            return ext_se.end();
        }

        // Encode as if it's inner type.
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + serde::Serialize>(self, _name: &'static str, idx: u32, variant: &'static str, value: &T) -> Result<Self::Ok, Self::Error> {
        // encode as a map from variant idx to its attributed data, like: {idx => value}
        encode::write_map_len(&mut self.wr, 1)?;
        C::write_variant_ident(self, idx, variant)?;
        value.serialize(self)
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Error> {
        self.maybe_unknown_len_compound(len, |wr, len| encode::write_array_len(wr, len))
    }

    //TODO: normal compund
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        encode::write_array_len(&mut self.wr, len as u32)?;

        self.compound()
    }

    fn serialize_tuple_struct(self, _name: &'static str, len: usize) ->
        Result<Self::SerializeTupleStruct, Self::Error>
    {
        encode::write_array_len(&mut self.wr, len as u32)?;

        self.compound()
    }

    fn serialize_tuple_variant(self, _name: &'static str, idx: u32, variant: &'static str, len: usize) ->
        Result<Self::SerializeTupleVariant, Error>
    {
        // encode as a map from variant idx to a sequence of its attributed data, like: {idx => [v1,...,vN]}
        encode::write_map_len(&mut self.wr, 1)?;
        C::write_variant_ident(self, idx, variant)?;
        self.serialize_tuple(len)
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Error> {
        self.maybe_unknown_len_compound(len, |wr, len| encode::write_map_len(wr, len))
    }

    fn serialize_struct(self, _name: &'static str, len: usize) ->
        Result<Self::SerializeStruct, Self::Error>
    {
        C::write_struct_len(self, len)?;
        self.compound()
    }

    fn serialize_struct_variant(self, name: &'static str, id: u32, variant: &'static str, len: usize) ->
        Result<Self::SerializeStructVariant, Error>
    {
        // encode as a map from variant idx to a sequence of its attributed data, like: {idx => [v1,...,vN]}
        encode::write_map_len(&mut self.wr, 1)?;
        C::write_variant_ident(self, id, variant)?;
        self.serialize_struct(name, len)
    }
}

impl<'a, W: Write + 'a> serde::Serializer for &mut ExtFieldSerializer<'a, W> {
    type Ok = ();
    type Error = Error;

    type SerializeSeq = serde::ser::Impossible<(), Error>;
    type SerializeTuple = serde::ser::Impossible<(), Error>;
    type SerializeTupleStruct = serde::ser::Impossible<(), Error>;
    type SerializeTupleVariant = serde::ser::Impossible<(), Error>;
    type SerializeMap = serde::ser::Impossible<(), Error>;
    type SerializeStruct = serde::ser::Impossible<(), Error>;
    type SerializeStructVariant = serde::ser::Impossible<(), Error>;

    #[inline]
    fn serialize_i8(self, value: i8) -> Result<Self::Ok, Self::Error> {
        if self.tag.is_none() {
            self.tag.replace(value);
            Ok(())
        } else {
            Err(Error::InvalidDataModel("expected i8 and bytes, unexpected second i8"))
        }
    }

    #[inline]
    fn serialize_bytes(self, val: &[u8]) -> Result<Self::Ok, Self::Error> {
        if let Some(tag) = self.tag.take() {
            encode::write_ext_meta(self.wr, val.len() as u32, tag)?;
            self.wr
                .write_all(val)
                .map_err(|err| Error::InvalidValueWrite(ValueWriteError::InvalidDataWrite(err)))?;

            self.finish = true;

            Ok(())
        } else {
            Err(Error::InvalidDataModel("expected i8 and bytes, received bytes first"))
        }
    }

    #[inline]
    fn serialize_bool(self, _val: bool) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, bool unexpected"))
    }

    #[inline]
    fn serialize_i16(self, _val: i16) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, i16 unexpected"))
    }

    #[inline]
    fn serialize_i32(self, _val: i32) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, i32 unexpected"))
    }

    #[inline]
    fn serialize_i64(self, _val: i64) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, i64 unexpected"))
    }

    #[inline]
    fn serialize_u8(self, _val: u8) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, u8 unexpected"))
    }

    #[inline]
    fn serialize_u16(self, _val: u16) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, u16 unexpected"))
    }

    #[inline]
    fn serialize_u32(self, _val: u32) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, u32 unexpected"))
    }

    #[inline]
    fn serialize_u64(self, _val: u64) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, u64 unexpected"))
    }

    #[inline]
    fn serialize_f32(self, _val: f32) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, f32 unexpected"))
    }

    #[inline]
    fn serialize_f64(self, _val: f64) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, f64 unexpected"))
    }

    #[inline]
    fn serialize_char(self, _val: char) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, char unexpected"))
    }

    #[inline]
    fn serialize_str(self, _val: &str) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, str unexpected"))
    }

    #[inline]
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, unit unexpected"))
    }

    #[inline]
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, unit struct unexpected"))
    }

    #[inline]
    fn serialize_unit_variant(self, _name: &'static str, _idx: u32, _variant: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, unit variant unexpected"))
    }

    #[inline]
    fn serialize_newtype_struct<T: ?Sized>(self, _name: &'static str, _value: &T) -> Result<Self::Ok, Self::Error>
        where T: Serialize
    {
        Err(Error::InvalidDataModel("expected i8 and bytes, newtype struct unexpected"))
    }

    fn serialize_newtype_variant<T: ?Sized>(self, _name: &'static str, _idx: u32, _variant: &'static str, _value: &T) -> Result<Self::Ok, Self::Error>
        where T: Serialize
    {
        Err(Error::InvalidDataModel("expected i8 and bytes, newtype variant unexpected"))
    }

    #[inline]
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, none unexpected"))
    }

    #[inline]
    fn serialize_some<T: ?Sized>(self, _value: &T) -> Result<Self::Ok, Self::Error>
        where T: Serialize
    {
        Err(Error::InvalidDataModel("expected i8 and bytes, some unexpected"))
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, seq unexpected"))
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, tuple unexpected"))
    }

    fn serialize_tuple_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeTupleStruct, Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, tuple struct unexpected"))
    }

    fn serialize_tuple_variant(self, _name: &'static str, _idx: u32, _variant: &'static str, _len: usize) -> Result<Self::SerializeTupleVariant, Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, tuple variant unexpected"))
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, map unexpected"))
    }

    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeStruct, Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, struct unexpected"))
    }

    fn serialize_struct_variant(self, _name: &'static str, _idx: u32, _variant: &'static str, _len: usize) -> Result<Self::SerializeStructVariant, Error> {
        Err(Error::InvalidDataModel("expected i8 and bytes, struct variant unexpected"))
    }
}

impl<'a, W: Write + 'a> serde::ser::Serializer for &mut ExtSerializer<'a, W> {
    type Ok = ();
    type Error = Error;

    type SerializeSeq = serde::ser::Impossible<(), Error>;
    type SerializeTuple = Self;
    type SerializeTupleStruct = serde::ser::Impossible<(), Error>;
    type SerializeTupleVariant = serde::ser::Impossible<(), Error>;
    type SerializeMap = serde::ser::Impossible<(), Error>;
    type SerializeStruct = serde::ser::Impossible<(), Error>;
    type SerializeStructVariant = serde::ser::Impossible<(), Error>;


    #[cold]
    fn serialize_bytes(self, _val: &[u8]) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received bytes"))
    }

    #[cold]
    fn serialize_bool(self, _val: bool) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received bool"))
    }

    #[cold]
    fn serialize_i8(self, _value: i8) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received i8"))
    }

    #[cold]
    fn serialize_i16(self, _val: i16) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received i16"))
    }

    #[cold]
    fn serialize_i32(self, _val: i32) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received i32"))
    }

    #[cold]
    fn serialize_i64(self, _val: i64) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received i64"))
    }

    #[cold]
    fn serialize_u8(self, _val: u8) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received u8"))
    }

    #[cold]
    fn serialize_u16(self, _val: u16) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received u16"))
    }

    #[cold]
    fn serialize_u32(self, _val: u32) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received u32"))
    }

    #[cold]
    fn serialize_u64(self, _val: u64) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received u64"))
    }

    #[cold]
    fn serialize_f32(self, _val: f32) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received f32"))
    }

    #[cold]
    fn serialize_f64(self, _val: f64) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received f64"))
    }

    #[cold]
    fn serialize_char(self, _val: char) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received char"))
    }

    #[cold]
    fn serialize_str(self, _val: &str) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received str"))
    }

    #[cold]
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received unit"))
    }

    #[cold]
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received unit_struct"))
    }

    #[cold]
    fn serialize_unit_variant(self, _name: &'static str, _idx: u32, _variant: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received unit_variant"))
    }

    #[cold]
    fn serialize_newtype_struct<T: ?Sized>(self, _name: &'static str, _value: &T) -> Result<Self::Ok, Self::Error>
        where T: Serialize
    {
        Err(Error::InvalidDataModel("expected tuple, received newtype_struct"))
    }

    #[cold]
    fn serialize_newtype_variant<T: ?Sized>(self, _name: &'static str, _idx: u32, _variant: &'static str, _value: &T) -> Result<Self::Ok, Self::Error>
        where T: Serialize
    {
        Err(Error::InvalidDataModel("expected tuple, received newtype_variant"))
    }

    #[cold]
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received none"))
    }

    #[cold]
    fn serialize_some<T: ?Sized>(self, _value: &T) -> Result<Self::Ok, Self::Error>
        where T: Serialize
    {
        Err(Error::InvalidDataModel("expected tuple, received some"))
    }

    #[cold]
    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Err(Error::InvalidDataModel("expected tuple, received seq"))
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Error> {
        // FIXME check len
        self.tuple_received = true;

        Ok(self)
    }

    #[cold]
    fn serialize_tuple_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeTupleStruct, Error> {
        Err(Error::InvalidDataModel("expected tuple, received tuple_struct"))
    }

    #[cold]
    fn serialize_tuple_variant(self, _name: &'static str, _idx: u32, _variant: &'static str, _len: usize) -> Result<Self::SerializeTupleVariant, Error> {
        Err(Error::InvalidDataModel("expected tuple, received tuple_variant"))
    }

    #[cold]
    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Error> {
        Err(Error::InvalidDataModel("expected tuple, received map"))
    }

    #[cold]
    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeStruct, Error> {
        Err(Error::InvalidDataModel("expected tuple, received struct"))
    }

    #[cold]
    fn serialize_struct_variant(self, _name: &'static str, _idx: u32, _variant: &'static str, _len: usize) -> Result<Self::SerializeStructVariant, Error> {
        Err(Error::InvalidDataModel("expected tuple, received struct_variant"))
    }
}

impl<'a, W: Write + 'a> SerializeTuple for &mut ExtSerializer<'a, W> {
    type Ok = ();
    type Error = Error;

    #[inline]
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        value.serialize(&mut self.fields_se)
    }

    #[inline(always)]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}


impl<'a, W: Write + 'a> ExtSerializer<'a, W> {
    #[inline]
    fn new<C>(ser: &'a mut Serializer<W, C>) -> Self {
        Self {
            fields_se: ExtFieldSerializer::new(ser),
            tuple_received: false
        }
    }

    #[inline]
    fn end(self) -> Result<(), Error> {
        if !self.tuple_received {
            Err(Error::InvalidDataModel("expected tuple, received nothing"))
        } else {
            self.fields_se.end()
        }
    }
}

impl<'a, W: Write + 'a> ExtFieldSerializer<'a, W> {
    #[inline]
    fn new<C>(ser: &'a mut Serializer<W, C>) -> Self {
        Self {
           wr: UnderlyingWrite::get_mut(ser),
           tag: None,
           finish: false
        }
    }

    #[inline]
    fn end(self) -> Result<(), Error> {
        if self.finish {
            Ok(())
        } else {
            Err(Error::InvalidDataModel("expected i8 and bytes"))
        }
    }
}

/// Serialize the given data structure as MessagePack into the I/O stream.
/// This function uses compact representation - structures as arrays
///
/// Serialization can fail if `T`'s implementation of `Serialize` decides to fail.
#[inline]
pub fn write<W, T>(wr: &mut W, val: &T) -> Result<(), Error>
where
    W: Write + ?Sized,
    T: Serialize + ?Sized
{
    val.serialize(&mut Serializer::new(wr))
}

/// Serialize the given data structure as MessagePack into the I/O stream.
/// This function serializes structures as maps
///
/// Serialization can fail if `T`'s implementation of `Serialize` decides to fail.
pub fn write_named<W, T>(wr: &mut W, val: &T) -> Result<(), Error>
where
    W: Write + ?Sized,
    T: Serialize + ?Sized
{
    let mut se = Serializer::new(wr)
        .with_struct_map();
    val.serialize(&mut se)
}

/// Serialize the given data structure as a MessagePack byte vector.
/// This method uses compact representation, structs are serialized as arrays
///
/// Serialization can fail if `T`'s implementation of `Serialize` decides to fail.
#[inline]
pub fn to_vec<T>(val: &T) -> Result<Vec<u8>, Error>
where
    T: Serialize + ?Sized
{
    let mut wr = Vec::with_capacity(128);
    write(&mut wr, val)?;
    Ok(wr)
}

/// Serializes data structure into byte vector as a map
/// Resulting MessagePack message will contain field names
///
/// # Errors
///
/// Serialization can fail if `T`'s implementation of `Serialize` decides to fail.
#[inline]
pub fn to_vec_named<T>(val: &T) -> Result<Vec<u8>, Error>
where
    T: Serialize + ?Sized
{
    let mut wr = Vec::with_capacity(128);
    write_named(&mut wr, val)?;
    Ok(wr)
}

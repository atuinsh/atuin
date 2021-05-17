// Copyright 2012-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Support code for encoding and decoding types.
//!
//! In order to allow extensibility in both what types can be encoded and how
//! they are encoded, encoding and decoding are split into two part each. An
//! implementation of the Encodable trait knows how to turn a specific type into
//! a generic form, and then uses an implementation of the Encoder trait to turn
//! this into concrete output (such as a JSON string). Decoder and Decodable do
//! the same for decoding.

/*
Core encoding and decoding interfaces.
*/

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::path;
use std::rc::Rc;
use std::sync::Arc;
use std::marker::PhantomData;
use std::borrow::Cow;

use cap_capacity;

/// Trait for writing out an encoding when serializing.
///
/// This trait provides methods to encode basic types and generic forms of
/// collections.  Implementations of `Encodable` use it to perform the actual
/// encoding of a type.
///
/// It is unspecified what is done with the encoding - it could be stored in a
/// variable, or written directly to a file, for example.
///
/// Encoders can expect to only have a single "root" method call made on this
/// trait. Non-trivial types will call one of the collection-emitting methods,
/// passing a function that may call other methods on the trait, but once the
/// collection-emitting method has returned, encoding should be complete.
pub trait Encoder {
    /// The error type for method results.
    type Error;

    // Primitive types:
    /// Emit a nil value.
    ///
    /// For example, this might be stored as the null keyword in JSON.
    fn emit_nil(&mut self) -> Result<(), Self::Error>;

    /// Emit a usize value.
    fn emit_usize(&mut self, v: usize) -> Result<(), Self::Error>;

    /// Emit a u64 value.
    fn emit_u64(&mut self, v: u64) -> Result<(), Self::Error>;

    /// Emit a u32 value.
    fn emit_u32(&mut self, v: u32) -> Result<(), Self::Error>;

    /// Emit a u16 value.
    fn emit_u16(&mut self, v: u16) -> Result<(), Self::Error>;

    /// Emit a u8 value.
    fn emit_u8(&mut self, v: u8) -> Result<(), Self::Error>;

    /// Emit a isize value.
    fn emit_isize(&mut self, v: isize) -> Result<(), Self::Error>;

    /// Emit a i64 value.
    fn emit_i64(&mut self, v: i64) -> Result<(), Self::Error>;

    /// Emit a i32 value.
    fn emit_i32(&mut self, v: i32) -> Result<(), Self::Error>;

    /// Emit a i16 value.
    fn emit_i16(&mut self, v: i16) -> Result<(), Self::Error>;

    /// Emit a i8 value.
    fn emit_i8(&mut self, v: i8) -> Result<(), Self::Error>;

    /// Emit a bool value.
    ///
    /// For example, this might be stored as the true and false keywords in
    /// JSON.
    fn emit_bool(&mut self, v: bool) -> Result<(), Self::Error>;

    /// Emit a f64 value.
    fn emit_f64(&mut self, v: f64) -> Result<(), Self::Error>;

    /// Emit a f32 value.
    fn emit_f32(&mut self, v: f32) -> Result<(), Self::Error>;

    /// Emit a char value.
    ///
    /// Note that strings should be emitted using `emit_str`, not as a sequence
    /// of `emit_char` calls.
    fn emit_char(&mut self, v: char) -> Result<(), Self::Error>;

    /// Emit a string value.
    fn emit_str(&mut self, v: &str) -> Result<(), Self::Error>;

    // Compound types:
    /// Emit an enumeration value.
    ///
    /// * `name` indicates the enumeration type name.
    /// * `f` is a function that will call `emit_enum_variant` or
    ///   `emit_enum_struct_variant` as appropriate to write the actual value.
    fn emit_enum<F>(&mut self, name: &str, f: F) -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit a enumeration variant value with no or unnamed data.
    ///
    /// This should only be called from a function passed to `emit_enum`.
    /// Variants with named data should use `emit_enum_struct_variant`.
    ///
    /// * `v_name` is the variant name
    /// * `v_id` is the numeric identifier for the variant.
    /// * `len` is the number of data items associated with the variant.
    /// * `f` is a function that will call `emit_enum_variant_arg` for each data
    ///   item. It may not be called if len is 0.
    ///
    /// # Examples
    ///
    /// ```
    /// use rustc_serialize::Encodable;
    /// use rustc_serialize::Encoder;
    ///
    /// enum Message {
    ///     Quit,
    ///     ChangeColor(i32, i32, i32),
    /// }
    ///
    /// impl Encodable for Message {
    ///     fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
    ///         s.emit_enum("Message", |s| {
    ///             match *self {
    ///                 Message::Quit => {
    ///                     s.emit_enum_variant("Quit", 0, 0, |s| Ok(()))
    ///                 }
    ///                 Message::ChangeColor(r, g, b) => {
    ///                     s.emit_enum_variant("ChangeColor", 1, 3, |s| {
    ///                         try!(s.emit_enum_variant_arg(0, |s| {
    ///                             s.emit_i32(r)
    ///                         }));
    ///                         try!(s.emit_enum_variant_arg(1, |s| {
    ///                             s.emit_i32(g)
    ///                         }));
    ///                         try!(s.emit_enum_variant_arg(2, |s| {
    ///                             s.emit_i32(b)
    ///                         }));
    ///                         Ok(())
    ///                     })
    ///                 }
    ///             }
    ///         })
    ///     }
    /// }
    /// ```
    fn emit_enum_variant<F>(&mut self, v_name: &str,
                            v_id: usize,
                            len: usize,
                            f: F) -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit an unnamed data item for an enumeration variant.
    ///
    /// This should only be called from a function passed to
    /// `emit_enum_variant`.
    ///
    /// * `a_idx` is the (zero-based) index of the data item.
    /// * `f` is a function that will call the appropriate emit method to encode
    ///   the data object.
    ///
    /// Note that variant data items must be emitted in order - starting with
    /// index `0` and finishing with index `len-1`.
    fn emit_enum_variant_arg<F>(&mut self, a_idx: usize, f: F)
                                -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit a enumeration variant value with no or named data.
    ///
    /// This should only be called from a function passed to `emit_enum`.
    /// Variants with unnamed data should use `emit_enum_variant`.
    ///
    /// * `v_name` is the variant name.
    /// * `v_id` is the numeric identifier for the variant.
    /// * `len` is the number of data items associated with the variant.
    /// * `f` is a function that will call `emit_enum_struct_variant_field` for
    ///   each data item. It may not be called if `len` is `0`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rustc_serialize::Encodable;
    /// use rustc_serialize::Encoder;
    ///
    /// enum Message {
    ///     Quit,
    ///     Move { x: i32, y: i32 },
    /// }
    ///
    /// impl Encodable for Message {
    ///     fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
    ///         s.emit_enum("Message", |s| {
    ///             match *self {
    ///                 Message::Quit => {
    ///                     s.emit_enum_struct_variant("Quit", 0, 0, |s| Ok(()))
    ///                 }
    ///                 Message::Move { x: x, y: y } => {
    ///                     s.emit_enum_struct_variant("Move", 1, 2, |s| {
    ///                         try!(s.emit_enum_struct_variant_field("x", 0, |s| {
    ///                             s.emit_i32(x)
    ///                         }));
    ///                         try!(s.emit_enum_struct_variant_field("y", 1, |s| {
    ///                             s.emit_i32(y)
    ///                         }));
    ///                         Ok(())
    ///                     })
    ///                 }
    ///             }
    ///         })
    ///     }
    /// }
    /// ```
    fn emit_enum_struct_variant<F>(&mut self, v_name: &str,
                                   v_id: usize,
                                   len: usize,
                                   f: F) -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit a named data item for an enumeration variant.
    ///
    /// This should only be called from a function passed to
    /// `emit_enum_struct_variant`.
    ///
    /// * `f_name` is the name of the data item field.
    /// * `f_idx` is its (zero-based) index.
    /// * `f` is a function that will call the appropriate emit method to encode
    ///   the data object.
    ///
    /// Note that fields must be emitted in order - starting with index `0` and
    /// finishing with index `len-1`.
    fn emit_enum_struct_variant_field<F>(&mut self,
                                         f_name: &str,
                                         f_idx: usize,
                                         f: F) -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit a struct value.
    ///
    /// * `name` is the name of the struct.
    /// * `len` is the number of members.
    /// * `f` is a function that calls `emit_struct_field` for each member.
    ///
    /// # Examples
    ///
    /// ```
    /// use rustc_serialize::Encodable;
    /// use rustc_serialize::Encoder;
    ///
    /// struct Point {
    ///     x: i32,
    ///     y: i32,
    /// }
    ///
    /// impl Encodable for Point {
    ///     fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
    ///         s.emit_struct("Point", 2, |s| {
    ///             try!(s.emit_struct_field("x", 0, |s| {
    ///                 s.emit_i32(self.x)
    ///             }));
    ///             try!(s.emit_struct_field("y", 1, |s| {
    ///                 s.emit_i32(self.y)
    ///             }));
    ///             Ok(())
    ///         })
    ///     }
    /// }
    /// ```
    fn emit_struct<F>(&mut self, name: &str, len: usize, f: F)
                      -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;
    /// Emit a field item for a struct.
    ///
    /// This should only be called from a function passed to `emit_struct`.
    ///
    /// * `f_name` is the name of the data item field.
    /// * `f_idx` is its (zero-based) index.
    /// * `f` is a function that will call the appropriate emit method to encode
    ///   the data object.
    ///
    /// Note that fields must be emitted in order - starting with index `0` and
    /// finishing with index `len-1`.
    fn emit_struct_field<F>(&mut self, f_name: &str, f_idx: usize, f: F)
                            -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit a tuple value.
    ///
    /// * `len` is the number of items in the tuple.
    /// * `f` is a function that calls `emit_tuple_arg` for each member.
    ///
    /// Note that external `Encodable` implementations should not normally need
    /// to use this method directly; it is meant for the use of this module's
    /// own implementation of `Encodable` for tuples.
    fn emit_tuple<F>(&mut self, len: usize, f: F) -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit a data item for a tuple.
    ///
    /// This should only be called from a function passed to `emit_tuple`.
    ///
    /// * `idx` is the (zero-based) index of the data item.
    /// * `f` is a function that will call the appropriate emit method to encode
    ///   the data object.
    ///
    /// Note that tuple items must be emitted in order - starting with index `0`
    /// and finishing with index `len-1`.
    fn emit_tuple_arg<F>(&mut self, idx: usize, f: F) -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit a tuple struct value.
    ///
    /// * `name` is the name of the tuple struct.
    /// * `len` is the number of items in the tuple struct.
    /// * `f` is a function that calls `emit_tuple_struct_arg` for each member.
    ///
    /// # Examples
    ///
    /// ```
    /// use rustc_serialize::Encodable;
    /// use rustc_serialize::Encoder;
    ///
    /// struct Pair(i32,i32);
    ///
    /// impl Encodable for Pair {
    ///     fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
    ///         let Pair(first,second) = *self;
    ///         s.emit_tuple_struct("Pair", 2, |s| {
    ///             try!(s.emit_tuple_arg(0, |s| {
    ///                 s.emit_i32(first)
    ///             }));
    ///             try!(s.emit_tuple_arg(1, |s| {
    ///                 s.emit_i32(second)
    ///             }));
    ///             Ok(())
    ///         })
    ///     }
    /// }
    /// ```
    fn emit_tuple_struct<F>(&mut self, name: &str, len: usize, f: F)
                            -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit a data item for a tuple struct.
    ///
    /// This should only be called from a function passed to
    /// `emit_tuple_struct`.
    ///
    /// * `f_idx` is the (zero-based) index of the data item.
    /// * `f` is a function that will call the appropriate emit method to encode
    ///   the data object.
    ///
    /// Note that tuple items must be emitted in order - starting with index `0`
    /// and finishing with index `len-1`.
    fn emit_tuple_struct_arg<F>(&mut self, f_idx: usize, f: F)
                                -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    // Specialized types:
    /// Emit an optional value.
    ///
    /// `f` is a function that will call either `emit_option_none` or
    /// `emit_option_some` as appropriate.
    ///
    /// This method allows encoders to handle `Option<T>` values specially,
    /// rather than using the generic enum methods, because many encoding
    /// formats have a built-in "optional" concept.
    ///
    /// Note that external `Encodable` implementations should not normally need
    /// to use this method directly; it is meant for the use of this module's
    /// own implementation of `Encodable` for `Option<T>`.
    fn emit_option<F>(&mut self, f: F) -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit the `None` optional value.
    ///
    /// This should only be called from a function passed to `emit_option`.
    fn emit_option_none(&mut self) -> Result<(), Self::Error>;

    /// Emit the `Some(x)` optional value.
    ///
    /// `f` is a function that will call the appropriate emit method to encode
    /// the data object.
    ///
    /// This should only be called from a function passed to `emit_option`.
    fn emit_option_some<F>(&mut self, f: F) -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit a sequence of values.
    ///
    /// This should be used for both array-like ordered sequences and set-like
    /// unordered ones.
    ///
    /// * `len` is the number of values in the sequence.
    /// * `f` is a function that will call `emit_seq_elt` for each value in the
    ///   sequence.
    fn emit_seq<F>(&mut self, len: usize, f: F) -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit an element in a sequence.
    ///
    /// This should only be called from a function passed to `emit_seq`.
    ///
    /// * `idx` is the (zero-based) index of the value in the sequence.
    /// * `f` is a function that will call the appropriate emit method to encode
    ///   the data object.
    ///
    /// Note that sequence elements must be emitted in order - starting with
    /// index `0` and finishing with index `len-1`.
    fn emit_seq_elt<F>(&mut self, idx: usize, f: F) -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit an associative container (map).
    ///
    /// * `len` is the number of entries in the map.
    /// * `f` is a function that will call `emit_map_elt_key` and
    ///   `emit_map_elt_val` for each entry in the map.
    ///
    /// # Examples
    ///
    /// ```
    /// use rustc_serialize::Encodable;
    /// use rustc_serialize::Encoder;
    ///
    /// struct SimpleMap<K,V> {
    ///     entries: Vec<(K,V)>,
    /// }
    ///
    /// impl<K:Encodable,V:Encodable> Encodable for SimpleMap<K,V> {
    ///     fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
    ///         s.emit_map(self.entries.len(), |s| {
    ///             for (i, e) in self.entries.iter().enumerate() {
    ///                 let (ref k, ref v) = *e;
    ///                 try!(s.emit_map_elt_key(i, |s| k.encode(s)));
    ///                 try!(s.emit_map_elt_val(i, |s| v.encode(s)));
    ///             }
    ///             Ok(())
    ///         })
    ///     }
    /// }
    /// ```
    fn emit_map<F>(&mut self, len: usize, f: F) -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit the key for an entry in a map.
    ///
    /// This should only be called from a function passed to `emit_map`.
    ///
    /// * `idx` is the (zero-based) index of the entry in the map
    /// * `f` is a function that will call the appropriate emit method to encode
    ///   the key.
    ///
    /// Note that map entries must be emitted in order - starting with index `0`
    /// and finishing with index `len-1` - and for each entry, the key should be
    /// emitted followed immediately by the value.
    fn emit_map_elt_key<F>(&mut self, idx: usize, f: F) -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;

    /// Emit the value for an entry in a map.
    ///
    /// This should only be called from a function passed to `emit_map`.
    ///
    /// * `idx` is the (zero-based) index of the entry in the map
    /// * `f` is a function that will call the appropriate emit method to encode
    ///   the value.
    ///
    /// Note that map entries must be emitted in order - starting with index `0`
    /// and finishing with index `len-1` - and for each entry, the key should be
    /// emitted followed immediately by the value.
    fn emit_map_elt_val<F>(&mut self, idx: usize, f: F) -> Result<(), Self::Error>
        where F: FnOnce(&mut Self) -> Result<(), Self::Error>;
}

/// Trait for reading in an encoding for deserialization.
///
/// This trait provides methods to decode basic types and generic forms of
/// collections.  Implementations of `Decodable` use it to perform the actual
/// decoding of a type.
///
/// Note that, as is typical with deserialization, the design of this API
/// assumes you know in advance the form of the data you are decoding (ie: what
/// type is encoded).
///
/// Decoders can expect to only have a single "root" method call made on this
/// trait. Non-trivial types will call one of the collection-reading methods,
/// passing a function that may call other methods on the trait, but once the
/// collection-reading method has returned, decoding should be complete.
pub trait Decoder {
    /// The error type for method results.
    type Error;

    // Primitive types:
    /// Read a nil value.
    fn read_nil(&mut self) -> Result<(), Self::Error>;

    /// Read a usize value.
    fn read_usize(&mut self) -> Result<usize, Self::Error>;

    /// Read a u64 value.
    fn read_u64(&mut self) -> Result<u64, Self::Error>;

    /// Read a u32 value.
    fn read_u32(&mut self) -> Result<u32, Self::Error>;

    /// Read a u16 value.
    fn read_u16(&mut self) -> Result<u16, Self::Error>;

    /// Read a u8 value.
    fn read_u8(&mut self) -> Result<u8, Self::Error>;

    /// Read a isize value.
    fn read_isize(&mut self) -> Result<isize, Self::Error>;

    /// Read a i64 value.
    fn read_i64(&mut self) -> Result<i64, Self::Error>;

    /// Read a i32 value.
    fn read_i32(&mut self) -> Result<i32, Self::Error>;

    /// Read a i16 value.
    fn read_i16(&mut self) -> Result<i16, Self::Error>;

    /// Read a i8 value.
    fn read_i8(&mut self) -> Result<i8, Self::Error>;

    /// Read a bool value.
    fn read_bool(&mut self) -> Result<bool, Self::Error>;

    /// Read a f64 value.
    fn read_f64(&mut self) -> Result<f64, Self::Error>;

    /// Read a f32 value.
    fn read_f32(&mut self) -> Result<f32, Self::Error>;

    /// Read a char value.
    fn read_char(&mut self) -> Result<char, Self::Error>;

    /// Read a string value.
    fn read_str(&mut self) -> Result<String, Self::Error>;

    // Compound types:
    /// Read an enumeration value.
    ///
    /// * `name` indicates the enumeration type name. It may be used to
    ///   sanity-check the data being read.
    /// * `f` is a function that will call `read_enum_variant` (or
    ///   `read_enum_struct_variant`) to read the actual value.
    fn read_enum<T, F>(&mut self, name: &str, f: F) -> Result<T, Self::Error>
        where F: FnOnce(&mut Self) -> Result<T, Self::Error>;

    /// Read an enumeration value.
    ///
    /// * `names` is a list of the enumeration variant names.
    /// * `f` is a function that will call `read_enum_variant_arg` or
    ///   `read_enum_struct_variant_field` as appropriate to read the
    ///   associated values. It will be passed the index into `names` for the
    ///   variant that is encoded.
    fn read_enum_variant<T, F>(&mut self, names: &[&str], f: F)
                               -> Result<T, Self::Error>
        where F: FnMut(&mut Self, usize) -> Result<T, Self::Error>;

    /// Read an unnamed data item for an enumeration variant.
    ///
    /// This should only be called from a function passed to `read_enum_variant`
    /// or `read_enum_struct_variant`, and only when the index provided to that
    /// function indicates that the variant has associated unnamed data. It
    /// should be called once for each associated data item.
    ///
    /// * `a_idx` is the (zero-based) index of the data item.
    /// * `f` is a function that will call the appropriate read method to deocde
    ///   the data object.
    ///
    /// Note that variant data items must be read in order - starting with index
    /// `0` and finishing with index `len-1`. Implementations may use `a_idx`,
    /// the call order or both to select the correct data to decode.
    fn read_enum_variant_arg<T, F>(&mut self, a_idx: usize, f: F)
                                   -> Result<T, Self::Error>
        where F: FnOnce(&mut Self) -> Result<T, Self::Error>;

    /// Read an enumeration value.
    ///
    /// This is identical to `read_enum_variant`, and is only provided for
    /// symmetry with the `Encoder` API.
    fn read_enum_struct_variant<T, F>(&mut self, names: &[&str], f: F)
                                      -> Result<T, Self::Error>
        where F: FnMut(&mut Self, usize) -> Result<T, Self::Error>;

    /// Read a named data item for an enumeration variant.
    ///
    /// This should only be called from a function passed to `read_enum_variant`
    /// or `read_enum_struct_variant`, and only when the index provided to that
    /// function indicates that the variant has associated named data. It should
    /// be called once for each associated field.
    ///
    /// * `f_name` is the name of the field.
    /// * `f_idx` is the (zero-based) index of the data item.
    /// * `f` is a function that will call the appropriate read method to deocde
    ///   the data object.
    ///
    /// Note that fields must be read in order - starting with index `0` and
    /// finishing with index `len-1`. Implementations may use `f_idx`, `f_name`,
    /// the call order or any combination to choose the correct data to decode,
    /// and may (but are not required to) return an error if these are
    /// inconsistent.
    fn read_enum_struct_variant_field<T, F>(&mut self,
                                            f_name: &str,
                                            f_idx: usize,
                                            f: F)
                                            -> Result<T, Self::Error>
        where F: FnOnce(&mut Self) -> Result<T, Self::Error>;

    /// Read an struct value.
    ///
    /// * `s_name` indicates the struct type name. It may be used to
    ///   sanity-check the data being read.
    /// * `len` indicates the number of fields in the struct.
    /// * `f` is a function that will call `read_struct_field` for each field in
    ///   the struct.
    fn read_struct<T, F>(&mut self, s_name: &str, len: usize, f: F)
                         -> Result<T, Self::Error>
        where F: FnOnce(&mut Self) -> Result<T, Self::Error>;

    /// Read a field for a struct value.
    ///
    /// This should only be called from a function passed to `read_struct`. It
    /// should be called once for each associated field.
    ///
    /// * `f_name` is the name of the field.
    /// * `f_idx` is the (zero-based) index of the data item.
    /// * `f` is a function that will call the appropriate read method to deocde
    ///   the data object.
    ///
    /// Note that fields must be read in order - starting with index `0` and
    /// finishing with index `len-1`. Implementations may use `f_idx`, `f_name`,
    /// the call order or any combination to choose the correct data to decode,
    /// and may (but are not required to) return an error if these are
    /// inconsistent.
    fn read_struct_field<T, F>(&mut self,
                               f_name: &str,
                               f_idx: usize,
                               f: F)
                               -> Result<T, Self::Error>
        where F: FnOnce(&mut Self) -> Result<T, Self::Error>;

    /// Read a tuple value.
    ///
    /// * `len` is the number of items in the tuple.
    /// * `f` is a function that will call `read_tuple_arg` for each item in the
    ///   tuple.
    ///
    /// Note that external `Decodable` implementations should not normally need
    /// to use this method directly; it is meant for the use of this module's
    /// own implementation of `Decodable` for tuples.
    fn read_tuple<T, F>(&mut self, len: usize, f: F) -> Result<T, Self::Error>
        where F: FnOnce(&mut Self) -> Result<T, Self::Error>;

    /// Read a data item for a tuple.
    ///
    /// This should only be called from a function passed to `read_tuple`.
    ///
    /// * `a_idx` is the (zero-based) index of the data item.
    /// * `f` is a function that will call the appropriate read method to encode
    ///   the data object.
    ///
    /// Note that tuple items must be read in order - starting with index `0`
    /// and finishing with index `len-1`.
    fn read_tuple_arg<T, F>(&mut self, a_idx: usize, f: F)
                            -> Result<T, Self::Error>
        where F: FnOnce(&mut Self) -> Result<T, Self::Error>;

    /// Read a tuple struct value.
    ///
    /// * `s_name` is the name of the tuple struct.
    /// * `len` is the number of items in the tuple struct.
    /// * `f` is a function that calls `read_tuple_struct_arg` for each member.
    fn read_tuple_struct<T, F>(&mut self, s_name: &str, len: usize, f: F)
                               -> Result<T, Self::Error>
        where F: FnOnce(&mut Self) -> Result<T, Self::Error>;

    /// Read a data item for a tuple struct.
    ///
    /// This should only be called from a function passed to
    /// `read_tuple_struct`.
    ///
    /// * `a_idx` is the (zero-based) index of the data item.
    /// * `f` is a function that will call the appropriate read method to encode
    ///   the data object.
    ///
    /// Note that tuple struct items must be read in order - starting with index
    /// `0` and finishing with index `len-1`.
    fn read_tuple_struct_arg<T, F>(&mut self, a_idx: usize, f: F)
                                   -> Result<T, Self::Error>
        where F: FnOnce(&mut Self) -> Result<T, Self::Error>;

    // Specialized types:
    /// Read an optional value.
    ///
    /// `f` is a function that will will be passed be passed `false` if the
    /// value is unset, and `true` if it is set. If the function is passed
    /// `true`, it will call the appropriate read methods to read the associated
    /// data type.
    ///
    /// This method allows decoders to handle `Option<T>` values specially,
    /// rather than using the generic enum methods, because many encoding
    /// formats have a built-in "optional" concept.
    ///
    /// Note that external `Decodable` implementations should not normally need
    /// to use this method directly; it is meant for the use of this module's
    /// own implementation of `Decodable` for `Option<T>`.
    fn read_option<T, F>(&mut self, f: F) -> Result<T, Self::Error>
        where F: FnMut(&mut Self, bool) -> Result<T, Self::Error>;

    /// Read a sequence of values.
    ///
    /// This should be used for both array-like ordered sequences and set-like
    /// unordered ones.
    ///
    /// * `f` is a function that will be passed the length of the sequence, and
    ///   will call `read_seq_elt` for each value in the sequence.
    fn read_seq<T, F>(&mut self, f: F) -> Result<T, Self::Error>
        where F: FnOnce(&mut Self, usize) -> Result<T, Self::Error>;

    /// Read an element in the sequence.
    ///
    /// This should only be called from a function passed to `read_seq`.
    ///
    /// * `idx` is the (zero-based) index of the value in the sequence.
    /// * `f` is a function that will call the appropriate read method to decode
    ///   the data object.
    ///
    /// Note that sequence elements must be read in order - starting with index
    /// `0` and finishing with index `len-1`.
    fn read_seq_elt<T, F>(&mut self, idx: usize, f: F) -> Result<T, Self::Error>
        where F: FnOnce(&mut Self) -> Result<T, Self::Error>;

    /// Read an associative container (map).
    ///
    /// * `f` is a function that will be passed the number of entries in the
    ///   map, and will call `read_map_elt_key` and `read_map_elt_val` to decode
    ///   each entry.
    fn read_map<T, F>(&mut self, f: F) -> Result<T, Self::Error>
        where F: FnOnce(&mut Self, usize) -> Result<T, Self::Error>;

    /// Read the key for an entry in a map.
    ///
    /// This should only be called from a function passed to `read_map`.
    ///
    /// * `idx` is the (zero-based) index of the entry in the map
    /// * `f` is a function that will call the appropriate read method to decode
    ///   the key.
    ///
    /// Note that map entries must be read in order - starting with index `0`
    /// and finishing with index `len-1` - and for each entry, the key should be
    /// read followed immediately by the value.
    fn read_map_elt_key<T, F>(&mut self, idx: usize, f: F)
                              -> Result<T, Self::Error>
        where F: FnOnce(&mut Self) -> Result<T, Self::Error>;

    /// Read the value for an entry in a map.
    ///
    /// This should only be called from a function passed to `read_map`.
    ///
    /// * `idx` is the (zero-based) index of the entry in the map
    /// * `f` is a function that will call the appropriate read method to decode
    ///   the value.
    ///
    /// Note that map entries must be read in order - starting with index `0`
    /// and finishing with index `len-1` - and for each entry, the key should be
    /// read followed immediately by the value.
    fn read_map_elt_val<T, F>(&mut self, idx: usize, f: F)
                              -> Result<T, Self::Error>
        where F: FnOnce(&mut Self) -> Result<T, Self::Error>;

    // Failure
    /// Record a decoding error.
    ///
    /// This allows `Decodable` implementations to report an error using a
    /// `Decoder` implementation's error type when inconsistent data is read.
    /// For example, when reading a fixed-length array and the wrong length is
    /// given by `read_seq`.
    fn error(&mut self, err: &str) -> Self::Error;
}

/// Trait for serializing a type.
///
/// This can be implemented for custom data types to allow them to be encoded
/// with `Encoder` implementations. Most of Rust's built-in or standard data
/// types (like `i32` and `Vec<T>`) have `Encodable` implementations provided by
/// this module.
///
/// Note that, in general, you should let the compiler implement this for you by
/// using the `derive(RustcEncodable)` attribute.
///
/// # Examples
///
/// ```rust
/// extern crate rustc_serialize;
///
/// #[derive(RustcEncodable)]
/// struct Point {
///     x: i32,
///     y: i32,
/// }
/// # fn main() {}
/// ```
///
/// This generates code equivalent to:
///
/// ```rust
/// extern crate rustc_serialize;
/// use rustc_serialize::Encodable;
/// use rustc_serialize::Encoder;
///
/// struct Point {
///     x: i32,
///     y: i32,
/// }
///
/// impl Encodable for Point {
///     fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
///         s.emit_struct("Point", 2, |s| {
///             try!(s.emit_struct_field("x", 0, |s| {
///                 s.emit_i32(self.x)
///             }));
///             try!(s.emit_struct_field("y", 1, |s| {
///                 s.emit_i32(self.y)
///             }));
///             Ok(())
///         })
///     }
/// }
/// # fn main() {}
/// ```
pub trait Encodable {
    /// Serialize a value using an `Encoder`.
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error>;
}

/// Trait for deserializing a type.
///
/// This can be implemented for custom data types to allow them to be decoded
/// with `Decoder` implementations. Most of Rust's built-in or standard data
/// types (like `i32` and `Vec<T>`) have `Decodable` implementations provided by
/// this module.
///
/// Note that, in general, you should let the compiler implement this for you by
/// using the `derive(RustcDecodable)` attribute.
///
/// # Examples
///
/// ```rust
/// extern crate rustc_serialize;
///
/// #[derive(RustcDecodable)]
/// struct Point {
///     x: i32,
///     y: i32,
/// }
/// # fn main() {}
/// ```
///
/// This generates code equivalent to:
///
/// ```rust
/// extern crate rustc_serialize;
/// use rustc_serialize::Decodable;
/// use rustc_serialize::Decoder;
///
/// struct Point {
///     x: i32,
///     y: i32,
/// }
///
/// impl Decodable for Point {
///     fn decode<D: Decoder>(d: &mut D) -> Result<Point, D::Error> {
///         d.read_struct("Point", 2, |d| {
///             let x = try!(d.read_struct_field("x", 0, |d| { d.read_i32() }));
///             let y = try!(d.read_struct_field("y", 1, |d| { d.read_i32() }));
///             Ok(Point{ x: x, y: y })
///         })
///     }
/// }
/// # fn main() {}
/// ```
pub trait Decodable: Sized {
    /// Deserialize a value using a `Decoder`.
    fn decode<D: Decoder>(d: &mut D) -> Result<Self, D::Error>;
}

impl Encodable for usize {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_usize(*self)
    }
}

impl Decodable for usize {
    fn decode<D: Decoder>(d: &mut D) -> Result<usize, D::Error> {
        d.read_usize()
    }
}

impl Encodable for u8 {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_u8(*self)
    }
}

impl Decodable for u8 {
    fn decode<D: Decoder>(d: &mut D) -> Result<u8, D::Error> {
        d.read_u8()
    }
}

impl Encodable for u16 {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_u16(*self)
    }
}

impl Decodable for u16 {
    fn decode<D: Decoder>(d: &mut D) -> Result<u16, D::Error> {
        d.read_u16()
    }
}

impl Encodable for u32 {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_u32(*self)
    }
}

impl Decodable for u32 {
    fn decode<D: Decoder>(d: &mut D) -> Result<u32, D::Error> {
        d.read_u32()
    }
}

impl Encodable for u64 {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_u64(*self)
    }
}

impl Decodable for u64 {
    fn decode<D: Decoder>(d: &mut D) -> Result<u64, D::Error> {
        d.read_u64()
    }
}

impl Encodable for isize {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_isize(*self)
    }
}

impl Decodable for isize {
    fn decode<D: Decoder>(d: &mut D) -> Result<isize, D::Error> {
        d.read_isize()
    }
}

impl Encodable for i8 {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_i8(*self)
    }
}

impl Decodable for i8 {
    fn decode<D: Decoder>(d: &mut D) -> Result<i8, D::Error> {
        d.read_i8()
    }
}

impl Encodable for i16 {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_i16(*self)
    }
}

impl Decodable for i16 {
    fn decode<D: Decoder>(d: &mut D) -> Result<i16, D::Error> {
        d.read_i16()
    }
}

impl Encodable for i32 {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_i32(*self)
    }
}

impl Decodable for i32 {
    fn decode<D: Decoder>(d: &mut D) -> Result<i32, D::Error> {
        d.read_i32()
    }
}

impl Encodable for i64 {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_i64(*self)
    }
}

impl Decodable for i64 {
    fn decode<D: Decoder>(d: &mut D) -> Result<i64, D::Error> {
        d.read_i64()
    }
}

impl Encodable for str {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_str(self)
    }
}

impl Encodable for String {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_str(self)
    }
}

impl Decodable for String {
    fn decode<D: Decoder>(d: &mut D) -> Result<String, D::Error> {
        d.read_str()
    }
}

impl Encodable for f32 {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_f32(*self)
    }
}

impl Decodable for f32 {
    fn decode<D: Decoder>(d: &mut D) -> Result<f32, D::Error> {
        d.read_f32()
    }
}

impl Encodable for f64 {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_f64(*self)
    }
}

impl Decodable for f64 {
    fn decode<D: Decoder>(d: &mut D) -> Result<f64, D::Error> {
        d.read_f64()
    }
}

impl Encodable for bool {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_bool(*self)
    }
}

impl Decodable for bool {
    fn decode<D: Decoder>(d: &mut D) -> Result<bool, D::Error> {
        d.read_bool()
    }
}

impl Encodable for char {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_char(*self)
    }
}

impl Decodable for char {
    fn decode<D: Decoder>(d: &mut D) -> Result<char, D::Error> {
        d.read_char()
    }
}

impl Encodable for () {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_nil()
    }
}

impl Decodable for () {
    fn decode<D: Decoder>(d: &mut D) -> Result<(), D::Error> {
        d.read_nil()
    }
}

impl<'a, T: ?Sized + Encodable> Encodable for &'a T {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        (**self).encode(s)
    }
}

impl<T: ?Sized + Encodable> Encodable for Box<T> {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        (**self).encode(s)
    }
}

impl< T: Decodable> Decodable for Box<T> {
    fn decode<D: Decoder>(d: &mut D) -> Result<Box<T>, D::Error> {
        Ok(Box::new(try!(Decodable::decode(d))))
    }
}

impl< T: Decodable> Decodable for Box<[T]> {
    fn decode<D: Decoder>(d: &mut D) -> Result<Box<[T]>, D::Error> {
        let v: Vec<T> = try!(Decodable::decode(d));
        Ok(v.into_boxed_slice())
    }
}

impl<T:Encodable> Encodable for Rc<T> {
    #[inline]
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        (**self).encode(s)
    }
}

impl<T:Decodable> Decodable for Rc<T> {
    #[inline]
    fn decode<D: Decoder>(d: &mut D) -> Result<Rc<T>, D::Error> {
        Ok(Rc::new(try!(Decodable::decode(d))))
    }
}

impl<'a, T:Encodable + ToOwned + ?Sized> Encodable for Cow<'a, T> {
    #[inline]
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        (**self).encode(s)
    }
}

impl<'a, T: ?Sized> Decodable for Cow<'a, T>
    where T: ToOwned, T::Owned: Decodable
{
    #[inline]
    fn decode<D: Decoder>(d: &mut D) -> Result<Cow<'static, T>, D::Error> {
        Ok(Cow::Owned(try!(Decodable::decode(d))))
    }
}

impl<T:Encodable> Encodable for [T] {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_seq(self.len(), |s| {
            for (i, e) in self.iter().enumerate() {
                try!(s.emit_seq_elt(i, |s| e.encode(s)))
            }
            Ok(())
        })
    }
}

impl<T:Encodable> Encodable for Vec<T> {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_seq(self.len(), |s| {
            for (i, e) in self.iter().enumerate() {
                try!(s.emit_seq_elt(i, |s| e.encode(s)))
            }
            Ok(())
        })
    }
}

impl<T:Decodable> Decodable for Vec<T> {
    fn decode<D: Decoder>(d: &mut D) -> Result<Vec<T>, D::Error> {
        d.read_seq(|d, len| {
            let mut v = Vec::with_capacity(cap_capacity::<T>(len));
            for i in 0..len {
                v.push(try!(d.read_seq_elt(i, |d| Decodable::decode(d))));
            }
            Ok(v)
        })
    }
}

impl<T:Encodable> Encodable for Option<T> {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_option(|s| {
            match *self {
                None => s.emit_option_none(),
                Some(ref v) => s.emit_option_some(|s| v.encode(s)),
            }
        })
    }
}

impl<T:Decodable> Decodable for Option<T> {
    fn decode<D: Decoder>(d: &mut D) -> Result<Option<T>, D::Error> {
        d.read_option(|d, b| {
            if b {
                Ok(Some(try!(Decodable::decode(d))))
            } else {
                Ok(None)
            }
        })
    }
}

impl<T:Encodable, E:Encodable> Encodable for Result<T, E> {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_enum("Result", |s| {
            match *self {
                Ok(ref v) => {
                    s.emit_enum_variant("Ok", 0, 1, |s| {
                        try!(s.emit_enum_variant_arg(0, |s| {
                            v.encode(s)
                        }));
                        Ok(())
                    })
                }
                Err(ref v) => {
                    s.emit_enum_variant("Err", 1, 1, |s| {
                        try!(s.emit_enum_variant_arg(0, |s| {
                            v.encode(s)
                        }));
                        Ok(())
                    })
                }
            }
        })
    }
}

impl<T: Decodable, E: Decodable> Decodable for Result<T, E> {
    fn decode<D: Decoder>(d: &mut D) -> Result<Result<T, E>, D::Error> {
        d.read_enum("Result", |d| {
            d.read_enum_variant(&["Ok", "Err"], |d, idx| {
                match idx {
                    0 => {
                        d.read_enum_variant_arg(0, |d| {
                            T::decode(d)
                        }).map(|v| Ok(v))
                    }
                    1 => {
                        d.read_enum_variant_arg(0, |d| {
                            E::decode(d)
                        }).map(|v| Err(v))
                    }
                    _ => panic!("Internal error"),
                }
            })
        })
    }
}

impl<T> Encodable for PhantomData<T> {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        s.emit_nil()
    }
}

impl<T> Decodable for PhantomData<T> {
    fn decode<D: Decoder>(_d: &mut D) -> Result<PhantomData<T>, D::Error> {
        Ok(PhantomData)
    }
}

macro_rules! peel {
    ($name:ident, $($other:ident,)*) => (tuple! { $($other,)* })
}

/// Evaluates to the number of identifiers passed to it, for example:
/// `count_idents!(a, b, c) == 3
macro_rules! count_idents {
    () => { 0 };
    ($_i:ident, $($rest:ident,)*) => { 1 + count_idents!($($rest,)*) }
}

macro_rules! tuple {
    () => ();
    ( $($name:ident,)+ ) => (
        impl<$($name:Decodable),*> Decodable for ($($name,)*) {
            fn decode<D: Decoder>(d: &mut D) -> Result<($($name,)*), D::Error> {
                let len: usize = count_idents!($($name,)*);
                d.read_tuple(len, |d| {
                    let mut i = 0;
                    let ret = ($(try!(d.read_tuple_arg({ i+=1; i-1 },
                                                       |d| -> Result<$name,D::Error> {
                        Decodable::decode(d)
                    })),)*);
                    return Ok(ret);
                })
            }
        }
        impl<$($name:Encodable),*> Encodable for ($($name,)*) {
            #[allow(non_snake_case)]
            fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
                let ($(ref $name,)*) = *self;
                let mut n = 0;
                $(let $name = $name; n += 1;)*
                s.emit_tuple(n, |s| {
                    let mut i = 0;
                    $(try!(s.emit_tuple_arg({ i+=1; i-1 }, |s| $name.encode(s)));)*
                    Ok(())
                })
            }
        }
        peel! { $($name,)* }
    )
}

tuple! { T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, }

macro_rules! array {
    () => ();
    ($len:expr, $($idx:expr,)*) => {
        impl<T:Decodable> Decodable for [T; $len] {
            fn decode<D: Decoder>(d: &mut D) -> Result<[T; $len], D::Error> {
                d.read_seq(|d, len| {
                    if len != $len {
                        return Err(d.error("wrong array length"));
                    }
                    Ok([$(
                        try!(d.read_seq_elt($len - $idx - 1,
                                            |d| Decodable::decode(d)))
                    ),*])
                })
            }
        }

        impl<T:Encodable> Encodable for [T; $len] {
            fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
                s.emit_seq($len, |s| {
                    for i in 0..$len {
                        try!(s.emit_seq_elt(i, |s| self[i].encode(s)));
                    }
                    Ok(())
                })
            }
        }
        array! { $($idx,)* }
    }
}

array! {
    32, 31, 30, 29, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19, 18, 17, 16,
    15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0,
}

impl Encodable for path::Path {
    #[cfg(target_os = "redox")]
    fn encode<S: Encoder>(&self, e: &mut S) -> Result<(), S::Error> {
        self.as_os_str().to_str().unwrap().encode(e)
    }
    #[cfg(unix)]
    fn encode<S: Encoder>(&self, e: &mut S) -> Result<(), S::Error> {
        use std::os::unix::prelude::*;
        self.as_os_str().as_bytes().encode(e)
    }
    #[cfg(windows)]
    fn encode<S: Encoder>(&self, e: &mut S) -> Result<(), S::Error> {
        use std::os::windows::prelude::*;
        let v = self.as_os_str().encode_wide().collect::<Vec<_>>();
        v.encode(e)
    }
}

impl Encodable for path::PathBuf {
    fn encode<S: Encoder>(&self, e: &mut S) -> Result<(), S::Error> {
        (**self).encode(e)
    }
}

impl Decodable for path::PathBuf {
    #[cfg(target_os = "redox")]
    fn decode<D: Decoder>(d: &mut D) -> Result<path::PathBuf, D::Error> {
        let string: String = try!(Decodable::decode(d));
        let s: OsString = OsString::from(string);
        let mut p = path::PathBuf::new();
        p.push(s);
        Ok(p)
    }
    #[cfg(unix)]
    fn decode<D: Decoder>(d: &mut D) -> Result<path::PathBuf, D::Error> {
        use std::os::unix::prelude::*;
        let bytes: Vec<u8> = try!(Decodable::decode(d));
        let s: OsString = OsStringExt::from_vec(bytes);
        let mut p = path::PathBuf::new();
        p.push(s);
        Ok(p)
    }
    #[cfg(windows)]
    fn decode<D: Decoder>(d: &mut D) -> Result<path::PathBuf, D::Error> {
        use std::os::windows::prelude::*;
        let bytes: Vec<u16> = try!(Decodable::decode(d));
        let s: OsString = OsStringExt::from_wide(&bytes);
        let mut p = path::PathBuf::new();
        p.push(s);
        Ok(p)
    }
}

impl<T: Encodable + Copy> Encodable for Cell<T> {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        self.get().encode(s)
    }
}

impl<T: Decodable + Copy> Decodable for Cell<T> {
    fn decode<D: Decoder>(d: &mut D) -> Result<Cell<T>, D::Error> {
        Ok(Cell::new(try!(Decodable::decode(d))))
    }
}

// FIXME: #15036
// Should use `try_borrow`, returning a
// `encoder.error("attempting to Encode borrowed RefCell")`
// from `encode` when `try_borrow` returns `None`.

impl<T: Encodable> Encodable for RefCell<T> {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        self.borrow().encode(s)
    }
}

impl<T: Decodable> Decodable for RefCell<T> {
    fn decode<D: Decoder>(d: &mut D) -> Result<RefCell<T>, D::Error> {
        Ok(RefCell::new(try!(Decodable::decode(d))))
    }
}

impl<T:Encodable> Encodable for Arc<T> {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        (**self).encode(s)
    }
}

impl<T:Decodable+Send+Sync> Decodable for Arc<T> {
    fn decode<D: Decoder>(d: &mut D) -> Result<Arc<T>, D::Error> {
        Ok(Arc::new(try!(Decodable::decode(d))))
    }
}

// ___________________________________________________________________________
// Helper routines

/// Trait with helper functions for implementing `Encodable`.
///
/// This trait is implemented for everything that implements `Encoder`.
/// `Encodable` implementations can make use of it to make their implementations
/// easier.
pub trait EncoderHelpers: Encoder {
    /// Emit a vector as a sequence.
    ///
    /// Storing sequences as vectors is a common pattern. This method makes
    /// encoding such sequences easier by wrapping the calls to
    /// `Encoder::emit_seq` and `Encoder::emit_seq_elt`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rustc_serialize::Encodable;
    /// use rustc_serialize::Encoder;
    /// use rustc_serialize::EncoderHelpers;
    ///
    /// struct NumberSequence {
    ///     elements: Vec<i32>,
    /// }
    ///
    /// impl Encodable for NumberSequence {
    ///     fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
    ///         s.emit_struct("NumberSequence", 1, |s| {
    ///             s.emit_struct_field("elements", 0, |s| {
    ///                 s.emit_from_vec(&self.elements, |s,e| {
    ///                     s.emit_i32(*e)
    ///                 })
    ///             })
    ///         })
    ///     }
    /// }
    /// ```
    fn emit_from_vec<T, F>(&mut self, v: &[T], f: F)
                           -> Result<(), <Self as Encoder>::Error>
        where F: FnMut(&mut Self, &T) -> Result<(), <Self as Encoder>::Error>;
}

impl<S:Encoder> EncoderHelpers for S {
    fn emit_from_vec<T, F>(&mut self, v: &[T], mut f: F) -> Result<(), S::Error> where
        F: FnMut(&mut S, &T) -> Result<(), S::Error>,
    {
        self.emit_seq(v.len(), |this| {
            for (i, e) in v.iter().enumerate() {
                try!(this.emit_seq_elt(i, |this| {
                    f(this, e)
                }));
            }
            Ok(())
        })
    }
}

/// Trait with helper functions for implementing `Decodable`.
///
/// This trait is implemented for everything that implements `Decoder`.
/// `Decodable` implementations can make use of it to make their implementations
/// easier.
pub trait DecoderHelpers: Decoder {
    /// Read a sequence into a vector.
    ///
    /// Storing sequences as vectors is a common pattern. This method makes
    /// deserializing such sequences easier by wrapping the calls to
    /// `Decoder::read_seq` and `Decoder::read_seq_elt`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rustc_serialize::Decodable;
    /// use rustc_serialize::Decoder;
    /// use rustc_serialize::DecoderHelpers;
    ///
    /// struct NumberSequence {
    ///     elements: Vec<i32>,
    /// }
    ///
    /// impl Decodable for NumberSequence {
    ///     fn decode<D: Decoder>(d: &mut D) -> Result<NumberSequence, D::Error> {
    ///         d.read_struct("NumberSequence", 2, |d| {
    ///             Ok(NumberSequence{
    ///                 elements: try!(d.read_struct_field("elements", 0, |d| {
    ///                     d.read_to_vec(|d| { d.read_i32() })
    ///                 }))
    ///             })
    ///         })
    ///     }
    /// }
    /// ```
    fn read_to_vec<T, F>(&mut self, f: F)
                         -> Result<Vec<T>, <Self as Decoder>::Error> where
        F: FnMut(&mut Self) -> Result<T, <Self as Decoder>::Error>;
}

impl<D: Decoder> DecoderHelpers for D {
    fn read_to_vec<T, F>(&mut self, mut f: F) -> Result<Vec<T>, D::Error> where F:
        FnMut(&mut D) -> Result<T, D::Error>,
    {
        self.read_seq(|this, len| {
            let mut v = Vec::with_capacity(cap_capacity::<T>(len));
            for i in 0..len {
                v.push(try!(this.read_seq_elt(i, |this| f(this))));
            }
            Ok(v)
        })
    }
}

#[test]
#[allow(unused_variables)]
fn capacity_rules() {
    use std::usize::MAX;
    use std::collections::{HashMap, HashSet};

    struct MyDecoder;
    impl Decoder for MyDecoder {
        type Error = ();

        // Primitive types:
        fn read_nil(&mut self) -> Result<(), Self::Error> { Err(()) }
        fn read_usize(&mut self) -> Result<usize, Self::Error> { Err(()) }
        fn read_u64(&mut self) -> Result<u64, Self::Error> { Err(()) }
        fn read_u32(&mut self) -> Result<u32, Self::Error> { Err(()) }
        fn read_u16(&mut self) -> Result<u16, Self::Error> { Err(()) }
        fn read_u8(&mut self) -> Result<u8, Self::Error> { Err(()) }
        fn read_isize(&mut self) -> Result<isize, Self::Error> { Err(()) }
        fn read_i64(&mut self) -> Result<i64, Self::Error> { Err(()) }
        fn read_i32(&mut self) -> Result<i32, Self::Error> { Err(()) }
        fn read_i16(&mut self) -> Result<i16, Self::Error> { Err(()) }
        fn read_i8(&mut self) -> Result<i8, Self::Error> { Err(()) }
        fn read_bool(&mut self) -> Result<bool, Self::Error> { Err(()) }
        fn read_f64(&mut self) -> Result<f64, Self::Error> { Err(()) }
        fn read_f32(&mut self) -> Result<f32, Self::Error> { Err(()) }
        fn read_char(&mut self) -> Result<char, Self::Error> { Err(()) }
        fn read_str(&mut self) -> Result<String, Self::Error> { Err(()) }

        // Compound types:
        fn read_enum<T, F>(&mut self, name: &str, f: F) -> Result<T, Self::Error>
            where F: FnOnce(&mut Self) -> Result<T, Self::Error> { Err(()) }

        fn read_enum_variant<T, F>(&mut self, names: &[&str], f: F)
                                   -> Result<T, Self::Error>
            where F: FnMut(&mut Self, usize) -> Result<T, Self::Error> { Err(()) }
        fn read_enum_variant_arg<T, F>(&mut self, a_idx: usize, f: F)
                                       -> Result<T, Self::Error>
            where F: FnOnce(&mut Self) -> Result<T, Self::Error> { Err(()) }

        fn read_enum_struct_variant<T, F>(&mut self, names: &[&str], f: F)
                                          -> Result<T, Self::Error>
            where F: FnMut(&mut Self, usize) -> Result<T, Self::Error> { Err(()) }
        fn read_enum_struct_variant_field<T, F>(&mut self,
                                                f_name: &str,
                                                f_idx: usize,
                                                f: F)
                                                -> Result<T, Self::Error>
            where F: FnOnce(&mut Self) -> Result<T, Self::Error> { Err(()) }

        fn read_struct<T, F>(&mut self, s_name: &str, len: usize, f: F)
                             -> Result<T, Self::Error>
            where F: FnOnce(&mut Self) -> Result<T, Self::Error> { Err(()) }
        fn read_struct_field<T, F>(&mut self,
                                   f_name: &str,
                                   f_idx: usize,
                                   f: F)
                                   -> Result<T, Self::Error>
            where F: FnOnce(&mut Self) -> Result<T, Self::Error> { Err(()) }

        fn read_tuple<T, F>(&mut self, len: usize, f: F) -> Result<T, Self::Error>
            where F: FnOnce(&mut Self) -> Result<T, Self::Error> { Err(()) }
        fn read_tuple_arg<T, F>(&mut self, a_idx: usize, f: F)
                                -> Result<T, Self::Error>
            where F: FnOnce(&mut Self) -> Result<T, Self::Error> { Err(()) }

        fn read_tuple_struct<T, F>(&mut self, s_name: &str, len: usize, f: F)
                                   -> Result<T, Self::Error>
            where F: FnOnce(&mut Self) -> Result<T, Self::Error> { Err(()) }
        fn read_tuple_struct_arg<T, F>(&mut self, a_idx: usize, f: F)
                                       -> Result<T, Self::Error>
            where F: FnOnce(&mut Self) -> Result<T, Self::Error> { Err(()) }

        // Specialized types:
        fn read_option<T, F>(&mut self, f: F) -> Result<T, Self::Error>
            where F: FnMut(&mut Self, bool) -> Result<T, Self::Error> { Err(()) }

        fn read_seq<T, F>(&mut self, f: F) -> Result<T, Self::Error>
            where F: FnOnce(&mut Self, usize) -> Result<T, Self::Error> {
                f(self, MAX)
            }
        fn read_seq_elt<T, F>(&mut self, idx: usize, f: F) -> Result<T, Self::Error>
            where F: FnOnce(&mut Self) -> Result<T, Self::Error> { Err(()) }

        fn read_map<T, F>(&mut self, f: F) -> Result<T, Self::Error>
            where F: FnOnce(&mut Self, usize) -> Result<T, Self::Error> {
            f(self, MAX)
        }
        fn read_map_elt_key<T, F>(&mut self, idx: usize, f: F)
                                  -> Result<T, Self::Error>
            where F: FnOnce(&mut Self) -> Result<T, Self::Error> { Err(()) }
        fn read_map_elt_val<T, F>(&mut self, idx: usize, f: F)
                                  -> Result<T, Self::Error>
            where F: FnOnce(&mut Self) -> Result<T, Self::Error> { Err(()) }

        // Failure
        fn error(&mut self, err: &str) -> Self::Error { () }
    }

    let mut dummy = MyDecoder;
    let vec_result: Result<Vec<u8>, ()> = Decodable::decode(&mut dummy);
    assert!(vec_result.is_err());

    let map_result: Result<HashMap<u8, u8>, ()> = Decodable::decode(&mut dummy);
    assert!(map_result.is_err());

    let set_result: Result<HashSet<u8>, ()> = Decodable::decode(&mut dummy);
    assert!(set_result.is_err());
}

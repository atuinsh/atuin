#[cfg(feature = "std")]
#[doc(hidden)]
#[macro_export]
macro_rules! forward_to_deserialize_method {
    ($func:ident($($arg:ty),*)) => {
        #[inline]
        fn $func<__V>(&mut self, $(_: $arg,)* visitor: __V) -> ::std::result::Result<__V::Value, Self::Error>
            where __V: $crate::de::Visitor
        {
            self.deserialize(visitor)
        }
    };
}

#[cfg(not(feature = "std"))]
#[doc(hidden)]
#[macro_export]
macro_rules! forward_to_deserialize_method {
    ($func:ident($($arg:ty),*)) => {
        #[inline]
        fn $func<__V>(&mut self, $(_: $arg,)* visitor: __V) -> ::core::result::Result<__V::Value, Self::Error>
            where __V: $crate::de::Visitor
        {
            self.deserialize(visitor)
        }
    };
}

#[cfg(feature = "std")]
#[doc(hidden)]
#[macro_export]
macro_rules! forward_to_deserialize_enum {
    () => {
        #[inline]
        fn deserialize_enum<__V>(&mut self, _: &str, _: &[&str], _: __V) -> ::std::result::Result<__V::Value, Self::Error>
            where __V: $crate::de::EnumVisitor
        {
            Err($crate::de::Error::invalid_type($crate::de::Type::Enum))
        }
    };
}

#[cfg(not(feature = "std"))]
#[doc(hidden)]
#[macro_export]
macro_rules! forward_to_deserialize_enum {
    () => {
        #[inline]
        fn deserialize_enum<__V>(&mut self, _: &str, _: &[&str], _: __V) -> ::core::result::Result<__V::Value, Self::Error>
            where __V: $crate::de::EnumVisitor
        {
            Err($crate::de::Error::invalid_type($crate::de::Type::Enum))
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! forward_to_deserialize_helper {
    (bool) => {
        forward_to_deserialize_method!{deserialize_bool()}
    };
    (usize) => {
        forward_to_deserialize_method!{deserialize_usize()}
    };
    (u8) => {
        forward_to_deserialize_method!{deserialize_u8()}
    };
    (u16) => {
        forward_to_deserialize_method!{deserialize_u16()}
    };
    (u32) => {
        forward_to_deserialize_method!{deserialize_u32()}
    };
    (u64) => {
        forward_to_deserialize_method!{deserialize_u64()}
    };
    (isize) => {
        forward_to_deserialize_method!{deserialize_isize()}
    };
    (i8) => {
        forward_to_deserialize_method!{deserialize_i8()}
    };
    (i16) => {
        forward_to_deserialize_method!{deserialize_i16()}
    };
    (i32) => {
        forward_to_deserialize_method!{deserialize_i32()}
    };
    (i64) => {
        forward_to_deserialize_method!{deserialize_i64()}
    };
    (f32) => {
        forward_to_deserialize_method!{deserialize_f32()}
    };
    (f64) => {
        forward_to_deserialize_method!{deserialize_f64()}
    };
    (char) => {
        forward_to_deserialize_method!{deserialize_char()}
    };
    (str) => {
        forward_to_deserialize_method!{deserialize_str()}
    };
    (string) => {
        forward_to_deserialize_method!{deserialize_string()}
    };
    (unit) => {
        forward_to_deserialize_method!{deserialize_unit()}
    };
    (option) => {
        forward_to_deserialize_method!{deserialize_option()}
    };
    (seq) => {
        forward_to_deserialize_method!{deserialize_seq()}
    };
    (seq_fixed_size) => {
        forward_to_deserialize_method!{deserialize_seq_fixed_size(usize)}
    };
    (bytes) => {
        forward_to_deserialize_method!{deserialize_bytes()}
    };
    (map) => {
        forward_to_deserialize_method!{deserialize_map()}
    };
    (unit_struct) => {
        forward_to_deserialize_method!{deserialize_unit_struct(&'static str)}
    };
    (newtype_struct) => {
        forward_to_deserialize_method!{deserialize_newtype_struct(&'static str)}
    };
    (tuple_struct) => {
        forward_to_deserialize_method!{deserialize_tuple_struct(&'static str, usize)}
    };
    (struct) => {
        forward_to_deserialize_method!{deserialize_struct(&'static str, &'static [&'static str])}
    };
    (struct_field) => {
        forward_to_deserialize_method!{deserialize_struct_field()}
    };
    (tuple) => {
        forward_to_deserialize_method!{deserialize_tuple(usize)}
    };
    (ignored_any) => {
        forward_to_deserialize_method!{deserialize_ignored_any()}
    };
    (enum) => {
        forward_to_deserialize_enum!();
    };
}

/// Helper to forward `Deserializer` methods to `Deserializer::deserialize`.
/// Every given method ignores all arguments and forwards to `deserialize`.
/// Note that `deserialize_enum` simply returns an `Error::invalid_type`; a
/// better approach is tracked in [serde-rs/serde#521][1].
///
/// ```rust,ignore
/// impl Deserializer for MyDeserializer {
///     fn deserialize<V>(&mut self, visitor: V) -> Result<V::Value, Self::Error>
///         where V: Visitor
///     {
///         /* ... */
///     }
///
///     forward_to_deserialize! {
///         bool usize u8 u16 u32 u64 isize i8 i16 i32 i64 f32 f64 char str string
///         unit option seq seq_fixed_size bytes map unit_struct newtype_struct
///         tuple_struct struct struct_field tuple enum ignored_any
///     }
/// }
/// ```
///
/// [1]: https://github.com/serde-rs/serde/issues/521
#[macro_export]
macro_rules! forward_to_deserialize {
    ($($func:ident)*) => {
        $(forward_to_deserialize_helper!{$func})*
    };
}

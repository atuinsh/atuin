//! Structured values.

use std::fmt;

extern crate value_bag;

#[cfg(feature = "kv_unstable_sval")]
extern crate sval;

#[cfg(feature = "kv_unstable_serde")]
extern crate serde;

use self::value_bag::ValueBag;

pub use kv::Error;

/// A type that can be converted into a [`Value`](struct.Value.html).
pub trait ToValue {
    /// Perform the conversion.
    fn to_value(&self) -> Value;
}

impl<'a, T> ToValue for &'a T
where
    T: ToValue + ?Sized,
{
    fn to_value(&self) -> Value {
        (**self).to_value()
    }
}

impl<'v> ToValue for Value<'v> {
    fn to_value(&self) -> Value {
        Value {
            inner: self.inner.clone(),
        }
    }
}

/// A value in a structured key-value pair.
///
/// # Capturing values
///
/// There are a few ways to capture a value:
///
/// - Using the `Value::capture_*` methods.
/// - Using the `Value::from_*` methods.
/// - Using the `ToValue` trait.
/// - Using the standard `From` trait.
///
/// ## Using the `Value::capture_*` methods
///
/// `Value` offers a few constructor methods that capture values of different kinds.
/// These methods require a `T: 'static` to support downcasting.
///
/// ```
/// use log::kv::Value;
///
/// let value = Value::capture_debug(&42i32);
///
/// assert_eq!(Some(42), value.to_i64());
/// ```
///
/// ## Using the `Value::from_*` methods
///
/// `Value` offers a few constructor methods that capture values of different kinds.
/// These methods don't require `T: 'static`, but can't support downcasting.
///
/// ```
/// use log::kv::Value;
///
/// let value = Value::from_debug(&42i32);
///
/// assert_eq!(None, value.to_i64());
/// ```
///
/// ## Using the `ToValue` trait
///
/// The `ToValue` trait can be used to capture values generically.
/// It's the bound used by `Source`.
///
/// ```
/// # use log::kv::ToValue;
/// let value = 42i32.to_value();
///
/// assert_eq!(Some(42), value.to_i64());
/// ```
///
/// ```
/// # use std::fmt::Debug;
/// use log::kv::ToValue;
///
/// let value = (&42i32 as &dyn Debug).to_value();
///
/// assert_eq!(None, value.to_i64());
/// ```
///
/// ## Using the standard `From` trait
///
/// Standard types that implement `ToValue` also implement `From`.
///
/// ```
/// use log::kv::Value;
///
/// let value = Value::from(42i32);
///
/// assert_eq!(Some(42), value.to_i64());
/// ```
pub struct Value<'v> {
    inner: ValueBag<'v>,
}

impl<'v> Value<'v> {
    /// Get a value from a type implementing `ToValue`.
    pub fn from_any<T>(value: &'v T) -> Self
    where
        T: ToValue,
    {
        value.to_value()
    }

    /// Get a value from a type implementing `std::fmt::Debug`.
    pub fn capture_debug<T>(value: &'v T) -> Self
    where
        T: fmt::Debug + 'static,
    {
        Value {
            inner: ValueBag::capture_debug(value),
        }
    }

    /// Get a value from a type implementing `std::fmt::Display`.
    pub fn capture_display<T>(value: &'v T) -> Self
    where
        T: fmt::Display + 'static,
    {
        Value {
            inner: ValueBag::capture_display(value),
        }
    }

    /// Get a value from an error.
    #[cfg(feature = "kv_unstable_std")]
    pub fn capture_error<T>(err: &'v T) -> Self
    where
        T: std::error::Error + 'static,
    {
        Value {
            inner: ValueBag::capture_error(err),
        }
    }

    #[cfg(feature = "kv_unstable_serde")]
    /// Get a value from a type implementing `serde::Serialize`.
    pub fn capture_serde<T>(value: &'v T) -> Self
    where
        T: self::serde::Serialize + 'static,
    {
        Value {
            inner: ValueBag::capture_serde1(value),
        }
    }

    /// Get a value from a type implementing `sval::value::Value`.
    #[cfg(feature = "kv_unstable_sval")]
    pub fn capture_sval<T>(value: &'v T) -> Self
    where
        T: self::sval::value::Value + 'static,
    {
        Value {
            inner: ValueBag::capture_sval1(value),
        }
    }

    /// Get a value from a type implementing `std::fmt::Debug`.
    pub fn from_debug<T>(value: &'v T) -> Self
    where
        T: fmt::Debug,
    {
        Value {
            inner: ValueBag::from_debug(value),
        }
    }

    /// Get a value from a type implementing `std::fmt::Display`.
    pub fn from_display<T>(value: &'v T) -> Self
    where
        T: fmt::Display,
    {
        Value {
            inner: ValueBag::from_display(value),
        }
    }

    /// Get a value from a type implementing `serde::Serialize`.
    #[cfg(feature = "kv_unstable_serde")]
    pub fn from_serde<T>(value: &'v T) -> Self
    where
        T: self::serde::Serialize,
    {
        Value {
            inner: ValueBag::from_serde1(value),
        }
    }

    /// Get a value from a type implementing `sval::value::Value`.
    #[cfg(feature = "kv_unstable_sval")]
    pub fn from_sval<T>(value: &'v T) -> Self
    where
        T: self::sval::value::Value,
    {
        Value {
            inner: ValueBag::from_sval1(value),
        }
    }

    /// Get a value from a dynamic `std::fmt::Debug`.
    pub fn from_dyn_debug(value: &'v dyn fmt::Debug) -> Self {
        Value {
            inner: ValueBag::from_dyn_debug(value),
        }
    }

    /// Get a value from a dynamic `std::fmt::Display`.
    pub fn from_dyn_display(value: &'v dyn fmt::Display) -> Self {
        Value {
            inner: ValueBag::from_dyn_display(value),
        }
    }

    /// Get a value from a dynamic error.
    #[cfg(feature = "kv_unstable_std")]
    pub fn from_dyn_error(err: &'v (dyn std::error::Error + 'static)) -> Self {
        Value {
            inner: ValueBag::from_dyn_error(err),
        }
    }

    /// Get a value from a type implementing `sval::value::Value`.
    #[cfg(feature = "kv_unstable_sval")]
    pub fn from_dyn_sval(value: &'v dyn self::sval::value::Value) -> Self {
        Value {
            inner: ValueBag::from_dyn_sval1(value),
        }
    }

    /// Get a value from an internal primitive.
    fn from_value_bag<T>(value: T) -> Self
    where
        T: Into<ValueBag<'v>>,
    {
        Value {
            inner: value.into(),
        }
    }

    /// Check whether this value can be downcast to `T`.
    pub fn is<T: 'static>(&self) -> bool {
        self.inner.is::<T>()
    }

    /// Try downcast this value to `T`.
    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.inner.downcast_ref::<T>()
    }
}

impl<'v> fmt::Debug for Value<'v> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.inner, f)
    }
}

impl<'v> fmt::Display for Value<'v> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.inner, f)
    }
}

impl ToValue for dyn fmt::Debug {
    fn to_value(&self) -> Value {
        Value::from_dyn_debug(self)
    }
}

impl ToValue for dyn fmt::Display {
    fn to_value(&self) -> Value {
        Value::from_dyn_display(self)
    }
}

#[cfg(feature = "kv_unstable_std")]
impl ToValue for dyn std::error::Error + 'static {
    fn to_value(&self) -> Value {
        Value::from_dyn_error(self)
    }
}

#[cfg(feature = "kv_unstable_serde")]
impl<'v> self::serde::Serialize for Value<'v> {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: self::serde::Serializer,
    {
        self.inner.serialize(s)
    }
}

#[cfg(feature = "kv_unstable_sval")]
impl<'v> self::sval::value::Value for Value<'v> {
    fn stream(&self, stream: &mut self::sval::value::Stream) -> self::sval::value::Result {
        self::sval::value::Value::stream(&self.inner, stream)
    }
}

#[cfg(feature = "kv_unstable_sval")]
impl ToValue for dyn self::sval::value::Value {
    fn to_value(&self) -> Value {
        Value::from_dyn_sval(self)
    }
}

impl ToValue for str {
    fn to_value(&self) -> Value {
        Value::from(self)
    }
}

impl<'v> From<&'v str> for Value<'v> {
    fn from(value: &'v str) -> Self {
        Value::from_value_bag(value)
    }
}

impl ToValue for () {
    fn to_value(&self) -> Value {
        Value::from_value_bag(())
    }
}

impl<T> ToValue for Option<T>
where
    T: ToValue,
{
    fn to_value(&self) -> Value {
        match *self {
            Some(ref value) => value.to_value(),
            None => Value::from_value_bag(()),
        }
    }
}

macro_rules! impl_to_value_primitive {
    ($($into_ty:ty,)*) => {
        $(
            impl ToValue for $into_ty {
                fn to_value(&self) -> Value {
                    Value::from(*self)
                }
            }

            impl<'v> From<$into_ty> for Value<'v> {
                fn from(value: $into_ty) -> Self {
                    Value::from_value_bag(value)
                }
            }
        )*
    };
}

macro_rules! impl_value_to_primitive {
    ($(#[doc = $doc:tt] $into_name:ident -> $into_ty:ty,)*) => {
        impl<'v> Value<'v> {
            $(
                #[doc = $doc]
                pub fn $into_name(&self) -> Option<$into_ty> {
                    self.inner.$into_name()
                }
            )*
        }
    }
}

impl_to_value_primitive![
    usize, u8, u16, u32, u64, u128, isize, i8, i16, i32, i64, i128, f32, f64, char, bool,
];

impl_value_to_primitive![
    #[doc = "Try convert this value into a `u64`."]
    to_u64 -> u64,
    #[doc = "Try convert this value into a `i64`."]
    to_i64 -> i64,
    #[doc = "Try convert this value into a `u128`."]
    to_u128 -> u128,
    #[doc = "Try convert this value into a `i128`."]
    to_i128 -> i128,
    #[doc = "Try convert this value into a `f64`."]
    to_f64 -> f64,
    #[doc = "Try convert this value into a `char`."]
    to_char -> char,
    #[doc = "Try convert this value into a `bool`."]
    to_bool -> bool,
];

impl<'v> Value<'v> {
    /// Try convert this value into an error.
    #[cfg(feature = "kv_unstable_std")]
    pub fn to_borrowed_error(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.to_borrowed_error()
    }

    /// Try convert this value into a borrowed string.
    pub fn to_borrowed_str(&self) -> Option<&str> {
        self.inner.to_borrowed_str()
    }
}

#[cfg(feature = "kv_unstable_std")]
mod std_support {
    use super::*;

    use std::borrow::Cow;

    impl<T> ToValue for Box<T>
    where
        T: ToValue + ?Sized,
    {
        fn to_value(&self) -> Value {
            (**self).to_value()
        }
    }

    impl ToValue for String {
        fn to_value(&self) -> Value {
            Value::from(&**self)
        }
    }

    impl<'v> ToValue for Cow<'v, str> {
        fn to_value(&self) -> Value {
            Value::from(&**self)
        }
    }

    impl<'v> Value<'v> {
        /// Try convert this value into a string.
        pub fn to_str(&self) -> Option<Cow<str>> {
            self.inner.to_str()
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) use super::value_bag::test::Token;

    impl<'v> Value<'v> {
        pub(crate) fn to_token(&self) -> Token {
            self.inner.to_token()
        }
    }

    fn unsigned() -> impl Iterator<Item = Value<'static>> {
        vec![
            Value::from(8u8),
            Value::from(16u16),
            Value::from(32u32),
            Value::from(64u64),
            Value::from(1usize),
        ]
        .into_iter()
    }

    fn signed() -> impl Iterator<Item = Value<'static>> {
        vec![
            Value::from(-8i8),
            Value::from(-16i16),
            Value::from(-32i32),
            Value::from(-64i64),
            Value::from(-1isize),
        ]
        .into_iter()
    }

    fn float() -> impl Iterator<Item = Value<'static>> {
        vec![Value::from(32.32f32), Value::from(64.64f64)].into_iter()
    }

    fn bool() -> impl Iterator<Item = Value<'static>> {
        vec![Value::from(true), Value::from(false)].into_iter()
    }

    fn str() -> impl Iterator<Item = Value<'static>> {
        vec![Value::from("a string"), Value::from("a loong string")].into_iter()
    }

    fn char() -> impl Iterator<Item = Value<'static>> {
        vec![Value::from('a'), Value::from('⛰')].into_iter()
    }

    #[test]
    fn test_capture_fmt() {
        assert_eq!(Some(42u64), Value::capture_display(&42).to_u64());
        assert_eq!(Some(42u64), Value::capture_debug(&42).to_u64());

        assert!(Value::from_display(&42).to_u64().is_none());
        assert!(Value::from_debug(&42).to_u64().is_none());
    }

    #[cfg(feature = "kv_unstable_std")]
    #[test]
    fn test_capture_error() {
        let err = std::io::Error::from(std::io::ErrorKind::Other);

        assert!(Value::capture_error(&err).to_borrowed_error().is_some());
        assert!(Value::from_dyn_error(&err).to_borrowed_error().is_some());
    }

    #[cfg(feature = "kv_unstable_serde")]
    #[test]
    fn test_capture_serde() {
        assert_eq!(Some(42u64), Value::capture_serde(&42).to_u64());

        assert_eq!(Some(42u64), Value::from_serde(&42).to_u64());
    }

    #[cfg(feature = "kv_unstable_sval")]
    #[test]
    fn test_capture_sval() {
        assert_eq!(Some(42u64), Value::capture_sval(&42).to_u64());

        assert_eq!(Some(42u64), Value::from_sval(&42).to_u64());
    }

    #[test]
    fn test_to_value_display() {
        assert_eq!(42u64.to_value().to_string(), "42");
        assert_eq!(42i64.to_value().to_string(), "42");
        assert_eq!(42.01f64.to_value().to_string(), "42.01");
        assert_eq!(true.to_value().to_string(), "true");
        assert_eq!('a'.to_value().to_string(), "a");
        assert_eq!("a loong string".to_value().to_string(), "a loong string");
        assert_eq!(Some(true).to_value().to_string(), "true");
        assert_eq!(().to_value().to_string(), "None");
        assert_eq!(Option::None::<bool>.to_value().to_string(), "None");
    }

    #[test]
    fn test_to_value_structured() {
        assert_eq!(42u64.to_value().to_token(), Token::U64(42));
        assert_eq!(42i64.to_value().to_token(), Token::I64(42));
        assert_eq!(42.01f64.to_value().to_token(), Token::F64(42.01));
        assert_eq!(true.to_value().to_token(), Token::Bool(true));
        assert_eq!('a'.to_value().to_token(), Token::Char('a'));
        assert_eq!(
            "a loong string".to_value().to_token(),
            Token::Str("a loong string".into())
        );
        assert_eq!(Some(true).to_value().to_token(), Token::Bool(true));
        assert_eq!(().to_value().to_token(), Token::None);
        assert_eq!(Option::None::<bool>.to_value().to_token(), Token::None);
    }

    #[test]
    fn test_to_number() {
        for v in unsigned() {
            assert!(v.to_u64().is_some());
            assert!(v.to_i64().is_some());
        }

        for v in signed() {
            assert!(v.to_i64().is_some());
        }

        for v in unsigned().chain(signed()).chain(float()) {
            assert!(v.to_f64().is_some());
        }

        for v in bool().chain(str()).chain(char()) {
            assert!(v.to_u64().is_none());
            assert!(v.to_i64().is_none());
            assert!(v.to_f64().is_none());
        }
    }

    #[test]
    fn test_to_str() {
        for v in str() {
            assert!(v.to_borrowed_str().is_some());

            #[cfg(feature = "kv_unstable_std")]
            assert!(v.to_str().is_some());
        }

        let short_lived = String::from("short lived");
        let v = Value::from(&*short_lived);

        assert!(v.to_borrowed_str().is_some());

        #[cfg(feature = "kv_unstable_std")]
        assert!(v.to_str().is_some());

        for v in unsigned().chain(signed()).chain(float()).chain(bool()) {
            assert!(v.to_borrowed_str().is_none());

            #[cfg(feature = "kv_unstable_std")]
            assert!(v.to_str().is_none());
        }
    }

    #[test]
    fn test_to_bool() {
        for v in bool() {
            assert!(v.to_bool().is_some());
        }

        for v in unsigned()
            .chain(signed())
            .chain(float())
            .chain(str())
            .chain(char())
        {
            assert!(v.to_bool().is_none());
        }
    }

    #[test]
    fn test_to_char() {
        for v in char() {
            assert!(v.to_char().is_some());
        }

        for v in unsigned()
            .chain(signed())
            .chain(float())
            .chain(str())
            .chain(bool())
        {
            assert!(v.to_char().is_none());
        }
    }

    #[test]
    fn test_downcast_ref() {
        #[derive(Debug)]
        struct Foo(u64);

        let v = Value::capture_debug(&Foo(42));

        assert!(v.is::<Foo>());
        assert_eq!(42u64, v.downcast_ref::<Foo>().expect("invalid downcast").0);
    }
}

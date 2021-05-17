use error::*;
use serde::de::{Deserialize, Deserializer, Visitor};
use std::collections::HashMap;
use std::fmt;
use std::fmt::Display;

/// Underlying kind of the configuration value.
#[derive(Debug, Clone, PartialEq)]
pub enum ValueKind {
    Nil,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Table(Table),
    Array(Array),
}

pub type Array = Vec<Value>;
pub type Table = HashMap<String, Value>;

impl Default for ValueKind {
    fn default() -> Self {
        ValueKind::Nil
    }
}

impl<T> From<Option<T>> for ValueKind
where
    T: Into<ValueKind>,
{
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => value.into(),
            None => ValueKind::Nil,
        }
    }
}

impl From<String> for ValueKind {
    fn from(value: String) -> Self {
        ValueKind::String(value)
    }
}

impl<'a> From<&'a str> for ValueKind {
    fn from(value: &'a str) -> Self {
        ValueKind::String(value.into())
    }
}

impl From<i64> for ValueKind {
    fn from(value: i64) -> Self {
        ValueKind::Integer(value)
    }
}

impl From<f64> for ValueKind {
    fn from(value: f64) -> Self {
        ValueKind::Float(value)
    }
}

impl From<bool> for ValueKind {
    fn from(value: bool) -> Self {
        ValueKind::Boolean(value)
    }
}

impl<T> From<HashMap<String, T>> for ValueKind
where
    T: Into<Value>,
{
    fn from(values: HashMap<String, T>) -> Self {
        let mut r = HashMap::new();

        for (k, v) in values {
            r.insert(k.clone(), v.into());
        }

        ValueKind::Table(r)
    }
}

impl<T> From<Vec<T>> for ValueKind
where
    T: Into<Value>,
{
    fn from(values: Vec<T>) -> Self {
        let mut l = Vec::new();

        for v in values {
            l.push(v.into());
        }

        ValueKind::Array(l)
    }
}

impl Display for ValueKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ValueKind::String(ref value) => write!(f, "{}", value),
            ValueKind::Boolean(value) => write!(f, "{}", value),
            ValueKind::Integer(value) => write!(f, "{}", value),
            ValueKind::Float(value) => write!(f, "{}", value),
            ValueKind::Nil => write!(f, "nil"),

            // TODO: Figure out a nice Display for these
            ValueKind::Table(ref table) => write!(f, "{:?}", table),
            ValueKind::Array(ref array) => write!(f, "{:?}", array),
        }
    }
}

/// A configuration value.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct Value {
    /// A description of the original location of the value.
    ///
    /// A Value originating from a File might contain:
    /// ```text
    /// Settings.toml
    /// ```
    ///
    /// A Value originating from the environment would contain:
    /// ```text
    /// the envrionment
    /// ```
    ///
    /// A Value originating from a remote source might contain:
    /// ```text
    /// etcd+http://127.0.0.1:2379
    /// ```
    origin: Option<String>,

    /// Underlying kind of the configuration value.
    pub kind: ValueKind,
}

impl Value {
    /// Create a new value instance that will remember its source uri.
    pub fn new<V>(origin: Option<&String>, kind: V) -> Self
    where
        V: Into<ValueKind>,
    {
        Value {
            origin: origin.cloned(),
            kind: kind.into(),
        }
    }

    /// Attempt to deserialize this value into the requested type.
    pub fn try_into<'de, T: Deserialize<'de>>(self) -> Result<T> {
        T::deserialize(self)
    }

    /// Returns `self` as a bool, if possible.
    // FIXME: Should this not be `try_into_*` ?
    pub fn into_bool(self) -> Result<bool> {
        match self.kind {
            ValueKind::Boolean(value) => Ok(value),
            ValueKind::Integer(value) => Ok(value != 0),
            ValueKind::Float(value) => Ok(value != 0.0),

            ValueKind::String(ref value) => {
                match value.to_lowercase().as_ref() {
                    "1" | "true" | "on" | "yes" => Ok(true),
                    "0" | "false" | "off" | "no" => Ok(false),

                    // Unexpected string value
                    s => Err(ConfigError::invalid_type(
                        self.origin.clone(),
                        Unexpected::Str(s.into()),
                        "a boolean",
                    )),
                }
            }

            // Unexpected type
            ValueKind::Nil => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Unit,
                "a boolean",
            )),
            ValueKind::Table(_) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Map,
                "a boolean",
            )),
            ValueKind::Array(_) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Seq,
                "a boolean",
            )),
        }
    }

    /// Returns `self` into an i64, if possible.
    // FIXME: Should this not be `try_into_*` ?
    pub fn into_int(self) -> Result<i64> {
        match self.kind {
            ValueKind::Integer(value) => Ok(value),

            ValueKind::String(ref s) => {
                match s.to_lowercase().as_ref() {
                    "true" | "on" | "yes" => Ok(1),
                    "false" | "off" | "no" => Ok(0),
                    _ => {
                        s.parse().map_err(|_| {
                            // Unexpected string
                            ConfigError::invalid_type(
                                self.origin.clone(),
                                Unexpected::Str(s.clone()),
                                "an integer",
                            )
                        })
                    }
                }
            }

            ValueKind::Boolean(value) => Ok(if value { 1 } else { 0 }),
            ValueKind::Float(value) => Ok(value.round() as i64),

            // Unexpected type
            ValueKind::Nil => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Unit,
                "an integer",
            )),
            ValueKind::Table(_) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Map,
                "an integer",
            )),
            ValueKind::Array(_) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Seq,
                "an integer",
            )),
        }
    }

    /// Returns `self` into a f64, if possible.
    // FIXME: Should this not be `try_into_*` ?
    pub fn into_float(self) -> Result<f64> {
        match self.kind {
            ValueKind::Float(value) => Ok(value),

            ValueKind::String(ref s) => {
                match s.to_lowercase().as_ref() {
                    "true" | "on" | "yes" => Ok(1.0),
                    "false" | "off" | "no" => Ok(0.0),
                    _ => {
                        s.parse().map_err(|_| {
                            // Unexpected string
                            ConfigError::invalid_type(
                                self.origin.clone(),
                                Unexpected::Str(s.clone()),
                                "a floating point",
                            )
                        })
                    }
                }
            }

            ValueKind::Integer(value) => Ok(value as f64),
            ValueKind::Boolean(value) => Ok(if value { 1.0 } else { 0.0 }),

            // Unexpected type
            ValueKind::Nil => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Unit,
                "a floating point",
            )),
            ValueKind::Table(_) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Map,
                "a floating point",
            )),
            ValueKind::Array(_) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Seq,
                "a floating point",
            )),
        }
    }

    /// Returns `self` into a str, if possible.
    // FIXME: Should this not be `try_into_*` ?
    pub fn into_str(self) -> Result<String> {
        match self.kind {
            ValueKind::String(value) => Ok(value),

            ValueKind::Boolean(value) => Ok(value.to_string()),
            ValueKind::Integer(value) => Ok(value.to_string()),
            ValueKind::Float(value) => Ok(value.to_string()),

            // Cannot convert
            ValueKind::Nil => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Unit,
                "a string",
            )),
            ValueKind::Table(_) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Map,
                "a string",
            )),
            ValueKind::Array(_) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Seq,
                "a string",
            )),
        }
    }

    /// Returns `self` into an array, if possible
    // FIXME: Should this not be `try_into_*` ?
    pub fn into_array(self) -> Result<Vec<Value>> {
        match self.kind {
            ValueKind::Array(value) => Ok(value),

            // Cannot convert
            ValueKind::Float(value) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Float(value),
                "an array",
            )),
            ValueKind::String(value) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Str(value),
                "an array",
            )),
            ValueKind::Integer(value) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Integer(value),
                "an array",
            )),
            ValueKind::Boolean(value) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Bool(value),
                "an array",
            )),
            ValueKind::Nil => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Unit,
                "an array",
            )),
            ValueKind::Table(_) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Map,
                "an array",
            )),
        }
    }

    /// If the `Value` is a Table, returns the associated Map.
    // FIXME: Should this not be `try_into_*` ?
    pub fn into_table(self) -> Result<HashMap<String, Value>> {
        match self.kind {
            ValueKind::Table(value) => Ok(value),

            // Cannot convert
            ValueKind::Float(value) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Float(value),
                "a map",
            )),
            ValueKind::String(value) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Str(value),
                "a map",
            )),
            ValueKind::Integer(value) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Integer(value),
                "a map",
            )),
            ValueKind::Boolean(value) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Bool(value),
                "a map",
            )),
            ValueKind::Nil => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Unit,
                "a map",
            )),
            ValueKind::Array(_) => Err(ConfigError::invalid_type(
                self.origin,
                Unexpected::Seq,
                "a map",
            )),
        }
    }
}

impl<'de> Deserialize<'de> for Value {
    #[inline]
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ValueVisitor;

        impl<'de> Visitor<'de> for ValueVisitor {
            type Value = Value;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("any valid configuration value")
            }

            #[inline]
            fn visit_bool<E>(self, value: bool) -> ::std::result::Result<Value, E> {
                Ok(value.into())
            }

            #[inline]
            fn visit_i8<E>(self, value: i8) -> ::std::result::Result<Value, E> {
                Ok((value as i64).into())
            }

            #[inline]
            fn visit_i16<E>(self, value: i16) -> ::std::result::Result<Value, E> {
                Ok((value as i64).into())
            }

            #[inline]
            fn visit_i32<E>(self, value: i32) -> ::std::result::Result<Value, E> {
                Ok((value as i64).into())
            }

            #[inline]
            fn visit_i64<E>(self, value: i64) -> ::std::result::Result<Value, E> {
                Ok(value.into())
            }

            #[inline]
            fn visit_u8<E>(self, value: u8) -> ::std::result::Result<Value, E> {
                Ok((value as i64).into())
            }

            #[inline]
            fn visit_u16<E>(self, value: u16) -> ::std::result::Result<Value, E> {
                Ok((value as i64).into())
            }

            #[inline]
            fn visit_u32<E>(self, value: u32) -> ::std::result::Result<Value, E> {
                Ok((value as i64).into())
            }

            #[inline]
            fn visit_u64<E>(self, value: u64) -> ::std::result::Result<Value, E> {
                // FIXME: This is bad
                Ok((value as i64).into())
            }

            #[inline]
            fn visit_f64<E>(self, value: f64) -> ::std::result::Result<Value, E> {
                Ok(value.into())
            }

            #[inline]
            fn visit_str<E>(self, value: &str) -> ::std::result::Result<Value, E>
            where
                E: ::serde::de::Error,
            {
                self.visit_string(String::from(value))
            }

            #[inline]
            fn visit_string<E>(self, value: String) -> ::std::result::Result<Value, E> {
                Ok(value.into())
            }

            #[inline]
            fn visit_none<E>(self) -> ::std::result::Result<Value, E> {
                Ok(Value::new(None, ValueKind::Nil))
            }

            #[inline]
            fn visit_some<D>(self, deserializer: D) -> ::std::result::Result<Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                Deserialize::deserialize(deserializer)
            }

            #[inline]
            fn visit_unit<E>(self) -> ::std::result::Result<Value, E> {
                Ok(Value::new(None, ValueKind::Nil))
            }

            #[inline]
            fn visit_seq<V>(self, mut visitor: V) -> ::std::result::Result<Value, V::Error>
            where
                V: ::serde::de::SeqAccess<'de>,
            {
                let mut vec = Array::new();

                while let Some(elem) = visitor.next_element()? {
                    vec.push(elem);
                }

                Ok(vec.into())
            }

            fn visit_map<V>(self, mut visitor: V) -> ::std::result::Result<Value, V::Error>
            where
                V: ::serde::de::MapAccess<'de>,
            {
                let mut values = Table::new();

                while let Some((key, value)) = visitor.next_entry()? {
                    values.insert(key, value);
                }

                Ok(values.into())
            }
        }

        deserializer.deserialize_any(ValueVisitor)
    }
}

impl<T> From<T> for Value
where
    T: Into<ValueKind>,
{
    fn from(value: T) -> Self {
        Value {
            origin: None,
            kind: value.into(),
        }
    }
}

impl Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.kind)
    }
}

use nom;
use serde::de;
use serde::ser;
use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::result;

#[derive(Debug)]
pub enum Unexpected {
    Bool(bool),
    Integer(i64),
    Float(f64),
    Str(String),
    Unit,
    Seq,
    Map,
}

impl fmt::Display for Unexpected {
    fn fmt(&self, f: &mut fmt::Formatter) -> result::Result<(), fmt::Error> {
        match *self {
            Unexpected::Bool(b) => write!(f, "boolean `{}`", b),
            Unexpected::Integer(i) => write!(f, "integer `{}`", i),
            Unexpected::Float(v) => write!(f, "floating point `{}`", v),
            Unexpected::Str(ref s) => write!(f, "string {:?}", s),
            Unexpected::Unit => write!(f, "unit value"),
            Unexpected::Seq => write!(f, "sequence"),
            Unexpected::Map => write!(f, "map"),
        }
    }
}

/// Represents all possible errors that can occur when working with
/// configuration.
pub enum ConfigError {
    /// Configuration is frozen and no further mutations can be made.
    Frozen,

    /// Configuration property was not found
    NotFound(String),

    /// Configuration path could not be parsed.
    PathParse(nom::error::ErrorKind),

    /// Configuration could not be parsed from file.
    FileParse {
        /// The URI used to access the file (if not loaded from a string).
        /// Example: `/path/to/config.json`
        uri: Option<String>,

        /// The captured error from attempting to parse the file in its desired format.
        /// This is the actual error object from the library used for the parsing.
        cause: Box<dyn Error + Send + Sync>,
    },

    /// Value could not be converted into the requested type.
    Type {
        /// The URI that references the source that the value came from.
        /// Example: `/path/to/config.json` or `Environment` or `etcd://localhost`
        // TODO: Why is this called Origin but FileParse has a uri field?
        origin: Option<String>,

        /// What we found when parsing the value
        unexpected: Unexpected,

        /// What was expected when parsing the value
        expected: &'static str,

        /// The key in the configuration hash of this value (if available where the
        /// error is generated).
        key: Option<String>,
    },

    /// Custom message
    Message(String),

    /// Unadorned error from a foreign origin.
    Foreign(Box<dyn Error + Send + Sync>),
}

impl ConfigError {
    // FIXME: pub(crate)
    #[doc(hidden)]
    pub fn invalid_type(
        origin: Option<String>,
        unexpected: Unexpected,
        expected: &'static str,
    ) -> Self {
        ConfigError::Type {
            origin,
            unexpected,
            expected,
            key: None,
        }
    }

    // FIXME: pub(crate)
    #[doc(hidden)]
    pub fn extend_with_key(self, key: &str) -> Self {
        match self {
            ConfigError::Type {
                origin,
                unexpected,
                expected,
                ..
            } => ConfigError::Type {
                origin,
                unexpected,
                expected,
                key: Some(key.into()),
            },

            _ => self,
        }
    }

    fn prepend(self, segment: String, add_dot: bool) -> Self {
        let concat = |key: Option<String>| {
            let key = key.unwrap_or_else(String::new);
            let dot = if add_dot && key.as_bytes().get(0).unwrap_or(&b'[') != &b'[' {
                "."
            } else {
                ""
            };
            format!("{}{}{}", segment, dot, key)
        };
        match self {
            ConfigError::Type {
                origin,
                unexpected,
                expected,
                key,
            } => ConfigError::Type {
                origin,
                unexpected,
                expected,
                key: Some(concat(key)),
            },
            ConfigError::NotFound(key) => ConfigError::NotFound(concat(Some(key))),
            _ => self,
        }
    }

    pub(crate) fn prepend_key(self, key: String) -> Self {
        self.prepend(key, true)
    }

    pub(crate) fn prepend_index(self, idx: usize) -> Self {
        self.prepend(format!("[{}]", idx), false)
    }
}

/// Alias for a `Result` with the error type set to `ConfigError`.
pub type Result<T> = result::Result<T, ConfigError>;

// Forward Debug to Display for readable panic! messages
impl fmt::Debug for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", *self)
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ConfigError::Frozen => write!(f, "configuration is frozen"),

            ConfigError::PathParse(ref kind) => write!(f, "{}", kind.description()),

            ConfigError::Message(ref s) => write!(f, "{}", s),

            ConfigError::Foreign(ref cause) => write!(f, "{}", cause),

            ConfigError::NotFound(ref key) => {
                write!(f, "configuration property {:?} not found", key)
            }

            ConfigError::Type {
                ref origin,
                ref unexpected,
                expected,
                ref key,
            } => {
                write!(f, "invalid type: {}, expected {}", unexpected, expected)?;

                if let Some(ref key) = *key {
                    write!(f, " for key `{}`", key)?;
                }

                if let Some(ref origin) = *origin {
                    write!(f, " in {}", origin)?;
                }

                Ok(())
            }

            ConfigError::FileParse { ref cause, ref uri } => {
                write!(f, "{}", cause)?;

                if let Some(ref uri) = *uri {
                    write!(f, " in {}", uri)?;
                }

                Ok(())
            }
        }
    }
}

impl Error for ConfigError {}

impl de::Error for ConfigError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        ConfigError::Message(msg.to_string())
    }
}

impl ser::Error for ConfigError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        ConfigError::Message(msg.to_string())
    }
}

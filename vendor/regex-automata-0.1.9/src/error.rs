use std::error;
use std::fmt;
use std::result;

use regex_syntax;

pub type Result<T> = result::Result<T, Error>;

/// An error that occurred during the construction of a DFA.
#[derive(Clone, Debug)]
pub struct Error {
    kind: ErrorKind,
}

/// The kind of error that occurred.
#[derive(Clone, Debug)]
pub enum ErrorKind {
    /// An error that occurred while parsing a regular expression. Note that
    /// this error may be printed over multiple lines, and is generally
    /// intended to be end user readable on its own.
    Syntax(String),
    /// An error that occurred because an unsupported regex feature was used.
    /// The message string describes which unsupported feature was used.
    ///
    /// The primary regex features that are unsupported are those that require
    /// look-around, such as the `^` and `$` anchors and the word boundary
    /// assertion `\b`. These may be supported in the future.
    Unsupported(String),
    /// An error that occurred when attempting to serialize a DFA to bytes.
    Serialize(String),
    /// An error that occurs when constructing a DFA would require the use of
    /// a state ID that overflows the chosen state ID representation. For
    /// example, if one is using `u8` for state IDs and builds a DFA with
    /// 257 states, then the last state's ID will be `256` which cannot be
    /// represented with `u8`.
    ///
    /// Typically, this error occurs in the determinization process of building
    /// a DFA (the conversion step from NFA to DFA). It can also occur when
    /// trying to build a smaller DFA from an existing one.
    StateIDOverflow {
        /// The maximum possible state ID.
        max: usize,
    },
    /// An error that occurs when premultiplication of state IDs is requested,
    /// but doing so would overflow the chosen state ID representation.
    ///
    /// When `max == requested_max`, then the state ID would overflow `usize`.
    PremultiplyOverflow {
        /// The maximum possible state id.
        max: usize,
        /// The maximum ID required by premultiplication.
        requested_max: usize,
    },
}

impl Error {
    /// Return the kind of this error.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    pub(crate) fn syntax(err: regex_syntax::Error) -> Error {
        Error { kind: ErrorKind::Syntax(err.to_string()) }
    }

    pub(crate) fn unsupported_anchor() -> Error {
        let msg = r"anchors such as ^, $, \A and \z are not supported";
        Error { kind: ErrorKind::Unsupported(msg.to_string()) }
    }

    pub(crate) fn unsupported_word() -> Error {
        let msg = r"word boundary assertions (\b and \B) are not supported";
        Error { kind: ErrorKind::Unsupported(msg.to_string()) }
    }

    pub(crate) fn unsupported_longest_match() -> Error {
        let msg = "unachored searches with longest match \
                   semantics are not supported";
        Error { kind: ErrorKind::Unsupported(msg.to_string()) }
    }

    pub(crate) fn serialize(message: &str) -> Error {
        Error { kind: ErrorKind::Serialize(message.to_string()) }
    }

    pub(crate) fn state_id_overflow(max: usize) -> Error {
        Error { kind: ErrorKind::StateIDOverflow { max } }
    }

    pub(crate) fn premultiply_overflow(
        max: usize,
        requested_max: usize,
    ) -> Error {
        Error { kind: ErrorKind::PremultiplyOverflow { max, requested_max } }
    }
}

impl error::Error for Error {
    fn description(&self) -> &str {
        match self.kind {
            ErrorKind::Syntax(_) => "syntax error",
            ErrorKind::Unsupported(_) => "unsupported syntax",
            ErrorKind::Serialize(_) => "serialization error",
            ErrorKind::StateIDOverflow { .. } => {
                "state id representation too small"
            }
            ErrorKind::PremultiplyOverflow { .. } => {
                "state id representation too small for premultiplication"
            }
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.kind {
            ErrorKind::Syntax(ref msg) => write!(f, "{}", msg),
            ErrorKind::Unsupported(ref msg) => write!(f, "{}", msg),
            ErrorKind::Serialize(ref msg) => {
                write!(f, "DFA serialization error: {}", msg)
            }
            ErrorKind::StateIDOverflow { max } => write!(
                f,
                "building the DFA failed because it required building \
                 more states that can be identified, where the maximum \
                 ID for the chosen representation is {}",
                max,
            ),
            ErrorKind::PremultiplyOverflow { max, requested_max } => {
                if max == requested_max {
                    write!(
                        f,
                        "premultiplication of states requires the ability to \
                         represent a state ID greater than what can fit on \
                         this platform's usize, which is {}",
                        ::std::usize::MAX,
                    )
                } else {
                    write!(
                        f,
                        "premultiplication of states requires the ability to \
                         represent at least a state ID of {}, but the chosen \
                         representation only permits a maximum state ID of {}",
                        requested_max, max,
                    )
                }
            }
        }
    }
}

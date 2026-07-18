//! String-related utilities and extension traits.
mod command_str;
#[cfg(feature = "unicode")]
pub mod ellipsis;
mod escape_non_printable_posix_ext;

pub use command_str::{CommandStr, ContainsNul};
#[cfg(feature = "unicode")]
pub use ellipsis::EllipsizeExt;
pub use escape_non_printable_posix_ext::EscapeNonPrintablePosixExt;

//! Byte counts and percentages, in the text forms used by `config.toml` (`1MB`, `10%`).
//!
//! Both types support ordinary arithmetic through operator overloads, and a [`Percent`] can be
//! multiplied with any primitive number or with a [`ByteSize`] to take that share of it. One rule
//! covers all of it: integer arithmetic here **saturates** instead of overflowing. These values
//! come from config files and disk accounting, where a result that does not fit means "as much as
//! there is", and where panicking would be worse than clamping. Float shares are plain float math.

mod byte_size;
mod percent;
pub mod text_or_bytes;

pub use byte_size::{ByteSize, ByteSizeParseError, HumanByteSize};
pub use percent::{Percent, PercentParseError};

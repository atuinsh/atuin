//! atuin's MessagePack helpers: the [`decode`] and [`encode`] submodules mirror
//! `rmp`'s own module layout, but every `read_*`/`write_*` returns our own
//! [`DecodeError`]/[`EncodeError`] (which convert into [`eyre::Report`]) and the
//! modules add owned-string, optional, array-length and end-of-input helpers.

pub mod decode;
pub mod encode;

pub use decode::DecodeError;
pub use encode::EncodeError;

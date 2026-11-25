//! JSON utilities: framed lists, streaming, and the schemas of config files.

pub mod framed;
pub mod js;
#[cfg(feature = "jsonl")]
pub mod jsonl;
pub mod schema;

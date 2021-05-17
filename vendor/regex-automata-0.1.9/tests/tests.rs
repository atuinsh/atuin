#[cfg(feature = "std")]
#[macro_use]
extern crate lazy_static;
#[cfg(feature = "std")]
extern crate regex;
#[cfg(feature = "std")]
extern crate regex_automata;
#[cfg(feature = "std")]
extern crate serde;
#[cfg(feature = "std")]
extern crate serde_bytes;
#[cfg(feature = "std")]
#[macro_use]
extern crate serde_derive;
#[cfg(feature = "std")]
extern crate toml;

#[cfg(feature = "std")]
mod collection;
#[cfg(feature = "std")]
mod regression;
#[cfg(feature = "std")]
mod suite;
#[cfg(feature = "std")]
mod unescape;

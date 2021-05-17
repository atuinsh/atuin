//! Module specifying float.

// Utilities.
mod exponent;
mod trim;
mod validate;

#[macro_use]
mod interface;

#[macro_use]
mod traits;

// Formats
mod standard;

cfg_if! {
if #[cfg(feature = "format")] {
    mod generic;
    mod permissive;
    mod ignore;
}}

// Re-export interface and traits.
pub(super) use standard::*;
pub(super) use traits::*;

cfg_if! {
if #[cfg(feature = "format")] {
    pub(super) use generic::*;
    pub(super) use permissive::*;
    pub(super) use ignore::*;
}}

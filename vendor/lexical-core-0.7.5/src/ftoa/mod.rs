//! Fast lexical float-to-string conversion routines.

// Hide implementation details.
#[cfg(feature = "radix")]
mod radix;

cfg_if! {
if #[cfg(feature = "grisu3")] {
    mod grisu3;
} else if #[cfg(feature = "ryu")] {
    mod ryu;
} else {
    mod grisu2;
}}  // cfg_if

mod api;

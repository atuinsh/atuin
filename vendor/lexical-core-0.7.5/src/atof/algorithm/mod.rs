//! Algorithms for parsing strings to floats.

// Hide implementation details.
#[macro_use]
mod format;

cfg_if! {
if #[cfg(feature = "correct")] {
mod bhcomp;
mod bigcomp;
mod alias;
mod bignum;
mod cached;
mod cached_float80;
mod cached_float160;
mod errors;
mod large_powers;
mod math;
mod small_powers;

#[cfg(limb_width_32)]
mod large_powers_32;

#[cfg(limb_width_32)]
mod small_powers_32;

#[cfg(limb_width_64)]
mod large_powers_64;

// Required for fast-path, keep on all platforms.
mod small_powers_64;

}}  // cfg_if

// Export algorithms.
#[cfg(feature = "correct")]
pub(crate) mod correct;

#[cfg(not(feature = "correct"))]
pub(crate) mod incorrect;

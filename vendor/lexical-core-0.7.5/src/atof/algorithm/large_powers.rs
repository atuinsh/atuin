//! Precalculated large powers for prime numbers for `b^2^i`.
//!
//! We only need powers such that `b^n <= 2^1075` for `bigcomp`.
//! However, for `bhcomp`, we need at least as many digits as are
//! input. We tentatively accept up to ~2^15.
//!
//! The larger powers are **quite** large (~3Kb per radix), so we'd rather
//! not include them in binaries unless necessary.

use super::math::Limb;

#[cfg(limb_width_32)]
use super::large_powers_32::*;

#[cfg(limb_width_64)]
use super::large_powers_64::*;

// HELPER

/// Get the correct large power from the radix.
#[allow(unused_variables)]
pub(in crate::atof::algorithm) fn get_large_powers(radix: u32)
    -> &'static [&'static [Limb]]
{
    #[cfg(not(feature = "radix"))] {
        &POW5
    }

    #[cfg(feature = "radix")] {
        match radix {
            3  => &POW3,
            5  => &POW5,
            7  => &POW7,
            11  => &POW11,
            13  => &POW13,
            17  => &POW17,
            19  => &POW19,
            23  => &POW23,
            29  => &POW29,
            31  => &POW31,
            _  => unreachable!(),
        }
    }
}

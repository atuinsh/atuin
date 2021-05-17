//! Precalculated small powers.

use super::math::Limb;
use super::small_powers_64;

#[cfg(limb_width_32)]
use super::small_powers_32::*;

#[cfg(limb_width_64)]
use super::small_powers_64::*;

// ASSERTIONS
const_assert!(POW5[1] / POW5[0] == 5);
const_assert!(POW10[1] / POW10[0] == 10);

cfg_if! {
if #[cfg(feature = "radix")] {
// Ensure our small powers are valid.
const_assert!(POW2[1] / POW2[0] == 2);
const_assert!(POW3[1] / POW3[0] == 3);
const_assert!(POW4[1] / POW4[0] == 4);
const_assert!(POW6[1] / POW6[0] == 6);
const_assert!(POW7[1] / POW7[0] == 7);
const_assert!(POW8[1] / POW8[0] == 8);
const_assert!(POW9[1] / POW9[0] == 9);
const_assert!(POW11[1] / POW11[0] == 11);
const_assert!(POW12[1] / POW12[0] == 12);
const_assert!(POW13[1] / POW13[0] == 13);
const_assert!(POW14[1] / POW14[0] == 14);
const_assert!(POW15[1] / POW15[0] == 15);
const_assert!(POW16[1] / POW16[0] == 16);
const_assert!(POW17[1] / POW17[0] == 17);
const_assert!(POW18[1] / POW18[0] == 18);
const_assert!(POW19[1] / POW19[0] == 19);
const_assert!(POW20[1] / POW20[0] == 20);
const_assert!(POW21[1] / POW21[0] == 21);
const_assert!(POW22[1] / POW22[0] == 22);
const_assert!(POW23[1] / POW23[0] == 23);
const_assert!(POW24[1] / POW24[0] == 24);
const_assert!(POW25[1] / POW25[0] == 25);
const_assert!(POW26[1] / POW26[0] == 26);
const_assert!(POW27[1] / POW27[0] == 27);
const_assert!(POW28[1] / POW28[0] == 28);
const_assert!(POW29[1] / POW29[0] == 29);
const_assert!(POW30[1] / POW30[0] == 30);
const_assert!(POW31[1] / POW31[0] == 31);
const_assert!(POW32[1] / POW32[0] == 32);
const_assert!(POW33[1] / POW33[0] == 33);
const_assert!(POW34[1] / POW34[0] == 34);
const_assert!(POW35[1] / POW35[0] == 35);
const_assert!(POW36[1] / POW36[0] == 36);

}} //cfg_if

// HELPER

/// Get the correct small power from the radix.
pub(in crate::atof::algorithm) fn get_small_powers(radix: u32)
    -> &'static [Limb]
{
    #[cfg(not(feature = "radix"))] {
        match radix {
            5  => &POW5,
            10 => &POW10,
            _  => unreachable!()
        }
    }

    #[cfg(feature = "radix")] {
        match radix {
            2  => &POW2,
            3  => &POW3,
            4  => &POW4,
            5  => &POW5,
            6  => &POW6,
            7  => &POW7,
            8  => &POW8,
            9  => &POW9,
            10  => &POW10,
            11  => &POW11,
            12  => &POW12,
            13  => &POW13,
            14  => &POW14,
            15  => &POW15,
            16  => &POW16,
            17  => &POW17,
            18  => &POW18,
            19  => &POW19,
            20  => &POW20,
            21  => &POW21,
            22  => &POW22,
            23  => &POW23,
            24  => &POW24,
            25  => &POW25,
            26  => &POW26,
            27  => &POW27,
            28  => &POW28,
            29  => &POW29,
            30  => &POW30,
            31  => &POW31,
            32  => &POW32,
            33  => &POW33,
            34  => &POW34,
            35  => &POW35,
            36  => &POW36,
            _  => unreachable!(),
        }
    }
}

/// Get the correct 64-bit small power from the radix.
pub(in crate::atof::algorithm) fn get_small_powers_64(radix: u32)
    -> &'static [u64]
{
    #[cfg(not(feature = "radix"))] {
        match radix {
            5  => &small_powers_64::POW5,
            10 => &small_powers_64::POW10,
            _  => unreachable!()
        }
    }

    #[cfg(feature = "radix")] {
        match radix {
            2  => &small_powers_64::POW2,
            3  => &small_powers_64::POW3,
            4  => &small_powers_64::POW4,
            5  => &small_powers_64::POW5,
            6  => &small_powers_64::POW6,
            7  => &small_powers_64::POW7,
            8  => &small_powers_64::POW8,
            9  => &small_powers_64::POW9,
            10  => &small_powers_64::POW10,
            11  => &small_powers_64::POW11,
            12  => &small_powers_64::POW12,
            13  => &small_powers_64::POW13,
            14  => &small_powers_64::POW14,
            15  => &small_powers_64::POW15,
            16  => &small_powers_64::POW16,
            17  => &small_powers_64::POW17,
            18  => &small_powers_64::POW18,
            19  => &small_powers_64::POW19,
            20  => &small_powers_64::POW20,
            21  => &small_powers_64::POW21,
            22  => &small_powers_64::POW22,
            23  => &small_powers_64::POW23,
            24  => &small_powers_64::POW24,
            25  => &small_powers_64::POW25,
            26  => &small_powers_64::POW26,
            27  => &small_powers_64::POW27,
            28  => &small_powers_64::POW28,
            29  => &small_powers_64::POW29,
            30  => &small_powers_64::POW30,
            31  => &small_powers_64::POW31,
            32  => &small_powers_64::POW32,
            33  => &small_powers_64::POW33,
            34  => &small_powers_64::POW34,
            35  => &small_powers_64::POW35,
            36  => &small_powers_64::POW36,
            _  => unreachable!(),
        }
    }
}

//! Macros for bit-wise shifts.

use crate::lib::mem;
use crate::util::*;
use super::float::ExtendedFloat;
use super::mantissa::Mantissa;

// SHIFT RIGHT

// Shift extended-precision float right `shift` bytes.
perftools_inline!{
pub(super) fn shr<M: Mantissa, T: Integer>(fp: &mut ExtendedFloat<M>, shift: T)
{
    let bits: T = as_cast(mem::size_of::<M>() * 8);
    debug_assert!(shift < bits, "shr() overflow in shift right.");

    fp.mant >>= as_cast::<M, _>(shift);
    fp.exp += shift.as_i32();
}}

// Shift extended-precision float right `shift` bytes.
//
// Accepts when the shift is the same as the type size, and
// sets the value to 0.
perftools_inline!{
pub(super) fn overflowing_shr<M: Mantissa, T: Integer>(fp: &mut ExtendedFloat<M>, shift: T)
{
    let bits: T = as_cast(mem::size_of::<M>() * 8);
    debug_assert!(shift <= bits, "overflowing_shr() overflow in shift right.");

    fp.mant = match shift == bits {
        true  => M::ZERO,
        false => fp.mant >> as_cast::<M, _>(shift),
    };
    fp.exp += shift.as_i32();
}}

// Shift extended-precision float left `shift` bytes.
perftools_inline!{
pub(super) fn shl<M: Mantissa, T: Integer>(fp: &mut ExtendedFloat<M>, shift: T)
{
    let bits: T = as_cast(mem::size_of::<M>() * 8);
    debug_assert!(shift < bits, "shl() overflow in shift left.");

    fp.mant <<= as_cast::<M, _>(shift);
    fp.exp -= shift.as_i32();
}}

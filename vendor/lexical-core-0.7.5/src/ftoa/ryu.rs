//! Wrapper around David Tolnay's ryu.

use ryu::raw;
use crate::util::*;

// F32

perftools_inline!{
/// Wrapper for ryu.
///
/// `f` must be non-special (NaN or infinite), non-negative,
/// and non-zero.
pub(crate) fn float_decimal<'a>(f: f32, bytes: &'a mut [u8])
    -> usize
{
    unsafe {
        raw::format32(f, bytes.as_mut_ptr())
    }
}}

// F64

perftools_inline!{
/// Wrapper for ryu.
///
/// `d` must be non-special (NaN or infinite), non-negative,
/// and non-zero.
pub(crate) fn double_decimal<'a>(d: f64, bytes: &'a mut [u8])
    -> usize
{
    unsafe {
        raw::format64(d, bytes.as_mut_ptr())
    }
}}

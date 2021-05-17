//! Wrapped float that behaves like an integer.
//!
//! Comprehensive wrapper for the Float trait that acts like an Integer
//! trait, allowed floats to be mocked as integers.
//! Operations natively supported by floats are wrapped, while
//! those that can be mocked (like overflow-checked operations)
//! are implemented, and others (like bitshift assigns) are unimplemented.

use crate::lib::{cmp, fmt, iter, ops};
use super::cast::*;
use super::config::*;
use super::num::*;
use super::primitive::*;

// WRAPPED FLOAT

/// Wrap a float to act like an integer.
///
/// Required for the lossy atof algorithm.
#[derive(Clone, Copy, Debug, PartialOrd)]
pub(crate) struct WrappedFloat<T: Float>
{
    /// Internal data.
    data: T,
}

impl<T: Float> WrappedFloat<T> {
    /// Wrap existing float.
    #[inline]
    pub fn from_float(t: T) -> Self {
        WrappedFloat { data: t }
    }

    /// Consume wrapper and return float.
    #[inline]
    pub fn into_inner(self) -> T {
        self.data
    }
}

// IMPL AS PRIMITIVE

impl<T: Float> PartialEq for WrappedFloat<T> {
    fn eq(&self, other: &Self) -> bool {
        // Need to return true when both are NaN, since the default
        // PartialEq for floats returns false when both are NaN.
        // We demand total ordering, do it the right way,
        if self.data.is_nan() && other.data.is_nan() {
            true
        } else {
            self.data == other.data
        }
    }
}

impl<T: Float> AsPrimitive for WrappedFloat<T> {
    #[inline]
    fn as_u8(self) -> u8 {
        as_cast(self.data)
    }

    #[inline]
    fn as_u16(self) -> u16 {
        as_cast(self.data)
    }

    #[inline]
    fn as_u32(self) -> u32 {
        as_cast(self.data)
    }

    #[inline]
    fn as_u64(self) -> u64 {
        as_cast(self.data)
    }

    #[inline]
    fn as_u128(self) -> u128 {
        as_cast(self.data)
    }

    #[inline]
    fn as_usize(self) -> usize {
        as_cast(self.data)
    }

    #[inline]
    fn as_i8(self) -> i8 {
        as_cast(self.data)
    }

    #[inline]
    fn as_i16(self) -> i16 {
        as_cast(self.data)
    }

    #[inline]
    fn as_i32(self) -> i32 {
        as_cast(self.data)
    }

    #[inline]
    fn as_i64(self) -> i64 {
        as_cast(self.data)
    }

    #[inline]
    fn as_i128(self) -> i128 {
        as_cast(self.data)
    }

    #[inline]
    fn as_isize(self) -> isize {
        as_cast(self.data)
    }

    #[inline]
    fn as_f32(self) -> f32 {
        as_cast(self.data)
    }

    #[inline]
    fn as_f64(self) -> f64 {
        as_cast(self.data)
    }
}

// IMPL TRY PRIMITIVE

impl<T: Float> TryPrimitive for WrappedFloat<T> {
    #[inline]
    fn try_u8(self) -> Option<u8> {
        try_cast(self.data)
    }

    #[inline]
    fn try_u16(self) -> Option<u16> {
        try_cast(self.data)
    }

    #[inline]
    fn try_u32(self) -> Option<u32> {
        try_cast(self.data)
    }

    #[inline]
    fn try_u64(self) -> Option<u64> {
        try_cast(self.data)
    }

    #[inline]
    fn try_u128(self) -> Option<u128> {
        try_cast(self.data)
    }

    #[inline]
    fn try_usize(self) -> Option<usize> {
        try_cast(self.data)
    }

    #[inline]
    fn try_i8(self) -> Option<i8> {
        try_cast(self.data)
    }

    #[inline]
    fn try_i16(self) -> Option<i16> {
        try_cast(self.data)
    }

    #[inline]
    fn try_i32(self) -> Option<i32> {
        try_cast(self.data)
    }

    #[inline]
    fn try_i64(self) -> Option<i64> {
        try_cast(self.data)
    }

    #[inline]
    fn try_i128(self) -> Option<i128> {
        try_cast(self.data)
    }

    #[inline]
    fn try_isize(self) -> Option<isize> {
        try_cast(self.data)
    }

    #[inline]
    fn try_f32(self) -> Option<f32> {
        try_cast(self.data)
    }

    #[inline]
    fn try_f64(self) -> Option<f64> {
        try_cast(self.data)
    }
}

// IMPL AS CAST

impl<T: Float> AsCast for WrappedFloat<T> {
    #[inline]
    fn as_cast<N: AsPrimitive>(n: N) -> Self {
        // Wrap to wide float and back down. Should be a no-op. Just indirection.
        WrappedFloat { data: as_cast(n.as_f64()) }
    }
}

// IMPL TRY CAST

impl<N: Primitive, T: Float + TryCast<N>> TryCast<N> for WrappedFloat<T> {
    #[inline]
    fn try_cast(self) -> Option<N> {
        try_cast(self.data)
    }
}

// IMPL PRIMITIVE

impl<T: Float> fmt::Display for WrappedFloat<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.data, f)
    }
}

impl<T: Float> Primitive for WrappedFloat<T> {
}

// IMPL NUMBER

impl<T: Float> iter::Product for WrappedFloat<T> {
    #[inline]
    fn product<Iter: Iterator<Item=WrappedFloat<T>>>(iter: Iter) -> Self {
        iter.fold(Self::from_float(T::ONE), ops::Mul::mul)
    }
}

impl<T: Float> iter::Sum for WrappedFloat<T> {
    #[inline]
    fn sum<Iter: Iterator<Item=WrappedFloat<T>>>(iter: Iter) -> Self {
        iter.fold(Self::from_float(T::ZERO), ops::Add::add)
    }
}

/// Implement arithmetic operations.
macro_rules! ops_impl {
    ($($t:ident, $meth:ident ;)*) => ($(
        impl<T: Float> ops::$t for WrappedFloat<T> {
            type Output = Self;

            #[inline]
            fn $meth(self, other: Self) -> Self::Output {
                WrappedFloat { data: self.data.$meth(other.data) }
            }
        }
    )*);
}

ops_impl! {
    Add, add ;
    Div, div ;
    Mul, mul ;
    Rem, rem ;
    Sub, sub ;
}

/// Implement arithmetic assignment operations.
macro_rules! ops_assign_impl {
    ($($t:ident, $meth:ident ;)*) => ($(
        impl<T: Float> ops::$t for WrappedFloat<T> {
            #[inline]
            fn $meth(&mut self, other: Self) {
                self.data.$meth(other.data)
            }
        }
    )*);
}

ops_assign_impl! {
    AddAssign, add_assign ;
    DivAssign, div_assign ;
    MulAssign, mul_assign ;
    RemAssign, rem_assign ;
    SubAssign, sub_assign ;
}

impl<T: Float> Number for WrappedFloat<T> {
    const FORMATTED_SIZE: usize = T::FORMATTED_SIZE;
    const FORMATTED_SIZE_DECIMAL: usize = T::FORMATTED_SIZE_DECIMAL;
    const IS_SIGNED: bool = T::IS_SIGNED;
}

// IMPL INTEGER

impl<T: Float> Ord for WrappedFloat<T> {
    #[inline]
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        // Implements total ordering for a float, while keeping typical
        // behavior. All ordering is preserved, except for NaN,
        // such that if both are NaN, they compare equal.
        // PartialOrd are fails to provide an Ordering if
        // either are NaN, so we just need to provide consistent
        // ordering if either is NaN.
        if let Some(ordering) = self.partial_cmp(&other) {
            ordering
        } else if !self.data.is_nan() {
            cmp::Ordering::Less
        } else if other.data.is_nan() {
            cmp::Ordering::Equal
        } else {
            cmp::Ordering::Greater
        }
    }
}

impl<T: Float> Eq for WrappedFloat<T> {
}

impl<T: Float> fmt::Octal for WrappedFloat<T> {
    #[inline]
    fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
        unimplemented!()
    }
}

impl<T: Float> fmt::LowerHex for WrappedFloat<T> {
    #[inline]
    fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
        unimplemented!()
    }
}

impl<T: Float> fmt::UpperHex for WrappedFloat<T> {
    #[inline]
    fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
        unimplemented!()
    }
}

/// Unimplement bitwise operations.
macro_rules! bitwise_impl {
    ($($t:ident, $meth:ident ;)*) => ($(
        impl<T: Float> ops::$t for WrappedFloat<T> {
            type Output = Self;

            #[inline]
            fn $meth(self, _: Self) -> Self::Output {
                unimplemented!()
            }
        }
    )*);
}

bitwise_impl! {
    BitAnd, bitand ;
    BitOr, bitor ;
    BitXor, bitxor ;
}

/// Unimplement bitwise assignment operations.
macro_rules! bitwise_assign_impl {
    ($($t:ident, $meth:ident ;)*) => ($(
        impl<T: Float> ops::$t for WrappedFloat<T> {
            #[inline]
            fn $meth(&mut self, _: Self) {
                unimplemented!()
            }
        }
    )*);
}

bitwise_assign_impl! {
    BitAndAssign, bitand_assign ;
    BitOrAssign, bitor_assign ;
    BitXorAssign, bitxor_assign ;
}

impl<T: Float> ops::Not for WrappedFloat<T> {
    type Output = Self;

    #[inline]
    fn not(self) -> Self::Output {
        unimplemented!()
    }
}

/// Unimplement bitshift operations.
macro_rules! bitshift_impl {
    ($t:tt, $meth:ident ; $($s:ty)*) => ($(
        // Iterate over all primitives.
        impl<T: Float> ops::$t<$s> for WrappedFloat<T> {
            type Output = Self;

            #[inline]
            fn $meth(self, _: $s) -> Self::Output {
                unimplemented!()
            }
        }
    )*);
    ($($t:ident, $meth:ident ;)*) => ($(
        // Base case, same as self.
        impl<T: Float> ops::$t<> for WrappedFloat<T> {
            type Output = Self;

            #[inline]
            fn $meth(self, _: Self) -> Self::Output {
                unimplemented!()
            }
        }

        bitshift_impl!($t, $meth ; u8 u16 u32 u64 usize i8 i16 i32 i64 isize);
    )*);
}

bitshift_impl! {
    Shl, shl ;
    Shr, shr ;
}

/// Unimplement bitshift assignment operations.
macro_rules! bitshift_assign_impl {
    ($t:tt, $meth:ident ; $($s:ty)*) => ($(
        // Iterate over all primitives.
        impl<T: Float> ops::$t<$s> for WrappedFloat<T> {
            #[inline]
            fn $meth(&mut self, _: $s) {
                unimplemented!()
            }
        }
    )*);
    ($($t:ident, $meth:ident ;)*) => ($(
        // Base case, same as self.
        impl<T: Float> ops::$t<> for WrappedFloat<T> {
            #[inline]
            fn $meth(&mut self, _: Self) {
                unimplemented!()
            }
        }

        bitshift_assign_impl!($t, $meth ; u8 u16 u32 u64 usize i8 i16 i32 i64 isize);
    )*);
}

bitshift_assign_impl! {
    ShlAssign, shl_assign ;
    ShrAssign, shr_assign ;
}

impl<T: Float> Integer for WrappedFloat<T> {
    const ZERO: Self = WrappedFloat { data: T::ZERO };
    const ONE: Self = WrappedFloat { data: T::ONE };
    const TWO: Self = WrappedFloat { data: T::TWO };
    const MAX: Self = WrappedFloat { data: T::MAX };
    const MIN: Self = WrappedFloat { data: T::MIN };
    const BITS: usize = T::BITS;

    #[inline]
    fn max_value() -> Self {
        Self::MAX
    }

    #[inline]
    fn min_value() -> Self {
        Self::MIN
    }

    #[inline]
    fn count_ones(self) -> u32 {
        unreachable!()
    }

    #[inline]
    fn count_zeros(self) -> u32 {
        unreachable!()
    }

    #[inline]
    fn leading_zeros(self) -> u32 {
        unreachable!()
    }

    #[inline]
    fn trailing_zeros(self) -> u32 {
        unreachable!()
    }

    #[inline]
    fn pow(self, _: u32) -> Self {
        unreachable!()
    }

    #[inline]
    fn checked_add(self, i: Self) -> Option<Self> {
        Some(self + i)
    }

    #[inline]
    fn checked_sub(self, i: Self) -> Option<Self> {
        Some(self - i)
    }

    #[inline]
    fn checked_mul(self, i: Self) -> Option<Self> {
        Some(self * i)
    }

    #[inline]
    fn checked_div(self, i: Self) -> Option<Self> {
        Some(self / i)
    }

    #[inline]
    fn checked_rem(self, i: Self) -> Option<Self> {
        Some(self % i)
    }

    #[inline]
    fn checked_neg(self) -> Option<Self> {
        Some(-self)
    }

    #[inline]
    fn checked_shl(self, _: u32) -> Option<Self> {
        unimplemented!()
    }

    #[inline]
    fn checked_shr(self, _: u32) -> Option<Self> {
        unimplemented!()
    }

    #[inline]
    fn wrapping_add(self, i: Self) -> Self {
        self + i
    }

    #[inline]
    fn wrapping_sub(self, i: Self) -> Self {
        self - i
    }

    #[inline]
    fn wrapping_mul(self, i: Self) -> Self {
        self * i
    }

    #[inline]
    fn wrapping_div(self, i: Self) -> Self {
        self / i
    }

    #[inline]
    fn wrapping_rem(self, i: Self) -> Self {
        self % i
    }

    #[inline]
    fn wrapping_neg(self) -> Self {
        -self
    }

    #[inline]
    fn wrapping_shl(self, _: u32) -> Self {
        unimplemented!()
    }

    #[inline]
    fn wrapping_shr(self, _: u32) -> Self {
        unimplemented!()
    }

    #[inline]
    fn overflowing_add(self, i: Self) -> (Self, bool) {
        (self + i, false)
    }

    #[inline]
    fn overflowing_sub(self, i: Self) -> (Self, bool) {
        (self - i, false)
    }

    #[inline]
    fn overflowing_mul(self, i: Self) -> (Self, bool) {
        (self * i, false)
    }

    #[inline]
    fn overflowing_div(self, i: Self) -> (Self, bool) {
        (self / i, false)
    }

    #[inline]
    fn overflowing_rem(self, i: Self) -> (Self, bool) {
        (self % i, false)
    }

    #[inline]
    fn overflowing_neg(self) -> (Self, bool) {
        (-self, false)
    }

    #[inline]
    fn overflowing_shl(self, _: u32) -> (Self, bool) {
        unimplemented!()
    }

    #[inline]
    fn overflowing_shr(self, _: u32) -> (Self, bool) {
        unimplemented!()
    }

    #[inline]
    fn saturating_add(self, i: Self) -> Self {
        self + i
    }

    #[inline]
    fn saturating_sub(self, i: Self) -> Self {
        self - i
    }

    #[inline]
    fn saturating_mul(self, i: Self) -> Self {
        self * i
    }
}

// SIGNED INTEGER

impl<T: Float> ops::Neg for WrappedFloat<T> {
    type Output = Self;

    #[inline]
    fn neg(self) -> Self {
        WrappedFloat { data: -self.data }
    }
}

impl<T: Float> SignedInteger for WrappedFloat<T> {
    fn checked_abs(self) -> Option<Self> {
        Some(self.wrapping_abs())
    }

    fn wrapping_abs(self) -> Self {
        WrappedFloat { data: self.data.abs() }
    }

    fn overflowing_abs(self) -> (Self, bool) {
        (self.wrapping_abs(), false)
    }
}

// TEST
// ----

#[cfg(test)]
mod tests {
    use super::*;

    fn check_integer<T: Integer>(mut x: T) {
        // Copy, partialeq, partialord, ord, eq
        let _ = x;
        assert!(x > T::ONE);
        assert!(x != T::ONE);
        assert_eq!(x.min(T::ONE), T::ONE);
        assert_eq!(x.max(T::ONE), x);

        // Operations
        let _ = x + T::ONE;
        let _ = x - T::ONE;
        let _ = x * T::ONE;
        let _ = x / T::ONE;
        let _ = x % T::ONE;
        x += T::ONE;
        x -= T::ONE;
        x *= T::ONE;
        x /= T::ONE;
        x %= T::ONE;

        // Functions
        assert!(T::ZERO.is_zero());
        assert!(!T::ONE.is_zero());
        assert!(T::ONE.is_one());
        assert!(!T::ZERO.is_one());

        // As cast
        let _: u8 = as_cast(x);
        let _: u16 = as_cast(x);
        let _: u32 = as_cast(x);
        let _: u64 = as_cast(x);
        let _: u128 = as_cast(x);
        let _: usize = as_cast(x);
        let _: i8 = as_cast(x);
        let _: i16 = as_cast(x);
        let _: i32 = as_cast(x);
        let _: i64 = as_cast(x);
        let _: i128 = as_cast(x);
        let _: isize = as_cast(x);
        let _: f32 = as_cast(x);
        let _: f64 = as_cast(x);
    }

    fn check_try_cast_compile<T: Integer>(x: T) {
        // Try cast
        let _: Option<u8> = try_cast(x);
        let _: Option<u16> = try_cast(x);
        let _: Option<u32> = try_cast(x);
        let _: Option<u64> = try_cast(x);
        let _: Option<u128> = try_cast(x);
        let _: Option<usize> = try_cast(x);
        let _: Option<i8> = try_cast(x);
        let _: Option<i16> = try_cast(x);
        let _: Option<i32> = try_cast(x);
        let _: Option<i64> = try_cast(x);
        let _: Option<i128> = try_cast(x);
        let _: Option<isize> = try_cast(x);
        let _: Option<f32> = try_cast(x);
        let _: Option<f64> = try_cast(x);
    }

    #[allow(dead_code)]     // Compile-only
    fn try_cast_test() {
        check_try_cast_compile(WrappedFloat::from_float(65f32));
        check_try_cast_compile(WrappedFloat::from_float(65f64));
    }

    #[test]
    fn integer_test() {
        check_integer(WrappedFloat::from_float(65f32));
        check_integer(WrappedFloat::from_float(65f64));
    }
}

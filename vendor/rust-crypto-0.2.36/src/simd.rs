// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

pub use self::fake::*;

pub trait SimdExt {
    fn simd_eq(self, rhs: Self) -> Self;
}

impl SimdExt for fake::u32x4 {
    fn simd_eq(self, rhs: Self) -> Self {
        if self == rhs {
            fake::u32x4(0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff)
        } else {
            fake::u32x4(0, 0, 0, 0)
        }
    }
}

mod fake {
    use std::ops::{Add, BitAnd, BitOr, BitXor, Shl, Shr, Sub};

    #[derive(Clone, Copy, PartialEq, Eq)]
    #[allow(non_camel_case_types)]
    pub struct u32x4(pub u32, pub u32, pub u32, pub u32);

    impl Add for u32x4 {
        type Output = u32x4;

        fn add(self, rhs: u32x4) -> u32x4 {
            u32x4(
                self.0.wrapping_add(rhs.0),
                self.1.wrapping_add(rhs.1),
                self.2.wrapping_add(rhs.2),
                self.3.wrapping_add(rhs.3))
        }
    }

    impl Sub for u32x4 {
        type Output = u32x4;

        fn sub(self, rhs: u32x4) -> u32x4 {
            u32x4(
                self.0.wrapping_sub(rhs.0),
                self.1.wrapping_sub(rhs.1),
                self.2.wrapping_sub(rhs.2),
                self.3.wrapping_sub(rhs.3))
        }
    }

    impl BitAnd for u32x4 {
        type Output = u32x4;

        fn bitand(self, rhs: u32x4) -> u32x4 {
            u32x4(self.0 & rhs.0, self.1 & rhs.1, self.2 & rhs.2, self.3 & rhs.3)
        }
    }

    impl BitOr for u32x4 {
        type Output = u32x4;

        fn bitor(self, rhs: u32x4) -> u32x4 {
            u32x4(self.0 | rhs.0, self.1 | rhs.1, self.2 | rhs.2, self.3 | rhs.3)
        }
    }

    impl BitXor for u32x4 {
        type Output = u32x4;

        fn bitxor(self, rhs: u32x4) -> u32x4 {
            u32x4(self.0 ^ rhs.0, self.1 ^ rhs.1, self.2 ^ rhs.2, self.3 ^ rhs.3)
        }
    }

    impl Shl<usize> for u32x4 {
        type Output = u32x4;

        fn shl(self, amt: usize) -> u32x4 {
            u32x4(self.0 << amt, self.1 << amt, self.2 << amt, self.3 << amt)
        }
    }

    impl Shl<u32x4> for u32x4 {
        type Output = u32x4;

        fn shl(self, rhs: u32x4) -> u32x4 {
            u32x4(self.0 << rhs.0, self.1 << rhs.1, self.2 << rhs.2, self.3 << rhs.3)
        }
    }

    impl Shr<usize> for u32x4 {
        type Output = u32x4;

        fn shr(self, amt: usize) -> u32x4 {
            u32x4(self.0 >> amt, self.1 >> amt, self.2 >> amt, self.3 >> amt)
        }
    }

    impl Shr<u32x4> for u32x4 {
        type Output = u32x4;

        fn shr(self, rhs: u32x4) -> u32x4 {
            u32x4(self.0 >> rhs.0, self.1 >> rhs.1, self.2 >> rhs.2, self.3 >> rhs.3)
        }
    }

    #[derive(Clone, Copy)]
    #[allow(non_camel_case_types)]
    pub struct u64x2(pub u64, pub u64);

    impl Add for u64x2 {
        type Output = u64x2;

        fn add(self, rhs: u64x2) -> u64x2 {
            u64x2(self.0.wrapping_add(rhs.0), self.1.wrapping_add(rhs.1))
        }
    }
}

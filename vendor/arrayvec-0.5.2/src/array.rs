
/// Trait for fixed size arrays.
///
/// This trait is implemented for some specific array sizes, see
/// the implementor list below. At the current state of Rust we can't
/// make this fully general for every array size.
///
/// The following crate features add more array sizes (and they are not
/// enabled by default due to their impact on compliation speed).
///
/// - `array-sizes-33-128`: All sizes 33 to 128 are implemented
///   (a few in this range are included by default).
/// - `array-sizes-129-255`: All sizes 129 to 255 are implemented
///   (a few in this range are included by default).
///
/// ## Safety
///
/// This trait can *only* be implemented by fixed-size arrays or types with
/// *exactly* the representation of a fixed size array (of the right element
/// type and capacity).
///
/// Normally this trait is an implementation detail of arrayvec and doesn’t
/// need implementing.
pub unsafe trait Array {
    /// The array’s element type
    type Item;
    /// The smallest type that can index and tell the length of the array.
    #[doc(hidden)]
    type Index: Index;
    /// The array's element capacity
    const CAPACITY: usize;
    fn as_slice(&self) -> &[Self::Item];
    fn as_mut_slice(&mut self) -> &mut [Self::Item];
}

pub trait Index : PartialEq + Copy {
    const ZERO: Self;
    fn to_usize(self) -> usize;
    fn from(_: usize) -> Self;
}

impl Index for () {
    const ZERO: Self = ();
    #[inline(always)]
    fn to_usize(self) -> usize { 0 }
    #[inline(always)]
    fn from(_ix: usize) ->  Self { () }
}

impl Index for bool {
    const ZERO: Self = false;
    #[inline(always)]
    fn to_usize(self) -> usize { self as usize }
    #[inline(always)]
    fn from(ix: usize) ->  Self { ix != 0 }
}

impl Index for u8 {
    const ZERO: Self = 0;
    #[inline(always)]
    fn to_usize(self) -> usize { self as usize }
    #[inline(always)]
    fn from(ix: usize) ->  Self { ix as u8 }
}

impl Index for u16 {
    const ZERO: Self = 0;
    #[inline(always)]
    fn to_usize(self) -> usize { self as usize }
    #[inline(always)]
    fn from(ix: usize) ->  Self { ix as u16 }
}

impl Index for u32 {
    const ZERO: Self = 0;
    #[inline(always)]
    fn to_usize(self) -> usize { self as usize }
    #[inline(always)]
    fn from(ix: usize) ->  Self { ix as u32 }
}

impl Index for usize {
    const ZERO: Self = 0;
    #[inline(always)]
    fn to_usize(self) -> usize { self }
    #[inline(always)]
    fn from(ix: usize) ->  Self { ix }
}

macro_rules! fix_array_impl {
    ($index_type:ty, $len:expr ) => (
        unsafe impl<T> Array for [T; $len] {
            type Item = T;
            type Index = $index_type;
            const CAPACITY: usize = $len;
            #[doc(hidden)]
            fn as_slice(&self) -> &[Self::Item] { self }
            #[doc(hidden)]
            fn as_mut_slice(&mut self) -> &mut [Self::Item] { self }
        }
    )
}

macro_rules! fix_array_impl_recursive {
    ($index_type:ty, ) => ();
    ($index_type:ty, $($len:expr,)*) => (
        $(fix_array_impl!($index_type, $len);)*
    );
}


fix_array_impl_recursive!((), 0,);
fix_array_impl_recursive!(bool, 1,);
fix_array_impl_recursive!(u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14,
                          15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
                          28, 29, 30, 31, );

#[cfg(not(feature="array-sizes-33-128"))]
fix_array_impl_recursive!(u8, 32, 40, 48, 50, 56, 64, 72, 96, 100, 128, );

#[cfg(feature="array-sizes-33-128")]
fix_array_impl_recursive!(u8, 
32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51,
52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71,
72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91,
92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108,
109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124,
125, 126, 127, 128,
);

#[cfg(not(feature="array-sizes-129-255"))]
fix_array_impl_recursive!(u8, 160, 192, 200, 224,);

#[cfg(feature="array-sizes-129-255")]
fix_array_impl_recursive!(u8,
129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140,
141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 156,
157, 158, 159, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172,
173, 174, 175, 176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188,
189, 190, 191, 192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 203, 204,
205, 206, 207, 208, 209, 210, 211, 212, 213, 214, 215, 216, 217, 218, 219, 220,
221, 222, 223, 224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 235, 236,
237, 238, 239, 240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252,
253, 254, 255,
);

fix_array_impl_recursive!(u16, 256, 384, 512, 768, 1024, 2048, 4096, 8192, 16384, 32768,);
// This array size doesn't exist on 16-bit
#[cfg(any(target_pointer_width="32", target_pointer_width="64"))]
fix_array_impl_recursive!(u32, 1 << 16,);


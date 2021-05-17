#[cfg(feature = "specialize")]
use crate::HasherExt;
use core::hash::Hash;
use core::hash::Hasher;

/// Provides a way to get an optimized hasher for a given data type.
/// Rather than using a Hasher generically which can hash any value, this provides a way to get a specialized hash
/// for a specific type. So this may be faster for primitive types. It does however consume the hasher in the process.
/// #Example
/// ```
/// use std::hash::BuildHasher;
/// use ahash::RandomState;
/// use ahash::CallHasher;
///
/// let hash_builder = RandomState::new();
/// //...
/// let value = 17;
/// let hash = value.get_hash(hash_builder.build_hasher());
/// ```
pub trait CallHasher: Hash {
    fn get_hash<H: Hasher>(&self, hasher: H) -> u64;
}

#[cfg(not(feature = "specialize"))]
impl<T> CallHasher for T
where
    T: Hash,
{
    #[inline]
    fn get_hash<H: Hasher>(&self, mut hasher: H) -> u64 {
        self.hash(&mut hasher);
        hasher.finish()
    }
}

#[cfg(feature = "specialize")]
impl<T> CallHasher for T
where
    T: Hash,
{
    #[inline]
    default fn get_hash<H: Hasher>(&self, mut hasher: H) -> u64 {
        self.hash(&mut hasher);
        hasher.finish()
    }
}

macro_rules! call_hasher_impl {
    ($typ:ty) => {
        #[cfg(feature = "specialize")]
        impl CallHasher for $typ {
            #[inline]
            fn get_hash<H: Hasher>(&self, hasher: H) -> u64 {
                hasher.hash_u64(*self as u64)
            }
        }
    };
}
call_hasher_impl!(u8);
call_hasher_impl!(u16);
call_hasher_impl!(u32);
call_hasher_impl!(u64);
call_hasher_impl!(i8);
call_hasher_impl!(i16);
call_hasher_impl!(i32);
call_hasher_impl!(i64);

#[cfg(feature = "specialize")]
impl CallHasher for u128 {
    #[inline]
    fn get_hash<H: Hasher>(&self, mut hasher: H) -> u64 {
        hasher.write_u128(*self);
        hasher.short_finish()
    }
}

#[cfg(feature = "specialize")]
impl CallHasher for i128 {
    #[inline]
    fn get_hash<H: Hasher>(&self, mut hasher: H) -> u64 {
        hasher.write_u128(*self as u128);
        hasher.short_finish()
    }
}

#[cfg(feature = "specialize")]
impl CallHasher for [u8] {
    #[inline]
    fn get_hash<H: Hasher>(&self, mut hasher: H) -> u64 {
        hasher.write(self);
        hasher.finish()
    }
}

#[cfg(all(feature = "specialize", feature = "std"))]
impl CallHasher for Vec<u8> {
    #[inline]
    fn get_hash<H: Hasher>(&self, mut hasher: H) -> u64 {
        hasher.write(self);
        hasher.finish()
    }
}

#[cfg(feature = "specialize")]
impl CallHasher for str {
    #[inline]
    fn get_hash<H: Hasher>(&self, mut hasher: H) -> u64 {
        hasher.write(self.as_bytes());
        hasher.finish()
    }
}

#[cfg(all(feature = "specialize", feature = "std"))]
impl CallHasher for String {
    #[inline]
    fn get_hash<H: Hasher>(&self, mut hasher: H) -> u64 {
        hasher.write(self.as_bytes());
        hasher.finish()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::*;

    #[test]
    #[cfg(feature = "specialize")]
    pub fn test_specialized_invoked() {
        let shortened = 0_u64.get_hash(AHasher::new_with_keys(1, 2));
        let mut hasher = AHasher::new_with_keys(1, 2);
        0_u64.hash(&mut hasher);
        assert_ne!(hasher.finish(), shortened);
    }

    /// Tests that some non-trivial transformation takes place.
    #[test]
    pub fn test_input_processed() {
        let hasher = || AHasher::new_with_keys(3, 2);
        assert_ne!(0, 0_u64.get_hash(hasher()));
        assert_ne!(1, 0_u64.get_hash(hasher()));
        assert_ne!(2, 0_u64.get_hash(hasher()));
        assert_ne!(3, 0_u64.get_hash(hasher()));
        assert_ne!(4, 0_u64.get_hash(hasher()));
        assert_ne!(5, 0_u64.get_hash(hasher()));

        assert_ne!(0, 1_u64.get_hash(hasher()));
        assert_ne!(1, 1_u64.get_hash(hasher()));
        assert_ne!(2, 1_u64.get_hash(hasher()));
        assert_ne!(3, 1_u64.get_hash(hasher()));
        assert_ne!(4, 1_u64.get_hash(hasher()));
        assert_ne!(5, 1_u64.get_hash(hasher()));

        let xored = 0_u64.get_hash(hasher()) ^ 1_u64.get_hash(hasher());
        assert_ne!(0, xored);
        assert_ne!(1, xored);
        assert_ne!(2, xored);
        assert_ne!(3, xored);
        assert_ne!(4, xored);
        assert_ne!(5, xored);
    }
}

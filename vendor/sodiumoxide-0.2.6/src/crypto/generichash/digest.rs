use ffi::crypto_generichash_BYTES_MAX;
use std::cmp::{Eq, Ordering, PartialEq, PartialOrd};
use std::fmt::{Debug, Formatter};
use std::hash::{Hash, Hasher};
use std::ops::{Index, Range, RangeFrom, RangeFull, RangeTo};

/// Digest-structure
///
/// This structure contains a fixed sized array as a buffer and a length to
/// represent dynamic sized digest outputs.
#[derive(Clone)]
pub struct Digest {
    pub(super) len: usize,
    pub(super) data: [u8; crypto_generichash_BYTES_MAX as usize],
}

impl Debug for Digest {
    fn fmt(&self, formatter: &mut Formatter) -> ::std::fmt::Result {
        write!(formatter, "Digest({:?})", &self[..])
    }
}

impl PartialEq for Digest {
    fn eq(&self, other: &Digest) -> bool {
        use utils::memcmp;
        if other.len != self.len {
            return false;
        }
        memcmp(self.as_ref(), other.as_ref())
    }
}

impl Eq for Digest {}

impl AsRef<[u8]> for Digest {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.data[0..self.len]
    }
}

impl PartialOrd for Digest {
    #[inline]
    fn partial_cmp(&self, other: &Digest) -> Option<Ordering> {
        PartialOrd::partial_cmp(self.as_ref(), other.as_ref())
    }

    #[inline]
    fn lt(&self, other: &Digest) -> bool {
        PartialOrd::lt(self.as_ref(), other.as_ref())
    }

    #[inline]
    fn le(&self, other: &Digest) -> bool {
        PartialOrd::le(self.as_ref(), other.as_ref())
    }

    #[inline]
    fn ge(&self, other: &Digest) -> bool {
        PartialOrd::ge(self.as_ref(), other.as_ref())
    }

    #[inline]
    fn gt(&self, other: &Digest) -> bool {
        PartialOrd::gt(self.as_ref(), other.as_ref())
    }
}

impl Ord for Digest {
    #[inline]
    fn cmp(&self, other: &Digest) -> Ordering {
        Ord::cmp(self.as_ref(), other.as_ref())
    }
}

impl Hash for Digest {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(self.as_ref(), state)
    }
}

/// Allows a user to access the byte contents of an object as a slice.
///
/// WARNING: it might be tempting to do comparisons on objects
/// by using `x[a..b] == y[a..b]`. This will open up for timing attacks
/// when comparing for example authenticator tags. Because of this only
/// use the comparison functions exposed by the sodiumoxide API.
impl Index<Range<usize>> for Digest {
    type Output = [u8];
    fn index(&self, index: Range<usize>) -> &[u8] {
        self.as_ref().index(index)
    }
}

/// Allows a user to access the byte contents of an object as a slice.
///
/// WARNING: it might be tempting to do comparisons on objects
/// by using `x[..b] == y[..b]`. This will open up for timing attacks
/// when comparing for example authenticator tags. Because of this only
/// use the comparison functions exposed by the sodiumoxide API.
impl Index<RangeTo<usize>> for Digest {
    type Output = [u8];
    fn index(&self, index: RangeTo<usize>) -> &[u8] {
        self.as_ref().index(index)
    }
}

/// Allows a user to access the byte contents of an object as a slice.
///
/// WARNING: it might be tempting to do comparisons on objects
/// by using `x[a..] == y[a..]`. This will open up for timing attacks
/// when comparing for example authenticator tags. Because of this only
/// use the comparison functions exposed by the sodiumoxide API.
impl Index<RangeFrom<usize>> for Digest {
    type Output = [u8];
    fn index(&self, index: RangeFrom<usize>) -> &[u8] {
        self.as_ref().index(index)
    }
}

/// Allows a user to access the byte contents of an object as a slice.
///
/// WARNING: it might be tempting to do comparisons on objects
/// by using `x[] == y[]`. This will open up for timing attacks
/// when comparing for example authenticator tags. Because of this only
/// use the comparison functions exposed by the sodiumoxide API.
impl Index<RangeFull> for Digest {
    type Output = [u8];
    fn index(&self, index: RangeFull) -> &[u8] {
        self.as_ref().index(index)
    }
}

use core::fmt;

/// A representation of byte oriented equivalence classes.
///
/// This is used in a DFA to reduce the size of the transition table. This can
/// have a particularly large impact not only on the total size of a dense DFA,
/// but also on compile times.
#[derive(Clone, Copy)]
pub struct ByteClasses([u8; 256]);

impl ByteClasses {
    /// Creates a new set of equivalence classes where all bytes are mapped to
    /// the same class.
    pub fn empty() -> ByteClasses {
        ByteClasses([0; 256])
    }

    /// Creates a new set of equivalence classes where each byte belongs to
    /// its own equivalence class.
    pub fn singletons() -> ByteClasses {
        let mut classes = ByteClasses::empty();
        for i in 0..256 {
            classes.set(i as u8, i as u8);
        }
        classes
    }

    /// Copies the byte classes given. The given slice must have length 0 or
    /// length 256. Slices of length 0 are treated as singletons (every byte
    /// is its own class).
    pub fn from_slice(slice: &[u8]) -> ByteClasses {
        assert!(slice.is_empty() || slice.len() == 256);

        if slice.is_empty() {
            ByteClasses::singletons()
        } else {
            let mut classes = ByteClasses::empty();
            for (b, &class) in slice.iter().enumerate() {
                classes.set(b as u8, class);
            }
            classes
        }
    }

    /// Set the equivalence class for the given byte.
    #[inline]
    pub fn set(&mut self, byte: u8, class: u8) {
        self.0[byte as usize] = class;
    }

    /// Get the equivalence class for the given byte.
    #[inline]
    pub fn get(&self, byte: u8) -> u8 {
        self.0[byte as usize]
    }

    /// Get the equivalence class for the given byte while forcefully
    /// eliding bounds checks.
    #[inline]
    pub unsafe fn get_unchecked(&self, byte: u8) -> u8 {
        *self.0.get_unchecked(byte as usize)
    }

    /// Return the total number of elements in the alphabet represented by
    /// these equivalence classes. Equivalently, this returns the total number
    /// of equivalence classes.
    #[inline]
    pub fn alphabet_len(&self) -> usize {
        self.0[255] as usize + 1
    }

    /// Returns true if and only if every byte in this class maps to its own
    /// equivalence class. Equivalently, there are 256 equivalence classes
    /// and each class contains exactly one byte.
    #[inline]
    pub fn is_singleton(&self) -> bool {
        self.alphabet_len() == 256
    }

    /// Returns an iterator over a sequence of representative bytes from each
    /// equivalence class. Namely, this yields exactly N items, where N is
    /// equivalent to the number of equivalence classes. Each item is an
    /// arbitrary byte drawn from each equivalence class.
    ///
    /// This is useful when one is determinizing an NFA and the NFA's alphabet
    /// hasn't been converted to equivalence classes yet. Picking an arbitrary
    /// byte from each equivalence class then permits a full exploration of
    /// the NFA instead of using every possible byte value.
    #[cfg(feature = "std")]
    pub fn representatives(&self) -> ByteClassRepresentatives {
        ByteClassRepresentatives { classes: self, byte: 0, last_class: None }
    }

    /// Returns all of the bytes in the given equivalence class.
    ///
    /// The second element in the tuple indicates the number of elements in
    /// the array.
    fn elements(&self, equiv: u8) -> ([u8; 256], usize) {
        let (mut array, mut len) = ([0; 256], 0);
        for b in 0..256 {
            if self.get(b as u8) == equiv {
                array[len] = b as u8;
                len += 1;
            }
        }
        (array, len)
    }
}

impl fmt::Debug for ByteClasses {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.is_singleton() {
            write!(f, "ByteClasses({{singletons}})")
        } else {
            write!(f, "ByteClasses(")?;
            for equiv in 0..self.alphabet_len() {
                let (members, len) = self.elements(equiv as u8);
                write!(f, "{} => {:?}", equiv, &members[..len])?;
            }
            write!(f, ")")
        }
    }
}

/// An iterator over representative bytes from each equivalence class.
#[cfg(feature = "std")]
#[derive(Debug)]
pub struct ByteClassRepresentatives<'a> {
    classes: &'a ByteClasses,
    byte: usize,
    last_class: Option<u8>,
}

#[cfg(feature = "std")]
impl<'a> Iterator for ByteClassRepresentatives<'a> {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        while self.byte < 256 {
            let byte = self.byte as u8;
            let class = self.classes.get(byte);
            self.byte += 1;

            if self.last_class != Some(class) {
                self.last_class = Some(class);
                return Some(byte);
            }
        }
        None
    }
}

/// A byte class set keeps track of an *approximation* of equivalence classes
/// of bytes during NFA construction. That is, every byte in an equivalence
/// class cannot discriminate between a match and a non-match.
///
/// For example, in the regex `[ab]+`, the bytes `a` and `b` would be in the
/// same equivalence class because it never matters whether an `a` or a `b` is
/// seen, and no combination of `a`s and `b`s in the text can discriminate
/// a match.
///
/// Note though that this does not compute the minimal set of equivalence
/// classes. For example, in the regex `[ac]+`, both `a` and `c` are in the
/// same equivalence class for the same reason that `a` and `b` are in the
/// same equivalence class in the aforementioned regex. However, in this
/// implementation, `a` and `c` are put into distinct equivalence classes.
/// The reason for this is implementation complexity. In the future, we should
/// endeavor to compute the minimal equivalence classes since they can have a
/// rather large impact on the size of the DFA.
///
/// The representation here is 256 booleans, all initially set to false. Each
/// boolean maps to its corresponding byte based on position. A `true` value
/// indicates the end of an equivalence class, where its corresponding byte
/// and all of the bytes corresponding to all previous contiguous `false`
/// values are in the same equivalence class.
///
/// This particular representation only permits contiguous ranges of bytes to
/// be in the same equivalence class, which means that we can never discover
/// the true minimal set of equivalence classes.
#[cfg(feature = "std")]
#[derive(Debug)]
pub struct ByteClassSet(Vec<bool>);

#[cfg(feature = "std")]
impl ByteClassSet {
    /// Create a new set of byte classes where all bytes are part of the same
    /// equivalence class.
    pub fn new() -> Self {
        ByteClassSet(vec![false; 256])
    }

    /// Indicate the the range of byte given (inclusive) can discriminate a
    /// match between it and all other bytes outside of the range.
    pub fn set_range(&mut self, start: u8, end: u8) {
        debug_assert!(start <= end);
        if start > 0 {
            self.0[start as usize - 1] = true;
        }
        self.0[end as usize] = true;
    }

    /// Convert this boolean set to a map that maps all byte values to their
    /// corresponding equivalence class. The last mapping indicates the largest
    /// equivalence class identifier (which is never bigger than 255).
    pub fn byte_classes(&self) -> ByteClasses {
        let mut classes = ByteClasses::empty();
        let mut class = 0u8;
        let mut i = 0;
        loop {
            classes.set(i as u8, class as u8);
            if i >= 255 {
                break;
            }
            if self.0[i] {
                class = class.checked_add(1).unwrap();
            }
            i += 1;
        }
        classes
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "std")]
    #[test]
    fn byte_classes() {
        use super::ByteClassSet;

        let mut set = ByteClassSet::new();
        set.set_range(b'a', b'z');

        let classes = set.byte_classes();
        assert_eq!(classes.get(0), 0);
        assert_eq!(classes.get(1), 0);
        assert_eq!(classes.get(2), 0);
        assert_eq!(classes.get(b'a' - 1), 0);
        assert_eq!(classes.get(b'a'), 1);
        assert_eq!(classes.get(b'm'), 1);
        assert_eq!(classes.get(b'z'), 1);
        assert_eq!(classes.get(b'z' + 1), 2);
        assert_eq!(classes.get(254), 2);
        assert_eq!(classes.get(255), 2);

        let mut set = ByteClassSet::new();
        set.set_range(0, 2);
        set.set_range(4, 6);
        let classes = set.byte_classes();
        assert_eq!(classes.get(0), 0);
        assert_eq!(classes.get(1), 0);
        assert_eq!(classes.get(2), 0);
        assert_eq!(classes.get(3), 1);
        assert_eq!(classes.get(4), 2);
        assert_eq!(classes.get(5), 2);
        assert_eq!(classes.get(6), 2);
        assert_eq!(classes.get(7), 3);
        assert_eq!(classes.get(255), 3);
    }

    #[cfg(feature = "std")]
    #[test]
    fn full_byte_classes() {
        use super::ByteClassSet;

        let mut set = ByteClassSet::new();
        for i in 0..256u16 {
            set.set_range(i as u8, i as u8);
        }
        assert_eq!(set.byte_classes().alphabet_len(), 256);
    }
}

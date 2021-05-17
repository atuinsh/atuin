#[cfg(feature = "std")]
use core::fmt;
#[cfg(feature = "std")]
use core::iter;
use core::marker::PhantomData;
use core::mem::size_of;
#[cfg(feature = "std")]
use std::collections::HashMap;

#[cfg(feature = "std")]
use byteorder::{BigEndian, LittleEndian};
use byteorder::{ByteOrder, NativeEndian};

use classes::ByteClasses;
use dense;
use dfa::DFA;
#[cfg(feature = "std")]
use error::{Error, Result};
#[cfg(feature = "std")]
use state_id::{dead_id, usize_to_state_id, write_state_id_bytes, StateID};
#[cfg(not(feature = "std"))]
use state_id::{dead_id, StateID};

/// A sparse table-based deterministic finite automaton (DFA).
///
/// In contrast to a [dense DFA](enum.DenseDFA.html), a sparse DFA uses a
/// more space efficient representation for its transition table. Consequently,
/// sparse DFAs can use much less memory than dense DFAs, but this comes at a
/// price. In particular, reading the more space efficient transitions takes
/// more work, and consequently, searching using a sparse DFA is typically
/// slower than a dense DFA.
///
/// A sparse DFA can be built using the default configuration via the
/// [`SparseDFA::new`](enum.SparseDFA.html#method.new) constructor. Otherwise,
/// one can configure various aspects of a dense DFA via
/// [`dense::Builder`](dense/struct.Builder.html), and then convert a dense
/// DFA to a sparse DFA using
/// [`DenseDFA::to_sparse`](enum.DenseDFA.html#method.to_sparse).
///
/// In general, a sparse DFA supports all the same operations as a dense DFA.
///
/// Making the choice between a dense and sparse DFA depends on your specific
/// work load. If you can sacrifice a bit of search time performance, then a
/// sparse DFA might be the best choice. In particular, while sparse DFAs are
/// probably always slower than dense DFAs, you may find that they are easily
/// fast enough for your purposes!
///
/// # State size
///
/// A `SparseDFA` has two type parameters, `T` and `S`. `T` corresponds to
/// the type of the DFA's transition table while `S` corresponds to the
/// representation used for the DFA's state identifiers as described by the
/// [`StateID`](trait.StateID.html) trait. This type parameter is typically
/// `usize`, but other valid choices provided by this crate include `u8`,
/// `u16`, `u32` and `u64`. The primary reason for choosing a different state
/// identifier representation than the default is to reduce the amount of
/// memory used by a DFA. Note though, that if the chosen representation cannot
/// accommodate the size of your DFA, then building the DFA will fail and
/// return an error.
///
/// While the reduction in heap memory used by a DFA is one reason for choosing
/// a smaller state identifier representation, another possible reason is for
/// decreasing the serialization size of a DFA, as returned by
/// [`to_bytes_little_endian`](enum.SparseDFA.html#method.to_bytes_little_endian),
/// [`to_bytes_big_endian`](enum.SparseDFA.html#method.to_bytes_big_endian)
/// or
/// [`to_bytes_native_endian`](enum.DenseDFA.html#method.to_bytes_native_endian).
///
/// The type of the transition table is typically either `Vec<u8>` or `&[u8]`,
/// depending on where the transition table is stored. Note that this is
/// different than a dense DFA, whose transition table is typically
/// `Vec<S>` or `&[S]`. The reason for this is that a sparse DFA always reads
/// its transition table from raw bytes because the table is compactly packed.
///
/// # Variants
///
/// This DFA is defined as a non-exhaustive enumeration of different types of
/// dense DFAs. All of the variants use the same internal representation
/// for the transition table, but they vary in how the transition table is
/// read. A DFA's specific variant depends on the configuration options set via
/// [`dense::Builder`](dense/struct.Builder.html). The default variant is
/// `ByteClass`.
///
/// # The `DFA` trait
///
/// This type implements the [`DFA`](trait.DFA.html) trait, which means it
/// can be used for searching. For example:
///
/// ```
/// use regex_automata::{DFA, SparseDFA};
///
/// # fn example() -> Result<(), regex_automata::Error> {
/// let dfa = SparseDFA::new("foo[0-9]+")?;
/// assert_eq!(Some(8), dfa.find(b"foo12345"));
/// # Ok(()) }; example().unwrap()
/// ```
///
/// The `DFA` trait also provides an assortment of other lower level methods
/// for DFAs, such as `start_state` and `next_state`. While these are correctly
/// implemented, it is an anti-pattern to use them in performance sensitive
/// code on the `SparseDFA` type directly. Namely, each implementation requires
/// a branch to determine which type of sparse DFA is being used. Instead,
/// this branch should be pushed up a layer in the code since walking the
/// transitions of a DFA is usually a hot path. If you do need to use these
/// lower level methods in performance critical code, then you should match on
/// the variants of this DFA and use each variant's implementation of the `DFA`
/// trait directly.
#[derive(Clone, Debug)]
pub enum SparseDFA<T: AsRef<[u8]>, S: StateID = usize> {
    /// A standard DFA that does not use byte classes.
    Standard(Standard<T, S>),
    /// A DFA that shrinks its alphabet to a set of equivalence classes instead
    /// of using all possible byte values. Any two bytes belong to the same
    /// equivalence class if and only if they can be used interchangeably
    /// anywhere in the DFA while never discriminating between a match and a
    /// non-match.
    ///
    /// Unlike dense DFAs, sparse DFAs do not tend to benefit nearly as much
    /// from using byte classes. In some cases, using byte classes can even
    /// marginally increase the size of a sparse DFA's transition table. The
    /// reason for this is that a sparse DFA already compacts each state's
    /// transitions separate from whether byte classes are used.
    ByteClass(ByteClass<T, S>),
    /// Hints that destructuring should not be exhaustive.
    ///
    /// This enum may grow additional variants, so this makes sure clients
    /// don't count on exhaustive matching. (Otherwise, adding a new variant
    /// could break existing code.)
    #[doc(hidden)]
    __Nonexhaustive,
}

#[cfg(feature = "std")]
impl SparseDFA<Vec<u8>, usize> {
    /// Parse the given regular expression using a default configuration and
    /// return the corresponding sparse DFA.
    ///
    /// The default configuration uses `usize` for state IDs and reduces the
    /// alphabet size by splitting bytes into equivalence classes. The
    /// resulting DFA is *not* minimized.
    ///
    /// If you want a non-default configuration, then use the
    /// [`dense::Builder`](dense/struct.Builder.html)
    /// to set your own configuration, and then call
    /// [`DenseDFA::to_sparse`](enum.DenseDFA.html#method.to_sparse)
    /// to create a sparse DFA.
    ///
    /// # Example
    ///
    /// ```
    /// use regex_automata::{DFA, SparseDFA};
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let dfa = SparseDFA::new("foo[0-9]+bar")?;
    /// assert_eq!(Some(11), dfa.find(b"foo12345bar"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    pub fn new(pattern: &str) -> Result<SparseDFA<Vec<u8>, usize>> {
        dense::Builder::new()
            .build(pattern)
            .and_then(|dense| dense.to_sparse())
    }
}

#[cfg(feature = "std")]
impl<S: StateID> SparseDFA<Vec<u8>, S> {
    /// Create a new empty sparse DFA that never matches any input.
    ///
    /// # Example
    ///
    /// In order to build an empty DFA, callers must provide a type hint
    /// indicating their choice of state identifier representation.
    ///
    /// ```
    /// use regex_automata::{DFA, SparseDFA};
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let dfa: SparseDFA<Vec<u8>, usize> = SparseDFA::empty();
    /// assert_eq!(None, dfa.find(b""));
    /// assert_eq!(None, dfa.find(b"foo"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    pub fn empty() -> SparseDFA<Vec<u8>, S> {
        dense::DenseDFA::empty().to_sparse().unwrap()
    }

    pub(crate) fn from_dense_sized<T: AsRef<[S]>, A: StateID>(
        dfa: &dense::Repr<T, S>,
    ) -> Result<SparseDFA<Vec<u8>, A>> {
        Repr::from_dense_sized(dfa).map(|r| r.into_sparse_dfa())
    }
}

impl<T: AsRef<[u8]>, S: StateID> SparseDFA<T, S> {
    /// Cheaply return a borrowed version of this sparse DFA. Specifically, the
    /// DFA returned always uses `&[u8]` for its transition table while keeping
    /// the same state identifier representation.
    pub fn as_ref<'a>(&'a self) -> SparseDFA<&'a [u8], S> {
        match *self {
            SparseDFA::Standard(Standard(ref r)) => {
                SparseDFA::Standard(Standard(r.as_ref()))
            }
            SparseDFA::ByteClass(ByteClass(ref r)) => {
                SparseDFA::ByteClass(ByteClass(r.as_ref()))
            }
            SparseDFA::__Nonexhaustive => unreachable!(),
        }
    }

    /// Return an owned version of this sparse DFA. Specifically, the DFA
    /// returned always uses `Vec<u8>` for its transition table while keeping
    /// the same state identifier representation.
    ///
    /// Effectively, this returns a sparse DFA whose transition table lives
    /// on the heap.
    #[cfg(feature = "std")]
    pub fn to_owned(&self) -> SparseDFA<Vec<u8>, S> {
        match *self {
            SparseDFA::Standard(Standard(ref r)) => {
                SparseDFA::Standard(Standard(r.to_owned()))
            }
            SparseDFA::ByteClass(ByteClass(ref r)) => {
                SparseDFA::ByteClass(ByteClass(r.to_owned()))
            }
            SparseDFA::__Nonexhaustive => unreachable!(),
        }
    }

    /// Returns the memory usage, in bytes, of this DFA.
    ///
    /// The memory usage is computed based on the number of bytes used to
    /// represent this DFA's transition table. This typically corresponds to
    /// heap memory usage.
    ///
    /// This does **not** include the stack size used up by this DFA. To
    /// compute that, used `std::mem::size_of::<SparseDFA>()`.
    pub fn memory_usage(&self) -> usize {
        self.repr().memory_usage()
    }

    fn repr(&self) -> &Repr<T, S> {
        match *self {
            SparseDFA::Standard(ref r) => &r.0,
            SparseDFA::ByteClass(ref r) => &r.0,
            SparseDFA::__Nonexhaustive => unreachable!(),
        }
    }
}

/// Routines for converting a sparse DFA to other representations, such as
/// smaller state identifiers or raw bytes suitable for persistent storage.
#[cfg(feature = "std")]
impl<T: AsRef<[u8]>, S: StateID> SparseDFA<T, S> {
    /// Create a new sparse DFA whose match semantics are equivalent to
    /// this DFA, but attempt to use `u8` for the representation of state
    /// identifiers. If `u8` is insufficient to represent all state identifiers
    /// in this DFA, then this returns an error.
    ///
    /// This is a convenience routine for `to_sized::<u8>()`.
    pub fn to_u8(&self) -> Result<SparseDFA<Vec<u8>, u8>> {
        self.to_sized()
    }

    /// Create a new sparse DFA whose match semantics are equivalent to
    /// this DFA, but attempt to use `u16` for the representation of state
    /// identifiers. If `u16` is insufficient to represent all state
    /// identifiers in this DFA, then this returns an error.
    ///
    /// This is a convenience routine for `to_sized::<u16>()`.
    pub fn to_u16(&self) -> Result<SparseDFA<Vec<u8>, u16>> {
        self.to_sized()
    }

    /// Create a new sparse DFA whose match semantics are equivalent to
    /// this DFA, but attempt to use `u32` for the representation of state
    /// identifiers. If `u32` is insufficient to represent all state
    /// identifiers in this DFA, then this returns an error.
    ///
    /// This is a convenience routine for `to_sized::<u32>()`.
    #[cfg(any(target_pointer_width = "32", target_pointer_width = "64"))]
    pub fn to_u32(&self) -> Result<SparseDFA<Vec<u8>, u32>> {
        self.to_sized()
    }

    /// Create a new sparse DFA whose match semantics are equivalent to
    /// this DFA, but attempt to use `u64` for the representation of state
    /// identifiers. If `u64` is insufficient to represent all state
    /// identifiers in this DFA, then this returns an error.
    ///
    /// This is a convenience routine for `to_sized::<u64>()`.
    #[cfg(target_pointer_width = "64")]
    pub fn to_u64(&self) -> Result<SparseDFA<Vec<u8>, u64>> {
        self.to_sized()
    }

    /// Create a new sparse DFA whose match semantics are equivalent to
    /// this DFA, but attempt to use `A` for the representation of state
    /// identifiers. If `A` is insufficient to represent all state identifiers
    /// in this DFA, then this returns an error.
    ///
    /// An alternative way to construct such a DFA is to use
    /// [`DenseDFA::to_sparse_sized`](enum.DenseDFA.html#method.to_sparse_sized).
    /// In general, picking the appropriate size upon initial construction of
    /// a sparse DFA is preferred, since it will do the conversion in one
    /// step instead of two.
    pub fn to_sized<A: StateID>(&self) -> Result<SparseDFA<Vec<u8>, A>> {
        self.repr().to_sized().map(|r| r.into_sparse_dfa())
    }

    /// Serialize a sparse DFA to raw bytes in little endian format.
    ///
    /// If the state identifier representation of this DFA has a size different
    /// than 1, 2, 4 or 8 bytes, then this returns an error. All
    /// implementations of `StateID` provided by this crate satisfy this
    /// requirement.
    pub fn to_bytes_little_endian(&self) -> Result<Vec<u8>> {
        self.repr().to_bytes::<LittleEndian>()
    }

    /// Serialize a sparse DFA to raw bytes in big endian format.
    ///
    /// If the state identifier representation of this DFA has a size different
    /// than 1, 2, 4 or 8 bytes, then this returns an error. All
    /// implementations of `StateID` provided by this crate satisfy this
    /// requirement.
    pub fn to_bytes_big_endian(&self) -> Result<Vec<u8>> {
        self.repr().to_bytes::<BigEndian>()
    }

    /// Serialize a sparse DFA to raw bytes in native endian format.
    /// Generally, it is better to pick an explicit endianness using either
    /// `to_bytes_little_endian` or `to_bytes_big_endian`. This routine is
    /// useful in tests where the DFA is serialized and deserialized on the
    /// same platform.
    ///
    /// If the state identifier representation of this DFA has a size different
    /// than 1, 2, 4 or 8 bytes, then this returns an error. All
    /// implementations of `StateID` provided by this crate satisfy this
    /// requirement.
    pub fn to_bytes_native_endian(&self) -> Result<Vec<u8>> {
        self.repr().to_bytes::<NativeEndian>()
    }
}

impl<'a, S: StateID> SparseDFA<&'a [u8], S> {
    /// Deserialize a sparse DFA with a specific state identifier
    /// representation.
    ///
    /// Deserializing a DFA using this routine will never allocate heap memory.
    /// This is also guaranteed to be a constant time operation that does not
    /// vary with the size of the DFA.
    ///
    /// The bytes given should be generated by the serialization of a DFA with
    /// either the
    /// [`to_bytes_little_endian`](enum.DenseDFA.html#method.to_bytes_little_endian)
    /// method or the
    /// [`to_bytes_big_endian`](enum.DenseDFA.html#method.to_bytes_big_endian)
    /// endian, depending on the endianness of the machine you are
    /// deserializing this DFA from.
    ///
    /// If the state identifier representation is `usize`, then deserialization
    /// is dependent on the pointer size. For this reason, it is best to
    /// serialize DFAs using a fixed size representation for your state
    /// identifiers, such as `u8`, `u16`, `u32` or `u64`.
    ///
    /// # Panics
    ///
    /// The bytes given should be *trusted*. In particular, if the bytes
    /// are not a valid serialization of a DFA, or if the endianness of the
    /// serialized bytes is different than the endianness of the machine that
    /// is deserializing the DFA, then this routine will panic. Moreover, it
    /// is possible for this deserialization routine to succeed even if the
    /// given bytes do not represent a valid serialized sparse DFA.
    ///
    /// # Safety
    ///
    /// This routine is unsafe because it permits callers to provide an
    /// arbitrary transition table with possibly incorrect transitions. While
    /// the various serialization routines will never return an incorrect
    /// transition table, there is no guarantee that the bytes provided here
    /// are correct. While deserialization does many checks (as documented
    /// above in the panic conditions), this routine does not check that the
    /// transition table is correct. Given an incorrect transition table, it is
    /// possible for the search routines to access out-of-bounds memory because
    /// of explicit bounds check elision.
    ///
    /// # Example
    ///
    /// This example shows how to serialize a DFA to raw bytes, deserialize it
    /// and then use it for searching. Note that we first convert the DFA to
    /// using `u16` for its state identifier representation before serializing
    /// it. While this isn't strictly necessary, it's good practice in order to
    /// decrease the size of the DFA and to avoid platform specific pitfalls
    /// such as differing pointer sizes.
    ///
    /// ```
    /// use regex_automata::{DFA, DenseDFA, SparseDFA};
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let sparse = SparseDFA::new("foo[0-9]+")?;
    /// let bytes = sparse.to_u16()?.to_bytes_native_endian()?;
    ///
    /// let dfa: SparseDFA<&[u8], u16> = unsafe {
    ///     SparseDFA::from_bytes(&bytes)
    /// };
    ///
    /// assert_eq!(Some(8), dfa.find(b"foo12345"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    pub unsafe fn from_bytes(buf: &'a [u8]) -> SparseDFA<&'a [u8], S> {
        Repr::from_bytes(buf).into_sparse_dfa()
    }
}

impl<T: AsRef<[u8]>, S: StateID> DFA for SparseDFA<T, S> {
    type ID = S;

    #[inline]
    fn start_state(&self) -> S {
        self.repr().start_state()
    }

    #[inline]
    fn is_match_state(&self, id: S) -> bool {
        self.repr().is_match_state(id)
    }

    #[inline]
    fn is_dead_state(&self, id: S) -> bool {
        self.repr().is_dead_state(id)
    }

    #[inline]
    fn is_match_or_dead_state(&self, id: S) -> bool {
        self.repr().is_match_or_dead_state(id)
    }

    #[inline]
    fn is_anchored(&self) -> bool {
        self.repr().is_anchored()
    }

    #[inline]
    fn next_state(&self, current: S, input: u8) -> S {
        match *self {
            SparseDFA::Standard(ref r) => r.next_state(current, input),
            SparseDFA::ByteClass(ref r) => r.next_state(current, input),
            SparseDFA::__Nonexhaustive => unreachable!(),
        }
    }

    #[inline]
    unsafe fn next_state_unchecked(&self, current: S, input: u8) -> S {
        self.next_state(current, input)
    }

    // We specialize the following methods because it lets us lift the
    // case analysis between the different types of sparse DFAs. Instead of
    // doing the case analysis for every transition, we do it once before
    // searching. For sparse DFAs, this doesn't seem to benefit performance as
    // much as it does for the dense DFAs, but it's easy to do so we might as
    // well do it.

    #[inline]
    fn is_match_at(&self, bytes: &[u8], start: usize) -> bool {
        match *self {
            SparseDFA::Standard(ref r) => r.is_match_at(bytes, start),
            SparseDFA::ByteClass(ref r) => r.is_match_at(bytes, start),
            SparseDFA::__Nonexhaustive => unreachable!(),
        }
    }

    #[inline]
    fn shortest_match_at(&self, bytes: &[u8], start: usize) -> Option<usize> {
        match *self {
            SparseDFA::Standard(ref r) => r.shortest_match_at(bytes, start),
            SparseDFA::ByteClass(ref r) => r.shortest_match_at(bytes, start),
            SparseDFA::__Nonexhaustive => unreachable!(),
        }
    }

    #[inline]
    fn find_at(&self, bytes: &[u8], start: usize) -> Option<usize> {
        match *self {
            SparseDFA::Standard(ref r) => r.find_at(bytes, start),
            SparseDFA::ByteClass(ref r) => r.find_at(bytes, start),
            SparseDFA::__Nonexhaustive => unreachable!(),
        }
    }

    #[inline]
    fn rfind_at(&self, bytes: &[u8], start: usize) -> Option<usize> {
        match *self {
            SparseDFA::Standard(ref r) => r.rfind_at(bytes, start),
            SparseDFA::ByteClass(ref r) => r.rfind_at(bytes, start),
            SparseDFA::__Nonexhaustive => unreachable!(),
        }
    }
}

/// A standard sparse DFA that does not use premultiplication or byte classes.
///
/// Generally, it isn't necessary to use this type directly, since a
/// `SparseDFA` can be used for searching directly. One possible reason why
/// one might want to use this type directly is if you are implementing your
/// own search routines by walking a DFA's transitions directly. In that case,
/// you'll want to use this type (or any of the other DFA variant types)
/// directly, since they implement `next_state` more efficiently.
#[derive(Clone, Debug)]
pub struct Standard<T: AsRef<[u8]>, S: StateID = usize>(Repr<T, S>);

impl<T: AsRef<[u8]>, S: StateID> DFA for Standard<T, S> {
    type ID = S;

    #[inline]
    fn start_state(&self) -> S {
        self.0.start_state()
    }

    #[inline]
    fn is_match_state(&self, id: S) -> bool {
        self.0.is_match_state(id)
    }

    #[inline]
    fn is_dead_state(&self, id: S) -> bool {
        self.0.is_dead_state(id)
    }

    #[inline]
    fn is_match_or_dead_state(&self, id: S) -> bool {
        self.0.is_match_or_dead_state(id)
    }

    #[inline]
    fn is_anchored(&self) -> bool {
        self.0.is_anchored()
    }

    #[inline]
    fn next_state(&self, current: S, input: u8) -> S {
        self.0.state(current).next(input)
    }

    #[inline]
    unsafe fn next_state_unchecked(&self, current: S, input: u8) -> S {
        self.next_state(current, input)
    }
}

/// A sparse DFA that shrinks its alphabet.
///
/// Alphabet shrinking is achieved by using a set of equivalence classes
/// instead of using all possible byte values. Any two bytes belong to the same
/// equivalence class if and only if they can be used interchangeably anywhere
/// in the DFA while never discriminating between a match and a non-match.
///
/// Unlike dense DFAs, sparse DFAs do not tend to benefit nearly as much from
/// using byte classes. In some cases, using byte classes can even marginally
/// increase the size of a sparse DFA's transition table. The reason for this
/// is that a sparse DFA already compacts each state's transitions separate
/// from whether byte classes are used.
///
/// Generally, it isn't necessary to use this type directly, since a
/// `SparseDFA` can be used for searching directly. One possible reason why
/// one might want to use this type directly is if you are implementing your
/// own search routines by walking a DFA's transitions directly. In that case,
/// you'll want to use this type (or any of the other DFA variant types)
/// directly, since they implement `next_state` more efficiently.
#[derive(Clone, Debug)]
pub struct ByteClass<T: AsRef<[u8]>, S: StateID = usize>(Repr<T, S>);

impl<T: AsRef<[u8]>, S: StateID> DFA for ByteClass<T, S> {
    type ID = S;

    #[inline]
    fn start_state(&self) -> S {
        self.0.start_state()
    }

    #[inline]
    fn is_match_state(&self, id: S) -> bool {
        self.0.is_match_state(id)
    }

    #[inline]
    fn is_dead_state(&self, id: S) -> bool {
        self.0.is_dead_state(id)
    }

    #[inline]
    fn is_match_or_dead_state(&self, id: S) -> bool {
        self.0.is_match_or_dead_state(id)
    }

    #[inline]
    fn is_anchored(&self) -> bool {
        self.0.is_anchored()
    }

    #[inline]
    fn next_state(&self, current: S, input: u8) -> S {
        let input = self.0.byte_classes.get(input);
        self.0.state(current).next(input)
    }

    #[inline]
    unsafe fn next_state_unchecked(&self, current: S, input: u8) -> S {
        self.next_state(current, input)
    }
}

/// The underlying representation of a sparse DFA. This is shared by all of
/// the different variants of a sparse DFA.
#[derive(Clone)]
#[cfg_attr(not(feature = "std"), derive(Debug))]
struct Repr<T: AsRef<[u8]>, S: StateID = usize> {
    anchored: bool,
    start: S,
    state_count: usize,
    max_match: S,
    byte_classes: ByteClasses,
    trans: T,
}

impl<T: AsRef<[u8]>, S: StateID> Repr<T, S> {
    fn into_sparse_dfa(self) -> SparseDFA<T, S> {
        if self.byte_classes.is_singleton() {
            SparseDFA::Standard(Standard(self))
        } else {
            SparseDFA::ByteClass(ByteClass(self))
        }
    }

    fn as_ref<'a>(&'a self) -> Repr<&'a [u8], S> {
        Repr {
            anchored: self.anchored,
            start: self.start,
            state_count: self.state_count,
            max_match: self.max_match,
            byte_classes: self.byte_classes.clone(),
            trans: self.trans(),
        }
    }

    #[cfg(feature = "std")]
    fn to_owned(&self) -> Repr<Vec<u8>, S> {
        Repr {
            anchored: self.anchored,
            start: self.start,
            state_count: self.state_count,
            max_match: self.max_match,
            byte_classes: self.byte_classes.clone(),
            trans: self.trans().to_vec(),
        }
    }

    /// Return a convenient representation of the given state.
    ///
    /// This is marked as inline because it doesn't seem to get inlined
    /// otherwise, which leads to a fairly significant performance loss (~25%).
    #[inline]
    fn state<'a>(&'a self, id: S) -> State<'a, S> {
        let mut pos = id.to_usize();
        let ntrans = NativeEndian::read_u16(&self.trans()[pos..]) as usize;
        pos += 2;
        let input_ranges = &self.trans()[pos..pos + (ntrans * 2)];
        pos += 2 * ntrans;
        let next = &self.trans()[pos..pos + (ntrans * size_of::<S>())];
        State { _state_id_repr: PhantomData, ntrans, input_ranges, next }
    }

    /// Return an iterator over all of the states in this DFA.
    ///
    /// The iterator returned yields tuples, where the first element is the
    /// state ID and the second element is the state itself.
    #[cfg(feature = "std")]
    fn states<'a>(&'a self) -> StateIter<'a, T, S> {
        StateIter { dfa: self, id: dead_id() }
    }

    fn memory_usage(&self) -> usize {
        self.trans().len()
    }

    fn start_state(&self) -> S {
        self.start
    }

    fn is_match_state(&self, id: S) -> bool {
        self.is_match_or_dead_state(id) && !self.is_dead_state(id)
    }

    fn is_dead_state(&self, id: S) -> bool {
        id == dead_id()
    }

    fn is_match_or_dead_state(&self, id: S) -> bool {
        id <= self.max_match
    }

    fn is_anchored(&self) -> bool {
        self.anchored
    }

    fn trans(&self) -> &[u8] {
        self.trans.as_ref()
    }

    /// Create a new sparse DFA whose match semantics are equivalent to this
    /// DFA, but attempt to use `A` for the representation of state
    /// identifiers. If `A` is insufficient to represent all state identifiers
    /// in this DFA, then this returns an error.
    #[cfg(feature = "std")]
    fn to_sized<A: StateID>(&self) -> Result<Repr<Vec<u8>, A>> {
        // To build the new DFA, we proceed much like the initial construction
        // of the sparse DFA. Namely, since the state ID size is changing,
        // we don't actually know all of our state IDs until we've allocated
        // all necessary space. So we do one pass that allocates all of the
        // storage we need, and then another pass to fill in the transitions.

        let mut trans = Vec::with_capacity(size_of::<A>() * self.state_count);
        let mut map: HashMap<S, A> = HashMap::with_capacity(self.state_count);
        for (old_id, state) in self.states() {
            let pos = trans.len();
            map.insert(old_id, usize_to_state_id(pos)?);

            let n = state.ntrans;
            let zeros = 2 + (n * 2) + (n * size_of::<A>());
            trans.extend(iter::repeat(0).take(zeros));

            NativeEndian::write_u16(&mut trans[pos..], n as u16);
            let (s, e) = (pos + 2, pos + 2 + (n * 2));
            trans[s..e].copy_from_slice(state.input_ranges);
        }

        let mut new = Repr {
            anchored: self.anchored,
            start: map[&self.start],
            state_count: self.state_count,
            max_match: map[&self.max_match],
            byte_classes: self.byte_classes.clone(),
            trans,
        };
        for (&old_id, &new_id) in map.iter() {
            let old_state = self.state(old_id);
            let mut new_state = new.state_mut(new_id);
            for i in 0..new_state.ntrans {
                let next = map[&old_state.next_at(i)];
                new_state.set_next_at(i, usize_to_state_id(next.to_usize())?);
            }
        }
        new.start = map[&self.start];
        new.max_match = map[&self.max_match];
        Ok(new)
    }

    /// Serialize a sparse DFA to raw bytes using the provided endianness.
    ///
    /// If the state identifier representation of this DFA has a size different
    /// than 1, 2, 4 or 8 bytes, then this returns an error. All
    /// implementations of `StateID` provided by this crate satisfy this
    /// requirement.
    ///
    /// Unlike dense DFAs, the result is not necessarily aligned since a
    /// sparse DFA's transition table is always read as a sequence of bytes.
    #[cfg(feature = "std")]
    fn to_bytes<A: ByteOrder>(&self) -> Result<Vec<u8>> {
        let label = b"rust-regex-automata-sparse-dfa\x00";
        let size =
            // For human readable label.
            label.len()
            // endiannes check, must be equal to 0xFEFF for native endian
            + 2
            // For version number.
            + 2
            // Size of state ID representation, in bytes.
            // Must be 1, 2, 4 or 8.
            + 2
            // For DFA misc options. (Currently unused.)
            + 2
            // For start state.
            + 8
            // For state count.
            + 8
            // For max match state.
            + 8
            // For byte class map.
            + 256
            // For transition table.
            + self.trans().len();

        let mut i = 0;
        let mut buf = vec![0; size];

        // write label
        for &b in label {
            buf[i] = b;
            i += 1;
        }
        // endianness check
        A::write_u16(&mut buf[i..], 0xFEFF);
        i += 2;
        // version number
        A::write_u16(&mut buf[i..], 1);
        i += 2;
        // size of state ID
        let state_size = size_of::<S>();
        if ![1, 2, 4, 8].contains(&state_size) {
            return Err(Error::serialize(&format!(
                "state size of {} not supported, must be 1, 2, 4 or 8",
                state_size
            )));
        }
        A::write_u16(&mut buf[i..], state_size as u16);
        i += 2;
        // DFA misc options
        let mut options = 0u16;
        if self.anchored {
            options |= dense::MASK_ANCHORED;
        }
        A::write_u16(&mut buf[i..], options);
        i += 2;
        // start state
        A::write_u64(&mut buf[i..], self.start.to_usize() as u64);
        i += 8;
        // state count
        A::write_u64(&mut buf[i..], self.state_count as u64);
        i += 8;
        // max match state
        A::write_u64(&mut buf[i..], self.max_match.to_usize() as u64);
        i += 8;
        // byte class map
        for b in (0..256).map(|b| b as u8) {
            buf[i] = self.byte_classes.get(b);
            i += 1;
        }
        // transition table
        for (_, state) in self.states() {
            A::write_u16(&mut buf[i..], state.ntrans as u16);
            i += 2;
            buf[i..i + (state.ntrans * 2)].copy_from_slice(state.input_ranges);
            i += state.ntrans * 2;
            for j in 0..state.ntrans {
                write_state_id_bytes::<A, _>(&mut buf[i..], state.next_at(j));
                i += size_of::<S>();
            }
        }

        assert_eq!(size, i, "expected to consume entire buffer");

        Ok(buf)
    }
}

impl<'a, S: StateID> Repr<&'a [u8], S> {
    /// The implementation for deserializing a sparse DFA from raw bytes.
    unsafe fn from_bytes(mut buf: &'a [u8]) -> Repr<&'a [u8], S> {
        // skip over label
        match buf.iter().position(|&b| b == b'\x00') {
            None => panic!("could not find label"),
            Some(i) => buf = &buf[i + 1..],
        }

        // check that current endianness is same as endianness of DFA
        let endian_check = NativeEndian::read_u16(buf);
        buf = &buf[2..];
        if endian_check != 0xFEFF {
            panic!(
                "endianness mismatch, expected 0xFEFF but got 0x{:X}. \
                 are you trying to load a SparseDFA serialized with a \
                 different endianness?",
                endian_check,
            );
        }

        // check that the version number is supported
        let version = NativeEndian::read_u16(buf);
        buf = &buf[2..];
        if version != 1 {
            panic!(
                "expected version 1, but found unsupported version {}",
                version,
            );
        }

        // read size of state
        let state_size = NativeEndian::read_u16(buf) as usize;
        if state_size != size_of::<S>() {
            panic!(
                "state size of SparseDFA ({}) does not match \
                 requested state size ({})",
                state_size,
                size_of::<S>(),
            );
        }
        buf = &buf[2..];

        // read miscellaneous options
        let opts = NativeEndian::read_u16(buf);
        buf = &buf[2..];

        // read start state
        let start = S::from_usize(NativeEndian::read_u64(buf) as usize);
        buf = &buf[8..];

        // read state count
        let state_count = NativeEndian::read_u64(buf) as usize;
        buf = &buf[8..];

        // read max match state
        let max_match = S::from_usize(NativeEndian::read_u64(buf) as usize);
        buf = &buf[8..];

        // read byte classes
        let byte_classes = ByteClasses::from_slice(&buf[..256]);
        buf = &buf[256..];

        Repr {
            anchored: opts & dense::MASK_ANCHORED > 0,
            start,
            state_count,
            max_match,
            byte_classes,
            trans: buf,
        }
    }
}

#[cfg(feature = "std")]
impl<S: StateID> Repr<Vec<u8>, S> {
    /// The implementation for constructing a sparse DFA from a dense DFA.
    fn from_dense_sized<T: AsRef<[S]>, A: StateID>(
        dfa: &dense::Repr<T, S>,
    ) -> Result<Repr<Vec<u8>, A>> {
        // In order to build the transition table, we need to be able to write
        // state identifiers for each of the "next" transitions in each state.
        // Our state identifiers correspond to the byte offset in the
        // transition table at which the state is encoded. Therefore, we do not
        // actually know what the state identifiers are until we've allocated
        // exactly as much space as we need for each state. Thus, construction
        // of the transition table happens in two passes.
        //
        // In the first pass, we fill out the shell of each state, which
        // includes the transition count, the input byte ranges and zero-filled
        // space for the transitions. In this first pass, we also build up a
        // map from the state identifier index of the dense DFA to the state
        // identifier in this sparse DFA.
        //
        // In the second pass, we fill in the transitions based on the map
        // built in the first pass.

        let mut trans = Vec::with_capacity(size_of::<A>() * dfa.state_count());
        let mut remap: Vec<A> = vec![dead_id(); dfa.state_count()];
        for (old_id, state) in dfa.states() {
            let pos = trans.len();

            remap[dfa.state_id_to_index(old_id)] = usize_to_state_id(pos)?;
            // zero-filled space for the transition count
            trans.push(0);
            trans.push(0);

            let mut trans_count = 0;
            for (b1, b2, _) in state.sparse_transitions() {
                trans_count += 1;
                trans.push(b1);
                trans.push(b2);
            }
            // fill in the transition count
            NativeEndian::write_u16(&mut trans[pos..], trans_count);

            // zero-fill the actual transitions
            let zeros = trans_count as usize * size_of::<A>();
            trans.extend(iter::repeat(0).take(zeros));
        }

        let mut new = Repr {
            anchored: dfa.is_anchored(),
            start: remap[dfa.state_id_to_index(dfa.start_state())],
            state_count: dfa.state_count(),
            max_match: remap[dfa.state_id_to_index(dfa.max_match_state())],
            byte_classes: dfa.byte_classes().clone(),
            trans,
        };
        for (old_id, old_state) in dfa.states() {
            let new_id = remap[dfa.state_id_to_index(old_id)];
            let mut new_state = new.state_mut(new_id);
            let sparse = old_state.sparse_transitions();
            for (i, (_, _, next)) in sparse.enumerate() {
                let next = remap[dfa.state_id_to_index(next)];
                new_state.set_next_at(i, next);
            }
        }
        Ok(new)
    }

    /// Return a convenient mutable representation of the given state.
    fn state_mut<'a>(&'a mut self, id: S) -> StateMut<'a, S> {
        let mut pos = id.to_usize();
        let ntrans = NativeEndian::read_u16(&self.trans[pos..]) as usize;
        pos += 2;

        let size = (ntrans * 2) + (ntrans * size_of::<S>());
        let ranges_and_next = &mut self.trans[pos..pos + size];
        let (input_ranges, next) = ranges_and_next.split_at_mut(ntrans * 2);
        StateMut { _state_id_repr: PhantomData, ntrans, input_ranges, next }
    }
}

#[cfg(feature = "std")]
impl<T: AsRef<[u8]>, S: StateID> fmt::Debug for Repr<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fn state_status<T: AsRef<[u8]>, S: StateID>(
            dfa: &Repr<T, S>,
            id: S,
        ) -> &'static str {
            if id == dead_id() {
                if dfa.is_match_state(id) {
                    "D*"
                } else {
                    "D "
                }
            } else if id == dfa.start_state() {
                if dfa.is_match_state(id) {
                    ">*"
                } else {
                    "> "
                }
            } else {
                if dfa.is_match_state(id) {
                    " *"
                } else {
                    "  "
                }
            }
        }

        writeln!(f, "SparseDFA(")?;
        for (id, state) in self.states() {
            let status = state_status(self, id);
            writeln!(f, "{}{:06}: {:?}", status, id.to_usize(), state)?;
        }
        writeln!(f, ")")?;
        Ok(())
    }
}

/// An iterator over all states in a sparse DFA.
///
/// This iterator yields tuples, where the first element is the state ID and
/// the second element is the state itself.
#[cfg(feature = "std")]
#[derive(Debug)]
struct StateIter<'a, T: AsRef<[u8]> + 'a, S: StateID + 'a = usize> {
    dfa: &'a Repr<T, S>,
    id: S,
}

#[cfg(feature = "std")]
impl<'a, T: AsRef<[u8]>, S: StateID> Iterator for StateIter<'a, T, S> {
    type Item = (S, State<'a, S>);

    fn next(&mut self) -> Option<(S, State<'a, S>)> {
        if self.id.to_usize() >= self.dfa.trans().len() {
            return None;
        }
        let id = self.id;
        let state = self.dfa.state(id);
        self.id = S::from_usize(self.id.to_usize() + state.bytes());
        Some((id, state))
    }
}

/// A representation of a sparse DFA state that can be cheaply materialized
/// from a state identifier.
#[derive(Clone)]
struct State<'a, S: StateID = usize> {
    /// The state identifier representation used by the DFA from which this
    /// state was extracted. Since our transition table is compacted in a
    /// &[u8], we don't actually use the state ID type parameter explicitly
    /// anywhere, so we fake it. This prevents callers from using an incorrect
    /// state ID representation to read from this state.
    _state_id_repr: PhantomData<S>,
    /// The number of transitions in this state.
    ntrans: usize,
    /// Pairs of input ranges, where there is one pair for each transition.
    /// Each pair specifies an inclusive start and end byte range for the
    /// corresponding transition.
    input_ranges: &'a [u8],
    /// Transitions to the next state. This slice contains native endian
    /// encoded state identifiers, with `S` as the representation. Thus, there
    /// are `ntrans * size_of::<S>()` bytes in this slice.
    next: &'a [u8],
}

impl<'a, S: StateID> State<'a, S> {
    /// Searches for the next transition given an input byte. If no such
    /// transition could be found, then a dead state is returned.
    fn next(&self, input: u8) -> S {
        // This straight linear search was observed to be much better than
        // binary search on ASCII haystacks, likely because a binary search
        // visits the ASCII case last but a linear search sees it first. A
        // binary search does do a little better on non-ASCII haystacks, but
        // not by much. There might be a better trade off lurking here.
        for i in 0..self.ntrans {
            let (start, end) = self.range(i);
            if start <= input && input <= end {
                return self.next_at(i);
            }
            // We could bail early with an extra branch: if input < b1, then
            // we know we'll never find a matching transition. Interestingly,
            // this extra branch seems to not help performance, or will even
            // hurt it. It's likely very dependent on the DFA itself and what
            // is being searched.
        }
        dead_id()
    }

    /// Returns the inclusive input byte range for the ith transition in this
    /// state.
    fn range(&self, i: usize) -> (u8, u8) {
        (self.input_ranges[i * 2], self.input_ranges[i * 2 + 1])
    }

    /// Returns the next state for the ith transition in this state.
    fn next_at(&self, i: usize) -> S {
        S::read_bytes(&self.next[i * size_of::<S>()..])
    }

    /// Return the total number of bytes that this state consumes in its
    /// encoded form.
    #[cfg(feature = "std")]
    fn bytes(&self) -> usize {
        2 + (self.ntrans * 2) + (self.ntrans * size_of::<S>())
    }
}

#[cfg(feature = "std")]
impl<'a, S: StateID> fmt::Debug for State<'a, S> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut transitions = vec![];
        for i in 0..self.ntrans {
            let next = self.next_at(i);
            if next == dead_id() {
                continue;
            }

            let (start, end) = self.range(i);
            if start == end {
                transitions.push(format!(
                    "{} => {}",
                    escape(start),
                    next.to_usize()
                ));
            } else {
                transitions.push(format!(
                    "{}-{} => {}",
                    escape(start),
                    escape(end),
                    next.to_usize(),
                ));
            }
        }
        write!(f, "{}", transitions.join(", "))
    }
}

/// A representation of a mutable sparse DFA state that can be cheaply
/// materialized from a state identifier.
#[cfg(feature = "std")]
struct StateMut<'a, S: StateID = usize> {
    /// The state identifier representation used by the DFA from which this
    /// state was extracted. Since our transition table is compacted in a
    /// &[u8], we don't actually use the state ID type parameter explicitly
    /// anywhere, so we fake it. This prevents callers from using an incorrect
    /// state ID representation to read from this state.
    _state_id_repr: PhantomData<S>,
    /// The number of transitions in this state.
    ntrans: usize,
    /// Pairs of input ranges, where there is one pair for each transition.
    /// Each pair specifies an inclusive start and end byte range for the
    /// corresponding transition.
    input_ranges: &'a mut [u8],
    /// Transitions to the next state. This slice contains native endian
    /// encoded state identifiers, with `S` as the representation. Thus, there
    /// are `ntrans * size_of::<S>()` bytes in this slice.
    next: &'a mut [u8],
}

#[cfg(feature = "std")]
impl<'a, S: StateID> StateMut<'a, S> {
    /// Sets the ith transition to the given state.
    fn set_next_at(&mut self, i: usize, next: S) {
        next.write_bytes(&mut self.next[i * size_of::<S>()..]);
    }
}

#[cfg(feature = "std")]
impl<'a, S: StateID> fmt::Debug for StateMut<'a, S> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let state = State {
            _state_id_repr: self._state_id_repr,
            ntrans: self.ntrans,
            input_ranges: self.input_ranges,
            next: self.next,
        };
        fmt::Debug::fmt(&state, f)
    }
}

/// Return the given byte as its escaped string form.
#[cfg(feature = "std")]
fn escape(b: u8) -> String {
    use std::ascii;

    String::from_utf8(ascii::escape_default(b).collect::<Vec<_>>()).unwrap()
}

/// A binary search routine specialized specifically to a sparse DFA state's
/// transitions. Specifically, the transitions are defined as a set of pairs
/// of input bytes that delineate an inclusive range of bytes. If the input
/// byte is in the range, then the corresponding transition is a match.
///
/// This binary search accepts a slice of these pairs and returns the position
/// of the matching pair (the ith transition), or None if no matching pair
/// could be found.
///
/// Note that this routine is not currently used since it was observed to
/// either decrease performance when searching ASCII, or did not provide enough
/// of a boost on non-ASCII haystacks to be worth it. However, we leave it here
/// for posterity in case we can find a way to use it.
///
/// In theory, we could use the standard library's search routine if we could
/// cast a `&[u8]` to a `&[(u8, u8)]`, but I don't believe this is currently
/// guaranteed to be safe and is thus UB (since I don't think the in-memory
/// representation of `(u8, u8)` has been nailed down).
#[inline(always)]
#[allow(dead_code)]
fn binary_search_ranges(ranges: &[u8], needle: u8) -> Option<usize> {
    debug_assert!(ranges.len() % 2 == 0, "ranges must have even length");
    debug_assert!(ranges.len() <= 512, "ranges should be short");

    let (mut left, mut right) = (0, ranges.len() / 2);
    while left < right {
        let mid = (left + right) / 2;
        let (b1, b2) = (ranges[mid * 2], ranges[mid * 2 + 1]);
        if needle < b1 {
            right = mid;
        } else if needle > b2 {
            left = mid + 1;
        } else {
            return Some(mid);
        }
    }
    None
}

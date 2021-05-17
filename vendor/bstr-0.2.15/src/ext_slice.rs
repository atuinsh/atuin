#[cfg(feature = "std")]
use std::borrow::Cow;
#[cfg(feature = "std")]
use std::ffi::OsStr;
#[cfg(feature = "std")]
use std::path::Path;

use core::{cmp, iter, ops, ptr, slice, str};
use memchr::{memchr, memrchr};

use ascii;
use bstr::BStr;
use byteset;
#[cfg(feature = "std")]
use ext_vec::ByteVec;
use search::{PrefilterState, TwoWay};
#[cfg(feature = "unicode")]
use unicode::{
    whitespace_len_fwd, whitespace_len_rev, GraphemeIndices, Graphemes,
    SentenceIndices, Sentences, WordIndices, Words, WordsWithBreakIndices,
    WordsWithBreaks,
};
use utf8::{self, CharIndices, Chars, Utf8Chunks, Utf8Error};

/// A short-hand constructor for building a `&[u8]`.
///
/// This idiosyncratic constructor is useful for concisely building byte string
/// slices. Its primary utility is in conveniently writing byte string literals
/// in a uniform way. For example, consider this code that does not compile:
///
/// ```ignore
/// let strs = vec![b"a", b"xy"];
/// ```
///
/// The above code doesn't compile because the type of the byte string literal
/// `b"a"` is `&'static [u8; 1]`, and the type of `b"xy"` is
/// `&'static [u8; 2]`. Since their types aren't the same, they can't be stored
/// in the same `Vec`. (This is dissimilar from normal Unicode string slices,
/// where both `"a"` and `"xy"` have the same type of `&'static str`.)
///
/// One way of getting the above code to compile is to convert byte strings to
/// slices. You might try this:
///
/// ```ignore
/// let strs = vec![&b"a", &b"xy"];
/// ```
///
/// But this just creates values with type `& &'static [u8; 1]` and
/// `& &'static [u8; 2]`. Instead, you need to force the issue like so:
///
/// ```
/// let strs = vec![&b"a"[..], &b"xy"[..]];
/// // or
/// let strs = vec![b"a".as_ref(), b"xy".as_ref()];
/// ```
///
/// But neither of these are particularly convenient to type, especially when
/// it's something as common as a string literal. Thus, this constructor
/// permits writing the following instead:
///
/// ```
/// use bstr::B;
///
/// let strs = vec![B("a"), B(b"xy")];
/// ```
///
/// Notice that this also lets you mix and match both string literals and byte
/// string literals. This can be quite convenient!
#[allow(non_snake_case)]
#[inline]
pub fn B<'a, B: ?Sized + AsRef<[u8]>>(bytes: &'a B) -> &'a [u8] {
    bytes.as_ref()
}

impl ByteSlice for [u8] {
    #[inline]
    fn as_bytes(&self) -> &[u8] {
        self
    }

    #[inline]
    fn as_bytes_mut(&mut self) -> &mut [u8] {
        self
    }
}

/// Ensure that callers cannot implement `ByteSlice` by making an
/// umplementable trait its super trait.
pub trait Sealed {}
impl Sealed for [u8] {}

/// A trait that extends `&[u8]` with string oriented methods.
pub trait ByteSlice: Sealed {
    /// A method for accessing the raw bytes of this type. This is always a
    /// no-op and callers shouldn't care about it. This only exists for making
    /// the extension trait work.
    #[doc(hidden)]
    fn as_bytes(&self) -> &[u8];

    /// A method for accessing the raw bytes of this type, mutably. This is
    /// always a no-op and callers shouldn't care about it. This only exists
    /// for making the extension trait work.
    #[doc(hidden)]
    fn as_bytes_mut(&mut self) -> &mut [u8];

    /// Return this byte slice as a `&BStr`.
    ///
    /// Use `&BStr` is useful because of its `fmt::Debug` representation
    /// and various other trait implementations (such as `PartialEq` and
    /// `PartialOrd`). In particular, the `Debug` implementation for `BStr`
    /// shows its bytes as a normal string. For invalid UTF-8, hex escape
    /// sequences are used.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// println!("{:?}", b"foo\xFFbar".as_bstr());
    /// ```
    #[inline]
    fn as_bstr(&self) -> &BStr {
        BStr::new(self.as_bytes())
    }

    /// Return this byte slice as a `&mut BStr`.
    ///
    /// Use `&mut BStr` is useful because of its `fmt::Debug` representation
    /// and various other trait implementations (such as `PartialEq` and
    /// `PartialOrd`). In particular, the `Debug` implementation for `BStr`
    /// shows its bytes as a normal string. For invalid UTF-8, hex escape
    /// sequences are used.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let mut bytes = *b"foo\xFFbar";
    /// println!("{:?}", &mut bytes.as_bstr_mut());
    /// ```
    #[inline]
    fn as_bstr_mut(&mut self) -> &mut BStr {
        BStr::new_mut(self.as_bytes_mut())
    }

    /// Create an immutable byte string from an OS string slice.
    ///
    /// On Unix, this always succeeds and is zero cost. On non-Unix systems,
    /// this returns `None` if the given OS string is not valid UTF-8. (For
    /// example, on Windows, file paths are allowed to be a sequence of
    /// arbitrary 16-bit integers. Not all such sequences can be transcoded to
    /// valid UTF-8.)
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use std::ffi::OsStr;
    ///
    /// use bstr::{B, ByteSlice};
    ///
    /// let os_str = OsStr::new("foo");
    /// let bs = <[u8]>::from_os_str(os_str).expect("should be valid UTF-8");
    /// assert_eq!(bs, B("foo"));
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn from_os_str(os_str: &OsStr) -> Option<&[u8]> {
        #[cfg(unix)]
        #[inline]
        fn imp(os_str: &OsStr) -> Option<&[u8]> {
            use std::os::unix::ffi::OsStrExt;

            Some(os_str.as_bytes())
        }

        #[cfg(not(unix))]
        #[inline]
        fn imp(os_str: &OsStr) -> Option<&[u8]> {
            os_str.to_str().map(|s| s.as_bytes())
        }

        imp(os_str)
    }

    /// Create an immutable byte string from a file path.
    ///
    /// On Unix, this always succeeds and is zero cost. On non-Unix systems,
    /// this returns `None` if the given path is not valid UTF-8. (For example,
    /// on Windows, file paths are allowed to be a sequence of arbitrary 16-bit
    /// integers. Not all such sequences can be transcoded to valid UTF-8.)
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use bstr::{B, ByteSlice};
    ///
    /// let path = Path::new("foo");
    /// let bs = <[u8]>::from_path(path).expect("should be valid UTF-8");
    /// assert_eq!(bs, B("foo"));
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn from_path(path: &Path) -> Option<&[u8]> {
        Self::from_os_str(path.as_os_str())
    }

    /// Safely convert this byte string into a `&str` if it's valid UTF-8.
    ///
    /// If this byte string is not valid UTF-8, then an error is returned. The
    /// error returned indicates the first invalid byte found and the length
    /// of the error.
    ///
    /// In cases where a lossy conversion to `&str` is acceptable, then use one
    /// of the [`to_str_lossy`](trait.ByteSlice.html#method.to_str_lossy) or
    /// [`to_str_lossy_into`](trait.ByteSlice.html#method.to_str_lossy_into)
    /// methods.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice, ByteVec};
    ///
    /// # fn example() -> Result<(), bstr::Utf8Error> {
    /// let s = B("☃βツ").to_str()?;
    /// assert_eq!("☃βツ", s);
    ///
    /// let mut bstring = <Vec<u8>>::from("☃βツ");
    /// bstring.push(b'\xFF');
    /// let err = bstring.to_str().unwrap_err();
    /// assert_eq!(8, err.valid_up_to());
    /// # Ok(()) }; example().unwrap()
    /// ```
    #[inline]
    fn to_str(&self) -> Result<&str, Utf8Error> {
        utf8::validate(self.as_bytes()).map(|_| {
            // SAFETY: This is safe because of the guarantees provided by
            // utf8::validate.
            unsafe { str::from_utf8_unchecked(self.as_bytes()) }
        })
    }

    /// Unsafely convert this byte string into a `&str`, without checking for
    /// valid UTF-8.
    ///
    /// # Safety
    ///
    /// Callers *must* ensure that this byte string is valid UTF-8 before
    /// calling this method. Converting a byte string into a `&str` that is
    /// not valid UTF-8 is considered undefined behavior.
    ///
    /// This routine is useful in performance sensitive contexts where the
    /// UTF-8 validity of the byte string is already known and it is
    /// undesirable to pay the cost of an additional UTF-8 validation check
    /// that [`to_str`](trait.ByteSlice.html#method.to_str) performs.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// // SAFETY: This is safe because string literals are guaranteed to be
    /// // valid UTF-8 by the Rust compiler.
    /// let s = unsafe { B("☃βツ").to_str_unchecked() };
    /// assert_eq!("☃βツ", s);
    /// ```
    #[inline]
    unsafe fn to_str_unchecked(&self) -> &str {
        str::from_utf8_unchecked(self.as_bytes())
    }

    /// Convert this byte string to a valid UTF-8 string by replacing invalid
    /// UTF-8 bytes with the Unicode replacement codepoint (`U+FFFD`).
    ///
    /// If the byte string is already valid UTF-8, then no copying or
    /// allocation is performed and a borrrowed string slice is returned. If
    /// the byte string is not valid UTF-8, then an owned string buffer is
    /// returned with invalid bytes replaced by the replacement codepoint.
    ///
    /// This method uses the "substitution of maximal subparts" (Unicode
    /// Standard, Chapter 3, Section 9) strategy for inserting the replacement
    /// codepoint. Specifically, a replacement codepoint is inserted whenever a
    /// byte is found that cannot possibly lead to a valid code unit sequence.
    /// If there were previous bytes that represented a prefix of a well-formed
    /// code unit sequence, then all of those bytes are substituted with a
    /// single replacement codepoint. The "substitution of maximal subparts"
    /// strategy is the same strategy used by
    /// [W3C's Encoding standard](https://www.w3.org/TR/encoding/).
    /// For a more precise description of the maximal subpart strategy, see
    /// the Unicode Standard, Chapter 3, Section 9. See also
    /// [Public Review Issue #121](http://www.unicode.org/review/pr-121.html).
    ///
    /// N.B. Rust's standard library also appears to use the same strategy,
    /// but it does not appear to be an API guarantee.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use std::borrow::Cow;
    ///
    /// use bstr::ByteSlice;
    ///
    /// let mut bstring = <Vec<u8>>::from("☃βツ");
    /// assert_eq!(Cow::Borrowed("☃βツ"), bstring.to_str_lossy());
    ///
    /// // Add a byte that makes the sequence invalid.
    /// bstring.push(b'\xFF');
    /// assert_eq!(Cow::Borrowed("☃βツ\u{FFFD}"), bstring.to_str_lossy());
    /// ```
    ///
    /// This demonstrates the "maximal subpart" substitution logic.
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// // \x61 is the ASCII codepoint for 'a'.
    /// // \xF1\x80\x80 is a valid 3-byte code unit prefix.
    /// // \xE1\x80 is a valid 2-byte code unit prefix.
    /// // \xC2 is a valid 1-byte code unit prefix.
    /// // \x62 is the ASCII codepoint for 'b'.
    /// //
    /// // In sum, each of the prefixes is replaced by a single replacement
    /// // codepoint since none of the prefixes are properly completed. This
    /// // is in contrast to other strategies that might insert a replacement
    /// // codepoint for every single byte.
    /// let bs = B(b"\x61\xF1\x80\x80\xE1\x80\xC2\x62");
    /// assert_eq!("a\u{FFFD}\u{FFFD}\u{FFFD}b", bs.to_str_lossy());
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn to_str_lossy(&self) -> Cow<str> {
        match utf8::validate(self.as_bytes()) {
            Ok(()) => {
                // SAFETY: This is safe because of the guarantees provided by
                // utf8::validate.
                unsafe {
                    Cow::Borrowed(str::from_utf8_unchecked(self.as_bytes()))
                }
            }
            Err(err) => {
                let mut lossy = String::with_capacity(self.as_bytes().len());
                let (valid, after) =
                    self.as_bytes().split_at(err.valid_up_to());
                // SAFETY: This is safe because utf8::validate guarantees
                // that all of `valid` is valid UTF-8.
                lossy.push_str(unsafe { str::from_utf8_unchecked(valid) });
                lossy.push_str("\u{FFFD}");
                if let Some(len) = err.error_len() {
                    after[len..].to_str_lossy_into(&mut lossy);
                }
                Cow::Owned(lossy)
            }
        }
    }

    /// Copy the contents of this byte string into the given owned string
    /// buffer, while replacing invalid UTF-8 code unit sequences with the
    /// Unicode replacement codepoint (`U+FFFD`).
    ///
    /// This method uses the same "substitution of maximal subparts" strategy
    /// for inserting the replacement codepoint as the
    /// [`to_str_lossy`](trait.ByteSlice.html#method.to_str_lossy) method.
    ///
    /// This routine is useful for amortizing allocation. However, unlike
    /// `to_str_lossy`, this routine will _always_ copy the contents of this
    /// byte string into the destination buffer, even if this byte string is
    /// valid UTF-8.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use std::borrow::Cow;
    ///
    /// use bstr::ByteSlice;
    ///
    /// let mut bstring = <Vec<u8>>::from("☃βツ");
    /// // Add a byte that makes the sequence invalid.
    /// bstring.push(b'\xFF');
    ///
    /// let mut dest = String::new();
    /// bstring.to_str_lossy_into(&mut dest);
    /// assert_eq!("☃βツ\u{FFFD}", dest);
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn to_str_lossy_into(&self, dest: &mut String) {
        let mut bytes = self.as_bytes();
        dest.reserve(bytes.len());
        loop {
            match utf8::validate(bytes) {
                Ok(()) => {
                    // SAFETY: This is safe because utf8::validate guarantees
                    // that all of `bytes` is valid UTF-8.
                    dest.push_str(unsafe { str::from_utf8_unchecked(bytes) });
                    break;
                }
                Err(err) => {
                    let (valid, after) = bytes.split_at(err.valid_up_to());
                    // SAFETY: This is safe because utf8::validate guarantees
                    // that all of `valid` is valid UTF-8.
                    dest.push_str(unsafe { str::from_utf8_unchecked(valid) });
                    dest.push_str("\u{FFFD}");
                    match err.error_len() {
                        None => break,
                        Some(len) => bytes = &after[len..],
                    }
                }
            }
        }
    }

    /// Create an OS string slice from this byte string.
    ///
    /// On Unix, this always succeeds and is zero cost. On non-Unix systems,
    /// this returns a UTF-8 decoding error if this byte string is not valid
    /// UTF-8. (For example, on Windows, file paths are allowed to be a
    /// sequence of arbitrary 16-bit integers. There is no obvious mapping from
    /// an arbitrary sequence of 8-bit integers to an arbitrary sequence of
    /// 16-bit integers.)
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let os_str = b"foo".to_os_str().expect("should be valid UTF-8");
    /// assert_eq!(os_str, "foo");
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn to_os_str(&self) -> Result<&OsStr, Utf8Error> {
        #[cfg(unix)]
        #[inline]
        fn imp(bytes: &[u8]) -> Result<&OsStr, Utf8Error> {
            use std::os::unix::ffi::OsStrExt;

            Ok(OsStr::from_bytes(bytes))
        }

        #[cfg(not(unix))]
        #[inline]
        fn imp(bytes: &[u8]) -> Result<&OsStr, Utf8Error> {
            bytes.to_str().map(OsStr::new)
        }

        imp(self.as_bytes())
    }

    /// Lossily create an OS string slice from this byte string.
    ///
    /// On Unix, this always succeeds and is zero cost. On non-Unix systems,
    /// this will perform a UTF-8 check and lossily convert this byte string
    /// into valid UTF-8 using the Unicode replacement codepoint.
    ///
    /// Note that this can prevent the correct roundtripping of file paths on
    /// non-Unix systems such as Windows, where file paths are an arbitrary
    /// sequence of 16-bit integers.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let os_str = b"foo\xFFbar".to_os_str_lossy();
    /// assert_eq!(os_str.to_string_lossy(), "foo\u{FFFD}bar");
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn to_os_str_lossy(&self) -> Cow<OsStr> {
        #[cfg(unix)]
        #[inline]
        fn imp(bytes: &[u8]) -> Cow<OsStr> {
            use std::os::unix::ffi::OsStrExt;

            Cow::Borrowed(OsStr::from_bytes(bytes))
        }

        #[cfg(not(unix))]
        #[inline]
        fn imp(bytes: &[u8]) -> Cow<OsStr> {
            use std::ffi::OsString;

            match bytes.to_str_lossy() {
                Cow::Borrowed(x) => Cow::Borrowed(OsStr::new(x)),
                Cow::Owned(x) => Cow::Owned(OsString::from(x)),
            }
        }

        imp(self.as_bytes())
    }

    /// Create a path slice from this byte string.
    ///
    /// On Unix, this always succeeds and is zero cost. On non-Unix systems,
    /// this returns a UTF-8 decoding error if this byte string is not valid
    /// UTF-8. (For example, on Windows, file paths are allowed to be a
    /// sequence of arbitrary 16-bit integers. There is no obvious mapping from
    /// an arbitrary sequence of 8-bit integers to an arbitrary sequence of
    /// 16-bit integers.)
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let path = b"foo".to_path().expect("should be valid UTF-8");
    /// assert_eq!(path.as_os_str(), "foo");
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn to_path(&self) -> Result<&Path, Utf8Error> {
        self.to_os_str().map(Path::new)
    }

    /// Lossily create a path slice from this byte string.
    ///
    /// On Unix, this always succeeds and is zero cost. On non-Unix systems,
    /// this will perform a UTF-8 check and lossily convert this byte string
    /// into valid UTF-8 using the Unicode replacement codepoint.
    ///
    /// Note that this can prevent the correct roundtripping of file paths on
    /// non-Unix systems such as Windows, where file paths are an arbitrary
    /// sequence of 16-bit integers.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = b"foo\xFFbar";
    /// let path = bs.to_path_lossy();
    /// assert_eq!(path.to_string_lossy(), "foo\u{FFFD}bar");
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn to_path_lossy(&self) -> Cow<Path> {
        use std::path::PathBuf;

        match self.to_os_str_lossy() {
            Cow::Borrowed(x) => Cow::Borrowed(Path::new(x)),
            Cow::Owned(x) => Cow::Owned(PathBuf::from(x)),
        }
    }

    /// Create a new byte string by repeating this byte string `n` times.
    ///
    /// # Panics
    ///
    /// This function panics if the capacity of the new byte string would
    /// overflow.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// assert_eq!(b"foo".repeatn(4), B("foofoofoofoo"));
    /// assert_eq!(b"foo".repeatn(0), B(""));
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn repeatn(&self, n: usize) -> Vec<u8> {
        let bs = self.as_bytes();
        let mut dst = vec![0; bs.len() * n];
        for i in 0..n {
            dst[i * bs.len()..(i + 1) * bs.len()].copy_from_slice(bs);
        }
        dst
    }

    /// Returns true if and only if this byte string contains the given needle.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// assert!(b"foo bar".contains_str("foo"));
    /// assert!(b"foo bar".contains_str("bar"));
    /// assert!(!b"foo".contains_str("foobar"));
    /// ```
    #[inline]
    fn contains_str<B: AsRef<[u8]>>(&self, needle: B) -> bool {
        self.find(needle).is_some()
    }

    /// Returns true if and only if this byte string has the given prefix.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// assert!(b"foo bar".starts_with_str("foo"));
    /// assert!(!b"foo bar".starts_with_str("bar"));
    /// assert!(!b"foo".starts_with_str("foobar"));
    /// ```
    #[inline]
    fn starts_with_str<B: AsRef<[u8]>>(&self, prefix: B) -> bool {
        self.as_bytes().starts_with(prefix.as_ref())
    }

    /// Returns true if and only if this byte string has the given suffix.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// assert!(b"foo bar".ends_with_str("bar"));
    /// assert!(!b"foo bar".ends_with_str("foo"));
    /// assert!(!b"bar".ends_with_str("foobar"));
    /// ```
    #[inline]
    fn ends_with_str<B: AsRef<[u8]>>(&self, suffix: B) -> bool {
        self.as_bytes().ends_with(suffix.as_ref())
    }

    /// Returns the index of the first occurrence of the given needle.
    ///
    /// The needle may be any type that can be cheaply converted into a
    /// `&[u8]`. This includes, but is not limited to, `&str` and `&[u8]`.
    ///
    /// Note that if you're are searching for the same needle in many
    /// different small haystacks, it may be faster to initialize a
    /// [`Finder`](struct.Finder.html) once, and reuse it for each search.
    ///
    /// # Complexity
    ///
    /// This routine is guaranteed to have worst case linear time complexity
    /// with respect to both the needle and the haystack. That is, this runs
    /// in `O(needle.len() + haystack.len())` time.
    ///
    /// This routine is also guaranteed to have worst case constant space
    /// complexity.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"foo bar baz";
    /// assert_eq!(Some(0), s.find("foo"));
    /// assert_eq!(Some(4), s.find("bar"));
    /// assert_eq!(None, s.find("quux"));
    /// ```
    #[inline]
    fn find<B: AsRef<[u8]>>(&self, needle: B) -> Option<usize> {
        Finder::new(needle.as_ref()).find(self.as_bytes())
    }

    /// Returns the index of the last occurrence of the given needle.
    ///
    /// The needle may be any type that can be cheaply converted into a
    /// `&[u8]`. This includes, but is not limited to, `&str` and `&[u8]`.
    ///
    /// Note that if you're are searching for the same needle in many
    /// different small haystacks, it may be faster to initialize a
    /// [`FinderReverse`](struct.FinderReverse.html) once, and reuse it for
    /// each search.
    ///
    /// # Complexity
    ///
    /// This routine is guaranteed to have worst case linear time complexity
    /// with respect to both the needle and the haystack. That is, this runs
    /// in `O(needle.len() + haystack.len())` time.
    ///
    /// This routine is also guaranteed to have worst case constant space
    /// complexity.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"foo bar baz";
    /// assert_eq!(Some(0), s.rfind("foo"));
    /// assert_eq!(Some(4), s.rfind("bar"));
    /// assert_eq!(Some(8), s.rfind("ba"));
    /// assert_eq!(None, s.rfind("quux"));
    /// ```
    #[inline]
    fn rfind<B: AsRef<[u8]>>(&self, needle: B) -> Option<usize> {
        FinderReverse::new(needle.as_ref()).rfind(self.as_bytes())
    }

    /// Returns an iterator of the non-overlapping occurrences of the given
    /// needle. The iterator yields byte offset positions indicating the start
    /// of each match.
    ///
    /// # Complexity
    ///
    /// This routine is guaranteed to have worst case linear time complexity
    /// with respect to both the needle and the haystack. That is, this runs
    /// in `O(needle.len() + haystack.len())` time.
    ///
    /// This routine is also guaranteed to have worst case constant space
    /// complexity.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"foo bar foo foo quux foo";
    /// let matches: Vec<usize> = s.find_iter("foo").collect();
    /// assert_eq!(matches, vec![0, 8, 12, 21]);
    /// ```
    ///
    /// An empty string matches at every position, including the position
    /// immediately following the last byte:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let matches: Vec<usize> = b"foo".find_iter("").collect();
    /// assert_eq!(matches, vec![0, 1, 2, 3]);
    ///
    /// let matches: Vec<usize> = b"".find_iter("").collect();
    /// assert_eq!(matches, vec![0]);
    /// ```
    #[inline]
    fn find_iter<'a, B: ?Sized + AsRef<[u8]>>(
        &'a self,
        needle: &'a B,
    ) -> Find<'a> {
        Find::new(self.as_bytes(), needle.as_ref())
    }

    /// Returns an iterator of the non-overlapping occurrences of the given
    /// needle in reverse. The iterator yields byte offset positions indicating
    /// the start of each match.
    ///
    /// # Complexity
    ///
    /// This routine is guaranteed to have worst case linear time complexity
    /// with respect to both the needle and the haystack. That is, this runs
    /// in `O(needle.len() + haystack.len())` time.
    ///
    /// This routine is also guaranteed to have worst case constant space
    /// complexity.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"foo bar foo foo quux foo";
    /// let matches: Vec<usize> = s.rfind_iter("foo").collect();
    /// assert_eq!(matches, vec![21, 12, 8, 0]);
    /// ```
    ///
    /// An empty string matches at every position, including the position
    /// immediately following the last byte:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let matches: Vec<usize> = b"foo".rfind_iter("").collect();
    /// assert_eq!(matches, vec![3, 2, 1, 0]);
    ///
    /// let matches: Vec<usize> = b"".rfind_iter("").collect();
    /// assert_eq!(matches, vec![0]);
    /// ```
    #[inline]
    fn rfind_iter<'a, B: ?Sized + AsRef<[u8]>>(
        &'a self,
        needle: &'a B,
    ) -> FindReverse<'a> {
        FindReverse::new(self.as_bytes(), needle.as_ref())
    }

    /// Returns the index of the first occurrence of the given byte. If the
    /// byte does not occur in this byte string, then `None` is returned.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// assert_eq!(Some(10), b"foo bar baz".find_byte(b'z'));
    /// assert_eq!(None, b"foo bar baz".find_byte(b'y'));
    /// ```
    #[inline]
    fn find_byte(&self, byte: u8) -> Option<usize> {
        memchr(byte, self.as_bytes())
    }

    /// Returns the index of the last occurrence of the given byte. If the
    /// byte does not occur in this byte string, then `None` is returned.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// assert_eq!(Some(10), b"foo bar baz".rfind_byte(b'z'));
    /// assert_eq!(None, b"foo bar baz".rfind_byte(b'y'));
    /// ```
    #[inline]
    fn rfind_byte(&self, byte: u8) -> Option<usize> {
        memrchr(byte, self.as_bytes())
    }

    /// Returns the index of the first occurrence of the given codepoint.
    /// If the codepoint does not occur in this byte string, then `None` is
    /// returned.
    ///
    /// Note that if one searches for the replacement codepoint, `\u{FFFD}`,
    /// then only explicit occurrences of that encoding will be found. Invalid
    /// UTF-8 sequences will not be matched.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// assert_eq!(Some(10), b"foo bar baz".find_char('z'));
    /// assert_eq!(Some(4), B("αβγγδ").find_char('γ'));
    /// assert_eq!(None, b"foo bar baz".find_char('y'));
    /// ```
    #[inline]
    fn find_char(&self, ch: char) -> Option<usize> {
        self.find(ch.encode_utf8(&mut [0; 4]))
    }

    /// Returns the index of the last occurrence of the given codepoint.
    /// If the codepoint does not occur in this byte string, then `None` is
    /// returned.
    ///
    /// Note that if one searches for the replacement codepoint, `\u{FFFD}`,
    /// then only explicit occurrences of that encoding will be found. Invalid
    /// UTF-8 sequences will not be matched.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// assert_eq!(Some(10), b"foo bar baz".rfind_char('z'));
    /// assert_eq!(Some(6), B("αβγγδ").rfind_char('γ'));
    /// assert_eq!(None, b"foo bar baz".rfind_char('y'));
    /// ```
    #[inline]
    fn rfind_char(&self, ch: char) -> Option<usize> {
        self.rfind(ch.encode_utf8(&mut [0; 4]))
    }

    /// Returns the index of the first occurrence of any of the bytes in the
    /// provided set.
    ///
    /// The `byteset` may be any type that can be cheaply converted into a
    /// `&[u8]`. This includes, but is not limited to, `&str` and `&[u8]`, but
    /// note that passing a `&str` which contains multibyte characters may not
    /// behave as you expect: each byte in the `&str` is treated as an
    /// individual member of the byte set.
    ///
    /// Note that order is irrelevant for the `byteset` parameter, and
    /// duplicate bytes present in its body are ignored.
    ///
    /// # Complexity
    ///
    /// This routine is guaranteed to have worst case linear time complexity
    /// with respect to both the set of bytes and the haystack. That is, this
    /// runs in `O(byteset.len() + haystack.len())` time.
    ///
    /// This routine is also guaranteed to have worst case constant space
    /// complexity.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// assert_eq!(b"foo bar baz".find_byteset(b"zr"), Some(6));
    /// assert_eq!(b"foo baz bar".find_byteset(b"bzr"), Some(4));
    /// assert_eq!(None, b"foo baz bar".find_byteset(b"\t\n"));
    /// ```
    #[inline]
    fn find_byteset<B: AsRef<[u8]>>(&self, byteset: B) -> Option<usize> {
        byteset::find(self.as_bytes(), byteset.as_ref())
    }

    /// Returns the index of the first occurrence of a byte that is not a member
    /// of the provided set.
    ///
    /// The `byteset` may be any type that can be cheaply converted into a
    /// `&[u8]`. This includes, but is not limited to, `&str` and `&[u8]`, but
    /// note that passing a `&str` which contains multibyte characters may not
    /// behave as you expect: each byte in the `&str` is treated as an
    /// individual member of the byte set.
    ///
    /// Note that order is irrelevant for the `byteset` parameter, and
    /// duplicate bytes present in its body are ignored.
    ///
    /// # Complexity
    ///
    /// This routine is guaranteed to have worst case linear time complexity
    /// with respect to both the set of bytes and the haystack. That is, this
    /// runs in `O(byteset.len() + haystack.len())` time.
    ///
    /// This routine is also guaranteed to have worst case constant space
    /// complexity.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// assert_eq!(b"foo bar baz".find_not_byteset(b"fo "), Some(4));
    /// assert_eq!(b"\t\tbaz bar".find_not_byteset(b" \t\r\n"), Some(2));
    /// assert_eq!(b"foo\nbaz\tbar".find_not_byteset(b"\t\n"), Some(0));
    /// ```
    #[inline]
    fn find_not_byteset<B: AsRef<[u8]>>(&self, byteset: B) -> Option<usize> {
        byteset::find_not(self.as_bytes(), byteset.as_ref())
    }

    /// Returns the index of the last occurrence of any of the bytes in the
    /// provided set.
    ///
    /// The `byteset` may be any type that can be cheaply converted into a
    /// `&[u8]`. This includes, but is not limited to, `&str` and `&[u8]`, but
    /// note that passing a `&str` which contains multibyte characters may not
    /// behave as you expect: each byte in the `&str` is treated as an
    /// individual member of the byte set.
    ///
    /// Note that order is irrelevant for the `byteset` parameter, and duplicate
    /// bytes present in its body are ignored.
    ///
    /// # Complexity
    ///
    /// This routine is guaranteed to have worst case linear time complexity
    /// with respect to both the set of bytes and the haystack. That is, this
    /// runs in `O(byteset.len() + haystack.len())` time.
    ///
    /// This routine is also guaranteed to have worst case constant space
    /// complexity.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// assert_eq!(b"foo bar baz".rfind_byteset(b"agb"), Some(9));
    /// assert_eq!(b"foo baz bar".rfind_byteset(b"rabz "), Some(10));
    /// assert_eq!(b"foo baz bar".rfind_byteset(b"\n123"), None);
    /// ```
    #[inline]
    fn rfind_byteset<B: AsRef<[u8]>>(&self, byteset: B) -> Option<usize> {
        byteset::rfind(self.as_bytes(), byteset.as_ref())
    }

    /// Returns the index of the last occurrence of a byte that is not a member
    /// of the provided set.
    ///
    /// The `byteset` may be any type that can be cheaply converted into a
    /// `&[u8]`. This includes, but is not limited to, `&str` and `&[u8]`, but
    /// note that passing a `&str` which contains multibyte characters may not
    /// behave as you expect: each byte in the `&str` is treated as an
    /// individual member of the byte set.
    ///
    /// Note that order is irrelevant for the `byteset` parameter, and
    /// duplicate bytes present in its body are ignored.
    ///
    /// # Complexity
    ///
    /// This routine is guaranteed to have worst case linear time complexity
    /// with respect to both the set of bytes and the haystack. That is, this
    /// runs in `O(byteset.len() + haystack.len())` time.
    ///
    /// This routine is also guaranteed to have worst case constant space
    /// complexity.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// assert_eq!(b"foo bar baz,\t".rfind_not_byteset(b",\t"), Some(10));
    /// assert_eq!(b"foo baz bar".rfind_not_byteset(b"rabz "), Some(2));
    /// assert_eq!(None, b"foo baz bar".rfind_not_byteset(b"barfoz "));
    /// ```
    #[inline]
    fn rfind_not_byteset<B: AsRef<[u8]>>(&self, byteset: B) -> Option<usize> {
        byteset::rfind_not(self.as_bytes(), byteset.as_ref())
    }

    /// Returns an iterator over the fields in a byte string, separated by
    /// contiguous whitespace.
    ///
    /// # Example
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B("  foo\tbar\t\u{2003}\nquux   \n");
    /// let fields: Vec<&[u8]> = s.fields().collect();
    /// assert_eq!(fields, vec![B("foo"), B("bar"), B("quux")]);
    /// ```
    ///
    /// A byte string consisting of just whitespace yields no elements:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// assert_eq!(0, B("  \n\t\u{2003}\n  \t").fields().count());
    /// ```
    #[inline]
    fn fields(&self) -> Fields {
        Fields::new(self.as_bytes())
    }

    /// Returns an iterator over the fields in a byte string, separated by
    /// contiguous codepoints satisfying the given predicate.
    ///
    /// If this byte string is not valid UTF-8, then the given closure will
    /// be called with a Unicode replacement codepoint when invalid UTF-8
    /// bytes are seen.
    ///
    /// # Example
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = b"123foo999999bar1quux123456";
    /// let fields: Vec<&[u8]> = s.fields_with(|c| c.is_numeric()).collect();
    /// assert_eq!(fields, vec![B("foo"), B("bar"), B("quux")]);
    /// ```
    ///
    /// A byte string consisting of all codepoints satisfying the predicate
    /// yields no elements:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// assert_eq!(0, b"1911354563".fields_with(|c| c.is_numeric()).count());
    /// ```
    #[inline]
    fn fields_with<F: FnMut(char) -> bool>(&self, f: F) -> FieldsWith<F> {
        FieldsWith::new(self.as_bytes(), f)
    }

    /// Returns an iterator over substrings of this byte string, separated
    /// by the given byte string. Each element yielded is guaranteed not to
    /// include the splitter substring.
    ///
    /// The splitter may be any type that can be cheaply converted into a
    /// `&[u8]`. This includes, but is not limited to, `&str` and `&[u8]`.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let x: Vec<&[u8]> = b"Mary had a little lamb".split_str(" ").collect();
    /// assert_eq!(x, vec![
    ///     B("Mary"), B("had"), B("a"), B("little"), B("lamb"),
    /// ]);
    ///
    /// let x: Vec<&[u8]> = b"".split_str("X").collect();
    /// assert_eq!(x, vec![b""]);
    ///
    /// let x: Vec<&[u8]> = b"lionXXtigerXleopard".split_str("X").collect();
    /// assert_eq!(x, vec![B("lion"), B(""), B("tiger"), B("leopard")]);
    ///
    /// let x: Vec<&[u8]> = b"lion::tiger::leopard".split_str("::").collect();
    /// assert_eq!(x, vec![B("lion"), B("tiger"), B("leopard")]);
    /// ```
    ///
    /// If a string contains multiple contiguous separators, you will end up
    /// with empty strings yielded by the iterator:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let x: Vec<&[u8]> = b"||||a||b|c".split_str("|").collect();
    /// assert_eq!(x, vec![
    ///     B(""), B(""), B(""), B(""), B("a"), B(""), B("b"), B("c"),
    /// ]);
    ///
    /// let x: Vec<&[u8]> = b"(///)".split_str("/").collect();
    /// assert_eq!(x, vec![B("("), B(""), B(""), B(")")]);
    /// ```
    ///
    /// Separators at the start or end of a string are neighbored by empty
    /// strings.
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let x: Vec<&[u8]> = b"010".split_str("0").collect();
    /// assert_eq!(x, vec![B(""), B("1"), B("")]);
    /// ```
    ///
    /// When the empty string is used as a separator, it splits every **byte**
    /// in the byte string, along with the beginning and end of the byte
    /// string.
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let x: Vec<&[u8]> = b"rust".split_str("").collect();
    /// assert_eq!(x, vec![
    ///     B(""), B("r"), B("u"), B("s"), B("t"), B(""),
    /// ]);
    ///
    /// // Splitting by an empty string is not UTF-8 aware. Elements yielded
    /// // may not be valid UTF-8!
    /// let x: Vec<&[u8]> = B("☃").split_str("").collect();
    /// assert_eq!(x, vec![
    ///     B(""), B(b"\xE2"), B(b"\x98"), B(b"\x83"), B(""),
    /// ]);
    /// ```
    ///
    /// Contiguous separators, especially whitespace, can lead to possibly
    /// surprising behavior. For example, this code is correct:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let x: Vec<&[u8]> = b"    a  b c".split_str(" ").collect();
    /// assert_eq!(x, vec![
    ///     B(""), B(""), B(""), B(""), B("a"), B(""), B("b"), B("c"),
    /// ]);
    /// ```
    ///
    /// It does *not* give you `["a", "b", "c"]`. For that behavior, use
    /// [`fields`](#method.fields) instead.
    #[inline]
    fn split_str<'a, B: ?Sized + AsRef<[u8]>>(
        &'a self,
        splitter: &'a B,
    ) -> Split<'a> {
        Split::new(self.as_bytes(), splitter.as_ref())
    }

    /// Returns an iterator over substrings of this byte string, separated by
    /// the given byte string, in reverse. Each element yielded is guaranteed
    /// not to include the splitter substring.
    ///
    /// The splitter may be any type that can be cheaply converted into a
    /// `&[u8]`. This includes, but is not limited to, `&str` and `&[u8]`.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let x: Vec<&[u8]> =
    ///     b"Mary had a little lamb".rsplit_str(" ").collect();
    /// assert_eq!(x, vec![
    ///     B("lamb"), B("little"), B("a"), B("had"), B("Mary"),
    /// ]);
    ///
    /// let x: Vec<&[u8]> = b"".rsplit_str("X").collect();
    /// assert_eq!(x, vec![b""]);
    ///
    /// let x: Vec<&[u8]> = b"lionXXtigerXleopard".rsplit_str("X").collect();
    /// assert_eq!(x, vec![B("leopard"), B("tiger"), B(""), B("lion")]);
    ///
    /// let x: Vec<&[u8]> = b"lion::tiger::leopard".rsplit_str("::").collect();
    /// assert_eq!(x, vec![B("leopard"), B("tiger"), B("lion")]);
    /// ```
    ///
    /// If a string contains multiple contiguous separators, you will end up
    /// with empty strings yielded by the iterator:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let x: Vec<&[u8]> = b"||||a||b|c".rsplit_str("|").collect();
    /// assert_eq!(x, vec![
    ///     B("c"), B("b"), B(""), B("a"), B(""), B(""), B(""), B(""),
    /// ]);
    ///
    /// let x: Vec<&[u8]> = b"(///)".rsplit_str("/").collect();
    /// assert_eq!(x, vec![B(")"), B(""), B(""), B("(")]);
    /// ```
    ///
    /// Separators at the start or end of a string are neighbored by empty
    /// strings.
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let x: Vec<&[u8]> = b"010".rsplit_str("0").collect();
    /// assert_eq!(x, vec![B(""), B("1"), B("")]);
    /// ```
    ///
    /// When the empty string is used as a separator, it splits every **byte**
    /// in the byte string, along with the beginning and end of the byte
    /// string.
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let x: Vec<&[u8]> = b"rust".rsplit_str("").collect();
    /// assert_eq!(x, vec![
    ///     B(""), B("t"), B("s"), B("u"), B("r"), B(""),
    /// ]);
    ///
    /// // Splitting by an empty string is not UTF-8 aware. Elements yielded
    /// // may not be valid UTF-8!
    /// let x: Vec<&[u8]> = B("☃").rsplit_str("").collect();
    /// assert_eq!(x, vec![B(""), B(b"\x83"), B(b"\x98"), B(b"\xE2"), B("")]);
    /// ```
    ///
    /// Contiguous separators, especially whitespace, can lead to possibly
    /// surprising behavior. For example, this code is correct:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let x: Vec<&[u8]> = b"    a  b c".rsplit_str(" ").collect();
    /// assert_eq!(x, vec![
    ///     B("c"), B("b"), B(""), B("a"), B(""), B(""), B(""), B(""),
    /// ]);
    /// ```
    ///
    /// It does *not* give you `["a", "b", "c"]`.
    #[inline]
    fn rsplit_str<'a, B: ?Sized + AsRef<[u8]>>(
        &'a self,
        splitter: &'a B,
    ) -> SplitReverse<'a> {
        SplitReverse::new(self.as_bytes(), splitter.as_ref())
    }

    /// Returns an iterator of at most `limit` substrings of this byte string,
    /// separated by the given byte string. If `limit` substrings are yielded,
    /// then the last substring will contain the remainder of this byte string.
    ///
    /// The needle may be any type that can be cheaply converted into a
    /// `&[u8]`. This includes, but is not limited to, `&str` and `&[u8]`.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let x: Vec<_> = b"Mary had a little lamb".splitn_str(3, " ").collect();
    /// assert_eq!(x, vec![B("Mary"), B("had"), B("a little lamb")]);
    ///
    /// let x: Vec<_> = b"".splitn_str(3, "X").collect();
    /// assert_eq!(x, vec![b""]);
    ///
    /// let x: Vec<_> = b"lionXXtigerXleopard".splitn_str(3, "X").collect();
    /// assert_eq!(x, vec![B("lion"), B(""), B("tigerXleopard")]);
    ///
    /// let x: Vec<_> = b"lion::tiger::leopard".splitn_str(2, "::").collect();
    /// assert_eq!(x, vec![B("lion"), B("tiger::leopard")]);
    ///
    /// let x: Vec<_> = b"abcXdef".splitn_str(1, "X").collect();
    /// assert_eq!(x, vec![B("abcXdef")]);
    ///
    /// let x: Vec<_> = b"abcdef".splitn_str(2, "X").collect();
    /// assert_eq!(x, vec![B("abcdef")]);
    ///
    /// let x: Vec<_> = b"abcXdef".splitn_str(0, "X").collect();
    /// assert!(x.is_empty());
    /// ```
    #[inline]
    fn splitn_str<'a, B: ?Sized + AsRef<[u8]>>(
        &'a self,
        limit: usize,
        splitter: &'a B,
    ) -> SplitN<'a> {
        SplitN::new(self.as_bytes(), splitter.as_ref(), limit)
    }

    /// Returns an iterator of at most `limit` substrings of this byte string,
    /// separated by the given byte string, in reverse. If `limit` substrings
    /// are yielded, then the last substring will contain the remainder of this
    /// byte string.
    ///
    /// The needle may be any type that can be cheaply converted into a
    /// `&[u8]`. This includes, but is not limited to, `&str` and `&[u8]`.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let x: Vec<_> =
    ///     b"Mary had a little lamb".rsplitn_str(3, " ").collect();
    /// assert_eq!(x, vec![B("lamb"), B("little"), B("Mary had a")]);
    ///
    /// let x: Vec<_> = b"".rsplitn_str(3, "X").collect();
    /// assert_eq!(x, vec![b""]);
    ///
    /// let x: Vec<_> = b"lionXXtigerXleopard".rsplitn_str(3, "X").collect();
    /// assert_eq!(x, vec![B("leopard"), B("tiger"), B("lionX")]);
    ///
    /// let x: Vec<_> = b"lion::tiger::leopard".rsplitn_str(2, "::").collect();
    /// assert_eq!(x, vec![B("leopard"), B("lion::tiger")]);
    ///
    /// let x: Vec<_> = b"abcXdef".rsplitn_str(1, "X").collect();
    /// assert_eq!(x, vec![B("abcXdef")]);
    ///
    /// let x: Vec<_> = b"abcdef".rsplitn_str(2, "X").collect();
    /// assert_eq!(x, vec![B("abcdef")]);
    ///
    /// let x: Vec<_> = b"abcXdef".rsplitn_str(0, "X").collect();
    /// assert!(x.is_empty());
    /// ```
    #[inline]
    fn rsplitn_str<'a, B: ?Sized + AsRef<[u8]>>(
        &'a self,
        limit: usize,
        splitter: &'a B,
    ) -> SplitNReverse<'a> {
        SplitNReverse::new(self.as_bytes(), splitter.as_ref(), limit)
    }

    /// Replace all matches of the given needle with the given replacement, and
    /// the result as a new `Vec<u8>`.
    ///
    /// This routine is useful as a convenience. If you need to reuse an
    /// allocation, use [`replace_into`](#method.replace_into) instead.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"this is old".replace("old", "new");
    /// assert_eq!(s, "this is new".as_bytes());
    /// ```
    ///
    /// When the pattern doesn't match:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"this is old".replace("nada nada", "limonada");
    /// assert_eq!(s, "this is old".as_bytes());
    /// ```
    ///
    /// When the needle is an empty string:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"foo".replace("", "Z");
    /// assert_eq!(s, "ZfZoZoZ".as_bytes());
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn replace<N: AsRef<[u8]>, R: AsRef<[u8]>>(
        &self,
        needle: N,
        replacement: R,
    ) -> Vec<u8> {
        let mut dest = Vec::with_capacity(self.as_bytes().len());
        self.replace_into(needle, replacement, &mut dest);
        dest
    }

    /// Replace up to `limit` matches of the given needle with the given
    /// replacement, and the result as a new `Vec<u8>`.
    ///
    /// This routine is useful as a convenience. If you need to reuse an
    /// allocation, use [`replacen_into`](#method.replacen_into) instead.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"foofoo".replacen("o", "z", 2);
    /// assert_eq!(s, "fzzfoo".as_bytes());
    /// ```
    ///
    /// When the pattern doesn't match:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"foofoo".replacen("a", "z", 2);
    /// assert_eq!(s, "foofoo".as_bytes());
    /// ```
    ///
    /// When the needle is an empty string:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"foo".replacen("", "Z", 2);
    /// assert_eq!(s, "ZfZoo".as_bytes());
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn replacen<N: AsRef<[u8]>, R: AsRef<[u8]>>(
        &self,
        needle: N,
        replacement: R,
        limit: usize,
    ) -> Vec<u8> {
        let mut dest = Vec::with_capacity(self.as_bytes().len());
        self.replacen_into(needle, replacement, limit, &mut dest);
        dest
    }

    /// Replace all matches of the given needle with the given replacement,
    /// and write the result into the provided `Vec<u8>`.
    ///
    /// This does **not** clear `dest` before writing to it.
    ///
    /// This routine is useful for reusing allocation. For a more convenient
    /// API, use [`replace`](#method.replace) instead.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"this is old";
    ///
    /// let mut dest = vec![];
    /// s.replace_into("old", "new", &mut dest);
    /// assert_eq!(dest, "this is new".as_bytes());
    /// ```
    ///
    /// When the pattern doesn't match:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"this is old";
    ///
    /// let mut dest = vec![];
    /// s.replace_into("nada nada", "limonada", &mut dest);
    /// assert_eq!(dest, "this is old".as_bytes());
    /// ```
    ///
    /// When the needle is an empty string:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"foo";
    ///
    /// let mut dest = vec![];
    /// s.replace_into("", "Z", &mut dest);
    /// assert_eq!(dest, "ZfZoZoZ".as_bytes());
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn replace_into<N: AsRef<[u8]>, R: AsRef<[u8]>>(
        &self,
        needle: N,
        replacement: R,
        dest: &mut Vec<u8>,
    ) {
        let (needle, replacement) = (needle.as_ref(), replacement.as_ref());

        let mut last = 0;
        for start in self.find_iter(needle) {
            dest.push_str(&self.as_bytes()[last..start]);
            dest.push_str(replacement);
            last = start + needle.len();
        }
        dest.push_str(&self.as_bytes()[last..]);
    }

    /// Replace up to `limit` matches of the given needle with the given
    /// replacement, and write the result into the provided `Vec<u8>`.
    ///
    /// This does **not** clear `dest` before writing to it.
    ///
    /// This routine is useful for reusing allocation. For a more convenient
    /// API, use [`replacen`](#method.replacen) instead.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"foofoo";
    ///
    /// let mut dest = vec![];
    /// s.replacen_into("o", "z", 2, &mut dest);
    /// assert_eq!(dest, "fzzfoo".as_bytes());
    /// ```
    ///
    /// When the pattern doesn't match:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"foofoo";
    ///
    /// let mut dest = vec![];
    /// s.replacen_into("a", "z", 2, &mut dest);
    /// assert_eq!(dest, "foofoo".as_bytes());
    /// ```
    ///
    /// When the needle is an empty string:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let s = b"foo";
    ///
    /// let mut dest = vec![];
    /// s.replacen_into("", "Z", 2, &mut dest);
    /// assert_eq!(dest, "ZfZoo".as_bytes());
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn replacen_into<N: AsRef<[u8]>, R: AsRef<[u8]>>(
        &self,
        needle: N,
        replacement: R,
        limit: usize,
        dest: &mut Vec<u8>,
    ) {
        let (needle, replacement) = (needle.as_ref(), replacement.as_ref());

        let mut last = 0;
        for start in self.find_iter(needle).take(limit) {
            dest.push_str(&self.as_bytes()[last..start]);
            dest.push_str(replacement);
            last = start + needle.len();
        }
        dest.push_str(&self.as_bytes()[last..]);
    }

    /// Returns an iterator over the bytes in this byte string.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = b"foobar";
    /// let bytes: Vec<u8> = bs.bytes().collect();
    /// assert_eq!(bytes, bs);
    /// ```
    #[inline]
    fn bytes(&self) -> Bytes {
        Bytes { it: self.as_bytes().iter() }
    }

    /// Returns an iterator over the Unicode scalar values in this byte string.
    /// If invalid UTF-8 is encountered, then the Unicode replacement codepoint
    /// is yielded instead.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = b"\xE2\x98\x83\xFF\xF0\x9D\x9E\x83\xE2\x98\x61";
    /// let chars: Vec<char> = bs.chars().collect();
    /// assert_eq!(vec!['☃', '\u{FFFD}', '𝞃', '\u{FFFD}', 'a'], chars);
    /// ```
    ///
    /// Codepoints can also be iterated over in reverse:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = b"\xE2\x98\x83\xFF\xF0\x9D\x9E\x83\xE2\x98\x61";
    /// let chars: Vec<char> = bs.chars().rev().collect();
    /// assert_eq!(vec!['a', '\u{FFFD}', '𝞃', '\u{FFFD}', '☃'], chars);
    /// ```
    #[inline]
    fn chars(&self) -> Chars {
        Chars::new(self.as_bytes())
    }

    /// Returns an iterator over the Unicode scalar values in this byte string
    /// along with their starting and ending byte index positions. If invalid
    /// UTF-8 is encountered, then the Unicode replacement codepoint is yielded
    /// instead.
    ///
    /// Note that this is slightly different from the `CharIndices` iterator
    /// provided by the standard library. Aside from working on possibly
    /// invalid UTF-8, this iterator provides both the corresponding starting
    /// and ending byte indices of each codepoint yielded. The ending position
    /// is necessary to slice the original byte string when invalid UTF-8 bytes
    /// are converted into a Unicode replacement codepoint, since a single
    /// replacement codepoint can substitute anywhere from 1 to 3 invalid bytes
    /// (inclusive).
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = b"\xE2\x98\x83\xFF\xF0\x9D\x9E\x83\xE2\x98\x61";
    /// let chars: Vec<(usize, usize, char)> = bs.char_indices().collect();
    /// assert_eq!(chars, vec![
    ///     (0, 3, '☃'),
    ///     (3, 4, '\u{FFFD}'),
    ///     (4, 8, '𝞃'),
    ///     (8, 10, '\u{FFFD}'),
    ///     (10, 11, 'a'),
    /// ]);
    /// ```
    ///
    /// Codepoints can also be iterated over in reverse:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = b"\xE2\x98\x83\xFF\xF0\x9D\x9E\x83\xE2\x98\x61";
    /// let chars: Vec<(usize, usize, char)> = bs
    ///     .char_indices()
    ///     .rev()
    ///     .collect();
    /// assert_eq!(chars, vec![
    ///     (10, 11, 'a'),
    ///     (8, 10, '\u{FFFD}'),
    ///     (4, 8, '𝞃'),
    ///     (3, 4, '\u{FFFD}'),
    ///     (0, 3, '☃'),
    /// ]);
    /// ```
    #[inline]
    fn char_indices(&self) -> CharIndices {
        CharIndices::new(self.as_bytes())
    }

    /// Iterate over chunks of valid UTF-8.
    ///
    /// The iterator returned yields chunks of valid UTF-8 separated by invalid
    /// UTF-8 bytes, if they exist. Invalid UTF-8 bytes are always 1-3 bytes,
    /// which are determined via the "substitution of maximal subparts"
    /// strategy described in the docs for the
    /// [`ByteSlice::to_str_lossy`](trait.ByteSlice.html#method.to_str_lossy)
    /// method.
    ///
    /// # Examples
    ///
    /// This example shows how to gather all valid and invalid chunks from a
    /// byte slice:
    ///
    /// ```
    /// use bstr::{ByteSlice, Utf8Chunk};
    ///
    /// let bytes = b"foo\xFD\xFEbar\xFF";
    ///
    /// let (mut valid_chunks, mut invalid_chunks) = (vec![], vec![]);
    /// for chunk in bytes.utf8_chunks() {
    ///     if !chunk.valid().is_empty() {
    ///         valid_chunks.push(chunk.valid());
    ///     }
    ///     if !chunk.invalid().is_empty() {
    ///         invalid_chunks.push(chunk.invalid());
    ///     }
    /// }
    ///
    /// assert_eq!(valid_chunks, vec!["foo", "bar"]);
    /// assert_eq!(invalid_chunks, vec![b"\xFD", b"\xFE", b"\xFF"]);
    /// ```
    #[inline]
    fn utf8_chunks(&self) -> Utf8Chunks {
        Utf8Chunks { bytes: self.as_bytes() }
    }

    /// Returns an iterator over the grapheme clusters in this byte string.
    /// If invalid UTF-8 is encountered, then the Unicode replacement codepoint
    /// is yielded instead.
    ///
    /// # Examples
    ///
    /// This example shows how multiple codepoints can combine to form a
    /// single grapheme cluster:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = "a\u{0300}\u{0316}\u{1F1FA}\u{1F1F8}".as_bytes();
    /// let graphemes: Vec<&str> = bs.graphemes().collect();
    /// assert_eq!(vec!["à̖", "🇺🇸"], graphemes);
    /// ```
    ///
    /// This shows that graphemes can be iterated over in reverse:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = "a\u{0300}\u{0316}\u{1F1FA}\u{1F1F8}".as_bytes();
    /// let graphemes: Vec<&str> = bs.graphemes().rev().collect();
    /// assert_eq!(vec!["🇺🇸", "à̖"], graphemes);
    /// ```
    #[cfg(feature = "unicode")]
    #[inline]
    fn graphemes(&self) -> Graphemes {
        Graphemes::new(self.as_bytes())
    }

    /// Returns an iterator over the grapheme clusters in this byte string
    /// along with their starting and ending byte index positions. If invalid
    /// UTF-8 is encountered, then the Unicode replacement codepoint is yielded
    /// instead.
    ///
    /// # Examples
    ///
    /// This example shows how to get the byte offsets of each individual
    /// grapheme cluster:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = "a\u{0300}\u{0316}\u{1F1FA}\u{1F1F8}".as_bytes();
    /// let graphemes: Vec<(usize, usize, &str)> =
    ///     bs.grapheme_indices().collect();
    /// assert_eq!(vec![(0, 5, "à̖"), (5, 13, "🇺🇸")], graphemes);
    /// ```
    ///
    /// This example shows what happens when invalid UTF-8 is enountered. Note
    /// that the offsets are valid indices into the original string, and do
    /// not necessarily correspond to the length of the `&str` returned!
    ///
    /// ```
    /// use bstr::{ByteSlice, ByteVec};
    ///
    /// let mut bytes = vec![];
    /// bytes.push_str("a\u{0300}\u{0316}");
    /// bytes.push(b'\xFF');
    /// bytes.push_str("\u{1F1FA}\u{1F1F8}");
    ///
    /// let graphemes: Vec<(usize, usize, &str)> =
    ///     bytes.grapheme_indices().collect();
    /// assert_eq!(
    ///     graphemes,
    ///     vec![(0, 5, "à̖"), (5, 6, "\u{FFFD}"), (6, 14, "🇺🇸")]
    /// );
    /// ```
    #[cfg(feature = "unicode")]
    #[inline]
    fn grapheme_indices(&self) -> GraphemeIndices {
        GraphemeIndices::new(self.as_bytes())
    }

    /// Returns an iterator over the words in this byte string. If invalid
    /// UTF-8 is encountered, then the Unicode replacement codepoint is yielded
    /// instead.
    ///
    /// This is similar to
    /// [`words_with_breaks`](trait.ByteSlice.html#method.words_with_breaks),
    /// except it only returns elements that contain a "word" character. A word
    /// character is defined by UTS #18 (Annex C) to be the combination of the
    /// `Alphabetic` and `Join_Control` properties, along with the
    /// `Decimal_Number`, `Mark` and `Connector_Punctuation` general
    /// categories.
    ///
    /// Since words are made up of one or more codepoints, this iterator
    /// yields `&str` elements. When invalid UTF-8 is encountered, replacement
    /// codepoints are [substituted](index.html#handling-of-invalid-utf-8).
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = br#"The quick ("brown") fox can't jump 32.3 feet, right?"#;
    /// let words: Vec<&str> = bs.words().collect();
    /// assert_eq!(words, vec![
    ///     "The", "quick", "brown", "fox", "can't",
    ///     "jump", "32.3", "feet", "right",
    /// ]);
    /// ```
    #[cfg(feature = "unicode")]
    #[inline]
    fn words(&self) -> Words {
        Words::new(self.as_bytes())
    }

    /// Returns an iterator over the words in this byte string along with
    /// their starting and ending byte index positions.
    ///
    /// This is similar to
    /// [`words_with_break_indices`](trait.ByteSlice.html#method.words_with_break_indices),
    /// except it only returns elements that contain a "word" character. A word
    /// character is defined by UTS #18 (Annex C) to be the combination of the
    /// `Alphabetic` and `Join_Control` properties, along with the
    /// `Decimal_Number`, `Mark` and `Connector_Punctuation` general
    /// categories.
    ///
    /// Since words are made up of one or more codepoints, this iterator
    /// yields `&str` elements. When invalid UTF-8 is encountered, replacement
    /// codepoints are [substituted](index.html#handling-of-invalid-utf-8).
    ///
    /// # Examples
    ///
    /// This example shows how to get the byte offsets of each individual
    /// word:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = b"can't jump 32.3 feet";
    /// let words: Vec<(usize, usize, &str)> = bs.word_indices().collect();
    /// assert_eq!(words, vec![
    ///     (0, 5, "can't"),
    ///     (6, 10, "jump"),
    ///     (11, 15, "32.3"),
    ///     (16, 20, "feet"),
    /// ]);
    /// ```
    #[cfg(feature = "unicode")]
    #[inline]
    fn word_indices(&self) -> WordIndices {
        WordIndices::new(self.as_bytes())
    }

    /// Returns an iterator over the words in this byte string, along with
    /// all breaks between the words. Concatenating all elements yielded by
    /// the iterator results in the original string (modulo Unicode replacement
    /// codepoint substitutions if invalid UTF-8 is encountered).
    ///
    /// Since words are made up of one or more codepoints, this iterator
    /// yields `&str` elements. When invalid UTF-8 is encountered, replacement
    /// codepoints are [substituted](index.html#handling-of-invalid-utf-8).
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = br#"The quick ("brown") fox can't jump 32.3 feet, right?"#;
    /// let words: Vec<&str> = bs.words_with_breaks().collect();
    /// assert_eq!(words, vec![
    ///     "The", " ", "quick", " ", "(", "\"", "brown", "\"", ")",
    ///     " ", "fox", " ", "can't", " ", "jump", " ", "32.3", " ", "feet",
    ///     ",", " ", "right", "?",
    /// ]);
    /// ```
    #[cfg(feature = "unicode")]
    #[inline]
    fn words_with_breaks(&self) -> WordsWithBreaks {
        WordsWithBreaks::new(self.as_bytes())
    }

    /// Returns an iterator over the words and their byte offsets in this
    /// byte string, along with all breaks between the words. Concatenating
    /// all elements yielded by the iterator results in the original string
    /// (modulo Unicode replacement codepoint substitutions if invalid UTF-8 is
    /// encountered).
    ///
    /// Since words are made up of one or more codepoints, this iterator
    /// yields `&str` elements. When invalid UTF-8 is encountered, replacement
    /// codepoints are [substituted](index.html#handling-of-invalid-utf-8).
    ///
    /// # Examples
    ///
    /// This example shows how to get the byte offsets of each individual
    /// word:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = b"can't jump 32.3 feet";
    /// let words: Vec<(usize, usize, &str)> =
    ///     bs.words_with_break_indices().collect();
    /// assert_eq!(words, vec![
    ///     (0, 5, "can't"),
    ///     (5, 6, " "),
    ///     (6, 10, "jump"),
    ///     (10, 11, " "),
    ///     (11, 15, "32.3"),
    ///     (15, 16, " "),
    ///     (16, 20, "feet"),
    /// ]);
    /// ```
    #[cfg(feature = "unicode")]
    #[inline]
    fn words_with_break_indices(&self) -> WordsWithBreakIndices {
        WordsWithBreakIndices::new(self.as_bytes())
    }

    /// Returns an iterator over the sentences in this byte string.
    ///
    /// Typically, a sentence will include its trailing punctuation and
    /// whitespace. Concatenating all elements yielded by the iterator
    /// results in the original string (modulo Unicode replacement codepoint
    /// substitutions if invalid UTF-8 is encountered).
    ///
    /// Since sentences are made up of one or more codepoints, this iterator
    /// yields `&str` elements. When invalid UTF-8 is encountered, replacement
    /// codepoints are [substituted](index.html#handling-of-invalid-utf-8).
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = b"I want this. Not that. Right now.";
    /// let sentences: Vec<&str> = bs.sentences().collect();
    /// assert_eq!(sentences, vec![
    ///     "I want this. ",
    ///     "Not that. ",
    ///     "Right now.",
    /// ]);
    /// ```
    #[cfg(feature = "unicode")]
    #[inline]
    fn sentences(&self) -> Sentences {
        Sentences::new(self.as_bytes())
    }

    /// Returns an iterator over the sentences in this byte string along with
    /// their starting and ending byte index positions.
    ///
    /// Typically, a sentence will include its trailing punctuation and
    /// whitespace. Concatenating all elements yielded by the iterator
    /// results in the original string (modulo Unicode replacement codepoint
    /// substitutions if invalid UTF-8 is encountered).
    ///
    /// Since sentences are made up of one or more codepoints, this iterator
    /// yields `&str` elements. When invalid UTF-8 is encountered, replacement
    /// codepoints are [substituted](index.html#handling-of-invalid-utf-8).
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let bs = b"I want this. Not that. Right now.";
    /// let sentences: Vec<(usize, usize, &str)> =
    ///     bs.sentence_indices().collect();
    /// assert_eq!(sentences, vec![
    ///     (0, 13, "I want this. "),
    ///     (13, 23, "Not that. "),
    ///     (23, 33, "Right now."),
    /// ]);
    /// ```
    #[cfg(feature = "unicode")]
    #[inline]
    fn sentence_indices(&self) -> SentenceIndices {
        SentenceIndices::new(self.as_bytes())
    }

    /// An iterator over all lines in a byte string, without their
    /// terminators.
    ///
    /// For this iterator, the only line terminators recognized are `\r\n` and
    /// `\n`.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = b"\
    /// foo
    ///
    /// bar\r
    /// baz
    ///
    ///
    /// quux";
    /// let lines: Vec<&[u8]> = s.lines().collect();
    /// assert_eq!(lines, vec![
    ///     B("foo"), B(""), B("bar"), B("baz"), B(""), B(""), B("quux"),
    /// ]);
    /// ```
    #[inline]
    fn lines(&self) -> Lines {
        Lines::new(self.as_bytes())
    }

    /// An iterator over all lines in a byte string, including their
    /// terminators.
    ///
    /// For this iterator, the only line terminator recognized is `\n`. (Since
    /// line terminators are included, this also handles `\r\n` line endings.)
    ///
    /// Line terminators are only included if they are present in the original
    /// byte string. For example, the last line in a byte string may not end
    /// with a line terminator.
    ///
    /// Concatenating all elements yielded by this iterator is guaranteed to
    /// yield the original byte string.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = b"\
    /// foo
    ///
    /// bar\r
    /// baz
    ///
    ///
    /// quux";
    /// let lines: Vec<&[u8]> = s.lines_with_terminator().collect();
    /// assert_eq!(lines, vec![
    ///     B("foo\n"),
    ///     B("\n"),
    ///     B("bar\r\n"),
    ///     B("baz\n"),
    ///     B("\n"),
    ///     B("\n"),
    ///     B("quux"),
    /// ]);
    /// ```
    #[inline]
    fn lines_with_terminator(&self) -> LinesWithTerminator {
        LinesWithTerminator::new(self.as_bytes())
    }

    /// Return a byte string slice with leading and trailing whitespace
    /// removed.
    ///
    /// Whitespace is defined according to the terms of the `White_Space`
    /// Unicode property.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B(" foo\tbar\t\u{2003}\n");
    /// assert_eq!(s.trim(), B("foo\tbar"));
    /// ```
    #[cfg(feature = "unicode")]
    #[inline]
    fn trim(&self) -> &[u8] {
        self.trim_start().trim_end()
    }

    /// Return a byte string slice with leading whitespace removed.
    ///
    /// Whitespace is defined according to the terms of the `White_Space`
    /// Unicode property.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B(" foo\tbar\t\u{2003}\n");
    /// assert_eq!(s.trim_start(), B("foo\tbar\t\u{2003}\n"));
    /// ```
    #[cfg(feature = "unicode")]
    #[inline]
    fn trim_start(&self) -> &[u8] {
        let start = whitespace_len_fwd(self.as_bytes());
        &self.as_bytes()[start..]
    }

    /// Return a byte string slice with trailing whitespace removed.
    ///
    /// Whitespace is defined according to the terms of the `White_Space`
    /// Unicode property.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B(" foo\tbar\t\u{2003}\n");
    /// assert_eq!(s.trim_end(), B(" foo\tbar"));
    /// ```
    #[cfg(feature = "unicode")]
    #[inline]
    fn trim_end(&self) -> &[u8] {
        let end = whitespace_len_rev(self.as_bytes());
        &self.as_bytes()[..end]
    }

    /// Return a byte string slice with leading and trailing characters
    /// satisfying the given predicate removed.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = b"123foo5bar789";
    /// assert_eq!(s.trim_with(|c| c.is_numeric()), B("foo5bar"));
    /// ```
    #[inline]
    fn trim_with<F: FnMut(char) -> bool>(&self, mut trim: F) -> &[u8] {
        self.trim_start_with(&mut trim).trim_end_with(&mut trim)
    }

    /// Return a byte string slice with leading characters satisfying the given
    /// predicate removed.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = b"123foo5bar789";
    /// assert_eq!(s.trim_start_with(|c| c.is_numeric()), B("foo5bar789"));
    /// ```
    #[inline]
    fn trim_start_with<F: FnMut(char) -> bool>(&self, mut trim: F) -> &[u8] {
        for (s, _, ch) in self.char_indices() {
            if !trim(ch) {
                return &self.as_bytes()[s..];
            }
        }
        b""
    }

    /// Return a byte string slice with trailing characters satisfying the
    /// given predicate removed.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = b"123foo5bar789";
    /// assert_eq!(s.trim_end_with(|c| c.is_numeric()), B("123foo5bar"));
    /// ```
    #[inline]
    fn trim_end_with<F: FnMut(char) -> bool>(&self, mut trim: F) -> &[u8] {
        for (_, e, ch) in self.char_indices().rev() {
            if !trim(ch) {
                return &self.as_bytes()[..e];
            }
        }
        b""
    }

    /// Returns a new `Vec<u8>` containing the lowercase equivalent of this
    /// byte string.
    ///
    /// In this case, lowercase is defined according to the `Lowercase` Unicode
    /// property.
    ///
    /// If invalid UTF-8 is seen, or if a character has no lowercase variant,
    /// then it is written to the given buffer unchanged.
    ///
    /// Note that some characters in this byte string may expand into multiple
    /// characters when changing the case, so the number of bytes written to
    /// the given byte string may not be equivalent to the number of bytes in
    /// this byte string.
    ///
    /// If you'd like to reuse an allocation for performance reasons, then use
    /// [`to_lowercase_into`](#method.to_lowercase_into) instead.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B("HELLO Β");
    /// assert_eq!("hello β".as_bytes(), s.to_lowercase().as_bytes());
    /// ```
    ///
    /// Scripts without case are not changed:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B("农历新年");
    /// assert_eq!("农历新年".as_bytes(), s.to_lowercase().as_bytes());
    /// ```
    ///
    /// Invalid UTF-8 remains as is:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B(b"FOO\xFFBAR\xE2\x98BAZ");
    /// assert_eq!(B(b"foo\xFFbar\xE2\x98baz"), s.to_lowercase().as_bytes());
    /// ```
    #[cfg(all(feature = "std", feature = "unicode"))]
    #[inline]
    fn to_lowercase(&self) -> Vec<u8> {
        let mut buf = vec![];
        self.to_lowercase_into(&mut buf);
        buf
    }

    /// Writes the lowercase equivalent of this byte string into the given
    /// buffer. The buffer is not cleared before written to.
    ///
    /// In this case, lowercase is defined according to the `Lowercase`
    /// Unicode property.
    ///
    /// If invalid UTF-8 is seen, or if a character has no lowercase variant,
    /// then it is written to the given buffer unchanged.
    ///
    /// Note that some characters in this byte string may expand into multiple
    /// characters when changing the case, so the number of bytes written to
    /// the given byte string may not be equivalent to the number of bytes in
    /// this byte string.
    ///
    /// If you don't need to amortize allocation and instead prefer
    /// convenience, then use [`to_lowercase`](#method.to_lowercase) instead.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B("HELLO Β");
    ///
    /// let mut buf = vec![];
    /// s.to_lowercase_into(&mut buf);
    /// assert_eq!("hello β".as_bytes(), buf.as_bytes());
    /// ```
    ///
    /// Scripts without case are not changed:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B("农历新年");
    ///
    /// let mut buf = vec![];
    /// s.to_lowercase_into(&mut buf);
    /// assert_eq!("农历新年".as_bytes(), buf.as_bytes());
    /// ```
    ///
    /// Invalid UTF-8 remains as is:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B(b"FOO\xFFBAR\xE2\x98BAZ");
    ///
    /// let mut buf = vec![];
    /// s.to_lowercase_into(&mut buf);
    /// assert_eq!(B(b"foo\xFFbar\xE2\x98baz"), buf.as_bytes());
    /// ```
    #[cfg(all(feature = "std", feature = "unicode"))]
    #[inline]
    fn to_lowercase_into(&self, buf: &mut Vec<u8>) {
        // TODO: This is the best we can do given what std exposes I think.
        // If we roll our own case handling, then we might be able to do this
        // a bit faster. We shouldn't roll our own case handling unless we
        // need to, e.g., for doing caseless matching or case folding.

        // TODO(BUG): This doesn't handle any special casing rules.

        buf.reserve(self.as_bytes().len());
        for (s, e, ch) in self.char_indices() {
            if ch == '\u{FFFD}' {
                buf.push_str(&self.as_bytes()[s..e]);
            } else if ch.is_ascii() {
                buf.push_char(ch.to_ascii_lowercase());
            } else {
                for upper in ch.to_lowercase() {
                    buf.push_char(upper);
                }
            }
        }
    }

    /// Returns a new `Vec<u8>` containing the ASCII lowercase equivalent of
    /// this byte string.
    ///
    /// In this case, lowercase is only defined in ASCII letters. Namely, the
    /// letters `A-Z` are converted to `a-z`. All other bytes remain unchanged.
    /// In particular, the length of the byte string returned is always
    /// equivalent to the length of this byte string.
    ///
    /// If you'd like to reuse an allocation for performance reasons, then use
    /// [`make_ascii_lowercase`](#method.make_ascii_lowercase) to perform
    /// the conversion in place.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B("HELLO Β");
    /// assert_eq!("hello Β".as_bytes(), s.to_ascii_lowercase().as_bytes());
    /// ```
    ///
    /// Invalid UTF-8 remains as is:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B(b"FOO\xFFBAR\xE2\x98BAZ");
    /// assert_eq!(s.to_ascii_lowercase(), B(b"foo\xFFbar\xE2\x98baz"));
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn to_ascii_lowercase(&self) -> Vec<u8> {
        self.as_bytes().to_ascii_lowercase()
    }

    /// Convert this byte string to its lowercase ASCII equivalent in place.
    ///
    /// In this case, lowercase is only defined in ASCII letters. Namely, the
    /// letters `A-Z` are converted to `a-z`. All other bytes remain unchanged.
    ///
    /// If you don't need to do the conversion in
    /// place and instead prefer convenience, then use
    /// [`to_ascii_lowercase`](#method.to_ascii_lowercase) instead.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let mut s = <Vec<u8>>::from("HELLO Β");
    /// s.make_ascii_lowercase();
    /// assert_eq!(s, "hello Β".as_bytes());
    /// ```
    ///
    /// Invalid UTF-8 remains as is:
    ///
    /// ```
    /// use bstr::{B, ByteSlice, ByteVec};
    ///
    /// let mut s = <Vec<u8>>::from_slice(b"FOO\xFFBAR\xE2\x98BAZ");
    /// s.make_ascii_lowercase();
    /// assert_eq!(s, B(b"foo\xFFbar\xE2\x98baz"));
    /// ```
    #[inline]
    fn make_ascii_lowercase(&mut self) {
        self.as_bytes_mut().make_ascii_lowercase();
    }

    /// Returns a new `Vec<u8>` containing the uppercase equivalent of this
    /// byte string.
    ///
    /// In this case, uppercase is defined according to the `Uppercase`
    /// Unicode property.
    ///
    /// If invalid UTF-8 is seen, or if a character has no uppercase variant,
    /// then it is written to the given buffer unchanged.
    ///
    /// Note that some characters in this byte string may expand into multiple
    /// characters when changing the case, so the number of bytes written to
    /// the given byte string may not be equivalent to the number of bytes in
    /// this byte string.
    ///
    /// If you'd like to reuse an allocation for performance reasons, then use
    /// [`to_uppercase_into`](#method.to_uppercase_into) instead.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B("hello β");
    /// assert_eq!(s.to_uppercase(), B("HELLO Β"));
    /// ```
    ///
    /// Scripts without case are not changed:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B("农历新年");
    /// assert_eq!(s.to_uppercase(), B("农历新年"));
    /// ```
    ///
    /// Invalid UTF-8 remains as is:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B(b"foo\xFFbar\xE2\x98baz");
    /// assert_eq!(s.to_uppercase(), B(b"FOO\xFFBAR\xE2\x98BAZ"));
    /// ```
    #[cfg(all(feature = "std", feature = "unicode"))]
    #[inline]
    fn to_uppercase(&self) -> Vec<u8> {
        let mut buf = vec![];
        self.to_uppercase_into(&mut buf);
        buf
    }

    /// Writes the uppercase equivalent of this byte string into the given
    /// buffer. The buffer is not cleared before written to.
    ///
    /// In this case, uppercase is defined according to the `Uppercase`
    /// Unicode property.
    ///
    /// If invalid UTF-8 is seen, or if a character has no uppercase variant,
    /// then it is written to the given buffer unchanged.
    ///
    /// Note that some characters in this byte string may expand into multiple
    /// characters when changing the case, so the number of bytes written to
    /// the given byte string may not be equivalent to the number of bytes in
    /// this byte string.
    ///
    /// If you don't need to amortize allocation and instead prefer
    /// convenience, then use [`to_uppercase`](#method.to_uppercase) instead.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B("hello β");
    ///
    /// let mut buf = vec![];
    /// s.to_uppercase_into(&mut buf);
    /// assert_eq!(buf, B("HELLO Β"));
    /// ```
    ///
    /// Scripts without case are not changed:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B("农历新年");
    ///
    /// let mut buf = vec![];
    /// s.to_uppercase_into(&mut buf);
    /// assert_eq!(buf, B("农历新年"));
    /// ```
    ///
    /// Invalid UTF-8 remains as is:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B(b"foo\xFFbar\xE2\x98baz");
    ///
    /// let mut buf = vec![];
    /// s.to_uppercase_into(&mut buf);
    /// assert_eq!(buf, B(b"FOO\xFFBAR\xE2\x98BAZ"));
    /// ```
    #[cfg(all(feature = "std", feature = "unicode"))]
    #[inline]
    fn to_uppercase_into(&self, buf: &mut Vec<u8>) {
        // TODO: This is the best we can do given what std exposes I think.
        // If we roll our own case handling, then we might be able to do this
        // a bit faster. We shouldn't roll our own case handling unless we
        // need to, e.g., for doing caseless matching or case folding.
        buf.reserve(self.as_bytes().len());
        for (s, e, ch) in self.char_indices() {
            if ch == '\u{FFFD}' {
                buf.push_str(&self.as_bytes()[s..e]);
            } else if ch.is_ascii() {
                buf.push_char(ch.to_ascii_uppercase());
            } else {
                for upper in ch.to_uppercase() {
                    buf.push_char(upper);
                }
            }
        }
    }

    /// Returns a new `Vec<u8>` containing the ASCII uppercase equivalent of
    /// this byte string.
    ///
    /// In this case, uppercase is only defined in ASCII letters. Namely, the
    /// letters `a-z` are converted to `A-Z`. All other bytes remain unchanged.
    /// In particular, the length of the byte string returned is always
    /// equivalent to the length of this byte string.
    ///
    /// If you'd like to reuse an allocation for performance reasons, then use
    /// [`make_ascii_uppercase`](#method.make_ascii_uppercase) to perform
    /// the conversion in place.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B("hello β");
    /// assert_eq!(s.to_ascii_uppercase(), B("HELLO β"));
    /// ```
    ///
    /// Invalid UTF-8 remains as is:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let s = B(b"foo\xFFbar\xE2\x98baz");
    /// assert_eq!(s.to_ascii_uppercase(), B(b"FOO\xFFBAR\xE2\x98BAZ"));
    /// ```
    #[cfg(feature = "std")]
    #[inline]
    fn to_ascii_uppercase(&self) -> Vec<u8> {
        self.as_bytes().to_ascii_uppercase()
    }

    /// Convert this byte string to its uppercase ASCII equivalent in place.
    ///
    /// In this case, uppercase is only defined in ASCII letters. Namely, the
    /// letters `a-z` are converted to `A-Z`. All other bytes remain unchanged.
    ///
    /// If you don't need to do the conversion in
    /// place and instead prefer convenience, then use
    /// [`to_ascii_uppercase`](#method.to_ascii_uppercase) instead.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let mut s = <Vec<u8>>::from("hello β");
    /// s.make_ascii_uppercase();
    /// assert_eq!(s, B("HELLO β"));
    /// ```
    ///
    /// Invalid UTF-8 remains as is:
    ///
    /// ```
    /// use bstr::{B, ByteSlice, ByteVec};
    ///
    /// let mut s = <Vec<u8>>::from_slice(b"foo\xFFbar\xE2\x98baz");
    /// s.make_ascii_uppercase();
    /// assert_eq!(s, B(b"FOO\xFFBAR\xE2\x98BAZ"));
    /// ```
    #[inline]
    fn make_ascii_uppercase(&mut self) {
        self.as_bytes_mut().make_ascii_uppercase();
    }

    /// Reverse the bytes in this string, in place.
    ///
    /// This is not necessarily a well formed operation! For example, if this
    /// byte string contains valid UTF-8 that isn't ASCII, then reversing the
    /// string will likely result in invalid UTF-8 and otherwise non-sensical
    /// content.
    ///
    /// Note that this is equivalent to the generic `[u8]::reverse` method.
    /// This method is provided to permit callers to explicitly differentiate
    /// between reversing bytes, codepoints and graphemes.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let mut s = <Vec<u8>>::from("hello");
    /// s.reverse_bytes();
    /// assert_eq!(s, "olleh".as_bytes());
    /// ```
    #[inline]
    fn reverse_bytes(&mut self) {
        self.as_bytes_mut().reverse();
    }

    /// Reverse the codepoints in this string, in place.
    ///
    /// If this byte string is valid UTF-8, then its reversal by codepoint
    /// is also guaranteed to be valid UTF-8.
    ///
    /// This operation is equivalent to the following, but without allocating:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let mut s = <Vec<u8>>::from("foo☃bar");
    ///
    /// let mut chars: Vec<char> = s.chars().collect();
    /// chars.reverse();
    ///
    /// let reversed: String = chars.into_iter().collect();
    /// assert_eq!(reversed, "rab☃oof");
    /// ```
    ///
    /// Note that this is not necessarily a well formed operation. For example,
    /// if this byte string contains grapheme clusters with more than one
    /// codepoint, then those grapheme clusters will not necessarily be
    /// preserved. If you'd like to preserve grapheme clusters, then use
    /// [`reverse_graphemes`](#method.reverse_graphemes) instead.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let mut s = <Vec<u8>>::from("foo☃bar");
    /// s.reverse_chars();
    /// assert_eq!(s, "rab☃oof".as_bytes());
    /// ```
    ///
    /// This example shows that not all reversals lead to a well formed string.
    /// For example, in this case, combining marks are used to put accents over
    /// some letters, and those accent marks must appear after the codepoints
    /// they modify.
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let mut s = <Vec<u8>>::from("résumé");
    /// s.reverse_chars();
    /// assert_eq!(s, B(b"\xCC\x81emus\xCC\x81er"));
    /// ```
    ///
    /// A word of warning: the above example relies on the fact that
    /// `résumé` is in decomposed normal form, which means there are separate
    /// codepoints for the accents above `e`. If it is instead in composed
    /// normal form, then the example works:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let mut s = <Vec<u8>>::from("résumé");
    /// s.reverse_chars();
    /// assert_eq!(s, B("émusér"));
    /// ```
    ///
    /// The point here is to be cautious and not assume that just because
    /// `reverse_chars` works in one case, that it therefore works in all
    /// cases.
    #[inline]
    fn reverse_chars(&mut self) {
        let mut i = 0;
        loop {
            let (_, size) = utf8::decode(&self.as_bytes()[i..]);
            if size == 0 {
                break;
            }
            if size > 1 {
                self.as_bytes_mut()[i..i + size].reverse_bytes();
            }
            i += size;
        }
        self.reverse_bytes();
    }

    /// Reverse the graphemes in this string, in place.
    ///
    /// If this byte string is valid UTF-8, then its reversal by grapheme
    /// is also guaranteed to be valid UTF-8.
    ///
    /// This operation is equivalent to the following, but without allocating:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let mut s = <Vec<u8>>::from("foo☃bar");
    ///
    /// let mut graphemes: Vec<&str> = s.graphemes().collect();
    /// graphemes.reverse();
    ///
    /// let reversed = graphemes.concat();
    /// assert_eq!(reversed, "rab☃oof");
    /// ```
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let mut s = <Vec<u8>>::from("foo☃bar");
    /// s.reverse_graphemes();
    /// assert_eq!(s, "rab☃oof".as_bytes());
    /// ```
    ///
    /// This example shows how this correctly handles grapheme clusters,
    /// unlike `reverse_chars`.
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// let mut s = <Vec<u8>>::from("résumé");
    /// s.reverse_graphemes();
    /// assert_eq!(s, "émusér".as_bytes());
    /// ```
    #[cfg(feature = "unicode")]
    #[inline]
    fn reverse_graphemes(&mut self) {
        use unicode::decode_grapheme;

        let mut i = 0;
        loop {
            let (_, size) = decode_grapheme(&self.as_bytes()[i..]);
            if size == 0 {
                break;
            }
            if size > 1 {
                self.as_bytes_mut()[i..i + size].reverse_bytes();
            }
            i += size;
        }
        self.reverse_bytes();
    }

    /// Returns true if and only if every byte in this byte string is ASCII.
    ///
    /// ASCII is an encoding that defines 128 codepoints. A byte corresponds to
    /// an ASCII codepoint if and only if it is in the inclusive range
    /// `[0, 127]`.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// assert!(B("abc").is_ascii());
    /// assert!(!B("☃βツ").is_ascii());
    /// assert!(!B(b"\xFF").is_ascii());
    /// ```
    #[inline]
    fn is_ascii(&self) -> bool {
        ascii::first_non_ascii_byte(self.as_bytes()) == self.as_bytes().len()
    }

    /// Returns true if and only if the entire byte string is valid UTF-8.
    ///
    /// If you need location information about where a byte string's first
    /// invalid UTF-8 byte is, then use the [`to_str`](#method.to_str) method.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// assert!(B("abc").is_utf8());
    /// assert!(B("☃βツ").is_utf8());
    /// // invalid bytes
    /// assert!(!B(b"abc\xFF").is_utf8());
    /// // surrogate encoding
    /// assert!(!B(b"\xED\xA0\x80").is_utf8());
    /// // incomplete sequence
    /// assert!(!B(b"\xF0\x9D\x9Ca").is_utf8());
    /// // overlong sequence
    /// assert!(!B(b"\xF0\x82\x82\xAC").is_utf8());
    /// ```
    #[inline]
    fn is_utf8(&self) -> bool {
        utf8::validate(self.as_bytes()).is_ok()
    }

    /// Returns the last byte in this byte string, if it's non-empty. If this
    /// byte string is empty, this returns `None`.
    ///
    /// Note that this is like the generic `[u8]::last`, except this returns
    /// the byte by value instead of a reference to the byte.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::ByteSlice;
    ///
    /// assert_eq!(Some(b'z'), b"baz".last_byte());
    /// assert_eq!(None, b"".last_byte());
    /// ```
    #[inline]
    fn last_byte(&self) -> Option<u8> {
        let bytes = self.as_bytes();
        bytes.get(bytes.len().saturating_sub(1)).map(|&b| b)
    }

    /// Returns the index of the first non-ASCII byte in this byte string (if
    /// any such indices exist). Specifically, it returns the index of the
    /// first byte with a value greater than or equal to `0x80`.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::{ByteSlice, B};
    ///
    /// assert_eq!(Some(3), b"abc\xff".find_non_ascii_byte());
    /// assert_eq!(None, b"abcde".find_non_ascii_byte());
    /// assert_eq!(Some(0), B("😀").find_non_ascii_byte());
    /// ```
    #[inline]
    fn find_non_ascii_byte(&self) -> Option<usize> {
        let index = ascii::first_non_ascii_byte(self.as_bytes());
        if index == self.as_bytes().len() {
            None
        } else {
            Some(index)
        }
    }

    /// Copies elements from one part of the slice to another part of itself,
    /// where the parts may be overlapping.
    ///
    /// `src` is the range within this byte string to copy from, while `dest`
    /// is the starting index of the range within this byte string to copy to.
    /// The length indicated by `src` must be less than or equal to the number
    /// of bytes from `dest` to the end of the byte string.
    ///
    /// # Panics
    ///
    /// Panics if either range is out of bounds, or if `src` is too big to fit
    /// into `dest`, or if the end of `src` is before the start.
    ///
    /// # Examples
    ///
    /// Copying four bytes within a byte string:
    ///
    /// ```
    /// use bstr::{B, ByteSlice};
    ///
    /// let mut buf = *b"Hello, World!";
    /// let s = &mut buf;
    /// s.copy_within_str(1..5, 8);
    /// assert_eq!(s, B("Hello, Wello!"));
    /// ```
    #[inline]
    fn copy_within_str<R>(&mut self, src: R, dest: usize)
    where
        R: ops::RangeBounds<usize>,
    {
        // TODO: Deprecate this once slice::copy_within stabilizes.
        let src_start = match src.start_bound() {
            ops::Bound::Included(&n) => n,
            ops::Bound::Excluded(&n) => {
                n.checked_add(1).expect("attempted to index slice beyond max")
            }
            ops::Bound::Unbounded => 0,
        };
        let src_end = match src.end_bound() {
            ops::Bound::Included(&n) => {
                n.checked_add(1).expect("attempted to index slice beyond max")
            }
            ops::Bound::Excluded(&n) => n,
            ops::Bound::Unbounded => self.as_bytes().len(),
        };
        assert!(src_start <= src_end, "src end is before src start");
        assert!(src_end <= self.as_bytes().len(), "src is out of bounds");
        let count = src_end - src_start;
        assert!(
            dest <= self.as_bytes().len() - count,
            "dest is out of bounds",
        );

        // SAFETY: This is safe because we use ptr::copy to handle overlapping
        // copies, and is also safe because we've checked all the bounds above.
        // Finally, we are only dealing with u8 data, which is Copy, which
        // means we can copy without worrying about ownership/destructors.
        unsafe {
            ptr::copy(
                self.as_bytes().get_unchecked(src_start),
                self.as_bytes_mut().get_unchecked_mut(dest),
                count,
            );
        }
    }
}

/// A single substring searcher fixed to a particular needle.
///
/// The purpose of this type is to permit callers to construct a substring
/// searcher that can be used to search haystacks without the overhead of
/// constructing the searcher in the first place. This is a somewhat niche
/// concern when it's necessary to re-use the same needle to search multiple
/// different haystacks with as little overhead as possible. In general, using
/// [`ByteSlice::find`](trait.ByteSlice.html#method.find)
/// or
/// [`ByteSlice::find_iter`](trait.ByteSlice.html#method.find_iter)
/// is good enough, but `Finder` is useful when you can meaningfully observe
/// searcher construction time in a profile.
///
/// When the `std` feature is enabled, then this type has an `into_owned`
/// version which permits building a `Finder` that is not connected to the
/// lifetime of its needle.
#[derive(Clone, Debug)]
pub struct Finder<'a> {
    searcher: TwoWay<'a>,
}

impl<'a> Finder<'a> {
    /// Create a new finder for the given needle.
    #[inline]
    pub fn new<B: ?Sized + AsRef<[u8]>>(needle: &'a B) -> Finder<'a> {
        Finder { searcher: TwoWay::forward(needle.as_ref()) }
    }

    /// Convert this finder into its owned variant, such that it no longer
    /// borrows the needle.
    ///
    /// If this is already an owned finder, then this is a no-op. Otherwise,
    /// this copies the needle.
    ///
    /// This is only available when the `std` feature is enabled.
    #[cfg(feature = "std")]
    #[inline]
    pub fn into_owned(self) -> Finder<'static> {
        Finder { searcher: self.searcher.into_owned() }
    }

    /// Returns the needle that this finder searches for.
    ///
    /// Note that the lifetime of the needle returned is tied to the lifetime
    /// of the finder, and may be shorter than the `'a` lifetime. Namely, a
    /// finder's needle can be either borrowed or owned, so the lifetime of the
    /// needle returned must necessarily be the shorter of the two.
    #[inline]
    pub fn needle(&self) -> &[u8] {
        self.searcher.needle()
    }

    /// Returns the index of the first occurrence of this needle in the given
    /// haystack.
    ///
    /// The haystack may be any type that can be cheaply converted into a
    /// `&[u8]`. This includes, but is not limited to, `&str` and `&[u8]`.
    ///
    /// # Complexity
    ///
    /// This routine is guaranteed to have worst case linear time complexity
    /// with respect to both the needle and the haystack. That is, this runs
    /// in `O(needle.len() + haystack.len())` time.
    ///
    /// This routine is also guaranteed to have worst case constant space
    /// complexity.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::Finder;
    ///
    /// let haystack = "foo bar baz";
    /// assert_eq!(Some(0), Finder::new("foo").find(haystack));
    /// assert_eq!(Some(4), Finder::new("bar").find(haystack));
    /// assert_eq!(None, Finder::new("quux").find(haystack));
    /// ```
    #[inline]
    pub fn find<B: AsRef<[u8]>>(&self, haystack: B) -> Option<usize> {
        self.searcher.find(haystack.as_ref())
    }
}

/// A single substring reverse searcher fixed to a particular needle.
///
/// The purpose of this type is to permit callers to construct a substring
/// searcher that can be used to search haystacks without the overhead of
/// constructing the searcher in the first place. This is a somewhat niche
/// concern when it's necessary to re-use the same needle to search multiple
/// different haystacks with as little overhead as possible. In general, using
/// [`ByteSlice::rfind`](trait.ByteSlice.html#method.rfind)
/// or
/// [`ByteSlice::rfind_iter`](trait.ByteSlice.html#method.rfind_iter)
/// is good enough, but `FinderReverse` is useful when you can meaningfully
/// observe searcher construction time in a profile.
///
/// When the `std` feature is enabled, then this type has an `into_owned`
/// version which permits building a `FinderReverse` that is not connected to
/// the lifetime of its needle.
#[derive(Clone, Debug)]
pub struct FinderReverse<'a> {
    searcher: TwoWay<'a>,
}

impl<'a> FinderReverse<'a> {
    /// Create a new reverse finder for the given needle.
    #[inline]
    pub fn new<B: ?Sized + AsRef<[u8]>>(needle: &'a B) -> FinderReverse<'a> {
        FinderReverse { searcher: TwoWay::reverse(needle.as_ref()) }
    }

    /// Convert this finder into its owned variant, such that it no longer
    /// borrows the needle.
    ///
    /// If this is already an owned finder, then this is a no-op. Otherwise,
    /// this copies the needle.
    ///
    /// This is only available when the `std` feature is enabled.
    #[cfg(feature = "std")]
    #[inline]
    pub fn into_owned(self) -> FinderReverse<'static> {
        FinderReverse { searcher: self.searcher.into_owned() }
    }

    /// Returns the needle that this finder searches for.
    ///
    /// Note that the lifetime of the needle returned is tied to the lifetime
    /// of this finder, and may be shorter than the `'a` lifetime. Namely,
    /// a finder's needle can be either borrowed or owned, so the lifetime of
    /// the needle returned must necessarily be the shorter of the two.
    #[inline]
    pub fn needle(&self) -> &[u8] {
        self.searcher.needle()
    }

    /// Returns the index of the last occurrence of this needle in the given
    /// haystack.
    ///
    /// The haystack may be any type that can be cheaply converted into a
    /// `&[u8]`. This includes, but is not limited to, `&str` and `&[u8]`.
    ///
    /// # Complexity
    ///
    /// This routine is guaranteed to have worst case linear time complexity
    /// with respect to both the needle and the haystack. That is, this runs
    /// in `O(needle.len() + haystack.len())` time.
    ///
    /// This routine is also guaranteed to have worst case constant space
    /// complexity.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```
    /// use bstr::FinderReverse;
    ///
    /// let haystack = "foo bar baz";
    /// assert_eq!(Some(0), FinderReverse::new("foo").rfind(haystack));
    /// assert_eq!(Some(4), FinderReverse::new("bar").rfind(haystack));
    /// assert_eq!(None, FinderReverse::new("quux").rfind(haystack));
    /// ```
    #[inline]
    pub fn rfind<B: AsRef<[u8]>>(&self, haystack: B) -> Option<usize> {
        self.searcher.rfind(haystack.as_ref())
    }
}

/// An iterator over non-overlapping substring matches.
///
/// Matches are reported by the byte offset at which they begin.
///
/// `'a` is the shorter of two lifetimes: the byte string being searched or the
/// byte string being looked for.
#[derive(Debug)]
pub struct Find<'a> {
    haystack: &'a [u8],
    prestate: PrefilterState,
    searcher: TwoWay<'a>,
    pos: usize,
}

impl<'a> Find<'a> {
    fn new(haystack: &'a [u8], needle: &'a [u8]) -> Find<'a> {
        let searcher = TwoWay::forward(needle);
        let prestate = searcher.prefilter_state();
        Find { haystack, prestate, searcher, pos: 0 }
    }
}

impl<'a> Iterator for Find<'a> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        if self.pos > self.haystack.len() {
            return None;
        }
        let result = self
            .searcher
            .find_with(&mut self.prestate, &self.haystack[self.pos..]);
        match result {
            None => None,
            Some(i) => {
                let pos = self.pos + i;
                self.pos = pos + cmp::max(1, self.searcher.needle().len());
                Some(pos)
            }
        }
    }
}

/// An iterator over non-overlapping substring matches in reverse.
///
/// Matches are reported by the byte offset at which they begin.
///
/// `'a` is the shorter of two lifetimes: the byte string being searched or the
/// byte string being looked for.
#[derive(Debug)]
pub struct FindReverse<'a> {
    haystack: &'a [u8],
    prestate: PrefilterState,
    searcher: TwoWay<'a>,
    /// When searching with an empty needle, this gets set to `None` after
    /// we've yielded the last element at `0`.
    pos: Option<usize>,
}

impl<'a> FindReverse<'a> {
    fn new(haystack: &'a [u8], needle: &'a [u8]) -> FindReverse<'a> {
        let searcher = TwoWay::reverse(needle);
        let prestate = searcher.prefilter_state();
        let pos = Some(haystack.len());
        FindReverse { haystack, prestate, searcher, pos }
    }

    fn haystack(&self) -> &'a [u8] {
        self.haystack
    }

    fn needle(&self) -> &[u8] {
        self.searcher.needle()
    }
}

impl<'a> Iterator for FindReverse<'a> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        let pos = match self.pos {
            None => return None,
            Some(pos) => pos,
        };
        let result = self
            .searcher
            .rfind_with(&mut self.prestate, &self.haystack[..pos]);
        match result {
            None => None,
            Some(i) => {
                if pos == i {
                    self.pos = pos.checked_sub(1);
                } else {
                    self.pos = Some(i);
                }
                Some(i)
            }
        }
    }
}

/// An iterator over the bytes in a byte string.
///
/// `'a` is the lifetime of the byte string being traversed.
#[derive(Clone, Debug)]
pub struct Bytes<'a> {
    it: slice::Iter<'a, u8>,
}

impl<'a> Bytes<'a> {
    /// Views the remaining underlying data as a subslice of the original data.
    /// This has the same lifetime as the original slice,
    /// and so the iterator can continue to be used while this exists.
    #[inline]
    pub fn as_slice(&self) -> &'a [u8] {
        self.it.as_slice()
    }
}

impl<'a> Iterator for Bytes<'a> {
    type Item = u8;

    #[inline]
    fn next(&mut self) -> Option<u8> {
        self.it.next().map(|&b| b)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }
}

impl<'a> DoubleEndedIterator for Bytes<'a> {
    #[inline]
    fn next_back(&mut self) -> Option<u8> {
        self.it.next_back().map(|&b| b)
    }
}

impl<'a> ExactSizeIterator for Bytes<'a> {
    #[inline]
    fn len(&self) -> usize {
        self.it.len()
    }
}

impl<'a> iter::FusedIterator for Bytes<'a> {}

/// An iterator over the fields in a byte string, separated by whitespace.
///
/// This iterator splits on contiguous runs of whitespace, such that the fields
/// in `foo\t\t\n  \nbar` are `foo` and `bar`.
///
/// `'a` is the lifetime of the byte string being split.
#[derive(Debug)]
pub struct Fields<'a> {
    it: FieldsWith<'a, fn(char) -> bool>,
}

impl<'a> Fields<'a> {
    fn new(bytes: &'a [u8]) -> Fields<'a> {
        Fields { it: bytes.fields_with(|ch| ch.is_whitespace()) }
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = &'a [u8];

    #[inline]
    fn next(&mut self) -> Option<&'a [u8]> {
        self.it.next()
    }
}

/// An iterator over fields in the byte string, separated by a predicate over
/// codepoints.
///
/// This iterator splits a byte string based on its predicate function such
/// that the elements returned are separated by contiguous runs of codepoints
/// for which the predicate returns true.
///
/// `'a` is the lifetime of the byte string being split, while `F` is the type
/// of the predicate, i.e., `FnMut(char) -> bool`.
#[derive(Debug)]
pub struct FieldsWith<'a, F> {
    f: F,
    bytes: &'a [u8],
    chars: CharIndices<'a>,
}

impl<'a, F: FnMut(char) -> bool> FieldsWith<'a, F> {
    fn new(bytes: &'a [u8], f: F) -> FieldsWith<'a, F> {
        FieldsWith { f, bytes, chars: bytes.char_indices() }
    }
}

impl<'a, F: FnMut(char) -> bool> Iterator for FieldsWith<'a, F> {
    type Item = &'a [u8];

    #[inline]
    fn next(&mut self) -> Option<&'a [u8]> {
        let (start, mut end);
        loop {
            match self.chars.next() {
                None => return None,
                Some((s, e, ch)) => {
                    if !(self.f)(ch) {
                        start = s;
                        end = e;
                        break;
                    }
                }
            }
        }
        while let Some((_, e, ch)) = self.chars.next() {
            if (self.f)(ch) {
                break;
            }
            end = e;
        }
        Some(&self.bytes[start..end])
    }
}

/// An iterator over substrings in a byte string, split by a separator.
///
/// `'a` is the lifetime of the byte string being split.
#[derive(Debug)]
pub struct Split<'a> {
    finder: Find<'a>,
    /// The end position of the previous match of our splitter. The element
    /// we yield corresponds to the substring starting at `last` up to the
    /// beginning of the next match of the splitter.
    last: usize,
    /// Only set when iteration is complete. A corner case here is when a
    /// splitter is matched at the end of the haystack. At that point, we still
    /// need to yield an empty string following it.
    done: bool,
}

impl<'a> Split<'a> {
    fn new(haystack: &'a [u8], splitter: &'a [u8]) -> Split<'a> {
        let finder = haystack.find_iter(splitter);
        Split { finder, last: 0, done: false }
    }
}

impl<'a> Iterator for Split<'a> {
    type Item = &'a [u8];

    #[inline]
    fn next(&mut self) -> Option<&'a [u8]> {
        let haystack = self.finder.haystack;
        match self.finder.next() {
            Some(start) => {
                let next = &haystack[self.last..start];
                self.last = start + self.finder.searcher.needle().len();
                Some(next)
            }
            None => {
                if self.last >= haystack.len() {
                    if !self.done {
                        self.done = true;
                        Some(b"")
                    } else {
                        None
                    }
                } else {
                    let s = &haystack[self.last..];
                    self.last = haystack.len();
                    self.done = true;
                    Some(s)
                }
            }
        }
    }
}

/// An iterator over substrings in a byte string, split by a separator, in
/// reverse.
///
/// `'a` is the lifetime of the byte string being split, while `F` is the type
/// of the predicate, i.e., `FnMut(char) -> bool`.
#[derive(Debug)]
pub struct SplitReverse<'a> {
    finder: FindReverse<'a>,
    /// The end position of the previous match of our splitter. The element
    /// we yield corresponds to the substring starting at `last` up to the
    /// beginning of the next match of the splitter.
    last: usize,
    /// Only set when iteration is complete. A corner case here is when a
    /// splitter is matched at the end of the haystack. At that point, we still
    /// need to yield an empty string following it.
    done: bool,
}

impl<'a> SplitReverse<'a> {
    fn new(haystack: &'a [u8], splitter: &'a [u8]) -> SplitReverse<'a> {
        let finder = haystack.rfind_iter(splitter);
        SplitReverse { finder, last: haystack.len(), done: false }
    }
}

impl<'a> Iterator for SplitReverse<'a> {
    type Item = &'a [u8];

    #[inline]
    fn next(&mut self) -> Option<&'a [u8]> {
        let haystack = self.finder.haystack();
        match self.finder.next() {
            Some(start) => {
                let nlen = self.finder.needle().len();
                let next = &haystack[start + nlen..self.last];
                self.last = start;
                Some(next)
            }
            None => {
                if self.last == 0 {
                    if !self.done {
                        self.done = true;
                        Some(b"")
                    } else {
                        None
                    }
                } else {
                    let s = &haystack[..self.last];
                    self.last = 0;
                    self.done = true;
                    Some(s)
                }
            }
        }
    }
}

/// An iterator over at most `n` substrings in a byte string, split by a
/// separator.
///
/// `'a` is the lifetime of the byte string being split, while `F` is the type
/// of the predicate, i.e., `FnMut(char) -> bool`.
#[derive(Debug)]
pub struct SplitN<'a> {
    split: Split<'a>,
    limit: usize,
    count: usize,
}

impl<'a> SplitN<'a> {
    fn new(
        haystack: &'a [u8],
        splitter: &'a [u8],
        limit: usize,
    ) -> SplitN<'a> {
        let split = haystack.split_str(splitter);
        SplitN { split, limit, count: 0 }
    }
}

impl<'a> Iterator for SplitN<'a> {
    type Item = &'a [u8];

    #[inline]
    fn next(&mut self) -> Option<&'a [u8]> {
        self.count += 1;
        if self.count > self.limit || self.split.done {
            None
        } else if self.count == self.limit {
            Some(&self.split.finder.haystack[self.split.last..])
        } else {
            self.split.next()
        }
    }
}

/// An iterator over at most `n` substrings in a byte string, split by a
/// separator, in reverse.
///
/// `'a` is the lifetime of the byte string being split, while `F` is the type
/// of the predicate, i.e., `FnMut(char) -> bool`.
#[derive(Debug)]
pub struct SplitNReverse<'a> {
    split: SplitReverse<'a>,
    limit: usize,
    count: usize,
}

impl<'a> SplitNReverse<'a> {
    fn new(
        haystack: &'a [u8],
        splitter: &'a [u8],
        limit: usize,
    ) -> SplitNReverse<'a> {
        let split = haystack.rsplit_str(splitter);
        SplitNReverse { split, limit, count: 0 }
    }
}

impl<'a> Iterator for SplitNReverse<'a> {
    type Item = &'a [u8];

    #[inline]
    fn next(&mut self) -> Option<&'a [u8]> {
        self.count += 1;
        if self.count > self.limit || self.split.done {
            None
        } else if self.count == self.limit {
            Some(&self.split.finder.haystack()[..self.split.last])
        } else {
            self.split.next()
        }
    }
}

/// An iterator over all lines in a byte string, without their terminators.
///
/// For this iterator, the only line terminators recognized are `\r\n` and
/// `\n`.
///
/// `'a` is the lifetime of the byte string being iterated over.
pub struct Lines<'a> {
    it: LinesWithTerminator<'a>,
}

impl<'a> Lines<'a> {
    fn new(bytes: &'a [u8]) -> Lines<'a> {
        Lines { it: LinesWithTerminator::new(bytes) }
    }
}

impl<'a> Iterator for Lines<'a> {
    type Item = &'a [u8];

    #[inline]
    fn next(&mut self) -> Option<&'a [u8]> {
        let mut line = self.it.next()?;
        if line.last_byte() == Some(b'\n') {
            line = &line[..line.len() - 1];
            if line.last_byte() == Some(b'\r') {
                line = &line[..line.len() - 1];
            }
        }
        Some(line)
    }
}

/// An iterator over all lines in a byte string, including their terminators.
///
/// For this iterator, the only line terminator recognized is `\n`. (Since
/// line terminators are included, this also handles `\r\n` line endings.)
///
/// Line terminators are only included if they are present in the original
/// byte string. For example, the last line in a byte string may not end with
/// a line terminator.
///
/// Concatenating all elements yielded by this iterator is guaranteed to yield
/// the original byte string.
///
/// `'a` is the lifetime of the byte string being iterated over.
pub struct LinesWithTerminator<'a> {
    bytes: &'a [u8],
}

impl<'a> LinesWithTerminator<'a> {
    fn new(bytes: &'a [u8]) -> LinesWithTerminator<'a> {
        LinesWithTerminator { bytes }
    }
}

impl<'a> Iterator for LinesWithTerminator<'a> {
    type Item = &'a [u8];

    #[inline]
    fn next(&mut self) -> Option<&'a [u8]> {
        match self.bytes.find_byte(b'\n') {
            None if self.bytes.is_empty() => None,
            None => {
                let line = self.bytes;
                self.bytes = b"";
                Some(line)
            }
            Some(end) => {
                let line = &self.bytes[..end + 1];
                self.bytes = &self.bytes[end + 1..];
                Some(line)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ext_slice::{ByteSlice, B};
    use tests::LOSSY_TESTS;

    #[test]
    fn to_str_lossy() {
        for (i, &(expected, input)) in LOSSY_TESTS.iter().enumerate() {
            let got = B(input).to_str_lossy();
            assert_eq!(
                expected.as_bytes(),
                got.as_bytes(),
                "to_str_lossy(ith: {:?}, given: {:?})",
                i,
                input,
            );

            let mut got = String::new();
            B(input).to_str_lossy_into(&mut got);
            assert_eq!(
                expected.as_bytes(),
                got.as_bytes(),
                "to_str_lossy_into",
            );

            let got = String::from_utf8_lossy(input);
            assert_eq!(expected.as_bytes(), got.as_bytes(), "std");
        }
    }

    #[test]
    #[should_panic]
    fn copy_within_fail1() {
        let mut buf = *b"foobar";
        let s = &mut buf;
        s.copy_within_str(0..2, 5);
    }

    #[test]
    #[should_panic]
    fn copy_within_fail2() {
        let mut buf = *b"foobar";
        let s = &mut buf;
        s.copy_within_str(3..2, 0);
    }

    #[test]
    #[should_panic]
    fn copy_within_fail3() {
        let mut buf = *b"foobar";
        let s = &mut buf;
        s.copy_within_str(5..7, 0);
    }

    #[test]
    #[should_panic]
    fn copy_within_fail4() {
        let mut buf = *b"foobar";
        let s = &mut buf;
        s.copy_within_str(0..1, 6);
    }
}

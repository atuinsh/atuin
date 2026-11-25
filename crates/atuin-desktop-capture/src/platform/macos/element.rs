//! Retained AX elements, and the CoreFoundation values that their attributes hold.

use std::borrow::Cow;
use std::ops::Deref;
use std::ptr;

use core_foundation::array::CFArray;
use core_foundation::base::{CFIndex, CFRange, CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::{CFString, CFStringGetBytes, kCFStringEncodingUTF8};

use super::ffi;

/// The process id of an application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pid(i32);

impl Pid {
    #[must_use]
    pub const fn new(raw: i32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> i32 {
        self.0
    }
}

/// A retained `AXUIElement`. Equality is `CFEqual`, and the hash is `CFHash`.
#[derive(Debug, Clone, PartialEq)]
pub struct Element(CFType);

impl Eq for Element {}

impl std::hash::Hash for Element {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // SAFETY: the element is live. Elements that are equal have equal hashes.
        let hash = unsafe { core_foundation::base::CFHash(self.0.as_CFTypeRef()) };
        state.write_usize(hash);
    }
}

// SAFETY: `CFType` is not `Send` in general, but this one always holds an `AXUIElement`. That
// is an immutable, reference-counted handle, and the AX API accepts calls on it from any
// thread.
unsafe impl Send for Element {}
// SAFETY: the handle is immutable, and the AX API accepts calls on it from any thread. Every
// use of the handle through a shared reference is an AX call.
unsafe impl Sync for Element {}

impl Element {
    pub fn system_wide() -> Self {
        // SAFETY: the function has no preconditions.
        let element = unsafe { ffi::AXUIElementCreateSystemWide() };
        // SAFETY: the function returned a new element that the caller owns.
        Self(unsafe { CFType::wrap_under_create_rule(element) })
    }

    pub fn application(pid: Pid) -> Self {
        // SAFETY: the function accepts any pid.
        let element = unsafe { ffi::AXUIElementCreateApplication(pid.0) };
        // SAFETY: the function returned a new element that the caller owns.
        Self(unsafe { CFType::wrap_under_create_rule(element) })
    }

    /// Returns the element that `value` holds, when it holds one.
    pub fn from_value(value: CFType) -> Option<Self> {
        // SAFETY: the function only returns a constant.
        (value.type_of() == unsafe { ffi::AXUIElementGetTypeID() }).then_some(Self(value))
    }

    pub fn as_ptr(&self) -> ffi::AXUIElementRef {
        self.0.as_CFTypeRef()
    }

    pub fn attribute(&self, name: &'static str) -> Option<CFType> {
        self.try_attribute(name).ok().flatten()
    }

    /// The value of the attribute `name`, or `None` when the element lacks the attribute or
    /// a value for it.
    ///
    /// Fails with the AX error of a call that couldn't read the attribute, such as a call to
    /// an application that didn't answer in time.
    pub fn try_attribute(&self, name: &'static str) -> Result<Option<CFType>, ffi::AXError> {
        let name = CFString::from_static_string(name);
        let mut value: CFTypeRef = ptr::null();
        // SAFETY: `self` and `name` are live, and `value` outlives the call.
        let error = unsafe {
            ffi::AXUIElementCopyAttributeValue(
                self.as_ptr(),
                name.as_concrete_TypeRef(),
                &raw mut value,
            )
        };
        match error {
            ffi::AX_SUCCESS if !value.is_null() => {
                // SAFETY: a successful call stores a value that the caller owns.
                Ok(Some(unsafe { CFType::wrap_under_create_rule(value) }))
            }
            ffi::AX_SUCCESS => Ok(None),
            error if is_missing(error) => Ok(None),
            error => Err(error),
        }
    }

    /// Fetches the attributes that `names` lists in one call.
    pub fn attributes(&self, names: &AttributeNames) -> Option<AttributeValues> {
        let mut values = ptr::null();
        // SAFETY: `self` and the array of names are live, and `values` outlives the call.
        let error = unsafe {
            ffi::AXUIElementCopyMultipleAttributeValues(
                self.as_ptr(),
                names.0.as_concrete_TypeRef(),
                0,
                &raw mut values,
            )
        };
        if error != ffi::AX_SUCCESS || values.is_null() {
            return None;
        }
        // SAFETY: a successful call stores an array that the caller owns.
        Some(AttributeValues(unsafe { CFArray::wrap_under_create_rule(values) }))
    }

    pub fn string(&self, name: &'static str) -> Option<String> {
        string_of(&self.attribute(name)?)
    }

    /// Whether the attribute `name` holds the string `text`.
    pub fn has(&self, name: &'static str, text: &str) -> bool {
        self.attribute(name).is_some_and(|value| is_text(&value, text))
    }

    pub fn element(&self, name: &'static str) -> Option<Self> {
        Self::from_value(self.attribute(name)?)
    }

    /// Returns the elements that the attribute `name` holds. The elements are empty when the
    /// call fails.
    pub fn elements(&self, name: &'static str) -> Elements {
        self.attribute(name).map(|value| elements_of(&value)).unwrap_or_default()
    }

    /// Returns the process that the element belongs to.
    pub fn pid(&self) -> Option<Pid> {
        let mut pid = 0;
        // SAFETY: `self` is live, and `pid` outlives the call.
        let error = unsafe { ffi::AXUIElementGetPid(self.as_ptr(), &raw mut pid) };
        (error == ffi::AX_SUCCESS).then_some(Pid(pid))
    }

    pub fn set_true(&self, name: &'static str) {
        let name = CFString::from_static_string(name);
        // SAFETY: every argument is a live CoreFoundation object.
        unsafe {
            ffi::AXUIElementSetAttributeValue(
                self.as_ptr(),
                name.as_concrete_TypeRef(),
                CFBoolean::true_value().as_CFTypeRef(),
            );
        }
    }
}

/// The names of attributes that [`Element::attributes`] fetches in one call.
pub struct AttributeNames(CFArray<CFString>);

// SAFETY: the array and its strings are immutable CoreFoundation objects, which any thread may
// use.
unsafe impl Send for AttributeNames {}
// SAFETY: the array and its strings are immutable CoreFoundation objects, which any thread may
// use.
unsafe impl Sync for AttributeNames {}

impl AttributeNames {
    pub fn new<const N: usize>(names: [&'static str; N]) -> Self {
        Self(CFArray::from_CFTypes(&names.map(CFString::from_static_string)))
    }
}

/// The values that one call of [`Element::attributes`] fetched, in the order of their names.
///
/// A missing attribute holds an error value, which fails every conversion.
pub struct AttributeValues(CFArray);

impl AttributeValues {
    pub fn get(&self, index: usize) -> Option<CFType> {
        let index = isize::try_from(index).ok()?;
        // SAFETY: the array holds live CoreFoundation objects, and wrapping retains the item.
        self.0.get(index).map(|item| unsafe { CFType::wrap_under_get_rule(*item) })
    }

    pub fn text(&self, index: usize) -> Option<CFString> {
        self.get(index)?.downcast::<CFString>()
    }

    /// Whether the attribute at `index` holds the string `text`.
    pub fn is(&self, index: usize, text: &str) -> bool {
        self.get(index).is_some_and(|value| is_text(&value, text))
    }

    pub fn element(&self, index: usize) -> Option<Element> {
        Element::from_value(self.get(index)?)
    }
}

/// Whether `error` tells that an element lacks an attribute or a value for it. The same call
/// gives the same answer when it is made again.
pub const fn is_missing(error: ffi::AXError) -> bool {
    matches!(error, ffi::AX_ATTRIBUTE_UNSUPPORTED | ffi::AX_NO_VALUE)
}

/// The AX error that `value` holds, for a value that [`Element::attributes`] fetched in place
/// of an attribute that it couldn't read.
pub fn ax_error_of(value: &CFType) -> Option<ffi::AXError> {
    // SAFETY: the function only returns a constant.
    if value.type_of() != unsafe { ffi::AXValueGetTypeID() } {
        return None;
    }
    let mut error: ffi::AXError = ffi::AX_SUCCESS;
    // SAFETY: `value` is an `AXValue`. The call writes one `AXError` only when the value holds
    // an error, and `error` has room for it.
    let stored = unsafe {
        ffi::AXValueGetValue(value.as_CFTypeRef(), ffi::AX_VALUE_AX_ERROR, (&raw mut error).cast())
    };
    (stored != 0).then_some(error)
}

pub fn string_of(value: &CFType) -> Option<String> {
    value.downcast::<CFString>().map(|string| string_from(&string))
}

/// Returns the text of `string` in a new `String`.
///
/// Converting through `Display` would copy the text twice, because a string that the AX API
/// creates at run time isn't stored as UTF-8.
pub fn string_from(string: &CFString) -> String {
    Cow::from(string).into_owned()
}

/// How many bytes of text [`AxStr`] holds on the stack. The roles, subroles, and notification
/// names of the AX API are shorter.
const INLINE_BYTES: usize = 64;

/// The text of a `CFString`, which [`str_of`] copies to the stack when it is short.
pub enum AxStr<'a> {
    /// Text of up to [`INLINE_BYTES`] bytes, which `len` bytes of `bytes` hold as valid UTF-8.
    Inline {
        bytes: [u8; INLINE_BYTES],
        len: usize,
    },
    Other(Cow<'a, str>),
}

impl Deref for AxStr<'_> {
    type Target = str;

    fn deref(&self) -> &str {
        match self {
            // SAFETY: `str_of` checked that the bytes are valid UTF-8.
            Self::Inline { bytes, len } => unsafe { std::str::from_utf8_unchecked(&bytes[..*len]) },
            Self::Other(text) => text,
        }
    }
}

/// Returns the text of `string`.
///
/// Doesn't allocate for a string of up to [`INLINE_BYTES`] bytes. A longer string allocates
/// unless CoreFoundation stores it as UTF-8 already. A string that the AX API creates at run
/// time is never stored as UTF-8.
pub fn str_of(string: &CFString) -> AxStr<'_> {
    let mut bytes = [0; INLINE_BYTES];
    let length = string.char_len();
    let capacity = CFIndex::try_from(INLINE_BYTES).unwrap_or(CFIndex::MAX);
    let mut used: CFIndex = 0;
    // SAFETY: `string` is live, the range covers it, and `bytes` has room for `capacity` bytes.
    // `used` outlives the call.
    let converted = unsafe {
        CFStringGetBytes(
            string.as_concrete_TypeRef(),
            CFRange::init(0, length),
            kCFStringEncodingUTF8,
            0,
            0,
            bytes.as_mut_ptr(),
            capacity,
            &raw mut used,
        )
    };
    // The call converts what fits, so a string that it did not convert whole is too long.
    if converted == length
        && let Ok(len) = usize::try_from(used)
        && std::str::from_utf8(&bytes[..len]).is_ok()
    {
        return AxStr::Inline { bytes, len };
    }
    AxStr::Other(Cow::from(string))
}

/// Whether `value` is the string `text`.
pub fn is_text(value: &CFType, text: &str) -> bool {
    value.downcast::<CFString>().is_some_and(|string| *str_of(&string) == *text)
}

/// Returns the elements that `value` holds, when `value` is an array.
pub fn elements_of(value: &CFType) -> Elements {
    value.downcast::<CFArray>().map_or_else(Elements::default, Elements::new)
}

/// The elements of an array that an attribute holds, such as the children of an element.
///
/// The iterator skips the entries that aren't elements.
#[derive(Clone, Default)]
pub struct Elements {
    array: Option<CFArray>,
    /// The index of the next entry from the front.
    front: CFIndex,
    /// The index after the next entry from the back.
    back: CFIndex,
}

impl Elements {
    fn new(array: CFArray) -> Self {
        let back = array.len();
        Self {
            array: Some(array),
            front: 0,
            back,
        }
    }

    /// How many entries are left. Every entry of an array of elements is an element.
    pub fn entries(&self) -> usize {
        usize::try_from(self.back - self.front).unwrap_or(0)
    }

    /// Returns the element at `index`, or `None` for an entry that isn't an element.
    fn at(&self, index: CFIndex) -> Option<Element> {
        let item = self.array.as_ref()?.get(index)?;
        // SAFETY: the array holds live CoreFoundation objects, and wrapping retains the item.
        Element::from_value(unsafe { CFType::wrap_under_get_rule(*item) })
    }
}

impl Iterator for Elements {
    type Item = Element;

    fn next(&mut self) -> Option<Element> {
        while self.front < self.back {
            let index = self.front;
            self.front += 1;
            if let Some(element) = self.at(index) {
                return Some(element);
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.entries()))
    }
}

impl DoubleEndedIterator for Elements {
    fn next_back(&mut self) -> Option<Element> {
        while self.front < self.back {
            self.back -= 1;
            if let Some(element) = self.at(self.back) {
                return Some(element);
            }
        }
        None
    }
}

impl std::iter::FusedIterator for Elements {}

/// Reads an `AXValue` that holds a pair of doubles, such as a `CGPoint` or a `CGSize`.
pub fn pair_of(value: &CFType, kind: u32) -> Option<[f64; 2]> {
    // SAFETY: the function only returns a constant.
    if value.type_of() != unsafe { ffi::AXValueGetTypeID() } {
        return None;
    }
    let mut pair = [0.0f64; 2];
    // SAFETY: `value` is an `AXValue`. The call writes two doubles only when `kind` matches the
    // stored type, and `pair` has room for them.
    let stored =
        unsafe { ffi::AXValueGetValue(value.as_CFTypeRef(), kind, pair.as_mut_ptr().cast()) };
    (stored != 0).then_some(pair)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Held {
        Inline,
        Other,
    }

    #[rstest]
    #[case::empty(String::new(), Held::Inline)]
    #[case::role("AXStaticText".to_owned(), Held::Inline)]
    #[case::accented("Données".to_owned(), Held::Inline)]
    #[case::exactly_inline("x".repeat(INLINE_BYTES), Held::Inline)]
    #[case::one_byte_too_long("x".repeat(INLINE_BYTES + 1), Held::Other)]
    #[case::wide_char_across_the_end(format!("{}é", "x".repeat(INLINE_BYTES - 1)), Held::Other)]
    fn str_of_keeps_the_text(#[case] text: String, #[case] held: Held) {
        // The AX API builds its strings at run time, as this one is built.
        let string = CFString::new(&text);

        let read = str_of(&string);

        assert_eq!(&*read, text);
        let actual = match read {
            AxStr::Inline { .. } => Held::Inline,
            AxStr::Other(_) => Held::Other,
        };
        assert_eq!(actual, held);
    }

    /// Creating the element of an application doesn't call the application.
    #[rstest]
    fn elements_skip_the_entries_that_are_not_elements() {
        let first = Element::application(Pid::new(1));
        let last = Element::application(Pid::new(2));
        let array = CFArray::from_CFTypes(&[
            first.0.clone(),
            CFString::new("not an element").as_CFType(),
            last.0.clone(),
        ]);
        let elements = elements_of(&array.as_CFType());

        assert_eq!(elements.entries(), 3);
        assert_eq!(elements.clone().collect::<Vec<_>>(), [first.clone(), last.clone()]);
        assert_eq!(elements.rev().collect::<Vec<_>>(), [last, first]);
    }

    #[rstest]
    fn the_elements_of_a_string_are_empty() {
        let elements = elements_of(&CFString::new("text").as_CFType());
        assert_eq!(elements.entries(), 0);
        assert_eq!(elements.count(), 0);
    }
}

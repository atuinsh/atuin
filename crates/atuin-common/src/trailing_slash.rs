use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::path::{self, MAIN_SEPARATOR, MAIN_SEPARATOR_STR, Path};

/// The platform path separator as a single byte. `MAIN_SEPARATOR` is always
/// ASCII (`/` or `\`), so this is exact and its byte can be matched against
/// the encoded form of any `OsStr` (ASCII bytes never occur inside a
/// multi-byte sequence in UTF-8 or WTF-8).
const SEPARATOR_BYTE: u8 = MAIN_SEPARATOR as u8;

/// A path-like value guaranteed to end with the platform separator
/// (`std::path::MAIN_SEPARATOR`), kept in its original OS-native encoding.
///
/// The trailing separator is a **prefix-boundary marker**: it lets directory
/// keys be compared with a `starts_with` prefix test without a shorter path
/// spuriously matching a sibling — without it, prefix `/home/user` would match
/// `/home/user-other`. It is deliberately NOT `Path` normalization (`Path`
/// treats trailing separators as insignificant).
///
/// The value is held as `Cow<OsStr>`, so non-UTF-8 paths are preserved
/// losslessly — read it back with [`as_os_str`](Self::as_os_str),
/// [`as_path`](Self::as_path), or the `AsRef` impls.
///
/// Following [`Path`], this type does **not** implement [`Display`](std::fmt::Display):
/// rendering to text can be lossy, so it is opt-in via [`display`](Self::display)
/// (which returns [`std::path::Display`], exactly like [`Path::display`]). The
/// daemon search index calls `.display().to_string()` to build its UTF-8 `String`
/// keys; its directory data is already UTF-8, so no loss occurs there in practice.
///
/// Construct one via [`TrailingSlashExt::with_trailing_slash`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrailingSlash<'a>(Cow<'a, OsStr>);

impl TrailingSlash<'_> {
    /// The value as an `&OsStr`, in its original encoding (lossless).
    pub fn as_os_str(&self) -> &OsStr {
        &self.0
    }

    /// The value as an `&Path` (lossless).
    pub fn as_path(&self) -> &Path {
        Path::new(self.as_os_str())
    }

    /// A [`Display`](std::fmt::Display)able adapter, mirroring [`Path::display`].
    /// Rendering substitutes U+FFFD for any non-UTF-8 bytes — the single,
    /// explicit lossy conversion.
    pub fn display(&self) -> path::Display<'_> {
        self.as_path().display()
    }
}

impl AsRef<OsStr> for TrailingSlash<'_> {
    fn as_ref(&self) -> &OsStr {
        self.as_os_str()
    }
}

impl AsRef<Path> for TrailingSlash<'_> {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

/// Extension providing [`with_trailing_slash`](TrailingSlashExt::with_trailing_slash)
/// on string- and path-like values.
pub trait TrailingSlashExt {
    /// Returns a view of `self` with a guaranteed trailing platform separator,
    /// appending one only if not already present. The original encoding is
    /// preserved; no UTF-8 conversion occurs.
    fn with_trailing_slash(&self) -> TrailingSlash<'_>;
}

impl TrailingSlashExt for OsStr {
    fn with_trailing_slash(&self) -> TrailingSlash<'_> {
        if self.as_encoded_bytes().last() == Some(&SEPARATOR_BYTE) {
            TrailingSlash(Cow::Borrowed(self))
        } else {
            let mut owned = OsString::with_capacity(self.len() + MAIN_SEPARATOR_STR.len());
            owned.push(self);
            owned.push(MAIN_SEPARATOR_STR);
            TrailingSlash(Cow::Owned(owned))
        }
    }
}

impl TrailingSlashExt for Path {
    fn with_trailing_slash(&self) -> TrailingSlash<'_> {
        self.as_os_str().with_trailing_slash()
    }
}

impl TrailingSlashExt for str {
    fn with_trailing_slash(&self) -> TrailingSlash<'_> {
        OsStr::new(self).with_trailing_slash()
    }
}

#[cfg(test)]
mod tests {
    use super::TrailingSlashExt;
    use std::ffi::OsStr;
    use std::path::{MAIN_SEPARATOR, MAIN_SEPARATOR_STR, Path, PathBuf};

    #[test]
    fn appends_separator_when_missing() {
        assert_eq!(
            "foo".with_trailing_slash().display().to_string(),
            format!("foo{MAIN_SEPARATOR_STR}")
        );
    }

    #[test]
    fn leaves_existing_trailing_separator_untouched() {
        let already = format!("foo{MAIN_SEPARATOR_STR}");
        assert_eq!(already.with_trailing_slash().display().to_string(), already);
    }

    #[test]
    fn empty_string_becomes_bare_separator() {
        assert_eq!(
            "".with_trailing_slash().display().to_string(),
            MAIN_SEPARATOR_STR.to_string()
        );
    }

    #[test]
    fn works_via_string_auto_deref() {
        let owned = String::from("bar");
        assert_eq!(
            owned.with_trailing_slash().display().to_string(),
            format!("bar{MAIN_SEPARATOR_STR}")
        );
    }

    #[test]
    fn works_for_path_and_pathbuf() {
        let p: &Path = Path::new("baz");
        assert_eq!(
            p.with_trailing_slash().display().to_string(),
            format!("baz{MAIN_SEPARATOR_STR}")
        );

        let pb = PathBuf::from("qux");
        assert_eq!(
            pb.with_trailing_slash().display().to_string(),
            format!("qux{MAIN_SEPARATOR_STR}")
        );
    }

    #[test]
    fn as_path_round_trips_without_allocation_when_already_terminated() {
        let already = format!("dir{MAIN_SEPARATOR_STR}");
        let ts = OsStr::new(&already).with_trailing_slash();
        assert_eq!(ts.as_path(), Path::new(&already));
    }

    // A non-UTF-8 path (a lone continuation byte 0x80 is invalid UTF-8) must be
    // carried through byte-for-byte, with only the ASCII separator appended —
    // no lossy U+FFFD substitution. Unix-only because that is where `OsStr` can
    // hold arbitrary bytes.
    #[cfg(unix)]
    #[test]
    fn preserves_non_utf8_bytes_losslessly() {
        use std::os::unix::ffi::OsStrExt;

        let raw = OsStr::from_bytes(&[0x66, 0x80, 0x6f]); // "f", invalid byte, "o"
        let out = raw.with_trailing_slash();

        let mut expected = raw.as_encoded_bytes().to_vec();
        expected.push(MAIN_SEPARATOR as u8);

        assert_eq!(out.as_os_str().as_encoded_bytes(), expected.as_slice());
    }
}

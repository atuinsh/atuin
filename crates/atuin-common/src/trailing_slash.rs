use std::borrow::Cow;
use std::fmt::{self, Display};
use std::path::{MAIN_SEPARATOR_STR, Path};

/// A view over a path-like value that renders with a guaranteed trailing
/// path separator (`std::path::MAIN_SEPARATOR`).
///
/// The trailing separator is a **prefix-boundary marker**, not a filesystem
/// operation: it lets directory strings be compared with `str::starts_with`
/// without a shorter path spuriously matching a sibling — e.g. without it,
/// prefix `/home/user` would match `/home/user-other`. `Path`/`PathBuf`
/// deliberately treat trailing separators as insignificant, so this
/// intentionally renders to a string via `Display` rather than returning a
/// `Path`.
///
/// Construct one via [`TrailingSlashExt::with_trailing_slash`].
pub struct TrailingSlash<'a>(Cow<'a, str>);

impl Display for TrailingSlash<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)?;
        if !self.0.ends_with(MAIN_SEPARATOR_STR) {
            f.write_str(MAIN_SEPARATOR_STR)?;
        }
        Ok(())
    }
}

/// Extension providing [`with_trailing_slash`](TrailingSlashExt::with_trailing_slash)
/// on string- and path-like values.
pub trait TrailingSlashExt {
    /// Returns a [`Display`]able view of `self` with a guaranteed trailing
    /// platform separator. Appends one only if not already present.
    fn with_trailing_slash(&self) -> TrailingSlash<'_>;
}

impl TrailingSlashExt for str {
    fn with_trailing_slash(&self) -> TrailingSlash<'_> {
        TrailingSlash(Cow::Borrowed(self))
    }
}

impl TrailingSlashExt for Path {
    fn with_trailing_slash(&self) -> TrailingSlash<'_> {
        // Paths are not guaranteed UTF-8; render lossily. Directory keys used
        // by the search index originate as UTF-8 `String`s, so this branch is
        // only exercised by genuine `Path`/`PathBuf` callers.
        TrailingSlash(self.to_string_lossy())
    }
}

#[cfg(test)]
mod tests {
    use super::TrailingSlashExt;
    use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};

    #[test]
    fn appends_separator_when_missing() {
        assert_eq!(
            "foo".with_trailing_slash().to_string(),
            format!("foo{MAIN_SEPARATOR_STR}")
        );
    }

    #[test]
    fn leaves_existing_trailing_separator_untouched() {
        let already = format!("foo{MAIN_SEPARATOR_STR}");
        assert_eq!(already.with_trailing_slash().to_string(), already);
    }

    #[test]
    fn empty_string_becomes_bare_separator() {
        assert_eq!(
            "".with_trailing_slash().to_string(),
            MAIN_SEPARATOR_STR.to_string()
        );
    }

    #[test]
    fn works_via_string_auto_deref() {
        let owned = String::from("bar");
        assert_eq!(
            owned.with_trailing_slash().to_string(),
            format!("bar{MAIN_SEPARATOR_STR}")
        );
    }

    #[test]
    fn works_for_path_and_pathbuf() {
        let p: &Path = Path::new("baz");
        assert_eq!(
            p.with_trailing_slash().to_string(),
            format!("baz{MAIN_SEPARATOR_STR}")
        );

        let pb = PathBuf::from("qux");
        assert_eq!(
            pb.with_trailing_slash().to_string(),
            format!("qux{MAIN_SEPARATOR_STR}")
        );
    }
}

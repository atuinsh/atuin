//! General-purpose URL utilities.

use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UrlAppendError {
    /// The URL given cannot be a base URL (e.g. `mailto:` or `data:`).
    ///
    /// These are usually URLs which do not support file-path-like semantics.
    #[error("URL cannot be a base: it has no hierarchical path to append segments to")]
    NonSplittable,

    #[error("path segments cannot be . or ..")]
    DotSegment,
}

/// Extension methods for [`Url`].
pub trait UrlAppendExt {
    /// Append `segments`, each as its own percent-encoded path segment.
    ///
    /// Unlike [`Url::join`], which drops a base's `/prefix` (see example) this always appends.
    /// Note that `/` are encoded to `%2F` rather than treated as a separator.
    ///
    /// ```
    /// use atuin_common::url::UrlAppendExt;
    ///
    /// let base = url::Url::parse("https://host.example/atuin").unwrap();
    ///
    /// assert_eq!(base.append(["foo/bar"])?.as_str(), "https://host.example/atuin/foo%2Fbar");
    /// assert_eq!(base.append(["foo", "bar"])?.as_str(), "https://host.example/atuin/foo/bar");
    ///
    /// // Url::join behavior:
    /// assert_eq!(base.join("foo/bar").unwrap().as_str(), "https://host.example/foo/bar");
    /// # Ok::<(), atuin_common::url::UrlAppendError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// See [`UrlAppendError`].
    fn append<I, S>(&self, segments: I) -> Result<Url, UrlAppendError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>;

    /// Append a fixed, multi-segment `path` in which `/` separates segments.
    ///
    /// It is your responsibility to ensure you do not want `/` characters to be encoded. See
    /// [`UrlAppendExt::append`] for the safer version.
    ///
    /// ```
    /// use atuin_common::url::UrlAppendExt;
    ///
    /// let base = url::Url::parse("https://host.example/atuin").unwrap();
    /// assert_eq!(
    ///     base.append_path("api/v0/me")?.as_str(),
    ///     "https://host.example/atuin/api/v0/me",
    /// );
    /// # Ok::<(), atuin_common::url::UrlAppendError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// See [`UrlAppendError`].
    fn append_path(&self, path: &'static str) -> Result<Url, UrlAppendError> {
        self.append(path.split('/').filter(|s| !s.is_empty()))
    }
}

impl UrlAppendExt for Url {
    fn append<I, S>(&self, segments: I) -> Result<Url, UrlAppendError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let trailing_empty = self.path_segments().map_or(0, |segments| {
            segments.rev().take_while(|s| s.is_empty()).count()
        });

        let mut url = self.clone();
        let mut path = url
            .path_segments_mut()
            .map_err(|()| UrlAppendError::NonSplittable)?;

        for _ in 0..trailing_empty {
            path.pop_if_empty();
        }

        for segment in segments {
            let segment = segment.as_ref();
            if matches!(segment, "." | "..") {
                return Err(UrlAppendError::DotSegment);
            }
            path.push(segment);
        }
        drop(path);

        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn parse(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[rstest]
    #[case("https://api.atuin.sh", "me", "https://api.atuin.sh/me")]
    #[case("https://api.atuin.sh/", "me", "https://api.atuin.sh/me")]
    #[case("https://host.example/atuin", "api", "https://host.example/atuin/api")]
    #[case("https://host.example/atuin/", "api", "https://host.example/atuin/api")]
    #[case("https://h.example", "a/b", "https://h.example/a%2Fb")]
    #[case(
        "https://h.example/x",
        "../../etc",
        "https://h.example/x/..%2F..%2Fetc"
    )]
    #[case("https://h.example", "john doe", "https://h.example/john%20doe")]
    #[case("https://h.example", "a?b#c", "https://h.example/a%3Fb%23c")]
    #[case("https://h.example:8443/x", "y", "https://h.example:8443/x/y")]
    #[case(
        "https://host.example/atuin//",
        "api",
        "https://host.example/atuin/api"
    )]
    #[case(
        "https://host.example/atuin////",
        "api",
        "https://host.example/atuin/api"
    )]
    #[case("https://h.example//", "me", "https://h.example/me")]
    #[case("https://h.example////", "me", "https://h.example/me")]
    #[case("https://h.example/a//b", "me", "https://h.example/a//b/me")]
    #[case("https://h.example/a//b//", "me", "https://h.example/a//b/me")]
    fn append_cases(#[case] base: &str, #[case] segment: &str, #[case] expected: &str) {
        assert_eq!(parse(base).append([segment]).unwrap().as_str(), expected);
    }

    #[rstest]
    #[case(".")]
    #[case("..")]
    fn dot_segments_are_rejected(#[case] segment: &str) {
        assert_eq!(
            parse("https://host.example/atuin").append([segment]),
            Err(UrlAppendError::DotSegment),
        );
        assert_eq!(
            parse("https://host.example/atuin").append(["user", segment]),
            Err(UrlAppendError::DotSegment),
        );
    }

    #[test]
    fn dot_segments_are_rejected_by_append_path() {
        assert_eq!(
            parse("https://h.example").append_path("api/../v0"),
            Err(UrlAppendError::DotSegment),
        );
    }

    #[test]
    fn dot_lookalikes_are_still_appended() {
        assert_eq!(
            parse("https://h.example")
                .append(["...", ".hidden", "a.b"])
                .unwrap()
                .as_str(),
            "https://h.example/.../.hidden/a.b",
        );
    }

    #[test]
    fn append_encodes_each_segment_independently() {
        // Two segments: each is encoded on its own, so the space in "john doe"
        // becomes %20 but does not merge with "user".
        assert_eq!(
            parse("https://h.example")
                .append(["user", "john doe"])
                .unwrap()
                .as_str(),
            "https://h.example/user/john%20doe",
        );

        // A `/` inside one element still doesn't act as a separator: it's
        // encoded to %2F, landing as part of that single segment.
        assert_eq!(
            parse("https://h.example")
                .append(["a", "b/c"])
                .unwrap()
                .as_str(),
            "https://h.example/a/b%2Fc",
        );
    }

    #[rstest]
    #[case("https://api.atuin.sh", "api/v0/me", "https://api.atuin.sh/api/v0/me")]
    #[case(
        "https://host.example/atuin",
        "api/v0/me",
        "https://host.example/atuin/api/v0/me"
    )]
    #[case(
        "https://host.example/atuin/",
        "api/v0/me",
        "https://host.example/atuin/api/v0/me"
    )]
    #[case("https://h.example", "/api/v0/me", "https://h.example/api/v0/me")]
    #[case("https://h.example", "account", "https://h.example/account")]
    #[case("https://h.example//", "api/v0/me", "https://h.example/api/v0/me")]
    #[case(
        "https://host.example/atuin//",
        "api/v0/me",
        "https://host.example/atuin/api/v0/me"
    )]
    fn append_path_cases(#[case] base: &str, #[case] path: &'static str, #[case] expected: &str) {
        assert_eq!(parse(base).append_path(path).unwrap().as_str(), expected);
    }

    #[test]
    fn cannot_be_a_base_url_is_an_error() {
        let url = parse("mailto:me@example.com");
        assert_eq!(url.append(["x"]), Err(UrlAppendError::NonSplittable));
        assert_eq!(url.append_path("a/b"), Err(UrlAppendError::NonSplittable));
    }
}

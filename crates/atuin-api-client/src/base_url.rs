use url::Url;

use crate::{Client, HookState};

/// Why a URL cannot be a [`Client`]'s base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BaseUrlError {
    #[error("a base URL needs a hierarchical path, like https://host/prefix")]
    CannotBeABase,
}

impl Client {
    /// Build a client for the server at `base` that sends every request through `http`.
    ///
    /// `base` keeps its path prefix and drops trailing slashes, as `UrlAppendExt::append_path`
    /// does, so a self-hosted server can live under `https://host/atuin/`. Its query goes ahead of
    /// every operation's own, and its fragment is never sent.
    ///
    /// # Errors
    ///
    /// [`BaseUrlError`] when paths cannot follow `base`.
    pub fn from_http(base: &Url, http: reqwest::Client) -> Result<Self, BaseUrlError> {
        let Base { prefix, query } = Base::parse(base)?;
        #[allow(clippy::disallowed_methods, reason = "the one place that normalises the base")]
        Ok(Self::new_with_client(&prefix, http, HookState::for_base(query)))
    }
}

/// A base URL split the way progenitor needs it.
#[derive(Debug, PartialEq, Eq)]
struct Base {
    /// The prefix the generated code formats operation paths onto, e.g. `https://host/atuin`.
    ///
    /// It formats `{prefix}/api/v0/me` as a string, so a trailing slash would double and a query
    /// would land before the path.
    prefix: String,
    /// The base's query, e.g. `tok=1`, if it has a non-empty one.
    query: Option<String>,
}

impl Base {
    fn parse(base: &Url) -> Result<Self, BaseUrlError> {
        if base.cannot_be_a_base() {
            return Err(BaseUrlError::CannotBeABase);
        }
        let mut prefix = base.clone();
        prefix.set_query(None);
        prefix.set_fragment(None);
        Ok(Self {
            prefix: prefix.as_str().trim_end_matches('/').to_owned(),
            query: base.query().filter(|query| !query.is_empty()).map(str::to_owned),
        })
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::url::UrlAppendExt;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use url::Url;

    use super::{Base, BaseUrlError};

    #[rstest]
    fn prefixes_operation_paths_like_append_path(
        #[values(
            "https://api.atuin.sh",
            "https://api.atuin.sh/",
            "https://host.example/atuin",
            "https://host.example/atuin/",
            "https://host.example/atuin//",
            "https://host.example/atuin////",
            "https://h.example//",
            "https://h.example/a//b",
            "https://h.example/a//b//",
            "https://h.example:8443/x",
            "http://127.0.0.1:8888/nested/prefix/",
            "https://host.example/atuin/?tok=1",
            "https://host.example/atuin#top",
            "https://host.example/atuin/?tok=1&b=%20#top"
        )]
        base: &str,
        #[values("api/v0/me", "user/ellie")] path: &'static str,
    ) {
        let base = Url::parse(base).unwrap();
        let Base { prefix, query } = Base::parse(&base).unwrap();
        let mut expected = base.append_path(path).unwrap();
        expected.set_fragment(None);

        let mut built = Url::parse(&format!("{prefix}/{path}")).unwrap();
        built.set_query(query.as_deref());

        assert_eq!(built.as_str(), expected.as_str());
    }

    #[rstest]
    #[case::bare("https://host.example/atuin", None)]
    #[case::empty("https://host.example/atuin?", None)]
    #[case::query("https://host.example/atuin/?tok=1&b=%20", Some("tok=1&b=%20"))]
    fn keeps_the_query_apart(#[case] base: &str, #[case] query: Option<&str>) {
        let base = Base::parse(&Url::parse(base).unwrap()).unwrap();

        assert_eq!(base.query.as_deref(), query);
    }

    #[rstest]
    fn rejects_urls_paths_cannot_follow() {
        let base = Url::parse("mailto:me@example.com").unwrap();

        assert_eq!(Base::parse(&base), Err(BaseUrlError::CannotBeABase));
    }
}

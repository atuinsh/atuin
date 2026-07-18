use std::fmt;
use std::path::{MAIN_SEPARATOR, MAIN_SEPARATOR_STR, Path, PathBuf};

/// A [`Display`](fmt::Display) adapter for a path with optional enrichments, built via
/// [`DisplayRichExt::display_rich`].
///
/// With no options set, the `Display` output is byte-identical to [`Path::display`]. Options layer
/// on top, in precedence order:
///
/// - [`relative_to`](Self::relative_to): if the path is under `base`, render relative to `base`.
/// - [`tilde`](Self::tilde): if the path is under `home`, render it as `~` + separator + remainder.
/// - [`trailing_slash`](Self::trailing_slash): the rendered text ends with the platform separator.
///
/// Like [`Path::display`], rendering is lossy for non-UTF-8 paths.
#[derive(Clone, Debug)]
pub struct RichDisplay<'a> {
    path: &'a Path,
    trailing_slash: bool,
    relative_to: Option<PathBuf>,
    tilde: Option<PathBuf>,
}

impl<'a> RichDisplay<'a> {
    fn new(path: &'a Path) -> Self {
        Self {
            path,
            trailing_slash: false,
            relative_to: None,
            tilde: None,
        }
    }

    #[must_use]
    pub fn trailing_slash(self, enabled: bool) -> Self {
        Self {
            trailing_slash: enabled,
            ..self
        }
    }

    #[must_use]
    pub fn relative_to(self, base: impl AsRef<Path>) -> Self {
        Self {
            relative_to: Some(base.as_ref().to_path_buf()),
            ..self
        }
    }

    /// If the path is under `home` (and no `relative_to` base matched), render it
    /// as `~` + separator + remainder. Off by default.
    #[must_use]
    pub fn tilde(self, home: impl AsRef<Path>) -> Self {
        Self {
            tilde: Some(home.as_ref().to_path_buf()),
            ..self
        }
    }
}

impl fmt::Display for RichDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Resolve the body to render: the (possibly stripped) path, and whether
        // a `~` prefix precedes it. `relative_to` wins over `tilde`.
        let (tilde, body) = if let Some(base) = &self.relative_to
            && let Ok(rel) = self.path.strip_prefix(base)
        {
            (false, rel)
        } else if let Some(home) = &self.tilde
            && let Ok(rel) = self.path.strip_prefix(home)
        {
            (true, rel)
        } else {
            (false, self.path)
        };

        if tilde {
            f.write_str("~")?;
            f.write_str(MAIN_SEPARATOR_STR)?;
        }
        write!(f, "{}", body.display())?;

        let ends_with_separator = match body.as_os_str().as_encoded_bytes().last() {
            Some(&byte) => byte == MAIN_SEPARATOR as u8,
            None => tilde,
        };

        if self.trailing_slash && !ends_with_separator {
            f.write_str(MAIN_SEPARATOR_STR)?;
        }

        Ok(())
    }
}

/// Extension adding [`display_rich`](DisplayRichExt::display_rich) to any
/// path-like value.
pub trait DisplayRichExt {
    /// Returns a [`RichDisplay`] builder for this path.
    fn display_rich(&self) -> RichDisplay<'_>;
}

impl<T: AsRef<Path> + ?Sized> DisplayRichExt for T {
    fn display_rich(&self) -> RichDisplay<'_> {
        RichDisplay::new(self.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::DisplayRichExt;
    use rstest::rstest;
    use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};

    /// With no options, output is byte-identical to `Path::display()`.
    #[rstest]
    #[case(Path::new("relative").join("some").join("dir"))]
    #[case(Path::new("etc").join("hosts"))]
    fn base_output_matches_path_display(#[case] path: PathBuf) {
        assert_eq!(path.display_rich().to_string(), path.display().to_string());
    }

    /// `trailing_slash(true)` appends the platform separator only when missing;
    /// `trailing_slash(false)` leaves the base output untouched.
    #[rstest]
    #[case::appends_when_missing("foo".to_string(), true, format!("foo{MAIN_SEPARATOR_STR}"))]
    #[case::idempotent_when_terminated(
        format!("foo{MAIN_SEPARATOR_STR}"),
        true,
        format!("foo{MAIN_SEPARATOR_STR}")
    )]
    #[case::disabled_is_base("foo".to_string(), false, "foo".to_string())]
    #[case::empty_becomes_bare_separator(String::new(), true, MAIN_SEPARATOR_STR.to_string())]
    fn trailing_slash_rendering(
        #[case] input: String,
        #[case] enabled: bool,
        #[case] expected: String,
    ) {
        assert_eq!(
            input.display_rich().trailing_slash(enabled).to_string(),
            expected
        );
    }

    /// `display_rich()` is available on every `AsRef<Path>` input type.
    #[rstest]
    fn works_for_all_asref_path_types() {
        let expected = format!("bar{MAIN_SEPARATOR_STR}");
        assert_eq!(
            "bar".display_rich().trailing_slash(true).to_string(),
            expected
        );
        assert_eq!(
            String::from("bar")
                .display_rich()
                .trailing_slash(true)
                .to_string(),
            expected
        );
        assert_eq!(
            Path::new("bar")
                .display_rich()
                .trailing_slash(true)
                .to_string(),
            expected
        );
        assert_eq!(
            PathBuf::from("bar")
                .display_rich()
                .trailing_slash(true)
                .to_string(),
            expected
        );
    }

    /// `relative_to(base)` strips `base` when the path is under it (empty when
    /// equal), and passes the path through otherwise.
    #[rstest]
    #[case::strips_prefix(
        Path::new("home").join("user").join("project").join("src"),
        Path::new("home").join("user"),
        Path::new("project").join("src").display().to_string()
    )]
    #[case::base_itself_is_empty(
        Path::new("home").join("user"),
        Path::new("home").join("user"),
        String::new()
    )]
    #[case::passes_through_when_not_under_base(
        Path::new("etc").join("hosts"),
        Path::new("home").join("user"),
        Path::new("etc").join("hosts").display().to_string()
    )]
    fn relative_to_rendering(
        #[case] path: PathBuf,
        #[case] base: PathBuf,
        #[case] expected: String,
    ) {
        assert_eq!(path.display_rich().relative_to(&base).to_string(), expected);
    }

    /// `tilde(home)` renders `~` + separator + remainder when under `home`
    /// (`~` + separator for `home` itself), and passes through otherwise.
    #[rstest]
    #[case::abbreviates_home(
        Path::new("home").join("user").join("project"),
        Path::new("home").join("user"),
        format!("~{MAIN_SEPARATOR_STR}project")
    )]
    #[case::home_root_is_tilde_separator(
        Path::new("home").join("user"),
        Path::new("home").join("user"),
        format!("~{MAIN_SEPARATOR_STR}")
    )]
    #[case::passes_through_when_not_under_home(
        Path::new("etc").join("hosts"),
        Path::new("home").join("user"),
        Path::new("etc").join("hosts").display().to_string()
    )]
    fn tilde_rendering(#[case] path: PathBuf, #[case] home: PathBuf, #[case] expected: String) {
        assert_eq!(path.display_rich().tilde(&home).to_string(), expected);
    }

    /// `relative_to` takes precedence over `tilde` when both match.
    #[rstest]
    fn relative_to_takes_priority_over_tilde() {
        let dir = Path::new("home").join("user");
        let p = dir.join("proj");
        assert_eq!(
            p.display_rich().relative_to(&dir).tilde(&dir).to_string(),
            "proj"
        );
    }

    /// Enrichments compose with `trailing_slash`. The `relative_to`-to-base
    /// case yields an empty body, so `trailing_slash` renders a bare separator
    /// — pinning that edge, which no current caller reaches.
    #[rstest]
    #[case::tilde(
        Path::new("home").join("user").join("proj"),
        None,
        Some(Path::new("home").join("user")),
        format!("~{MAIN_SEPARATOR_STR}proj{MAIN_SEPARATOR_STR}")
    )]
    #[case::relative_to_base_itself(
        Path::new("home").join("user"),
        Some(Path::new("home").join("user")),
        None,
        MAIN_SEPARATOR_STR.to_string()
    )]
    fn composes_with_trailing_slash(
        #[case] path: PathBuf,
        #[case] relative_to: Option<PathBuf>,
        #[case] tilde: Option<PathBuf>,
        #[case] expected: String,
    ) {
        let mut d = path.display_rich();
        if let Some(base) = &relative_to {
            d = d.relative_to(base);
        }
        if let Some(home) = &tilde {
            d = d.tilde(home);
        }
        assert_eq!(d.trailing_slash(true).to_string(), expected);
    }
}

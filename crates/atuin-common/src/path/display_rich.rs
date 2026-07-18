use std::fmt;
use std::path::{MAIN_SEPARATOR, MAIN_SEPARATOR_STR, Path, PathBuf};

/// The platform path separator as a single byte. `MAIN_SEPARATOR` is always
/// ASCII (`/` or `\`), so its byte can be matched against the encoded form of
/// any `OsStr` — an ASCII byte never occurs inside a multi-byte UTF-8/WTF-8
/// sequence.
const SEPARATOR_BYTE: u8 = MAIN_SEPARATOR as u8;

/// A [`Display`](fmt::Display) adapter for a path with optional enrichments,
/// built via [`DisplayRichExt::display_rich`].
///
/// With no options set, the `Display` output is byte-identical to
/// [`Path::display`]. Options layer on top, in precedence order:
///
/// - [`relative_to`](Self::relative_to): if the path is under `base`, render it
///   relative to `base` (highest priority).
/// - [`tilde`](Self::tilde): otherwise, if the path is under `home`, render it
///   as `~` + separator + remainder.
/// - [`trailing_slash`](Self::trailing_slash): ensure the rendered text ends
///   with the platform separator.
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

    /// Ensure the rendered path ends with the platform separator, appending one
    /// only if not already present. Off by default.
    #[must_use]
    pub fn trailing_slash(mut self, enabled: bool) -> Self {
        self.trailing_slash = enabled;
        self
    }

    /// If the path is under `base`, render it relative to `base`. Takes priority
    /// over [`tilde`](Self::tilde). Off by default.
    #[must_use]
    pub fn relative_to(mut self, base: impl AsRef<Path>) -> Self {
        self.relative_to = Some(base.as_ref().to_path_buf());
        self
    }

    /// If the path is under `home` (and no `relative_to` base matched), render it
    /// as `~` + separator + remainder. Off by default.
    #[must_use]
    pub fn tilde(mut self, home: impl AsRef<Path>) -> Self {
        self.tilde = Some(home.as_ref().to_path_buf());
        self
    }

    /// Resolves the body to render: the (possibly stripped) path, and whether a
    /// `~` prefix precedes it. `relative_to` wins over `tilde`.
    fn body(&self) -> (bool, &Path) {
        if let Some(base) = &self.relative_to
            && let Ok(rel) = self.path.strip_prefix(base)
        {
            return (false, rel);
        }
        if let Some(home) = &self.tilde
            && let Ok(rel) = self.path.strip_prefix(home)
        {
            return (true, rel);
        }
        (false, self.path)
    }
}

impl fmt::Display for RichDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (tilde, body) = self.body();

        if tilde {
            f.write_str("~")?;
            f.write_str(MAIN_SEPARATOR_STR)?;
        }
        write!(f, "{}", body.display())?;

        if self.trailing_slash && !ends_with_separator(tilde, body) {
            f.write_str(MAIN_SEPARATOR_STR)?;
        }

        Ok(())
    }
}

/// Whether the rendered text (a possible `~<sep>` prefix plus `body`) already
/// ends with the platform separator. An empty body ends in a separator only
/// when the `~<sep>` prefix was written.
fn ends_with_separator(tilde_prefix: bool, body: &Path) -> bool {
    match body.as_os_str().as_encoded_bytes().last() {
        Some(&byte) => byte == SEPARATOR_BYTE,
        None => tilde_prefix,
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
    use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};

    #[test]
    fn base_output_matches_path_display() {
        let p = Path::new("relative").join("some").join("dir");
        assert_eq!(p.display_rich().to_string(), p.display().to_string());
    }

    #[test]
    fn trailing_slash_appends_when_missing() {
        assert_eq!(
            "foo".display_rich().trailing_slash(true).to_string(),
            format!("foo{MAIN_SEPARATOR_STR}")
        );
    }

    #[test]
    fn trailing_slash_is_idempotent_when_already_terminated() {
        let already = format!("foo{MAIN_SEPARATOR_STR}");
        assert_eq!(
            already.display_rich().trailing_slash(true).to_string(),
            already
        );
    }

    #[test]
    fn trailing_slash_false_is_base_output() {
        assert_eq!(
            "foo".display_rich().trailing_slash(false).to_string(),
            "foo"
        );
    }

    #[test]
    fn empty_with_trailing_slash_is_bare_separator() {
        assert_eq!(
            "".display_rich().trailing_slash(true).to_string(),
            MAIN_SEPARATOR_STR
        );
    }

    #[test]
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

    #[test]
    fn relative_to_strips_prefix() {
        let base = Path::new("home").join("user");
        let p = base.join("project").join("src");
        assert_eq!(
            p.display_rich().relative_to(&base).to_string(),
            Path::new("project").join("src").display().to_string()
        );
    }

    #[test]
    fn relative_to_base_itself_renders_empty() {
        let base = Path::new("home").join("user");
        assert_eq!(base.display_rich().relative_to(&base).to_string(), "");
    }

    #[test]
    fn relative_to_passes_through_when_not_under_base() {
        let base = Path::new("home").join("user");
        let p = Path::new("etc").join("hosts");
        assert_eq!(
            p.display_rich().relative_to(&base).to_string(),
            p.display().to_string()
        );
    }

    #[test]
    fn tilde_abbreviates_home() {
        let home = Path::new("home").join("user");
        let p = home.join("project");
        assert_eq!(
            p.display_rich().tilde(&home).to_string(),
            format!("~{MAIN_SEPARATOR_STR}project")
        );
    }

    #[test]
    fn tilde_of_home_root_is_tilde_separator() {
        let home = Path::new("home").join("user");
        assert_eq!(
            home.display_rich().tilde(&home).to_string(),
            format!("~{MAIN_SEPARATOR_STR}")
        );
    }

    #[test]
    fn tilde_passes_through_when_not_under_home() {
        let home = Path::new("home").join("user");
        let p = Path::new("etc").join("hosts");
        assert_eq!(
            p.display_rich().tilde(&home).to_string(),
            p.display().to_string()
        );
    }

    #[test]
    fn relative_to_takes_priority_over_tilde() {
        let dir = Path::new("home").join("user");
        let p = dir.join("proj");
        assert_eq!(
            p.display_rich().relative_to(&dir).tilde(&dir).to_string(),
            "proj"
        );
    }

    #[test]
    fn tilde_composes_with_trailing_slash() {
        let home = Path::new("home").join("user");
        let p = home.join("proj");
        assert_eq!(
            p.display_rich()
                .tilde(&home)
                .trailing_slash(true)
                .to_string(),
            format!("~{MAIN_SEPARATOR_STR}proj{MAIN_SEPARATOR_STR}")
        );
    }
}

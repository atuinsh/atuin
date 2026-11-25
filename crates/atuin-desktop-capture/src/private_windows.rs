//! Tells the windows of a browser's private browsing mode apart from the browser's other
//! windows.
//!
//! No platform marks a private window in a common way, so this module holds what each browser
//! does:
//!
//! - Firefox gives only a private window the button that clears the private session. The
//!   platform backends look for [`FIREFOX_MARKER_ID`] near the top of the window. Each
//!   platform's quirks also give the English titles of a private window, which cover a window
//!   without the button.
//! - Safari marks a private window only in its title. The macOS backend reads the format of
//!   the title from Safari's own translations.
//! - The browsers that are built on Chromium mark a private window only in the accessible
//!   title. [`chromium_titles`] reads the browser's translations of the title.

mod chromium;
mod pak;

pub use self::chromium::chromium_titles;
#[cfg(test)]
pub use self::pak::tests as pak_tests;
use crate::backend::Error;

/// The DOM id of Firefox's button that clears the private session, which only a private
/// window has.
///
/// The user can remove the button from the toolbar, so the title is checked as well.
pub const FIREFOX_MARKER_ID: &str = "reset-pbm-toolbar-button";

/// The placeholders that stand for the page's title in a browser's format of a window title.
/// Chromium writes `$1`, and Cocoa writes `%@` or `%1$@`.
const PLACEHOLDERS: [&str; 3] = ["%1$@", "%@", "$1"];

/// Whether a window carries Firefox's marker of a private window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Present,
    Absent,
    /// The search failed.
    Unknown,
    /// The browser doesn't mark its private windows with the marker.
    Unused,
}

/// Whether a window is private, from what its check found. `title` is `None` when the
/// window's title wasn't read.
///
/// # Errors
///
/// Fails with [`Error::NoAnswer`] when the marker search failed, or when the title is needed
/// and wasn't read.
pub fn is_private(
    marker: Marker,
    title: Option<&str>,
    titles: &PrivateTitles,
) -> Result<bool, Error> {
    match marker {
        Marker::Present => return Ok(true),
        Marker::Unknown => return Err(Error::NoAnswer),
        Marker::Absent | Marker::Unused => {}
    }
    if titles.is_empty() {
        return Ok(false);
    }
    Ok(titles.matches(title.ok_or(Error::NoAnswer)?))
}

/// The titles of private windows that a browser's translations give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Translations {
    /// The translations of every language were read.
    Complete(PrivateTitles),
    /// The translations of some language couldn't be read, so the titles lack that language.
    Partial(PrivateTitles),
}

/// One format of a private window's title.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TitleFormat {
    /// The page's title with text before and after it. At least one of the two is not empty.
    Around {
        prefix: String,
        suffix: String,
    },
    /// A title without the page's title, such as the title of a window without a page.
    Exact(String),
}

impl TitleFormat {
    /// Parses a format with at most one placeholder. Returns `None` for a format that would
    /// match every title.
    fn parse(format: &str) -> Option<Self> {
        let parsed = PLACEHOLDERS.iter().find_map(|placeholder| {
            let (prefix, suffix) = format.split_once(placeholder)?;
            Some(Self::Around {
                prefix: prefix.to_owned(),
                suffix: suffix.to_owned(),
            })
        });
        match parsed.unwrap_or_else(|| Self::Exact(format.to_owned())) {
            Self::Around { prefix, suffix }
                if prefix.trim().is_empty() && suffix.trim().is_empty() =>
            {
                None
            }
            Self::Exact(title) if title.trim().is_empty() => None,
            format => Some(format),
        }
    }

    fn matches(&self, title: &str) -> bool {
        match self {
            Self::Around { prefix, suffix } => {
                title.len() > prefix.len() + suffix.len()
                    && title.starts_with(prefix.as_str())
                    && title.ends_with(suffix.as_str())
            }
            Self::Exact(exact) => title == exact,
        }
    }
}

/// The formats of the titles of one browser's private windows, in every language that the
/// browser ships.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrivateTitles {
    formats: Vec<TitleFormat>,
}

impl PrivateTitles {
    /// The titles that `formats` describe. A format holds `$1`, `%@`, or `%1$@` where the
    /// page's title goes, or no placeholder for a title that must match exactly.
    #[must_use]
    pub fn from_formats<S: AsRef<str>>(formats: impl IntoIterator<Item = S>) -> Self {
        let mut titles = Self::default();
        titles.extend(formats);
        titles
    }

    /// Adds `formats` to the titles. Formats that are already present are skipped.
    pub fn extend<S: AsRef<str>>(&mut self, formats: impl IntoIterator<Item = S>) {
        for format in formats.into_iter().filter_map(|format| TitleFormat::parse(format.as_ref())) {
            if !self.formats.contains(&format) {
                self.formats.push(format);
            }
        }
    }

    /// Whether the window titled `title` is a private window.
    #[must_use]
    pub fn matches(&self, title: &str) -> bool {
        self.formats.iter().any(|format| format.matches(title))
    }

    /// Whether the titles are empty, so that no window matches them.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.formats.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn titles() -> PrivateTitles {
        PrivateTitles::from_formats(["$1 (Incognito)"])
    }

    #[rstest]
    #[case::marker(Marker::Present, None, Ok(true))]
    #[case::marker_with_a_normal_title(Marker::Present, Some("Page"), Ok(true))]
    #[case::failed_marker_search(Marker::Unknown, Some("Page (Incognito)"), Err(()))]
    #[case::private_title(Marker::Absent, Some("Page (Incognito)"), Ok(true))]
    #[case::normal_title(Marker::Absent, Some("Page"), Ok(false))]
    #[case::unread_title(Marker::Absent, None, Err(()))]
    #[case::chromium_private(Marker::Unused, Some("Page (Incognito)"), Ok(true))]
    #[case::chromium_normal(Marker::Unused, Some("Page"), Ok(false))]
    #[case::chromium_unread_title(Marker::Unused, None, Err(()))]
    #[case::empty_title(Marker::Unused, Some(""), Ok(false))]
    fn verdicts(
        #[case] marker: Marker,
        #[case] title: Option<&str>,
        #[case] expected: Result<bool, ()>,
    ) {
        assert_eq!(is_private(marker, title, &titles()).map_err(|_| ()), expected);
    }

    #[rstest]
    fn without_titles_the_title_is_not_needed(
        #[values(Marker::Absent, Marker::Unused)] marker: Marker,
    ) {
        assert_eq!(is_private(marker, None, &PrivateTitles::default()).ok(), Some(false));
    }

    #[rstest]
    #[case::chromium("$1 (Incognito)", "Rich page - Chromium (Incognito)", true)]
    #[case::chromium_normal("$1 (Incognito)", "Rich page - Chromium", false)]
    #[case::only_the_suffix("$1 (Incognito)", " (Incognito)", false)]
    #[case::suffix_in_the_middle("$1 (Incognito)", "a (Incognito) b", false)]
    #[case::full_width_parentheses("$1（无痕）", "页面 - Brave（无痕）", true)]
    #[case::right_to_left_mark(
        "$1\u{200f} (InPrivate)",
        "דף - Microsoft Edge\u{200f} (InPrivate)",
        true
    )]
    #[case::safari("%@, Private Browsing", "#infra - Chat, Private Browsing", true)]
    #[case::safari_numbered("%1$@, Private Browsing", "Notes, Private Browsing", true)]
    #[case::safari_german("%@, „Privates Surfen“", "Notizen, „Privates Surfen“", true)]
    #[case::prefix("Private: $1", "Private: Notes", true)]
    #[case::prefix_alone("Private: $1", "Private: ", false)]
    #[case::exact("Private Browsing", "Private Browsing", true)]
    #[case::exact_with_more("Private Browsing", "Notes, Private Browsing", false)]
    fn matches_titles(#[case] format: &str, #[case] title: &str, #[case] expected: bool) {
        assert_eq!(PrivateTitles::from_formats([format]).matches(title), expected);
    }

    #[rstest]
    fn empty_titles_match_nothing() {
        let titles = PrivateTitles::from_formats(["$1", ""]);
        assert!(titles.is_empty());
        assert!(!titles.matches(""));
        assert!(!PrivateTitles::from_formats(["$1 (Incognito)"]).is_empty());
    }

    #[rstest]
    #[case::placeholder_alone("$1")]
    #[case::placeholder_with_blanks(" %@ ")]
    #[case::empty("")]
    #[case::blank("  ")]
    fn skips_formats_that_match_everything(#[case] format: &str) {
        assert_eq!(PrivateTitles::from_formats([format]), PrivateTitles::default());
    }

    #[rstest]
    fn extend_skips_duplicates() {
        let mut titles = PrivateTitles::from_formats(["$1 (Privat)"]);
        titles.extend(["$1 (Privat)", "$1 (Private)"]);
        assert_eq!(titles.formats.len(), 2);
    }
}

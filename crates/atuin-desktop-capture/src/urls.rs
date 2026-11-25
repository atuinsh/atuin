//! What the URLs of documents say about them.

/// Whether a person can visit `url`, as opposed to a page that the browser draws itself, such
/// as `about:newtab`.
pub fn is_visitable(url: &str) -> bool {
    ["http://", "https://", "file://"].iter().any(|scheme| url.starts_with(scheme))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::web("https://example.com/", true)]
    #[case::plain_web("http://example.com/", true)]
    #[case::file("file:///home/me/page.html", true)]
    #[case::new_tab("about:newtab", false)]
    #[case::browser_page("chrome://settings/", false)]
    fn pages_that_a_person_can_visit_have_a_web_or_file_scheme(
        #[case] url: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(is_visitable(url), expected);
    }
}

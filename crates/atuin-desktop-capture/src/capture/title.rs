//! Tells the titles of one document apart from those of another.

/// The words that applications add to a title while a document has unsaved changes.
const EDIT_MARKERS: [&str; 6] = ["[modified]", "[+]", "— Edited", "- Edited", "(modified)", "*"];

/// Whether two titles of one window name the same document, and differ at most in a marker
/// for unsaved changes.
///
/// An editor adds such a marker on the first key press. Without this check, every edited
/// document would get a segment for its clean state and another for its changed state.
#[must_use]
pub fn same_document(a: &str, b: &str) -> bool {
    /// Yields the words of `title` that are not markers.
    fn words(title: &str) -> impl Iterator<Item = &str> {
        let mut rest = title;
        std::iter::from_fn(move || {
            loop {
                rest = rest.trim_start_matches([' ', '●', '•']);
                if rest.is_empty() {
                    return None;
                }
                if let Some(after) = EDIT_MARKERS.iter().find_map(|m| rest.strip_prefix(m)) {
                    rest = after;
                    continue;
                }
                let end = rest
                    .char_indices()
                    .find(|&(i, c)| {
                        matches!(c, ' ' | '●' | '•')
                            || EDIT_MARKERS.iter().any(|m| rest[i..].starts_with(m))
                    })
                    .map_or(rest.len(), |(i, _)| i);
                let (word, after) = rest.split_at(end);
                rest = after;
                return Some(word);
            }
        })
    }
    words(a).eq(words(b))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::gedit("demo.rs (~/code) - gedit", "*demo.rs (~/code) - gedit", true)]
    #[case::vs_code("demo.rs - Visual Studio Code", "● demo.rs - Visual Studio Code", true)]
    #[case::kate("demo.rs — Kate", "demo.rs * — Kate", true)]
    #[case::textedit("Untitled", "Untitled — Edited", true)]
    #[case::vim("notes.txt - VIM", "notes.txt [+] - VIM", true)]
    #[case::another_file("demo.rs - gedit", "*main.rs - gedit", false)]
    #[case::another_page("Inbox (3)", "Inbox (4)", false)]
    fn titles_that_differ_in_an_edit_marker_name_one_document(
        #[case] before: &str,
        #[case] after: &str,
        #[case] same: bool,
    ) {
        assert_eq!(same_document(before, after), same);
        assert_eq!(same_document(after, before), same);
    }
}

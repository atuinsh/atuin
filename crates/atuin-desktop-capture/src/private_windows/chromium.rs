//! Reads the titles of private windows from the translations of a browser that is built on
//! Chromium.
//!
//! A browser keeps one resource pack per language. The packs name a message by a number that
//! changes from build to build, but every pack of one build gives the same message the same
//! number, so the English pack tells which number the format of a private window's title has.
//! The other packs hold that format in their languages.

use std::path::Path;

use tracing::debug;

use super::pak::{FoundText, Pak};
use super::{PrivateTitles, Translations};

/// The English formats of the accessible title of a private window. Each one is the text of
/// exactly one message in its browser's English pack.
///
/// Edge's pack also holds `$1 - [InPrivate]`, which the title doesn't use.
const ENGLISH_FORMATS: [&str; 3] = [
    // Chromium and Chrome.
    "$1 (Incognito)",
    // Edge.
    "$1 (InPrivate)",
    // Brave.
    "$1 (Private)",
];

impl PrivateTitles {
    /// The English titles of the private windows of every known browser that is built on
    /// Chromium, for a browser whose translations can't be read.
    #[must_use]
    pub fn chromium_in_english() -> Self {
        Self::from_formats(ENGLISH_FORMATS)
    }
}

/// The titles of private windows in every language of a browser, read from its English pack
/// at `english` and the packs of its other languages at `others`.
///
/// Returns `None` when the English pack can't be read. The titles are empty when the English
/// pack holds none of the known formats, as the pack of an Electron application does. The
/// translations are partial when a pack of another language can't be read. That pack adds the
/// formats that were read before its read failed.
#[must_use]
pub fn chromium_titles<P: AsRef<Path>>(
    english: &Path,
    others: impl IntoIterator<Item = P>,
) -> Option<Translations> {
    let found = match english_formats(english) {
        Ok(Some(found)) => found,
        Ok(None) => {
            debug!(path = %english.display(), "the English pack has an unknown layout");
            return None;
        }
        Err(error) => {
            debug!(path = %english.display(), %error, "the English pack can't be read");
            return None;
        }
    };
    let mut titles = PrivateTitles::from_formats(found.iter().map(|found| found.text));
    if found.is_empty() {
        return Some(Translations::Complete(titles));
    }
    let mut complete = true;
    for path in others {
        let path = path.as_ref();
        if let Err(error) = add_translations(path, &found, &mut titles) {
            debug!(path = %path.display(), %error, "a pack can't be read");
            complete = false;
        }
    }
    Some(if complete {
        Translations::Complete(titles)
    } else {
        Translations::Partial(titles)
    })
}

/// The known formats that the English pack at `path` holds. Returns `None` for a file of an
/// unknown layout.
fn english_formats(path: &Path) -> std::io::Result<Option<Vec<FoundText<'static>>>> {
    let Some(mut pak) = Pak::open(path)? else {
        return Ok(None);
    };
    pak.find_each(&ENGLISH_FORMATS).map(Some)
}

/// Adds the translations of the `found` formats in the pack at `path` to `titles`.
fn add_translations(
    path: &Path,
    found: &[FoundText<'_>],
    titles: &mut PrivateTitles,
) -> std::io::Result<()> {
    let Some(mut pak) = Pak::open(path)? else {
        return Ok(());
    };
    for found in found {
        titles.extend(pak.get(found.id)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::private_windows::pak::tests::{Built, Resource, build, dir, write};

    #[rstest]
    fn reads_the_format_in_every_language(dir: TempDir) {
        // The decoy comes first, so that a search for a part of the text would find it.
        let english = write(
            &dir,
            "en-US.pak",
            &build(
                Built::Wide,
                &[
                    Resource {
                        id: 4,
                        text: "$1 - [InPrivate]",
                    },
                    Resource {
                        id: 9,
                        text: "$1 (InPrivate)",
                    },
                ],
                &[],
            ),
        );
        let german = [Resource {
            id: 9,
            text: "$1 („InPrivate“)",
        }];
        let others = [
            write(&dir, "de.pak", &build(Built::Wide, &german, &[])),
            // A pack for a grammatical gender lacks the message.
            write(
                &dir,
                "de_FEMININE.pak",
                &build(Built::Wide, &[Resource { id: 3, text: "x" }], &[]),
            ),
            write(&dir, "broken.pak", b"not a pack"),
        ];
        let Some(Translations::Complete(titles)) = chromium_titles(&english, &others) else {
            panic!("every pack reads");
        };
        assert!(titles.matches("Seite - Microsoft Edge („InPrivate“)"));
        assert!(titles.matches("Page - Microsoft Edge (InPrivate)"));
        assert!(!titles.matches("Page - [InPrivate]"));
        assert!(!titles.matches("Page - Microsoft Edge"));
    }

    #[rstest]
    fn electron_applications_have_empty_titles(dir: TempDir) {
        let texts = [
            Resource {
                id: 1,
                text: "Copy",
            },
            Resource {
                id: 2,
                text: "Paste",
            },
        ];
        let english = write(&dir, "en-US.pak", &build(Built::Version5, &texts, &[]));
        assert_eq!(
            chromium_titles(&english, [&english]),
            Some(Translations::Complete(PrivateTitles::default()))
        );
    }

    /// For example, a package upgrade can remove a pack between the listing and the read.
    #[rstest]
    fn a_pack_that_fails_to_read_makes_the_translations_partial(dir: TempDir) {
        let english = [Resource {
            id: 9,
            text: "$1 (Incognito)",
        }];
        let german = [Resource {
            id: 9,
            text: "$1 (Inkognito)",
        }];
        let english = write(&dir, "en-US.pak", &build(Built::Version5, &english, &[]));
        let others = [
            write(&dir, "de.pak", &build(Built::Version5, &german, &[])),
            dir.path().join("missing.pak"),
        ];
        let Some(Translations::Partial(titles)) = chromium_titles(&english, &others) else {
            panic!("the translations are partial");
        };
        assert!(titles.matches("Seite - Chromium (Inkognito)"));
        assert!(titles.matches("Page - Chromium (Incognito)"));
    }

    #[rstest]
    #[case::missing(None)]
    #[case::not_a_pack(Some(b"not a pack".as_slice()))]
    fn an_unreadable_english_pack_gives_nothing(dir: TempDir, #[case] bytes: Option<&[u8]>) {
        let english = match bytes {
            Some(bytes) => write(&dir, "en-US.pak", bytes),
            None => dir.path().join("en-US.pak"),
        };
        assert_eq!(chromium_titles(&english, std::iter::empty::<&Path>()), None);
    }
}

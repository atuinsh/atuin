//! Localization of user-facing text.
//!
//! Messages are Fluent strings in `i18n/<language>/atuin.ftl`, looked up by id with [`fl!`]. `fl!`
//! fails the build if an id is missing from the `en-US` catalogue or its arguments don't match the
//! message's variables.

use std::sync::LazyLock;

use i18n_embed::LanguageLoader;
use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
use i18n_embed::unic_langid::LanguageIdentifier;

#[allow(
    clippy::same_name_method,
    reason = "RustEmbed derives inherent `get`/`iter` next to its trait methods, as sibling impls"
)]
mod embed {
    #[derive(rust_embed::RustEmbed)]
    #[folder = "i18n"]
    pub struct Localizations;
}
use embed::Localizations;

/// The message catalogue for the user's language, negotiated on first use.
pub static LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader: FluentLanguageLoader = fluent_language_loader!();

    let requested: Vec<_> = requested_language().into_iter().collect();
    if let Err(err) = i18n_embed::select(&loader, &Localizations, &requested) {
        tracing::warn!(?err, "failed to load translations; falling back to en-US");
        loader
            .load_fallback_language(&Localizations)
            .expect("fl! checks the en-US catalogue at compile time");
    }

    // Fluent wraps arguments in U+2068/U+2069 for bidi isolation; in CLI output those invisible
    // characters end up in copied text and in scripts matching on it. This only applies to
    // bundles that are already loaded, so it MUST come after loading.
    loader.set_use_isolating(false);
    loader
});

/// The language named by the first non-empty of `LC_ALL`, `LC_MESSAGES` and `LANG`, if any.
///
/// Do **not** swap this for `DesktopLanguageRequester`: on macOS it goes through CoreFoundation,
/// and a daemon that forks after CoreFoundation is initialized crashes on its next use of it.
fn requested_language() -> Option<LanguageIdentifier> {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|var| std::env::var(var).ok().filter(|locale| !locale.is_empty()))?;
    language_from_locale(&locale)
}

/// Parse a POSIX locale such as `en_US.UTF-8` into a language, if it names one (`C` does not).
fn language_from_locale(locale: &str) -> Option<LanguageIdentifier> {
    let name = locale.split(['.', '@']).next()?;
    if matches!(name, "C" | "POSIX") {
        return None;
    }
    name.replace('_', "-").parse().ok()
}

/// Look up a message in the user's language: `fl!("message-id", name = value)`.
macro_rules! fl {
    ($message_id:literal) => {
        i18n_embed_fl::fl!($crate::i18n::LOADER, $message_id)
    };
    ($message_id:literal, $($args:expr),* $(,)?) => {
        i18n_embed_fl::fl!($crate::i18n::LOADER, $message_id, $($args),*)
    };
}

/// [`write!`](std::write) a message looked up with [`fl!`].
macro_rules! writet {
    ($dst:expr, $($args:tt)+) => {
        ::std::write!($dst, "{}", $crate::i18n::fl!($($args)+))
    };
}

pub(crate) use fl;
pub(crate) use writet;

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::language_from_locale;

    #[rstest]
    #[case::language_and_region("en_US.UTF-8", Some("en-US"))]
    #[case::modifier("ga_IE@euro", Some("ga-IE"))]
    #[case::language_only("de", Some("de"))]
    #[case::c("C", None)]
    #[case::c_utf8("C.UTF-8", None)]
    #[case::posix("POSIX", None)]
    #[case::garbage("x", None)]
    fn parses_posix_locales(#[case] locale: &str, #[case] expected: Option<&str>) {
        assert_eq!(
            language_from_locale(locale).map(|language| language.to_string()),
            expected.map(str::to_owned)
        );
    }
}

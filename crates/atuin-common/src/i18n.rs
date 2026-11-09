//! Localization of user-facing text.
//!
//! Each crate that shows messages keeps a Fluent catalogue in `i18n/<language>/<crate>.ftl`,
//! declared with [`catalogue!`] as `crate::i18n::LOADER`. [`fl!`] looks a message up in it, and
//! fails the build if an id is missing from the `en-US` catalogue or its arguments don't match the
//! message's variables.

use i18n_embed::fluent::FluentLanguageLoader;
use i18n_embed::unic_langid::LanguageIdentifier;
use i18n_embed::{I18nAssets, LanguageLoader as _};
#[doc(hidden)]
pub use i18n_embed_fl as __fl;

mod catalogue {
    crate::__atuin_catalogue!();
}

pub(crate) use catalogue::LOADER;

/// Loads a crate's catalogue in the user's language.
pub trait LanguageLoaderExt {
    /// `self` with the catalogue from `assets` in the user's language, or in `en-US` without one.
    #[must_use]
    fn negotiated(self, assets: &dyn I18nAssets) -> Self;
}

impl LanguageLoaderExt for FluentLanguageLoader {
    fn negotiated(self, assets: &dyn I18nAssets) -> Self {
        /// The language named by the first non-empty of `LC_ALL`, `LC_MESSAGES` and `LANG`, if any.
        ///
        /// Do **not** swap this for `DesktopLanguageRequester`: on macOS it goes through
        /// CoreFoundation, and a daemon that forks after CoreFoundation is initialized crashes on
        /// its next use of it.
        fn requested_language() -> Option<LanguageIdentifier> {
            let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
                .into_iter()
                .find_map(|var| crate::env::var(var).ok().filter(|locale| !locale.is_empty()))?;
            language_from_locale(&locale)
        }

        /// Parse a POSIX locale such as `en_US.UTF-8` into a language, if it names one (`C` does
        /// not).
        fn language_from_locale(locale: &str) -> Option<LanguageIdentifier> {
            let name = locale.split(['.', '@']).next()?;
            if matches!(name, "C" | "POSIX") {
                return None;
            }
            name.replace('_', "-").parse().ok()
        }

        let requested: Vec<_> = requested_language().into_iter().collect();
        if let Err(err) = i18n_embed::select(&self, assets, &requested) {
            tracing::warn!(?err, "failed to load translations; falling back to en-US");
            self.load_fallback_language(assets)
                .expect("fl! checks the en-US catalogue at compile time");
        }

        // Fluent wraps arguments in U+2068/U+2069 for bidi isolation; in CLI output those
        // invisible characters end up in copied text and in scripts matching on it. This only
        // applies to bundles that are already loaded, so it MUST come after loading.
        self.set_use_isolating(false);
        self
    }
}

/// Declares the calling crate's catalogue, `i18n/<language>/<crate>.ftl`, as `LOADER`.
///
/// The crate needs `i18n-embed` and `rust-embed` dependencies and an `i18n.toml`.
#[doc(hidden)]
#[macro_export]
macro_rules! __atuin_catalogue {
    () => {
        #[allow(
            clippy::same_name_method,
            reason = "RustEmbed derives inherent `get`/`iter` next to its trait methods, as \
                      sibling impls"
        )]
        #[allow(
            clippy::disallowed_methods,
            reason = "RustEmbed's derive canonicalizes its folder in debug builds"
        )]
        mod localizations {
            #[derive(rust_embed::RustEmbed)]
            #[folder = "i18n"]
            pub struct Localizations;
        }

        /// The message catalogue for the user's language, negotiated on first use.
        pub static LOADER: ::std::sync::LazyLock<i18n_embed::fluent::FluentLanguageLoader> =
            ::std::sync::LazyLock::new(|| {
                $crate::i18n::LanguageLoaderExt::negotiated(
                    i18n_embed::fluent::fluent_language_loader!(),
                    &localizations::Localizations,
                )
            });
    };
}

/// Look up a message in `$loader`: `fl!(LOADER; "message-id", name = value)`, or one of its
/// attributes: `fl!(LOADER; "message-id", "long")`.
#[doc(hidden)]
#[macro_export]
macro_rules! __atuin_fl {
    ($loader:path; $message_id:literal) => {
        $crate::i18n::__fl::fl!($loader, $message_id)
    };
    ($loader:path; $message_id:literal, $attribute_id:literal) => {
        $crate::i18n::__fl::fl!($loader, $message_id, $attribute_id)
    };
    // Each value is parenthesized: i18n-embed-fl expands it as `$value.into()`, so `n = *x` would
    // otherwise become `*(x.into())`.
    ($loader:path; $message_id:literal, $attribute_id:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::__fl::fl!($loader, $message_id, $attribute_id, $($name = ($value)),+)
    };
    ($loader:path; $message_id:literal, $($name:ident = $value:expr),* $(,)?) => {
        $crate::i18n::__fl::fl!($loader, $message_id, $($name = ($value)),*)
    };
}

pub use __atuin_catalogue as catalogue;
pub use __atuin_fl as fl;

//! Localization of user-facing text.
//!
//! Messages are Fluent strings in `i18n/<language>/atuin.ftl`, looked up by id with [`fl!`]. `fl!`
//! fails the build if an id is missing from the `en-US` catalogue or its arguments don't match the
//! message's variables.

use std::sync::LazyLock;

use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
use i18n_embed::{DesktopLanguageRequester, LanguageLoader};

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

    let requested = DesktopLanguageRequester::requested_languages();
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

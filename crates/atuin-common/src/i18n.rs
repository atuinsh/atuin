use i18n_embed::{
    DesktopLanguageRequester,
    fluent::{FluentLanguageLoader, fluent_language_loader},
};
pub use i18n_embed_fl::fl;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../i18n"] // path to the compiled localization resources
struct Localizations;

pub use atuin_macro::tl;
use lazy_static::lazy_static;

lazy_static! {
    // We assume that one LOADER is sufficient. Fluent provides more
    // flexibility, but for now, this simplifies integration.
    pub static ref LOADER: FluentLanguageLoader = {
        // Load languages from central internationalization folder.
        let language_loader: FluentLanguageLoader = fluent_language_loader!();
        let requested_languages = DesktopLanguageRequester::requested_languages();

        let _result = i18n_embed::select(
            &language_loader, &Localizations, &requested_languages);
        language_loader
    };
}

#[macro_export]
macro_rules! t {
    // Case that t!("foo bar") is called with no runtime parameters to interpolate.
    ($message_id:literal) => {
        $crate::i18n::tl!($crate::i18n::fl, $crate::i18n::LOADER, $message_id)
    };

    // Case that t!("foo %{bar}", bar=baz.to_string()) is called with runtime parameters to interpolate.
    ($message_id:literal, $($args:expr),*) => {{
        $crate::i18n::tl!($crate::i18n::fl, $crate::i18n::LOADER, $message_id, $($args), *)
    }};
}

pub use t;

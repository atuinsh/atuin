use i18n_embed::{
    fluent::{FluentLanguageLoader, fluent_language_loader},
};
use lazy_static::lazy_static;
pub use i18n_embed_fl::fl;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "tests/i18n"] // path to the compiled localization resources
struct Localizations;

pub use atuin_macro::tl;

lazy_static! {
    // We assume that one LOADER is sufficient. Fluent provides more
    // flexibility, but for now, this simplifies integration.
    pub static ref LOADER: FluentLanguageLoader = {
        // Load languages from central internationalization folder.
        let language_loader: FluentLanguageLoader = fluent_language_loader!();
        let requested_languages = vec!["en-GB".parse().unwrap()];

        let _result = i18n_embed::select(
            &language_loader, &Localizations, &requested_languages);
        language_loader
    };
}

#[test]
fn basic_tr_without_parameter() {
    assert_eq!(
        tl!(fl, LOADER, "Danger, Bill Bobinson"),
        "Danger, William of Bobinson"
    );
}

#[test]
fn basic_tr_with_parameter() {
    assert_eq!(
        tl!(fl, LOADER, "unrecognized subcommand '%{subcommand}'", subcommand = "SUB"),
        "unrecognised subcommand '\u{2068}SUB\u{2069}'"
    );
}

#[test]
fn tr_with_non_en_range_without_parameter() {
    let language_loader: FluentLanguageLoader = fluent_language_loader!();
    let requested_languages = vec!["ga-IE".parse().unwrap()];

    let _result = i18n_embed::select(&language_loader, &Localizations, &requested_languages);

    assert_eq!(
        tl!(fl, language_loader, "Danger, Bill Bobinson"),
        "Contúirt, a Uilliam Mac Bhoboin"
    );
}

#[test]
fn tr_with_non_en_range_with_parameter() {
    let language_loader: FluentLanguageLoader = fluent_language_loader!();
    let requested_languages = vec!["hi-IN".parse().unwrap()];

    let _result = i18n_embed::select(&language_loader, &Localizations, &requested_languages);

    assert_eq!(
        tl!(fl, language_loader, "Hello, my name is %{name}", name = "रीमा"),
        "नमस्ते, मेरा नाम \u{2068}रीमा\u{2069} है।"
    );
}

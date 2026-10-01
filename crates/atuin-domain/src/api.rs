use std::sync::LazyLock;

use semver::Version;

// the usage of X- has been deprecated for quite along time, it turns out
pub static ATUIN_HEADER_VERSION: &str = "Atuin-Version";
pub static ATUIN_CARGO_VERSION: &str = env!("CARGO_PKG_VERSION");
/// The `User-Agent` Atuin's clients send, e.g. `atuin/18.23.0`.
pub static ATUIN_USER_AGENT: &str = concat!("atuin/", env!("CARGO_PKG_VERSION"));

pub static ATUIN_VERSION: LazyLock<Version> =
    LazyLock::new(|| Version::parse(ATUIN_CARGO_VERSION).expect("failed to parse self semver"));

//! See https://github.com/daboross/fern/issues/38
use log::log_enabled;

#[test]
fn ensure_enabled_is_a_deep_check() {
    let dummy = fern::Dispatch::new()
        .level(log::LevelFilter::Warn)
        .chain(std::io::stdout());

    let stdout = fern::Dispatch::new()
        .level(log::LevelFilter::Info)
        .level_for("abc", log::LevelFilter::Debug)
        .chain(std::io::stdout());

    fern::Dispatch::new()
        .chain(stdout)
        .chain(dummy)
        .apply()
        .unwrap();

    assert!(!log_enabled!(log::Level::Debug));
}

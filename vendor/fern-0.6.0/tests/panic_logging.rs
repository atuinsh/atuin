//! Test the functionality of panicking on error+ log messages.
use log::Level::*;

mod support;

use support::manual_log;

#[test]
#[should_panic(expected = "special panic message here")]
fn test_panic_panics() {
    let (_max_level, logger) = fern::Dispatch::new().chain(fern::Panic).into_log();

    let l = &*logger;

    manual_log(l, Info, "special panic message here");
}

fn warn_and_higher_panics_config() -> Box<dyn log::Log> {
    let (_max_level, logger) = fern::Dispatch::new()
        .chain(
            fern::Dispatch::new()
                .level(log::LevelFilter::Warn)
                .chain(fern::Panic),
        )
        .chain(std::io::stdout())
        .into_log();
    logger
}

#[test]
fn double_chained_with_panics_no_info_panic() {
    let l = &*warn_and_higher_panics_config();

    manual_log(l, Info, "this should not panic");
}

#[test]
#[should_panic(expected = "this should panic")]
fn double_chained_with_panics_yes_error_panic() {
    let l = &*warn_and_higher_panics_config();

    manual_log(l, Error, "this should panic");
}

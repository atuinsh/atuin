//! Support module for tests
use std::fmt;

/// Utility to manually enter a log message into a logger. All extra metadata
/// (target, line number, etc) will be blank.
pub fn manual_log<T, U>(logger: &T, level: log::Level, message: U)
where
    T: log::Log + ?Sized,
    U: fmt::Display,
{
    logger.log(
        &log::RecordBuilder::new()
            .args(format_args!("{}", message))
            .level(level)
            .build(),
    );
}

//! Helpers for errors.

use std::error::Error;
use std::fmt;

/// Displays an error followed by each of its sources, separated by `": "`.
///
/// For example, an error "could not read ~/.profile" with the source "permission denied" displays
/// as "could not read ~/.profile: permission denied".
#[derive(Clone, Copy)]
pub struct WithSources<'a>(pub &'a (dyn Error + 'static));

impl fmt::Display for WithSources<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)?;
        let mut source = self.0.source();
        while let Some(error) = source {
            write!(f, ": {error}")?;
            source = error.source();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("could not read the file")]
    struct Outer(#[source] std::io::Error);

    #[rstest]
    fn lists_every_source() {
        let error = Outer(std::io::Error::other("permission denied"));
        assert_eq!(WithSources(&error).to_string(), "could not read the file: permission denied");
    }

    #[rstest]
    fn an_error_without_a_source_stands_alone() {
        let error = std::io::Error::other("gone");
        assert_eq!(WithSources(&error).to_string(), "gone");
    }
}

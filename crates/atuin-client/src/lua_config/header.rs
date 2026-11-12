//! The header of a config file: the `-- atuin:<key>=<value>` comments at its top.

use std::borrow::Cow;

use serde::Deserialize;
use serde::de::value::{self, MapDeserializer};
use thiserror::Error;

/// The version of the config a file is written for. It decides what the file's environment holds
/// and what the file must return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Version {
    #[serde(rename = "1")]
    V1,
}

/// What a config file's header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub version: Version,
}

#[derive(Debug, Error)]
pub enum HeaderError {
    #[error("`atuin:{0}` needs a value, as in `atuin:{0}=<value>`")]
    NoValue(String),
    #[error(transparent)]
    Invalid(#[from] value::Error),
}

/// Reads the header of a config file from its source.
pub fn parse(source: &[u8]) -> Result<Header, HeaderError> {
    let directives = directives(source)
        .map(|directive| match directive.split_once('=') {
            Some((key, value)) => Ok((key.trim().to_owned(), value.trim().to_owned())),
            None => Err(HeaderError::NoValue(directive.into_owned())),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let header =
        Header::deserialize(MapDeserializer::<_, value::Error>::new(directives.into_iter()))?;
    Ok(header)
}

/// The text after `atuin:` on each directive line of the header in `source`.
fn directives(source: &[u8]) -> impl Iterator<Item = Cow<'_, str>> {
    source
        .split(|&byte| byte == b'\n')
        .map(<[u8]>::trim_ascii)
        .take_while(|line| line.is_empty() || line.starts_with(b"--"))
        .filter_map(|line| line.strip_prefix(b"--")?.trim_ascii_start().strip_prefix(b"atuin:"))
        .map(String::from_utf8_lossy)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::alone("-- atuin:version=1")]
    #[case::above_code("-- atuin:version=1\nreturn {}")]
    #[case::below_a_luau_directive("--!strict\n-- atuin:version=1\nreturn {}")]
    #[case::among_comments_and_blank_lines(
        "-- notes\n\n-- atuin:version=1\n--!strict\n\nreturn {}"
    )]
    #[case::crlf("-- atuin:version=1\r\nreturn {}")]
    #[case::unspaced("--atuin:version=1\nreturn {}")]
    #[case::spaced("--  atuin: version = 1 \nreturn {}")]
    fn a_header_declares_the_version(#[case] source: &str) {
        let header = parse(source.as_bytes()).expect("a header");

        assert_eq!(header.version, Version::V1);
    }

    /// A file runs against what its version gives it, so a header that doesn't settle the version
    /// fails the file before it runs.
    #[rstest]
    #[case::empty("", "missing field `version`")]
    #[case::no_header("return {}", "missing field `version`")]
    #[case::after_code("return {}\n-- atuin:version=1", "missing field `version`")]
    #[case::newer("-- atuin:version=2", "unknown variant `2`, expected `1`")]
    #[case::empty_version("-- atuin:version=", "unknown variant ``, expected `1`")]
    #[case::no_value(
        "-- atuin:version",
        "`atuin:version` needs a value, as in `atuin:version=<value>`"
    )]
    #[case::twice("-- atuin:version=1\n-- atuin:version=1", "duplicate field `version`")]
    #[case::unknown_key(
        "-- atuin:version=1\n-- atuin:colour=blue",
        "unknown field `colour`, expected `version`"
    )]
    fn a_header_that_says_something_else_is_invalid(#[case] source: &str, #[case] expected: &str) {
        let err = parse(source.as_bytes()).expect_err("an invalid header");

        assert_eq!(err.to_string(), expected);
    }
}

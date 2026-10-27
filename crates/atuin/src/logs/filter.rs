//! The log file's filter, read from `ATUIN_LOG`.

use std::str::FromStr;

use tracing::level_filters::STATIC_MAX_LEVEL;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::{Directive, LevelFilter, ParseError};

use crate::i18n::fl;

/// The log file's filter, and what was wrong with `ATUIN_LOG`.
pub(super) struct FileFilter {
    pub(super) filter: EnvFilter,
    /// Reported as user messages once logging is up, since nothing can be logged before.
    pub(super) warnings: Vec<DirectiveWarning>,
}

/// A directive in `ATUIN_LOG` that the filter skipped or can't honour.
#[derive(Debug, thiserror::Error)]
pub(super) enum DirectiveWarning {
    #[error(
        "{}",
        fl!("log-directive-invalid", directive = .directive.clone(), error = .source.to_string())
    )]
    Invalid {
        directive: String,
        #[source]
        source: ParseError,
    },
    #[error("{}", fl!("log-directive-unavailable", directive = .0.to_string()))]
    AboveStaticMax(Directive),
}

impl FileFilter {
    /// What the file records when `ATUIN_LOG` is unset: atuin's own crates at `info`, the rest at
    /// `warn`.
    const DEFAULT: &str = "warn,atuin=info";

    /// Build the filter from `ATUIN_LOG`.
    pub(super) fn from_env() -> Self {
        Self::parse(atuin_common::env::var("ATUIN_LOG").ok().as_deref(), STATIC_MAX_LEVEL)
    }

    /// Build the filter from comma-separated `EnvFilter` directives, falling back to
    /// [`Self::DEFAULT`] when none of them is usable.
    ///
    /// Never goes through `EnvFilter`'s own parsers: they print warnings to stderr themselves, which
    /// would land on top of a TUI.
    pub(super) fn parse(directives: Option<&str>, static_max: LevelFilter) -> Self {
        let mut warnings = Vec::new();
        let mut parsed: Vec<Directive> = directives
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|piece| !piece.is_empty())
            .filter_map(|piece| {
                Directive::from_str(piece)
                    .map_err(|source| {
                        warnings.push(DirectiveWarning::Invalid {
                            directive: piece.to_owned(),
                            source,
                        });
                    })
                    .ok()
            })
            .collect();
        if parsed.is_empty() {
            parsed = Self::DEFAULT
                .split(',')
                .map(|piece| piece.parse().expect("the default directives parse"))
                .collect();
        }

        warnings.extend(
            parsed
                .iter()
                .filter(|directive| Self::level(directive) > static_max)
                .cloned()
                .map(DirectiveWarning::AboveStaticMax),
        );

        let quiet_regexp = "sqlx_sqlite::regexp=off".parse().expect("a valid directive");
        let filter = parsed
            .into_iter()
            .chain([quiet_regexp])
            .fold(EnvFilter::default(), EnvFilter::add_directive);
        Self { filter, warnings }
    }

    /// The level a directive enables; its `Display` always ends with it (`atuin_daemon=trace`).
    fn level(directive: &Directive) -> LevelFilter {
        directive
            .to_string()
            .rsplit('=')
            .next()
            .and_then(|level| level.parse().ok())
            .unwrap_or(LevelFilter::TRACE)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use tracing_subscriber::filter::LevelFilter;

    use super::{DirectiveWarning, FileFilter};

    #[rstest]
    #[case::unset(None, LevelFilter::INFO)]
    #[case::blank(Some("  "), LevelFilter::INFO)]
    #[case::debug(Some("debug"), LevelFilter::DEBUG)]
    #[case::off(Some("off"), LevelFilter::OFF)]
    fn parse_sets_the_max_level(#[case] directives: Option<&str>, #[case] max: LevelFilter) {
        let parsed = FileFilter::parse(directives, LevelFilter::TRACE);

        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.filter.max_level_hint(), Some(max));
    }

    #[rstest]
    fn invalid_directive_is_reported_and_skipped() {
        let parsed = FileFilter::parse(Some("debug,atuin=loud"), LevelFilter::TRACE);

        assert!(
            matches!(parsed.warnings[..], [DirectiveWarning::Invalid { .. }]),
            "{:?}",
            parsed.warnings
        );
        assert_eq!(parsed.filter.max_level_hint(), Some(LevelFilter::DEBUG));
    }

    #[rstest]
    fn only_invalid_directives_fall_back_to_the_default() {
        let parsed = FileFilter::parse(Some("atuin=loud"), LevelFilter::TRACE);

        assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
        assert_eq!(parsed.filter.max_level_hint(), Some(LevelFilter::INFO));
    }

    #[rstest]
    fn directive_above_the_static_max_is_reported() {
        let parsed = FileFilter::parse(Some("atuin_daemon=trace"), LevelFilter::DEBUG);

        assert!(
            matches!(parsed.warnings[..], [DirectiveWarning::AboveStaticMax(_)]),
            "{:?}",
            parsed.warnings
        );
    }
}

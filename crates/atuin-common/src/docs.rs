//! Links into the versioned documentation site.
//!
//! docs.atuin.sh is versioned with mike, and the three kinds of version it
//! publishes are not equally durable (see `.github/actions/docs-deploy-*`):
//!
//! - a stable release publishes an `X.Y` directory, kept forever
//! - `main` tracks unreleased docs and is never pruned
//! - a prerelease publishes its *full* version, which is pruned as soon as the
//!   matching stable release lands
//!
//! So a prerelease build must not link to its own preview: those links would
//! start 404ing the moment the release it previews ships. Only stable builds
//! pin to themselves; everything else points at `main`, which is both durable
//! and the closest match for a build that is ahead of the last release.
//!
//! `atuin-common` shares the workspace version, so `CARGO_PKG_VERSION` here is
//! the version of the binary being built.

/// The docs.atuin.sh version segment matching this build.
pub const VERSION: &str = version_segment(env!("CARGO_PKG_VERSION"));

/// A URL for `path` (e.g. `guide/sync/#login`) in this build's documentation.
pub fn url(path: &str) -> String {
    format!("https://docs.atuin.sh/{VERSION}/{path}")
}

const fn version_segment(version: &str) -> &str {
    let bytes = version.as_bytes();

    let mut i = 0;
    while i < bytes.len() {
        // A prerelease has no directory that outlives the release it previews.
        if bytes[i] == b'-' {
            return "main";
        }
        i += 1;
    }

    // Stable releases are published as `X.Y`, so cut at the second dot.
    let mut end = 0;
    let mut dots = 0;
    while end < bytes.len() {
        if bytes[end] == b'.' {
            dots += 1;
            if dots == 2 {
                break;
            }
        }
        end += 1;
    }

    match std::str::from_utf8(bytes.split_at(end).0) {
        Ok(segment) => segment,
        // Unreachable: an ASCII-delimited prefix of a `str` is still UTF-8.
        Err(_) => "main",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_releases_pin_to_their_minor() {
        assert_eq!(version_segment("18.17.0"), "18.17");
        assert_eq!(version_segment("18.17.1"), "18.17");
        assert_eq!(version_segment("19.0.0"), "19.0");
        assert_eq!(version_segment("100.200.300"), "100.200");
    }

    #[test]
    fn prereleases_fall_back_to_main() {
        // Pinning these to `18.18.0-beta.2` would 404 once 18.18.0 shipped and
        // the preview was pruned.
        assert_eq!(version_segment("18.18.0-beta.2"), "main");
        assert_eq!(version_segment("18.16.0-beta.1"), "main");
        assert_eq!(version_segment("19.0.0-rc.1"), "main");
    }

    #[test]
    fn this_build_resolves_to_a_published_version() {
        // Either `X.Y` or `main`; never a full version, and never empty.
        assert!(!VERSION.is_empty());
        assert!(VERSION == "main" || VERSION.split('.').count() == 2);
        assert!(!VERSION.contains('-'));
    }

    #[test]
    fn urls_are_absolute_and_versioned() {
        assert_eq!(
            url("guide/sync/#login"),
            format!("https://docs.atuin.sh/{VERSION}/guide/sync/#login")
        );
    }
}

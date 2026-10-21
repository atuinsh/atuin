//! [`serde_with`] converters for paths a user writes, as in a config file.
//!
//! A relative path in a config file would otherwise resolve against the working directory of
//! whichever process reads it, so `socket_path = "atuin.sock"` names a different socket for a
//! daemon started from `~` than for a client running in a project. [`RelativeTo`] resolves it
//! once, at deserialization, against a directory fixed by its [`PathAnchor`].

use std::io;
use std::marker::PhantomData;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_with::{DeserializeAs, SerializeAs};

use crate::path::AbsPathBuf;

/// The directory [`RelativeTo`] resolves relative paths against.
pub trait PathAnchor {
    fn dir() -> io::Result<AbsPathBuf>;
}

/// [`serde_with`] converter for a path that expands a leading `~` and `$VAR`, `${VAR}` or
/// `${VAR:-default}`, reads `$$` as a literal `$`, and rejects an empty path or an unset variable.
/// That is all it takes from the shell: no globbing or command substitution.
///
/// A relative path stays relative; see [`RelativeTo`] for one that must not depend on the working
/// directory.
pub struct ShellExpanded;

impl ShellExpanded {
    /// `path`, written so that it expands back to itself: a path Atuin worked out, which must not
    /// be expanded a second time.
    #[must_use]
    pub fn escape(path: &str) -> String {
        path.replace('$', "$$")
    }

    fn expand<E: de::Error>(raw: &str) -> Result<PathBuf, E> {
        if raw.is_empty() {
            return Err(E::custom("path is empty"));
        }
        let home = || crate::dirs::try_home_dir().ok()?.into_os_string().into_string().ok();
        let expanded =
            shellexpand::full_with_context(raw, home, |var| crate::env::var(var).map(Some))
                .map_err(|err| E::custom(format!("failed to expand {raw:?}: {err}")))?;
        Ok(expanded.into_owned().into())
    }
}

impl<'de> DeserializeAs<'de, PathBuf> for ShellExpanded {
    fn deserialize_as<D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
        Self::expand(&String::deserialize(deserializer)?)
    }
}

impl SerializeAs<PathBuf> for ShellExpanded {
    fn serialize_as<S: Serializer>(source: &PathBuf, serializer: S) -> Result<S::Ok, S::Error> {
        source.serialize(serializer)
    }
}

/// [`serde_with`] converter for a path that expands like [`ShellExpanded`], then resolves a
/// relative result against `A`'s directory into an [`AbsPathBuf`], so it names the same file in
/// every process.
pub struct RelativeTo<A>(PhantomData<A>);

impl<'de, A: PathAnchor> DeserializeAs<'de, AbsPathBuf> for RelativeTo<A> {
    fn deserialize_as<D: Deserializer<'de>>(deserializer: D) -> Result<AbsPathBuf, D::Error> {
        let relative = match AbsPathBuf::new(ShellExpanded::deserialize_as(deserializer)?) {
            Ok(path) => return Ok(path),
            Err(err) => err.into_path_buf(),
        };
        let dir = A::dir()
            .map_err(|err| de::Error::custom(format!("failed to resolve {relative:?}: {err}")))?;
        dir.resolve(&relative).map_err(|_| {
            de::Error::custom(format!("failed to resolve {relative:?} against {:?}", dir.as_path()))
        })
    }
}

impl<A> SerializeAs<AbsPathBuf> for RelativeTo<A> {
    fn serialize_as<S: Serializer>(source: &AbsPathBuf, serializer: S) -> Result<S::Ok, S::Error> {
        source.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use proptest::prelude::*;
    use rstest::{fixture, rstest};
    use serde_json::Value;
    use serde_with::As;

    use super::*;
    use crate::env::MockEnv;

    struct Anchor;

    impl PathAnchor for Anchor {
        fn dir() -> io::Result<AbsPathBuf> {
            Ok(AbsPathBuf::new(PathBuf::from(if cfg!(windows) {
                r"C:\anchor"
            } else {
                "/anchor"
            }))
            .unwrap())
        }
    }

    struct Unavailable;

    impl PathAnchor for Unavailable {
        fn dir() -> io::Result<AbsPathBuf> {
            Err(io::Error::other("no anchor"))
        }
    }

    #[fixture]
    fn env() -> MockEnv {
        let env = MockEnv::install();
        env.set("HOME", "/home/user");
        env.set("RUNTIME_DIR", "/run/user");
        env.set("NAME", "atuin");
        env
    }

    /// Deserialize `raw` through the converter `C`, in the mock environment `_env`.
    fn deserialize<C: for<'de> DeserializeAs<'de, T>, T: Into<PathBuf>>(
        _env: &MockEnv,
        raw: &str,
    ) -> Result<PathBuf, serde_json::Error> {
        As::<C>::deserialize(Value::from(raw)).map(Into::into)
    }

    #[cfg(unix)]
    #[rstest]
    #[case::bare_name("atuin.sock", "/anchor/atuin.sock")]
    #[case::nested("run/atuin.sock", "/anchor/run/atuin.sock")]
    #[case::parent("../atuin.sock", "/anchor/../atuin.sock")]
    #[case::absolute("/tmp/atuin.sock", "/tmp/atuin.sock")]
    #[case::tilde("~/atuin.sock", "/home/user/atuin.sock")]
    #[case::bare_tilde("~", "/home/user")]
    #[case::tilde_mid_name("a~b", "/anchor/a~b")]
    #[case::absolute_variable("$RUNTIME_DIR/atuin.sock", "/run/user/atuin.sock")]
    #[case::relative_variable("${NAME}.sock", "/anchor/atuin.sock")]
    #[case::unset_variable_with_a_default("${UNSET:-/tmp}/atuin.sock", "/tmp/atuin.sock")]
    #[case::lone_dollar("a$/atuin.sock", "/anchor/a$/atuin.sock")]
    #[case::escaped_dollar("$$NAME.sock", "/anchor/$NAME.sock")]
    fn relative_to_resolves(env: MockEnv, #[case] raw: &str, #[case] expected: &str) {
        assert_eq!(deserialize::<RelativeTo<Anchor>, _>(&env, raw).unwrap(), Path::new(expected));
    }

    #[rstest]
    #[case::empty("", "path is empty")]
    #[case::unset_variable("$UNSET/atuin.sock", "UNSET")]
    #[case::unset_braced_variable("${UNSET}/atuin.sock", "UNSET")]
    #[cfg_attr(windows, case::drive_relative("D:atuin.sock", "failed to resolve"))]
    fn relative_to_rejects(env: MockEnv, #[case] raw: &str, #[case] expected: &str) {
        let err = deserialize::<RelativeTo<Anchor>, _>(&env, raw).unwrap_err().to_string();
        assert!(err.contains(expected), "error should mention {expected:?}, got: {err}");
    }

    /// Only a relative path needs the anchor, so only a relative path fails without one.
    #[cfg(unix)]
    #[rstest]
    #[case::relative("atuin.sock", Err("no anchor"))]
    #[case::absolute("/tmp/atuin.sock", Ok("/tmp/atuin.sock"))]
    fn relative_to_needs_the_anchor_only_for_relative_paths(
        env: MockEnv,
        #[case] raw: &str,
        #[case] expected: Result<&str, &str>,
    ) {
        let resolved = deserialize::<RelativeTo<Unavailable>, _>(&env, raw);
        match expected {
            Ok(path) => assert_eq!(resolved.unwrap(), Path::new(path)),
            Err(cause) => assert!(resolved.unwrap_err().to_string().contains(cause)),
        }
    }

    #[cfg(unix)]
    #[rstest]
    #[case::relative("search.log", "search.log")]
    #[case::tilde("~/search.log", "/home/user/search.log")]
    #[case::variable("$NAME/search.log", "atuin/search.log")]
    fn shell_expanded_leaves_relative_paths_relative(
        env: MockEnv,
        #[case] raw: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(deserialize::<ShellExpanded, _>(&env, raw).unwrap(), Path::new(expected));
    }

    proptest! {
        /// A path Atuin hands back to the config, escaped, comes out of it as it went in.
        #[test]
        fn escaped_paths_expand_to_themselves(path in "[^~].*") {
            let expanded = ShellExpanded::expand::<serde_json::Error>(&ShellExpanded::escape(&path));
            prop_assert_eq!(expanded.unwrap(), PathBuf::from(path));
        }
    }
}

//! [`serde_with`] converters for paths a user writes, as in a config file.
//!
//! A relative path in a config file would otherwise resolve against the working directory of
//! whichever process reads it, so `socket_path = "atuin.sock"` names a different socket for a
//! daemon started from `~` than for a client running in a project. [`RelativeTo`] resolves it
//! once, at deserialization, against a directory fixed by its [`PathAnchor`].

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::env::VarError;
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
/// `${VAR:-default}`, and reads `$$` as a literal `$`. That is all it takes from the shell: no
/// globbing or command substitution.
///
/// A path naming a variable that isn't set is taken as written, unexpanded, so that a `$` in a
/// directory's name keeps naming that directory; [`ShellExpanded::expand`] tells which paths those
/// are, for the caller to warn about. An empty path is rejected.
///
/// A relative path stays relative; see [`RelativeTo`] for one that must not depend on the working
/// directory.
pub struct ShellExpanded;

/// Errors returned by [`ShellExpanded::expand`].
#[derive(Debug, thiserror::Error)]
pub enum ExpandError {
    #[error("path is empty")]
    Empty,
    #[error("${var}: {error}")]
    Variable {
        var: String,
        error: VarError,
    },
}

impl ShellExpanded {
    /// `path`, written so that it expands back to itself: a path Atuin worked out, which must not
    /// be expanded a second time.
    #[must_use]
    pub fn escape(path: &str) -> String {
        path.replace('$', "$$")
    }

    /// The variables expanding `raw` reads, such as `HISTDIR` for `$HISTDIR/history.db`: those
    /// another process needs the same values of to expand it to the same path.
    #[must_use]
    pub fn variables(raw: &str) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        shellexpand::env_with_context_no_errors(raw, |name| {
            names.insert(name.to_owned());
            None::<&str>
        });
        names
    }

    /// `raw`, expanded.
    pub fn expand(raw: &str) -> Result<String, ExpandError> {
        if raw.is_empty() {
            return Err(ExpandError::Empty);
        }
        let home = || crate::dirs::try_home_dir().ok()?.into_os_string().into_string().ok();
        shellexpand::full_with_context(raw, home, |var| crate::env::var(var).map(Some))
            .map(Cow::into_owned)
            .map_err(|err| ExpandError::Variable {
                var: err.var_name,
                error: err.cause,
            })
    }
}

impl<'de> DeserializeAs<'de, PathBuf> for ShellExpanded {
    fn deserialize_as<D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
        let raw = String::deserialize(deserializer)?;
        match Self::expand(&raw) {
            Ok(expanded) => Ok(expanded.into()),
            Err(ExpandError::Variable { .. }) => Ok(raw.into()),
            Err(err @ ExpandError::Empty) => Err(de::Error::custom(err)),
        }
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

impl<A: PathAnchor> RelativeTo<A> {
    /// `path`, resolved against `A`'s directory if it is relative: the second half of
    /// deserializing it, after [expanding](ShellExpanded) it.
    pub fn resolve<E: de::Error>(path: PathBuf) -> Result<AbsPathBuf, E> {
        let relative = match AbsPathBuf::new(path) {
            Ok(path) => return Ok(path),
            Err(err) => err.into_path_buf(),
        };
        let dir =
            A::dir().map_err(|err| E::custom(format!("failed to resolve {relative:?}: {err}")))?;
        dir.resolve(&relative).map_err(|_| {
            E::custom(format!("failed to resolve {relative:?} against {:?}", dir.as_path()))
        })
    }
}

impl<'de, A: PathAnchor> DeserializeAs<'de, AbsPathBuf> for RelativeTo<A> {
    fn deserialize_as<D: Deserializer<'de>>(deserializer: D) -> Result<AbsPathBuf, D::Error> {
        Self::resolve(ShellExpanded::deserialize_as(deserializer)?)
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

    /// The variables a path reads are those its expansion looks up, which an escaped `$` isn't.
    #[rstest]
    #[case::bare("$HISTDIR/history.db", &["HISTDIR"])]
    #[case::braced("${HISTDIR}/history.db", &["HISTDIR"])]
    #[case::several("$A/${B}/$A", &["A", "B"])]
    #[case::escaped("$$HISTDIR/history.db", &[])]
    #[case::home("~/history.db", &[])]
    fn variables_are_those_expansion_reads(#[case] raw: &str, #[case] names: &[&str]) {
        let names: BTreeSet<String> = names.iter().map(|name| (*name).to_owned()).collect();
        assert_eq!(ShellExpanded::variables(raw), names);
    }

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
    #[case::unset_variable("$UNSET/atuin.sock", "/anchor/$UNSET/atuin.sock")]
    #[case::unset_braced_variable("${UNSET}/atuin.sock", "/anchor/${UNSET}/atuin.sock")]
    #[case::unset_variable_in_an_absolute_path("/tmp/a$b/atuin.sock", "/tmp/a$b/atuin.sock")]
    #[case::unset_variable_beside_a_tilde("~/$UNSET.sock", "/anchor/~/$UNSET.sock")]
    fn relative_to_resolves(env: MockEnv, #[case] raw: &str, #[case] expected: &str) {
        assert_eq!(deserialize::<RelativeTo<Anchor>, _>(&env, raw).unwrap(), Path::new(expected));
    }

    #[rstest]
    #[case::empty("", "path is empty")]
    #[cfg_attr(windows, case::drive_relative("D:atuin.sock", "failed to resolve"))]
    fn relative_to_rejects(env: MockEnv, #[case] raw: &str, #[case] expected: &str) {
        let err = deserialize::<RelativeTo<Anchor>, _>(&env, raw).unwrap_err().to_string();
        assert!(err.contains(expected), "error should mention {expected:?}, got: {err}");
    }

    /// `expand` names the variable that leaves a path as written, for the caller's warning.
    #[rstest]
    #[case::empty("", None)]
    #[case::unset_variable("$UNSET/atuin.sock", Some("UNSET"))]
    #[case::unset_braced_variable("${UNSET}/atuin.sock", Some("UNSET"))]
    #[case::unset_after_a_set_one("$NAME/$UNSET.sock", Some("UNSET"))]
    fn expand_names_the_unset_variable(
        env: MockEnv,
        #[case] raw: &str,
        #[case] unset: Option<&str>,
    ) {
        env.remove("UNSET");
        match (ShellExpanded::expand(raw), unset) {
            (Err(ExpandError::Empty), None) => {}
            (Err(ExpandError::Variable { var, .. }), Some(unset)) => assert_eq!(var, unset),
            (expanded, unset) => panic!("expected {unset:?} to be unset, got {expanded:?}"),
        }
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
            prop_assert_eq!(ShellExpanded::expand(&ShellExpanded::escape(&path)).unwrap(), path);
        }
    }
}

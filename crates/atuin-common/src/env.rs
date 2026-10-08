//! Utilities for reading environment variables.

use std::env::VarError;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

/// Wrapper around [`std::env::var_os`] that can be mocked in tests.
#[must_use]
pub fn var_os(key: impl AsRef<OsStr>) -> Option<OsString> {
    #[cfg(any(test, feature = "test-utils"))]
    if let Some(value) = mock::get_os(key.as_ref()) {
        return value;
    }

    #[expect(clippy::disallowed_methods, reason = "this is the wrapper the lint references")]
    std::env::var_os(key)
}

/// Wrapper around [`std::env::var`] that can be mocked in tests.
pub fn var(key: impl AsRef<OsStr>) -> Result<String, VarError> {
    #[cfg(any(test, feature = "test-utils"))]
    if let Some(value) = mock::get(key.as_ref()) {
        return value;
    }

    #[expect(clippy::disallowed_methods, reason = "this is the wrapper the lint references")]
    std::env::var(key)
}

/// Read an environment variable that must be nonempty.
///
/// This function will never return an empty string: if the environment variable is set but empty,
/// [`None`] is returned.
#[must_use]
pub fn var_nonempty(name: &str) -> Option<OsString> {
    var_os(name).filter(|value| !value.is_empty())
}

/// Read an environment variable that must be an absolute path.
///
/// This is usually done in the name of XDG-compliance which requires that paths given through
/// environment variables are absolute.
pub fn var_abspath(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(var_nonempty(name)?);
    if !path.is_absolute() {
        tracing::warn!("ignoring relative path in {name}: {}", path.display());
        return None;
    }
    Some(path)
}

#[cfg(any(test, feature = "test-utils"))]
pub use mock::MockEnv;

#[cfg(any(test, feature = "test-utils"))]
pub(crate) mod mock {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::env::VarError;
    use std::ffi::{OsStr, OsString};
    use std::marker::PhantomData;

    thread_local! {
        /// The current thread's mock environment, if one is installed.
        static MOCK: RefCell<Option<HashMap<OsString, OsString>>> = const { RefCell::new(None) };
    }

    /// Whether the current thread has a mock environment installed.
    pub fn is_installed() -> bool {
        MOCK.with_borrow(Option::is_some)
    }

    pub(super) fn get_os(key: &OsStr) -> Option<Option<OsString>> {
        MOCK.with_borrow(|mock| mock.as_ref().map(|vars| vars.get(key).cloned()))
    }

    pub(super) fn get(key: &OsStr) -> Option<Result<String, VarError>> {
        get_os(key).map(|value| {
            value.ok_or(VarError::NotPresent)?.into_string().map_err(VarError::NotUnicode)
        })
    }

    /// A mock environment for the current thread, in which every variable starts out unset.
    ///
    /// While it is installed, [`var`](super::var) and [`var_os`](super::var_os) read from it
    /// instead of the real environment. It is uninstalled on drop.
    #[derive(Debug)]
    pub struct MockEnv {
        /// The mock belongs to the thread that installed it, so the guard has to stay there.
        _not_send: PhantomData<*const ()>,
    }

    impl MockEnv {
        /// Install an empty mock environment for the current thread.
        ///
        /// # Panics
        ///
        /// If the current thread already has one.
        #[must_use]
        pub fn install() -> Self {
            MOCK.with_borrow_mut(|mock| {
                assert!(mock.is_none(), "a mock environment is already installed on this thread");
                *mock = Some(HashMap::new());
            });
            Self {
                _not_send: PhantomData,
            }
        }

        pub fn set(&self, key: impl Into<OsString>, value: impl Into<OsString>) {
            Self::with_vars(|vars| vars.insert(key.into(), value.into()));
        }

        pub fn remove(&self, key: impl AsRef<OsStr>) {
            Self::with_vars(|vars| vars.remove(key.as_ref()));
        }

        fn with_vars<T>(f: impl FnOnce(&mut HashMap<OsString, OsString>) -> T) -> T {
            MOCK.with_borrow_mut(|mock| {
                f(mock.as_mut().expect("the mock environment is installed"))
            })
        }
    }

    impl Drop for MockEnv {
        fn drop(&mut self) {
            MOCK.with_borrow_mut(|mock| *mock = None);
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};

    use super::*;

    #[fixture]
    fn env() -> MockEnv {
        MockEnv::install()
    }

    #[rstest]
    fn a_mock_starts_out_empty() {
        let _env = MockEnv::install();
        // `PATH` is set in practically every real environment.
        assert_eq!(var_os("PATH"), None);
        assert_eq!(var("PATH"), Err(VarError::NotPresent));
    }

    #[rstest]
    fn set_and_removed_variables_are_read_from_the_mock(env: MockEnv) {
        env.set("ATUIN_TEST_VAR", "value");
        assert_eq!(var_os("ATUIN_TEST_VAR"), Some("value".into()));
        assert_eq!(var("ATUIN_TEST_VAR").as_deref(), Ok("value"));

        env.remove("ATUIN_TEST_VAR");
        assert_eq!(var_os("ATUIN_TEST_VAR"), None);
    }

    #[cfg(unix)]
    #[rstest]
    fn non_unicode_values_are_reported_like_std(env: MockEnv) {
        use std::os::unix::ffi::OsStringExt;

        let value = OsString::from_vec(vec![0xff]);
        env.set("ATUIN_TEST_VAR", value.clone());
        assert_eq!(var("ATUIN_TEST_VAR"), Err(VarError::NotUnicode(value)));
    }

    #[rstest]
    fn the_real_environment_is_read_without_a_mock() {
        assert!(var_os("PATH").is_some());
    }

    #[rstest]
    fn dropping_the_mock_uninstalls_it() {
        drop(MockEnv::install());
        assert!(var_os("PATH").is_some());
        // And another can be installed.
        drop(MockEnv::install());
    }

    #[rstest]
    #[case::empty("", None)]
    #[case::nonempty("value", Some("value"))]
    fn var_nonempty_treats_empty_as_unset(
        env: MockEnv,
        #[case] value: &str,
        #[case] expected: Option<&str>,
    ) {
        env.set("ATUIN_TEST_VAR", value);
        assert_eq!(var_nonempty("ATUIN_TEST_VAR"), expected.map(OsString::from));
    }

    #[cfg(unix)]
    #[rstest]
    #[case::absolute("/tmp/atuin", Some("/tmp/atuin"))]
    #[case::relative("tmp/atuin", None)]
    #[case::empty("", None)]
    fn var_abspath_ignores_relative_paths(
        env: MockEnv,
        #[case] value: &str,
        #[case] expected: Option<&str>,
    ) {
        env.set("ATUIN_TEST_VAR", value);
        assert_eq!(var_abspath("ATUIN_TEST_VAR"), expected.map(PathBuf::from));
    }
}

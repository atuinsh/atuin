//! What stands between a desktop and a useful capture, and how to remove it.
//!
//! [`Inspect::unready_apps`](crate::Inspect::unready_apps) finds the running applications
//! that don't expose their content until the user acts,
//! [`Platform::session_variables`](crate::Platform::session_variables) describes the
//! variables that decide whether applications expose their content, and
//! [`Platform::access_lifetime`](crate::Platform::access_lifetime) tells how long a granted
//! permission lasts.

use std::ffi::{OsStr, OsString};

/// How long the permission to use the accessibility API lasts once the user grants it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessLifetime {
    /// The permission survives new builds of this program, or the platform needs none.
    Durable,
    /// The permission covers this exact build. A rebuild or an update needs a new grant.
    ThisBuild,
    /// The platform did not tell.
    Unknown,
}

/// A running application that doesn't expose its content until the user acts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadyApp {
    /// The name of the application's executable.
    pub name: String,
    pub pid: u32,
    pub remedy: Remedy,
}

/// What makes an application expose its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Remedy {
    /// Restart the application at a time when a client is connected.
    Restart,
    /// Restart the application once the session sets the [`SessionVariable`] that it needs.
    ///
    /// The application runs in a sandbox that hides whether a client is connected, so the variable
    /// is the only thing that makes it expose its content.
    RestartWithVariable,
}

/// Why running applications can be missing from what the accessibility API lists, in sentences
/// for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MissingAppNotes {
    /// Why an application that started before a client connected can be missing.
    ///
    /// The applications that need a restart for this reason are
    /// [`unready_apps`](crate::Inspect::unready_apps).
    pub started_early: Option<&'static str>,
    /// Why an application can be missing right after [`wake_apps`](crate::Backend::wake_apps)
    /// asked the applications to expose their content.
    pub just_woken: Option<&'static str>,
}

/// A variable of the desktop session that decides whether applications expose their content.
///
/// Applications read the variable when they start, so an application has the variable only
/// when the session that starts the application passes the variable on. Sessions differ in the
/// files that they read.
#[derive(Debug, PartialEq, Eq)]
pub struct SessionVariable {
    pub name: &'static str,
    pub exposure: Exposure,
    /// The value at which the variable keeps applications from exposing their content.
    pub hiding_value: &'static str,
    /// The variables that keep this variable from hiding applications when the session sets
    /// any of them.
    pub unless_set: &'static [&'static str],
    /// What the variable does to applications, in sentences for the user.
    pub reason: &'static str,
}

/// How a [`SessionVariable`] lets applications expose their content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exposure {
    /// Applications expose their content only when the variable has this value. Setup sets
    /// the value in each session.
    Requires(&'static str),
    /// Applications expose their content unless the variable has its hiding value. Setup sets
    /// this value in each session, which overrides a hiding value that the session sets.
    Overrides(&'static str),
    /// Applications expose their content unless the variable has its hiding value. Setup leaves
    /// the variable alone.
    Unless,
}

impl SessionVariable {
    /// The value that setup sets in each session. `None` for a variable that setup leaves
    /// alone.
    #[must_use]
    pub const fn setup_value(&self) -> Option<&'static str> {
        match self.exposure {
            Exposure::Requires(value) | Exposure::Overrides(value) => Some(value),
            Exposure::Unless => None,
        }
    }

    /// Whether the variable, as this process's environment sets it, keeps applications from
    /// exposing their content.
    #[must_use]
    pub fn hides_in_this_session(&self) -> bool {
        self.hides_with(|name| atuin_common::env::var_os(name))
    }

    /// Whether the variable keeps applications from exposing their content in a session whose
    /// variables `value_of` looks up.
    fn hides_with(&self, value_of: impl Fn(&str) -> Option<OsString>) -> bool {
        self.hides_at(value_of(self.name).as_deref())
            && !self.unless_set.iter().any(|name| value_of(name).is_some())
    }

    /// Whether the variable keeps applications from exposing their content at `value`, where
    /// `None` means that the variable is unset.
    fn hides_at(&self, value: Option<&OsStr>) -> bool {
        match self.exposure {
            Exposure::Requires(needed) => value.is_none_or(|value| value != needed),
            Exposure::Overrides(_) | Exposure::Unless => {
                value.is_some_and(|value| value == self.hiding_value)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const fn variable(exposure: Exposure) -> SessionVariable {
        SessionVariable {
            name: "NAME",
            exposure,
            hiding_value: "0",
            unless_set: &[],
            reason: "",
        }
    }

    #[rstest]
    #[case::required_and_set(Exposure::Requires("1"), Some("1"), false)]
    #[case::required_and_unset(Exposure::Requires("1"), None, true)]
    #[case::required_and_other(Exposure::Requires("1"), Some("2"), true)]
    #[case::overridden_and_unset(Exposure::Overrides("1"), None, false)]
    #[case::overridden_and_hiding(Exposure::Overrides("1"), Some("0"), true)]
    #[case::unless_and_unset(Exposure::Unless, None, false)]
    #[case::unless_and_hiding(Exposure::Unless, Some("0"), true)]
    #[case::unless_and_other(Exposure::Unless, Some("1"), false)]
    fn variables_hide_at_their_values(
        #[case] exposure: Exposure,
        #[case] value: Option<&str>,
        #[case] hides: bool,
    ) {
        assert_eq!(variable(exposure).hides_at(value.map(OsStr::new)), hides);
    }

    #[rstest]
    #[case::alone(&[("NAME", "0")], true)]
    #[case::with_an_exception(&[("NAME", "0"), ("OTHER", "1")], false)]
    #[case::exception_alone(&[("OTHER", "1")], false)]
    fn variables_hide_only_without_their_exceptions(
        #[case] session: &[(&str, &str)],
        #[case] hides: bool,
    ) {
        let variable = SessionVariable {
            unless_set: &["OTHER"],
            ..variable(Exposure::Unless)
        };
        let value_of = |name: &str| {
            session.iter().find(|(set, _)| *set == name).map(|(_, value)| OsString::from(value))
        };
        assert_eq!(variable.hides_with(value_of), hides);
    }
}

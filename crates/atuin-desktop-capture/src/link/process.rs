//! The contract between the daemon and the helper's process: the variables that the daemon
//! sets for the helper, and the exits of the helper that the daemon tells apart.

use serde::{Deserialize, Serialize};

/// The variable that names the command through which the user runs the helper's `apps` and
/// `doctor`, such as `atuin desktop-capture`, so that the hints of those commands name it.
///
/// The daemon passes the command in a variable because a helper of another build ignores a
/// variable that it doesn't know. The same helper would refuse an argument that it doesn't
/// know.
pub const COMMAND_VARIABLE: &str = "ATUIN_DESKTOP_CAPTURE_COMMAND";

/// The variable that names a file descriptor open for writing. The helper's `apps` writes the
/// applications to capture to the descriptor, as one [`AppsRecord`] in JSON.
///
/// Without the variable, the helper's `apps` prints the `apps` setting for the user to copy.
pub const APPS_RECORD_FD_VARIABLE: &str = "ATUIN_DESKTOP_CAPTURE_APPS_FD";

/// The applications that the helper's `apps` suggests to capture, in the order of its scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppsRecord {
    pub apps: Vec<String>,
}

/// An exit of the helper's `capture` that the daemon tells apart from a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureExit {
    /// The permission to use the accessibility API is missing.
    NeedsPermission,
    /// No desktop session is running, or the session ended.
    NoDesktop,
}

impl CaptureExit {
    /// Every exit, for [`from_code`](Self::from_code) to search.
    const ALL: [Self; 2] = [Self::NeedsPermission, Self::NoDesktop];

    /// The exit status that stands for this exit.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::NeedsPermission => 3,
            Self::NoDesktop => 4,
        }
    }

    /// The exit that `code` stands for, if any.
    #[must_use]
    pub fn from_code(code: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|exit| i32::from(exit.code()) == code)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::needs_permission(CaptureExit::NeedsPermission)]
    #[case::no_desktop(CaptureExit::NoDesktop)]
    fn capture_exit_round_trips(#[case] exit: CaptureExit) {
        assert_eq!(CaptureExit::from_code(i32::from(exit.code())), Some(exit));
    }

    #[rstest]
    #[case::success(0)]
    #[case::failure(1)]
    fn other_codes_are_no_capture_exit(#[case] code: i32) {
        assert_eq!(CaptureExit::from_code(code), None);
    }
}

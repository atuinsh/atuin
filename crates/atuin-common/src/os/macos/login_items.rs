//! What the user lets run in the background, in System Settings, under General > Login Items.
#![allow(unsafe_code, reason = "ServiceManagement's API is Objective-C.")]

use std::path::Path;

use objc2_foundation::NSURL;
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// Whether the user lets a launchd job run, by its switch in Login Items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginItem {
    /// Switched on, or not listed yet.
    Allowed,
    /// Switched off. launchd still loads the job when asked to, so whoever loads it has to check.
    SwitchedOff,
}

impl LoginItem {
    /// The switch for the job of the legacy plist at `plist`, in a `LaunchAgents` or
    /// `LaunchDaemons` directory.
    ///
    /// Every job is allowed before macOS 13, which has no such switch.
    #[must_use]
    pub fn of_legacy_plist(plist: &Path) -> Self {
        if !objc2::available!(macos = 13.0) {
            return Self::Allowed;
        }
        let Some(url) = NSURL::from_file_path(plist) else {
            return Self::Allowed;
        };
        // SAFETY: `statusForLegacyURL:` exists from macOS 13, and takes any file URL.
        let status = unsafe { SMAppService::statusForLegacyURL(&url) };
        if status == SMAppServiceStatus::RequiresApproval {
            Self::SwitchedOff
        } else {
            Self::Allowed
        }
    }
}

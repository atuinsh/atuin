//! What the supervisor takes from the settings.

use std::path::PathBuf;

use atuin_client::settings::{AppSelection, Settings};
use atuin_common::time::NonZeroDuration;
use atuin_desktop_capture::link::LinkConfig;
use atuin_desktop_capture::{PrivateWindows, Scope};

use crate::desktop_capture::helper_path;

/// What the supervisor takes from the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperSettings {
    pub enabled: bool,
    pub helper: PathBuf,
    /// What the helper captures. `None` when the settings don't select any application.
    pub link: Option<LinkConfig>,
}

/// How new settings apply to a running helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Nothing that the helper uses changed.
    None,
    /// The helper takes the new [`LinkConfig`] as it runs.
    Reconfigure,
    /// The settings need a new helper, or none.
    Respawn,
}

impl HelperSettings {
    pub fn from_settings(settings: &Settings) -> Self {
        let capture = &settings.desktop_capture;
        let scope = match capture.selection() {
            AppSelection::None => None,
            AppSelection::All => Some(Scope::All),
            AppSelection::Only(apps) => Some(Scope::Only(apps.to_vec())),
        };
        let link = scope.map(|scope| LinkConfig {
            scope,
            excluded: capture.exclude.clone(),
            secret_patterns: settings
                .security
                .redact_patterns
                .iter()
                .map(|pattern| pattern.as_str().to_owned())
                .collect(),
            builtin_secrets: settings.builtin_secrets(),
            poll_interval: capture.poll_interval.map(NonZeroDuration::get),
            read_budget: capture.read_budget,
            private_windows: match capture.private_windows {
                atuin_client::settings::PrivateWindows::Exclude => PrivateWindows::Exclude,
                atuin_client::settings::PrivateWindows::Capture => PrivateWindows::Capture,
            },
        });
        Self {
            enabled: capture.enabled,
            helper: helper_path(settings),
            link,
        }
    }

    /// Whether these settings run a helper.
    pub const fn captures(&self) -> bool {
        self.enabled && self.link.is_some()
    }

    /// How a change from `self` to `new` applies to a running helper.
    pub fn change_to(&self, new: &Self) -> Change {
        if new.enabled != self.enabled
            || new.helper != self.helper
            || new.link.is_some() != self.link.is_some()
        {
            Change::Respawn
        } else if new.link != self.link {
            Change::Reconfigure
        } else {
            Change::None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use atuin_common::secrets::BuiltinSecrets;
    use rstest::{fixture, rstest};

    use super::*;

    #[fixture]
    fn link() -> LinkConfig {
        LinkConfig {
            scope: Scope::Only(vec!["gedit".to_owned()]),
            excluded: Vec::new(),
            secret_patterns: Vec::new(),
            builtin_secrets: BuiltinSecrets::On,
            poll_interval: Some(Duration::from_secs(30)),
            read_budget: Duration::from_millis(1500),
            private_windows: PrivateWindows::Exclude,
        }
    }

    #[fixture]
    fn current(link: LinkConfig) -> HelperSettings {
        HelperSettings {
            enabled: true,
            helper: PathBuf::from("/helper"),
            link: Some(link),
        }
    }

    #[rstest]
    #[case::same(|_: &mut HelperSettings| {}, Change::None)]
    #[case::disabled(|new: &mut HelperSettings| new.enabled = false, Change::Respawn)]
    #[case::other_helper(
        |new: &mut HelperSettings| new.helper = PathBuf::from("/other"),
        Change::Respawn
    )]
    #[case::no_apps(|new: &mut HelperSettings| new.link = None, Change::Respawn)]
    #[case::other_apps(
        |new: &mut HelperSettings| {
            new.link.as_mut().expect("a link").scope = Scope::All;
        },
        Change::Reconfigure
    )]
    #[case::other_timing(
        |new: &mut HelperSettings| new.link.as_mut().expect("a link").poll_interval = None,
        Change::Reconfigure
    )]
    fn change_to(
        current: HelperSettings,
        #[case] edit: fn(&mut HelperSettings),
        #[case] expected: Change,
    ) {
        let mut new = current.clone();
        edit(&mut new);

        assert_eq!(current.change_to(&new), expected);
    }

    /// Settings that don't select any application don't run a helper, so any change that selects
    /// one starts a helper.
    #[rstest]
    fn selecting_apps_respawns(link: LinkConfig) {
        let without = HelperSettings {
            enabled: true,
            helper: PathBuf::from("/helper"),
            link: None,
        };
        let with = HelperSettings {
            link: Some(link),
            ..without.clone()
        };

        assert!(!without.captures());
        assert!(with.captures());
        assert_eq!(without.change_to(&without), Change::None);
        assert_eq!(without.change_to(&with), Change::Respawn);
    }
}

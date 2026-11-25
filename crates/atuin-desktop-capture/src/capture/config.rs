//! How capture is configured.

use std::time::Duration;

use atuin_common::string::StrExt as _;

use crate::backend::{Limits, PrivateWindows, WindowKind};

/// When capture takes snapshots.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// How long a window must stay quiet after it is activated, retitled, loaded, or
    /// rearranged. Switching a tab rearranges a window in one burst of events.
    pub activation_settle: Duration,
    /// How long a window must stay quiet after its text is edited. Typing sends an event
    /// for every key.
    pub content_settle: Duration,
    /// The longest that a continuous stream of changes, or a document that keeps loading, can
    /// postpone a snapshot.
    pub max_wait: Duration,
    /// The shortest time between two snapshots that content changes trigger.
    pub min_interval: Duration,
    /// The longest that this time grows to for a window that is slow to read.
    pub max_interval: Duration,
    /// How long the active window may go without a snapshot before capture reads the window
    /// anyway.
    ///
    /// Events can be missing or misleading, so the interval bounds how stale a snapshot can get.
    pub poll_interval: Option<Duration>,
    /// The longest that the poll interval grows to while polls keep finding nothing new.
    pub max_poll_interval: Duration,
    /// How long after a dialog appears capture reads the dialog, and how long each retry of a
    /// dialog without text waits. A dialog can close a moment after it appears.
    pub dialog_settle: Duration,
    /// The shortest rest between two reads of the sweep. A window that is slow to read is
    /// followed by a longer rest, as long as [`max_interval`](Self::max_interval) at most.
    ///
    /// An application answers a read on its own UI thread, so reading its windows back to
    /// back would stall it.
    pub sweep_rest: Duration,
    /// The shortest time between two reads of the same background window.
    pub background_interval: Duration,
    /// The longest that this time grows to for a background window that is slow to read.
    pub background_max_interval: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            activation_settle: Duration::from_millis(400),
            content_settle: Duration::from_millis(1500),
            max_wait: Duration::from_secs(5),
            min_interval: Duration::from_secs(1),
            max_interval: Duration::from_secs(30),
            poll_interval: Some(Duration::from_secs(30)),
            max_poll_interval: Duration::from_secs(300),
            dialog_settle: Duration::from_millis(100),
            sweep_rest: Duration::from_millis(250),
            background_interval: Duration::from_secs(10),
            background_max_interval: Duration::from_secs(300),
        }
    }
}

/// The applications that a filter starts from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub enum Scope {
    #[default]
    All,
    Only(Vec<String>),
}

/// Selects the applications that may be captured, by case-insensitive name.
#[derive(Debug, Clone, Default)]
pub struct AppFilter {
    pub scope: Scope,
    pub excluded: Vec<String>,
}

impl AppFilter {
    #[must_use]
    pub fn allows(&self, app: &str) -> bool {
        self.allows_app_named([app])
    }

    /// Whether the filter allows an application that goes by every one of `names`: one of the
    /// names is in scope, and none is excluded.
    #[must_use]
    pub fn allows_app_named<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> bool {
        let listed =
            |list: &[String], app: &str| list.iter().any(|name| name.eq_ignoring_case(app));
        let mut in_scope = false;
        for app in names {
            if listed(&self.excluded, app) {
                return false;
            }
            in_scope |= match &self.scope {
                Scope::All => true,
                Scope::Only(names) => listed(names, app),
            };
        }
        in_scope
    }

    /// Whether the filter allows every application, so that it also allows an application
    /// whose name is unknown.
    #[must_use]
    pub const fn allows_every_app(&self) -> bool {
        matches!(self.scope, Scope::All) && self.excluded.is_empty()
    }
}

/// Whether capture reads the windows in the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackgroundReads {
    /// Read every window apart from the active one once when capture starts, and again when
    /// its content changes.
    #[default]
    On,
    Off,
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub timing: Timing,
    pub limits: Limits,
    pub filter: AppFilter,
    pub background: BackgroundReads,
    pub private_windows: PrivateWindows,
}

impl Config {
    /// Whether capture records the content of a window of `kind`.
    #[must_use]
    pub const fn captures(&self, kind: WindowKind) -> bool {
        match kind {
            WindowKind::Other => true,
            WindowKind::Private => matches!(self.private_windows, PrivateWindows::Capture),
            WindowKind::Terminal | WindowKind::BrowserUi => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::everything(Scope::All, &[], "Firefox", true)]
    #[case::excluded(Scope::All, &["firefox"], "Firefox", false)]
    #[case::listed(Scope::Only(vec!["FIREFOX".into()]), &[], "firefox", true)]
    #[case::unlisted(Scope::Only(vec!["kate".into()]), &[], "firefox", false)]
    #[case::listed_but_excluded(Scope::Only(vec!["kate".into()]), &["Kate"], "kate", false)]
    #[case::excluded_in_another_case(Scope::All, &["ÉDITEUR"], "éditeur", false)]
    #[case::listed_in_another_case(Scope::Only(vec!["Écrire".into()]), &[], "éCRIRE", true)]
    fn filter_matches_names_case_insensitively(
        #[case] scope: Scope,
        #[case] excluded: &[&str],
        #[case] app: &str,
        #[case] allowed: bool,
    ) {
        let filter = AppFilter {
            scope,
            excluded: excluded.iter().map(|&name| name.to_owned()).collect(),
        };
        assert_eq!(filter.allows(app), allowed);
    }

    #[rstest]
    #[case::every_name_allowed(Scope::All, &[], &["md.obsidian.Obsidian", "Obsidian"], true)]
    #[case::one_name_excluded(Scope::All, &["obsidian"], &["md.obsidian.Obsidian", "Obsidian"], false)]
    #[case::one_name_listed(
        Scope::Only(vec!["Obsidian".into()]),
        &[],
        &["md.obsidian.Obsidian", "Obsidian"],
        true
    )]
    #[case::no_name_listed(Scope::Only(vec!["kate".into()]), &[], &["a.b", "B"], false)]
    #[case::listed_and_excluded(
        Scope::Only(vec!["md.obsidian.Obsidian".into()]),
        &["Obsidian"],
        &["md.obsidian.Obsidian", "Obsidian"],
        false
    )]
    fn an_app_with_several_names_needs_one_in_scope_and_none_excluded(
        #[case] scope: Scope,
        #[case] excluded: &[&str],
        #[case] names: &[&str],
        #[case] allowed: bool,
    ) {
        let filter = AppFilter {
            scope,
            excluded: excluded.iter().map(|&name| name.to_owned()).collect(),
        };
        assert_eq!(filter.allows_app_named(names.iter().copied()), allowed);
    }

    #[rstest]
    #[case::everything(Scope::All, &[], true)]
    #[case::one_excluded(Scope::All, &["firefox"], false)]
    #[case::listed(Scope::Only(vec!["firefox".into()]), &[], false)]
    fn every_app_is_allowed_only_by_a_filter_without_limits(
        #[case] scope: Scope,
        #[case] excluded: &[&str],
        #[case] allowed: bool,
    ) {
        let filter = AppFilter {
            scope,
            excluded: excluded.iter().map(|&name| name.to_owned()).collect(),
        };
        assert_eq!(filter.allows_every_app(), allowed);
    }
}

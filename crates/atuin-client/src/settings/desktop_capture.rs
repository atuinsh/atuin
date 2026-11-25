//! `[desktop_capture]`: capturing what applications show on the desktop.

use std::borrow::Cow;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use atuin_common::time::{AsUnitsDuration, DisableableUnitsDurationSchema, NonZeroDuration};
use atuin_common::units::Percent;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeSeq as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::serde_as;

use super::DiskUsageLimit;

/// The config key that turns desktop capture on.
pub const ENABLED_KEY: &str = "desktop_capture.enabled";

/// The config key of the applications that desktop capture reads.
pub const APPS_KEY: &str = "desktop_capture.apps";

/// The value of [`APPS_KEY`] that selects every application.
pub const ALL: &str = "all";

/// The message that says how to write the `apps` setting.
const APPS_HINT: &str = "use apps = \"all\" for every application, or a list of application names";

/// The value of the `apps` setting.
///
/// The string `"all"` selects every application, and a list of names selects those applications.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Apps {
    /// Every application.
    All,
    /// The named applications. An empty list selects none.
    Listed(Vec<String>),
}

impl JsonSchema for Apps {
    fn schema_name() -> Cow<'static, str> {
        "Apps".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "`\"all\"` for every application, or a list of application names.",
            "anyOf": [
                { "const": ALL },
                { "type": "array", "items": { "type": "string" } },
            ],
        })
    }
}

impl Default for Apps {
    fn default() -> Self {
        Self::Listed(Vec::new())
    }
}

impl Serialize for Apps {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::All => serializer.serialize_str(ALL),
            Self::Listed(names) => {
                let mut seq = serializer.serialize_seq(Some(names.len()))?;
                for name in names {
                    seq.serialize_element(name)?;
                }
                seq.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Apps {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct AppsVisitor;

        impl<'de> Visitor<'de> for AppsVisitor {
            type Value = Apps;

            /// The end of serde's sentence "invalid type: …, expected …".
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("\"all\" or a list of application names")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Apps, E> {
                if value == ALL {
                    Ok(Apps::All)
                } else {
                    Err(E::custom(format_args!("invalid value {value:?}: {APPS_HINT}")))
                }
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Apps, A::Error> {
                let mut names = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(256));
                while let Some(name) = seq.next_element::<String>()? {
                    if name == "*" {
                        return Err(de::Error::custom(format_args!(
                            "\"*\" is not an application name: {APPS_HINT}"
                        )));
                    }
                    names.push(name);
                }
                Ok(Apps::Listed(names))
            }
        }

        deserializer.deserialize_any(AppsVisitor)
    }
}

/// Whether desktop capture records the windows of a browser's private browsing mode, such as
/// incognito windows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PrivateWindows {
    /// Private windows are left out.
    #[default]
    Exclude,
    /// Private windows are captured like any other window.
    Capture,
}

/// The applications that desktop capture reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppSelection<'a> {
    /// No application: capture does not run.
    None,
    /// Every application.
    All,
    /// Only the named applications.
    Only(&'a [String]),
}

#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct DesktopCapture {
    /// Whether the daemon captures the desktop. Capture also needs [`apps`](Self::apps) to
    /// select an application.
    pub enabled: bool,

    /// The applications to capture: `"all"` or a list of application names.
    pub apps: Apps,

    /// The applications that are never captured, on top of the built-in terminal list.
    pub exclude: Vec<String>,

    /// Whether the private windows of browsers are captured: `"exclude"` or `"capture"`.
    pub private_windows: PrivateWindows,

    /// The capture helper's executable, for development.
    pub helper_path: Option<PathBuf>,

    /// How many minutes of desktop activity the Claude Code prompt hook adds to each prompt.
    /// Setup installs the hook if desktop capture is on.
    pub hook_minutes: u32,

    /// The most disk space that captured windows may use: an absolute size (`10GB`), a share of
    /// the disk that holds the data directory (`10%`), or `unlimited`. Past the limit, the
    /// daemon deletes the oldest windows and notifications first.
    pub max_disk_usage: DiskUsageLimit,

    /// How long the active window may go without a snapshot before capture reads it anyway.
    /// The wait doubles, up to five minutes, while the reads find nothing new. `"0s"` turns
    /// these reads off, so that capture reads a window only when the application reports a
    /// change.
    #[serde_as(as = "AsUnitsDuration")]
    #[schemars(with = "DisableableUnitsDurationSchema")]
    pub poll_interval: Option<NonZeroDuration>,

    /// How long one read of a window may take. A read that takes longer keeps what it read so
    /// far, and capture reads the window again later.
    #[serde_as(as = "AsUnitsDuration")]
    #[schemars(with = "AsUnitsDuration")]
    pub read_budget: Duration,
}

impl Default for DesktopCapture {
    fn default() -> Self {
        Self {
            enabled: false,
            apps: Apps::default(),
            exclude: Vec::new(),
            private_windows: PrivateWindows::Exclude,
            helper_path: None,
            hook_minutes: 10,
            max_disk_usage: DiskUsageLimit::Percent(Percent::new(10.0)),
            poll_interval: NonZeroDuration::new(Duration::from_secs(30)),
            read_budget: Duration::from_millis(1500),
        }
    }
}

impl DesktopCapture {
    /// The applications that [`apps`](Self::apps) selects.
    #[must_use]
    pub fn selection(&self) -> AppSelection<'_> {
        match &self.apps {
            Apps::All => AppSelection::All,
            Apps::Listed(names) if names.is_empty() => AppSelection::None,
            Apps::Listed(names) => AppSelection::Only(names),
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::all(Apps::All, AppSelection::All)]
    #[case::empty(Apps::Listed(Vec::new()), AppSelection::None)]
    #[case::named(
        Apps::Listed(vec!["firefox".to_owned(), "kate".to_owned()]),
        AppSelection::Only(&["firefox".to_owned(), "kate".to_owned()])
    )]
    fn selection(#[case] apps: Apps, #[case] expected: AppSelection<'_>) {
        let config = DesktopCapture {
            apps,
            ..DesktopCapture::default()
        };
        assert_eq!(config.selection(), expected);
    }

    #[rstest]
    #[case::absent("", PrivateWindows::Exclude)]
    #[case::exclude("private_windows = \"exclude\"", PrivateWindows::Exclude)]
    #[case::capture("private_windows = \"capture\"", PrivateWindows::Capture)]
    fn reads_private_windows(#[case] toml: &str, #[case] expected: PrivateWindows) {
        let config: DesktopCapture = toml::from_str(toml).unwrap();
        assert_eq!(config.private_windows, expected);
    }

    #[rstest]
    fn rejects_unknown_private_windows() {
        assert!(toml::from_str::<DesktopCapture>("private_windows = \"include\"").is_err());
    }

    fn listed(names: &[&str]) -> Apps {
        Apps::Listed(names.iter().map(|&name| name.to_owned()).collect())
    }

    #[rstest]
    #[case::all("apps = \"all\"", Apps::All)]
    #[case::empty("apps = []", listed(&[]))]
    #[case::named("apps = [\"firefox\", \"kate\"]", listed(&["firefox", "kate"]))]
    #[case::absent("", listed(&[]))]
    fn deserializes(#[case] toml: &str, #[case] expected: Apps) {
        let config: DesktopCapture = toml::from_str(toml).expect("valid config");
        assert_eq!(config.apps, expected);
    }

    #[rstest]
    #[case::upper("apps = \"ALL\"")]
    #[case::star("apps = \"*\"")]
    #[case::name("apps = \"firefox\"")]
    #[case::star_list("apps = [\"*\"]")]
    #[case::star_among_names("apps = [\"kate\", \"*\"]")]
    fn rejects(#[case] toml: &str) {
        let error = toml::from_str::<DesktopCapture>(toml).expect_err("invalid apps");
        assert!(error.to_string().contains("apps = \"all\""), "{error}");
    }

    /// A value of another type reads as one sentence in serde's message.
    #[rstest]
    #[case::integer(
        "apps = 5",
        "invalid type: integer `5`, expected \"all\" or a list of application names"
    )]
    #[case::table("apps = {}", "expected \"all\" or a list of application names")]
    fn rejects_another_type(#[case] toml: &str, #[case] message: &str) {
        let error = toml::from_str::<DesktopCapture>(toml).expect_err("invalid apps");
        assert!(error.to_string().contains(message), "{error}");
    }

    #[rstest]
    #[case::all(Apps::All, "apps = \"all\"\n")]
    #[case::empty(listed(&[]), "apps = []\n")]
    #[case::named(listed(&["firefox", "kate"]), "apps = [\"firefox\", \"kate\"]\n")]
    fn round_trips(#[case] apps: Apps, #[case] text: &str) {
        #[derive(Debug, Deserialize, Serialize, PartialEq)]
        struct Wrapper {
            apps: Apps,
        }
        let wrapper = Wrapper { apps };
        assert_eq!(toml::to_string(&wrapper).expect("serializes"), text);
        assert_eq!(toml::from_str::<Wrapper>(text).expect("parses"), wrapper);
    }
}

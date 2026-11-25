//! The protocol between the helper and the daemon.
//!
//! The helper reads the desktop and sends [`HelperFrame`]s to the daemon, and the daemon sends
//! [`DaemonFrame`]s back. Both sides exchange them as the frames of
//! [`atuin_common::rmp::frame`] on a byte stream. The protocol is private to one build of
//! Atuin, so the helper announces its build in [`HelperFrame::Hello`] and the daemon decides
//! whether it can talk to that build.
//!
//! [`process`] holds the rest of the contract between the daemon and the helper's process:
//! the exit codes of the helper and the variables that the daemon sets for the helper.

pub mod process;

use std::time::Duration;

use atuin_common::secrets::BuiltinSecrets;
use serde::{Deserialize, Serialize};

use crate::backend::PrivateWindows;
use crate::capture::{Record, Scope};

/// A message from the helper to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelperFrame {
    /// The first frame of every connection.
    Hello {
        /// The build of the helper.
        build: String,
    },
    Status(HelperStatus),
    Record(Record),
    /// The answer to [`DaemonFrame::Ping`].
    Pong,
}

/// What the helper reports about its access to the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelperStatus {
    Connected,
    /// The desktop refuses the helper's reads until the user grants access.
    NeedsPermission,
    Paused,
    Resumed,
    /// The screen locked, and the helper reads nothing until the screen unlocks.
    ScreenLocked,
    /// The screen unlocked, and the helper captures again. A helper whose desktop reports the
    /// state of the screen also sends this report when capture starts or resumes on an unlocked
    /// screen.
    ScreenUnlocked,
}

/// A message from the daemon to the helper.
///
/// `C` is the form of the configuration. The daemon sends a `DaemonFrame<&LinkConfig>`, which
/// writes the same bytes as the `DaemonFrame` that the helper reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DaemonFrame<C = LinkConfig> {
    /// Sets what the helper captures, and whether the helper captures. The daemon sends this
    /// frame right after the helper's hello, and again whenever the configuration changes.
    Configure {
        config: C,
        mode: Mode,
    },
    Pause,
    Resume,
    Shutdown,
    /// Asks the helper to answer with [`HelperFrame::Pong`], which it does in every phase, so
    /// that the daemon can tell a helper that stopped answering.
    Ping,
}

/// Whether the helper reads the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Running,
    /// The helper reads nothing until the daemon sends [`DaemonFrame::Resume`], or a
    /// [`DaemonFrame::Configure`] in [`Mode::Running`].
    Paused,
}

/// What the helper captures and what it removes from the capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkConfig {
    pub scope: Scope,
    /// The names of applications that the helper never captures.
    pub excluded: Vec<String>,
    /// Regular expressions for secrets, in addition to the builtin patterns.
    pub secret_patterns: Vec<String>,
    /// Whether the helper removes the secrets that Atuin recognizes by default.
    pub builtin_secrets: BuiltinSecrets,
    /// How long the active window may go without a snapshot before the helper reads it anyway.
    /// With `None`, the helper reads only on events.
    pub poll_interval: Option<Duration>,
    /// How long one read of a window may take.
    pub read_budget: Duration,
    pub private_windows: PrivateWindows,
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use atuin_common::rmp::frame::{read_frame, write_frame};
    use proptest::prelude::*;
    use proptest::test_runner::TestRunner;
    use rstest::rstest;
    use serde::de::DeserializeOwned;

    use super::*;
    use crate::backend::{Completeness, Mark, Node, ReadStats, Role, Visibility};
    use crate::capture::{
        Background, Content, ContentHash, Dialog, EncodedTree, HashMismatch, Notification, Segment,
        SegmentId, Snapshot, SnapshotContent, Trigger, WindowId,
    };

    fn role() -> impl Strategy<Value = Role> {
        prop_oneof![
            Just(Role::Container),
            Just(Role::Window),
            Just(Role::Document),
            Just(Role::Heading),
            Just(Role::Paragraph),
            Just(Role::ListItem),
            Just(Role::Cell),
            Just(Role::Row),
            Just(Role::ColumnHeader),
            Just(Role::Label),
            Just(Role::InlineText),
            Just(Role::Link),
            Just(Role::Editable),
            Just(Role::Secret),
            Just(Role::Terminal),
            Just(Role::Controls),
        ]
    }

    fn leaf() -> impl Strategy<Value = Node> {
        (
            role(),
            ".{0,12}",
            proptest::option::of(".{0,12}"),
            prop_oneof![Just(Visibility::Showing), Just(Visibility::Hidden)],
            ".{0,12}",
            proptest::option::of(".{0,24}"),
            proptest::option::of(1u8..=6),
            prop_oneof![Just(Mark::None), Just(Mark::Checked), Just(Mark::Unchecked)],
        )
            .prop_map(
                |(role, native_role, url, visibility, name, text, heading_level, mark)| Node {
                    role,
                    native_role: native_role.into(),
                    url,
                    visibility,
                    name,
                    text,
                    heading_level,
                    mark,
                    ..Node::default()
                },
            )
    }

    fn tree() -> impl Strategy<Value = Node> {
        leaf().prop_recursive(4, 40, 4, |inner| {
            (leaf(), proptest::collection::vec(inner, 0..4))
                .prop_map(|(node, children)| Node { children, ..node })
        })
    }

    fn time() -> impl Strategy<Value = SystemTime> {
        (0u64..4_000_000_000, 0u32..1_000_000_000).prop_map(|(secs, nanos)| {
            SystemTime::UNIX_EPOCH
                + Duration::from_secs(secs)
                + Duration::from_nanos(u64::from(nanos))
        })
    }

    fn trigger() -> impl Strategy<Value = Trigger> {
        prop_oneof![
            Just(Trigger::Activated),
            Just(Trigger::TitleChanged),
            Just(Trigger::Loaded),
            Just(Trigger::Edited),
            Just(Trigger::Restructured),
            Just(Trigger::Polled),
            Just(Trigger::Swept),
            Just(Trigger::Retried),
        ]
    }

    fn read_stats() -> impl Strategy<Value = ReadStats> {
        (
            0usize..100_000,
            prop_oneof![
                Just(Completeness::Complete),
                Just(Completeness::Truncated),
                Just(Completeness::OutOfTime),
            ],
            0usize..1_000,
        )
            .prop_map(|(nodes, completeness, unknown_roles)| ReadStats {
                nodes,
                completeness,
                unknown_roles,
            })
    }

    fn content() -> impl Strategy<Value = Content> {
        (tree(), (0u64..60_000).prop_map(Duration::from_millis), read_stats()).prop_map(
            |(root, read_time, read)| Content {
                tree: EncodedTree::encode(&root),
                read_time,
                read,
            },
        )
    }

    fn snapshot_content() -> impl Strategy<Value = SnapshotContent> {
        prop_oneof![
            content().prop_map(SnapshotContent::Changed),
            (tree(), (0u64..60_000).prop_map(Duration::from_millis), read_stats()).prop_map(
                |(root, read_time, read)| SnapshotContent::Unchanged {
                    hash: ContentHash::of(&root),
                    read_time,
                    read,
                }
            ),
        ]
    }

    /// What every record about a window says about the window, besides its content.
    #[derive(Debug)]
    struct About {
        at: SystemTime,
        trigger: Trigger,
        app: String,
        title: String,
        url: Option<String>,
    }

    fn about() -> impl Strategy<Value = About> {
        (time(), trigger(), ".{0,12}", ".{0,12}", proptest::option::of(".{0,12}")).prop_map(
            |(at, trigger, app, title, url)| About {
                at,
                trigger,
                app,
                title,
                url,
            },
        )
    }

    fn record() -> impl Strategy<Value = Record> {
        let ids = (any::<u64>(), any::<u64>());
        prop_oneof![
            (ids, about()).prop_map(|((id, window), about)| Record::SegmentStarted(Segment {
                id: SegmentId(id),
                window: WindowId(window),
                app: about.app,
                title: about.title,
                url: about.url,
                started: about.at,
            })),
            (ids, about(), snapshot_content()).prop_map(|((id, _), about, content)| {
                Record::Snapshot(Snapshot {
                    segment: SegmentId(id),
                    at: about.at,
                    trigger: about.trigger,
                    content,
                })
            }),
            (any::<u64>(), time()).prop_map(|(id, at)| Record::SegmentEnded {
                segment: SegmentId(id),
                at
            }),
            (ids, about(), content()).prop_map(|((_, window), about, content)| {
                Record::Dialog(Dialog {
                    window: WindowId(window),
                    app: about.app,
                    title: about.title,
                    at: about.at,
                    content,
                })
            }),
            (".{0,12}", ".{0,12}", time(), content()).prop_map(|(app, title, at, content)| {
                Record::Notification(Notification {
                    app,
                    title,
                    at,
                    content,
                })
            }),
            (ids, about(), content()).prop_map(|((_, window), about, content)| {
                Record::Background(Background {
                    window: WindowId(window),
                    app: about.app,
                    title: about.title,
                    url: about.url,
                    at: about.at,
                    trigger: about.trigger,
                    content,
                })
            }),
        ]
    }

    fn config() -> impl Strategy<Value = LinkConfig> {
        (
            prop_oneof![
                Just(Scope::All),
                proptest::collection::vec(".{0,8}", 0..4).prop_map(Scope::Only),
            ],
            proptest::collection::vec(".{0,8}", 0..4),
            proptest::collection::vec(".{0,8}", 0..4),
            prop_oneof![Just(BuiltinSecrets::On), Just(BuiltinSecrets::Off)],
            proptest::option::of(any::<u64>().prop_map(Duration::from_millis)),
            any::<u64>().prop_map(Duration::from_millis),
            prop_oneof![Just(PrivateWindows::Exclude), Just(PrivateWindows::Capture)],
        )
            .prop_map(
                |(
                    scope,
                    excluded,
                    secret_patterns,
                    builtin_secrets,
                    poll_interval,
                    read_budget,
                    private_windows,
                )| {
                    LinkConfig {
                        scope,
                        excluded,
                        secret_patterns,
                        builtin_secrets,
                        poll_interval,
                        read_budget,
                        private_windows,
                    }
                },
            )
    }

    fn daemon_frame() -> impl Strategy<Value = DaemonFrame> {
        prop_oneof![
            (config(), prop_oneof![Just(Mode::Running), Just(Mode::Paused)])
                .prop_map(|(config, mode)| DaemonFrame::Configure { config, mode }),
            Just(DaemonFrame::Pause),
            Just(DaemonFrame::Resume),
            Just(DaemonFrame::Shutdown),
            Just(DaemonFrame::Ping),
        ]
    }

    fn helper_frame() -> impl Strategy<Value = HelperFrame> {
        prop_oneof![
            ".{0,12}".prop_map(|build| HelperFrame::Hello { build }),
            prop_oneof![
                Just(HelperStatus::Connected),
                Just(HelperStatus::NeedsPermission),
                Just(HelperStatus::Paused),
                Just(HelperStatus::Resumed),
                Just(HelperStatus::ScreenLocked),
                Just(HelperStatus::ScreenUnlocked),
            ]
            .prop_map(HelperFrame::Status),
            record().prop_map(HelperFrame::Record),
            Just(HelperFrame::Pong),
        ]
    }

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(future)
    }

    async fn round_trip<F>(frame: &F) -> F
    where
        F: Serialize + DeserializeOwned,
    {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, frame).await.unwrap();
        let mut reader = bytes.as_slice();
        let back = read_frame(&mut reader).await.unwrap().unwrap();
        assert!(read_frame::<_, F>(&mut reader).await.unwrap().is_none());
        back
    }

    #[rstest]
    fn frames_round_trip() {
        TestRunner::default()
            .run(&daemon_frame(), |frame| {
                prop_assert_eq!(block_on(round_trip(&frame)), frame);
                Ok(())
            })
            .unwrap();
        TestRunner::default()
            .run(&helper_frame(), |frame| {
                let back = block_on(round_trip(&frame));
                if let HelperFrame::Record(Record::Snapshot(Snapshot {
                    content: SnapshotContent::Changed(content),
                    ..
                })) = &back
                {
                    let tree = &content.tree;
                    prop_assert_eq!(tree.verify(), Ok(()));
                    prop_assert_eq!(tree.hash(), ContentHash::of(&tree.decode().unwrap()));
                }
                prop_assert_eq!(back, frame);
                Ok(())
            })
            .unwrap();
    }

    #[rstest]
    #[tokio::test]
    async fn a_borrowed_configuration_reads_as_an_owned_one() {
        let config = LinkConfig {
            scope: Scope::Only(vec!["kate".to_owned()]),
            excluded: vec!["mail".to_owned()],
            secret_patterns: vec!["hunter2".to_owned()],
            builtin_secrets: BuiltinSecrets::On,
            poll_interval: Some(Duration::from_secs(30)),
            read_budget: Duration::from_millis(1500),
            private_windows: PrivateWindows::Capture,
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &DaemonFrame::Configure {
            config: &config,
            mode: Mode::Paused,
        })
        .await
        .unwrap();
        let back: DaemonFrame = read_frame(&mut bytes.as_slice()).await.unwrap().unwrap();
        assert_eq!(back, DaemonFrame::Configure {
            config,
            mode: Mode::Paused
        });
    }

    #[rstest]
    #[tokio::test]
    async fn ping_and_pong_round_trip() {
        let ping: DaemonFrame = DaemonFrame::Ping;
        assert_eq!(round_trip(&ping).await, ping);
        assert_eq!(round_trip(&HelperFrame::Pong).await, HelperFrame::Pong);
    }

    #[rstest]
    fn a_tree_whose_bytes_changed_on_the_way_fails_verification() {
        let root = Node {
            name: "Inbox".to_owned(),
            ..Node::default()
        };
        let mut message = rmp_serde::to_vec_named(&EncodedTree::encode(&root)).unwrap();
        let at = message.windows(5).position(|window| window == b"Inbox").unwrap();
        message[at..at + 5].copy_from_slice(b"Spam!");

        let tree: EncodedTree = rmp_serde::from_slice(&message).unwrap();

        assert_eq!(tree.verify(), Err(HashMismatch));
        assert_eq!(tree.decode().unwrap().name, "Spam!");
    }
}

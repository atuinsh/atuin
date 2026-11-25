use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use rstest::{fixture, rstest};
use tokio::time::Instant;

use super::*;
use crate::backend::{
    AppIdentity, Cause, Event, Extent, Limits, Node, Notice, PrivateWindows, Screen, WindowClass,
    WindowKind,
};
use crate::testing::{AppName, Page, Script, at};

pub const MS: Duration = Duration::from_millis(1);

#[fixture]
pub fn timing() -> Timing {
    Timing {
        activation_settle: 400 * MS,
        content_settle: 1500 * MS,
        max_wait: 5000 * MS,
        min_interval: 10_000 * MS,
        max_interval: 40_000 * MS,
        poll_interval: None,
        max_poll_interval: 240_000 * MS,
        dialog_settle: 100 * MS,
        sweep_rest: 250 * MS,
        background_interval: 10_000 * MS,
        background_max_interval: 300_000 * MS,
    }
}

/// The text of `content`, cut at `max_bytes`.
fn rendered(content: &Content, max_bytes: usize) -> String {
    crate::render(&content.tree.decode().expect("a captured tree decodes"), max_bytes).text
}

fn activated(window: &'static str) -> Event<Script> {
    Event::Activated { window }
}

fn deactivated(window: &'static str) -> Event<Script> {
    Event::Deactivated { window }
}

/// A configuration with `timing` that leaves out the application named `secrets`.
#[fixture]
fn without_secrets(timing: Timing) -> Config {
    Config {
        timing,
        filter: AppFilter {
            scope: Scope::All,
            excluded: vec!["secrets".into()],
        },
        ..Config::default()
    }
}

/// A capture of `script` with `timing` and the rest of the configuration by default.
fn capture(script: Script, timing: Timing) -> Capture<Script> {
    Capture::new(script, Config {
        timing,
        ..Config::default()
    })
}

/// Takes every record of `capture` until capture stops.
async fn drain(capture: &mut Capture<Script>) -> Vec<Record> {
    let mut records = Vec::new();
    while let Some(record) = capture.next().await {
        records.push(record);
    }
    records
}

/// The triggers of the snapshots among `records`.
fn snapshot_triggers(records: Vec<Record>) -> Vec<Trigger> {
    let snapshot = |record| match record {
        Record::Snapshot(snapshot) => Some(snapshot.trigger),
        _ => None,
    };
    records.into_iter().filter_map(snapshot).collect()
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_stopped_capture_gives_its_backend_back(timing: Timing) {
    let script = Script {
        apps: vec!["editor"],
        active: Some("editor/1"),
        ..Script::new([at(60_000, deactivated("editor/1"))], [Page::new(
            "editor/1",
            "fn main() {}",
        )])
    };
    let config = Config {
        timing,
        ..Config::default()
    };
    let mut capture = Capture::new(script, config.clone());
    assert!(matches!(capture.next().await, Some(Record::SegmentStarted(_))));
    capture.stop(WaitingEdit::Read).await;
    drain(&mut capture).await;

    let mut capture = Capture::new(capture.into_stopped().parts.backend, config);
    let Some(Record::SegmentStarted(segment)) = capture.next().await else {
        panic!("the second capture starts a segment for the active window");
    };
    assert_eq!(segment.window, WindowId(1));
}

/// A capture that continues from the ids of a stopped one hands out only new ids, so that
/// a store that outlives one capture never mixes up two windows or two segments.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_resumed_capture_never_reuses_an_id(timing: Timing) {
    let script = Script {
        apps: vec!["editor", "mail"],
        active: Some("editor/1"),
        ..Script::new([at(2_000, activated("mail/1")), at(60_000, deactivated("mail/1"))], [
            Page::new("editor/1", "fn main() {}"),
            Page::new("mail/1", "inbox"),
        ])
    };
    let config = Config {
        timing,
        background: BackgroundReads::Off,
        ..Config::default()
    };
    let mut seen = Ids::default();
    let mut capture = Capture::new(script, config.clone());
    let Some(Record::SegmentStarted(first)) = capture.next().await else {
        panic!("the first capture starts a segment for the active window");
    };
    seen.add(&first);
    capture.stop(WaitingEdit::Read).await;
    for record in drain(&mut capture).await {
        seen.add_record(&record);
    }

    let mut capture = Capture::resume(capture.into_stopped().parts, config);
    let mut resumed = Ids::default();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && resumed.segments.len() < 2 {
        let Some(record) = capture.next().await else {
            break;
        };
        resumed.add_record(&record);
    }
    assert_eq!(resumed.segments.len(), 2, "the resumed capture follows both windows");
    assert!(seen.segments.is_disjoint(&resumed.segments), "{seen:?} {resumed:?}");
    assert!(seen.windows.is_disjoint(&resumed.windows), "{seen:?} {resumed:?}");
}

/// The ids that a test saw in records.
#[derive(Debug, Default)]
struct Ids {
    segments: HashSet<SegmentId>,
    windows: HashSet<WindowId>,
}

impl Ids {
    fn add(&mut self, segment: &Segment) {
        self.segments.insert(segment.id);
        self.windows.insert(segment.window);
    }

    fn add_record(&mut self, record: &Record) {
        match record {
            Record::SegmentStarted(segment) => self.add(segment),
            Record::Dialog(Dialog { window, .. })
            | Record::Background(Background { window, .. }) => {
                self.windows.insert(*window);
            }
            Record::Snapshot(_) | Record::SegmentEnded { .. } | Record::Notification(_) => {}
        }
    }
}

#[rstest]
#[case::loads_in_time(2, 1_200)]
#[case::never_finishes(u32::MAX, 5_600)]
#[tokio::test(start_paused = true)]
async fn a_loading_document_postpones_the_snapshot(
    timing: Timing,
    #[case] loading_reads: u32,
    #[case] snapshot_ms: u64,
) {
    let start = Instant::now();
    let script = Script {
        loading_reads: AtomicU32::new(loading_reads),
        ..Script::new([at(0, activated("browser/1")), at(60_000, deactivated("browser/1"))], [
            Page::new("browser/1", "Build failed"),
        ])
    };
    let mut capture = capture(script, timing);

    // The window settles after 400 ms, and every read of a loading page costs another 400.
    // The longest wait of 5 s runs from the first read, and the next read after that counts.
    assert!(matches!(capture.next().await, Some(Record::SegmentStarted(_))));
    assert_eq!(start.elapsed(), Duration::from_millis(snapshot_ms));
    let Some(Record::Snapshot(snapshot)) = capture.next().await else {
        panic!("the snapshot follows the start of its segment");
    };
    assert_eq!(snapshot.trigger, Trigger::Activated);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_popup_leaves_its_window_in_one_segment(timing: Timing) {
    let script = Script::new(
        [
            at(0, activated("dialogs/settings")),
            // Opening a drop-down list hands the active state to the popup.
            at(2_000, deactivated("dialogs/settings")),
            at(2_010, activated("dialogs/popup")),
            at(9_000, activated("dialogs/settings")),
            at(20_000, activated("editor/1")),
            at(25_000, deactivated("editor/1")),
        ],
        [Page::new("dialogs/settings", "Tab width"), Page::new("editor/1", "fn main() {}")],
    );
    let script = Script {
        active_windows: HashMap::from([("dialogs", "dialogs/popup")]),
        classes: HashMap::from([("dialogs/popup", WindowClass::Popup)]),
        ..script
    };
    let mut capture = capture(script, timing);
    let mut segments = Vec::new();
    for record in drain(&mut capture).await {
        if let Record::SegmentStarted(segment) = record {
            segments.push(segment.app);
        }
    }
    assert_eq!(segments, ["dialogs", "editor"]);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn an_edit_is_read_before_capture_leaves_the_window(timing: Timing) {
    let script = Script::new(
        [
            at(0, activated("editor/1")),
            at(3_000, Event::ContentChanged {
                app: "editor",
                window: Some("editor/1"),
                cause: Cause::Edited,
            }),
            // The user leaves before the edit settles.
            at(3_200, activated("editor/2")),
            at(6_000, deactivated("editor/2")),
        ],
        [Page::new("editor/1", "a reply"), Page::new("editor/2", "notes")],
    );
    let mut capture = Capture::new(script, Config {
        timing,
        background: BackgroundReads::Off,
        ..Config::default()
    });
    let triggers = snapshot_triggers(drain(&mut capture).await);
    assert_eq!(triggers, [Trigger::Activated, Trigger::Edited, Trigger::Activated]);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_change_counts_only_in_the_window_that_capture_follows(timing: Timing) {
    let content = |window| Event::ContentChanged {
        app: "browser",
        window,
        cause: Cause::Edited,
    };
    let script = Script::new(
        [
            at(0, activated("browser/1")),
            // A chat in another window of the same application receives a message.
            at(5_000, content(Some("browser/2"))),
            at(12_000, content(Some("browser/1"))),
            // A change from a window that the platform could not tell can be anywhere in
            // the application.
            at(30_000, content(None)),
            at(40_000, deactivated("browser/1")),
        ],
        [Page::new("browser/1", "Build failed"), Page::new("browser/2", "new message")],
    );
    let mut capture = capture(script, timing);
    let mut triggers = Vec::new();
    let mut behind = Vec::new();
    for record in drain(&mut capture).await {
        match record {
            Record::Snapshot(snapshot) => triggers.push(snapshot.trigger),
            Record::Background(background) => behind.push(background.title),
            _ => {}
        }
    }
    assert_eq!(triggers, [Trigger::Activated, Trigger::Edited, Trigger::Edited]);
    // The change in the other window is read there, in the background.
    assert_eq!(behind, ["title of browser/2"]);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_background_window_is_read_again_when_it_changes(timing: Timing) {
    let edit = |ms| {
        at(ms, Event::ContentChanged {
            app: "chat",
            window: Some("chat/1"),
            cause: Cause::Edited,
        })
    };
    let script = Script {
        apps: vec!["editor", "chat"],
        active: Some("editor/1"),
        later_pages: vec![at(15_000, Page::new("chat/1", "hello\nthe deadline is Friday"))],
        ..Script::new(
            [
                // The window rests for 10 s after the sweep read it, and nothing changed.
                edit(2_000),
                // The message arrives, and the window is read once it has rested again.
                edit(16_000),
                // A burst of changes leads to one read, which finds nothing new.
                edit(30_000),
                edit(30_500),
                edit(31_000),
                // The user reads the chat, and leaves it.
                at(40_000, activated("chat/1")),
                at(50_000, deactivated("chat/1")),
                // The window goes on from what the user saw.
                edit(55_000),
                // Capture stops when the events run out.
                at(60_000, deactivated("chat/1")),
            ],
            [Page::new("editor/1", "fn main() {}"), Page::new("chat/1", "hello")],
        )
    };
    let mut capture = capture(script, timing);
    let mut behind = Vec::new();
    for record in drain(&mut capture).await {
        if let Record::Background(background) = record {
            behind.push((background.trigger, rendered(&background.content, 1000)));
        }
    }
    assert_eq!(behind, [
        (Trigger::Swept, "hello".to_owned()),
        (Trigger::Edited, "hello\nthe deadline is Friday".to_owned()),
    ]);
    // The sweep, three reads in the background, the snapshot while the chat was active,
    // and one more read in the background.
    assert_eq!(capture.backend().reads.lock()["chat/1"], 6);
}

/// A change in a background window whose read fails is read when the window has rested, and a
/// window whose reads keep failing is given up.
#[rstest]
#[case::once(1, &[Trigger::Swept, Trigger::Edited], 3)]
#[case::always(u32::MAX, &[Trigger::Swept], 5)]
#[tokio::test(start_paused = true)]
async fn a_background_read_that_fails_is_tried_again(
    timing: Timing,
    #[case] failing_reads: u32,
    #[case] expected: &[Trigger],
    #[case] reads: u32,
) {
    let script = Script {
        apps: vec!["editor", "chat"],
        active: Some("editor/1"),
        later_pages: vec![at(15_000, Page::new("chat/1", "hello\nthe deadline is Friday"))],
        ..Script::new(
            [
                at(16_000, Event::ContentChanged {
                    app: "chat",
                    window: Some("chat/1"),
                    cause: Cause::Edited,
                }),
                at(120_000, deactivated("editor/1")),
            ],
            [Page::new("editor/1", "fn main() {}"), Page::new("chat/1", "hello")],
        )
    };
    let mut capture = capture(script, timing);
    let mut behind = Vec::new();
    while let Some(record) = capture.next().await {
        match record {
            Record::Background(background) => behind.push(background.trigger),
            // The reads that follow the sweep and the editor's snapshot fail.
            Record::Snapshot(_) => {
                capture.backend().failing_reads.store(failing_reads, Ordering::Relaxed);
            }
            _ => {}
        }
    }
    assert_eq!(behind, expected);
    assert_eq!(capture.backend().reads.lock()["chat/1"], reads);
}

/// A background window whose reads failed until capture gave up keeps what capture last
/// handed out for it, so its next read of the same content doesn't hand it out again.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_background_window_that_recovers_keeps_its_content(timing: Timing) {
    let edit = |ms| at(ms, changed("chat/1", Cause::Edited));
    let script = Script {
        apps: vec!["editor", "chat"],
        active: Some("editor/1"),
        ..Script::new(
            [
                edit(16_000),
                // The application answers again, and the window shows what it showed.
                edit(120_000),
                at(180_000, deactivated("editor/1")),
            ],
            [Page::new("editor/1", "fn main() {}"), Page::new("chat/1", "hello")],
        )
    };
    let mut capture = capture(script, timing);
    let mut behind = Vec::new();
    while let Some(record) = capture.next().await {
        match record {
            Record::Background(background) => behind.push(background.trigger),
            // The four reads that follow the sweep and the editor's snapshot fail.
            Record::Snapshot(_) => capture.backend().failing_reads.store(4, Ordering::Relaxed),
            _ => {}
        }
    }
    assert_eq!(behind, [Trigger::Swept]);
    // The sweep, the failed read with its three retries, and the read after recovery.
    assert_eq!(capture.backend().reads.lock()["chat/1"], 6);
}

#[rstest]
#[case::once(1, &[Trigger::Activated, Trigger::Retried])]
#[case::always(u32::MAX, &[Trigger::Activated, Trigger::Retried, Trigger::Retried, Trigger::Retried])]
#[tokio::test(start_paused = true)]
async fn a_read_that_runs_out_of_time_is_tried_again(
    timing: Timing,
    #[case] slow_reads: u32,
    #[case] expected: &[Trigger],
) {
    let script = Script {
        slow_reads: AtomicU32::new(slow_reads),
        ..Script::new([at(0, activated("browser/1")), at(60_000, deactivated("browser/1"))], [
            Page::new("browser/1", "Build failed"),
        ])
    };
    let mut capture = capture(script, timing);
    let triggers = snapshot_triggers(drain(&mut capture).await);
    // The partial snapshot is kept, and each retry waits for the window to rest.
    assert_eq!(triggers, expected);
}

#[rstest]
#[case::once(1, 2, &[Trigger::Activated])]
#[case::always(u32::MAX, 4, &[])]
#[tokio::test(start_paused = true)]
async fn a_read_that_fails_is_tried_again(
    timing: Timing,
    #[case] failing_reads: u32,
    #[case] reads: u32,
    #[case] expected: &[Trigger],
) {
    let script = Script {
        failing_reads: AtomicU32::new(failing_reads),
        ..Script::new([at(0, activated("browser/1")), at(60_000, deactivated("browser/1"))], [
            Page::new("browser/1", "Build failed"),
        ])
    };
    let mut capture = capture(script, timing);
    let triggers = snapshot_triggers(drain(&mut capture).await);
    // A window that never answers is given up after three retries.
    assert_eq!(triggers, expected);
    assert_eq!(capture.backend().reads.lock()["browser/1"], reads);
}

fn changed(window: &'static str, cause: Cause) -> Event<Script> {
    Event::ContentChanged {
        app: window.split_once('/').map_or(window, |(app, _)| app),
        window: Some(window),
        cause,
    }
}

#[rstest]
#[case::an_edit(&[Cause::Edited], Trigger::Edited)]
// The burst takes the trigger of its first change, and the edit still waits.
#[case::an_edit_after_a_rearrangement(&[Cause::Restructured, Cause::Edited], Trigger::Restructured)]
#[tokio::test(start_paused = true)]
async fn stopping_reads_or_discards_an_edit_that_waits(
    timing: Timing,
    #[case] causes: &[Cause],
    #[case] trigger: Trigger,
    #[values(WaitingEdit::Read, WaitingEdit::Discard)] edit: WaitingEdit,
) {
    let changes = (3_000..).step_by(10).zip(causes);
    let script = Script {
        events: std::iter::once(at(0, activated("editor/1")))
            .chain(changes.map(|(ms, &cause)| at(ms, changed("editor/1", cause))))
            .chain([at(60_000, deactivated("editor/1"))])
            .collect(),
        ..Script::new([], [Page::new("editor/1", "a reply")])
    };
    let mut capture = capture(script, timing);
    let mut triggers = Vec::new();
    // Capture stops before the changes settle.
    let stop_at = capture.backend().start + 3_100 * MS;
    while let Ok(Some(record)) = tokio::time::timeout_at(stop_at, capture.next()).await {
        if let Record::Snapshot(snapshot) = record {
            triggers.push(snapshot.trigger);
        }
    }
    capture.stop(edit).await;
    for record in drain(&mut capture).await {
        if let Record::Snapshot(snapshot) = record {
            triggers.push(snapshot.trigger);
        }
    }
    let expected = match edit {
        WaitingEdit::Read => vec![Trigger::Activated, trigger],
        WaitingEdit::Discard => vec![Trigger::Activated],
    };
    assert_eq!(triggers, expected);
}

/// A store that failed to keep a snapshot gets the content of a window that stays the same
/// again, once the window has sent only unchanged snapshots for the longest poll interval.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_window_that_stays_the_same_sends_its_content_again(timing: Timing) {
    let script =
        Script::new([at(0, activated("editor/1")), at(200_000, deactivated("editor/1"))], [
            Page::new("editor/1", "fn main() {}"),
        ]);
    let mut capture = capture(script, Timing {
        poll_interval: Some(30_000 * MS),
        max_poll_interval: 60_000 * MS,
        ..timing
    });
    let changes: Vec<_> = drain(&mut capture)
        .await
        .into_iter()
        .filter_map(|record| match record {
            Record::Snapshot(snapshot) => Some(snapshot.content.change()),
            _ => None,
        })
        .collect();
    // The poll at 30 s finds nothing new. The polls at 90 s and 150 s send the content again,
    // because a minute has passed since the content last went out in full. The polls count
    // as unchanged, so the polls keep their longest interval.
    assert_eq!(changes, [Change::Changed, Change::Unchanged, Change::Changed, Change::Changed,]);
}

/// A daemon kills a helper that takes longer than a couple of seconds to stop, and the
/// segment would never end.
#[rstest]
#[tokio::test(start_paused = true)]
async fn stopping_gives_up_on_an_edit_that_takes_too_long_to_read(timing: Timing) {
    let script = Script {
        read_time: 10_000 * MS,
        ..Script::new(
            [
                at(0, activated("editor/1")),
                at(11_000, changed("editor/1", Cause::Edited)),
                at(60_000, deactivated("editor/1")),
            ],
            [Page::new("editor/1", "a reply")],
        )
    };
    let mut capture = capture(script, timing);
    let stop_at = capture.backend().start + 11_100 * MS;
    let mut records = Vec::new();
    while let Ok(Some(record)) = tokio::time::timeout_at(stop_at, capture.next()).await {
        records.push(record);
    }
    let stopping = Instant::now();
    capture.stop(WaitingEdit::Read).await;
    assert!(stopping.elapsed() <= STOP_READ_TIME, "{:?}", stopping.elapsed());
    records.extend(drain(&mut capture).await);
    let triggers: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            Record::Snapshot(snapshot) => Some(snapshot.trigger),
            _ => None,
        })
        .collect();
    assert_eq!(triggers, [Trigger::Activated]);
    assert!(matches!(records.last(), Some(Record::SegmentEnded { .. })), "{records:?}");
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_background_read_that_runs_out_of_time_is_tried_again(timing: Timing) {
    let script = Script {
        apps: vec!["editor", "chat"],
        active: Some("editor/1"),
        ..Script::new([at(30_000, deactivated("editor/1"))], [
            Page::new("editor/1", "fn main() {}"),
            Page::new("chat/1", "hello"),
        ])
    };
    // The sweep reads the chat first, while the editor's snapshot waits to settle.
    script.slow_reads.store(1, Ordering::Relaxed);
    let mut capture = capture(script, timing);
    drain(&mut capture).await;
    // The sweep, and once more after the window has rested.
    assert_eq!(capture.backend().reads.lock()["chat/1"], 2);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn an_alert_that_takes_the_focus_is_read_before_it_closes(timing: Timing) {
    let script = Script {
        // The alert is gone before the deactivated window's application is asked again.
        closes: HashMap::from([("files/alert", 5_150)]),
        classes: HashMap::from([("files/alert", WindowClass::Dialog)]),
        ..Script::new(
            [
                at(0, activated("files/1")),
                at(5_000, deactivated("files/1")),
                at(5_010, Event::Opened {
                    window: "files/alert",
                }),
                at(10_000, activated("files/1")),
            ],
            [Page::new("files/1", "Documents"), Page::new("files/alert", "Disk full")],
        )
    };
    let mut capture = capture(script, timing);
    let mut dialogs = Vec::new();
    for record in drain(&mut capture).await {
        if let Record::Dialog(dialog) = record {
            dialogs.push(rendered(&dialog.content, 100));
        }
    }
    assert_eq!(dialogs, ["Disk full"]);
}

/// The dialog check that follows an activation reuses the class that the activation found,
/// because each classification asks the application.
#[rstest]
#[tokio::test(start_paused = true)]
async fn an_activated_dialog_is_classified_once(timing: Timing) {
    let script = Script {
        classes: HashMap::from([("files/alert", WindowClass::Dialog)]),
        ..Script::new([at(0, activated("files/alert")), at(5_000, activated("files/1"))], [
            Page::new("files/alert", "Disk full"),
            Page::new("files/1", "Documents"),
        ])
    };
    let mut capture = capture(script, timing);
    let dialogs = drain(&mut capture)
        .await
        .into_iter()
        .filter(|record| matches!(record, Record::Dialog(_)))
        .count();
    assert_eq!(dialogs, 1);
    assert_eq!(capture.backend().classifications.lock()["files/alert"], 1);
}

/// Capture classifies only the windows of the applications that capture may read, because each
/// classification asks the application, which may hang.
#[rstest]
#[tokio::test(start_paused = true)]
async fn an_activation_classifies_only_the_windows_of_allowed_applications(
    without_secrets: Config,
) {
    let script = Script::new(
        [
            at(0, activated("editor/1")),
            at(2_000, activated("secrets/1")),
            at(4_000, activated("editor/1")),
        ],
        [Page::new("editor/1", "fn main() {}"), Page::new("secrets/1", "hunter2")],
    );
    let mut capture = Capture::new(script, without_secrets);
    drain(&mut capture).await;
    let classifications = capture.backend().classifications.lock();
    assert_eq!(classifications.get("editor/1"), Some(&2));
    assert_eq!(classifications.get("secrets/1"), None);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_window_keeps_its_id_when_the_user_returns(timing: Timing) {
    let script = Script::new(
        [
            at(0, activated("editor/1")),
            at(2_000, activated("browser/1")),
            at(4_000, activated("editor/1")),
            at(6_000, deactivated("editor/1")),
        ],
        [Page::new("editor/1", "fn main() {}"), Page::new("browser/1", "Build failed")],
    );
    let mut capture = capture(script, timing);
    let mut windows = Vec::new();
    for record in drain(&mut capture).await {
        if let Record::SegmentStarted(segment) = record {
            windows.push((segment.id.0, segment.window.0));
        }
    }
    assert_eq!(windows, [(1, 1), (2, 2), (3, 1)]);
}

/// Replaces every `hunter2`, the way a real redactor replaces a credential.
struct Stars;

impl Redact for Stars {
    fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if text.contains("hunter2") {
            Cow::Owned(text.replace("hunter2", "****"))
        } else {
            Cow::Borrowed(text)
        }
    }
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn credentials_leave_neither_in_content_nor_in_titles(timing: Timing) {
    let script = Script::new(
        [
            at(0, activated("editor/1")),
            at(2_000, Event::TitleChanged {
                window: "editor/1",
                title: "password hunter2.txt".into(),
            }),
            at(3_000, Event::Opened {
                window: "editor/alert",
            }),
            at(4_000, Event::Notified {
                notice: Notice {
                    app: "mail".into(),
                    pid: None,
                    app_id: String::new(),
                    title: "Your password is hunter2".into(),
                    body: "Code hunter2 expires soon".into(),
                },
            }),
            at(6_000, deactivated("editor/1")),
        ],
        [
            Page::new("editor/1", "export TOKEN=hunter2"),
            Page::new("editor/alert", "TOKEN=hunter2 was rejected"),
        ],
    );
    let script = Script {
        classes: HashMap::from([("editor/alert", WindowClass::Dialog)]),
        ..script
    };
    let config = Config {
        timing,
        ..Config::default()
    };
    let mut capture = Capture::new(script, config).with_redactor(Stars);
    let mut seen = Vec::new();
    for record in drain(&mut capture).await {
        match record {
            Record::SegmentStarted(segment) => seen.push(segment.title),
            Record::Snapshot(snapshot) => {
                if let SnapshotContent::Changed(content) = &snapshot.content {
                    seen.push(rendered(content, usize::MAX));
                }
            }
            Record::SegmentEnded { .. } => {}
            Record::Dialog(dialog) => {
                seen.push(rendered(&dialog.content, usize::MAX));
            }
            Record::Background(background) => {
                seen.push(rendered(&background.content, usize::MAX));
            }
            Record::Notification(notification) => {
                seen.push(notification.title);
                seen.push(rendered(&notification.content, usize::MAX));
            }
        }
    }
    assert_eq!(seen, [
        "title of editor/1",
        "export TOKEN=****",
        "password ****.txt",
        "export TOKEN=****",
        "TOKEN=**** was rejected",
        "Your password is ****",
        "Code **** expires soon",
    ]);
}

/// A message with a one-time code reaches the windows of messaging apps and mail too, so
/// every record and every title loses the code.
#[rstest]
#[tokio::test(start_paused = true)]
async fn one_time_codes_leave_every_record(timing: Timing) {
    let script = Script {
        apps: vec!["messages", "mail"],
        active: Some("messages/1"),
        classes: HashMap::from([("messages/alert", WindowClass::Dialog)]),
        ..Script::new(
            [
                at(2_000, Event::TitleChanged {
                    window: "messages/1",
                    title: "Your code is 482913".into(),
                }),
                at(3_000, Event::Opened {
                    window: "messages/alert",
                }),
                at(6_000, deactivated("messages/1")),
            ],
            [
                Page::new("messages/1", "Your verification code is 482913"),
                Page::new("messages/alert", "Login code: 771203"),
                Page::new("mail/1", "Use 556677 to verify your account"),
            ],
        )
    };
    let mut capture = capture(script, timing);
    let mut seen = Vec::new();
    for record in drain(&mut capture).await {
        match record {
            Record::SegmentStarted(segment) => seen.push(segment.title),
            Record::Snapshot(snapshot) => {
                if let SnapshotContent::Changed(content) = &snapshot.content {
                    seen.push(rendered(content, usize::MAX));
                }
            }
            Record::Dialog(dialog) => seen.push(rendered(&dialog.content, usize::MAX)),
            Record::Background(background) => {
                seen.push(rendered(&background.content, usize::MAX));
            }
            Record::SegmentEnded { .. } | Record::Notification(_) => {}
        }
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen, [
        "Login code: [one-time code]",
        "Use [one-time code] to verify your account",
        "Your code is [one-time code]",
        "Your verification code is [one-time code]",
        "title of messages/1",
    ]);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_terminal_window_leaves_no_records(timing: Timing) {
    let script = Script::new(
        [
            // The application is unknown, and the read finds that the window is a terminal.
            at(0, activated("newterm/console")),
            at(5_000, Event::TitleChanged {
                window: "newterm/console",
                title: "vim secrets.txt".into(),
            }),
            at(9_000, deactivated("newterm/console")),
        ],
        [Page::new("newterm/console", "tab title")],
    );
    let script = Script {
        kinds: HashMap::from([("newterm/console", WindowKind::Terminal)]),
        ..script
    };
    let mut capture = capture(script, timing);
    assert_eq!(capture.next().await, None);
}

/// A window that the backend reads as private isn't recorded when capture excludes private
/// windows, whether capture reads it in the background, as a dialog, or as the active window.
#[rstest]
#[case::excluded(PrivateWindows::Exclude, &[])]
#[case::captured(PrivateWindows::Capture, &["background", "dialog", "segment"])]
#[tokio::test(start_paused = true)]
async fn private_windows_are_captured_as_configured(
    timing: Timing,
    #[case] private_windows: PrivateWindows,
    #[case] expected: &[&str],
) {
    let script = Script {
        apps: vec!["editor", "browser", "private"],
        active: Some("editor/1"),
        kinds: HashMap::from([
            ("browser/private", WindowKind::Private),
            ("private/alert", WindowKind::Private),
        ]),
        classes: HashMap::from([("private/alert", WindowClass::Dialog)]),
        ..Script::new(
            [
                at(20_000, Event::Opened {
                    window: "private/alert",
                }),
                at(30_000, activated("browser/private")),
                at(40_000, deactivated("browser/private")),
            ],
            [
                Page::new("editor/1", "fn main() {}"),
                Page::new("browser/private", "Private page"),
                Page::new("private/alert", "Private prompt"),
            ],
        )
    };
    let mut capture = Capture::new(script, Config {
        timing,
        private_windows,
        ..Config::default()
    });
    let mut seen: Vec<&str> = Vec::new();
    for record in drain(&mut capture).await {
        match Seen::from(record) {
            Seen::Started { app, .. } if app == "browser" => seen.push("segment"),
            Seen::Background { text, .. } if text.contains("Private page") => {
                seen.push("background");
            }
            Seen::Dialog { text, .. } if text.contains("Private prompt") => seen.push("dialog"),
            _ => {}
        }
    }
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen, expected);
}

/// Focus moves inside an active window that isn't captured read the window again only once the
/// window has rested, and focus moves inside a captured window don't read the window again.
#[rstest]
#[case::private(WindowKind::Private, 2)]
#[case::other(WindowKind::Other, 1)]
#[tokio::test(start_paused = true)]
async fn focus_moves_read_a_window_that_is_not_captured_after_a_rest(
    timing: Timing,
    #[case] kind: WindowKind,
    #[case] reads: u32,
) {
    let focus = |ms| at(ms, Event::FocusMoved { app: "browser" });
    let script = Script {
        active_windows: HashMap::from([("browser", "browser/1")]),
        kinds: HashMap::from([("browser/1", kind)]),
        ..Script::new(
            [
                at(0, activated("browser/1")),
                focus(1_000),
                focus(2_000),
                focus(4_000),
                focus(8_000),
                // The window rested for the shortest rest of the active window.
                focus(15_000),
                at(20_000, deactivated("browser/1")),
            ],
            [Page::new("browser/1", "page")],
        )
    };
    let mut capture = capture(script, timing);
    drain(&mut capture).await;
    assert_eq!(capture.backend().reads.lock()["browser/1"], reads);
}

/// An editor whose terminal panel covers most of its window reads as a terminal.
///
/// When the user hides the panel during the window's rest, which is a focus move, capture
/// follows the window once the rest ends.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_window_that_becomes_capturable_while_it_rests_is_followed(timing: Timing) {
    let edit = |ms| at(ms, changed("code/1", Cause::Edited));
    let script = Script {
        active_windows: HashMap::from([("code", "code/1")]),
        terminal_reads: AtomicU32::new(1),
        ..Script::new(
            [
                at(0, activated("code/1")),
                at(1_000, Event::FocusMoved { app: "code" }),
                edit(12_000),
                edit(18_000),
                at(25_000, deactivated("code/1")),
            ],
            [Page::new("code/1", "fn main() {}")],
        )
    };
    let mut capture = capture(script, timing);
    let records = drain(&mut capture).await;
    let segments = records.iter().filter(|record| matches!(record, Record::SegmentStarted(_)));
    assert_eq!(segments.count(), 1);
    assert!(records.iter().any(|record| matches!(record, Record::Snapshot(_))));
}

/// A window whose content isn't captured and that rests without another activation isn't read
/// again when the rest ends.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_declined_window_without_a_missed_activation_is_not_read_again(timing: Timing) {
    let script = Script {
        active_windows: HashMap::from([("code", "code/1"), ("editor", "editor/1")]),
        terminal_reads: AtomicU32::new(1),
        ..Script::new(
            [
                at(0, activated("code/1")),
                at(1_000, Event::FocusMoved { app: "code" }),
                // The user moves on, which cancels the activation that capture skipped.
                at(2_000, activated("editor/1")),
                at(25_000, deactivated("editor/1")),
            ],
            [Page::new("code/1", "fn main() {}"), Page::new("editor/1", "notes")],
        )
    };
    let mut capture = capture(script, timing);
    drain(&mut capture).await;
    assert_eq!(capture.backend().reads.lock()["code/1"], 1);
}

/// An application that the backend leaves out, such as the alert that macOS keeps open after a
/// grant, doesn't leave any records.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_left_out_application_leaves_no_records(timing: Timing) {
    let script =
        Script::new([at(0, activated("alert/1")), at(9_000, deactivated("alert/1"))], [Page::new(
            "alert/1",
            "Accessibility Access",
        )]);
    let mut capture = capture(script, timing);
    assert_eq!(capture.next().await, None);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_window_without_text_gets_its_segment_after_the_retries(timing: Timing) {
    let start = Instant::now();
    let script = Script::new(
        [
            at(0, activated("canvas/1")),
            at(60_000, deactivated("canvas/1")),
            // Capture stops when the events run out.
            at(70_000, deactivated("other/1")),
        ],
        [Page::new("canvas/1", "")],
    );
    let mut capture = capture(script, timing);

    // The window settles after 400 ms, and each of the three retries waits 1500 ms.
    assert!(matches!(capture.next().await, Some(Record::SegmentStarted(_))));
    assert_eq!(start.elapsed(), Duration::from_millis(4_900));
    // The segment ends once the application still has no active window a moment later.
    assert!(matches!(capture.next().await, Some(Record::SegmentEnded { .. })));
    assert_eq!(start.elapsed(), Duration::from_millis(60_200));
}

/// A record reduced to what the scenario decides.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
    Started {
        segment: u64,
        window: u64,
        app: String,
        title: String,
        url: Option<String>,
    },
    Snapshot {
        segment: u64,
        trigger: Trigger,
        text: String,
    },
    Unchanged {
        segment: u64,
        trigger: Trigger,
    },
    Ended {
        segment: u64,
    },
    Dialog {
        window: u64,
        app: String,
        title: String,
        text: String,
    },
    Notification {
        app: String,
        title: String,
        body: String,
    },
    Background {
        window: u64,
        app: String,
        title: String,
        url: Option<String>,
        trigger: Trigger,
        text: String,
    },
}

impl From<Record> for Seen {
    fn from(record: Record) -> Self {
        match record {
            Record::SegmentStarted(segment) => Self::Started {
                segment: segment.id.0,
                window: segment.window.0,
                app: segment.app,
                title: segment.title,
                url: segment.url,
            },
            Record::Snapshot(snapshot) => match &snapshot.content {
                SnapshotContent::Changed(content) => Self::Snapshot {
                    segment: snapshot.segment.0,
                    trigger: snapshot.trigger,
                    text: rendered(content, usize::MAX),
                },
                SnapshotContent::Unchanged { .. } => Self::Unchanged {
                    segment: snapshot.segment.0,
                    trigger: snapshot.trigger,
                },
            },
            Record::SegmentEnded { segment, .. } => Self::Ended { segment: segment.0 },
            Record::Dialog(dialog) => Self::Dialog {
                window: dialog.window.0,
                app: dialog.app,
                title: dialog.title,
                text: rendered(&dialog.content, usize::MAX),
            },
            Record::Notification(notification) => Self::Notification {
                app: notification.app,
                title: notification.title,
                body: rendered(&notification.content, usize::MAX),
            },
            Record::Background(background) => Self::Background {
                window: background.window.0,
                app: background.app,
                title: background.title,
                url: background.url,
                trigger: background.trigger,
                text: rendered(&background.content, usize::MAX),
            },
        }
    }
}

#[rstest]
#[case::on(BackgroundReads::On)]
#[case::off(BackgroundReads::Off)]
#[tokio::test(start_paused = true)]
async fn a_sweep_reads_every_window_in_the_background_once(
    without_secrets: Config,
    #[case] reads: BackgroundReads,
) {
    let script = Script {
        apps: vec!["editor", "browser", "secrets", "newterm"],
        active: Some("editor/1"),
        // A background tab of the first browser window.
        hidden: HashMap::from([("browser/1", vec!["browser/tab"])]),
        kinds: HashMap::from([("newterm/console", WindowKind::Terminal)]),
        classes: HashMap::from([("browser/popup", WindowClass::Popup)]),
        ..Script::new(
            [
                // The user comes to a window that the sweep read.
                at(20_000, activated("browser/2")),
                at(30_000, deactivated("browser/2")),
            ],
            [
                Page::new("editor/1", "fn main() {}"),
                Page::new("browser/1", "Build failed").with_url("https://ci.example/run/1?token=x"),
                Page::new("browser/2", "Release notes"),
                Page::new("browser/tab", "Changelog"),
                Page::new("browser/popup", "Open in new tab"),
                Page::new("browser/blank", ""),
                Page::new("secrets/1", "hunter2"),
                Page::new("newterm/console", "$ ls"),
            ],
        )
    };
    let mut capture = Capture::new(script, Config {
        background: reads,
        ..without_secrets
    });
    let mut swept = Vec::new();
    let mut segment_windows = Vec::new();
    for record in drain(&mut capture).await {
        match Seen::from(record) {
            background @ Seen::Background { .. } => swept.push(background),
            Seen::Started { window, title, .. } => segment_windows.push((title, window)),
            _ => {}
        }
    }
    // The backend lists windows in no particular order.
    swept.sort_by_key(|seen| match seen {
        Seen::Background { title, .. } => title.clone(),
        _ => String::new(),
    });

    let background = |window, title: &str, url: Option<&str>, text: &str| Seen::Background {
        window,
        app: "browser".to_owned(),
        title: title.to_owned(),
        url: url.map(str::to_owned),
        trigger: Trigger::Swept,
        text: text.to_owned(),
    };
    if reads == BackgroundReads::Off {
        assert_eq!(swept, []);
        return;
    }
    // The ids that capture gave the windows, which depend on the order of the sweep.
    let ids: HashMap<String, u64> = swept
        .iter()
        .filter_map(|seen| match seen {
            Seen::Background { window, title, .. } => Some((title.clone(), *window)),
            _ => None,
        })
        .collect();
    let id = |title: &str| ids.get(title).copied().unwrap_or_default();
    // The active window, a popup, a blank window, a terminal, and the windows of an excluded
    // application are left out. The background tab follows its window.
    assert_eq!(swept, [
        background(
            id("title of browser/1"),
            "title of browser/1",
            Some("https://ci.example/run/1"),
            "Build failed"
        ),
        background(id("title of browser/2"), "title of browser/2", None, "Release notes"),
        background(id("title of browser/tab"), "title of browser/tab", None, "Changelog"),
    ]);
    // A swept window keeps its id when it becomes active.
    assert_eq!(segment_windows, [
        ("title of editor/1".to_owned(), 1),
        ("title of browser/2".to_owned(), id("title of browser/2")),
    ]);
}

fn notice(app: &str, title: &str) -> Event<Script> {
    Event::Notified {
        notice: Notice {
            app: app.to_owned(),
            pid: None,
            app_id: String::new(),
            title: title.to_owned(),
            body: "body".to_owned(),
        },
    }
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn notification_codes_are_scrubbed(timing: Timing) {
    let script = Script::new(
        [
            at(0, activated("editor/1")),
            at(2_000, Event::Notified {
                notice: Notice {
                    app: "mail".into(),
                    pid: None,
                    app_id: String::new(),
                    title: "Code 482913 for https://a.example/s?token=x".into(),
                    body: "Your login code is 99812".into(),
                },
            }),
            at(6_000, deactivated("editor/1")),
        ],
        [Page::new("editor/1", "fn main() {}")],
    );
    let mut capture = capture(script, timing);
    let mut seen = Vec::new();
    for record in drain(&mut capture).await {
        if let Record::Notification(notification) = record {
            seen.push(notification.title);
            seen.push(rendered(&notification.content, usize::MAX));
        }
    }
    assert_eq!(seen, [
        "Code [one-time code] for https://a.example/s",
        "Your login code is [one-time code]"
    ]);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_notification_is_recorded_when_it_arrives(without_secrets: Config) {
    let start = Instant::now();
    let script = Script::new(
        [
            at(0, activated("editor/1")),
            at(2_000, notice("mail", "New message")),
            // The applications that capture leaves out send nothing.
            at(2_000, notice("secrets", "Your code is 123456")),
            at(2_000, notice("kitty", "Command finished")),
            at(6_000, deactivated("editor/1")),
        ],
        [Page::new("editor/1", "fn main() {}")],
    );
    let mut capture = Capture::new(script, without_secrets);
    let mut notified_at = Vec::new();
    let mut seen = Vec::new();
    while let Some(record) = capture.next().await {
        if let Record::Notification(_) = record {
            notified_at.push(start.elapsed());
        }
        if !matches!(record, Record::Snapshot(_)) {
            seen.push(Seen::from(record));
        }
    }
    assert_eq!(notified_at, [2_000 * MS]);
    assert_eq!(seen, [
        Seen::Started {
            segment: 1,
            window: 1,
            app: "editor".to_owned(),
            title: "title of editor/1".to_owned(),
            url: None,
        },
        // The notification stands apart from the segment of the active window.
        Seen::Notification {
            app: "mail".to_owned(),
            title: "New message".to_owned(),
            body: "body".to_owned(),
        },
        Seen::Ended { segment: 1 },
    ]);
}

/// The sender of a notification chooses the name that the notification declares, so capture
/// also checks the application that runs as the process behind the notification.
#[rstest]
#[case::an_excluded_process(Scope::All, 1, 0)]
#[case::an_allowed_process(Scope::All, 2, 1)]
#[case::a_process_without_an_application(Scope::All, 99, 1)]
#[case::a_process_outside_the_scope(Scope::Only(vec!["mail".into()]), 3, 0)]
#[case::a_process_within_the_scope(Scope::Only(vec!["mail".into()]), 2, 1)]
#[tokio::test(start_paused = true)]
async fn notifications_are_filtered_by_their_process(
    without_secrets: Config,
    #[case] scope: Scope,
    #[case] pid: u32,
    #[case] expected: usize,
) {
    let script = Script {
        processes: HashMap::from([(1, "secrets"), (2, "mail"), (3, "kate")]),
        ..Script::new(
            [
                at(0, activated("mail/1")),
                at(2_000, Event::Notified {
                    notice: Notice {
                        app: "mail".to_owned(),
                        pid: Some(pid),
                        app_id: String::new(),
                        title: "New message".to_owned(),
                        body: "body".to_owned(),
                    },
                }),
                at(6_000, deactivated("mail/1")),
            ],
            [Page::new("mail/1", "Inbox")],
        )
    };
    let config = Config {
        filter: AppFilter {
            scope,
            ..without_secrets.filter.clone()
        },
        ..without_secrets
    };
    let mut capture = Capture::new(script, config);

    let records = drain(&mut capture).await;

    let notified = records.iter().filter(|record| matches!(record, Record::Notification(_)));
    assert_eq!(notified.count(), expected);
}

/// A notification can name its application by an app id, such as a Flatpak application's
/// notification that the portal passes on.
///
/// The filter applies to the names that the id stands for.
#[rstest]
#[case::excluded_by_its_entry_name(Scope::All, &["flare"], "de.schmidhuberj.Flare", "", None)]
#[case::excluded_by_a_translation(
    Scope::All,
    &["Flare (Signal)"],
    "de.schmidhuberj.Flare",
    "",
    None
)]
#[case::allowed(
    Scope::All,
    &["mail"],
    "de.schmidhuberj.Flare",
    "",
    Some("de.schmidhuberj.Flare")
)]
#[case::excluded_by_its_hint(Scope::All, &["Flare"], "Flare Messenger", "de.schmidhuberj.Flare", None)]
#[case::excluded_by_its_running_app(Scope::All, &["signal"], "org.signal.Signal", "", None)]
#[case::run_by_a_terminal(Scope::All, &[], "org.example.Term", "", None)]
#[case::listed_by_its_entry_name(
    Scope::Only(vec!["Flare".into()]),
    &[],
    "de.schmidhuberj.Flare",
    "",
    Some("de.schmidhuberj.Flare")
)]
#[case::an_unknown_id(Scope::All, &["Flare"], "org.other.App", "", Some("org.other.App"))]
#[tokio::test(start_paused = true)]
async fn notifications_are_filtered_by_the_names_of_their_app_ids(
    timing: Timing,
    #[case] scope: Scope,
    #[case] excluded: &[&str],
    #[case] app: &str,
    #[case] app_id: &str,
    #[case] expected: Option<&str>,
) {
    let script = Script {
        apps: vec!["mail", "signal", "kitty"],
        app_ids: HashMap::from([
            ("de.schmidhuberj.Flare", AppIdentity {
                entry_names: vec!["Flare".to_owned(), "Flare (Signal)".to_owned()],
                apps: Vec::new(),
            }),
            ("org.signal.Signal", AppIdentity {
                entry_names: Vec::new(),
                apps: vec!["signal"],
            }),
            ("org.example.Term", AppIdentity {
                entry_names: Vec::new(),
                apps: vec!["kitty"],
            }),
        ]),
        ..Script::new(
            [
                at(0, activated("mail/1")),
                at(2_000, Event::Notified {
                    notice: Notice {
                        app: app.to_owned(),
                        pid: None,
                        app_id: app_id.to_owned(),
                        title: "New message".to_owned(),
                        body: "body".to_owned(),
                    },
                }),
                at(6_000, deactivated("mail/1")),
            ],
            [Page::new("mail/1", "Inbox")],
        )
    };
    let config = Config {
        timing,
        filter: AppFilter {
            scope,
            excluded: excluded.iter().map(|&name| name.to_owned()).collect(),
        },
        ..Config::default()
    };
    let mut capture = Capture::new(script, config);

    let records = drain(&mut capture).await;

    let kept: Vec<&str> = records
        .iter()
        .filter_map(|record| match record {
            Record::Notification(notification) => Some(notification.app.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(kept, Vec::from_iter(expected));
}

/// The name that a notification declares is cleaned like a title, and cut short.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_declared_application_name_is_cleaned(timing: Timing) {
    let script = Script::new(
        [
            at(0, activated("editor/1")),
            at(2_000, notice(&format!("deploy code 482913 {}", "x".repeat(100)), "Done")),
            at(6_000, deactivated("editor/1")),
        ],
        [Page::new("editor/1", "fn main() {}")],
    );
    let mut capture = capture(script, timing);
    let apps: Vec<String> = drain(&mut capture)
        .await
        .into_iter()
        .filter_map(|record| match record {
            Record::Notification(notification) => Some(notification.app),
            _ => None,
        })
        .collect();
    let expected = format!("deploy code [one-time code] {}", "x".repeat(36));
    assert_eq!(apps, [expected]);
}

/// A notification that doesn't name its application may come from an application that the
/// filter leaves out, so capture keeps it only when the filter allows every application.
#[rstest]
#[case::every_application(Scope::All, &[], 1)]
#[case::one_application_left_out(Scope::All, &["secrets"], 0)]
#[case::listed_applications(Scope::Only(vec!["mail".into()]), &[], 0)]
#[tokio::test(start_paused = true)]
async fn a_notification_without_an_application_needs_every_application(
    timing: Timing,
    #[case] scope: Scope,
    #[case] excluded: &[&str],
    #[case] expected: usize,
) {
    let script = Script::new(
        [
            at(0, activated("mail/1")),
            at(2_000, notice("", "Your code is 123456")),
            at(6_000, deactivated("mail/1")),
        ],
        [Page::new("mail/1", "Inbox")],
    );
    let config = Config {
        timing,
        filter: AppFilter {
            scope,
            excluded: excluded.iter().map(|&name| name.to_owned()).collect(),
        },
        ..Config::default()
    };
    let mut capture = Capture::new(script, config);

    let titles: Vec<String> = drain(&mut capture)
        .await
        .into_iter()
        .filter_map(|record| match record {
            Record::Notification(notification) => Some(notification.title),
            _ => None,
        })
        .collect();

    assert_eq!(titles.len(), expected);
    // The title of a kept notification is scrubbed like any other.
    assert!(titles.iter().all(|title| title == "Your code is [one-time code]"));
}

/// A notification that doesn't name its application takes the name of the application that
/// runs as its process, and the filter judges that application.
#[rstest]
#[case::an_allowed_process(Scope::All, &["secrets"], 2, Some("mail"))]
#[case::an_excluded_process(Scope::All, &["secrets"], 1, None)]
#[case::a_process_within_the_scope(Scope::Only(vec!["mail".into()]), &[], 2, Some("mail"))]
#[case::a_process_outside_the_scope(Scope::Only(vec!["mail".into()]), &[], 3, None)]
#[case::a_terminal_process(Scope::All, &[], 4, None)]
#[case::a_process_without_an_application(Scope::All, &["secrets"], 99, None)]
#[case::a_process_without_an_application_and_every_application(Scope::All, &[], 99, Some(""))]
#[tokio::test(start_paused = true)]
async fn a_notification_without_an_application_takes_its_process_application(
    timing: Timing,
    #[case] scope: Scope,
    #[case] excluded: &[&str],
    #[case] pid: u32,
    #[case] expected: Option<&str>,
) {
    let script = Script {
        processes: HashMap::from([(1, "secrets"), (2, "mail"), (3, "kate"), (4, "kitty")]),
        ..Script::new(
            [
                at(0, activated("mail/1")),
                at(2_000, Event::Notified {
                    notice: Notice {
                        app: String::new(),
                        pid: Some(pid),
                        app_id: String::new(),
                        title: "New message".to_owned(),
                        body: "body".to_owned(),
                    },
                }),
                at(6_000, deactivated("mail/1")),
            ],
            [Page::new("mail/1", "Inbox")],
        )
    };
    let config = Config {
        timing,
        filter: AppFilter {
            scope,
            excluded: excluded.iter().map(|&name| name.to_owned()).collect(),
        },
        ..Config::default()
    };
    let mut capture = Capture::new(script, config);

    let apps: Vec<String> = drain(&mut capture)
        .await
        .into_iter()
        .filter_map(|record| match record {
            Record::Notification(notification) => Some(notification.app),
            _ => None,
        })
        .collect();

    assert_eq!(apps, Vec::from_iter(expected));
}

/// The first lookup of the process fails, and the second one finds its application.
///
/// A failed lookup keeps the notification only when the filter starts from every application,
/// and capture asks again for the next notification.
#[rstest]
#[case::an_excluded_process(Scope::All, 1, &["first"])]
#[case::a_process_within_the_scope(Scope::Only(vec!["mail".into()]), 2, &["second"])]
#[tokio::test(start_paused = true)]
async fn a_failed_process_lookup_is_asked_again(
    without_secrets: Config,
    #[case] scope: Scope,
    #[case] pid: u32,
    #[case] expected: &[&str],
) {
    let notified = |title: &str| Event::Notified {
        notice: Notice {
            app: "mail".to_owned(),
            pid: Some(pid),
            app_id: String::new(),
            title: title.to_owned(),
            body: "body".to_owned(),
        },
    };
    let script = Script {
        processes: HashMap::from([(1, "secrets"), (2, "mail")]),
        failing_process_lookups: AtomicU32::new(1),
        ..Script::new(
            [
                at(0, activated("mail/1")),
                at(2_000, notified("first")),
                at(3_000, notified("second")),
                at(6_000, deactivated("mail/1")),
            ],
            [Page::new("mail/1", "Inbox")],
        )
    };
    let config = Config {
        filter: AppFilter {
            scope,
            ..without_secrets.filter.clone()
        },
        ..without_secrets
    };
    let mut capture = Capture::new(script, config);

    let titles: Vec<String> = drain(&mut capture)
        .await
        .into_iter()
        .filter_map(|record| match record {
            Record::Notification(notification) => Some(notification.title),
            _ => None,
        })
        .collect();
    assert_eq!(titles, expected);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_dialog_is_read_as_soon_as_it_appears(without_secrets: Config) {
    let start = Instant::now();
    let mut script = Script::new(
        [
            at(0, activated("editor/1")),
            // A toolkit can announce one window twice.
            at(2_000, Event::Opened {
                window: "editor/alert",
            }),
            at(2_010, Event::Opened {
                window: "editor/alert",
            }),
            // An ordinary window, an alert without text, and the alerts of applications
            // that capture leaves out are not read.
            at(3_000, Event::Opened { window: "editor/2" }),
            at(3_000, Event::Opened {
                window: "blank/alert",
            }),
            at(3_000, Event::Opened {
                window: "secrets/alert",
            }),
            at(3_000, Event::Opened {
                window: "kitty/alert",
            }),
            // A program that only shows a dialog joins with the dialog already open. The
            // dialog can also be announced on its own, before or after the program joins.
            at(3_900, Event::Opened {
                window: "zenity/alert",
            }),
            at(4_000, Event::Joined { app: "zenity" }),
            at(4_050, Event::Opened {
                window: "zenity/alert",
            }),
            at(6_000, deactivated("editor/1")),
        ],
        [
            Page::new("editor/1", "fn main() {}"),
            Page::new("editor/2", "fn other() {}"),
            Page::new("editor/alert", "Disk full"),
            Page::new("blank/alert", ""),
            Page::new("secrets/alert", "Token expired"),
            Page::new("kitty/alert", "Process exited"),
            Page::new("zenity/alert", "Replace the file?"),
        ],
    );
    // The alert closes long before it could have settled as an active window.
    script.closes.insert("editor/alert", 2_300);
    for window in ["editor/alert", "blank/alert", "secrets/alert", "kitty/alert", "zenity/alert"] {
        script.classes.insert(window, WindowClass::Dialog);
    }
    let mut capture = Capture::new(script, without_secrets);

    let mut seen = Vec::new();
    let mut read_at = Vec::new();
    while let Some(record) = capture.next().await {
        if matches!(record, Record::Dialog(_)) {
            read_at.push(start.elapsed());
        }
        seen.push(Seen::from(record));
    }
    assert_eq!(read_at, [2_100 * MS, 4_000 * MS]);
    assert_eq!(seen, [
        Seen::Started {
            segment: 1,
            window: 1,
            app: "editor".to_owned(),
            title: "title of editor/1".to_owned(),
            url: None,
        },
        Seen::Snapshot {
            segment: 1,
            trigger: Trigger::Activated,
            text: "fn main() {}".to_owned(),
        },
        // The dialog stands apart from the segment of the active window, which goes on.
        Seen::Dialog {
            window: 2,
            app: "editor".to_owned(),
            title: "title of editor/alert".to_owned(),
            text: "Disk full".to_owned(),
        },
        Seen::Dialog {
            window: 3,
            app: "zenity".to_owned(),
            title: "title of zenity/alert".to_owned(),
            text: "Replace the file?".to_owned(),
        },
        Seen::Ended { segment: 1 },
    ]);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn dialogs_and_notification_windows_lose_their_one_time_codes(timing: Timing) {
    let opened = |window| Event::Opened { window };
    let script = Script::new(
        [
            at(0, activated("editor/1")),
            // A platform that reports notifications only as windows shows one.
            at(2_000, opened("mail/notification")),
            at(2_000, opened("mail/alert")),
            at(6_000, deactivated("editor/1")),
        ],
        [
            Page::new("editor/1", "fn main() {}"),
            Page::new("mail/notification", "Your login code is 482913"),
            Page::new("mail/alert", "Your login code is 482913"),
        ],
    );
    let script = Script {
        classes: HashMap::from([
            ("mail/notification", WindowClass::Notification),
            ("mail/alert", WindowClass::Dialog),
        ]),
        ..script
    };
    let mut capture = capture(script, timing);
    let mut texts = Vec::new();
    for record in drain(&mut capture).await {
        match record {
            Record::Dialog(dialog) => texts.push(rendered(&dialog.content, usize::MAX)),
            Record::Notification(notification) => {
                assert!(!notification.title.contains("482913"));
                texts.push(rendered(&notification.content, usize::MAX));
            }
            _ => {}
        }
    }
    texts.sort();
    assert_eq!(texts, ["Your login code is [one-time code]"; 2]);
}

/// A notification window that opens and is activated later is handed out only once.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_notification_window_that_is_activated_after_it_opens_is_handed_out_once(timing: Timing) {
    let script = Script::new(
        [
            at(0, activated("editor/1")),
            at(2_000, Event::Opened {
                window: "mail/notification",
            }),
            at(5_000, activated("mail/notification")),
            at(9_000, activated("editor/1")),
        ],
        [
            Page::new("editor/1", "fn main() {}"),
            Page::new("mail/notification", "New message from Ada"),
        ],
    );
    let script = Script {
        classes: HashMap::from([("mail/notification", WindowClass::Notification)]),
        ..script
    };
    let mut capture = capture(script, timing);
    let records = drain(&mut capture).await;
    assert!(!records.iter().any(|record| matches!(record, Record::Dialog(_))));
    let notifications =
        records.iter().filter(|record| matches!(record, Record::Notification(_))).count();
    assert_eq!(notifications, 1);
}

/// A notification window that the sweep, a change, or an activation reaches is handed out
/// only as a notification.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_notification_window_is_handed_out_as_a_notification(timing: Timing) {
    let script = Script {
        apps: vec!["editor", "mail"],
        active: Some("editor/1"),
        classes: HashMap::from([("mail/notification", WindowClass::Notification)]),
        ..Script::new(
            [
                at(2_000, changed("mail/notification", Cause::Edited)),
                at(20_000, activated("mail/notification")),
                at(30_000, deactivated("mail/notification")),
            ],
            [
                Page::new("editor/1", "fn main() {}"),
                Page::new("mail/notification", "Your login code is 482913"),
            ],
        )
    };
    let mut capture = capture(script, timing);
    let records = drain(&mut capture).await;
    assert!(!records.iter().any(|record| matches!(record, Record::Dialog(_))));
    let notifications: Vec<String> = records
        .iter()
        .filter_map(|record| match record {
            Record::Notification(notification) => Some(rendered(&notification.content, usize::MAX)),
            _ => None,
        })
        .collect();
    assert_eq!(notifications, ["Your login code is [one-time code]"]);
}

/// The applications of the segments among `records`, in order.
fn segment_apps(records: &[Record]) -> Vec<&str> {
    records
        .iter()
        .filter_map(|record| match record {
            Record::SegmentStarted(segment) => Some(segment.app.as_str()),
            _ => None,
        })
        .collect()
}

/// A notification window whose class is unknown at its activation is classified again later.
///
/// Capture doesn't follow the window meanwhile, because capture hands out a notification window
/// only as a notification.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_window_of_an_unknown_class_is_not_followed_as_a_notification(timing: Timing) {
    let script = Script {
        classes: HashMap::from([("mail/notification", WindowClass::Notification)]),
        failing_classifications: HashMap::from([("mail/notification", AtomicU32::new(1))]),
        active_windows: HashMap::from([("mail", "mail/notification")]),
        ..Script::new(
            [
                at(0, activated("editor/1")),
                at(2_000, activated("mail/notification")),
                at(9_000, activated("editor/1")),
                at(15_000, deactivated("editor/1")),
            ],
            [
                Page::new("editor/1", "fn main() {}"),
                Page::new("mail/notification", "Your login code is 482913"),
            ],
        )
    };
    let mut capture = capture(script, timing);
    let records = drain(&mut capture).await;
    assert_eq!(segment_apps(&records), ["editor", "editor"]);
    let notifications: Vec<String> = records
        .iter()
        .filter_map(|record| match record {
            Record::Notification(notification) => Some(rendered(&notification.content, usize::MAX)),
            _ => None,
        })
        .collect();
    assert_eq!(notifications, ["Your login code is [one-time code]"]);
}

/// Capture follows a window whose class was unknown at its activation once a later
/// classification answers, and gives up on a window that never answers.
#[rstest]
#[case::answers_later(1, &["editor", "files"])]
#[case::never_answers(u32::MAX, &["files"])]
#[tokio::test(start_paused = true)]
async fn a_window_of_an_unknown_class_is_followed_once_classified(
    timing: Timing,
    #[case] failures: u32,
    #[case] segments: &[&str],
) {
    let script = Script {
        failing_classifications: HashMap::from([("editor/1", AtomicU32::new(failures))]),
        active_windows: HashMap::from([("editor", "editor/1")]),
        ..Script::new(
            [
                at(0, activated("editor/1")),
                at(9_000, activated("files/1")),
                at(15_000, deactivated("files/1")),
            ],
            [Page::new("editor/1", "fn main() {}"), Page::new("files/1", "Documents")],
        )
    };
    let mut capture = capture(script, timing);
    let records = drain(&mut capture).await;
    assert_eq!(segment_apps(&records), segments);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_new_page_starts_its_segment_under_its_own_title(timing: Timing) {
    let page = Page::new("browser/1", "Build failed")
        .with_url("https://ci.example/run/1")
        .with_title("Run 1");
    let script = Script {
        // The page changes without a title event.
        later_pages: vec![at(
            10_000,
            Page::new("browser/1", "Release notes")
                .with_url("https://ci.example/notes")
                .with_title("Notes"),
        )],
        ..Script::new(
            [
                at(0, activated("browser/1")),
                at(10_000, Event::ContentChanged {
                    app: "browser",
                    window: Some("browser/1"),
                    cause: Cause::Loaded,
                }),
                at(30_000, deactivated("browser/1")),
            ],
            [page],
        )
    };
    let mut capture = capture(script, timing);
    let segments: Vec<_> = drain(&mut capture)
        .await
        .into_iter()
        .filter_map(|record| match record {
            Record::SegmentStarted(segment) => Some((segment.title, segment.url)),
            _ => None,
        })
        .collect();
    assert_eq!(segments, [
        ("Run 1".to_owned(), Some("https://ci.example/run/1".to_owned())),
        ("Notes".to_owned(), Some("https://ci.example/notes".to_owned())),
    ]);
}

/// An application whose handle comes to stand for another application, such as a process id
/// that the system reuses, is judged again by its new name.
#[rstest]
#[tokio::test(start_paused = true)]
async fn an_application_that_joins_is_named_again(without_secrets: Config) {
    let script = Script {
        later_names: vec![at(5_000, AppName {
            app: "42",
            name: "secrets",
        })],
        ..Script::new(
            [
                at(0, activated("42/1")),
                at(2_000, activated("other/1")),
                at(5_000, Event::Joined { app: "42" }),
                at(6_000, activated("42/1")),
                at(10_000, deactivated("42/1")),
            ],
            [Page::new("42/1", "fn main() {}"), Page::new("other/1", "inbox")],
        )
    };
    let mut capture = Capture::new(script, without_secrets);
    let apps: Vec<_> = drain(&mut capture)
        .await
        .into_iter()
        .filter_map(|record| match record {
            Record::SegmentStarted(segment) => Some(segment.app),
            _ => None,
        })
        .collect();
    assert_eq!(apps, ["42", "other"]);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_dialog_that_becomes_active_keeps_its_window_id(timing: Timing) {
    let script = Script::new(
        [
            at(0, Event::Opened {
                window: "editor/alert",
            }),
            at(50, activated("editor/alert")),
            at(6_000, deactivated("editor/alert")),
        ],
        [Page::new("editor/alert", "Save changes?")],
    );
    let script = Script {
        classes: HashMap::from([("editor/alert", WindowClass::Dialog)]),
        ..script
    };
    let mut capture = capture(script, timing);
    let mut windows = Vec::new();
    for record in drain(&mut capture).await {
        match record {
            Record::Dialog(dialog) => windows.push(("dialog", dialog.window.0)),
            Record::SegmentStarted(segment) => windows.push(("segment", segment.window.0)),
            Record::Snapshot(_)
            | Record::SegmentEnded { .. }
            | Record::Notification(_)
            | Record::Background(_) => {}
        }
    }
    assert_eq!(windows, [("dialog", 1), ("segment", 1)]);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_dialog_announced_only_by_its_activation_is_read_once(timing: Timing) {
    let start = Instant::now();
    let mut script = Script::new(
        [
            at(0, activated("editor/alert")),
            at(1_000, activated("editor/1")),
            // The user comes back to the dialog.
            at(3_000, activated("editor/alert")),
            at(5_000, activated("editor/1")),
            at(8_000, deactivated("editor/1")),
        ],
        [Page::new("editor/1", "fn main() {}"), Page::new("editor/alert", "Save changes?")],
    );
    script.closes.insert("editor/alert", 6_000);
    script.classes.insert("editor/alert", WindowClass::Dialog);

    let mut capture = capture(script, timing);
    let mut dialogs = Vec::new();
    while let Some(record) = capture.next().await {
        if let Record::Dialog(dialog) = record {
            dialogs.push((start.elapsed(), dialog.window.0));
        }
    }
    assert_eq!(dialogs, [(100 * MS, 1)]);
}

/// A dialog whose read fails is read again after the dialog settles, three times at most, and
/// a dialog that never answers doesn't leave a record.
#[rstest]
#[case::once(1, 2, &[200 * MS])]
#[case::always(u32::MAX, 4, &[])]
#[tokio::test(start_paused = true)]
async fn a_dialog_whose_read_fails_is_tried_again(
    timing: Timing,
    #[case] failing_reads: u32,
    #[case] reads: u32,
    #[case] expected: &[Duration],
) {
    let start = Instant::now();
    let script = Script {
        failing_reads: AtomicU32::new(failing_reads),
        classes: HashMap::from([("files/alert", WindowClass::Dialog)]),
        ..Script::new(
            [
                at(0, Event::Opened {
                    window: "files/alert",
                }),
                at(5_000, deactivated("files/alert")),
            ],
            [Page::new("files/alert", "Disk full")],
        )
    };
    let mut capture = capture(script, timing);
    let mut dialogs = Vec::new();
    while let Some(record) = capture.next().await {
        if let Record::Dialog(dialog) = record {
            assert_eq!(rendered(&dialog.content, 100), "Disk full");
            dialogs.push(start.elapsed());
        }
    }
    assert_eq!(dialogs, expected);
    assert_eq!(capture.backend().reads.lock()["files/alert"], reads);
}

#[rstest]
fn waiting_for_a_record_can_move_between_threads() {
    fn assert_send(_: &impl Send) {}

    let script = Script::new([], []);
    let mut capture = Capture::new(script, Config::default());
    assert_send(&capture.next());
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn events_become_segments_and_snapshots(without_secrets: Config) {
    let content = |cause| Event::ContentChanged {
        app: "browser",
        window: Some("browser/1"),
        cause,
    };
    let script = Script::new(
        [
            at(0, activated("editor/1")),
            // An activation from an inner object belongs to the window around it.
            at(600, activated("editor/1#toolbar")),
            at(1_000, activated("secrets/1")),
            at(1_500, activated("terminal/1")),
            at(2_000, activated("browser/1")),
            // A title that is replaced before the window settles never leaves capture.
            at(2_100, Event::TitleChanged {
                window: "browser/1",
                title: "ci?token=hunter2".into(),
            }),
            at(2_200, Event::TitleChanged {
                window: "browser/1",
                title: "first page".into(),
            }),
            // A burst of edits yields one snapshot, and its text matches the first one.
            at(20_000, content(Cause::Edited)),
            at(20_100, content(Cause::Edited)),
            at(30_000, Event::TitleChanged {
                window: "browser/1",
                title: "second page".into(),
            }),
            at(40_000, deactivated("browser/1")),
        ],
        [
            Page::new("editor/1", "fn main() {}"),
            Page::new("browser/1", "Build failed")
                .with_url("https://ci.example/run/1?token=hunter2"),
        ],
    );
    let config = Config {
        limits: Limits {
            max_nodes: 100,
            budget: Duration::from_secs(1),
            extent: Extent::Document,
            ..Limits::default()
        },
        background: BackgroundReads::On,
        ..without_secrets
    };
    let mut capture = Capture::new(script, config);
    let seen: Vec<Seen> = drain(&mut capture).await.into_iter().map(Seen::from).collect();

    let started = |segment, window, app: &str, title: &str, url: Option<&str>| Seen::Started {
        segment,
        window,
        app: app.to_owned(),
        title: title.to_owned(),
        url: url.map(str::to_owned),
    };
    let snapshot = |segment, trigger, text: &str| Seen::Snapshot {
        segment,
        trigger,
        text: text.to_owned(),
    };
    let url = Some("https://ci.example/run/1");
    assert_eq!(seen, [
        started(1, 1, "editor", "title of editor/1", None),
        snapshot(1, Trigger::Activated, "fn main() {}"),
        Seen::Ended { segment: 1 },
        started(2, 2, "browser", "first page", url),
        snapshot(2, Trigger::Activated, "Build failed"),
        Seen::Unchanged {
            segment: 2,
            trigger: Trigger::Edited,
        },
        Seen::Ended { segment: 2 },
        // A new title starts a new segment of the same window.
        started(3, 2, "browser", "second page", url),
        snapshot(3, Trigger::TitleChanged, "Build failed"),
        Seen::Ended { segment: 3 },
    ]);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn event_counts_name_their_applications(timing: Timing) {
    let script = Script {
        event_counts: vec![EventCount {
            app: "editor",
            event: "focus",
            detail: "gained".to_owned(),
            count: 3,
        }],
        ..Script::new([], [])
    };
    let mut capture = capture(script, timing);
    assert_eq!(capture.take_event_counts().await, [EventCount {
        app: "editor".to_owned(),
        event: "focus",
        detail: "gained".to_owned(),
        count: 3,
    }]);
}

/// A dialog that opens while its application is too busy to tell the dialog's class is
/// classified again once it settles.
#[rstest]
#[case::once(1, &[200 * MS])]
#[case::always(u32::MAX, &[])]
#[tokio::test(start_paused = true)]
async fn a_dialog_whose_class_is_unknown_is_classified_again(
    timing: Timing,
    #[case] failures: u32,
    #[case] expected: &[Duration],
) {
    let start = Instant::now();
    let script = Script {
        failing_classifications: HashMap::from([("files/alert", AtomicU32::new(failures))]),
        classes: HashMap::from([("files/alert", WindowClass::Dialog)]),
        ..Script::new(
            [
                at(0, Event::Opened {
                    window: "files/alert",
                }),
                at(5_000, deactivated("files/alert")),
            ],
            [Page::new("files/alert", "Disk full")],
        )
    };
    let mut capture = capture(script, timing);
    let mut dialogs = Vec::new();
    while let Some(record) = capture.next().await {
        if let Record::Dialog(_) = record {
            dialogs.push(start.elapsed());
        }
    }
    assert_eq!(dialogs, expected);
}

fn screen(screen: Screen) -> Event<Script> {
    Event::ScreenChanged { screen }
}

/// What a capture handed out, and when.
#[derive(Debug)]
struct Handed {
    ms: u64,
    output: Output,
    /// How many reads the backend served by then.
    reads: u32,
}

/// Takes every output of `capture` until capture stops.
async fn drain_outputs(capture: &mut Capture<Script>) -> Vec<Handed> {
    let start = Instant::now();
    let mut handed = Vec::new();
    while let Some(output) = capture.next_output().await {
        let ms = u64::try_from(start.elapsed().as_millis()).expect("a test lasts a few minutes");
        let reads = capture.backend().reads.lock().values().sum();
        handed.push(Handed { ms, output, reads });
    }
    handed
}

/// A desktop whose screen locks at 1.2 s and unlocks at 20 s. An edit waits in the active
/// editor at the lock, and the notes in the background change while the screen is locked.
fn locked_desktop() -> Script {
    let edited = |app, window| Event::ContentChanged {
        app,
        window: Some(window),
        cause: Cause::Edited,
    };
    Script {
        apps: vec!["editor", "notes"],
        active: Some("editor/1"),
        later_pages: vec![at(5_000, Page::new("notes/1", "groceries and milk"))],
        ..Script::new(
            [
                at(1_000, edited("editor", "editor/1")),
                at(1_200, screen(Screen::Locked)),
                at(3_000, edited("notes", "notes/1")),
                at(4_000, activated("notes/1")),
                at(20_000, screen(Screen::Unlocked)),
                at(60_000, deactivated("editor/1")),
            ],
            [Page::new("editor/1", "draft"), Page::new("notes/1", "groceries")],
        )
    }
}

/// The lock screen can show what the user doesn't want stored, so capture ends the segment at
/// the lock, drops the edit that waits, and reads nothing until the unlock.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_locked_screen_ends_the_segment_and_reads_nothing(timing: Timing) {
    let mut capture = capture(locked_desktop(), timing);
    let handed = drain_outputs(&mut capture).await;

    let position = |screen| {
        handed
            .iter()
            .position(|handed| handed.output == Output::Screen(screen))
            .expect("capture hands out the lock and the unlock")
    };
    let locked = position(Screen::Locked);
    let unlocked = position(Screen::Unlocked);
    assert!(
        matches!(handed[locked - 1].output, Output::Record(Record::SegmentEnded { .. })),
        "the segment ends before the lock goes out: {handed:?}"
    );
    assert_eq!(handed[locked].ms, 1_200);
    assert_eq!(unlocked, locked + 1, "nothing goes out while the screen is locked: {handed:?}");
    assert_eq!(handed[locked].reads, handed[unlocked].reads, "nothing is read while locked");
    let edits = handed.iter().filter(|handed| {
        matches!(&handed.output, Output::Record(Record::Snapshot(snapshot))
            if snapshot.trigger == Trigger::Edited)
    });
    assert_eq!(edits.count(), 0, "the edit that waited at the lock is dropped: {handed:?}");
}

/// The windows can change while the screen is locked, so capture follows the active window and
/// reads every window again once the screen unlocks.
#[rstest]
#[tokio::test(start_paused = true)]
async fn an_unlock_follows_the_desktop_again(timing: Timing) {
    let mut capture = capture(locked_desktop(), timing);
    let handed = drain_outputs(&mut capture).await;

    let mut started = Vec::new();
    let mut swept = Vec::new();
    for handed in handed.iter().filter(|handed| handed.ms >= 20_000) {
        match &handed.output {
            Output::Record(Record::SegmentStarted(segment)) => started.push(segment.app.clone()),
            Output::Record(Record::Background(background)) => {
                assert_eq!(background.trigger, Trigger::Swept);
                swept.push(rendered(&background.content, usize::MAX));
            }
            _ => {}
        }
    }
    assert_eq!(started, ["editor"]);
    assert_eq!(swept, ["groceries and milk"]);
}

/// A backend reports the state of the screen when it starts, and capture hands out every
/// report, so that the helper can tell the daemon. A report of the state that holds already
/// doesn't change anything.
#[rstest]
#[tokio::test(start_paused = true)]
async fn every_report_of_the_screen_goes_out(timing: Timing) {
    let script = Script {
        active: Some("editor/1"),
        ..Script::new(
            [
                at(0, screen(Screen::Unlocked)),
                at(1_000, screen(Screen::Locked)),
                at(2_000, screen(Screen::Locked)),
                at(3_000, screen(Screen::Unlocked)),
                at(60_000, deactivated("editor/1")),
            ],
            [Page::new("editor/1", "draft")],
        )
    };
    let mut capture = capture(script, timing);
    let handed = drain_outputs(&mut capture).await;

    let screens: Vec<_> = handed
        .iter()
        .filter_map(|handed| match handed.output {
            Output::Screen(screen) => Some((handed.ms, screen)),
            Output::Record(_) => None,
        })
        .collect();
    assert_eq!(screens, [
        (0, Screen::Unlocked),
        (1_000, Screen::Locked),
        (2_000, Screen::Locked),
        (3_000, Screen::Unlocked),
    ]);
    let started = handed
        .iter()
        .filter(|handed| matches!(handed.output, Output::Record(Record::SegmentStarted(_))));
    assert_eq!(started.count(), 2, "the second lock starts nothing: {handed:?}");
}

/// Whether `node` or an element under it holds `needle` in its name or its text.
fn holds(node: &Node, needle: &str) -> bool {
    node.name.contains(needle)
        || node.text.as_deref().is_some_and(|text| text.contains(needle))
        || node.children.iter().any(|child| holds(child, needle))
}

/// The terminal below each page shows up as a line of its own, and capture doesn't keep any of
/// the terminal's text, regardless of what the reader handed over. The active window, a window
/// in the background, and a dialog all follow the same rule.
#[rstest]
#[tokio::test(start_paused = true)]
async fn capture_keeps_no_text_of_a_terminal(timing: Timing) {
    const SECRET: &str = "TERMPROBE-12345";
    let script = Script {
        apps: vec!["code", "chat"],
        active: Some("code/1"),
        classes: HashMap::from([("code/alert", WindowClass::Dialog)]),
        terminals: HashMap::from([("code/1", SECRET), ("chat/1", SECRET), ("code/alert", SECRET)]),
        ..Script::new(
            [
                at(5_000, Event::Opened {
                    window: "code/alert",
                }),
                at(30_000, deactivated("code/1")),
            ],
            [
                Page::new("code/1", "fn main() {}"),
                Page::new("chat/1", "hello"),
                Page::new("code/alert", "Terminate the running process?"),
            ],
        )
    };
    let mut capture = capture(script, timing);
    let mut texts = Vec::new();
    for record in drain(&mut capture).await {
        let (kind, content) = match record {
            Record::Snapshot(Snapshot {
                content: SnapshotContent::Changed(content),
                ..
            }) => ("snapshot", content),
            Record::Background(background) => ("background", background.content),
            Record::Dialog(dialog) => ("dialog", dialog.content),
            _ => continue,
        };
        let tree = content.tree.decode().expect("a captured tree decodes");
        assert!(!holds(&tree, SECRET), "the {kind} keeps the terminal's text");
        texts.push((kind, rendered(&content, 1000)));
    }
    // The sweep also reads the dialog as a window in the background.
    texts.sort_unstable();
    texts.dedup();
    assert_eq!(texts, [
        ("background", "Terminate the running process?\n[terminal]".to_owned()),
        ("background", "hello\n[terminal]".to_owned()),
        ("dialog", "Terminate the running process?\n[terminal]".to_owned()),
        ("snapshot", "fn main() {}\n[terminal]".to_owned()),
    ]);
}

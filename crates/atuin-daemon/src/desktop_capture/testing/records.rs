//! Records and trees that stand in for the helper's, for the tests.

use std::borrow::Borrow;
use std::time::{Duration, SystemTime};

use atuin_desktop_capture::{
    Background, Completeness, Content, ContentHash, Dialog, EncodedTree, Node, Notification,
    ReadStats, Record, Role, Segment, SegmentId, Snapshot, SnapshotContent, Trigger, WindowId,
};

use crate::desktop_capture::store::{DesktopStore, IndexItem, Kinds, RunState, SegmentFilter};

fn node(role: Role, children: Vec<Node>) -> Node {
    let mut node = Node::default();
    node.role = role;
    node.children = children;
    node
}

fn paragraph(line: String) -> Node {
    let mut paragraph = node(Role::Paragraph, Vec::new());
    paragraph.text = Some(line);
    paragraph
}

/// A window that shows one paragraph of `line`.
pub fn window(line: &str) -> Node {
    node(Role::Window, vec![paragraph(line.to_owned())])
}

/// A window named "Log" that shows a document with a paragraph of each line.
pub fn page(lines: impl IntoIterator<Item = String>) -> Node {
    let mut document = node(Role::Document, lines.into_iter().map(paragraph).collect());
    document.url = Some("https://example.com/log".to_owned());
    let mut window = node(Role::Window, vec![document]);
    window.name = "Log".to_owned();
    window
}

/// A window of text that zstd cannot shrink much, which differs for each `seed`.
pub fn noise(seed: &str) -> Node {
    let mut text = String::new();
    let mut hash = blake3::hash(seed.as_bytes());
    for _ in 0..1024 {
        text.push_str(&hash.to_hex());
        hash = blake3::hash(hash.as_bytes());
    }
    window(&text)
}

/// A page of `n` numbered lines.
pub fn lines(n: usize) -> Node {
    page((1..=n).map(|i| format!("line {i}")))
}

pub fn hash(root: &Node) -> ContentHash {
    ContentHash::of(root)
}

/// The content of a read that found `root`.
pub fn content(root: &Node) -> Content {
    Content {
        tree: EncodedTree::encode(root),
        read_time: Duration::from_millis(5),
        read: ReadStats {
            nodes: 10,
            completeness: Completeness::Complete,
            unknown_roles: 0,
        },
    }
}

pub fn started(segment: u64, app: &str, title: &str, at: SystemTime) -> Record {
    Record::SegmentStarted(Segment {
        id: SegmentId(segment),
        window: WindowId(segment),
        app: app.to_owned(),
        title: title.to_owned(),
        url: None,
        started: at,
    })
}

pub fn snapshot(segment: u64, at: SystemTime, root: impl Borrow<Node>) -> Record {
    Record::Snapshot(Snapshot {
        segment: SegmentId(segment),
        at,
        trigger: Trigger::Polled,
        content: SnapshotContent::Changed(content(root.borrow())),
    })
}

/// A snapshot that found `root` unchanged since the segment's previous snapshot.
pub fn unchanged(segment: u64, at: SystemTime, root: &Node) -> Record {
    Record::Snapshot(Snapshot {
        segment: SegmentId(segment),
        at,
        trigger: Trigger::Polled,
        content: SnapshotContent::Unchanged {
            hash: hash(root),
            read_time: Duration::from_millis(5),
            read: ReadStats {
                nodes: 10,
                completeness: Completeness::Complete,
                unknown_roles: 0,
            },
        },
    })
}

pub fn ended(segment: u64, at: SystemTime) -> Record {
    Record::SegmentEnded {
        segment: SegmentId(segment),
        at,
    }
}

pub fn dialog(app: &str, title: &str, at: SystemTime, root: impl Borrow<Node>) -> Record {
    Record::Dialog(Dialog {
        window: WindowId(99),
        app: app.to_owned(),
        title: title.to_owned(),
        at,
        content: content(root.borrow()),
    })
}

pub fn background(
    window: u64,
    app: &str,
    title: &str,
    at: SystemTime,
    root: impl Borrow<Node>,
) -> Record {
    background_at(window, app, title, None, at, root)
}

pub fn background_at(
    window: u64,
    app: &str,
    title: &str,
    url: Option<&str>,
    at: SystemTime,
    root: impl Borrow<Node>,
) -> Record {
    Record::Background(Background {
        window: WindowId(window),
        app: app.to_owned(),
        title: title.to_owned(),
        url: url.map(str::to_owned),
        at,
        trigger: Trigger::Swept,
        content: content(root.borrow()),
    })
}

/// A background read whose tree comes with a hash that does not match it, which the store
/// refuses to write.
pub fn forged_background(window: u64, title: &str, at: SystemTime) -> Record {
    let Record::Background(mut read) = background(window, "kate", title, at, lines(1)) else {
        unreachable!("a background read");
    };
    let mut tree = serde_json::to_value(&read.content.tree).expect("serialize a tree");
    let other = serde_json::to_value(EncodedTree::encode(&lines(2))).expect("serialize a tree");
    tree["hash"] = other["hash"].clone();
    read.content.tree = serde_json::from_value(tree).expect("deserialize a tree");
    assert!(read.content.tree.verify().is_err());
    Record::Background(read)
}

pub fn notification(title: &str, at: SystemTime) -> Record {
    notification_from("mail", title, at)
}

pub fn notification_from(app: &str, title: &str, at: SystemTime) -> Record {
    Record::Notification(Notification {
        app: app.to_owned(),
        title: title.to_owned(),
        at,
        content: Content::of_notice(format!("body of {title}")),
    })
}

pub async fn write_all(
    store: &DesktopStore,
    run: &mut RunState,
    records: impl IntoIterator<Item = Record>,
) {
    for record in records {
        let _ = store.write(run, record).await.expect("write");
    }
}

/// Stores `records` as one run of the helper, and returns the new contents for the index.
pub async fn written(
    store: &DesktopStore,
    records: impl IntoIterator<Item = Record>,
) -> Vec<IndexItem> {
    let mut run = RunState::default();
    let mut items = Vec::new();
    for record in records {
        items.extend(store.write(&mut run, record).await.expect("write").item);
    }
    items
}

pub fn everything() -> SegmentFilter {
    SegmentFilter {
        from: SystemTime::UNIX_EPOCH,
        to: SystemTime::now() + Duration::from_secs(3600),
        app: None,
        kinds: Kinds::All,
    }
}

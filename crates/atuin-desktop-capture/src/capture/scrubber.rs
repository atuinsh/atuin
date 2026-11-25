//! Takes URL parameters, one-time codes, and credentials out of what capture hands out.

use std::borrow::Cow;
use std::time::Duration;

use tokio::time::Instant;
use tracing::debug;

use super::record::{Content, ContentHash, EncodedTree, SnapshotContent};
use crate::backend::{Completeness, Node, ReadStats, Role};
use crate::encoding::{self, MAX_DEPTH, element_len};
use crate::scrub::{scrub_one_time_codes, scrub_text_urls, scrub_title, scrub_url};

/// Takes credentials out of captured text before the text leaves capture.
///
/// Capture applies the redactor to the name and the text of every element, to URLs, and to
/// window titles. The patterns come from the caller, because Atuin already maintains them and
/// Atuin's redactor fits this trait.
pub trait Redact: Send + Sync {
    /// Returns `text` with every credential replaced, borrowed when there was none.
    fn redact<'a>(&self, text: &'a str) -> Cow<'a, str>;
}

/// Cleans titles, trees, and notifications.
#[derive(Default)]
pub struct Scrubber {
    redactor: Option<Box<dyn Redact>>,
}

/// The most characters that the name of an application keeps.
pub const MAX_APP_NAME_CHARS: usize = 64;

/// The most bytes that the encoded tree of a read takes.
///
/// A record must fit in one frame of the link to the daemon, which holds 64 MiB at most. The
/// bound leaves room for the rest of the record.
const MAX_TREE_BYTES: usize = 32 * 1024 * 1024;

/// What a tree lost to fit within [`MAX_DEPTH`] or [`MAX_TREE_BYTES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fit {
    Whole,
    /// Elements that nest too deep are dropped.
    TooDeep,
    /// The elements after the point where the tree ran out of room are dropped.
    OutOfRoom,
}

/// A tree that a read found, within [`MAX_DEPTH`], with what the read cost.
pub struct Found {
    root: Node,
    read_time: Duration,
    read: ReadStats,
    /// The most bytes that the encoded tree may take once it is cleaned.
    max_bytes: usize,
}

impl Found {
    /// Takes the tree that a read found.
    ///
    /// A tree that nests deeper than [`MAX_DEPTH`] loses its deepest elements. A tree that encodes
    /// to more than [`MAX_TREE_BYTES`] once it is cleaned loses the elements at its end. The read
    /// of a tree that lost elements counts as truncated.
    pub fn new(root: Node, read_time: Duration, read: ReadStats) -> Self {
        Self::within(root, read_time, read, MAX_TREE_BYTES)
    }

    fn within(mut root: Node, read_time: Duration, mut read: ReadStats, max_bytes: usize) -> Self {
        let fit = cut_deep(&mut root, 0);
        if fit != Fit::Whole {
            debug!(max_depth = MAX_DEPTH, "the tree nests too deep, and is cut");
            count_as_truncated(&mut read);
        }
        Self {
            root,
            read_time,
            read,
            max_bytes,
        }
    }

    /// The hash of the tree as the read found it, before the scrubber cleans it.
    fn raw_hash(&self) -> RawHash {
        RawHash(ContentHash::of(&self.root))
    }
}

/// Counts a read that lost elements as truncated, unless the read was already incomplete.
fn count_as_truncated(read: &mut ReadStats) {
    if read.completeness == Completeness::Complete {
        read.completeness = Completeness::Truncated;
    }
}

/// The hash of a tree as a read found it, within [`MAX_DEPTH`], before the scrubber cleaned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawHash(ContentHash);

/// The hashes of the content that capture last handed out for a window.
///
/// The raw hash lets the next read of the window skip the scrub when the window shows the same
/// tree. That holds because one [`Scrubber`] cleans the same tree the same way for its whole
/// life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    raw: RawHash,
    hash: ContentHash,
    /// What the cleaned tree lost to fit within its bytes, which the same tree loses again.
    fit: Fit,
}

impl Seen {
    /// The hash of the content that capture handed out.
    pub const fn hash(&self) -> ContentHash {
        self.hash
    }
}

/// A tree without URL parameters and credentials, within its bytes, with what its read cost.
pub struct Cleaned {
    pub root: Node,
    pub read_time: Duration,
    pub read: ReadStats,
    /// At least as many bytes as the encoded tree takes.
    encoded_len: usize,
    fit: Fit,
}

impl Cleaned {
    /// Encodes the tree.
    pub fn into_content(self) -> Content {
        Content {
            tree: EncodedTree::encode_with_capacity(&self.root, self.encoded_len),
            read_time: self.read_time,
            read: self.read,
        }
    }
}

impl Scrubber {
    pub fn set_redactor(&mut self, redactor: impl Redact + 'static) {
        self.redactor = Some(Box::new(redactor));
    }

    /// Removes URL parameters, credentials, and one-time codes from a title.
    pub fn title(&self, title: String) -> String {
        let mut title = scrub_title(title);
        self.clean_text(&mut title);
        title
    }

    /// Removes URL parameters, credentials, and one-time codes from the name of an
    /// application, and cuts the name to [`MAX_APP_NAME_CHARS`].
    ///
    /// The sender of a notification chooses the name that the notification declares, so the
    /// name can carry any text.
    pub fn app_name(&self, name: String) -> String {
        let mut name = self.title(name);
        if let Some((end, _)) = name.char_indices().nth(MAX_APP_NAME_CHARS) {
            name.truncate(end);
        }
        name
    }

    /// Reduces `url` to its scheme, host, and path, and takes credentials out of what remains.
    pub fn url(&self, url: String) -> String {
        let mut url = scrub_url(url);
        self.redact(&mut url);
        url
    }

    /// Cleans the tree that a read found, and encodes it.
    #[cfg(test)]
    pub fn content(&self, found: Found) -> Content {
        self.clean(found).into_content()
    }

    /// Compares the tree that a read found with what capture last handed out for the window,
    /// and updates `last`. Cleans and encodes only a tree that changed since `last`.
    pub fn content_since(&self, found: Found, last: &mut Option<Seen>) -> SnapshotContent {
        let raw = found.raw_hash();
        if let Some(seen) = *last
            && seen.raw == raw
        {
            let mut read = found.read;
            if seen.fit != Fit::Whole {
                count_as_truncated(&mut read);
            }
            return SnapshotContent::Unchanged {
                hash: seen.hash,
                read_time: found.read_time,
                read,
            };
        }
        let cleaned = self.clean(found);
        let fit = cleaned.fit;
        let content = cleaned.into_content();
        let hash = content.hash();
        let previous = last.replace(Seen { raw, hash, fit });
        if previous.is_some_and(|seen| seen.hash == hash) {
            SnapshotContent::Unchanged {
                hash,
                read_time: content.read_time,
                read: content.read,
            }
        } else {
            SnapshotContent::Changed(content)
        }
    }

    /// Takes the parameters of the URLs that `text` mentions, credentials, and one-time codes
    /// out of `text`.
    fn clean_text(&self, text: &mut String) {
        *text = scrub_text_urls(std::mem::take(text));
        self.redact(text);
        *text = scrub_one_time_codes(std::mem::take(text));
    }

    /// Cleans the tree that a read found, and then cuts the clean tree to its bytes, so that a
    /// cut never leaves part of a credential that redaction would have replaced whole.
    pub fn clean(&self, found: Found) -> Cleaned {
        let Found {
            mut root,
            read_time,
            mut read,
            max_bytes,
        } = found;
        let started = Instant::now();
        self.clean_node(&mut root);
        if self.redactor.is_some() {
            debug!(elapsed = ?started.elapsed(), "redacted the content");
        }
        let mut room = max_bytes.saturating_sub(encoding::HEADER_LEN);
        let fit = fit(&mut root, &mut room);
        if fit != Fit::Whole {
            debug!(max_bytes, "the tree encodes too large, and is cut");
            count_as_truncated(&mut read);
        }
        Cleaned {
            root,
            read_time,
            read,
            encoded_len: max_bytes - room,
            fit,
        }
    }

    /// Redacts `text` in place, and leaves a text without credentials untouched.
    fn redact(&self, text: &mut String) {
        if text.is_empty() {
            return;
        }
        if let Some(redactor) = &self.redactor
            && let Cow::Owned(redacted) = redactor.redact(text)
        {
            *text = redacted;
        }
    }

    /// Takes URL parameters, credentials, and one-time codes out of `node` and the elements
    /// under it.
    fn clean_node(&self, node: &mut Node) {
        // Readers leave the content of a terminal unread, and capture keeps only the element,
        // regardless of what a reader handed over.
        if node.role == Role::Terminal {
            *node = Node {
                role: Role::Terminal,
                native_role: std::mem::take(&mut node.native_role),
                visibility: node.visibility,
                ..Node::default()
            };
            return;
        }
        if let Some(url) = node.url.take() {
            node.url = Some(self.url(url));
        }
        // A window or a document is named after its title, which can carry URL parameters.
        if matches!(node.role, Role::Window | Role::Document) {
            node.name = scrub_title(std::mem::take(&mut node.name));
        }
        let texts = std::iter::once(&mut node.name).chain(node.text.as_mut());
        for text in texts {
            self.clean_text(text);
        }
        for child in &mut node.children {
            self.clean_node(child);
        }
    }
}

/// Drops the elements under `node` that nest deeper than [`MAX_DEPTH`]. `node` itself nests
/// `depth` deep.
fn cut_deep(node: &mut Node, depth: usize) -> Fit {
    if depth >= MAX_DEPTH {
        if node.children.is_empty() {
            return Fit::Whole;
        }
        node.children.clear();
        return Fit::TooDeep;
    }
    let mut fit = Fit::Whole;
    for child in &mut node.children {
        if cut_deep(child, depth + 1) != Fit::Whole {
            fit = Fit::TooDeep;
        }
    }
    fit
}

/// Drops the elements under `node` that don't fit in `room`, and takes the bytes of the elements
/// that stay from `room`. `room` holds the bytes that remain of the encoded tree.
///
/// Once the room runs out, the element that doesn't fit loses its children and the end of its
/// texts, and the elements after it in reading order are dropped.
fn fit(node: &mut Node, room: &mut usize) -> Fit {
    let own = element_len(node);
    if own > *room {
        node.children.clear();
        let mut over = element_len(node).saturating_sub(*room);
        for text in
            [node.text.as_mut(), node.url.as_mut(), Some(&mut node.name)].into_iter().flatten()
        {
            over = over.saturating_sub(cut_end(text, over));
        }
        *room = room.saturating_sub(element_len(node));
        return Fit::OutOfRoom;
    }
    *room -= own;
    for i in 0..node.children.len() {
        if self::fit(&mut node.children[i], room) == Fit::OutOfRoom {
            node.children.truncate(i + 1);
            return Fit::OutOfRoom;
        }
    }
    Fit::Whole
}

/// Cuts up to `bytes` bytes off the end of `text`, at a character boundary. Returns how many
/// bytes the cut took.
fn cut_end(text: &mut String, bytes: usize) -> usize {
    let len = text.len();
    text.truncate(text.floor_char_boundary(len.saturating_sub(bytes)));
    len - text.len()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rstest::{fixture, rstest};

    use super::*;
    use crate::encoding;

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

    #[fixture]
    fn scrubber() -> Scrubber {
        let mut scrubber = Scrubber::default();
        scrubber.set_redactor(Stars);
        scrubber
    }

    const fn stats(completeness: Completeness) -> ReadStats {
        ReadStats {
            nodes: 1,
            completeness,
            unknown_roles: 0,
        }
    }

    fn found(root: Node, read: ReadStats) -> Found {
        Found::new(root, Duration::ZERO, read)
    }

    /// A chain of `length` elements, each the only child of the one before.
    fn chain(length: usize) -> Node {
        let mut node = Node::default();
        for _ in 1..length {
            node = Node {
                children: vec![node],
                ..Node::default()
            };
        }
        node
    }

    fn depth(node: &Node) -> usize {
        let mut depth = 0;
        let mut node = node;
        while let Some(child) = node.children.first() {
            depth += 1;
            node = child;
        }
        depth
    }

    #[rstest]
    #[case::complete(Completeness::Complete, Completeness::Truncated)]
    #[case::out_of_time(Completeness::OutOfTime, Completeness::OutOfTime)]
    fn a_tree_that_nests_too_deep_is_cut(
        scrubber: Scrubber,
        #[case] read: Completeness,
        #[case] expected: Completeness,
    ) {
        let content = scrubber.content(found(chain(1_100), stats(read)));

        let root = content.tree.decode().expect("a cut tree decodes");
        assert_eq!(depth(&root), MAX_DEPTH);
        assert_eq!(content.read.completeness, expected);
        assert_eq!(content.tree.bytes(), encoding::encode(&root));
        assert_eq!(content.hash(), ContentHash::of(&root));
    }

    /// A window with `count` paragraphs that each hold `text_bytes` bytes of text.
    fn paragraphs(count: usize, text_bytes: usize) -> Node {
        let paragraph = Node {
            role: Role::Paragraph,
            text: Some("x".repeat(text_bytes)),
            ..Node::default()
        };
        Node {
            role: Role::Window,
            children: vec![paragraph; count],
            ..Node::default()
        }
    }

    #[rstest]
    #[case::many_elements(paragraphs(1_000, 100), 10_000)]
    #[case::one_long_text(paragraphs(1, 20_000), 10_000)]
    #[case::one_long_text_of_wide_characters(
        Node {
            text: Some("é€".repeat(4_000)),
            ..Node::default()
        },
        10_000
    )]
    #[case::a_long_text_after_short_ones(
        Node {
            children: vec![paragraphs(3, 10), paragraphs(1, 20_000), paragraphs(3, 10)],
            ..Node::default()
        },
        10_000
    )]
    // The cut keeps the length of the text below 128 bytes, so its length takes one byte both
    // before and after the cut.
    #[case::a_cut_that_keeps_the_length_of_numbers(paragraphs(1, 200), 150)]
    fn a_tree_that_encodes_too_large_is_cut(
        scrubber: Scrubber,
        #[case] root: Node,
        #[case] max_bytes: usize,
    ) {
        let whole = encoding::encode(&root).len();
        let found = Found::within(root, Duration::ZERO, stats(Completeness::Complete), max_bytes);
        let cleaned = scrubber.clean(found);
        let bound = cleaned.encoded_len;
        let content = cleaned.into_content();

        let encoded = content.tree.bytes().len();
        assert!(encoded <= max_bytes, "{encoded} bytes of {whole}");
        assert!(encoded > max_bytes / 2, "{encoded} bytes of {whole}");
        assert!(encoded <= bound, "{encoded} bytes, {bound} expected at most");
        assert_eq!(content.read.completeness, Completeness::Truncated);
        assert!(content.tree.decode().is_ok());
    }

    #[rstest]
    fn a_cut_through_a_credential_keeps_none_of_it(scrubber: Scrubber) {
        let root = Node {
            role: Role::Paragraph,
            text: Some(format!("{}password hunter2", "a".repeat(80))),
            ..Node::default()
        };
        // A cut before the redaction would leave "hu" of the credential.
        let max_bytes = encoding::encode(&root).len() - 5;
        let found = Found::within(root, Duration::ZERO, stats(Completeness::Complete), max_bytes);
        let text = scrubber.content(found).tree.decode().unwrap().text.unwrap();

        assert!(!text.contains("hu"), "{text}");
        assert!(text.ends_with("password **"), "{text}");
    }

    #[rstest]
    fn an_unchanged_tree_that_was_cut_still_counts_as_truncated(scrubber: Scrubber) {
        let read = || {
            let read = stats(Completeness::Complete);
            Found::within(paragraphs(1, 200), Duration::ZERO, read, 150)
        };
        let mut last = None;
        let _ = scrubber.content_since(read(), &mut last);
        let SnapshotContent::Unchanged { read, .. } = scrubber.content_since(read(), &mut last)
        else {
            panic!("the second read is unchanged");
        };
        assert_eq!(read.completeness, Completeness::Truncated);
    }

    #[rstest]
    fn a_tree_that_fits_is_kept_whole(scrubber: Scrubber) {
        let root = paragraphs(10, 100);
        let max_bytes = encoding::encode(&root).len();
        let found =
            Found::within(root.clone(), Duration::ZERO, stats(Completeness::Complete), max_bytes);
        let cleaned = scrubber.clean(found);

        assert_eq!(cleaned.root, root);
        assert_eq!(cleaned.read.completeness, Completeness::Complete);
    }

    #[rstest]
    fn a_tree_at_the_depth_limit_is_kept_whole(scrubber: Scrubber) {
        let cleaned = scrubber.clean(found(chain(MAX_DEPTH + 1), stats(Completeness::Complete)));

        assert_eq!(depth(&cleaned.root), MAX_DEPTH);
        let content = cleaned.into_content();
        assert_eq!(content.read.completeness, Completeness::Complete);
    }

    #[cfg(feature = "link")]
    #[rstest]
    #[tokio::test]
    async fn a_cut_tree_crosses_the_link(scrubber: Scrubber) {
        use std::time::SystemTime;

        use atuin_common::rmp::frame::{read_frame, write_frame};

        use crate::capture::{Dialog, Record, WindowId};
        use crate::link::HelperFrame;

        let content = scrubber.content(found(chain(1_100), stats(Completeness::Complete)));
        let frame = HelperFrame::Record(Record::Dialog(Dialog {
            window: WindowId(1),
            app: "deep".to_owned(),
            title: String::new(),
            at: SystemTime::UNIX_EPOCH,
            content,
        }));
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).await.unwrap();
        let back: Option<HelperFrame> = read_frame(&mut bytes.as_slice()).await.unwrap();
        assert_eq!(back, Some(frame));
    }

    #[rstest]
    #[case::window(Role::Window, "ci.html?token=hunter2 - Chromium", "ci.html - Chromium")]
    #[case::document(Role::Document, "cb#access_token=abc", "cb")]
    #[case::paragraph(Role::Paragraph, "see a?x=1", "see a?x=1")]
    fn window_and_document_names_lose_url_parameters(
        scrubber: Scrubber,
        #[case] role: Role,
        #[case] name: &str,
        #[case] expected: &str,
    ) {
        let root = Node {
            role: Role::Container,
            children: vec![Node {
                role,
                name: name.to_owned(),
                ..Node::default()
            }],
            ..Node::default()
        };
        let cleaned = scrubber.clean(found(root, stats(Completeness::Complete)));
        assert_eq!(cleaned.root.children[0].name, expected);
    }

    /// The URLs that an element's name or text mentions lose their parameters, regardless of
    /// the element's role.
    #[rstest]
    #[case::text_query(
        "",
        "Sign in at https://auth.example/cb?code=abc now",
        "Sign in at https://auth.example/cb now"
    )]
    #[case::text_fragment("", "https://app.example/#access_token=xyz", "https://app.example/")]
    #[case::text_anchor("", "https://docs.example/a#install", "https://docs.example/a#install")]
    #[case::name("https://e.example/?t=1", "", "https://e.example/")]
    fn urls_in_text_lose_their_parameters(
        scrubber: Scrubber,
        #[case] name: &str,
        #[case] text: &str,
        #[case] expected: &str,
    ) {
        let root = Node {
            role: Role::Container,
            children: vec![Node {
                role: Role::Paragraph,
                name: name.to_owned(),
                text: Some(text.to_owned()),
                ..Node::default()
            }],
            ..Node::default()
        };
        let cleaned = scrubber.clean(found(root, stats(Completeness::Complete)));
        let child = &cleaned.root.children[0];
        let scrubbed = if name.is_empty() {
            child.text.as_deref()
        } else {
            Some(&*child.name)
        };
        assert_eq!(scrubbed, Some(expected));
    }

    #[rstest]
    fn urls_lose_their_parameters_and_credentials(scrubber: Scrubber) {
        let url = "https://hooks.example/services/hunter2/x?token=abc".to_owned();
        assert_eq!(scrubber.url(url.clone()), "https://hooks.example/services/****/x");

        let root = Node {
            role: Role::Document,
            url: Some(url),
            ..Node::default()
        };
        let cleaned = scrubber.clean(found(root, stats(Completeness::Complete)));
        assert_eq!(cleaned.root.url.as_deref(), Some("https://hooks.example/services/****/x"));
    }

    #[rstest]
    fn trees_lose_one_time_codes(scrubber: Scrubber) {
        let root = Node {
            role: Role::Window,
            name: "Sign-in code 482913".to_owned(),
            children: vec![Node {
                role: Role::Paragraph,
                text: Some("Your code is 482913, password hunter2".to_owned()),
                ..Node::default()
            }],
            ..Node::default()
        };
        let read = stats(Completeness::Complete);
        let cleaned = scrubber.clean(found(root, read));
        assert_eq!(cleaned.root.name, "Sign-in code [one-time code]");
        let text = cleaned.root.children[0].text.as_deref();
        assert_eq!(text, Some("Your code is [one-time code], password ****"));
    }

    #[rstest]
    #[case::plain("Firefox", "Firefox")]
    #[case::credential("deploy token hunter2", "deploy token ****")]
    #[case::code("Your code is 482913", "Your code is [one-time code]")]
    #[case::long(&"é".repeat(100), &"é".repeat(MAX_APP_NAME_CHARS))]
    fn app_names_are_cleaned_and_cut(
        scrubber: Scrubber,
        #[case] name: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(scrubber.app_name(name.to_owned()), expected);
    }

    #[rstest]
    fn titles_lose_one_time_codes_and_credentials(scrubber: Scrubber) {
        let title = "Code 482913 for a?token=x password hunter2".to_owned();
        assert_eq!(scrubber.title(title), "Code [one-time code] for a password ****");
    }

    /// Counts the texts that it is asked to redact.
    #[derive(Clone, Default)]
    struct Counting(Arc<AtomicUsize>);

    impl Counting {
        fn calls(&self) -> usize {
            self.0.load(Ordering::SeqCst)
        }
    }

    impl Redact for Counting {
        fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Stars.redact(text)
        }
    }

    /// A page whose document is at `url`, and which shows `text`.
    fn page(url: &str, text: &str) -> Node {
        Node {
            role: Role::Document,
            url: Some(url.to_owned()),
            children: vec![Node {
                role: Role::Paragraph,
                text: Some(text.to_owned()),
                ..Node::default()
            }],
            ..Node::default()
        }
    }

    #[rstest]
    fn an_unchanged_raw_tree_skips_the_scrub() {
        let counting = Counting::default();
        let mut scrubber = Scrubber::default();
        scrubber.set_redactor(counting.clone());
        let read =
            || found(page("https://a.example/", "pw hunter2"), stats(Completeness::Complete));
        let mut last = None;

        let first = scrubber.content_since(read(), &mut last);
        let scrubbed = counting.calls();
        let second = scrubber.content_since(read(), &mut last);

        let SnapshotContent::Changed(content) = first else {
            panic!("the first read is a change: {first:?}");
        };
        assert_eq!(second, SnapshotContent::Unchanged {
            hash: content.hash(),
            read_time: Duration::ZERO,
            read: stats(Completeness::Complete),
        });
        assert!(scrubbed > 0);
        assert_eq!(counting.calls(), scrubbed);
    }

    /// A raw tree can change in what the scrubber removes, and the clean tree stays the same.
    #[rstest]
    fn a_change_that_the_scrub_removes_is_no_change(scrubber: Scrubber) {
        let mut last = None;
        let first = scrubber.content_since(
            found(page("https://a.example/?token=1", "hunter2"), stats(Completeness::Complete)),
            &mut last,
        );
        let second = scrubber.content_since(
            found(page("https://a.example/?token=2", "hunter2"), stats(Completeness::Complete)),
            &mut last,
        );

        assert!(matches!(first, SnapshotContent::Changed(_)), "{first:?}");
        assert!(matches!(second, SnapshotContent::Unchanged { .. }), "{second:?}");
        assert_eq!(second.hash(), first.hash());
    }

    #[rstest]
    fn an_unchanged_tree_that_nests_too_deep_stays_truncated(scrubber: Scrubber) {
        let mut last = None;
        let read = || found(chain(1_100), stats(Completeness::Complete));

        let first = scrubber.content_since(read(), &mut last);
        let second = scrubber.content_since(read(), &mut last);

        assert!(matches!(first, SnapshotContent::Changed(_)), "{first:?}");
        assert!(matches!(second, SnapshotContent::Unchanged { .. }), "{second:?}");
        assert_eq!(second.read().completeness, Completeness::Truncated);
        assert_eq!(second.hash(), first.hash());
    }

    #[rstest]
    fn empty_texts_are_not_redacted() {
        let counting = Counting::default();
        let mut scrubber = Scrubber::default();
        scrubber.set_redactor(counting.clone());
        let root = Node {
            role: Role::Window,
            children: vec![Node::default(), Node {
                name: "Inbox".to_owned(),
                text: Some(String::new()),
                ..Node::default()
            }],
            ..Node::default()
        };

        let _ = scrubber.clean(found(root, stats(Completeness::Complete)));

        assert_eq!(counting.calls(), 1);
    }
}

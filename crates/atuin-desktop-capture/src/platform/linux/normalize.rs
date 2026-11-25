//! Brings the trees that AT-SPI gives into the shape that every backend hands out.
//!
//! AT-SPI exposes the text of an element as hypertext. The text of an element with children
//! holds a marker for each child that it embeds, and toolkits disagree on which children get
//! a marker. [`normalize`] turns the text of each such element into runs of text that sit
//! between the children, so that no reader of a [`Node`] needs to know about markers.

use crate::backend::{Mark, Node, Role, Visibility};
use crate::content::{has_words, is_inline_shaped};

/// The character that marks the position of an embedded child inside an element's text.
pub const EMBEDDED: char = '\u{fffc}';

/// How the text of an element covers the children of the element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// The text marks every child. Firefox gives every child a marker. Chromium does too when
    /// no child is a plain run of text.
    EveryChildMarked,
    /// The text repeats every plain run of text and marks every other child. Chromium does
    /// this, so each run is in the tree twice.
    RunsInText,
    /// The counts fit neither layout. The text can hold a literal U+FFFC, or the element can
    /// have changed between the read of its children and the read of its text.
    Unknown,
}

impl Layout {
    /// Finds the layout of `text`, the text of an element with `children` children, of which
    /// `runs` are plain runs of text. See [`is_run`].
    pub fn of(text: &str, children: usize, runs: usize) -> Self {
        let markers = text.matches(EMBEDDED).count();
        if markers == children {
            Self::EveryChildMarked
        } else if runs > 0 && markers == children - runs {
            Self::RunsInText
        } else {
            Self::Unknown
        }
    }
}

/// Whether `node` is a plain run of text.
///
/// Firefox gives an inline element such as `<code>` the same role as a run, and Chromium
/// gives such an element a run of its own below it. Only the [`Layout`] of the parent tells
/// whether the parent's text repeats a run.
pub fn is_run(node: &Node) -> bool {
    node.role == Role::InlineText && node.children.is_empty()
}

/// Brings the tree under `node` into the shape that every backend hands out.
///
/// Afterwards, no text holds a marker. The text of an element is the element's own content.
/// An element that has text and children too shows its text in place of the children.
/// A table without row elements gets one row element for each line of cells.
pub fn normalize(node: &mut Node) {
    let runs = node.children.iter().filter(|child| is_run(child)).count();
    let layout = node.text.as_deref().map(|text| Layout::of(text, node.children.len(), runs));
    let runs_in_text = match (layout, node.text.as_deref()) {
        (Some(Layout::RunsInText), _) => true,
        (Some(Layout::Unknown), Some(text)) if text.contains(EMBEDDED) => {
            repeats_runs(text, &node.children)
        }
        _ => false,
    };
    if runs_in_text {
        node.children.retain(|child| !is_run(child));
    }
    for child in &mut node.children {
        normalize(child);
    }
    if let Some(text) = node.text.take_if(|text| text.contains(EMBEDDED)) {
        if name_labels_children(node.role, &text) {
            node.name.clear();
        }
        // A row shows only its cells.
        if node.role != Role::Row {
            node.children = embed(&text, std::mem::take(&mut node.children));
        }
    }
    group_rows(node);
}

/// Whether the name of an element with `role` and `text` only labels the element's children,
/// so that the name is not content.
///
/// Text that only marks children leaves the content to the children. A Chromium tab is named
/// after its page, whose title the window title already gives. A link is the exception: a
/// link that holds only an image is named after the image, and that name is the link's text.
pub fn name_labels_children(role: Role, text: &str) -> bool {
    role != Role::Link && text.contains(EMBEDDED) && !text.split(EMBEDDED).any(has_words)
}

/// Whether `text` holds the text of every plain run among `children`, in order.
///
/// Blanks do not count, because capture puts back blanks that Chromium leaves out of `text`.
/// A run without text is skipped.
fn repeats_runs(text: &str, children: &[Node]) -> bool {
    let mut rest = text;
    for run in children.iter().filter(|child| is_run(child)) {
        let Some(run) = run.text.as_deref().filter(|run| has_words(run)) else {
            continue;
        };
        match find_ignoring_blanks(rest, run) {
            Some(end) => rest = &rest[end..],
            None => return false,
        }
    }
    true
}

/// Finds `needle` in `haystack` with the blanks of both left out, and returns the byte offset
/// in `haystack` where the first match ends. An empty `needle` is never found.
fn find_ignoring_blanks(haystack: &str, needle: &str) -> Option<usize> {
    let solid = |c: &char| !c.is_whitespace();
    let first = needle.chars().find(solid)?;
    haystack.char_indices().filter(|&(_, c)| c == first).find_map(|(start, _)| {
        let mut candidate = haystack[start..].char_indices().filter(|(_, c)| solid(c));
        let mut end = start;
        for wanted in needle.chars().filter(solid) {
            let (at, c) = candidate.next()?;
            if c != wanted {
                return None;
            }
            end = start + at + c.len_utf8();
        }
        Some(end)
    })
}

/// Places `children` between the pieces of `text` that the markers separate.
///
/// The markers and the children pair up in order. Children without a marker follow the text,
/// and markers without a child disappear.
fn embed(text: &str, children: Vec<Node>) -> Vec<Node> {
    let mut out = Vec::with_capacity(children.len() * 2 + 1);
    let mut children = children.into_iter();
    let mut pieces = text.split(EMBEDDED).peekable();
    let mut before = pieces.next().unwrap_or_default();
    let mut previous: Option<Neighbour> = None;
    while let Some(&after) = pieces.peek()
        && let Some(child) = children.next()
    {
        pieces.next();
        let neighbour = Neighbour::of(&child);
        push_piece(&mut out, before, previous, Some(neighbour));
        // Two inline children with nothing between them are separate items, such as the
        // links of a navigation bar.
        if before.is_empty() && previous.is_some_and(Neighbour::is_inline) && neighbour.is_inline()
        {
            out.push(run(" ".to_owned()));
        }
        // A container beside words, such as `<b>` in a `<div>`, is part of the running text.
        if (has_words(before) || has_words(after)) && is_inline_container(&child) {
            splice(child, &mut out);
        } else {
            out.push(child);
        }
        previous = Some(neighbour);
        before = after;
    }
    if pieces.peek().is_none() {
        push_piece(&mut out, before, previous, None);
    } else {
        let rest: String = std::iter::once(before).chain(pieces).collect();
        push_piece(&mut out, &rest, previous, None);
    }
    out.extend(children);
    out
}

/// What the text around a child needs to know about the child.
#[derive(Debug, Clone, Copy)]
struct Neighbour {
    role: Role,
    showing: bool,
}

impl Neighbour {
    fn of(node: &Node) -> Self {
        Self {
            role: node.role,
            showing: node.visibility == Visibility::Showing,
        }
    }

    fn is_inline(self) -> bool {
        self.showing && matches!(self.role, Role::InlineText | Role::Link)
    }

    /// Whether the child always takes lines of its own, so blanks beside the child don't show.
    fn is_block(self) -> bool {
        self.showing
            && self.role.is_readable()
            && !matches!(self.role, Role::InlineText | Role::Link | Role::Container)
    }
}

/// Adds `piece` to `out` as a run of text. A piece of blanks beside a block is left out,
/// because the blanks don't show.
fn push_piece(
    out: &mut Vec<Node>,
    piece: &str,
    before: Option<Neighbour>,
    after: Option<Neighbour>,
) {
    let beside_block =
        before.is_some_and(Neighbour::is_block) || after.is_some_and(Neighbour::is_block);
    if piece.is_empty() || (piece.trim().is_empty() && beside_block) {
        return;
    }
    out.push(run(piece.to_owned()));
}

fn run(text: String) -> Node {
    Node {
        role: Role::InlineText,
        text: Some(text),
        ..Node::default()
    }
}

/// Whether `node` is a showing container that holds nothing but text, links, and other such
/// containers.
fn is_inline_container(node: &Node) -> bool {
    node.role == Role::Container
        && node.visibility == Visibility::Showing
        && node.mark == Mark::None
        && is_inline_shaped(node)
}

/// Adds the content of `node`, a container in running text, to `out` as runs and links.
fn splice(node: Node, out: &mut Vec<Node>) {
    if !is_inline_container(&node) {
        if node.visibility == Visibility::Showing {
            out.push(node);
        }
        return;
    }
    match node.text {
        // Text in running text shows in place of the children, blanks included.
        Some(text) if !text.is_empty() => out.push(Node {
            role: Role::InlineText,
            native_role: node.native_role,
            text: Some(text),
            ..Node::default()
        }),
        _ => {
            for child in node.children {
                splice(child, out);
            }
        }
    }
}

/// Puts the cells of a table without row elements into one row element for each line.
///
/// Qt lists the column headings of a tree view and then every cell, row after row.
fn group_rows(node: &mut Node) {
    let columns = node.children.iter().take_while(|child| child.role == Role::ColumnHeader).count();
    let only_cells = node.children[columns..].iter().all(|child| child.role == Role::Cell);
    if node.role == Role::Row || columns < 2 || !only_cells {
        return;
    }
    let mut cells = std::mem::take(&mut node.children).into_iter();
    loop {
        let row: Vec<Node> = cells.by_ref().take(columns).collect();
        if row.is_empty() {
            break;
        }
        node.children.push(Node {
            role: Role::Row,
            children: row,
            ..Node::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::render::render;

    fn node(role: Role, text: &str, children: Vec<Node>) -> Node {
        Node {
            role,
            text: (!text.is_empty()).then(|| text.to_owned()),
            children,
            ..Node::default()
        }
    }

    fn named(role: Role, name: &str) -> Node {
        Node {
            role,
            name: name.to_owned(),
            ..Node::default()
        }
    }

    fn hidden(mut node: Node) -> Node {
        node.visibility = Visibility::Hidden;
        node
    }

    fn rendered(mut root: Node) -> String {
        normalize(&mut root);
        render(&root, usize::MAX).text
    }

    /// Whether any text in the tree under `node` holds a marker.
    fn has_markers(node: &Node) -> bool {
        node.text.as_deref().is_some_and(|text| text.contains(EMBEDDED))
            || node.children.iter().any(has_markers)
    }

    #[rstest]
    #[case::leaf("text", 0, 0, Layout::EveryChildMarked)]
    #[case::firefox("a \u{fffc} b \u{fffc}", 2, 1, Layout::EveryChildMarked)]
    #[case::chromium("a \u{fffc} b", 3, 2, Layout::RunsInText)]
    #[case::chromium_only_runs("a b", 2, 2, Layout::RunsInText)]
    #[case::literal_marker_in_firefox("a \u{fffc} b \u{fffc}", 1, 0, Layout::Unknown)]
    #[case::literal_marker_in_chromium("a \u{fffc} b \u{fffc}", 3, 2, Layout::Unknown)]
    #[case::text_over_children("label", 1, 0, Layout::Unknown)]
    fn layouts_follow_the_counts(
        #[case] text: &str,
        #[case] children: usize,
        #[case] runs: usize,
        #[case] expected: Layout,
    ) {
        assert_eq!(Layout::of(text, children, runs), expected);
    }

    #[rstest]
    fn chromium_runs_are_kept_once() {
        // Chromium: the inline text children repeat the parent's text, and the link owns the
        // marker.
        let root = node(Role::Paragraph, "see \u{fffc} now", vec![
            node(Role::InlineText, "see ", vec![]),
            node(Role::Link, "the docs", vec![node(Role::InlineText, "the docs", vec![])]),
            node(Role::InlineText, " now", vec![]),
        ]);
        assert_eq!(rendered(root), "see the docs now");
    }

    #[rstest]
    fn a_firefox_inline_element_keeps_its_marker() {
        // Firefox: the code element and the link each own a marker.
        let root = node(Role::Paragraph, "why does \u{fffc} fail on \u{fffc}?", vec![
            node(Role::InlineText, "cargo test", vec![]),
            node(Role::Link, "CI run 7", vec![]),
        ]);
        assert_eq!(rendered(root), "why does cargo test fail on CI run 7?");
    }

    #[rstest]
    fn a_chromium_inline_element_keeps_its_marker_among_repeated_runs() {
        // Chromium: the heading repeats its two plain runs, and marks the code element and
        // the link. The code element repeats its own run.
        let code = node(Role::InlineText, "inline code", vec![node(
            Role::InlineText,
            "inline code",
            vec![],
        )]);
        let heading = node(Role::Heading, "Title with \u{fffc} and \u{fffc}", vec![
            node(Role::InlineText, "Title with ", vec![]),
            code,
            node(Role::InlineText, " and ", vec![]),
            node(Role::Link, "a link", vec![]),
        ]);
        assert_eq!(rendered(heading), "# Title with inline code and a link");
    }

    #[rstest]
    fn a_literal_marker_keeps_the_runs_of_firefox() {
        // Firefox: the page's own U+FFFC adds a marker, and the code element has none of the
        // parent's text.
        let root = node(Role::Paragraph, "a \u{fffc} b \u{fffc} c", vec![node(
            Role::InlineText,
            "code",
            vec![],
        )]);
        assert_eq!(rendered(root), "a code b  c");
    }

    #[rstest]
    fn a_literal_marker_keeps_the_runs_of_chromium_once() {
        // Chromium: the page's own U+FFFC adds a marker, so the counts don't fit any layout, and
        // the parent's text still repeats both runs.
        let root = node(Role::Paragraph, "one \u{fffc} two \u{fffc} three", vec![
            node(Role::InlineText, "one \u{fffc} two ", vec![]),
            node(Role::Link, "link", vec![]),
            node(Role::InlineText, " three", vec![]),
        ]);
        assert_eq!(rendered(root), "one link two  three");
    }

    #[rstest]
    fn text_over_unmarked_children_stays() {
        // GNOME Shell: a label holds its text again in a child.
        let root = node(Role::Label, "Authentication Required", vec![node(
            Role::Editable,
            "Authentication Required",
            vec![],
        )]);
        assert_eq!(rendered(root), "Authentication Required");
    }

    #[rstest]
    #[case::link(Role::Link, "Linked image", "Linked image")]
    #[case::tab(Role::Label, "Article page", "")]
    fn names_of_elements_that_only_mark_children(
        #[case] role: Role,
        #[case] name: &str,
        #[case] expected: &str,
    ) {
        // Chromium: a link around an image, and a tab with its close button.
        let mut root = named(role, name);
        root.text = Some("\u{fffc}".to_owned());
        root.children = vec![named(Role::Controls, "image")];
        assert_eq!(rendered(root), expected);
    }

    #[rstest]
    fn touching_inline_children_are_separated() {
        let link = |text| node(Role::Link, text, vec![]);
        let root = node(Role::Container, "\u{fffc}\u{fffc} (\u{fffc})", vec![
            link("All issues"),
            link("All pull requests"),
            link("#4703"),
        ]);
        assert_eq!(rendered(root), "All issues All pull requests (#4703)");
    }

    #[rstest]
    fn formatting_containers_continue_the_line() {
        // Chromium on Wikipedia: `<b>` is a section that repeats its run, and a footnote is a
        // superscript that holds a link, which holds sections for the brackets.
        let bracket =
            |text| node(Role::Container, text, vec![node(Role::InlineText, text, vec![])]);
        let footnote =
            node(Role::Container, "\u{fffc}", vec![node(Role::Link, "\u{fffc}", vec![node(
                Role::Container,
                "\u{fffc}1\u{fffc}",
                vec![bracket("["), node(Role::InlineText, "1", vec![]), bracket("]")],
            )])]);
        let root =
            node(Role::Paragraph, "\u{fffc}, formally \u{fffc}, is a standard.\u{fffc} It", vec![
                bracket("C23"),
                node(Role::InlineText, ", formally ", vec![]),
                bracket("ISO/IEC 9899:2024"),
                node(Role::InlineText, ", is a standard.", vec![]),
                footnote,
                node(Role::InlineText, " It", vec![]),
            ]);
        assert_eq!(rendered(root), "C23, formally ISO/IEC 9899:2024, is a standard.[1] It");
    }

    #[rstest]
    fn touching_formatting_is_one_word() {
        let span = |text| node(Role::Container, text, vec![]);
        let root =
            node(Role::Paragraph, "add \u{fffc}\u{fffc} prefixes", vec![span("0"), span("b")]);
        assert_eq!(rendered(root), "add 0b prefixes");
    }

    #[rstest]
    #[case::container(Role::Container, "ends here.[1] Next")]
    #[case::list_item(Role::ListItem, "- ends here.[1] Next")]
    fn a_container_beside_words_continues_the_line(#[case] role: Role, #[case] expected: &str) {
        // Firefox: a superscript between two runs.
        let root =
            node(role, "ends here.\u{fffc} Next", vec![node(Role::Container, "[1]", vec![])]);
        assert_eq!(rendered(root), expected);
    }

    #[rstest]
    fn blocks_in_a_list_item_stay_apart() {
        // Chromium: `<li><div>Title</div><div>Description</div></li>`.
        let root = node(Role::ListItem, "\u{fffc}\u{fffc}", vec![
            node(Role::Container, "Title", vec![]),
            node(Role::Container, "Description", vec![]),
        ]);
        assert_eq!(rendered(root), "- Title\nDescription");
    }

    #[rstest]
    fn a_prefix_stays_on_the_line_of_its_first_block() {
        let root = node(Role::Document, "", vec![
            node(Role::ListItem, "\u{fffc}", vec![node(Role::Paragraph, "Code", vec![])]),
            node(Role::Heading, "\u{fffc}", vec![node(Role::Container, "About", vec![])]),
        ]);
        assert_eq!(rendered(root), "- Code\n# About");
    }

    #[rstest]
    fn containers_side_by_side_stay_blocks() {
        // Chromium: a page's `<div>`s own adjacent markers, with no text between them.
        let root = node(Role::Document, "\u{fffc}\u{fffc}", vec![
            node(Role::Container, "first", vec![]),
            node(Role::Container, "second", vec![]),
        ]);
        assert_eq!(rendered(root), "first\nsecond");
    }

    #[rstest]
    fn a_container_of_blocks_in_running_text_stays_a_block() {
        let root = node(Role::Paragraph, "\u{fffc}", vec![node(Role::Container, "", vec![
            node(Role::Paragraph, "one", vec![]),
            node(Role::Paragraph, "two", vec![]),
        ])]);
        assert_eq!(rendered(root), "one\ntwo");
    }

    #[rstest]
    fn embedded_blocks_start_their_own_lines() {
        let root = node(Role::Container, "intro \u{fffc} outro", vec![node(
            Role::Paragraph,
            "middle",
            vec![],
        )]);
        assert_eq!(rendered(root), "intro\nmiddle\noutro");
    }

    #[rstest]
    fn hidden_children_keep_marker_positions() {
        let root = node(Role::Document, "\u{fffc}\u{fffc}\u{fffc}", vec![
            hidden(node(Role::Paragraph, "scrolled away", vec![])),
            node(Role::Paragraph, "on screen", vec![]),
            hidden(node(Role::Paragraph, "below the fold", vec![])),
        ]);
        assert_eq!(rendered(root), "on screen");
    }

    #[rstest]
    fn children_without_markers_follow_the_text() {
        // Chromium: a panel of the browser marks only some of its children.
        let root = node(Role::Container, "\u{fffc} middle", vec![
            node(Role::Paragraph, "first", vec![]),
            node(Role::Paragraph, "last", vec![]),
        ]);
        assert_eq!(rendered(root), "first\nmiddle\nlast");
    }

    #[rstest]
    fn rows_ignore_their_text() {
        let cell = |text| node(Role::Cell, text, vec![]);
        let root = node(Role::Row, "\u{fffc} \u{fffc}", vec![cell("a"), cell("b")]);
        assert_eq!(rendered(root), "a | b");
    }

    #[rstest]
    fn cells_after_column_headings_form_rows() {
        // Qt: a tree view lists its column headings, then every cell, with no row elements.
        let gone = hidden(named(Role::Cell, "scrolled away"));
        let mut root = node(Role::Container, "", vec![
            named(Role::ColumnHeader, "No."),
            named(Role::ColumnHeader, "Info"),
            named(Role::Cell, "4"),
            named(Role::Cell, "Client Hello"),
            gone.clone(),
            gone,
            named(Role::Cell, "5"),
            named(Role::Cell, "Alert"),
        ]);
        normalize(&mut root);
        let shape: Vec<(Role, usize)> =
            root.children.iter().map(|row| (row.role, row.children.len())).collect();
        assert_eq!(shape, [(Role::Row, 2); 4]);
        assert_eq!(render(&root, usize::MAX).text, "No. | Info\n4 | Client Hello\n5 | Alert");
    }

    #[rstest]
    #[case::one_column(&[Role::ColumnHeader, Role::Cell, Role::Cell])]
    #[case::other_children(&[Role::ColumnHeader, Role::ColumnHeader, Role::Paragraph])]
    fn other_children_form_no_rows(#[case] roles: &[Role]) {
        let mut root =
            node(Role::Container, "", roles.iter().map(|&role| named(role, "x")).collect());
        normalize(&mut root);
        assert!(root.children.iter().all(|child| child.role != Role::Row));
    }

    #[rstest]
    #[case::blanks("a b", " ", Some(1))]
    #[case::match_across_blanks("ab cd", "abc", Some(4))]
    #[case::first_candidate_fails("aab", "ab", Some(3))]
    #[case::missing("abc", "x", None)]
    #[case::empty_needle("abc", " ", None)]
    #[case::wide("日本 語", "本語", Some(10))]
    fn needles_are_found_without_blanks(
        #[case] haystack: &str,
        #[case] needle: &str,
        #[case] expected: Option<usize>,
    ) {
        let expected = expected.filter(|_| needle.chars().any(|c| !c.is_whitespace()));
        assert_eq!(find_ignoring_blanks(haystack, needle), expected);
    }

    mod properties {
        use proptest::prelude::*;

        use super::*;

        fn leaf() -> impl Strategy<Value = Node> {
            (
                prop::sample::select(vec![
                    Role::InlineText,
                    Role::Link,
                    Role::Container,
                    Role::Paragraph,
                    Role::Cell,
                    Role::ColumnHeader,
                ]),
                "[a \u{fffc}]{0,6}",
            )
                .prop_map(|(role, text)| node(role, &text, vec![]))
        }

        fn tree() -> impl Strategy<Value = Node> {
            leaf().prop_recursive(4, 40, 5, |inner| {
                (leaf(), prop::collection::vec(inner, 0..5)).prop_map(|(mut parent, children)| {
                    parent.children = children;
                    parent
                })
            })
        }

        proptest! {
            #[test]
            fn no_marker_survives(mut root in tree()) {
                normalize(&mut root);
                prop_assert!(!has_markers(&root));
            }
        }
    }
}

//! Renders an accessibility tree as Markdown-like text.

use crate::backend::{Completeness, Mark, Node, Role, Visibility};
use crate::content::{has_words, is_icon, is_inline_shaped, uses_name};

#[derive(Debug, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    pub completeness: Completeness,
}

/// What replaces the middle of a text that is too long.
const ELISION: &str = "\n[…]\n";

/// The line that stands for a terminal, whose content is never read.
const TERMINAL: &str = "[terminal]";

/// Renders the content under `root`.
///
/// A text that is longer than `max_bytes` keeps its beginning and its end, because the end of
/// a log is usually the part that matters.
#[must_use]
pub fn render(root: &Node, max_bytes: usize) -> Rendered {
    let mut writer = Writer::new(Markers::Write);
    writer.node(root, Flow::Own);
    let mut text = writer.out;
    text.truncate(text.trim_end().len());
    let completeness = if text.len() > max_bytes {
        let half = max_bytes / 2;
        let head = text.floor_char_boundary(half);
        let tail = text.ceil_char_boundary(text.len() - half);
        text.replace_range(head..tail, ELISION);
        Completeness::Truncated
    } else {
        Completeness::Complete
    };
    Rendered { text, completeness }
}

/// Whether `root` holds nothing that [`render`] would write.
///
/// This walks the tree without building any text.
#[must_use]
pub fn is_blank(root: &Node) -> bool {
    // The line that stands for a terminal shows where the terminal is.
    if root.role == Role::Terminal {
        return root.visibility != Visibility::Showing;
    }
    if !root.is_rendered() {
        return true;
    }
    let own = root.text.as_deref().is_some_and(has_words)
        || (uses_name(root.role) && has_words(&root.name));
    !own && root.children.iter().all(is_blank)
}

/// The first line of the text under `root`, cut to `max_chars` characters and an ellipsis.
///
/// The line has no Markdown marker of a heading, a list item, or an option, so a line that
/// starts with a `#` or a `-` of its own keeps it.
#[must_use]
pub fn first_line(root: &Node, max_chars: usize) -> String {
    let mut writer = Writer {
        until: Until::FirstBreak,
        ..Writer::new(Markers::Omit)
    };
    writer.node(root, Flow::Own);
    let mut line = writer.out;
    line.truncate(line.find('\n').unwrap_or(line.len()));
    line.truncate(line.trim_end().len());
    if let Some((cut, _)) = line.char_indices().nth(max_chars) {
        line.truncate(cut);
        line.push('…');
    }
    line
}

/// Whether the text starts lines with the Markdown markers of headings, list items, and options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Markers {
    Write,
    Omit,
}

/// How much of the tree a [`Writer`] writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Until {
    End,
    /// The writer stops after the first line break.
    FirstBreak,
}

struct Writer {
    out: String,
    /// Where the prefix of the current line ends, when the line holds nothing else yet.
    prefix_end: usize,
    markers: Markers,
    until: Until,
    /// Whether the writer reached the point where [`until`](Self::until) stops it.
    done: bool,
    /// The buffers that rows render their cells into, which every row reuses.
    rows: Option<Box<RowBuffers>>,
}

/// The buffers of a row: one for the text of a cell, and one for the line of the row.
struct RowBuffers {
    cell: Writer,
    line: String,
}

/// Where an element's content goes relative to the text around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    /// The role decides whether the element starts a line of its own.
    Own,
    /// The element continues the line of the text around it.
    Inline,
}

/// What the children of an element sit in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surroundings {
    /// A line of text, which an inline container continues.
    RunningText,
    Blocks,
}

/// Where a node started in the output, for undoing the node when it didn't write any content.
#[derive(Clone, Copy)]
struct Opened {
    /// Whether the node is a block, which takes lines of its own.
    block: bool,
    /// Where the node's marker starts.
    start: usize,
    /// The line start that the marker of the node around it marked.
    outer_end: usize,
    /// Where the node's content starts, after its marker.
    content: usize,
}

impl Writer {
    const fn new(markers: Markers) -> Self {
        Self {
            out: String::new(),
            prefix_end: 0,
            markers,
            until: Until::End,
            done: false,
            rows: None,
        }
    }

    /// Empties the writer for another text, and keeps its buffers.
    fn clear(&mut self) {
        self.out.clear();
        self.prefix_end = 0;
        self.done = false;
    }

    fn node(&mut self, node: &Node, flow: Flow) {
        if self.done {
            return;
        }
        if node.role == Role::Terminal {
            self.terminal(node);
            return;
        }
        if !node.is_rendered() {
            return;
        }
        // Browsers expose inline formatting such as `<b>` or `<sup>` as a container. Inside
        // running text, such a container continues the line.
        let surroundings = if flow == Flow::Inline || holds_running_text(node.role) {
            Surroundings::RunningText
        } else {
            Surroundings::Blocks
        };
        let opened = self.open(node, flow);
        if node.role == Role::Row {
            self.cells(&node.children);
        } else {
            self.content(node, &opened, surroundings);
        }
        self.close(opened);
    }

    /// Writes a line of its own that shows where the terminal `node` is. The line counts as
    /// layout, so a text without markers leaves it out.
    ///
    /// The parts of one terminal can stand side by side when a browser leaves the terminal
    /// element out of its tree, so a terminal right after another one doesn't add a line.
    fn terminal(&mut self, node: &Node) {
        if node.visibility != Visibility::Showing || self.markers != Markers::Write {
            return;
        }
        self.break_line();
        if !self.out.trim_end().ends_with(TERMINAL) {
            self.text(TERMINAL);
            self.break_line();
        }
    }

    /// Starts `node`: a block on a line of its own, and then the marker that starts the line of
    /// the node, if the node has one: the hashes of a heading, the dash of a list item, or the
    /// box of an option.
    fn open(&mut self, node: &Node, flow: Flow) -> Opened {
        let block = flow == Flow::Own && is_block(node.role);
        if block {
            self.break_line();
        }
        let start = self.out.len();
        let outer_end = self.prefix_end;
        match node.role {
            _ if self.markers == Markers::Omit => {}
            Role::Heading => {
                let level = usize::from(node.heading_level.unwrap_or(1).clamp(1, 6));
                self.out.extend(std::iter::repeat_n('#', level));
                self.out.push(' ');
            }
            Role::ListItem => self.out.push_str("- "),
            _ if node.mark == Mark::Checked => self.out.push_str("[x] "),
            _ if node.mark == Mark::Unchecked => self.out.push_str("[ ] "),
            _ => {}
        }
        if self.out.len() > start {
            self.prefix_end = self.out.len();
        }
        Opened {
            block,
            start,
            outer_end,
            content: self.out.len(),
        }
    }

    /// Ends a node that `open` started. A block or a prefix without content leaves nothing
    /// behind, and a block with content ends its line.
    fn close(&mut self, opened: Opened) {
        if self.out[opened.content..].trim().is_empty() {
            if opened.block || opened.content > opened.start {
                self.out.truncate(opened.start);
                // The prefix is gone, and with it the line start that it marked.
                self.prefix_end = opened.outer_end;
            }
        } else if opened.block {
            self.break_line();
        }
    }

    /// Writes the cells of one row of a table on a line of their own, set apart by `|`.
    fn cells(&mut self, cells: &[Node]) {
        let mut buffers = self.rows.take().unwrap_or_else(|| {
            Box::new(RowBuffers {
                cell: Self::new(self.markers),
                line: String::new(),
            })
        });
        let RowBuffers { cell: writer, line } = &mut *buffers;
        line.clear();
        let mut has_content = false;
        // A hidden cell keeps its place in the row, and controls take none.
        let readable = cells.iter().filter(|cell| cell.role.is_readable());
        for (i, cell) in readable.enumerate() {
            if i > 0 {
                line.push_str(" | ");
            }
            if cell.visibility == Visibility::Hidden {
                continue;
            }
            writer.clear();
            writer.node(cell, Flow::Own);
            let text = writer.out.trim_end();
            for (j, part) in text.lines().enumerate() {
                if j > 0 {
                    line.push(' ');
                }
                line.push_str(part.trim());
            }
            has_content |= !text.is_empty();
        }
        if has_content {
            self.break_line();
            self.text(line.trim_end());
            self.break_line();
        }
        self.rows = Some(buffers);
    }

    /// Writes the text and the children of `node`, which `opened` started.
    fn content(&mut self, node: &Node, opened: &Opened, surroundings: Surroundings) {
        match node.text.as_deref() {
            // An element's own text shows in place of its children. A run of blanks between
            // two inline elements is the space between them.
            Some(text) if !text.trim().is_empty() || (!opened.block && !text.is_empty()) => {
                self.text(text);
            }
            _ => {
                // A name that toolkits build from an element's children repeats them, such
                // as the cells of a GTK 4 list or the labels of GNOME Shell.
                if uses_name(node.role) && node.children.iter().all(is_blank) {
                    self.text(&node.name);
                }
                // Two links with nothing between them are separate items, such as the links
                // of a navigation bar.
                let mut after_link = false;
                for child in &node.children {
                    let link = child.role == Role::Link && child.visibility == Visibility::Showing;
                    if after_link && link && !self.at_line_start() {
                        self.out.push(' ');
                    }
                    self.node(child, child_flow(child, surroundings));
                    after_link = link;
                }
            }
        }
    }

    /// Whether the current line holds nothing except a Markdown prefix.
    fn at_line_start(&self) -> bool {
        self.out.is_empty() || self.out.ends_with('\n') || self.out.len() == self.prefix_end
    }

    fn break_line(&mut self) {
        if self.at_line_start() {
            return;
        }
        let trimmed = self.out.trim_end_matches(' ').len();
        self.out.truncate(trimmed);
        if !self.at_line_start() {
            self.out.push('\n');
            self.done = self.until == Until::FirstBreak;
        }
    }

    fn text(&mut self, text: &str) {
        if self.done {
            return;
        }
        let written = self.out.len();
        let at_line_start = self.at_line_start();
        if text.contains(is_icon) {
            let mut chars = text.chars().filter(|&c| !is_icon(c)).peekable();
            if at_line_start {
                while chars.next_if(|c| c.is_whitespace()).is_some() {}
            }
            self.out.extend(chars);
        } else if at_line_start {
            self.out.push_str(text.trim_start());
        } else {
            self.out.push_str(text);
        }
        self.done = self.until == Until::FirstBreak && self.out[written..].contains('\n');
    }
}

const fn is_block(role: Role) -> bool {
    !matches!(role, Role::InlineText | Role::Link)
}

/// Whether an element with this role holds a line of text rather than other blocks.
///
/// List items and cells often hold blocks, such as a title and a description, which browsers
/// give the same role as inline formatting. Their containers continue the line only beside
/// words.
const fn holds_running_text(role: Role) -> bool {
    matches!(role, Role::Paragraph | Role::Heading | Role::Label | Role::Link | Role::InlineText)
}

/// How `child` flows in `surroundings`.
///
/// A container in running text continues the line when it holds nothing but text and links.
fn child_flow(child: &Node, surroundings: Surroundings) -> Flow {
    if surroundings == Surroundings::RunningText
        && child.role == Role::Container
        && is_inline_shaped(child)
    {
        Flow::Inline
    } else {
        Flow::Own
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

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

    fn rendered(root: &Node) -> String {
        render(root, usize::MAX).text
    }

    #[rstest]
    fn blocks_become_lines_with_markdown_prefixes() {
        let mut heading = node(Role::Heading, "Build failed", vec![]);
        heading.heading_level = Some(2);
        let root = node(Role::Document, "", vec![
            heading,
            node(Role::Paragraph, "one error", vec![]),
            node(Role::ListItem, "first", vec![]),
            named(Role::Label, "status: red"),
        ]);
        assert_eq!(rendered(&root), "## Build failed\none error\n- first\nstatus: red");
    }

    #[rstest]
    fn links_side_by_side_are_set_apart() {
        // The AX API on macOS has no placeholders: the children follow one another.
        let link = |name: &str| named(Role::Link, name);
        let bar = node(Role::Container, "", vec![
            link("Home"),
            link("Docs"),
            node(Role::InlineText, " ", vec![]),
            link("Blog"),
        ]);
        assert_eq!(rendered(&bar), "Home Docs Blog");
    }

    #[rstest]
    #[case::text("first point", "- first point")]
    #[case::nothing("", "")]
    fn list_items_get_a_marker(#[case] text: &str, #[case] expected: &str) {
        assert_eq!(rendered(&node(Role::ListItem, text, vec![])), expected);
    }

    #[rstest]
    #[case(Mark::Checked, "[x] Display line numbers")]
    #[case(Mark::Unchecked, "[ ] Display line numbers")]
    #[case(Mark::None, "Display line numbers")]
    fn options_show_their_state(#[case] mark: Mark, #[case] expected: &str) {
        let mut option = named(Role::Label, "Display line numbers");
        option.mark = mark;
        assert_eq!(rendered(&option), expected);
    }

    #[rstest]
    fn a_name_is_left_out_when_the_children_have_content() {
        // GNOME Shell: a label holds its text again in a child.
        let mut label = named(Role::Label, "Authentication Required");
        label.children = vec![node(Role::Editable, "Authentication Required", vec![])];
        // GTK 4: a cell is named after the image and the label inside it.
        let mut cell = named(Role::Cell, "GConf GConf");
        cell.children = vec![node(Role::Container, "", vec![
            named(Role::Controls, "GConf"),
            named(Role::Label, "GConf"),
        ])];
        let root = node(Role::Window, "", vec![label, cell, named(Role::Cell, "1 item")]);
        assert_eq!(rendered(&root), "Authentication Required\nGConf\n1 item");
    }

    #[rstest]
    fn rows_put_their_cells_on_one_line() {
        // GTK 4: a column view holds a row of headings and a row element for each item.
        let cell = |text| node(Role::Cell, "", vec![named(Role::Label, text)]);
        let row = |cells| node(Role::Row, "", cells);
        let root = node(Role::Container, "", vec![
            row(vec![named(Role::ColumnHeader, "Name"), named(Role::ColumnHeader, "Size")]),
            row(vec![cell("GConf"), cell("1 item")]),
            row(vec![cell("KDE"), named(Role::Controls, "expander"), cell("")]),
            hidden(row(vec![cell("scrolled away"), cell("3 items")])),
        ]);
        assert_eq!(rendered(&root), "Name | Size\nGConf | 1 item\nKDE |");
    }

    #[rstest]
    fn icon_glyphs_are_left_out() {
        // VS Code: each toolbar icon is a codicon character inside a label.
        let root = node(Role::Window, "", vec![
            node(Role::Label, "\u{eaf0}", vec![]),
            node(Role::Label, "\u{eab4} src", vec![]),
            node(Role::Label, "report.rs", vec![]),
        ]);
        assert_eq!(rendered(&root), "src\nreport.rs");
    }

    #[rstest]
    #[case::secret(Role::Secret)]
    #[case::controls(Role::Controls)]
    fn unreadable_roles_are_omitted(#[case] role: Role) {
        let root = node(Role::Window, "", vec![
            node(role, "hunter2", vec![node(Role::Label, "nested", vec![])]),
            node(Role::Paragraph, "kept", vec![]),
        ]);
        assert_eq!(rendered(&root), "kept");
    }

    #[rstest]
    #[case::showing(Visibility::Showing, "above\n[terminal]\nbelow")]
    #[case::hidden(Visibility::Hidden, "above\nbelow")]
    fn a_terminal_shows_as_a_line_without_its_content(
        #[case] visibility: Visibility,
        #[case] expected: &str,
    ) {
        let mut terminal = node(Role::Terminal, "ls ~/secrets", vec![named(Role::Label, "row")]);
        terminal.name = "zsh".to_owned();
        terminal.visibility = visibility;
        let root = node(Role::Window, "", vec![
            node(Role::Paragraph, "above", vec![]),
            terminal,
            node(Role::Paragraph, "below", vec![]),
        ]);
        assert_eq!(rendered(&root), expected);
        assert_eq!(first_line(&root.children[1], 80), "");
        assert_eq!(is_blank(&root.children[1]), visibility == Visibility::Hidden);
    }

    /// Safari leaves xterm.js's terminal element out, and shows its parts side by side.
    #[rstest]
    fn the_parts_of_a_terminal_show_as_one_line() {
        let part = || node(Role::Terminal, "", vec![]);
        let root = node(Role::Document, "", vec![
            node(Role::Paragraph, "above", vec![]),
            part(),
            part(),
            hidden(node(Role::Paragraph, "hidden", vec![])),
            part(),
            node(Role::Paragraph, "below", vec![]),
            part(),
        ]);
        assert_eq!(rendered(&root), "above\n[terminal]\nbelow\n[terminal]");
    }

    #[rstest]
    fn an_empty_prefix_does_not_join_the_lines_after_it() {
        let root = node(Role::Document, "", vec![
            node(Role::ListItem, "", vec![]),
            node(Role::Paragraph, "ab", vec![]),
            node(Role::Paragraph, "cd", vec![]),
        ]);
        assert_eq!(rendered(&root), "ab\ncd");
    }

    #[rstest]
    fn empty_blocks_leave_no_prefix_behind() {
        let root = node(Role::Document, "", vec![
            node(Role::ListItem, "", vec![node(Role::Container, "  ", vec![])]),
            node(Role::Heading, "", vec![]),
            node(Role::Paragraph, "text", vec![]),
        ]);
        assert_eq!(rendered(&root), "text");
    }

    #[rstest]
    #[case::first_line("Authentication Required\nNeeded to run true", "Authentication Required")]
    #[case::long(&"x".repeat(120), &format!("{}…", "x".repeat(100)))]
    #[case::a_hash_of_its_own("#4711 failed", "#4711 failed")]
    fn first_line_takes_the_text_up_to_the_first_break(#[case] text: &str, #[case] expected: &str) {
        assert_eq!(first_line(&node(Role::Paragraph, text, vec![]), 100), expected);
    }

    #[rstest]
    fn first_line_leaves_out_markers() {
        let mut heading = node(Role::Heading, "Retry?", vec![]);
        heading.heading_level = Some(2);
        let mut option = named(Role::Label, "Remember");
        option.mark = Mark::Checked;
        for root in [heading, node(Role::ListItem, "Retry?", vec![]), option] {
            let expected = if root.role == Role::Label {
                "Remember"
            } else {
                "Retry?"
            };
            assert_eq!(first_line(&root, 100), expected);
        }
    }

    #[rstest]
    #[case::between_blocks(
        vec![node(Role::Paragraph, "first", vec![]), node(Role::Paragraph, "second", vec![])],
        "first\n"
    )]
    #[case::inside_a_text(vec![node(Role::Paragraph, "first\nsecond", vec![]), node(Role::Paragraph, "third", vec![])], "first\nsecond\n")]
    #[case::after_a_row(
        vec![node(Role::Row, "", vec![node(Role::Cell, "a", vec![]), node(Role::Cell, "b\nc", vec![])]), node(Role::Paragraph, "third", vec![])],
        "a | b c\n"
    )]
    fn the_first_line_is_written_without_the_rest(#[case] children: Vec<Node>, #[case] out: &str) {
        let mut writer = Writer {
            until: Until::FirstBreak,
            ..Writer::new(Markers::Omit)
        };
        writer.node(&node(Role::Container, "", children), Flow::Own);
        assert_eq!(writer.out, out);
    }

    #[rstest]
    fn rows_after_rows_reuse_their_buffers() {
        let cell = |text| node(Role::Cell, text, vec![]);
        let row = |cells| node(Role::Row, "", cells);
        let nested = node(Role::Cell, "", vec![row(vec![cell("inner a"), cell("inner b")])]);
        let root = node(Role::Container, "", vec![
            row(vec![cell("a"), cell("b")]),
            row(vec![nested, cell("c")]),
            row(vec![cell("d"), cell("e")]),
        ]);
        assert_eq!(rendered(&root), "a | b\ninner a | inner b | c\nd | e");
    }

    #[rstest]
    #[case::fits(100, "héllo\nwörld", Completeness::Complete)]
    #[case::keeps_both_ends(6, "hé\n[…]\nrld", Completeness::Truncated)]
    #[case::cuts_at_char_boundaries(4, "h\n[…]\nld", Completeness::Truncated)]
    fn long_output_keeps_its_beginning_and_end(
        #[case] max_bytes: usize,
        #[case] text: &str,
        #[case] completeness: Completeness,
    ) {
        let root = node(Role::Document, "", vec![
            node(Role::Paragraph, "héllo", vec![]),
            node(Role::Paragraph, "wörld", vec![]),
        ]);
        let expected = Rendered {
            text: text.to_owned(),
            completeness,
        };
        assert_eq!(render(&root, max_bytes), expected);
    }
}

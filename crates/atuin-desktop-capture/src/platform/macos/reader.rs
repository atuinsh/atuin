//! Reads a window into a tree of [`Node`]s.

use std::time::{Duration, Instant};

use core_foundation::base::CFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation::url::CFURL;

use super::element::{
    AttributeNames, AxStr, Element, Elements, elements_of, is_text, pair_of, str_of, string_from,
    string_of,
};
use super::{ffi, quirks};
use crate::backend::{Completeness, Extent, Limits, Loading, Mark, Node, ReadStats, Role};
use crate::layout::{Frame, reading_order};
use crate::outline::{Outline, OutlineEntry, Skip};
use crate::render::is_blank;

const MAX_DEPTH: usize = 150;

/// The attributes that one batched call fetches for every element. The call returns their
/// values in this order.
#[derive(Clone, Copy)]
enum Attribute {
    Role,
    Subrole,
    Title,
    Value,
    Description,
    Children,
    Position,
    Size,
    Url,
    Loaded,
    TitleElement,
    Header,
    ClassList,
}

impl Attribute {
    const ALL: [Self; 13] = [
        Self::Role,
        Self::Subrole,
        Self::Title,
        Self::Value,
        Self::Description,
        Self::Children,
        Self::Position,
        Self::Size,
        Self::Url,
        Self::Loaded,
        Self::TitleElement,
        Self::Header,
        Self::ClassList,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Role => "AXRole",
            Self::Subrole => "AXSubrole",
            Self::Title => "AXTitle",
            Self::Value => "AXValue",
            Self::Description => "AXDescription",
            Self::Children => "AXChildren",
            Self::Position => "AXPosition",
            Self::Size => "AXSize",
            Self::Url => "AXURL",
            Self::Loaded => "AXLoaded",
            Self::TitleElement => "AXTitleUIElement",
            Self::Header => "AXHeader",
            Self::ClassList => "AXDOMClassList",
        }
    }
}

/// Whether an element with this frame is scrolled out of the area that `clip` describes.
///
/// Chrome clamps an element that is scrolled out of a page to the edge of the page, and gives
/// the element a frame without area. Inside a document, a frame without area is out of view
/// too.
fn is_out_of_view(frame: Option<Frame>, clip: Option<Frame>, context: Context) -> bool {
    let (Some(frame), Some(clip)) = (frame, clip) else {
        return false;
    };
    if frame.has_area() {
        !frame.intersects(clip)
    } else {
        context == Context::Document
    }
}

/// The attributes of one element. The strings stay CoreFoundation strings until a read needs
/// them as text.
struct Raw {
    role: String,
    subrole: Option<CFString>,
    title: Option<CFString>,
    value: Option<CFType>,
    description: Option<CFString>,
    children: Elements,
    frame: Option<Frame>,
    url: Option<CFURL>,
    /// Whether a web area has finished loading. Other elements do not tell.
    loaded: Option<bool>,
    /// The label that names a control, which is an element of its own.
    title_element: Option<Element>,
    /// The group of a table that holds the headings of its columns, or the heading of a
    /// column.
    header: Option<Element>,
    /// Whether the element is the terminal of a web page or a part of one. See
    /// [`quirks::is_web_terminal`].
    web_terminal: bool,
}

impl Raw {
    fn subrole(&self) -> Option<AxStr<'_>> {
        self.subrole.as_ref().map(str_of)
    }

    fn title(&self) -> Option<AxStr<'_>> {
        self.title.as_ref().map(str_of)
    }

    /// The role of the element in `context`, regardless of its parent.
    fn role_in_context(&self, context: Context) -> Role {
        role_of(&self.role, self.subrole().as_deref(), context)
    }

    /// The text of an element with `role`, which only runs of text, labels, and fields have.
    fn text_for(&self, role: Role) -> Option<String> {
        if matches!(role, Role::InlineText | Role::Label | Role::Editable) {
            self.value.as_ref().and_then(string_of)
        } else {
            None
        }
    }

    /// The level of a heading, which its value states.
    fn heading_level(&self) -> Option<u8> {
        self.value
            .as_ref()
            .and_then(CFType::downcast::<CFNumber>)
            .and_then(|n| n.to_i32())
            .and_then(|level| u8::try_from(level).ok())
    }

    /// The name of the element: the text of the label that names it, its title, or its
    /// description.
    ///
    /// An element with children states its text through them, and its title repeats that.
    fn name(&mut self, label: Option<String>) -> String {
        if self.children.entries() > 0 {
            return String::new();
        }
        if let Some(label) = label {
            return label;
        }
        let title = match (self.role.as_str(), self.title.take()) {
            ("AXCell", Some(title)) => quirks::cell_title(string_from(&title)),
            (_, title) => title.as_ref().map(string_from),
        };
        title
            .filter(|title| !title.is_empty())
            .or_else(|| self.description.as_ref().map(string_from))
            .unwrap_or_default()
    }
}

/// The read role of an element in `scope`. `None` leaves the element out: a table lists its
/// cells again under its columns and headers, and only its rows count.
fn role_in(raw: &Raw, scope: Scope) -> Option<Role> {
    if raw.web_terminal {
        return Some(Role::Terminal);
    }
    Some(match (scope.parent, raw.role.as_str()) {
        (Parent::List, "AXGroup") => Role::ListItem,
        (_, "AXImage")
            if scope.context == Context::Application
                && quirks::names_file_on_icon(raw.title().as_deref()) =>
        {
            Role::Label
        }
        (Parent::Table, "AXRow") | (Parent::Other | Parent::List, _) => {
            raw.role_in_context(scope.context)
        }
        (Parent::Table, _) => return None,
    })
}

/// Whether an element is part of a document or part of the application around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    Application,
    Document,
}

/// Where the walk is: the kind of content around an element and the area that is in view.
#[derive(Clone, Copy)]
struct Scope {
    context: Context,
    /// The kind of parent, for the parents whose kind decides the role of a child.
    parent: Parent,
    clip: Option<Frame>,
    depth: usize,
}

impl Scope {
    /// The scope of a window, whose area in view is the window's frame.
    const fn root(clip: Option<Frame>) -> Self {
        Self {
            context: Context::Application,
            parent: Parent::Other,
            clip,
            depth: 0,
        }
    }

    /// The scope of the children of an element with `role` in this scope. A document's children
    /// are part of the document, and lie in its frame.
    fn inside(self, role: Role, raw: &Raw) -> Self {
        let document = role == Role::Document;
        Self {
            context: if document {
                Context::Document
            } else {
                self.context
            },
            parent: Parent::of(&raw.role),
            clip: if document {
                raw.frame.or(self.clip)
            } else {
                self.clip
            },
            depth: self.depth + 1,
        }
    }
}

/// The parents whose children the AX API describes only as plain groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parent {
    Other,
    /// The groups directly inside a list are its items.
    List,
    /// A table lists every cell under its rows, and again under its columns and headers.
    Table,
}

impl Parent {
    fn of(role: &str) -> Self {
        match role {
            "AXList" => Self::List,
            // An outline is a table whose rows can hold more rows, such as Finder's list view.
            "AXTable" | "AXOutline" => Self::Table,
            _ => Self::Other,
        }
    }
}

/// What a read of a window found.
pub struct WindowRead {
    /// The window and the elements in view inside it.
    pub root: Node,
    pub loading: Loading,
    /// The URL of the first document that the read reached.
    pub url: Option<String>,
    /// How many documents the read reached.
    pub documents: usize,
    pub stats: ReadStats,
}

/// Reads windows into trees, within the limits of one read.
pub struct Reader {
    attributes: AttributeNames,
    extent: Extent,
    deadline: Instant,
    max_nodes: usize,
    nodes: usize,
    completeness: Completeness,
    loading: Loading,
    url: Option<String>,
    /// How many documents the walk reached.
    documents: usize,
}

impl Reader {
    /// Reads `window` within `limits`. Returns `None` when the window doesn't answer.
    pub fn read_window(window: &Element, limits: Limits) -> Option<WindowRead> {
        let mut reader = Self::new(limits);
        let raw = reader.fetch(window)?;
        let scope = Scope::root(raw.frame);
        let mut root = reader.node(raw, scope, None).unwrap_or_default();
        root.role = Role::Window;
        Some(WindowRead {
            root,
            loading: reader.loading,
            url: reader.url,
            documents: reader.documents,
            stats: ReadStats {
                nodes: reader.nodes,
                completeness: reader.completeness,
                unknown_roles: 0,
            },
        })
    }

    /// Lists the elements of `window` without their text, as far as `max_entries` elements.
    /// Returns `None` when the window doesn't answer.
    pub fn outline(window: &Element, max_entries: usize) -> Option<Outline> {
        let mut reader = Self::new(Limits {
            max_nodes: max_entries,
            budget: Duration::from_secs(5),
            extent: Extent::Visible,
            ..Limits::default()
        });
        let root = reader.fetch(window)?;
        let scope = Scope::root(root.frame);
        let mut entries = Vec::new();
        let mut stack = vec![(scope, root)];
        while let Some((scope, mut raw)) = stack.pop() {
            let read_role = role_in(&raw, scope);
            let role = read_role.unwrap_or_else(|| raw.role_in_context(scope.context));
            let skipped = if is_out_of_view(raw.frame, scope.clip, scope.context) {
                Some(Skip::OutOfView)
            } else if read_role.is_some_and(Role::is_readable) {
                None
            } else {
                Some(Skip::Unreadable)
            };
            let inner = scope.inside(role, &raw);
            let children = raw.children.entries();
            for child in std::mem::take(&mut raw.children).rev() {
                if let Some(raw) = reader.fetch(&child) {
                    stack.push((inner, raw));
                }
            }
            entries.push(OutlineEntry {
                depth: scope.depth,
                role,
                characters: None,
                frame: raw.frame,
                children,
                skipped,
                native_role: raw.role,
            });
        }
        Some(Outline {
            entries,
            completeness: reader.completeness,
        })
    }

    fn new(limits: Limits) -> Self {
        Self {
            attributes: AttributeNames::new(Attribute::ALL.map(Attribute::name)),
            extent: limits.extent,
            deadline: Instant::now() + limits.budget,
            max_nodes: limits.max_nodes,
            nodes: 0,
            completeness: Completeness::Complete,
            loading: Loading::Finished,
            url: None,
            documents: 0,
        }
    }

    /// Fetches the attributes of `element` in one call.
    fn fetch(&mut self, element: &Element) -> Option<Raw> {
        if self.nodes >= self.max_nodes {
            self.completeness = Completeness::Truncated;
            return None;
        }
        if Instant::now() >= self.deadline {
            self.completeness = Completeness::OutOfTime;
            return None;
        }
        self.nodes += 1;
        let values = element.attributes(&self.attributes)?;
        let value = |attribute: Attribute| values.get(attribute as usize);
        let text = |attribute: Attribute| values.text(attribute as usize);
        let position = value(Attribute::Position).and_then(|v| pair_of(&v, ffi::AX_VALUE_CGPOINT));
        let size = value(Attribute::Size).and_then(|v| pair_of(&v, ffi::AX_VALUE_CGSIZE));
        Some(Raw {
            role: string_from(&text(Attribute::Role)?),
            subrole: text(Attribute::Subrole),
            title: text(Attribute::Title),
            value: value(Attribute::Value),
            description: text(Attribute::Description),
            children: value(Attribute::Children).map(|v| elements_of(&v)).unwrap_or_default(),
            frame: position.zip(size).map(|([x, y], [width, height])| Frame {
                x,
                y,
                width,
                height,
            }),
            url: value(Attribute::Url).and_then(|v| v.downcast::<CFURL>()),
            loaded: value(Attribute::Loaded)
                .and_then(|v| v.downcast::<CFBoolean>())
                .map(bool::from),
            title_element: value(Attribute::TitleElement).and_then(Element::from_value),
            header: value(Attribute::Header).and_then(Element::from_value),
            web_terminal: value(Attribute::ClassList)
                .is_some_and(|classes| quirks::is_web_terminal(&classes)),
        })
    }

    /// Converts a fetched element and its visible descendants. Returns `None` for an element
    /// that is out of view or doesn't carry content.
    ///
    /// `label` is the text of the label that names the element, when the label is an element
    /// of its own.
    fn node(&mut self, mut raw: Raw, scope: Scope, label: Option<String>) -> Option<Node> {
        let whole_document = self.extent == Extent::Document && scope.context == Context::Document;
        if !whole_document && is_out_of_view(raw.frame, scope.clip, scope.context) {
            return None;
        }
        if scope.context == Context::Application {
            if let Some(node) = choice(&mut raw, label.as_deref()) {
                return Some(node);
            }
            // A combo box without a label sits in a format bar, such as TextEdit's font size.
            if raw.role == "AXComboBox" {
                return None;
            }
        }
        let role = role_in(&raw, scope)?;
        if role == Role::Terminal {
            return Some(Node {
                role,
                native_role: raw.role.into(),
                ..Node::default()
            });
        }
        if !role.is_readable() {
            return None;
        }
        let url = raw
            .url
            .take()
            .filter(|_| role == Role::Document)
            .map(|url| string_from(&url.get_string()));
        if role == Role::Document {
            self.enter_document(&raw, url.as_ref());
        }
        let inner = scope.inside(role, &raw);
        let text = raw.text_for(role);
        let children = if text.as_deref().is_none_or(str::is_empty) && inner.depth < MAX_DEPTH {
            self.children(raw.children.clone(), raw.header.as_ref(), inner)
        } else {
            Vec::new()
        };
        Some(Node {
            role,
            mark: mark_of(&raw.role, raw.value.as_ref()),
            heading_level: if role == Role::Heading {
                raw.heading_level()
            } else {
                None
            },
            url,
            text,
            name: raw.name(label),
            children,
            native_role: raw.role.into(),
            ..Node::default()
        })
    }

    /// Notes a document that the walk reached: whether it loads, and its URL when it is the
    /// first.
    fn enter_document(&mut self, raw: &Raw, url: Option<&String>) {
        if raw.loaded == Some(false) {
            self.loading = Loading::InProgress;
        }
        self.documents += 1;
        if self.url.is_none() {
            self.url = url.cloned();
        }
    }

    /// Converts the children of an element, in the order that a person reads them. Pairs
    /// labels with their controls through [`pair_labels`].
    ///
    /// `header` is the group that holds the headings of a table's columns.
    fn children(
        &mut self,
        elements: Elements,
        header: Option<&Element>,
        scope: Scope,
    ) -> Vec<Node> {
        let mut fetched: Vec<Fetched> = elements
            .filter_map(|element| {
                Some(Fetched {
                    raw: self.fetch(&element)?,
                    element,
                    label: None,
                    taken: false,
                })
            })
            .collect();
        pair_labels(&mut fetched);
        let headings = if scope.parent == Parent::Table && scope.context == Context::Application {
            column_headings(&fetched, header)
        } else {
            Vec::new()
        };
        let mut placed = Vec::new();
        for fetched in fetched {
            if fetched.taken {
                continue;
            }
            let frame = fetched.raw.frame;
            // A blank element adds nothing to the text, and its frame would only disturb the
            // reading order. For example, Finder's icon view begins with an empty group as
            // large as the view, whose frame would put every icon on one row. A blank cell
            // keeps the cells after it in their columns.
            if let Some(node) = self.node(fetched.raw, scope, fetched.label)
                && (node.role == Role::Cell || !is_blank(&node))
            {
                placed.push(Placed { node, frame });
            }
        }
        match scope.context {
            Context::Application => in_reading_order(placed, column_header(headings)),
            Context::Document => {
                unwrap_inline_groups(&mut placed);
                restore_word_gaps(&mut placed);
                placed.into_iter().map(|placed| placed.node).collect()
            }
        }
    }
}

/// An element with its attributes.
struct Fetched {
    element: Element,
    raw: Raw,
    /// The text of the label that names the element, when a label among its siblings names
    /// it.
    label: Option<String>,
    /// Whether the element is a label that names a sibling, which leaves it out.
    taken: bool,
}

/// A converted element with the frame that places it among its siblings.
struct Placed {
    node: Node,
    frame: Option<Frame>,
}

/// The label of a control: its text, and the sibling that shows it, when one does.
struct FoundLabel {
    sibling: Option<usize>,
    text: String,
}

/// Pairs the controls among `fetched` siblings with the labels that name them.
///
/// A control that names a label of its own takes the label's text, and the label is left out.
/// For example, the switches of System Settings name labels of their own.
fn pair_labels(fetched: &mut [Fetched]) {
    for index in 0..fetched.len() {
        let raw = &fetched[index].raw;
        if !is_labelled(raw) {
            continue;
        }
        let Some(FoundLabel { sibling, text }) = label_of(raw, fetched) else {
            continue;
        };
        // A control with another name of its own, such as a check box under the heading of
        // its group, keeps its name, and the label stays a line of its own.
        let has_other_name = [&raw.title, &raw.description]
            .into_iter()
            .flatten()
            .map(str_of)
            .find(|name| !name.trim().is_empty())
            .is_some_and(|name| *name != *text);
        if has_other_name {
            continue;
        }
        if let Some(sibling) = sibling {
            fetched[sibling].taken = true;
        }
        fetched[index].label = Some(text);
    }
}

/// Finds the label of the control `raw` among its siblings.
fn label_of(raw: &Raw, fetched: &[Fetched]) -> Option<FoundLabel> {
    let is_label = |raw: &Raw| raw.role == "AXStaticText";
    if let Some(label) = &raw.title_element {
        let sibling =
            fetched.iter().position(|sibling| &sibling.element == label && is_label(&sibling.raw));
        let text = match sibling {
            Some(sibling) => fetched[sibling].raw.value.as_ref().and_then(string_of),
            None => label.string("AXValue"),
        };
        return Some(FoundLabel {
            sibling,
            text: text?,
        });
    }
    // A pop-up button of a save panel repeats the label beside it as its title.
    let title = raw.title().filter(|title| !title.trim().is_empty())?;
    let sibling = fetched.iter().position(|sibling| {
        is_label(&sibling.raw) && sibling.raw.value.as_ref().is_some_and(|v| is_text(v, &title))
    });
    Some(FoundLabel {
        sibling,
        text: (*title).to_owned(),
    })
}

/// The row that names the columns of a table, when it names two or more.
fn column_header(headings: Vec<Node>) -> Option<Node> {
    (headings.len() >= 2).then(|| Node {
        role: Role::Row,
        native_role: "AXHeader".into(),
        children: headings,
        ..Node::default()
    })
}

/// Orders the elements of an application's view as a person reads them, after `header`.
///
/// AppKit lists the elements of a view in the order that they were added, which need not be the
/// order on screen. For example, a save sheet lists its location before its question.
fn in_reading_order(placed: Vec<Placed>, header: Option<Node>) -> Vec<Node> {
    let rects: Vec<_> = placed.iter().map(|placed| placed.frame.map(Frame::on_pixels)).collect();
    let mut slots: Vec<Option<Node>> = placed.into_iter().map(|placed| Some(placed.node)).collect();
    header
        .into_iter()
        .chain(reading_order(&rects).into_iter().filter_map(|index| slots[index].take()))
        .collect()
}

/// Whether `node` is a plain run of text.
fn is_run(node: &Node) -> bool {
    node.role == Role::InlineText && node.children.is_empty()
}

/// Replaces a group that holds one run of text, and that sits beside a run on its line, with
/// that run.
///
/// Firefox wraps an inline block, such as a log's line number, in a group of its own. Such a
/// group is part of the line.
fn unwrap_inline_groups(placed: &mut [Placed]) {
    for index in 0..placed.len() {
        let beside = [index.checked_sub(1), Some(index + 1)].into_iter().flatten();
        let shares_a_line = beside.filter(|&other| other < placed.len()).any(|other| {
            is_run(&placed[other].node)
                && placed[index]
                    .frame
                    .zip(placed[other].frame)
                    .is_some_and(|(frame, other)| frame.shares_line(other))
        });
        let node = &mut placed[index].node;
        if shares_a_line
            && node.role == Role::Container
            && node.children.len() == 1
            && is_run(&node.children[0])
        {
            *node = node.children.remove(0);
        }
    }
}

/// Puts a blank between two runs of text where the page shows room between them.
///
/// WebKit gives every run of text an element, without the blanks between the runs that a page
/// shows, such as the gap after a log's line number.
fn restore_word_gaps(placed: &mut [Placed]) {
    for index in 1..placed.len() {
        let (before, after) = placed.split_at_mut(index);
        let (previous, next) = (&before[index - 1], &mut after[0]);
        if let (Some(previous_frame), Some(next_frame)) = (previous.frame, next.frame)
            && previous.node.role == Role::InlineText
            && next.node.role == Role::InlineText
            && previous_frame.leaves_room_for(next_frame)
            && previous
                .node
                .text
                .as_deref()
                .is_some_and(|text| !text.ends_with(char::is_whitespace))
            && let Some(text) = &mut next.node.text
            && !text.starts_with(char::is_whitespace)
        {
            text.insert(0, ' ');
        }
    }
}

/// Returns the headings of the columns of a table, from its children and its header.
///
/// The columns name their headings, as in Activity Monitor, or the table's header holds them,
/// as in Finder's list view.
fn column_headings(children: &[Fetched], header: Option<&Element>) -> Vec<Node> {
    let mut names: Vec<String> = children
        .iter()
        .filter(|child| child.raw.role == "AXColumn")
        .filter_map(|child| child.raw.header.as_ref()?.string("AXTitle"))
        .collect();
    if names.is_empty()
        && let Some(header) = header
    {
        names = header
            .elements("AXChildren")
            .filter_map(|heading| {
                heading
                    .string("AXTitle")
                    .filter(|title| !title.trim().is_empty())
                    .or_else(|| heading.string("AXDescription"))
            })
            .collect();
    }
    names
        .into_iter()
        .filter_map(quirks::cell_title)
        .filter(|name| !name.trim().is_empty())
        .map(|name| Node {
            role: Role::ColumnHeader,
            native_role: "AXColumn".into(),
            name,
            ..Node::default()
        })
        .collect()
}

/// Whether the element is a control that a label beside it names.
fn is_labelled(raw: &Raw) -> bool {
    matches!(
        raw.role.as_str(),
        "AXCheckBox" | "AXRadioButton" | "AXPopUpButton" | "AXTextField" | "AXComboBox"
    ) && raw.subrole().as_deref() != Some("AXSecureTextField")
}

/// Returns the mark of a check box or a radio button, which states whether it is set as the
/// number 1 or 0.
fn mark_of(role: &str, value: Option<&CFType>) -> Mark {
    if !matches!(role, "AXCheckBox" | "AXRadioButton") {
        return Mark::None;
    }
    match value.and_then(CFType::downcast::<CFNumber>).and_then(|n| n.to_i32()) {
        Some(0) => Mark::Unchecked,
        Some(_) => Mark::Checked,
        None => Mark::None,
    }
}

/// Reads a pop-up button or a one-line field that a label names as the label and the value,
/// such as "Dock position on screen: Bottom" or "Save As: Untitled".
///
/// A pop-up button without a label is a control of a toolbar or a format bar, such as a font
/// menu, and doesn't carry content. A field without a label keeps its role.
fn choice(raw: &mut Raw, label: Option<&str>) -> Option<Node> {
    let field = match raw.role.as_str() {
        "AXPopUpButton" => false,
        "AXTextField" | "AXComboBox" if raw.subrole().as_deref() != Some("AXSecureTextField") => {
            true
        }
        _ => return None,
    };
    let title = raw.title();
    let label = if field {
        label
    } else {
        label.or(title.as_deref())
    };
    let label = label.map(str::trim).filter(|label| !label.is_empty())?;
    let value = raw.value.as_ref().and_then(|value| value.downcast::<CFString>());
    let value = value.as_ref().map(str_of);
    let value = value.as_deref().unwrap_or_default().trim();
    // A pop-up button without a value is a menu, such as Chrome's tab search.
    if !field && value.is_empty() {
        return None;
    }
    let separator = match (label.ends_with(':'), value.is_empty()) {
        (_, true) => "",
        (true, false) => " ",
        (false, false) => ": ",
    };
    let name = format!("{label}{separator}{value}");
    // The title borrows `raw`, which gives up its role.
    drop(title);
    Some(Node {
        role: Role::Label,
        native_role: std::mem::take(&mut raw.role).into(),
        name,
        ..Node::default()
    })
}

fn role_of(role: &str, subrole: Option<&str>, context: Context) -> Role {
    if subrole == Some("AXSecureTextField") {
        return Role::Secret;
    }
    // A segment of a segmented control is a button of a format bar or a view switcher,
    // regardless of what its role says.
    if subrole == Some("AXSegment") {
        return Role::Controls;
    }
    match role {
        "AXWindow" | "AXSheet" | "AXDrawer" => Role::Window,
        "AXWebArea" => Role::Document,
        "AXHeading" => Role::Heading,
        "AXStaticText" if context == Context::Document => Role::InlineText,
        // Chrome wraps inline markup such as `<code>` in a group of its own.
        "AXGroup" if subrole.is_some_and(|subrole| subrole.ends_with("StyleGroup")) => {
            Role::InlineText
        }
        "AXStaticText" | "AXCheckBox" | "AXRadioButton" => Role::Label,
        "AXLink" => Role::Link,
        "AXTextArea" | "AXTextField" | "AXComboBox" => Role::Editable,
        "AXRow" => Role::Row,
        "AXCell" => Role::Cell,
        "AXMenuBar"
        | "AXMenuBarItem"
        | "AXMenu"
        | "AXMenuItem"
        | "AXMenuButton"
        | "AXToolbar"
        | "AXButton"
        | "AXPopUpButton"
        | "AXScrollBar"
        | "AXImage"
        | "AXSlider"
        | "AXIncrementor"
        | "AXSplitter"
        | "AXValueIndicator"
        | "AXProgressIndicator"
        | "AXBusyIndicator"
        | "AXColorWell"
        | "AXDisclosureTriangle"
        | "AXGrowArea"
        | "AXHandle"
        | "AXRuler"
        | "AXRulerMarker"
        | "AXDockItem"
        // The renderer writes its own list markers.
        | "AXListMarker" => Role::Controls,
        _ => Role::Container,
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const WINDOW: Frame = Frame {
        x: 100.0,
        y: 100.0,
        width: 800.0,
        height: 600.0,
    };

    #[rstest]
    #[case::inside(200.0, 200.0, true)]
    #[case::straddles_the_top(200.0, 90.0, true)]
    #[case::scrolled_below(200.0, 700.0, false)]
    #[case::scrolled_above(200.0, 50.0, false)]
    #[case::touching_the_edge(900.0, 200.0, false)]
    fn elements_outside_the_window_are_out_of_view(
        #[case] x: f64,
        #[case] y: f64,
        #[case] visible: bool,
    ) {
        let element = Frame {
            x,
            y,
            width: 50.0,
            height: 20.0,
        };
        assert_eq!(element.intersects(WINDOW), visible);
    }

    #[rstest]
    #[case::clamped_in_a_page(0.0, Context::Document, true)]
    #[case::empty_wrapper_in_app_chrome(0.0, Context::Application, false)]
    #[case::visible_in_a_page(20.0, Context::Document, false)]
    fn frames_without_area_are_out_of_view_only_in_documents(
        #[case] height: f64,
        #[case] context: Context,
        #[case] out_of_view: bool,
    ) {
        let frame = Frame {
            x: 150.0,
            y: 400.0,
            width: 500.0,
            height,
        };
        assert_eq!(is_out_of_view(Some(frame), Some(WINDOW), context), out_of_view);
    }

    #[rstest]
    #[case("AXTextField", Some("AXSecureTextField"), Context::Application, Role::Secret)]
    #[case("AXStaticText", None, Context::Document, Role::InlineText)]
    #[case("AXStaticText", None, Context::Application, Role::Label)]
    #[case("AXWebArea", None, Context::Application, Role::Document)]
    #[case("AXMenuBar", None, Context::Application, Role::Controls)]
    #[case("AXCheckBox", Some("AXSegment"), Context::Application, Role::Controls)]
    #[case("AXCheckBox", None, Context::Application, Role::Label)]
    #[case("AXGroup", Some("AXCodeStyleGroup"), Context::Document, Role::InlineText)]
    #[case("AXListMarker", None, Context::Document, Role::Controls)]
    #[case("AXGroup", None, Context::Document, Role::Container)]
    fn ax_roles_map_to_roles(
        #[case] role: &str,
        #[case] subrole: Option<&str>,
        #[case] context: Context,
        #[case] expected: Role,
    ) {
        assert_eq!(role_of(role, subrole, context), expected);
    }
}

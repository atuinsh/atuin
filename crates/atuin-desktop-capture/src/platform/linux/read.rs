//! Reads a window into a tree of [`Node`]s, element by element and level by level.
//!
//! A document inside the window is read from one listing of its elements when it can be, which
//! [`listing`] does.

mod budget;
mod fetch;
mod listing;

use std::collections::HashSet;
use std::ops::Range;
use std::pin::pin;

use futures::future::{self, Either};
use futures::stream::{self, StreamExt as _};
use tokio::time::Instant;

use self::budget::ListingBudget;
use self::fetch::{Indexed, in_order, optional};
use self::listing::Listing;
use super::apps::{Profile, Profiles, ToolkitLookup, Waiter};
use super::bus::{
    App, Bus, IN_FLIGHT, Object, STATE_BUSY, STATE_CHECKED, STATE_SHOWING, STATE_VISIBLE, Toolkit,
    has_state,
};
use super::normalize::normalize;
use super::private::{self, PrivateMarking, Subject};
use super::quirks;
use super::roles::{AtspiRole, Within};
use super::unlisted::Unlisted;
use crate::backend::{
    Completeness, Error, Extent, Limits, Loading, Mark, Node, PrivateWindows, ReadStats, Reading,
    Role, Visibility, WindowKind,
};
use crate::encoding;
use crate::layout::{Rect, reading_order};
use crate::urls::is_visitable;

/// How deep the tree of a read may nest, with the window at depth 0.
///
/// [`normalize`] adds a row element below each table that has none, which deepens the tree,
/// so the read keeps half of the decoder's depth in reserve.
pub const MAX_TREE_DEPTH: usize = encoding::MAX_DEPTH / 2;

/// The document that a window points at.
struct Embedded {
    url: Option<String>,
}

/// What a walk of a window found.
struct Walked {
    reading: Reading,
    /// The URL of every showing document, in the order of the walk.
    document_urls: Vec<String>,
    /// How many showing documents the window holds.
    documents: usize,
    /// Whether a dialog or an alert covers the window. See [`quirks::dialog_covers_page`].
    dialog_covers_page: bool,
}

/// Picks the URL that a window reports from the URLs of its documents.
///
/// A browser window can hold the page next to developer tools or a side panel, which have
/// URLs of the browser's own. Chromium points the window at whichever of them had the focus
/// last. A URL that a person can visit wins: first the embedded document's, then the first
/// in the walk. Without one, the same order decides among the rest.
fn page_url(embedded: Option<String>, walked: Vec<String>) -> Option<String> {
    let mut candidates = embedded.into_iter().chain(walked);
    let mut fallback = None;
    for url in candidates.by_ref() {
        if is_visitable(&url) {
            return Some(url);
        }
        fallback.get_or_insert(url);
    }
    fallback
}

/// Whether the application of a window may still be building the tree of a page. See
/// [`quirks::PAGE_BUILD_TIME`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageBuild {
    MayBeBuilding,
    Done,
}

/// Turns what the walk of a window found into the window's reading. `embedded` is the document
/// that the window points at.
fn classify(walked: Walked, embedded: Option<Embedded>, pages: PageBuild) -> Reading {
    let Walked {
        mut reading,
        document_urls,
        documents,
        dialog_covers_page,
    } = walked;
    normalize(&mut reading.root);
    // A window can name a document that the walk did not meet. Such a window is still
    // building the tree of that document. Some applications have not even named the document yet.
    let expects_a_document = embedded.is_some() || pages == PageBuild::MayBeBuilding;
    if expects_a_document && documents == 0 && !dialog_covers_page {
        reading.loading = Loading::InProgress;
    }
    let embedded = embedded.and_then(|embedded| embedded.url);
    // A browser's window can hold a panel of the browser's interface beside its page, so
    // only a window with nothing else is part of the interface.
    let mut urls = embedded.iter().chain(&document_urls).peekable();
    if urls.peek().is_some() && urls.all(|url| quirks::is_browser_ui(url)) {
        reading.kind = WindowKind::BrowserUi;
    }
    reading.url = page_url(embedded, document_urls);
    reading
}

/// The toolkit that `lookup` found, which tells whether a read needs a check of its privacy
/// when private windows are read as `reads`.
///
/// Fails when the lookup failed and the check might be needed. A read that records private
/// windows doesn't need the check, so the read goes on without the toolkit.
fn toolkit_for_check(
    lookup: &ToolkitLookup,
    reads: PrivateWindows,
) -> Result<Option<&Toolkit>, Error> {
    match lookup {
        ToolkitLookup::Named(toolkit) => Ok(Some(toolkit)),
        ToolkitLookup::Unnamed => Ok(None),
        ToolkitLookup::Failed => match reads {
            PrivateWindows::Exclude => Err(Error::NoAnswer),
            PrivateWindows::Capture => Ok(None),
        },
    }
}

/// Why an element did not tell its role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleFailure {
    /// The application took longer than the timeout of a call, as a busy browser does. The
    /// same call can succeed later.
    TimedOut,
    /// The call failed, for example because the element is gone.
    Failed,
}

impl From<Error> for RoleFailure {
    fn from(error: Error) -> Self {
        match error {
            Error::NoAnswer => Self::TimedOut,
            _ => Self::Failed,
        }
    }
}

/// Reads the answer to a call for the role of an element.
pub fn answered_role(answer: Result<AtspiRole, Error>) -> Result<AtspiRole, RoleFailure> {
    answer.map_err(RoleFailure::from)
}

/// Whether `object`, an element of a document whose role
/// [`quirks::may_be_web_terminal`] accepts, is the terminal of a web page or a part of one.
///
/// Fails like a call for the role when the element doesn't tell its classes, because the
/// element could be a terminal.
pub async fn is_web_terminal(bus: &Bus, object: &Object) -> Result<bool, RoleFailure> {
    let matches =
        bus.attribute_matches(object, "class", |class| class.is_some_and(quirks::is_web_terminal));
    Ok(matches.await?)
}

/// The elements of a read that did not tell their role. The read leaves each of them out with
/// everything below it, because such an element could be a password field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnknownRoles {
    pub count: usize,
    /// Whether any of the calls for the roles timed out, so that the read can find more later.
    pub timed_out: bool,
}

impl UnknownRoles {
    /// Counts the element whose role call failed with `failure`.
    pub fn add(&mut self, failure: RoleFailure) {
        self.count += 1;
        self.timed_out |= failure == RoleFailure::TimedOut;
    }

    fn merge(&mut self, other: Self) {
        self.count += other.count;
        self.timed_out |= other.timed_out;
    }

    /// The completeness of a read that would be `completeness` without these elements.
    ///
    /// A read in which a role call timed out ran out of time for that element.
    fn completeness(self, completeness: Completeness) -> Completeness {
        if self.timed_out {
            Completeness::OutOfTime
        } else {
            completeness
        }
    }
}

/// An element whose children are still object references.
struct Fetched {
    node: Node,
    /// The role of the element, when the element answered for it.
    atspi: Option<AtspiRole>,
    children: Vec<Object>,
    url: Option<String>,
    /// Whether the element is a showing document that is still loading.
    loading: Loading,
    /// Where the element is on screen, when the element is a showing terminal.
    terminal: Option<Rect>,
    /// How many elements below this one are already part of `node`, because the element is a
    /// document that was read from a listing.
    listed: usize,
    /// Where the element is on screen. Only elements outside documents are located.
    rect: Option<Rect>,
    /// Where the children of the element lie.
    within: Within,
    /// The element, and the elements of its listing, that did not tell their role.
    unknown_roles: UnknownRoles,
}

impl Fetched {
    /// Takes the URL of a document, which the walk reports and the node keeps.
    fn set_url(&mut self, url: Option<String>) {
        self.node.url.clone_from(&url);
        self.url = url;
    }
}

/// Which elements count as showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShowingRule {
    /// The elements whose state says that they are showing.
    ByState,
    /// Every element that answers for its state, as inside a document that is read whole.
    /// Inside a document, "not showing" mostly means "scrolled out of view".
    Everything,
}

/// What the state bits of an element say about how the element is read.
struct Seen {
    showing: bool,
    /// Whether the element reports that it is busy, as a document does while it loads.
    busy: bool,
}

impl Seen {
    /// Reads the state bits of an element with the role `atspi`.
    fn of(state: Option<&[u32]>, atspi: Option<AtspiRole>, rule: ShowingRule) -> Self {
        let visible_counts = atspi.is_some_and(quirks::shows_without_the_state);
        let showing = state.is_some_and(|state| {
            rule == ShowingRule::Everything
                || has_state(state, STATE_SHOWING)
                || (visible_counts && has_state(state, STATE_VISIBLE))
        });
        Self {
            busy: state.is_some_and(|state| has_state(state, STATE_BUSY)),
            showing,
        }
    }
}

/// What an element is.
#[derive(Clone, Copy)]
struct Identity {
    atspi: AtspiRole,
    /// Whether the element is the terminal of a web page or a part of one. See
    /// [`is_web_terminal`].
    web_terminal: bool,
}

/// An element that waits to be fetched, and where it hangs in the tree.
struct Queued {
    origin: Origin,
    object: Object,
    /// The role of the element, when a call before the walk already found it.
    known_role: Option<AtspiRole>,
}

/// A fetched element, and where it hangs in the tree.
struct Placed {
    origin: Origin,
    fetched: Fetched,
}

/// The elements that a walk fetched, each before its children.
#[derive(Default)]
struct Arena {
    nodes: Vec<Node>,
    /// The index of the parent of each element.
    parents: Vec<usize>,
    /// Where each element is on screen, when it was located.
    rects: Vec<Option<Rect>>,
    /// Whether the children of each element lie outside every document.
    in_window: Vec<bool>,
    /// Whether each element is one that GTK 4 leaves out of the tree. See
    /// [`quirks::drop_repeated_text`].
    unlisted: Vec<bool>,
}

impl Arena {
    fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Adds `fetched` under the element at `parent`. `inside` is where its children lie.
    fn push(&mut self, parent: usize, fetched: Fetched, inside: Place) {
        self.nodes.push(fetched.node);
        self.parents.push(parent);
        self.rects.push(fetched.rect);
        self.in_window.push(inside == Place::Window);
        self.unlisted.push(fetched.atspi == Some(AtspiRole::INVALID));
    }

    /// The area that the window, the first element, covers on screen.
    fn window_area(&self) -> i64 {
        self.rects.first().copied().flatten().map_or(0, Rect::area)
    }

    /// Hangs every element under its parent, and returns the window with its tree.
    ///
    /// Outside documents, the children follow their reading order: toolkits report children
    /// in creation order, which GTK reverses in a grid.
    fn into_tree(self) -> Node {
        let Self {
            mut nodes,
            parents,
            rects,
            in_window,
            unlisted,
        } = self;
        // The walk fetches level by level and keeps the order of each level, so the children
        // of each element lie next to each other in the arena.
        let mut child_ranges: Vec<Range<usize>> = vec![0..0; nodes.len()];
        for (index, &parent) in parents.iter().enumerate().skip(1) {
            let range = &mut child_ranges[parent];
            if range.end != index {
                debug_assert_eq!(range.end, 0, "the children of an element lie together");
                range.start = index;
            }
            range.end = index + 1;
        }
        // Children always follow their parent, so walking from the back completes each element
        // before it moves into its parent.
        for index in (0..nodes.len()).rev() {
            let children = child_ranges[index].clone();
            // A document from a listing brought its children along and has none in the arena.
            if children.len() >= 2 && in_window[index] {
                let order = reading_order(&rects[children.clone()]);
                nodes[index].children = order
                    .into_iter()
                    .map(|i| std::mem::take(&mut nodes[children.start + i]))
                    .collect();
            } else if !children.is_empty() {
                nodes[index].children = children.map(|c| std::mem::take(&mut nodes[c])).collect();
            }
            if unlisted[index] {
                quirks::drop_repeated_text(&mut nodes[index]);
            }
        }
        nodes.into_iter().next().unwrap_or_default()
    }
}

/// What a walk counts across the elements that it fetched.
#[derive(Default)]
struct WalkTotals {
    /// The URL of every showing document, in the order of the walk.
    document_urls: Vec<String>,
    /// How many showing documents the window holds.
    documents: usize,
    /// The largest area that a showing terminal covers.
    terminal_area: i64,
    loading: Loading,
    /// How many elements came from listings of documents, on top of the arena.
    listed: usize,
    /// Whether a showing child of the window covers the page. See
    /// [`quirks::dialog_covers_page`].
    dialog_covers_page: bool,
    unknown_roles: UnknownRoles,
}

impl WalkTotals {
    /// Counts `fetched`, which hangs at `origin`, and takes its URL.
    fn record(&mut self, origin: Origin, fetched: &mut Fetched) {
        let showing = fetched.node.visibility == Visibility::Showing;
        if fetched.node.role == Role::Document && showing {
            self.documents += 1;
        }
        if origin.depth == 1 && showing && fetched.atspi.is_some_and(quirks::dialog_covers_page) {
            self.dialog_covers_page = true;
        }
        self.document_urls.extend(fetched.url.take());
        self.terminal_area = self.terminal_area.max(fetched.terminal.map_or(0, Rect::area));
        self.listed += fetched.listed;
        self.unknown_roles.merge(fetched.unknown_roles);
        if fetched.loading == Loading::InProgress {
            self.loading = Loading::InProgress;
        }
    }

    /// Whether the window is a terminal, which keeps out the title of a terminal emulator that
    /// capture doesn't know by name. Such a title usually names the running command.
    ///
    /// A window counts as a terminal when a terminal covers at least half of it. An editor
    /// with a terminal panel stays an editor, and the read leaves out only the panel's content.
    fn window_kind(&self, window_area: i64) -> WindowKind {
        if self.terminal_area > 0 && self.terminal_area.saturating_mul(2) >= window_area {
            WindowKind::Terminal
        } else {
            WindowKind::Other
        }
    }
}

/// The mark of a check box or a radio button with these state bits.
pub fn option_mark(state: &[u32]) -> Mark {
    if has_state(state, STATE_CHECKED) {
        Mark::Checked
    } else {
        Mark::Unchecked
    }
}

/// Whether an element lies inside a document or in the window around it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Window,
    Document,
}

/// Whether the options below an element report their state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionState {
    Reported,
    /// The options lie in a GTK 3 popover. See [`quirks::hides_popover_option_state`].
    Unknown,
}

/// What the application of a window changes about how a walk reads the window's elements.
struct Adjustments {
    /// Whether the options in the window's popovers report their state.
    popovers: OptionState,
    /// The elements that the application leaves out of the tree. See
    /// [`quirks::leaves_roleless_elements_out`].
    unlisted: Unlisted,
    /// Whether an element without height is hidden. See [`quirks::hides_by_height`].
    hides_by_height: bool,
}

impl Adjustments {
    /// The adjustments for the windows of `app`, whose profile is `profile`.
    async fn of(bus: &Bus, app: &App, profile: &Profile) -> Self {
        let toolkit = profile.toolkit.named();
        let popovers = if toolkit.is_some_and(quirks::hides_popover_option_state) {
            OptionState::Unknown
        } else {
            OptionState::Reported
        };
        let unlisted = if toolkit.is_some_and(quirks::leaves_roleless_elements_out) {
            bus.unlisted(app).await
        } else {
            Unlisted::default()
        };
        Self {
            popovers,
            unlisted,
            hides_by_height: quirks::hides_by_height(profile),
        }
    }
}

/// Where a queued element hangs in the tree that the reader builds.
#[derive(Clone, Copy)]
struct Origin {
    parent: usize,
    /// How far below the window the element hangs.
    depth: usize,
    place: Place,
    within: Within,
    option_state: OptionState,
}

impl Origin {
    /// The origin of the element that a walk starts from.
    const fn root(place: Place) -> Self {
        Self {
            parent: usize::MAX,
            depth: 0,
            place,
            within: match place {
                Place::Window => Within::Window,
                Place::Document => Within::Document,
            },
            option_state: OptionState::Reported,
        }
    }

    /// The origin of the children of `fetched`, which hangs here at the index `parent`.
    fn inside(self, parent: usize, fetched: &Fetched) -> Self {
        Self {
            parent,
            depth: self.depth + 1,
            place: if fetched.node.role == Role::Document {
                Place::Document
            } else {
                self.place
            },
            within: fetched.within,
            option_state: self.option_state,
        }
    }
}

pub struct Reader<'a> {
    bus: &'a Bus,
    deadline: Instant,
    max_nodes: usize,
    extent: Extent,
    private_windows: PrivateWindows,
}

impl<'a> Reader<'a> {
    pub fn new(bus: &'a Bus, limits: Limits) -> Self {
        Self {
            bus,
            deadline: Instant::now() + limits.budget,
            max_nodes: limits.max_nodes,
            extent: limits.extent,
            private_windows: limits.private_windows,
        }
    }
}

impl Reader<'_> {
    /// Reads `window`. `profiles` tells what the application of the window is, and `private`
    /// tells whether the window is a private browser window.
    ///
    /// Fails when the application doesn't tell its toolkit, or when the check of a private
    /// window can't finish.
    pub async fn read(
        &self,
        window: &Object,
        profiles: &Profiles,
        private: &private::PrivateWindows,
    ) -> Result<Reading, Error> {
        let (profile, role) = future::join(
            profiles.profile(self.bus, window.app(), Waiter::Call),
            self.bus.role(window),
        )
        .await;
        let toolkit = toolkit_for_check(&profile.toolkit, self.private_windows)?;
        let role = role.ok();
        // A hidden document, such as a background tab, is read as a whole.
        let place = match role.map(AtspiRole::role) {
            Some(Role::Document) => Place::Document,
            _ => Place::Window,
        };
        // The check runs beside the walk, and a private window ends the walk.
        let check = pin!(self.is_private(window, Subject::of(role), toolkit, private));
        let content = pin!(self.read_content(window, place, role, &profile, profiles));
        let (is_private, reading) = match future::select(check, content).await {
            Either::Left((is_private, content)) => {
                if is_private? {
                    return Ok(Reading::of_private_window());
                }
                (false, content.await?)
            }
            Either::Right((reading, check)) => {
                let reading = reading?;
                (check.await?, reading)
            }
        };
        Ok(if is_private {
            Reading::of_private_window()
        } else {
            reading
        })
    }

    /// Whether `window`, which a read starts from as `subject`, is part of a private window
    /// whose content the read leaves out.
    async fn is_private(
        &self,
        window: &Object,
        subject: Subject,
        toolkit: Option<&Toolkit>,
        private: &private::PrivateWindows,
    ) -> Result<bool, Error> {
        let marking = toolkit
            .and_then(PrivateMarking::of)
            .filter(|_| self.private_windows == PrivateWindows::Exclude);
        let Some(marking) = marking else {
            return Ok(false);
        };
        private.is_private(self.bus, window, subject, marking).await
    }

    /// Reads the content of `window`, which a read starts from at `place`. `profile` is the
    /// profile of the window's application.
    async fn read_content(
        &self,
        window: &Object,
        place: Place,
        role: Option<AtspiRole>,
        profile: &Profile,
        profiles: &Profiles,
    ) -> Result<Reading, Error> {
        // The window points at one of its documents. The walk reports the URL of every
        // document that it reads, and `page_url` picks the one to report.
        let embedded = async {
            profiles.asks_for_relations(window.app());
            let embedded = self.bus.embedded_document(window).await?;
            Some(Embedded {
                url: self.bus.document_url(&embedded).await,
            })
        };
        let adjustments = Adjustments::of(self.bus, window.app(), profile).await;
        let root = Queued {
            origin: Origin::root(place),
            object: window.clone(),
            known_role: role,
        };
        let (walked, embedded) =
            future::join(self.walk(window, root, &adjustments), embedded).await;
        let pages = if profiles.may_be_building_pages(window.app()) {
            PageBuild::MayBeBuilding
        } else {
            PageBuild::Done
        };
        Ok(classify(walked?, embedded, pages))
    }

    /// Reads `window` element by element, level by level, starting from `root`.
    async fn walk(
        &self,
        window: &Object,
        root: Queued,
        adjustments: &Adjustments,
    ) -> Result<Walked, Error> {
        let mut arena = Arena::default();
        let mut totals = WalkTotals::default();
        let mut completeness = Completeness::Complete;
        // An application can list an element below itself, and the walk would never end.
        let mut visited = HashSet::from([root.object.clone()]);
        let mut frontier = vec![root];
        while !frontier.is_empty() {
            let queried = arena.len() + totals.listed + frontier.len();
            if queried > self.max_nodes {
                completeness = Completeness::Truncated;
                break;
            }
            if Instant::now() >= self.deadline {
                completeness = Completeness::OutOfTime;
                break;
            }
            let budget = ListingBudget::new(self.max_nodes - queried);
            let level = std::mem::take(&mut frontier);
            for Placed {
                origin,
                mut fetched,
            } in self.fetch_level(window, level, adjustments, &budget).await
            {
                totals.record(origin, &mut fetched);
                let inside = origin.inside(arena.len(), &fetched);
                let children = std::mem::take(&mut fetched.children);
                arena.push(origin.parent, fetched, inside.place);
                if !children.is_empty() && inside.depth >= MAX_TREE_DEPTH {
                    completeness = Completeness::Truncated;
                    continue;
                }
                let unvisited = children.into_iter().filter(|child| visited.insert(child.clone()));
                frontier.extend(unvisited.map(|object| Queued {
                    origin: inside,
                    object,
                    known_role: None,
                }));
            }
        }
        let nodes = arena.len() + totals.listed;
        let kind = totals.window_kind(arena.window_area());
        let unknown_roles = totals.unknown_roles;
        if unknown_roles.count > 0 {
            tracing::debug!(
                elements = unknown_roles.count,
                timed_out = unknown_roles.timed_out,
                "elements of a window did not tell their role"
            );
        }
        Ok(Walked {
            reading: Reading {
                root: arena.into_tree(),
                kind,
                loading: totals.loading,
                url: None,
                stats: ReadStats {
                    nodes,
                    completeness: unknown_roles.completeness(completeness),
                    unknown_roles: unknown_roles.count,
                },
            },
            document_urls: totals.document_urls,
            documents: totals.documents,
            dialog_covers_page: totals.dialog_covers_page,
        })
    }

    /// Fetches one level of the walk, many elements at a time. The listings of documents in
    /// the level share `budget`.
    ///
    /// The results come back in the order of `level`. A slow element, such as a large document,
    /// doesn't hold up the fetches of the elements after it.
    async fn fetch_level(
        &self,
        window: &Object,
        level: Vec<Queued>,
        adjustments: &Adjustments,
        budget: &ListingBudget,
    ) -> Vec<Placed> {
        let placed = stream::iter(level.into_iter().enumerate())
            .map(|(index, queued)| async move {
                let origin =
                    if queued.origin.parent == 0 && adjustments.popovers == OptionState::Unknown {
                        self.find_popover(window, &queued.object, queued.origin).await
                    } else {
                        queued.origin
                    };
                Indexed {
                    index,
                    value: Placed {
                        origin,
                        fetched: self.fetch(&queued, origin, adjustments, budget).await,
                    },
                }
            })
            .buffer_unordered(IN_FLIGHT)
            .collect()
            .await;
        in_order(placed)
    }

    /// Marks `origin` as lying in a popover when `object`, a child of `window`, is a popover
    /// that GTK 3 lists among the children of the window. The popover's parent is the element
    /// that opened the popover, and every other child has the window as its parent.
    async fn find_popover(&self, window: &Object, object: &Object, origin: Origin) -> Origin {
        match self.bus.parent(object).await {
            Ok(parent) if parent != *window => Origin {
                option_state: OptionState::Unknown,
                ..origin
            },
            _ => origin,
        }
    }

    /// Fetches one element. An element that fails to answer is treated as hidden, and an
    /// element that does not tell its role is left empty and counted.
    async fn fetch(
        &self,
        queued: &Queued,
        origin: Origin,
        adjustments: &Adjustments,
        budget: &ListingBudget,
    ) -> Fetched {
        let object = &queued.object;
        let Origin { place, within, .. } = origin;
        let (state, answer) =
            future::join(self.bus.state(object), self.identify(queued, place)).await;
        let mut unknown_roles = UnknownRoles::default();
        let identity = answer.inspect_err(|&failure| unknown_roles.add(failure)).ok();
        let atspi = identity.map(|identity| identity.atspi);
        let role = match identity {
            Some(Identity {
                web_terminal: true, ..
            }) => Role::Terminal,
            Some(identity) => within.role(identity.atspi),
            None => Role::Container,
        };
        let is_option = atspi.is_some_and(|atspi| atspi.is_option(within));
        let rule = if self.extent == Extent::Document && place == Place::Document {
            ShowingRule::Everything
        } else {
            ShowingRule::ByState
        };
        let state = state.ok();
        let seen = Seen::of(state.as_deref(), atspi, rule);
        let mut fetched = Fetched {
            node: Node {
                role,
                native_role: atspi.map(AtspiRole::native_name).unwrap_or_default(),
                mark: match state.as_deref() {
                    Some(_) if is_option && origin.option_state == OptionState::Unknown => {
                        Mark::None
                    }
                    Some(state) if is_option => option_mark(state),
                    _ => Mark::None,
                },
                visibility: if seen.showing {
                    Visibility::Showing
                } else {
                    Visibility::Hidden
                },
                ..Node::default()
            },
            atspi,
            children: Vec::new(),
            url: None,
            loading: Loading::Finished,
            terminal: None,
            listed: 0,
            rect: None,
            within: atspi.map_or(within, |atspi| within.inner(atspi)),
            unknown_roles,
        };
        if seen.showing && atspi == Some(AtspiRole::TERMINAL) {
            fetched.terminal = self.bus.extents(object).await;
        }
        // An element whose role is unknown could be a password field.
        let Some(atspi) = atspi.filter(|_| seen.showing && role.is_readable()) else {
            return fetched;
        };
        if fetched.within == Within::Button {
            // Only the children of a button can be read, because one of them can be a menu.
            fetched.children = self.bus.children(object).await.unwrap_or_default();
            return fetched;
        }
        if role == Role::Document && seen.busy {
            fetched.loading = Loading::InProgress;
        }
        if role == Role::Document
            && self.extent == Extent::Document
            && let Some(listed) = self.listing(origin, budget).read_document(object, atspi).await
        {
            let (url, rect) =
                future::join(self.bus.document_url(object), self.locate(object, place)).await;
            fetched.node = listed.document;
            fetched.listed = listed.nodes - 1;
            fetched.unknown_roles.merge(listed.unknown_roles);
            fetched.rect = rect;
            fetched.set_url(url);
            return fetched;
        }
        self.fetch_details(object, origin, adjustments, &mut fetched).await;
        fetched
    }

    /// Finds what the element of `queued`, which lies at `place`, is.
    async fn identify(&self, queued: &Queued, place: Place) -> Result<Identity, RoleFailure> {
        let atspi = match queued.known_role {
            Some(role) => role,
            None => answered_role(self.bus.role(&queued.object).await)?,
        };
        let web_terminal = if place == Place::Document && quirks::may_be_web_terminal(atspi) {
            is_web_terminal(self.bus, &queued.object).await?
        } else {
            false
        };
        Ok(Identity {
            atspi,
            web_terminal,
        })
    }

    /// The listing of a document that hangs at `origin`, within `budget`.
    fn listing<'a>(&'a self, origin: Origin, budget: &'a ListingBudget) -> Listing<'a> {
        Listing::new(self.bus, self.deadline, budget, MAX_TREE_DEPTH.saturating_sub(origin.depth))
    }

    /// Fetches the name, the text, and the children of a readable element that isn't a listed
    /// document.
    async fn fetch_details(
        &self,
        object: &Object,
        origin: Origin,
        adjustments: &Adjustments,
        fetched: &mut Fetched,
    ) {
        let role = fetched.node.role;
        let is_option = fetched.atspi.is_some_and(|atspi| atspi.is_option(origin.within));
        let ((name, children, text, heading_level, url), rect) = future::join(
            future::join5(
                self.bus.name(object),
                self.bus.children(object),
                self.bus.text(object),
                optional((role == Role::Heading).then(|| self.bus.heading_level(object))),
                optional((role == Role::Document).then(|| self.bus.document_url(object))),
            ),
            self.locate(object, origin.place),
        )
        .await;
        fetched.rect = rect;
        if adjustments.hides_by_height && rect.is_some_and(|rect| rect.height == 0) {
            fetched.node.visibility = Visibility::Hidden;
            return;
        }
        fetched.node.name = name.unwrap_or_default();
        fetched.node.text = text.ok();
        if is_option && quirks::names_option_by_label(&fetched.node) {
            fetched.node.name = self.bus.label(object).await.unwrap_or_default();
        }
        if role == Role::ListItem
            && let Some(text) = &mut fetched.node.text
        {
            quirks::strip_list_bullet(text);
        }
        fetched.node.heading_level = heading_level;
        // An option is read as its mark and its name. The children of a named option only
        // repeat the name, such as the title and the switch inside a libadwaita switch row.
        if !is_option || fetched.node.name.trim().is_empty() {
            let mut children = children.unwrap_or_default();
            adjustments.unlisted.insert_into(object, &mut children);
            fetched.children = children;
        }
        fetched.set_url(url);
    }

    /// Where `object` is on screen. Only elements outside documents are located.
    async fn locate(&self, object: &Object, place: Place) -> Option<Rect> {
        optional((place == Place::Window).then(|| self.bus.extents(object))).await
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::super::bus::ToolkitFamily;
    use super::*;

    #[rstest]
    #[case::the_embedded_page(Some("https://a.example/"), &["devtools://x", "https://a.example/"], Some("https://a.example/"))]
    #[case::a_side_panel_had_the_focus(Some("chrome://side-panel/"), &["file:///page.html", "chrome://side-panel/"], Some("file:///page.html"))]
    #[case::only_a_browser_page(Some("chrome://bookmarks/"), &["chrome://bookmarks/"], Some("chrome://bookmarks/"))]
    #[case::no_embedded_document(None, &["about:blank", "http://b.example/"], Some("http://b.example/"))]
    #[case::no_documents(None, &[], None)]
    fn the_reported_url_is_the_page(
        #[case] embedded: Option<&str>,
        #[case] walked: &[&str],
        #[case] expected: Option<&str>,
    ) {
        let walked = walked.iter().map(|&url| url.to_owned()).collect();
        assert_eq!(page_url(embedded.map(str::to_owned), walked).as_deref(), expected);
    }

    #[rstest]
    #[case::named_excluded(PrivateWindows::Exclude, Some("Gecko"), Ok(Some("Gecko")))]
    #[case::unnamed(PrivateWindows::Exclude, None, Ok(None))]
    #[case::failed_excluded(PrivateWindows::Exclude, Some("failed"), Err(()))]
    #[case::failed_captured(PrivateWindows::Capture, Some("failed"), Ok(None))]
    fn a_failed_toolkit_lookup_fails_only_a_read_that_checks_privacy(
        #[case] reads: PrivateWindows,
        #[case] toolkit: Option<&str>,
        #[case] expected: Result<Option<&str>, ()>,
    ) {
        let lookup = match toolkit {
            Some("failed") => ToolkitLookup::Failed,
            Some(name) => ToolkitLookup::Named(std::sync::Arc::new(Toolkit {
                name: name.to_owned(),
                family: ToolkitFamily::of(name, ""),
            })),
            None => ToolkitLookup::Unnamed,
        };
        let found = toolkit_for_check(&lookup, reads);
        assert_eq!(
            found.map(|toolkit| toolkit.map(|toolkit| toolkit.name.as_str())).map_err(|_| ()),
            expected
        );
    }

    #[rstest]
    #[case::window(Some(AtspiRole::FRAME), Subject::Window)]
    #[case::document(Some(AtspiRole::DOCUMENT_WEB), Subject::Document)]
    #[case::unknown(None, Subject::Document)]
    fn an_element_of_an_unknown_role_is_checked_against_its_window(
        #[case] role: Option<AtspiRole>,
        #[case] expected: Subject,
    ) {
        assert_eq!(Subject::of(role), expected);
    }

    fn fetched(text: Option<&str>, atspi: AtspiRole) -> Fetched {
        Fetched {
            node: Node {
                text: text.map(str::to_owned),
                ..Node::default()
            },
            atspi: Some(atspi),
            children: Vec::new(),
            url: None,
            loading: Loading::Finished,
            terminal: None,
            listed: 0,
            rect: None,
            within: Within::Window,
            unknown_roles: UnknownRoles::default(),
        }
    }

    /// GNOME Calculator's spin row: an element that GTK 4 leaves out of the tree, whose text
    /// repeats the value of the spin button inside it.
    #[rstest]
    #[case::unlisted(AtspiRole::INVALID, None)]
    #[case::listed(AtspiRole::LABEL, Some("2"))]
    fn only_unlisted_elements_lose_the_text_that_their_children_repeat(
        #[case] row: AtspiRole,
        #[case] expected: Option<&str>,
    ) {
        let mut arena = Arena::default();
        arena.push(usize::MAX, fetched(None, AtspiRole::FRAME), Place::Window);
        arena.push(0, fetched(Some("2"), row), Place::Window);
        arena.push(1, fetched(Some("2"), AtspiRole::SPIN_BUTTON), Place::Window);
        let window = arena.into_tree();
        assert_eq!(window.children[0].text.as_deref(), expected);
    }

    #[rstest]
    #[case::a_showing_alert(1, AtspiRole::ALERT, Visibility::Showing, true)]
    #[case::a_hidden_dialog(1, AtspiRole::DIALOG, Visibility::Hidden, false)]
    #[case::a_deeper_dialog(2, AtspiRole::DIALOG, Visibility::Showing, false)]
    #[case::a_panel(1, AtspiRole::STATIC, Visibility::Showing, false)]
    fn a_showing_dialog_of_the_window_covers_the_page(
        #[case] depth: usize,
        #[case] role: AtspiRole,
        #[case] visibility: Visibility,
        #[case] expected: bool,
    ) {
        let mut totals = WalkTotals::default();
        let origin = Origin {
            depth,
            ..Origin::root(Place::Window)
        };
        let mut child = fetched(None, role);
        child.node.visibility = visibility;
        totals.record(origin, &mut child);
        assert_eq!(totals.dialog_covers_page, expected);
    }

    #[rstest]
    #[case::a_timeout(Err(Error::NoAnswer), Err(RoleFailure::TimedOut))]
    #[case::a_failure(Err(Error::Platform("gone".into())), Err(RoleFailure::Failed))]
    #[case::an_answer(Ok(AtspiRole::LABEL), Ok(AtspiRole::LABEL))]
    fn a_timed_out_role_call_is_told_apart(
        #[case] answer: Result<AtspiRole, Error>,
        #[case] expected: Result<AtspiRole, RoleFailure>,
    ) {
        assert_eq!(answered_role(answer), expected);
    }

    #[rstest]
    #[case::none(&[], Completeness::Complete, 0)]
    #[case::a_failure(&[RoleFailure::Failed], Completeness::Complete, 1)]
    #[case::a_timeout(&[RoleFailure::Failed, RoleFailure::TimedOut], Completeness::OutOfTime, 2)]
    fn a_timed_out_role_call_makes_the_read_run_out_of_time(
        #[case] failures: &[RoleFailure],
        #[case] expected: Completeness,
        #[case] count: usize,
    ) {
        let mut totals = WalkTotals::default();
        for &failure in failures {
            let mut element = fetched(None, AtspiRole::LABEL);
            element.unknown_roles.add(failure);
            totals.record(Origin::root(Place::Window), &mut element);
        }
        assert_eq!(totals.unknown_roles.count, count);
        assert_eq!(totals.unknown_roles.completeness(Completeness::Complete), expected);
    }

    #[rstest]
    fn each_element_takes_its_children_in_reading_order() {
        let at = |text: &str, y: i32| {
            let mut element = fetched(Some(text), AtspiRole::LABEL);
            element.rect = Some(Rect {
                x: 0,
                y,
                width: 10,
                height: 10,
            });
            element
        };
        let mut arena = Arena::default();
        arena.push(usize::MAX, fetched(None, AtspiRole::FRAME), Place::Window);
        arena.push(0, at("a", 0), Place::Window);
        arena.push(0, at("b", 10), Place::Document);
        // GTK 3 lists the children of a grid in reverse order.
        arena.push(1, at("a2", 10), Place::Window);
        arena.push(1, at("a1", 0), Place::Window);
        arena.push(2, at("b2", 10), Place::Document);
        arena.push(2, at("b1", 0), Place::Document);
        let window = arena.into_tree();
        let texts = |node: &Node| -> Vec<String> {
            node.children.iter().filter_map(|child| child.text.clone()).collect()
        };
        assert_eq!(texts(&window), ["a", "b"]);
        assert_eq!(texts(&window.children[0]), ["a1", "a2"]);
        assert_eq!(texts(&window.children[1]), ["b2", "b1"], "a document keeps its order");
    }

    #[rstest]
    fn a_window_with_one_child_keeps_it() {
        let mut arena = Arena::default();
        arena.push(usize::MAX, fetched(None, AtspiRole::FRAME), Place::Window);
        arena.push(0, fetched(Some("a"), AtspiRole::LABEL), Place::Window);
        let window = arena.into_tree();
        assert_eq!(window.children.len(), 1);
    }
}

//! Finds the window and the document that an element belongs to.
//!
//! A content event names only the element that changed. Capture needs the element's window so
//! that capture can tell which window of the application changed. The document matters because
//! browsers keep hidden documents in the tree of a window. Firefox keeps every loaded background
//! tab there, and capture reads the changes of a background tab like those of a background
//! window. Firefox also keeps a preloaded New Tab page in every window. The rotating stories of
//! that page sent 96% of Firefox's content events in a test session, and those events don't
//! describe any page.

use std::collections::HashMap;
use std::time::Duration;

use futures::future;
use tokio::time::Instant;

use super::bus::{App, Bus, Object, STATE_SHOWING, has_state};
use super::pauses::{PausePolicy, Pauses};
use super::roles::AtspiRole;
use crate::backend::{Error, Role, Visibility};
use crate::urls::is_visitable;

/// The number of elements that the cache holds before it starts over.
///
/// Elements that disappear are never reported as gone, so the cache forgets all of them now and
/// then. An application can also reuse the path of an element that it removed.
const MAX_PLACEMENTS: usize = 50_000;

/// How long the result of a check on a document stays trusted, if the document's application
/// doesn't show or hide anything in the meantime.
const CHECK_LIFETIME: Duration = Duration::from_millis(500);

/// How long the placement of an element is trusted.
///
/// An element can move to another window without an event that the cache notices. For
/// example, a tab that the user drags into an existing window keeps its elements.
const PLACEMENT_LIFETIME: Duration = Duration::from_secs(10);

/// How deep a walk goes before it takes the highest element that it reached as the window.
const MAX_DEPTH: usize = 200;

/// How long placing one element may take. Every event waits while it is placed, and an
/// application that just started can take a fifth of a second to answer while it draws its
/// first window.
const BUDGET: Duration = Duration::from_millis(50);

/// How long the elements of an application stay unplaced, without calls, after the application
/// answered too slowly or not at all.
const PAUSES: PausePolicy = PausePolicy {
    slow: Duration::from_secs(1),
    silent: Duration::from_secs(5),
    max_slow: 1,
};

/// What to do with an element inside a hidden document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HiddenDocuments {
    /// Report [`Located::HiddenPage`] or [`Located::Hidden`].
    Report,
    /// Report the window without checking the state of the document.
    Ignore,
}

/// Where an element is.
pub enum Located {
    Window(Object),
    /// The element is inside a hidden page that a person can visit, such as a background tab.
    /// The object is the outermost document of the page.
    HiddenPage(Object),
    /// The element is inside another hidden document, such as a preloaded New Tab page.
    Hidden,
    /// A call failed, the application did not answer recently, or the element is the root of
    /// an application, which doesn't lie in any window.
    Unknown,
}

#[derive(Clone)]
struct Placement {
    window: Object,
    /// The nearest document at or above the element.
    document: Option<Object>,
    /// The outermost document at or above the element, which holds the nearest one.
    page: Option<Object>,
    /// When a walk found the window. An element below a cached element takes the time of the
    /// cached element.
    at: Instant,
}

/// An element that a walk passed without finding it in the cache.
struct Passed {
    element: Object,
    role: Role,
}

/// What a check found out about a document, and when.
struct Checked<T> {
    value: T,
    at: Instant,
}

impl<T: Copy> Checked<T> {
    fn current(&self, now: Instant) -> Option<T> {
        (now < self.at + CHECK_LIFETIME).then_some(self.value)
    }
}

/// Whether a document is a page that a person can visit, as opposed to one that the browser
/// draws itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Page,
    BrowserPage,
}

/// The placement of every element that a walk passed, by application.
#[derive(Default)]
struct Placements {
    by_app: HashMap<App, HashMap<Object, Placement>>,
    /// How many elements the cache holds across all applications.
    len: usize,
}

impl Placements {
    /// The placement of `element`, unless it is older than the [`PLACEMENT_LIFETIME`].
    fn get(&self, element: &Object, now: Instant) -> Option<&Placement> {
        let placement = self.by_app.get(element.app())?.get(element)?;
        (now < placement.at + PLACEMENT_LIFETIME).then_some(placement)
    }

    fn insert(&mut self, element: Object, placement: Placement) {
        if self.len >= MAX_PLACEMENTS {
            self.by_app.clear();
            self.len = 0;
        }
        let app = self.by_app.entry(element.app().clone()).or_default();
        if app.insert(element, placement).is_none() {
            self.len += 1;
        }
    }

    fn forget_app(&mut self, app: &str) {
        if let Some(placements) = self.by_app.remove(app) {
            self.len -= placements.len();
        }
    }

    /// Whether the cache holds `element` as a window.
    fn is_window(&self, element: &Object, now: Instant) -> bool {
        self.get(element, now).is_some_and(|placement| placement.window == *element)
    }
}

pub struct Locations {
    placements: Placements,
    visibilities: HashMap<Object, Checked<Visibility>>,
    kinds: HashMap<Object, Checked<Kind>>,
    pauses: Pauses,
}

impl Default for Locations {
    fn default() -> Self {
        Self {
            placements: Placements::default(),
            visibilities: HashMap::new(),
            kinds: HashMap::new(),
            pauses: Pauses::new(PAUSES),
        }
    }
}

impl Locations {
    /// Finds the top-level window of `element`, and checks its document when `hidden` asks.
    pub async fn locate(
        &mut self,
        bus: &Bus,
        element: &Object,
        hidden: HiddenDocuments,
    ) -> Located {
        let now = Instant::now();
        if element.is_root() || self.pauses.is_paused(element.app().as_str(), now) {
            return Located::Unknown;
        }
        let located =
            tokio::time::timeout(BUDGET, self.try_locate(bus, element, hidden, now)).await;
        let now = Instant::now();
        match located {
            Ok(Ok(located)) => {
                self.pauses.answered(element.app().as_str());
                return located;
            }
            Ok(Err(Error::NoAnswer)) => self.pauses.not_answered(element.app(), now),
            Ok(Err(_)) => {}
            Err(_) => {
                tracing::debug!(app = ?element.app(), "placing an element took too long");
                self.pauses.answered_slowly(element.app(), now);
            }
        }
        Located::Unknown
    }

    /// Forgets what checks found out about documents, after something was shown or hidden.
    ///
    /// Switching a tab shows one document and hides another.
    pub fn forget_visibilities(&mut self) {
        self.visibilities.clear();
        self.kinds.clear();
    }

    /// Forgets the placements of the elements of `app`, whose elements may have moved to
    /// another window.
    pub fn forget_app(&mut self, app: &str) {
        self.placements.forget_app(app);
    }

    /// Forgets the placements of the elements of the application of `element` when `element`
    /// is a window.
    pub fn forget_app_at_window(&mut self, element: &Object) {
        if self.placements.is_window(element, Instant::now()) {
            self.placements.forget_app(element.app().as_str());
        }
    }

    async fn try_locate(
        &mut self,
        bus: &Bus,
        element: &Object,
        hidden: HiddenDocuments,
        now: Instant,
    ) -> Result<Located, Error> {
        let placement = self.placement(bus, element, now).await?;
        let (Some(document), Some(page)) = (&placement.document, &placement.page) else {
            return Ok(Located::Window(placement.window));
        };
        if hidden == HiddenDocuments::Ignore
            || self.visibility(bus, document, now).await? == Visibility::Showing
        {
            return Ok(Located::Window(placement.window));
        }
        Ok(match self.kind(bus, page, now).await {
            Kind::Page => Located::HiddenPage(page.clone()),
            Kind::BrowserPage => Located::Hidden,
        })
    }

    async fn visibility(
        &mut self,
        bus: &Bus,
        document: &Object,
        now: Instant,
    ) -> Result<Visibility, Error> {
        if let Some(visibility) = self.visibilities.get(document).and_then(|c| c.current(now)) {
            return Ok(visibility);
        }
        let visibility = if has_state(&bus.state(document).await?, STATE_SHOWING) {
            Visibility::Showing
        } else {
            Visibility::Hidden
        };
        self.visibilities.insert(document.clone(), Checked {
            value: visibility,
            at: now,
        });
        Ok(visibility)
    }

    async fn kind(&mut self, bus: &Bus, document: &Object, now: Instant) -> Kind {
        if let Some(kind) = self.kinds.get(document).and_then(|c| c.current(now)) {
            return kind;
        }
        let kind = if bus.document_url(document).await.is_some_and(|url| is_visitable(&url)) {
            Kind::Page
        } else {
            Kind::BrowserPage
        };
        self.kinds.insert(document.clone(), Checked {
            value: kind,
            at: now,
        });
        kind
    }

    /// Walks from `element` up to the first element in the cache, or to the window, and caches
    /// the placement of every element that the walk passed.
    async fn placement(
        &mut self,
        bus: &Bus,
        element: &Object,
        now: Instant,
    ) -> Result<Placement, Error> {
        let mut passed: Vec<Passed> = Vec::new();
        let mut current = element.clone();
        let cached = loop {
            if let Some(placement) = self.placements.get(&current, now) {
                break Some(placement.clone());
            }
            let (role, parent) = future::join(bus.role(&current), bus.parent(&current)).await;
            let role = AtspiRole::role(role?);
            let parent = parent?;
            passed.push(Passed {
                element: current,
                role,
            });
            if parent.is_above_windows() || passed.len() >= MAX_DEPTH {
                break None;
            }
            current = parent;
        };
        let mut placement = cached.unwrap_or_else(|| Placement {
            window: passed.last().expect("the walk passed an element").element.clone(),
            document: None,
            page: None,
            at: now,
        });
        // Each element takes the window and the documents from the elements above it.
        for passed in passed.into_iter().rev() {
            if passed.role == Role::Document {
                placement.document = Some(passed.element.clone());
                placement.page.get_or_insert_with(|| passed.element.clone());
            }
            self.placements.insert(passed.element, placement.clone());
        }
        Ok(placement)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use zbus::ObjectPath;

    use super::*;

    fn object(app: &str, path: &'static str) -> Object {
        Object::new(App::new(app), ObjectPath::from_static_str_unchecked(path))
    }

    fn placed_in(window: &Object) -> Placement {
        Placement {
            window: window.clone(),
            document: None,
            page: None,
            at: Instant::now(),
        }
    }

    #[rstest]
    fn forgetting_an_application_keeps_the_others() {
        let mut placements = Placements::default();
        let window = object(":1.1", "/w");
        let other = object(":1.2", "/w");
        placements.insert(object(":1.1", "/e"), placed_in(&window));
        placements.insert(object(":1.2", "/e"), placed_in(&other));
        placements.forget_app(":1.1");
        let now = Instant::now();
        assert!(placements.get(&object(":1.1", "/e"), now).is_none());
        assert!(placements.get(&object(":1.2", "/e"), now).is_some());
        assert_eq!(placements.len, 1);
    }

    #[rstest]
    #[case::the_window("/w", true)]
    #[case::an_element_inside("/e", false)]
    #[case::an_unknown_element("/x", false)]
    fn only_a_cached_window_is_a_window(#[case] path: &'static str, #[case] expected: bool) {
        let mut placements = Placements::default();
        let window = object(":1.1", "/w");
        placements.insert(window.clone(), placed_in(&window));
        placements.insert(object(":1.1", "/e"), placed_in(&window));
        assert_eq!(placements.is_window(&object(":1.1", path), Instant::now()), expected);
    }

    /// A tab that the user drags into another window keeps its elements, so only the age of a
    /// placement tells that the window may have changed.
    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn a_placement_expires() {
        let mut placements = Placements::default();
        let window = object(":1.1", "/w");
        let element = object(":1.1", "/e");
        placements.insert(element.clone(), placed_in(&window));
        tokio::time::advance(PLACEMENT_LIFETIME - Duration::from_millis(1)).await;
        assert!(placements.get(&element, Instant::now()).is_some());
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(placements.get(&element, Instant::now()).is_none());
    }
}

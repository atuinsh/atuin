//! The signals that the backend listens for, and what each one means to capture.

use std::num::NonZero;
use std::sync::Arc;
use std::time::Duration;

use atuin_common::collections::{ForgetfulMap, Tally};
use futures::future::BoxFuture;
use futures::stream::SelectAll;
use futures::{FutureExt as _, StreamExt as _, future};
use parking_lot::Mutex;
use tokio::time::Instant;
use zbus::{MatchRule, MessageStream, ObjectPath, Structure, Variant, message};

use super::AtSpi;
use super::apps::{self, AppKind, KnownApp, Profiles, Waiter};
use super::bus::{App, Bus, Object, REGISTRY, ROOT_PATH};
use super::location::{HiddenDocuments, Located, Locations};
use super::notices::Notices;
use super::roles::AtspiRole;
use crate::backend::{Cause, Event, EventCount, Mark, Purpose};

/// Event counts for one application, keyed by event and then by event detail.
pub type AppTally = Tally<&'static str, Tally<String, u64>>;

/// Flattens the event counts of `app`.
pub fn event_counts<A: Clone>(app: A, tally: AppTally) -> impl Iterator<Item = EventCount<A>> {
    tally.into_iter().flat_map(move |(event, details)| {
        let app = app.clone();
        details.into_iter().map(move |(detail, count)| EventCount {
            app: app.clone(),
            event,
            detail,
            count,
        })
    })
}

/// How long an activation waits for a deactivation of the same window, which makes it void.
const ACTIVATION_HOLD: Duration = Duration::from_millis(50);

/// How many elements the [`EventQueue`] keeps the last "checked" value of. A full map is
/// emptied, and fills again as the elements announce their values.
const MAX_MARKS: usize = 10_000;

/// How many signals of one [`LISTENED_EVENTS`] entry wait for capture before the connection
/// that receives them stops reading.
const MAX_QUEUED_SIGNALS: NonZero<usize> = NonZero::new(4096).expect("the size is not zero");

/// An event that capture listens for.
struct ListenedEvent {
    /// The event in the registry's own spelling, which tells applications to send it.
    registry: &'static str,
    interface: &'static str,
    member: &'static str,
    /// The first argument of the signal, which AT-SPI calls the detail. `None` matches every
    /// detail.
    detail: Option<&'static str>,
}

impl ListenedEvent {
    /// The rule that makes the bus deliver the signals of this event.
    fn match_rule(&self) -> zbus::Result<MatchRule<'static>> {
        let rule = MatchRule::builder()
            .msg_type(message::Type::Signal)
            .interface(self.interface)
            .member(self.member);
        match self.detail {
            Some(detail) => rule.arg(0, detail),
            None => rule,
        }
        .build()
    }
}

/// The events that capture interprets.
///
/// The bus delivers only these. An application sends many other events, and closing a large
/// window sends thousands of them within a second.
const LISTENED_EVENTS: [ListenedEvent; 11] = [
    ListenedEvent {
        registry: "Window:Create:",
        interface: "org.a11y.atspi.Event.Window",
        member: "Create",
        detail: None,
    },
    ListenedEvent {
        registry: "Window:Activate:",
        interface: "org.a11y.atspi.Event.Window",
        member: "Activate",
        detail: None,
    },
    ListenedEvent {
        registry: "Window:Deactivate:",
        interface: "org.a11y.atspi.Event.Window",
        member: "Deactivate",
        detail: None,
    },
    ListenedEvent {
        registry: "Object:StateChanged:Active",
        interface: "org.a11y.atspi.Event.Object",
        member: "StateChanged",
        detail: Some("active"),
    },
    ListenedEvent {
        registry: "Object:StateChanged:Checked",
        interface: "org.a11y.atspi.Event.Object",
        member: "StateChanged",
        detail: Some("checked"),
    },
    ListenedEvent {
        registry: "Object:StateChanged:Focused",
        interface: "org.a11y.atspi.Event.Object",
        member: "StateChanged",
        detail: Some("focused"),
    },
    ListenedEvent {
        registry: "Object:StateChanged:Showing",
        interface: "org.a11y.atspi.Event.Object",
        member: "StateChanged",
        detail: Some("showing"),
    },
    ListenedEvent {
        registry: "Object:PropertyChange:AccessibleName",
        interface: "org.a11y.atspi.Event.Object",
        member: "PropertyChange",
        detail: Some("accessible-name"),
    },
    ListenedEvent {
        registry: "Object:TextChanged:",
        interface: "org.a11y.atspi.Event.Object",
        member: "TextChanged",
        detail: None,
    },
    ListenedEvent {
        registry: "Object:ChildrenChanged:",
        interface: "org.a11y.atspi.Event.Object",
        member: "ChildrenChanged",
        detail: None,
    },
    ListenedEvent {
        registry: "Document:LoadComplete:",
        interface: "org.a11y.atspi.Event.Document",
        member: "LoadComplete",
        detail: None,
    },
];

/// Opens a connection to the accessibility bus at `address` that receives the
/// [`LISTENED_EVENTS`], and asks the registry to have applications send them.
///
/// A connection stops reading while one of its signal queues is full, and a reply on that
/// connection then waits behind the signals until the call times out. The signals arrive on a
/// connection of their own, so that a burst of them delays only other signals.
async fn listen(address: &str) -> zbus::Result<SelectAll<MessageStream>> {
    let listener = zbus::connection::Builder::address(address).build().await?;
    let mut signals = SelectAll::new();
    for event in &LISTENED_EVENTS {
        let stream =
            MessageStream::for_match_rule(event.match_rule()?, &listener, Some(MAX_QUEUED_SIGNALS));
        signals.push(stream.await?);
    }
    for event in &LISTENED_EVENTS {
        // A fixed-size array would serialize as a struct, which the registry ignores silently.
        let no_properties: &[&str] = &[];
        listener
            .call_method(
                Some(REGISTRY),
                "/org/a11y/atspi/registry",
                Some(REGISTRY),
                "RegisterEvent",
                &(event.registry, no_properties, ""),
            )
            .await?;
    }
    Ok(signals)
}

/// An activation that counts only when no deactivation of the same window follows at once.
///
/// Qt on Wayland activates and deactivates its window in one moment when another application
/// takes the focus.
struct HeldActivation {
    window: Object,
    until: Instant,
}

/// A change to the content of an application, and the element that announced it.
struct ContentChange {
    source: Object,
    cause: Cause,
    hidden: HiddenDocuments,
}

/// What a signal means to capture.
enum Signal {
    Event(Event<AtSpi>),
    /// A content change, which needs its window before it becomes an event.
    Content(ContentChange),
    Activation(Object),
}

/// A signal whose sender, or whose added child, is still to be looked up.
struct Unsorted {
    msg: zbus::Message,
    /// The lookup, which keeps its progress when [`EventQueue::next`] is cancelled.
    ///
    /// The mutex makes the queue `Sync`. Only `&mut self` reaches the lookup, so the mutex is
    /// never locked.
    sorting: Mutex<BoxFuture<'static, Sorted>>,
}

/// What the lookups found out about a signal.
struct Sorted {
    /// The application that sent the signal.
    sender: App,
    kind: AppKind,
    /// The dialog or the alert that the signal added inside a window, as [`dialog_role`]
    /// found it.
    added_dialog: Option<Object>,
}

/// The signals of the accessibility bus and the notifications of the session, on their way to
/// becoming events.
pub struct EventQueue {
    signals: SelectAll<MessageStream>,
    /// The notifications of the session, when the session bus lets this process monitor it.
    notices: Option<Notices>,
    /// Event counts per application bus name, when the connection is for
    /// [`Purpose::Inspect`].
    tally: Option<Tally<String, AppTally>>,
    /// The unique bus name of the registry, whose own changes aren't content.
    registry: Option<App>,
    locations: Locations,
    /// A content change whose window is still to be found. The change waits here, so that
    /// cancelling [`next`](Self::next) during the search doesn't lose the change.
    unlocated: Option<ContentChange>,
    /// A signal whose sorting is still to finish. The signal waits here, so that cancelling
    /// [`next`](Self::next) during the lookups doesn't lose the signal.
    unsorted: Option<Unsorted>,
    /// An activation that waits for [`ACTIVATION_HOLD`] before it
    /// becomes an event.
    held: Option<HeldActivation>,
    marks: Marks,
}

impl EventQueue {
    /// Listens on the accessibility bus at `address`, and on the session bus for
    /// notifications when `purpose` is capture.
    ///
    /// `registry` is the unique bus name of the registry.
    pub async fn open(
        address: &str,
        purpose: Purpose,
        registry: Option<App>,
    ) -> zbus::Result<Self> {
        let signals = listen(address).await?;
        let notices = match purpose {
            Purpose::Capture => match Notices::watch().await {
                Ok(notices) => Some(notices),
                Err(error) => {
                    tracing::warn!(%error, "the session bus can't be monitored, so notifications come from the accessibility bus alone");
                    None
                }
            },
            Purpose::Inspect => None,
        };
        Ok(Self {
            signals,
            notices,
            tally: (purpose == Purpose::Inspect).then(Tally::default),
            registry,
            locations: Locations::default(),
            unlocated: None,
            unsorted: None,
            held: None,
            marks: Marks::default(),
        })
    }

    /// Whether the session's notifications arrive from the session bus.
    pub const fn watches_notices(&self) -> bool {
        self.notices.is_some()
    }

    /// Takes the event counts per application bus name.
    pub fn take_tally(&mut self) -> Tally<String, AppTally> {
        self.tally.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Waits for the next event.
    ///
    /// Drops content changes from hidden documents that aren't pages, such as Firefox's
    /// preloaded New Tab page. Cancelling the wait doesn't lose any signal.
    pub async fn next(&mut self, bus: &Bus, profiles: &Arc<Profiles>) -> Option<Event<AtSpi>> {
        loop {
            if self.unlocated.is_some() {
                if let Some(event) = self.locate_change(bus).await {
                    return Some(event);
                }
                continue;
            }
            if self.unsorted.is_some() {
                if let Some(event) = self.sort_signal().await {
                    return Some(event);
                }
                continue;
            }
            let notice = async {
                match &mut self.notices {
                    Some(notices) => notices.next().await,
                    None => future::pending().await,
                }
            };
            let held_until = self.held.as_ref().map(|held| held.until);
            tokio::select! {
                msg = self.signals.next() => {
                    if let Ok(msg) = msg?
                        && let Some(event) = self.receive(msg, bus, profiles)
                    {
                        return Some(event);
                    }
                }
                Some(notice) = notice => return Some(Event::Notified { notice }),
                () = sleep_until(held_until) => {
                    let held = self.held.take().expect("an activation is held");
                    return Some(Event::Activated { window: held.window });
                }
            }
        }
    }

    /// Finds the window of the change in [`unlocated`](Self::unlocated), and returns the
    /// event that the change becomes.
    ///
    /// The change stays in place until the search ends, so that a cancelled wait doesn't lose
    /// the change.
    async fn locate_change(&mut self, bus: &Bus) -> Option<Event<AtSpi>> {
        let change = self.unlocated.as_ref()?;
        let located = self.locations.locate(bus, &change.source, change.hidden).await;
        let change = self.unlocated.take().expect("the change waits until now");
        let window = match located {
            // A background tab changes as a window of its own.
            Located::Window(window) | Located::HiddenPage(window) => Some(window),
            Located::Unknown => None,
            Located::Hidden => return None,
        };
        Some(Event::ContentChanged {
            app: change.source.into_app(),
            window,
            cause: change.cause,
        })
    }

    /// Takes a signal off the bus. Sorts the signal at once when that doesn't need any calls, and
    /// otherwise starts the lookups in [`unsorted`](Self::unsorted).
    fn receive(
        &mut self,
        msg: zbus::Message,
        bus: &Bus,
        profiles: &Arc<Profiles>,
    ) -> Option<Event<AtSpi>> {
        let header = msg.header();
        // Every AT-SPI event signal names its sender.
        let sender = header.sender()?.as_str();
        let sorting = match profiles.known(sender) {
            Some(KnownApp { app, profile }) => {
                let child = profile
                    .adds_dialogs_inside_windows()
                    .then(|| added_child(&msg, &app))
                    .flatten();
                let Some(child) = child else {
                    return self.apply(&msg, Sorted {
                        sender: app,
                        kind: profile.kind,
                        added_dialog: None,
                    });
                };
                let bus = bus.clone();
                async move {
                    Sorted {
                        added_dialog: dialog_role(&bus, child).await,
                        sender: app,
                        kind: profile.kind,
                    }
                }
                .boxed()
            }
            None => {
                let sender = App::new(sender);
                let child = added_child(&msg, &sender);
                let bus = bus.clone();
                let profiles = Arc::clone(profiles);
                async move {
                    let profile = profiles.profile(&bus, &sender, Waiter::Event).await;
                    let added_dialog = match child {
                        Some(child) if profile.adds_dialogs_inside_windows() => {
                            dialog_role(&bus, child).await
                        }
                        _ => None,
                    };
                    Sorted {
                        sender,
                        kind: profile.kind,
                        added_dialog,
                    }
                }
                .boxed()
            }
        };
        self.unsorted = Some(Unsorted {
            msg,
            sorting: Mutex::new(sorting),
        });
        None
    }

    /// Finishes the lookups of the signal in [`unsorted`](Self::unsorted), and returns the
    /// event that the signal completes.
    ///
    /// The signal stays in place until every call about it has answered, so that a cancelled
    /// wait doesn't lose the signal.
    async fn sort_signal(&mut self) -> Option<Event<AtSpi>> {
        let unsorted = self.unsorted.as_mut()?;
        let sorted = unsorted.sorting.get_mut().await;
        let Unsorted { msg, .. } = self.unsorted.take().expect("the signal waits until now");
        self.apply(&msg, sorted)
    }

    /// Interprets a signal that an application of the kind `sorted.kind` sent.
    fn apply(&mut self, msg: &zbus::Message, sorted: Sorted) -> Option<Event<AtSpi>> {
        let Sorted {
            sender,
            kind,
            added_dialog,
        } = sorted;
        match kind {
            // An alert inside a window is a dialog of its own, and also changes the window,
            // whose change waits in `unlocated`.
            AppKind::Application => {
                let event = self.consider(msg, &sender);
                added_dialog.map(|window| Event::Opened { window }).or(event)
            }
            // The frame of a window repeats the window's activations and its title.
            AppKind::FrameDrawer => None,
            AppKind::Shell => self.consider_from_shell(msg, &sender, added_dialog),
        }
    }

    /// Interprets a signal, and returns the event that it completes.
    ///
    /// An activation waits for [`ACTIVATION_HOLD`], and a content change waits in
    /// [`unlocated`](Self::unlocated) for its window.
    fn consider(&mut self, msg: &zbus::Message, sender: &App) -> Option<Event<AtSpi>> {
        match self.interpret(msg, sender) {
            // Toolkits announce one activation with two signals, window:activate and the
            // active state, and the second must not pass the first on.
            Some(Signal::Activation(window))
                if self.held.as_ref().is_some_and(|held| held.window == window) =>
            {
                None
            }
            Some(Signal::Activation(window)) => {
                let until = Instant::now() + ACTIVATION_HOLD;
                let earlier = self.held.replace(HeldActivation { window, until })?;
                Some(Event::Activated {
                    window: earlier.window,
                })
            }
            Some(Signal::Event(Event::Deactivated { window }))
                if self.held.as_ref().is_some_and(|held| held.window == window) =>
            {
                tracing::debug!(?window, "dropping an activation that a deactivation followed");
                self.held = None;
                Some(Event::Deactivated { window })
            }
            Some(Signal::Event(event)) => Some(event),
            Some(Signal::Content(change)) => {
                self.unlocated = Some(change);
                None
            }
            None => None,
        }
    }

    /// Converts a signal of the desktop shell to an event when the event is about a window
    /// or a dialog that opened, such as a prompt for a password.
    ///
    /// The window of the application that the user worked in stays the active one while the
    /// shell's overview or one of its menus is open. The changes of the overview and the
    /// menus aren't content. The shell's own dialogs aren't windows on the bus, so a dialog
    /// that the shell adds to its stage counts as a window that opened.
    ///
    /// `dialog` is the dialog that the signal added, as [`dialog_role`] found it.
    fn consider_from_shell(
        &mut self,
        msg: &zbus::Message,
        sender: &App,
        dialog: Option<Object>,
    ) -> Option<Event<AtSpi>> {
        match self.interpret(msg, sender)? {
            Signal::Event(event @ Event::Opened { .. }) => Some(event),
            Signal::Content(_) => dialog.map(|window| Event::Opened { window }),
            Signal::Activation(_) | Signal::Event(_) => None,
        }
    }

    /// Counts the signal, which `sender` sent, and converts it to an event when capture needs
    /// it.
    fn interpret(&mut self, msg: &zbus::Message, sender: &App) -> Option<Signal> {
        with_fields(msg, sender, |signal| {
            if let Some(tally) = &mut self.tally {
                let sender = signal.sender.as_str();
                *tally.slot(sender).slot(&signal.kind.as_str()).slot(signal.detail) += 1;
            }
            let meaning = signal.meaning()?;
            self.weigh(meaning, &signal)
        })
    }

    /// Turns what a signal says into a signal for capture, with the state that the queue keeps.
    fn weigh(&mut self, meaning: Meaning, signal: &SignalFields<'_>) -> Option<Signal> {
        // The registry's root changes when an application joins or leaves the bus.
        let from_registry =
            self.registry.as_ref().is_some_and(|registry| registry == signal.sender);
        let content = |source, cause, hidden| {
            (!from_registry).then_some(Signal::Content(ContentChange {
                source,
                cause,
                hidden,
            }))
        };
        let event = match meaning {
            Meaning::Activated => return Some(Signal::Activation(signal.source())),
            Meaning::Deactivated => Event::Deactivated {
                window: signal.source(),
            },
            Meaning::Retitled(title) => Event::TitleChanged {
                window: signal.source(),
                title,
            },
            Meaning::FocusMoved => Event::FocusMoved {
                app: signal.sender.clone(),
            },
            // A new window can hold elements that another window held before, such as a
            // browser tab that the user dragged out of its window.
            Meaning::Opened => {
                self.locations.forget_app(signal.sender.as_str());
                Event::Opened {
                    window: signal.source(),
                }
            }
            Meaning::RootGained(child) => {
                self.locations.forget_app(child.app().as_str());
                if child.is_root() {
                    Event::Joined {
                        app: child.into_app(),
                    }
                } else {
                    Event::Opened { window: child }
                }
            }
            Meaning::Rearranged => {
                let source = signal.source();
                // A window that gains or loses children can gain or lose elements of another
                // window, such as a tab that the user dragged into it.
                self.locations.forget_app_at_window(&source);
                return content(source, Cause::Restructured, HiddenDocuments::Report);
            }
            Meaning::Changed(cause) => {
                return content(signal.source(), cause, HiddenDocuments::Report);
            }
            // A toggle changes one mark in one burst, as a rearrangement does. Reading soon
            // after it matters because a dialog of settings often closes right after the toggle.
            // An event that repeats the last value of its element doesn't change anything. GTK 3
            // popovers send such repeats every half second. See
            // [`quirks::hides_popover_option_state`](super::quirks::hides_popover_option_state).
            Meaning::Marked(mark) => {
                let source = signal.source();
                if !self.marks.changes(&source, mark) {
                    return None;
                }
                return content(source, Cause::Restructured, HiddenDocuments::Report);
            }
            // Switching a tab or expanding a section only shows and hides elements. A switch
            // can announce itself before the document that it shows reports the showing state,
            // so the event counts regardless of the state of its document.
            Meaning::Shown => {
                self.locations.forget_visibilities();
                return content(signal.source(), Cause::Restructured, HiddenDocuments::Ignore);
            }
        };
        Some(Signal::Event(event))
    }
}

/// Returns `child` when its role is a dialog or an alert. The role must arrive within the
/// [`BUDGET`](apps::BUDGET).
async fn dialog_role(bus: &Bus, child: Object) -> Option<Object> {
    let role = tokio::time::timeout(apps::BUDGET, bus.role(&child)).await.ok()?.ok()?;
    matches!(role, AtspiRole::DIALOG | AtspiRole::ALERT).then_some(child)
}

async fn sleep_until(until: Option<Instant>) {
    match until {
        Some(until) => tokio::time::sleep_until(until).await,
        None => future::pending().await,
    }
}

/// The element that a `children-changed:add` signal reports as added inside a window.
///
/// Returns `None` for a signal at a root, because a child that is added at a root is a new
/// window.
///
/// The body of the signal is parsed here and again when the signal is interpreted. The body is
/// small, so the second parse is cheap next to the role call that follows for the child.
fn added_child(msg: &zbus::Message, sender: &App) -> Option<Object> {
    // The member is in the header, so other signals skip the parsing of their bodies.
    if msg.header().member().is_none_or(|member| member.as_str() != "ChildrenChanged") {
        return None;
    }
    with_fields(msg, sender, |signal| {
        if signal.at_root() {
            None
        } else {
            signal.added_child()
        }
    })
}

/// The kinds of AT-SPI event signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventKind {
    WindowActivate,
    WindowDeactivate,
    WindowCreate,
    WindowOther,
    StateChanged,
    PropertyChange,
    TextChanged,
    ChildrenChanged,
    ObjectOther,
    LoadComplete,
    DocumentOther,
    Other,
}

impl EventKind {
    /// The kind of the signal `member` of the event interface `interface`, without the
    /// interface's `org.a11y.atspi.Event.` prefix.
    fn of(interface: &str, member: &str) -> Self {
        match (interface, member) {
            ("Window", "Activate") => Self::WindowActivate,
            ("Window", "Deactivate") => Self::WindowDeactivate,
            ("Window", "Create") => Self::WindowCreate,
            ("Window", _) => Self::WindowOther,
            ("Object", "StateChanged") => Self::StateChanged,
            ("Object", "PropertyChange") => Self::PropertyChange,
            ("Object", "TextChanged") => Self::TextChanged,
            ("Object", "ChildrenChanged") => Self::ChildrenChanged,
            ("Object", _) => Self::ObjectOther,
            ("Document", "LoadComplete") => Self::LoadComplete,
            ("Document", _) => Self::DocumentOther,
            _ => Self::Other,
        }
    }

    /// The event in the words of AT-SPI's clients, such as `object:state-changed`.
    const fn as_str(self) -> &'static str {
        match self {
            Self::WindowActivate => "window:activate",
            Self::WindowDeactivate => "window:deactivate",
            Self::WindowCreate => "window:create",
            Self::WindowOther => "window:other",
            Self::StateChanged => "object:state-changed",
            Self::PropertyChange => "object:property-change",
            Self::TextChanged => "object:text-changed",
            Self::ChildrenChanged => "object:children-changed",
            Self::ObjectOther => "object:other",
            Self::LoadComplete => "document:load-complete",
            Self::DocumentOther => "document:other",
            Self::Other => "other",
        }
    }
}

/// The parts of a signal that capture reads. AT-SPI signals carry the detail, two numbers, and
/// a value of the event.
struct SignalFields<'a> {
    kind: EventKind,
    detail: &'a str,
    sender: &'a App,
    path: Option<&'a ObjectPath<'a>>,
    fields: &'a [Variant<'a>],
}

/// Parses `msg`, which `sender` sent, and hands its parts to `read`. Returns `None` for a
/// message that isn't an AT-SPI event signal.
fn with_fields<T>(
    msg: &zbus::Message,
    sender: &App,
    read: impl FnOnce(SignalFields<'_>) -> Option<T>,
) -> Option<T> {
    let header = msg.header();
    let interface = header.interface()?.as_str().strip_prefix("org.a11y.atspi.Event.")?;
    let kind = EventKind::of(interface, header.member()?.as_str());
    let body = msg.body();
    let fields: Structure<'_> = body.deserialize().ok()?;
    let fields = fields.fields();
    let Some(Variant::Str(detail)) = fields.first() else {
        return None;
    };
    read(SignalFields {
        kind,
        detail: detail.as_str(),
        sender,
        path: header.path(),
        fields,
    })
}

/// What a signal says, before the state of the queue weighs in.
#[derive(Debug, PartialEq, Eq)]
enum Meaning {
    Activated,
    Deactivated,
    Retitled(String),
    FocusMoved,
    Opened,
    /// A root gained a child: a window, or, in the case of the registry's root, an
    /// application. Some toolkits announce new windows only this way.
    RootGained(Object),
    /// An element gained or lost children.
    Rearranged,
    Changed(Cause),
    /// An option became checked, or unchecked.
    Marked(Mark),
    /// Elements were shown or hidden.
    Shown,
}

impl SignalFields<'_> {
    fn meaning(&self) -> Option<Meaning> {
        let gained = self.gained();
        Some(match (self.kind, self.detail) {
            (EventKind::WindowActivate, _) => Meaning::Activated,
            (EventKind::StateChanged, "active") if gained => Meaning::Activated,
            (EventKind::WindowDeactivate, _) | (EventKind::StateChanged, "active") => {
                Meaning::Deactivated
            }
            (EventKind::PropertyChange, "accessible-name") => {
                Meaning::Retitled(self.title()?.to_owned())
            }
            (EventKind::StateChanged, "focused") if gained => Meaning::FocusMoved,
            (EventKind::WindowCreate, _) => Meaning::Opened,
            (EventKind::ChildrenChanged, added) if added.starts_with("add") && self.at_root() => {
                Meaning::RootGained(self.added_child()?)
            }
            (EventKind::LoadComplete, _) => Meaning::Changed(Cause::Loaded),
            (EventKind::TextChanged, _) => Meaning::Changed(Cause::Edited),
            (EventKind::ChildrenChanged, _) => Meaning::Rearranged,
            (EventKind::StateChanged, "checked") => Meaning::Marked(if gained {
                Mark::Checked
            } else {
                Mark::Unchecked
            }),
            (EventKind::StateChanged, "showing") => Meaning::Shown,
            _ => return None,
        })
    }

    /// The element that sent the signal.
    fn source(&self) -> Object {
        let path = self.path.expect("signals carry a path");
        Object::new(self.sender.clone(), path.clone())
    }

    /// Whether the element gained the state that a state change names.
    fn gained(&self) -> bool {
        matches!(self.fields.get(1), Some(Variant::I32(1)))
    }

    fn at_root(&self) -> bool {
        self.path.is_some_and(|path| path.as_str() == ROOT_PATH)
    }

    /// The element that a `children-changed:add` signal reports as added.
    fn added_child(&self) -> Option<Object> {
        if self.kind != EventKind::ChildrenChanged || !self.detail.starts_with("add") {
            return None;
        }
        let Some(Variant::Variant(child)) = self.fields.get(3) else {
            return None;
        };
        Object::from_variant(child, self.sender)
    }

    /// The new name that a property change of the name reports.
    fn title(&self) -> Option<&str> {
        let Some(Variant::Variant(title)) = self.fields.get(3) else {
            return None;
        };
        let Variant::Str(title) = &**title else {
            return None;
        };
        Some(title.as_str())
    }
}

/// The last mark that each element announced.
struct Marks(ForgetfulMap<Object, Mark>);

impl Default for Marks {
    fn default() -> Self {
        Self(ForgetfulMap::new(MAX_MARKS))
    }
}

impl Marks {
    /// Notes that `source` announced `mark`, and tells whether that changes the mark of
    /// `source`.
    fn changes(&mut self, source: &Object, mark: Mark) -> bool {
        if self.0.get(source) == Some(&mark) {
            return false;
        }
        self.0.insert(source.clone(), mark);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use rstest::rstest;

    use super::*;

    fn fields<'a>(
        kind: EventKind,
        detail: &'a str,
        fields: &'a [Variant<'a>],
        path: &'a ObjectPath<'a>,
    ) -> SignalFields<'a> {
        static SENDER: LazyLock<App> = LazyLock::new(|| App::new(":1.42"));
        SignalFields {
            kind,
            detail,
            sender: &SENDER,
            path: Some(path),
            fields,
        }
    }

    fn element() -> ObjectPath<'static> {
        ObjectPath::from_static_str_unchecked("/org/a11y/atspi/accessible/7")
    }

    fn root() -> ObjectPath<'static> {
        ObjectPath::from_static_str_unchecked(ROOT_PATH)
    }

    fn child(path: &str) -> Variant<'static> {
        let reference = Structure::builder()
            .add_field("")
            .add_field(ObjectPath::try_from(path.to_owned()).expect("a valid path"))
            .build()
            .expect("a reference");
        Variant::Variant(Box::new(Variant::Structure(reference)))
    }

    #[rstest]
    #[case::activation(EventKind::WindowActivate, "", 0, Some(Meaning::Activated))]
    #[case::gained_active(EventKind::StateChanged, "active", 1, Some(Meaning::Activated))]
    #[case::lost_active(EventKind::StateChanged, "active", 0, Some(Meaning::Deactivated))]
    #[case::deactivation(EventKind::WindowDeactivate, "", 0, Some(Meaning::Deactivated))]
    #[case::gained_focus(EventKind::StateChanged, "focused", 1, Some(Meaning::FocusMoved))]
    #[case::lost_focus(EventKind::StateChanged, "focused", 0, None)]
    #[case::window_created(EventKind::WindowCreate, "", 0, Some(Meaning::Opened))]
    #[case::loaded(EventKind::LoadComplete, "", 0, Some(Meaning::Changed(Cause::Loaded)))]
    #[case::edited(EventKind::TextChanged, "insert", 0, Some(Meaning::Changed(Cause::Edited)))]
    #[case::rearranged(EventKind::ChildrenChanged, "add", 0, Some(Meaning::Rearranged))]
    #[case::checked(EventKind::StateChanged, "checked", 1, Some(Meaning::Marked(Mark::Checked)))]
    #[case::unchecked(
        EventKind::StateChanged,
        "checked",
        0,
        Some(Meaning::Marked(Mark::Unchecked))
    )]
    #[case::shown(EventKind::StateChanged, "showing", 0, Some(Meaning::Shown))]
    #[case::ignored(EventKind::StateChanged, "selected", 1, None)]
    fn signals_mean_what_capture_needs(
        #[case] kind: EventKind,
        #[case] detail: &str,
        #[case] gained: i32,
        #[case] expected: Option<Meaning>,
    ) {
        let values = [Variant::from(detail), Variant::I32(gained), Variant::I32(0)];
        let path = element();
        assert_eq!(fields(kind, detail, &values, &path).meaning(), expected);
    }

    #[rstest]
    fn a_root_that_gains_a_child_gains_a_window() {
        let values = [Variant::from("add"), Variant::I32(0), Variant::I32(0), child("/w/1")];
        let path = root();
        let Some(Meaning::RootGained(window)) =
            fields(EventKind::ChildrenChanged, "add", &values, &path).meaning()
        else {
            panic!("no window");
        };
        assert_eq!(window.app().as_str(), ":1.42");
        assert_eq!(window.path(), "/w/1");
    }

    #[rstest]
    #[case::inside_a_window(element(), Some("/w/1"))]
    #[case::at_a_root(root(), None)]
    fn only_a_child_inside_a_window_can_be_an_added_dialog(
        #[case] path: ObjectPath<'static>,
        #[case] expected: Option<&str>,
    ) {
        let values = [Variant::from("add"), Variant::I32(0), Variant::I32(0), child("/w/1")];
        let signal = fields(EventKind::ChildrenChanged, "add", &values, &path);
        let added = (!signal.at_root()).then(|| signal.added_child()).flatten();
        assert_eq!(added.as_ref().map(Object::path), expected);
    }

    #[rstest]
    fn a_name_change_carries_the_title() {
        let title = Variant::Variant(Box::new(Variant::from("Report.odt")));
        let values = [Variant::from("accessible-name"), Variant::I32(0), Variant::I32(0), title];
        let path = element();
        assert_eq!(
            fields(EventKind::PropertyChange, "accessible-name", &values, &path).meaning(),
            Some(Meaning::Retitled("Report.odt".to_owned()))
        );
    }

    #[rstest]
    fn a_mark_counts_only_when_it_changes() {
        let mut marks = Marks::default();
        let source = Object::root(App::new(":1.42"));
        assert!(marks.changes(&source, Mark::Checked));
        assert!(!marks.changes(&source, Mark::Checked));
        assert!(marks.changes(&source, Mark::Unchecked));
    }

    /// Spells `event` the way that the registry does, from the fields of its match rule.
    fn registry_spelling(event: &ListenedEvent) -> String {
        let interface = event.interface.strip_prefix("org.a11y.atspi.Event.").unwrap_or_default();
        let detail: String = event
            .detail
            .unwrap_or_default()
            .split('-')
            .flat_map(|word| {
                let mut chars = word.chars();
                chars.next().map(|first| first.to_ascii_uppercase()).into_iter().chain(chars)
            })
            .collect();
        format!("{interface}:{}:{detail}", event.member)
    }

    #[rstest]
    fn each_listened_event_registers_what_its_rule_matches() {
        for event in &LISTENED_EVENTS {
            assert_eq!(event.registry, registry_spelling(event));
            assert!(event.match_rule().is_ok(), "{} builds no rule", event.registry);
        }
    }

    #[rstest]
    fn each_listened_event_has_a_kind_of_its_own() {
        for event in &LISTENED_EVENTS {
            let interface =
                event.interface.strip_prefix("org.a11y.atspi.Event.").unwrap_or_default();
            let kind = EventKind::of(interface, event.member);
            assert!(!kind.as_str().ends_with("other"), "{} has no kind", event.registry);
        }
    }
}

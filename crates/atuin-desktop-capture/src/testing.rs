//! A scripted backend for tests of code that drives a [`Capture`](crate::Capture).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use tokio::time::Instant;

use crate::backend::{
    AppIdentity, Backend, Completeness, Error, Event, EventCount, Limits, Loading, Node, Platform,
    ProcessApp, Purpose, ReadStats, Reading, Role, WindowClass, WindowKind,
};

/// A value that takes effect a number of milliseconds after the script starts.
#[derive(Debug, Clone, Copy)]
pub struct At<T> {
    pub ms: u64,
    pub value: T,
}

/// Makes `value` take effect `ms` milliseconds after the script starts.
pub const fn at<T>(ms: u64, value: T) -> At<T> {
    At { ms, value }
}

/// What a window shows: its text, and the URL and the title of its document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub window: &'static str,
    pub text: &'static str,
    pub url: Option<&'static str>,
    /// The window's title. `None` makes the title `title of <window>`.
    pub title: Option<&'static str>,
}

impl Page {
    /// A page of `window` with `text`, without a URL.
    #[must_use]
    pub const fn new(window: &'static str, text: &'static str) -> Self {
        Self {
            window,
            text,
            url: None,
            title: None,
        }
    }

    #[must_use]
    pub const fn with_url(self, url: &'static str) -> Self {
        Self {
            url: Some(url),
            ..self
        }
    }

    #[must_use]
    pub const fn with_title(self, title: &'static str) -> Self {
        Self {
            title: Some(title),
            ..self
        }
    }
}

/// The name that an application has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppName {
    pub app: &'static str,
    pub name: &'static str,
}

/// A backend that replays scripted events and serves one page per window.
///
/// A window is named `app/window`, and the part before the slash names its application.
pub struct Script {
    /// The moment from which the event times count.
    pub start: Instant,
    /// The events to replay, in the order of their times.
    pub events: VecDeque<At<Event<Self>>>,
    /// The page of each window that can be read, by window.
    pub pages: HashMap<&'static str, Page>,
    /// The kind that reads report for each window. Any other window is of the kind
    /// [`WindowKind::Other`].
    pub kinds: HashMap<&'static str, WindowKind>,
    /// The class of each window. Any other window is of the class [`WindowClass::Regular`].
    pub classes: HashMap<&'static str, WindowClass>,
    /// The pages that windows show from some time on. The latest one that took effect counts.
    pub later_pages: Vec<At<Page>>,
    /// The names that applications take from some time on, in place of their own. The
    /// latest one that took effect counts.
    pub later_names: Vec<At<AppName>>,
    /// How many reads still report a document that is loading.
    pub loading_reads: AtomicU32,
    /// When windows close, in milliseconds from the start. A closed window cannot be read.
    pub closes: HashMap<&'static str, u64>,
    /// The applications that the backend lists.
    pub apps: Vec<&'static str>,
    /// The window that is active when capture starts.
    pub active: Option<&'static str>,
    /// How many times each window was read.
    pub reads: parking_lot::Mutex<HashMap<&'static str, u32>>,
    /// How many times each window was classified.
    pub classifications: parking_lot::Mutex<HashMap<&'static str, u32>>,
    /// How many classifications of each window still fail without an answer.
    pub failing_classifications: HashMap<&'static str, AtomicU32>,
    /// The documents that each window holds without showing them.
    pub hidden: HashMap<&'static str, Vec<&'static str>>,
    /// The terminal that each window shows below its page, as a reader that kept the
    /// terminal's text would hand it over.
    pub terminals: HashMap<&'static str, &'static str>,
    /// How many reads still report a terminal, regardless of what [`kinds`](Self::kinds) says.
    pub terminal_reads: AtomicU32,
    /// How many reads still run out of time.
    pub slow_reads: AtomicU32,
    /// How many reads still fail without an answer.
    pub failing_reads: AtomicU32,
    /// The window that each application reports as active, when asked about the application.
    pub active_windows: HashMap<&'static str, &'static str>,
    /// The event counts that the next call to `take_event_counts` hands out.
    pub event_counts: Vec<EventCount<&'static str>>,
    /// The application that runs as each process.
    pub processes: HashMap<u32, &'static str>,
    /// What each app id stands for.
    pub app_ids: HashMap<&'static str, AppIdentity<&'static str>>,
    /// Whether the backend is paused. A test keeps a clone to watch the backend after the test
    /// hands the backend away. The backend starts paused, as a backend that is connected for
    /// capture does.
    pub paused: Arc<AtomicBool>,
    /// How long each read takes, regardless of its budget, as in an application that hangs.
    pub read_time: Duration,
    /// How many lookups of the application behind a process still fail.
    pub failing_process_lookups: AtomicU32,
}

impl Script {
    /// Replays `events` from now on, and serves `pages`.
    #[must_use]
    pub fn new<const E: usize, const P: usize>(
        events: [At<Event<Self>>; E],
        pages: [Page; P],
    ) -> Self {
        Self {
            start: Instant::now(),
            events: VecDeque::from(events),
            pages: pages.into_iter().map(|page| (page.window, page)).collect(),
            kinds: HashMap::new(),
            classes: HashMap::new(),
            later_pages: Vec::new(),
            later_names: Vec::new(),
            loading_reads: AtomicU32::new(0),
            closes: HashMap::new(),
            apps: Vec::new(),
            active: None,
            reads: parking_lot::Mutex::default(),
            classifications: parking_lot::Mutex::default(),
            failing_classifications: HashMap::new(),
            hidden: HashMap::new(),
            terminals: HashMap::new(),
            terminal_reads: AtomicU32::new(0),
            slow_reads: AtomicU32::new(0),
            failing_reads: AtomicU32::new(0),
            active_windows: HashMap::new(),
            event_counts: Vec::new(),
            processes: HashMap::new(),
            app_ids: HashMap::new(),
            paused: Arc::new(AtomicBool::new(true)),
            read_time: Duration::ZERO,
            failing_process_lookups: AtomicU32::new(0),
        }
    }

    /// The tree of `window`, whose page shows `text`.
    fn root(&self, window: &str, text: &str) -> Node {
        let page = Node {
            role: Role::Paragraph,
            text: Some(text.to_owned()),
            ..Node::default()
        };
        let Some(&terminal) = self.terminals.get(window) else {
            return page;
        };
        let row = Node {
            role: Role::ListItem,
            text: Some(terminal.to_owned()),
            ..Node::default()
        };
        let terminal = Node {
            role: Role::Terminal,
            name: terminal.to_owned(),
            text: Some(terminal.to_owned()),
            children: vec![row],
            ..Node::default()
        };
        Node {
            role: Role::Container,
            children: vec![page, terminal],
            ..Node::default()
        }
    }

    /// Whether the time `ms` after the start has come.
    fn has_come(&self, ms: u64) -> bool {
        self.start.elapsed() >= Duration::from_millis(ms)
    }

    /// The latest of `later` that took effect and that `wanted` accepts.
    fn latest<T: Copy>(&self, later: &[At<T>], wanted: impl Fn(&T) -> bool) -> Option<T> {
        later
            .iter()
            .filter(|later| self.has_come(later.ms) && wanted(&later.value))
            .max_by_key(|later| later.ms)
            .map(|later| later.value)
    }

    /// The page that `window` shows now.
    fn page(&self, window: &str) -> Page {
        self.latest(&self.later_pages, |page| page.window == window)
            .unwrap_or_else(|| self.pages[window])
    }
}

#[expect(clippy::unused_async_trait_impl, reason = "the trait methods are async")]
impl Backend for Script {
    type App = &'static str;
    type Window = &'static str;

    async fn connect(_: Purpose) -> Result<Self, Error> {
        Err(Error::Unsupported)
    }

    async fn wake_apps(&self) -> Option<Duration> {
        None
    }

    async fn apps(&self) -> Result<Vec<Self::App>, Error> {
        Ok(self.apps.clone())
    }

    async fn windows(&self, app: &Self::App) -> Result<Vec<Self::Window>, Error> {
        Ok(self.pages.keys().copied().filter(|window| self.app_of(window) == *app).collect())
    }

    fn app_of(&self, window: &Self::Window) -> Self::App {
        window.split_once('/').map_or(window, |(app, _)| app)
    }

    async fn app_of_process(&self, pid: u32) -> ProcessApp<Self::App> {
        if is_one_of(&self.failing_process_lookups) {
            return ProcessApp::Unknown;
        }
        self.processes.get(&pid).copied().map_or(ProcessApp::NoApp, ProcessApp::App)
    }

    async fn identify_app_id(&self, id: &str) -> AppIdentity<Self::App> {
        self.app_ids.get(id).cloned().unwrap_or_default()
    }

    /// The applications named `kitty` and `terminal` are terminals.
    fn is_terminal_name(name: &str) -> bool {
        ["kitty", "terminal"].iter().any(|terminal| terminal.eq_ignore_ascii_case(name))
    }

    /// The application named `alert` is left out.
    fn is_left_out(&self, app: &Self::App) -> bool {
        *app == "alert"
    }

    async fn app_name(&self, app: &Self::App) -> Result<String, Error> {
        let renamed = self.latest(&self.later_names, |name| name.app == *app);
        Ok(renamed.map_or(*app, |renamed| renamed.name).to_owned())
    }

    async fn active_window(&self) -> Option<Self::Window> {
        self.active
    }

    async fn active_window_of(&self, app: &Self::App) -> Option<Self::Window> {
        self.active_windows.get(app).copied()
    }

    async fn window_class(&self, window: &Self::Window) -> Result<WindowClass, Error> {
        *self.classifications.lock().entry(window).or_default() += 1;
        if self.failing_classifications.get(window).is_some_and(is_one_of) {
            return Err(Error::NoAnswer);
        }
        Ok(self.classes.get(window).copied().unwrap_or_default())
    }

    async fn top_level(&self, window: Self::Window) -> Self::Window {
        // An inner object is written as `window#part`.
        window.split_once('#').map_or(window, |(top, _)| top)
    }

    async fn window_title(&self, window: &Self::Window) -> Result<String, Error> {
        let title = self.pages.contains_key(window).then(|| self.page(window).title).flatten();
        Ok(title.map_or_else(|| format!("title of {window}"), str::to_owned))
    }

    async fn hidden_documents(&self, window: &Self::Window) -> Vec<Self::Window> {
        self.hidden.get(window).cloned().unwrap_or_default()
    }

    async fn read(&self, window: &Self::Window, _: Limits) -> Result<Reading, Error> {
        if self.closes.get(window).is_some_and(|&ms| self.has_come(ms)) {
            return Err(Error::NoAnswer);
        }
        *self.reads.lock().entry(window).or_default() += 1;
        if !self.read_time.is_zero() {
            tokio::time::sleep(self.read_time).await;
        }
        if is_one_of(&self.failing_reads) {
            return Err(Error::NoAnswer);
        }
        let page = self.page(window);
        let out_of_time = is_one_of(&self.slow_reads);
        let still_loading = is_one_of(&self.loading_reads);
        let kind = if is_one_of(&self.terminal_reads) {
            WindowKind::Terminal
        } else {
            self.kinds.get(window).copied().unwrap_or(WindowKind::Other)
        };
        Ok(Reading {
            kind,
            loading: if still_loading {
                Loading::InProgress
            } else {
                Loading::Finished
            },
            root: self.root(window, page.text),
            url: page.url.map(str::to_owned),
            stats: ReadStats {
                nodes: 1,
                completeness: if out_of_time {
                    Completeness::OutOfTime
                } else {
                    Completeness::Complete
                },
                unknown_roles: 0,
            },
        })
    }

    async fn next(&mut self) -> Option<Event<Self>> {
        let ms = self.events.front()?.ms;
        tokio::time::sleep_until(self.start + Duration::from_millis(ms)).await;
        self.events.pop_front().map(|event| event.value)
    }

    fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }

    fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }

    fn take_event_counts(&mut self) -> impl Iterator<Item = EventCount<Self::App>> + use<> {
        std::mem::take(&mut self.event_counts).into_iter()
    }
}
impl Platform for Script {}

/// Whether the current call is one of the `left` calls that remain, and counts `left` down.
fn is_one_of(left: &AtomicU32) -> bool {
    left.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| left.checked_sub(1)).is_ok()
}

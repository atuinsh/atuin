#![allow(missing_docs)]
use super::{
    event::MockEvent,
    field as mock_field,
    span::{MockSpan, NewSpan},
    Parent,
};
use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
};
use tracing::{
    level_filters::LevelFilter,
    span::{self, Attributes, Id},
    subscriber::Interest,
    Event, Metadata, Subscriber,
};

#[derive(Debug, Eq, PartialEq)]
enum Expect {
    Event(MockEvent),
    Enter(MockSpan),
    Exit(MockSpan),
    CloneSpan(MockSpan),
    DropSpan(MockSpan),
    Visit(MockSpan, mock_field::Expect),
    NewSpan(NewSpan),
    Nothing,
}

struct SpanState {
    name: &'static str,
    refs: usize,
}

struct Running<F: Fn(&Metadata<'_>) -> bool> {
    spans: Mutex<HashMap<Id, SpanState>>,
    expected: Arc<Mutex<VecDeque<Expect>>>,
    current: Mutex<Vec<Id>>,
    ids: AtomicUsize,
    max_level: Option<LevelFilter>,
    filter: F,
    name: String,
}

pub struct MockSubscriber<F: Fn(&Metadata<'_>) -> bool> {
    expected: VecDeque<Expect>,
    max_level: Option<LevelFilter>,
    filter: F,
    name: String,
}

pub struct MockHandle(Arc<Mutex<VecDeque<Expect>>>, String);

pub fn mock() -> MockSubscriber<fn(&Metadata<'_>) -> bool> {
    MockSubscriber {
        expected: VecDeque::new(),
        filter: (|_: &Metadata<'_>| true) as for<'r, 's> fn(&'r Metadata<'s>) -> _,
        max_level: None,
        name: thread::current()
            .name()
            .unwrap_or("mock_subscriber")
            .to_string(),
    }
}

impl<F> MockSubscriber<F>
where
    F: Fn(&Metadata<'_>) -> bool + 'static,
{
    /// Overrides the name printed by the mock subscriber's debugging output.
    ///
    /// The debugging output is displayed if the test panics, or if the test is
    /// run with `--nocapture`.
    ///
    /// By default, the mock subscriber's name is the  name of the test
    /// (*technically*, the name of the thread where it was created, which is
    /// the name of the test unless tests are run with `--test-threads=1`).
    /// When a test has only one mock subscriber, this is sufficient. However,
    /// some tests may include multiple subscribers, in order to test
    /// interactions between multiple subscribers. In that case, it can be
    /// helpful to give each subscriber a separate name to distinguish where the
    /// debugging output comes from.
    pub fn named(self, name: impl ToString) -> Self {
        Self {
            name: name.to_string(),
            ..self
        }
    }

    pub fn enter(mut self, span: MockSpan) -> Self {
        self.expected.push_back(Expect::Enter(span));
        self
    }

    pub fn event(mut self, event: MockEvent) -> Self {
        self.expected.push_back(Expect::Event(event));
        self
    }

    pub fn exit(mut self, span: MockSpan) -> Self {
        self.expected.push_back(Expect::Exit(span));
        self
    }

    pub fn clone_span(mut self, span: MockSpan) -> Self {
        self.expected.push_back(Expect::CloneSpan(span));
        self
    }

    #[allow(deprecated)]
    pub fn drop_span(mut self, span: MockSpan) -> Self {
        self.expected.push_back(Expect::DropSpan(span));
        self
    }

    pub fn done(mut self) -> Self {
        self.expected.push_back(Expect::Nothing);
        self
    }

    pub fn record<I>(mut self, span: MockSpan, fields: I) -> Self
    where
        I: Into<mock_field::Expect>,
    {
        self.expected.push_back(Expect::Visit(span, fields.into()));
        self
    }

    pub fn new_span<I>(mut self, new_span: I) -> Self
    where
        I: Into<NewSpan>,
    {
        self.expected.push_back(Expect::NewSpan(new_span.into()));
        self
    }

    pub fn with_filter<G>(self, filter: G) -> MockSubscriber<G>
    where
        G: Fn(&Metadata<'_>) -> bool + 'static,
    {
        MockSubscriber {
            expected: self.expected,
            filter,
            max_level: self.max_level,
            name: self.name,
        }
    }

    pub fn with_max_level_hint(self, hint: impl Into<LevelFilter>) -> Self {
        Self {
            max_level: Some(hint.into()),
            ..self
        }
    }

    pub fn run(self) -> impl Subscriber {
        let (subscriber, _) = self.run_with_handle();
        subscriber
    }

    pub fn run_with_handle(self) -> (impl Subscriber, MockHandle) {
        let expected = Arc::new(Mutex::new(self.expected));
        let handle = MockHandle(expected.clone(), self.name.clone());
        let subscriber = Running {
            spans: Mutex::new(HashMap::new()),
            expected,
            current: Mutex::new(Vec::new()),
            ids: AtomicUsize::new(1),
            filter: self.filter,
            max_level: self.max_level,
            name: self.name,
        };
        (subscriber, handle)
    }
}

impl<F> Subscriber for Running<F>
where
    F: Fn(&Metadata<'_>) -> bool + 'static,
{
    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        println!("[{}] enabled: {:#?}", self.name, meta);
        let enabled = (self.filter)(meta);
        println!("[{}] enabled -> {}", self.name, enabled);
        enabled
    }

    fn register_callsite(&self, meta: &'static Metadata<'static>) -> Interest {
        println!("[{}] register_callsite: {:#?}", self.name, meta);
        if self.enabled(meta) {
            Interest::always()
        } else {
            Interest::never()
        }
    }
    fn max_level_hint(&self) -> Option<LevelFilter> {
        self.max_level
    }

    fn record(&self, id: &Id, values: &span::Record<'_>) {
        let spans = self.spans.lock().unwrap();
        let mut expected = self.expected.lock().unwrap();
        let span = spans
            .get(id)
            .unwrap_or_else(|| panic!("[{}] no span for ID {:?}", self.name, id));
        println!(
            "[{}] record: {}; id={:?}; values={:?};",
            self.name, span.name, id, values
        );
        let was_expected = matches!(expected.front(), Some(Expect::Visit(_, _)));
        if was_expected {
            if let Expect::Visit(expected_span, mut expected_values) = expected.pop_front().unwrap()
            {
                if let Some(name) = expected_span.name() {
                    assert_eq!(name, span.name);
                }
                let mut checker = expected_values.checker(format!("span {}: ", span.name));
                values.record(&mut checker);
                checker.finish();
            }
        }
    }

    fn event(&self, event: &Event<'_>) {
        let name = event.metadata().name();
        println!("[{}] event: {};", self.name, name);
        match self.expected.lock().unwrap().pop_front() {
            None => {}
            Some(Expect::Event(mut expected)) => {
                let spans = self.spans.lock().unwrap();
                expected.check(event);
                match expected.parent {
                    Some(Parent::ExplicitRoot) => {
                        assert!(
                            event.is_root(),
                            "[{}] expected {:?} to be an explicit root event",
                            self.name,
                            name
                        );
                    }
                    Some(Parent::Explicit(expected_parent)) => {
                        let actual_parent =
                            event.parent().and_then(|id| spans.get(id)).map(|s| s.name);
                        assert_eq!(
                            Some(expected_parent.as_ref()),
                            actual_parent,
                            "[{}] expected {:?} to have explicit parent {:?}",
                            self.name,
                            name,
                            expected_parent,
                        );
                    }
                    Some(Parent::ContextualRoot) => {
                        assert!(
                            event.is_contextual(),
                            "[{}] expected {:?} to have a contextual parent",
                            self.name,
                            name
                        );
                        assert!(
                            self.current.lock().unwrap().last().is_none(),
                            "[{}] expected {:?} to be a root, but we were inside a span",
                            self.name,
                            name
                        );
                    }
                    Some(Parent::Contextual(expected_parent)) => {
                        assert!(
                            event.is_contextual(),
                            "[{}] expected {:?} to have a contextual parent",
                            self.name,
                            name
                        );
                        let stack = self.current.lock().unwrap();
                        let actual_parent =
                            stack.last().and_then(|id| spans.get(id)).map(|s| s.name);
                        assert_eq!(
                            Some(expected_parent.as_ref()),
                            actual_parent,
                            "[{}] expected {:?} to have contextual parent {:?}",
                            self.name,
                            name,
                            expected_parent,
                        );
                    }
                    None => {}
                }
            }
            Some(ex) => ex.bad(
                &self.name,
                format_args!("[{}] observed event {:?}", self.name, event),
            ),
        }
    }

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {
        // TODO: it should be possible to expect spans to follow from other spans
    }

    fn new_span(&self, span: &Attributes<'_>) -> Id {
        let meta = span.metadata();
        let id = self.ids.fetch_add(1, Ordering::SeqCst);
        let id = Id::from_u64(id as u64);
        println!(
            "[{}] new_span: name={:?}; target={:?}; id={:?};",
            self.name,
            meta.name(),
            meta.target(),
            id
        );
        let mut expected = self.expected.lock().unwrap();
        let was_expected = matches!(expected.front(), Some(Expect::NewSpan(_)));
        let mut spans = self.spans.lock().unwrap();
        if was_expected {
            if let Expect::NewSpan(mut expected) = expected.pop_front().unwrap() {
                let name = meta.name();
                expected
                    .span
                    .metadata
                    .check(meta, format_args!("span `{}`", name));
                let mut checker = expected.fields.checker(name.to_string());
                span.record(&mut checker);
                checker.finish();
                match expected.parent {
                    Some(Parent::ExplicitRoot) => {
                        assert!(
                            span.is_root(),
                            "[{}] expected {:?} to be an explicit root span",
                            self.name,
                            name
                        );
                    }
                    Some(Parent::Explicit(expected_parent)) => {
                        let actual_parent =
                            span.parent().and_then(|id| spans.get(id)).map(|s| s.name);
                        assert_eq!(
                            Some(expected_parent.as_ref()),
                            actual_parent,
                            "[{}] expected {:?} to have explicit parent {:?}",
                            self.name,
                            name,
                            expected_parent,
                        );
                    }
                    Some(Parent::ContextualRoot) => {
                        assert!(
                            span.is_contextual(),
                            "[{}] expected {:?} to have a contextual parent",
                            self.name,
                            name
                        );
                        assert!(
                            self.current.lock().unwrap().last().is_none(),
                            "[{}] expected {:?} to be a root, but we were inside a span",
                            self.name,
                            name
                        );
                    }
                    Some(Parent::Contextual(expected_parent)) => {
                        assert!(
                            span.is_contextual(),
                            "[{}] expected {:?} to have a contextual parent",
                            self.name,
                            name
                        );
                        let stack = self.current.lock().unwrap();
                        let actual_parent =
                            stack.last().and_then(|id| spans.get(id)).map(|s| s.name);
                        assert_eq!(
                            Some(expected_parent.as_ref()),
                            actual_parent,
                            "[{}] expected {:?} to have contextual parent {:?}",
                            self.name,
                            name,
                            expected_parent,
                        );
                    }
                    None => {}
                }
            }
        }
        spans.insert(
            id.clone(),
            SpanState {
                name: meta.name(),
                refs: 1,
            },
        );
        id
    }

    fn enter(&self, id: &Id) {
        let spans = self.spans.lock().unwrap();
        if let Some(span) = spans.get(id) {
            println!("[{}] enter: {}; id={:?};", self.name, span.name, id);
            match self.expected.lock().unwrap().pop_front() {
                None => {}
                Some(Expect::Enter(ref expected_span)) => {
                    if let Some(name) = expected_span.name() {
                        assert_eq!(name, span.name);
                    }
                }
                Some(ex) => ex.bad(&self.name, format_args!("entered span {:?}", span.name)),
            }
        };
        self.current.lock().unwrap().push(id.clone());
    }

    fn exit(&self, id: &Id) {
        if std::thread::panicking() {
            // `exit()` can be called in `drop` impls, so we must guard against
            // double panics.
            println!("[{}] exit {:?} while panicking", self.name, id);
            return;
        }
        let spans = self.spans.lock().unwrap();
        let span = spans
            .get(id)
            .unwrap_or_else(|| panic!("[{}] no span for ID {:?}", self.name, id));
        println!("[{}] exit: {}; id={:?};", self.name, span.name, id);
        match self.expected.lock().unwrap().pop_front() {
            None => {}
            Some(Expect::Exit(ref expected_span)) => {
                if let Some(name) = expected_span.name() {
                    assert_eq!(name, span.name);
                }
                let curr = self.current.lock().unwrap().pop();
                assert_eq!(
                    Some(id),
                    curr.as_ref(),
                    "[{}] exited span {:?}, but the current span was {:?}",
                    self.name,
                    span.name,
                    curr.as_ref().and_then(|id| spans.get(id)).map(|s| s.name)
                );
            }
            Some(ex) => ex.bad(&self.name, format_args!("exited span {:?}", span.name)),
        };
    }

    fn clone_span(&self, id: &Id) -> Id {
        let name = self.spans.lock().unwrap().get_mut(id).map(|span| {
            let name = span.name;
            println!(
                "[{}] clone_span: {}; id={:?}; refs={:?};",
                self.name, name, id, span.refs
            );
            span.refs += 1;
            name
        });
        if name.is_none() {
            println!("[{}] clone_span: id={:?};", self.name, id);
        }
        let mut expected = self.expected.lock().unwrap();
        let was_expected = if let Some(Expect::CloneSpan(ref span)) = expected.front() {
            assert_eq!(
                name,
                span.name(),
                "[{}] expected to clone a span named {:?}",
                self.name,
                span.name()
            );
            true
        } else {
            false
        };
        if was_expected {
            expected.pop_front();
        }
        id.clone()
    }

    fn drop_span(&self, id: Id) {
        let mut is_event = false;
        let name = if let Ok(mut spans) = self.spans.try_lock() {
            spans.get_mut(&id).map(|span| {
                let name = span.name;
                if name.contains("event") {
                    is_event = true;
                }
                println!(
                    "[{}] drop_span: {}; id={:?}; refs={:?};",
                    self.name, name, id, span.refs
                );
                span.refs -= 1;
                name
            })
        } else {
            None
        };
        if name.is_none() {
            println!("[{}] drop_span: id={:?}", self.name, id);
        }
        if let Ok(mut expected) = self.expected.try_lock() {
            let was_expected = match expected.front() {
                Some(Expect::DropSpan(ref span)) => {
                    // Don't assert if this function was called while panicking,
                    // as failing the assertion can cause a double panic.
                    if !::std::thread::panicking() {
                        assert_eq!(name, span.name());
                    }
                    true
                }
                Some(Expect::Event(_)) => {
                    if !::std::thread::panicking() {
                        assert!(is_event, "[{}] expected an event", self.name);
                    }
                    true
                }
                _ => false,
            };
            if was_expected {
                expected.pop_front();
            }
        }
    }
}

impl MockHandle {
    pub fn assert_finished(&self) {
        if let Ok(ref expected) = self.0.lock() {
            assert!(
                !expected.iter().any(|thing| thing != &Expect::Nothing),
                "[{}] more notifications expected: {:?}",
                self.1,
                **expected
            );
        }
    }
}

impl Expect {
    fn bad(&self, name: impl AsRef<str>, what: fmt::Arguments<'_>) {
        let name = name.as_ref();
        match self {
            Expect::Event(e) => panic!("[{}] expected event {}, but {} instead", name, e, what,),
            Expect::Enter(e) => panic!("[{}] expected to enter {} but {} instead", name, e, what,),
            Expect::Exit(e) => panic!("[{}] expected to exit {} but {} instead", name, e, what,),
            Expect::CloneSpan(e) => {
                panic!("[{}] expected to clone {} but {} instead", name, e, what,)
            }
            Expect::DropSpan(e) => {
                panic!("[{}] expected to drop {} but {} instead", name, e, what,)
            }
            Expect::Visit(e, fields) => panic!(
                "[{}] expected {} to record {} but {} instead",
                name, e, fields, what,
            ),
            Expect::NewSpan(e) => panic!("[{}] expected {} but {} instead", name, e, what),
            Expect::Nothing => panic!(
                "[{}] expected nothing else to happen, but {} instead",
                name, what,
            ),
        }
    }
}

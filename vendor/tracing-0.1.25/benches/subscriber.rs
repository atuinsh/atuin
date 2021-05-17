extern crate criterion;
extern crate tracing;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use tracing::Level;

use std::{
    fmt,
    sync::{Mutex, MutexGuard},
};
use tracing::{field, span, Event, Id, Metadata};

/// A subscriber that is enabled but otherwise does nothing.
struct EnabledSubscriber;

impl tracing::Subscriber for EnabledSubscriber {
    fn new_span(&self, span: &span::Attributes<'_>) -> Id {
        let _ = span;
        Id::from_u64(0xDEAD_FACE)
    }

    fn event(&self, event: &Event<'_>) {
        let _ = event;
    }

    fn record(&self, span: &Id, values: &span::Record<'_>) {
        let _ = (span, values);
    }

    fn record_follows_from(&self, span: &Id, follows: &Id) {
        let _ = (span, follows);
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        let _ = metadata;
        true
    }

    fn enter(&self, span: &Id) {
        let _ = span;
    }

    fn exit(&self, span: &Id) {
        let _ = span;
    }
}

/// Simulates a subscriber that records span data.
struct VisitingSubscriber(Mutex<String>);

struct Visitor<'a>(MutexGuard<'a, String>);

impl<'a> field::Visit for Visitor<'a> {
    fn record_debug(&mut self, _field: &field::Field, value: &dyn fmt::Debug) {
        use std::fmt::Write;
        let _ = write!(&mut *self.0, "{:?}", value);
    }
}

impl tracing::Subscriber for VisitingSubscriber {
    fn new_span(&self, span: &span::Attributes<'_>) -> Id {
        let mut visitor = Visitor(self.0.lock().unwrap());
        span.record(&mut visitor);
        Id::from_u64(0xDEAD_FACE)
    }

    fn record(&self, _span: &Id, values: &span::Record<'_>) {
        let mut visitor = Visitor(self.0.lock().unwrap());
        values.record(&mut visitor);
    }

    fn event(&self, event: &Event<'_>) {
        let mut visitor = Visitor(self.0.lock().unwrap());
        event.record(&mut visitor);
    }

    fn record_follows_from(&self, span: &Id, follows: &Id) {
        let _ = (span, follows);
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        let _ = metadata;
        true
    }

    fn enter(&self, span: &Id) {
        let _ = span;
    }

    fn exit(&self, span: &Id) {
        let _ = span;
    }
}

const N_SPANS: usize = 100;

fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("span_no_fields", |b| {
        tracing::subscriber::with_default(EnabledSubscriber, || {
            b.iter(|| span!(Level::TRACE, "span"))
        });
    });

    c.bench_function("enter_span", |b| {
        tracing::subscriber::with_default(EnabledSubscriber, || {
            let span = span!(Level::TRACE, "span");
            #[allow(clippy::unit_arg)]
            b.iter(|| black_box(span.in_scope(|| {})))
        });
    });

    c.bench_function("span_repeatedly", |b| {
        #[inline]
        fn mk_span(i: u64) -> tracing::Span {
            span!(Level::TRACE, "span", i = i)
        }

        let n = black_box(N_SPANS);
        tracing::subscriber::with_default(EnabledSubscriber, || {
            b.iter(|| (0..n).fold(mk_span(0), |_, i| mk_span(i as u64)))
        });
    });

    c.bench_function("span_with_fields", |b| {
        tracing::subscriber::with_default(EnabledSubscriber, || {
            b.iter(|| {
                span!(
                    Level::TRACE,
                    "span",
                    foo = "foo",
                    bar = "bar",
                    baz = 3,
                    quuux = tracing::field::debug(0.99)
                )
            })
        });
    });

    c.bench_function("span_with_fields_record", |b| {
        let subscriber = VisitingSubscriber(Mutex::new(String::from("")));
        tracing::subscriber::with_default(subscriber, || {
            b.iter(|| {
                span!(
                    Level::TRACE,
                    "span",
                    foo = "foo",
                    bar = "bar",
                    baz = 3,
                    quuux = tracing::field::debug(0.99)
                )
            })
        });
    });
}

fn bench_dispatch(c: &mut Criterion) {
    let mut group = c.benchmark_group("dispatch");
    group.bench_function("no_dispatch_get_ref", |b| {
        b.iter(|| {
            tracing::dispatcher::get_default(|current| {
                black_box(&current);
            })
        })
    });
    group.bench_function("no_dispatch_get_clone", |b| {
        b.iter(|| {
            let current = tracing::dispatcher::get_default(|current| current.clone());
            black_box(current);
        })
    });
    group.bench_function("get_ref", |b| {
        tracing::subscriber::with_default(EnabledSubscriber, || {
            b.iter(|| {
                tracing::dispatcher::get_default(|current| {
                    black_box(&current);
                })
            })
        })
    });
    group.bench_function("get_clone", |b| {
        tracing::subscriber::with_default(EnabledSubscriber, || {
            b.iter(|| {
                let current = tracing::dispatcher::get_default(|current| current.clone());
                black_box(current);
            })
        })
    });
    group.finish();
}

criterion_group!(benches, criterion_benchmark, bench_dispatch);
criterion_main!(benches);
